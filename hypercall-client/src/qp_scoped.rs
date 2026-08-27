//! Scoped Quote Provider client runtime.
//!
//! A typed outbound layer over the `/ws/quotes` protocol that replaces the
//! raw `mpsc::Receiver<ClientOutbound>` pipe of [`crate::qp_client`] for
//! indicative publishing:
//!
//! - **Per-scope conflating slots**: [`QpScopedHandle::set_indicative_quotes`]
//!   stores the latest snapshot per [`ScopeId`]; a fast-repricing scope
//!   supersedes only itself and can never starve or discard another scope's
//!   pending update.
//! - **Reconnect replay**: the server evicts a wallet's quotes when its last
//!   session disconnects, so on every (re)connect the runtime marks all
//!   slots dirty and republishes them. A quiet scope therefore reappears
//!   without waiting for its next natural update.
//! - **Capability fallback**: scoped frames are sent only when the server
//!   advertised [`CAP_SCOPED_INDICATIVE`] during auth. Against an older
//!   server, dirty slots are flushed as one legacy full-wallet
//!   `IndicativeQuoteUpdate` carrying the union of every slot.
//! - **TTL keepalive**: every live slot is republished on
//!   [`QpClientConfig::indicative_republish_interval`] even without
//!   changes, so the server's TTL sweep never evicts a quiet-but-live
//!   scope.
//! - **RFQ priority lane**: [`QpScopedHandle::send_rfq_response`] carries a
//!   deadline; responses are written before any pending indicative flush
//!   and are dropped (with a callback) once expired rather than sent stale.
//! - **Connection state**: [`QpScopedHandle::connection_state`] exposes a
//!   `watch` channel with the negotiated capabilities, so callers can gate
//!   RFQ handling on connectivity instead of inferring it from callbacks.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::{mpsc, watch, Notify};
use tokio::time::{sleep, timeout, Instant};
use tokio_tungstenite::tungstenite::Message;
use tracing::{info, warn};

use crate::qp_client::{
    connect_and_auth, forward_inbound_or_close, ClientOutbound, IndicativeQuote, QpClientCallbacks,
    QpClientConfig, QpDisconnectReason, QpWriteFailure, QpWriteOperation, ServerInbound,
    WS_WRITE_TIMEOUT,
};
use hypercall_ws_protocol::{ScopeId, CAP_SCOPED_INDICATIVE};

/// Connection state surfaced through [`QpScopedHandle::connection_state`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// Not connected; the runtime is between attempts.
    Disconnected,
    /// Authenticated. `capabilities` is the server's advertised list;
    /// [`ConnectionState::scoped_indicative`] reports the one this runtime
    /// acts on.
    Connected { capabilities: Vec<String> },
}

impl ConnectionState {
    pub fn scoped_indicative(&self) -> bool {
        match self {
            Self::Connected { capabilities } => {
                capabilities.iter().any(|c| c == CAP_SCOPED_INDICATIVE)
            }
            Self::Disconnected => false,
        }
    }
}

/// An RFQ response with the latest instant at which sending it is useful.
struct TimedRfqResponse {
    message: ClientOutbound,
    deadline: Instant,
}

/// An indicative frame selected for writing. Scoped frames share the slot
/// through the `Arc` so serialization never deep-clones the batch (5 heap
/// `String`s per leg at quote rate); the union fallback owns its one-off
/// merged batch by construction.
#[derive(Debug)]
enum OutboundFrame {
    Scoped {
        scope: ScopeId,
        quotes: Arc<Vec<IndicativeQuote>>,
    },
    Union(Vec<IndicativeQuote>),
}

/// Borrow-serializing shadow of the two indicative [`ClientOutbound`]
/// variants. MUST stay wire-identical to the owned enum's serde shape
/// (same tag field, same snake_case names); the
/// `outbound_frame_wire_shape_matches_client_outbound` test pins this.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OutboundFrameWire<'a> {
    ScopedIndicativeQuoteUpdate {
        scope: ScopeId,
        quotes: &'a [IndicativeQuote],
    },
    IndicativeQuoteUpdate {
        quotes: &'a [IndicativeQuote],
    },
}

impl OutboundFrame {
    fn to_json(&self) -> Result<String, serde_json::Error> {
        let wire = match self {
            Self::Scoped { scope, quotes } => OutboundFrameWire::ScopedIndicativeQuoteUpdate {
                scope: *scope,
                quotes,
            },
            Self::Union(quotes) => OutboundFrameWire::IndicativeQuoteUpdate { quotes },
        };
        serde_json::to_string(&wire)
    }
}

/// One item selected for the socket writer: RFQ responses always outrank
/// indicative frames.
enum OutboundItem {
    Rfq(ClientOutbound),
    Frame(OutboundFrame),
}

/// Latest-per-scope outbound snapshots plus dirty tracking. Pure state so
/// the conflation/fallback/replay rules are unit-testable without a socket.
#[derive(Default)]
struct ScopedOutboundState {
    slots: HashMap<ScopeId, Arc<Vec<IndicativeQuote>>>,
    dirty: VecDeque<ScopeId>,
    dirty_set: HashSet<ScopeId>,
    superseded: u64,
}

impl ScopedOutboundState {
    /// Store the latest snapshot for `scope` and mark it dirty. An empty
    /// snapshot is meaningful ("clear this scope" on the wire) but removes
    /// the slot afterwards so dead scopes are not replayed forever.
    fn set(&mut self, scope: ScopeId, quotes: Vec<IndicativeQuote>) {
        if self.slots.insert(scope, Arc::new(quotes)).is_some() && self.dirty_set.contains(&scope) {
            self.superseded += 1;
        }
        if self.dirty_set.insert(scope) {
            self.dirty.push_back(scope);
        }
    }

    /// Mark every slot dirty: the (re)connected server has none of our
    /// state (disconnect evicts the wallet), so everything must republish.
    fn mark_all_dirty(&mut self) {
        for scope in self.slots.keys() {
            if self.dirty_set.insert(*scope) {
                self.dirty.push_back(*scope);
            }
        }
    }

    /// Next frame to write, honoring the server's capability. Returns
    /// `None` when nothing is dirty. An empty ("clear this scope") slot is
    /// KEPT until [`Self::finish_scoped_clear`] confirms its write: if the
    /// write fails, reconnect replay re-marks the surviving slot and the
    /// clear is re-sent instead of silently lost.
    fn next_frame(&mut self, scoped_supported: bool) -> Option<OutboundFrame> {
        if scoped_supported {
            let scope = loop {
                let scope = self.dirty.pop_front()?;
                if self.dirty_set.remove(&scope) {
                    break scope;
                }
            };
            // Every dirty marker is paired with a slot insertion; a miss
            // here is a state-machine bug, and per repo policy broken
            // invariants crash loud rather than silently dropping frames.
            let quotes = self
                .slots
                .get(&scope)
                .expect("dirty scope must have a slot")
                .clone();
            return Some(OutboundFrame::Scoped { scope, quotes });
        }
        // Legacy fallback: one full-wallet snapshot carrying the union of
        // every slot. All dirtiness is discharged by a single frame.
        // Empty slots are KEPT until [`Self::finish_union_flush`] confirms
        // the write: if the cleared scope was the wallet's only slot and
        // the write failed, nothing else would ever go dirty again and
        // the clear would be silently lost.
        if self.dirty.is_empty() {
            return None;
        }
        self.dirty.clear();
        self.dirty_set.clear();
        let mut quotes = Vec::new();
        for slot in self.slots.values() {
            quotes.extend(slot.iter().cloned());
        }
        Some(OutboundFrame::Union(quotes))
    }

    /// Confirm a written "clear this scope" frame: drop the empty slot so
    /// the cleared scope stops replaying. A no-op if the caller re-set the
    /// scope (dirty again, or the slot is no longer empty) between the
    /// frame's selection and the write completing. "Confirmed" means the
    /// transport accepted the write; a frame lost after buffering is
    /// repaired by server-side eviction (full disconnect) or its TTL.
    fn finish_scoped_clear(&mut self, scope: ScopeId) {
        if !self.dirty_set.contains(&scope)
            && self.slots.get(&scope).is_some_and(|slot| slot.is_empty())
        {
            self.slots.remove(&scope);
        }
    }

    /// Confirm a written union frame: drop slots that are still empty and
    /// not re-dirtied, so cleared scopes stop replaying once their
    /// omission reached the wire.
    fn finish_union_flush(&mut self) {
        let Self {
            slots, dirty_set, ..
        } = self;
        slots.retain(|scope, slot| !slot.is_empty() || dirty_set.contains(scope));
    }

    fn take_superseded(&mut self) -> u64 {
        std::mem::take(&mut self.superseded)
    }
}

/// Shared state between the handle and the runtime task.
struct SharedScopedState {
    outbound: Mutex<ScopedOutboundState>,
    outbound_notify: Notify,
    connection: watch::Sender<ConnectionState>,
}

/// Caller-facing handle for the scoped QP runtime.
#[derive(Clone)]
pub struct QpScopedHandle {
    shared: Arc<SharedScopedState>,
    rfq_tx: mpsc::Sender<TimedRfqResponse>,
}

/// Error returned when an RFQ response cannot be queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RfqSendError {
    /// The bounded priority lane is full; the response was NOT queued.
    LaneFull,
    /// The runtime task has exited.
    RuntimeClosed,
}

impl std::fmt::Display for RfqSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LaneFull => f.write_str("RFQ priority lane full"),
            Self::RuntimeClosed => f.write_str("scoped QP runtime closed"),
        }
    }
}

impl std::error::Error for RfqSendError {}

impl QpScopedHandle {
    /// Publish the latest indicative snapshot for one scope. Never blocks;
    /// an unsent previous snapshot for the same scope is superseded.
    pub fn set_indicative_quotes(&self, scope: ScopeId, quotes: Vec<IndicativeQuote>) {
        self.shared
            .outbound
            .lock()
            .expect("scoped outbound lock poisoned")
            .set(scope, quotes);
        self.shared.outbound_notify.notify_one();
    }

    /// Queue a firm RFQ response with a delivery deadline. Responses are
    /// written ahead of any pending indicative flush; once `deadline`
    /// passes an unsent response is dropped and reported via
    /// [`QpClientCallbacks::on_stale_messages_drained`] rather than sent
    /// stale to the venue.
    pub fn send_rfq_response(
        &self,
        message: ClientOutbound,
        deadline: Instant,
    ) -> Result<(), RfqSendError> {
        match self.rfq_tx.try_send(TimedRfqResponse { message, deadline }) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => Err(RfqSendError::LaneFull),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(RfqSendError::RuntimeClosed),
        }
    }

    /// Watch the connection state (including negotiated capabilities).
    pub fn connection_state(&self) -> watch::Receiver<ConnectionState> {
        self.shared.connection.subscribe()
    }
}

/// Capacity of the RFQ priority lane. RFQ rate is venue-throttled; the
/// bound exists so a dead connection cannot buffer unbounded signed quotes.
const RFQ_LANE_CAPACITY: usize = 64;

/// Build the handle/runtime pair. Spawn [`run_qp_scoped_client`] with the
/// returned runtime half; keep the handle for publishing.
pub fn qp_scoped_channel() -> (QpScopedHandle, QpScopedRuntime) {
    let shared = Arc::new(SharedScopedState {
        outbound: Mutex::new(ScopedOutboundState::default()),
        outbound_notify: Notify::new(),
        connection: watch::channel(ConnectionState::Disconnected).0,
    });
    let (rfq_tx, rfq_rx) = mpsc::channel(RFQ_LANE_CAPACITY);
    (
        QpScopedHandle {
            shared: shared.clone(),
            rfq_tx,
        },
        QpScopedRuntime { shared, rfq_rx },
    )
}

/// Runtime half of [`qp_scoped_channel`], consumed by
/// [`run_qp_scoped_client`].
pub struct QpScopedRuntime {
    shared: Arc<SharedScopedState>,
    rfq_rx: mpsc::Receiver<TimedRfqResponse>,
}

/// Resolve the next outbound item: any live (unexpired) RFQ response
/// outranks any indicative frame; expired responses are dropped and
/// reported. Pends until something is writable.
///
/// Cancellation-safe as a `select!` arm: a message is only consumed at
/// points from which this function returns synchronously (no `.await`
/// between consumption and return), so dropping the future mid-pend never
/// loses a live RFQ response. State is also re-examined from scratch on
/// every call, so a lost notify permit can only delay via the next wakeup,
/// never skip work.
async fn next_outbound(
    rfq_rx: &mut mpsc::Receiver<TimedRfqResponse>,
    shared: &SharedScopedState,
    scoped_supported: bool,
    callbacks: &dyn QpClientCallbacks,
) -> OutboundItem {
    let mut rfq_open = true;
    loop {
        if rfq_open {
            loop {
                match rfq_rx.try_recv() {
                    Ok(timed) if Instant::now() <= timed.deadline => {
                        return OutboundItem::Rfq(timed.message);
                    }
                    Ok(_expired) => callbacks.on_stale_messages_drained(1),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        rfq_open = false;
                        break;
                    }
                }
            }
        }
        // The user-implemented callback runs OUTSIDE the outbound lock: a
        // callback that touches the handle (same mutex) must not deadlock,
        // and a slow one must not block publishers.
        let (superseded, frame) = {
            let mut outbound = shared
                .outbound
                .lock()
                .expect("scoped outbound lock poisoned");
            (
                outbound.take_superseded(),
                outbound.next_frame(scoped_supported),
            )
        };
        if superseded > 0 {
            callbacks.on_stale_messages_drained(superseded);
        }
        if let Some(frame) = frame {
            return OutboundItem::Frame(frame);
        }
        if rfq_open {
            tokio::select! {
                _ = shared.outbound_notify.notified() => {}
                maybe = rfq_rx.recv() => match maybe {
                    Some(timed) if Instant::now() <= timed.deadline => {
                        return OutboundItem::Rfq(timed.message);
                    }
                    Some(_expired) => callbacks.on_stale_messages_drained(1),
                    None => rfq_open = false,
                },
            }
        } else {
            shared.outbound_notify.notified().await;
        }
    }
}

/// Run the scoped QP client with auto-reconnect. Mirrors
/// [`crate::qp_client::run_qp_client`] but drives outbound from the scoped
/// handle instead of a raw message channel.
pub async fn run_qp_scoped_client(
    config: QpClientConfig,
    wallet: Arc<crate::wallet::HypercallWallet>,
    mut runtime: QpScopedRuntime,
    inbound_tx: mpsc::Sender<ServerInbound>,
    callbacks: Arc<dyn QpClientCallbacks>,
) {
    loop {
        let result = connect_and_run_scoped(
            &config,
            &wallet,
            &mut runtime,
            &inbound_tx,
            callbacks.as_ref(),
        )
        .await;
        // Publish Disconnected BEFORE the disconnect callbacks so a
        // callback consulting connection_state() never observes a
        // contradictory Connected. `send_replace`, never `send`:
        // `watch::Sender::send` refuses to STORE the value when no
        // receiver currently exists, so a caller subscribing later would
        // read a stale state forever.
        runtime
            .shared
            .connection
            .send_replace(ConnectionState::Disconnected);
        match result {
            Ok(()) => {
                warn!(
                    "Scoped QP WebSocket closed, reconnecting in {:?}",
                    config.reconnect_delay
                );
                callbacks.on_disconnected(QpDisconnectReason::Closed);
            }
            Err(e) if e == crate::qp_client::INBOUND_CHANNEL_CLOSED => {
                callbacks.on_disconnected(QpDisconnectReason::InboundChannelClosed);
                break;
            }
            Err(e) => {
                warn!(
                    "Scoped QP WebSocket error: {e}, reconnecting in {:?}",
                    config.reconnect_delay
                );
                callbacks.on_disconnected(QpDisconnectReason::Error);
            }
        }
        // Deliberately NOT draining the scoped slots: they are the replay
        // source for the next connection. Only expired RFQ responses are
        // dropped, in the outbound selection itself.
        sleep(config.reconnect_delay).await;
    }
}

async fn connect_and_run_scoped(
    config: &QpClientConfig,
    wallet: &crate::wallet::HypercallWallet,
    runtime: &mut QpScopedRuntime,
    inbound_tx: &mpsc::Sender<ServerInbound>,
    callbacks: &dyn QpClientCallbacks,
) -> Result<(), String> {
    let (mut ws_sender, mut ws_receiver, capabilities) =
        connect_and_auth(&config.api_url, wallet, callbacks).await?;
    let scoped_supported = capabilities.iter().any(|c| c == CAP_SCOPED_INDICATIVE);
    if !scoped_supported {
        info!("Server did not advertise {CAP_SCOPED_INDICATIVE}; using full-snapshot fallback");
    }
    // `send_replace` stores the value even with zero receivers, so a
    // caller that subscribes after connect still reads Connected.
    runtime
        .shared
        .connection
        .send_replace(ConnectionState::Connected { capabilities });

    // Reconnect replay: the server holds nothing for this wallet after a
    // full disconnect, so everything we know must be republished.
    runtime
        .shared
        .outbound
        .lock()
        .expect("scoped outbound lock poisoned")
        .mark_all_dirty();
    runtime.shared.outbound_notify.notify_one();

    let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
    // Keepalive republish: the server TTL-evicts rows whose snapshot has
    // not been re-received within its TTL, and this runtime only writes
    // when a slot is dirty, so a live scope whose prices simply stop
    // changing would silently vanish server-side. Re-marking every slot
    // dirty on an interval well inside the server TTL keeps quiet scopes
    // alive; conflation makes the republish cost one frame per scope. The
    // first tick is deferred a full period so it does not double-send the
    // reconnect replay above. Duration::ZERO disables the keepalive (a
    // zero interval period would panic); a daily tick stands in because
    // select! needs a live arm.
    let keepalive_period = if config.indicative_republish_interval.is_zero() {
        Duration::from_secs(24 * 60 * 60)
    } else {
        config.indicative_republish_interval
    };
    let mut keepalive =
        tokio::time::interval_at(Instant::now() + keepalive_period, keepalive_period);
    let mut last_activity = Instant::now();
    let heartbeat_timeout = Duration::from_secs(90);
    let QpScopedRuntime { shared, rfq_rx } = runtime;

    loop {
        // `biased` with reads first: outbound selection can always be
        // ready (a repricing flood keeps scopes dirty), and an
        // outbound-first ordering would starve inbound RfqRequests for the
        // duration of the flood -- worst at reconnect replay. Reads and
        // pings therefore outrank writes; RFQ-before-indicative priority
        // lives inside `next_outbound`.
        tokio::select! {
            biased;
            polled = ws_receiver.next() => {
                // Bind the raw poll result: a `Some(Ok(_))` pattern here
                // would DISABLE this arm on stream end/error and leave the
                // dead connection undetected until the heartbeat timeout.
                let frame = match polled {
                    Some(Ok(frame)) => frame,
                    Some(Err(e)) => return Err(format!("WebSocket read failed: {e}")),
                    None => return Ok(()),
                };
                last_activity = Instant::now();
                callbacks.on_transport_activity();
                match frame {
                    Message::Text(text) => match serde_json::from_str::<ServerInbound>(&text) {
                        Ok(msg) => forward_inbound_or_close(inbound_tx, msg, callbacks)?,
                        Err(e) => warn!("Failed to parse server message: {e}"),
                    },
                    Message::Binary(bytes) => {
                        if let Ok(text) = String::from_utf8(bytes.to_vec()) {
                            if let Ok(msg) = serde_json::from_str::<ServerInbound>(&text) {
                                forward_inbound_or_close(inbound_tx, msg, callbacks)?;
                            }
                        }
                    }
                    Message::Ping(data) => {
                        match timeout(WS_WRITE_TIMEOUT, ws_sender.send(Message::Pong(data))).await {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => callbacks
                                .on_write_failed(QpWriteOperation::Pong, QpWriteFailure::SendError),
                            Err(_) => callbacks
                                .on_write_failed(QpWriteOperation::Pong, QpWriteFailure::Timeout),
                        }
                        callbacks.on_transport_activity();
                    }
                    Message::Pong(_) => {}
                    Message::Close(_) => return Ok(()),
                    _ => {}
                }
            }
            _ = heartbeat.tick() => {
                if last_activity.elapsed() > heartbeat_timeout {
                    warn!(
                        "Scoped QP WebSocket heartbeat timeout ({}s with no activity), reconnecting",
                        last_activity.elapsed().as_secs()
                    );
                    return Ok(());
                }
                match timeout(WS_WRITE_TIMEOUT, ws_sender.send(Message::Ping(vec![]))).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        callbacks.on_write_failed(QpWriteOperation::Heartbeat, QpWriteFailure::SendError);
                        return Err(format!("Send heartbeat failed: {e}"));
                    }
                    Err(_) => {
                        callbacks.on_write_failed(QpWriteOperation::Heartbeat, QpWriteFailure::Timeout);
                        return Err("Send heartbeat timed out".to_string());
                    }
                }
                callbacks.on_transport_activity();
            }
            _ = keepalive.tick() => {
                let mut outbound = shared
                    .outbound
                    .lock()
                    .expect("scoped outbound lock poisoned");
                outbound.mark_all_dirty();
                drop(outbound);
                shared.outbound_notify.notify_one();
            }
            item = next_outbound(rfq_rx, shared, scoped_supported, callbacks) => {
                let json = match &item {
                    OutboundItem::Rfq(msg) => {
                        serde_json::to_string(msg).map_err(|e| format!("JSON encode: {e}"))?
                    }
                    OutboundItem::Frame(frame) => {
                        frame.to_json().map_err(|e| format!("JSON encode: {e}"))?
                    }
                };
                match timeout(WS_WRITE_TIMEOUT, ws_sender.send(Message::Text(json))).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        callbacks.on_write_failed(QpWriteOperation::Outbound, QpWriteFailure::SendError);
                        // A consumed RFQ response has no replay path
                        // (unlike indicative slots): report the loss so
                        // the caller can react before the taker's
                        // deadline lapses.
                        if matches!(item, OutboundItem::Rfq(_)) {
                            callbacks.on_stale_messages_drained(1);
                        }
                        return Err(format!("Send outbound failed: {e}"));
                    }
                    Err(_) => {
                        callbacks.on_write_failed(QpWriteOperation::Outbound, QpWriteFailure::Timeout);
                        if matches!(item, OutboundItem::Rfq(_)) {
                            callbacks.on_stale_messages_drained(1);
                        }
                        return Err("Send outbound timed out".to_string());
                    }
                }
                // The write is confirmed: cleared slots may now retire.
                // Before this point they must survive so a failed write is
                // replayed on reconnect.
                match &item {
                    OutboundItem::Frame(OutboundFrame::Scoped { scope, quotes })
                        if quotes.is_empty() =>
                    {
                        shared
                            .outbound
                            .lock()
                            .expect("scoped outbound lock poisoned")
                            .finish_scoped_clear(*scope);
                    }
                    OutboundItem::Frame(OutboundFrame::Union(_)) => {
                        shared
                            .outbound
                            .lock()
                            .expect("scoped outbound lock poisoned")
                            .finish_union_flush();
                    }
                    _ => {}
                }
                callbacks.on_transport_activity();
            }
        }
    }
}

#[cfg(test)]
#[path = "qp_scoped_test.rs"]
mod tests;
