//! Quote Provider (QP) WebSocket client for the `/ws/quotes` endpoint.
//!
//! Connects to the Hypercall API server as a registered Quote Provider,
//! authenticates via EIP-712 signature, and runs a bidirectional message
//! loop with auto-reconnect.
//!
//! # Usage
//!
//! ```rust,no_run
//! use std::sync::Arc;
//! use tokio::sync::mpsc;
//! use hypercall_client::{HypercallWallet, qp_client};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let wallet = Arc::new(HypercallWallet::from_private_key("0x...", 999)?);
//! let (outbound_tx, outbound_rx) = mpsc::channel(256);
//! let (inbound_tx, mut inbound_rx) = mpsc::channel(256);
//!
//! // Spawn the auto-reconnecting QP client
//! let config = qp_client::QpClientConfig::new("https://api.hypercall.xyz".into());
//! tokio::spawn(qp_client::run_qp_client(
//!     config,
//!     wallet,
//!     outbound_rx,
//!     inbound_tx,
//!     Arc::new(qp_client::NoopCallbacks),
//! ));
//!
//! // Handle inbound RFQ requests
//! while let Some(msg) = inbound_rx.recv().await {
//!     match msg {
//!         qp_client::ServerInbound::RfqRequest { rfq_id, legs, .. } => {
//!             println!("RFQ {rfq_id}: {} legs", legs.len());
//!         }
//!         _ => {}
//!     }
//! }
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};

use crate::wallet::HypercallWallet;

pub(crate) const INBOUND_CHANNEL_CLOSED: &str = "Inbound channel closed";
pub(crate) const WS_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Write half of an established QP WebSocket.
pub(crate) type WsSink = futures::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;
/// Read half of an established QP WebSocket.
pub(crate) type WsStream = futures::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

pub use hypercall_ws_protocol::{
    IndicativeQuote, QpClientMessage as ClientOutbound, QpResponseLeg as ResponseLeg,
    QpRfqLeg as RfqLeg, QpServerMessage as ServerInbound,
};

/// High-level reason the QP client disconnected from the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QpDisconnectReason {
    Closed,
    Error,
    InboundChannelClosed,
}

impl QpDisconnectReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Error => "error",
            Self::InboundChannelClosed => "inbound_closed",
        }
    }
}

impl std::fmt::Display for QpDisconnectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// WebSocket write operation that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QpWriteOperation {
    Auth,
    Heartbeat,
    Outbound,
    Pong,
}

impl QpWriteOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::Heartbeat => "heartbeat",
            Self::Outbound => "outbound",
            Self::Pong => "pong",
        }
    }
}

impl std::fmt::Display for QpWriteOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Reason a WebSocket write failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QpWriteFailure {
    Timeout,
    SendError,
}

impl QpWriteFailure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::SendError => "send_error",
        }
    }
}

impl std::fmt::Display for QpWriteFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Callbacks for observability hooks. Implement this to wire in your own
/// metrics framework (Prometheus, StatsD, etc.). The default [`NoopCallbacks`]
/// does nothing.
pub trait QpClientCallbacks: Send + Sync + 'static {
    fn on_connected(&self) {}
    fn on_disconnected(&self, _reason: QpDisconnectReason) {}
    fn on_stale_messages_drained(&self, _count: u64) {}
    fn on_inbound_message_dropped(&self) {}
    fn on_transport_activity(&self) {}
    fn on_write_failed(&self, _operation: QpWriteOperation, _failure: QpWriteFailure) {}
}

pub struct NoopCallbacks;
impl QpClientCallbacks for NoopCallbacks {}

/// Configuration for the QP WebSocket client.
pub struct QpClientConfig {
    pub api_url: String,
    pub reconnect_delay: Duration,
    /// How often the scoped runtime ([`crate::qp_scoped`]) republishes
    /// every live slot even without changes. The server TTL-evicts rows it
    /// has not re-received within its TTL (60s in production), so this
    /// must stay well inside that or quiet scopes silently vanish
    /// server-side. `Duration::ZERO` disables the keepalive entirely.
    /// Ignored by the legacy raw-channel client, whose callers send their
    /// own periodic snapshots.
    pub indicative_republish_interval: Duration,
}

impl QpClientConfig {
    pub fn new(api_url: String) -> Self {
        Self {
            api_url,
            reconnect_delay: Duration::from_secs(5),
            indicative_republish_interval: Duration::from_secs(15),
        }
    }
}

/// Run the QP WebSocket client with auto-reconnect.
///
/// - `outbound_rx`: messages to send to the server (indicative updates, RFQ responses)
/// - `inbound_tx`: received messages (RFQ requests) forwarded to the caller
/// - `callbacks`: observability hooks for metrics/logging
pub async fn run_qp_client(
    config: QpClientConfig,
    wallet: Arc<HypercallWallet>,
    mut outbound_rx: mpsc::Receiver<ClientOutbound>,
    inbound_tx: mpsc::Sender<ServerInbound>,
    callbacks: Arc<dyn QpClientCallbacks>,
) {
    loop {
        match connect_and_run(
            &config.api_url,
            &wallet,
            &mut outbound_rx,
            &inbound_tx,
            callbacks.as_ref(),
        )
        .await
        {
            Ok(_) => {
                warn!(
                    "QP WebSocket closed, reconnecting in {:?}",
                    config.reconnect_delay
                );
                callbacks.on_disconnected(QpDisconnectReason::Closed);
            }
            Err(e) => {
                if e == INBOUND_CHANNEL_CLOSED {
                    error!("QP inbound channel closed; stopping client task");
                    callbacks.on_disconnected(QpDisconnectReason::InboundChannelClosed);
                    break;
                }
                error!(
                    "QP WebSocket error: {}, reconnecting in {:?}",
                    e, config.reconnect_delay
                );
                callbacks.on_disconnected(QpDisconnectReason::Error);
            }
        }
        let mut drained = 0u64;
        while outbound_rx.try_recv().is_ok() {
            drained += 1;
        }
        if drained > 0 {
            info!(
                "Drained {} stale outbound messages before reconnect",
                drained
            );
            callbacks.on_stale_messages_drained(drained);
        }
        sleep(config.reconnect_delay).await;
    }
}

pub(crate) fn forward_inbound_or_close(
    inbound_tx: &mpsc::Sender<ServerInbound>,
    msg: ServerInbound,
    callbacks: &dyn QpClientCallbacks,
) -> Result<(), String> {
    match inbound_tx.try_send(msg) {
        Ok(()) => Ok(()),
        Err(TrySendError::Closed(_)) => Err(INBOUND_CHANNEL_CLOSED.to_string()),
        Err(TrySendError::Full(msg)) => {
            warn!(
                message = ?msg,
                "Inbound RFQ channel full, dropping server message to keep WebSocket writer responsive"
            );
            callbacks.on_inbound_message_dropped();
            Ok(())
        }
    }
}

/// Connect, authenticate, and return the socket halves plus the server's
/// advertised capabilities. Shared by the legacy raw-channel client and the
/// scoped runtime in [`crate::qp_scoped`].
pub(crate) async fn connect_and_auth(
    api_url: &str,
    wallet: &HypercallWallet,
    callbacks: &dyn QpClientCallbacks,
) -> Result<(WsSink, WsStream, Vec<String>), String> {
    let ws_url = if api_url.starts_with("https") {
        format!("{}/ws/quotes", api_url.replacen("https", "wss", 1))
    } else {
        format!("{}/ws/quotes", api_url.replacen("http", "ws", 1))
    };

    info!("Connecting to {}", ws_url);
    let (ws_stream, _) = connect_async(&ws_url)
        .await
        .map_err(|e| format!("WebSocket connect failed: {e}"))?;
    info!("WebSocket connected");
    callbacks.on_connected();
    callbacks.on_transport_activity();

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    // Authenticate with EIP-712 signature
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let nonce = wallet.next_nonce();
    let signature = wallet
        .sign_connect_quote_provider(alloy::primitives::U256::from(now_ms), nonce)
        .await
        .map_err(|e| format!("Failed to sign auth: {e}"))?;

    let auth_msg = ClientOutbound::ConnectQuoteProvider {
        wallet: wallet.address.as_hex(),
        timestamp: now_ms.to_string(),
        nonce,
        signature,
    };
    let json = serde_json::to_string(&auth_msg).map_err(|e| format!("JSON encode: {e}"))?;
    match timeout(WS_WRITE_TIMEOUT, ws_sender.send(Message::Text(json))).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            callbacks.on_write_failed(QpWriteOperation::Auth, QpWriteFailure::SendError);
            return Err(format!("Send auth failed: {e}"));
        }
        Err(_) => {
            callbacks.on_write_failed(QpWriteOperation::Auth, QpWriteFailure::Timeout);
            return Err("Send auth timed out".to_string());
        }
    }

    // Wait for auth response
    let auth_response = tokio::time::timeout(Duration::from_secs(10), ws_receiver.next())
        .await
        .map_err(|_| "Auth response timeout".to_string())?
        .ok_or_else(|| "Connection closed before auth".to_string())?
        .map_err(|e| format!("Auth frame error: {e}"))?;
    callbacks.on_transport_activity();

    let auth_text = match auth_response {
        Message::Text(t) => t,
        Message::Binary(b) => {
            String::from_utf8(b.to_vec()).map_err(|_| "Invalid binary frame".to_string())?
        }
        Message::Ping(data) => {
            match timeout(WS_WRITE_TIMEOUT, ws_sender.send(Message::Pong(data))).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    callbacks.on_write_failed(QpWriteOperation::Pong, QpWriteFailure::SendError);
                }
                Err(_) => {
                    callbacks.on_write_failed(QpWriteOperation::Pong, QpWriteFailure::Timeout);
                }
            }
            callbacks.on_transport_activity();
            return Err("Unexpected ping during auth".into());
        }
        Message::Pong(_) => return Err("Unexpected pong during auth".into()),
        _ => return Err("Unexpected frame type during auth".into()),
    };

    let server_msg: ServerInbound =
        serde_json::from_str(&auth_text).map_err(|e| format!("Auth parse: {e}"))?;
    let capabilities = match server_msg {
        ServerInbound::Authenticated {
            wallet: w,
            capabilities,
        } => {
            info!("Authenticated as QP: {}", w);
            capabilities
        }
        ServerInbound::AuthFailed { reason } => {
            return Err(format!("Authentication failed: {}", reason));
        }
        other => return Err(format!("Unexpected message during auth: {:?}", other)),
    };

    Ok((ws_sender, ws_receiver, capabilities))
}

async fn connect_and_run(
    api_url: &str,
    wallet: &HypercallWallet,
    outbound_rx: &mut mpsc::Receiver<ClientOutbound>,
    inbound_tx: &mpsc::Sender<ServerInbound>,
    callbacks: &dyn QpClientCallbacks,
) -> Result<(), String> {
    let (mut ws_sender, mut ws_receiver, _capabilities) =
        connect_and_auth(api_url, wallet, callbacks).await?;

    // Message loop with heartbeat timeout
    let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
    let mut last_activity = tokio::time::Instant::now();
    let heartbeat_timeout = Duration::from_secs(90);

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if last_activity.elapsed() > heartbeat_timeout {
                    warn!("QP WebSocket heartbeat timeout ({}s with no activity), reconnecting",
                          last_activity.elapsed().as_secs());
                    break;
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
            Some(msg) = outbound_rx.recv() => {
                let json = serde_json::to_string(&msg).map_err(|e| format!("JSON encode: {e}"))?;
                match timeout(WS_WRITE_TIMEOUT, ws_sender.send(Message::Text(json))).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        callbacks.on_write_failed(QpWriteOperation::Outbound, QpWriteFailure::SendError);
                        return Err(format!("Send outbound failed: {e}"));
                    }
                    Err(_) => {
                        callbacks.on_write_failed(QpWriteOperation::Outbound, QpWriteFailure::Timeout);
                        return Err("Send outbound timed out".to_string());
                    }
                }
                callbacks.on_transport_activity();
            }
            Some(Ok(frame)) = ws_receiver.next() => {
                last_activity = tokio::time::Instant::now();
                callbacks.on_transport_activity();
                match frame {
                    Message::Text(text) => {
                        match serde_json::from_str::<ServerInbound>(&text) {
                            Ok(msg) => {
                                forward_inbound_or_close(inbound_tx, msg, callbacks)?;
                            }
                            Err(e) => {
                                warn!("Failed to parse server message: {}", e);
                            }
                        }
                    }
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
                            Ok(Err(_)) => {
                                callbacks.on_write_failed(QpWriteOperation::Pong, QpWriteFailure::SendError);
                            }
                            Err(_) => {
                                callbacks.on_write_failed(QpWriteOperation::Pong, QpWriteFailure::Timeout);
                            }
                        }
                        callbacks.on_transport_activity();
                    }
                    Message::Pong(_) => {}
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            else => break,
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    const TEST_PRIVATE_KEY: &str =
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    #[derive(Default)]
    struct RecordingCallbacks {
        connected: AtomicUsize,
        transport_activity: AtomicUsize,
    }

    impl QpClientCallbacks for RecordingCallbacks {
        fn on_connected(&self) {
            self.connected.fetch_add(1, Ordering::Relaxed);
        }

        fn on_transport_activity(&self) {
            self.transport_activity.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[tokio::test]
    async fn connect_and_run_reports_transport_activity_for_ping_pong() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(stream).await.unwrap();

            let auth = ws.next().await.unwrap().unwrap();
            assert!(matches!(auth, Message::Text(_)));

            let authenticated = ServerInbound::Authenticated {
                wallet: "0x0000000000000000000000000000000000000000".to_string(),
                capabilities: Vec::new(),
            };
            ws.send(Message::Text(
                serde_json::to_string(&authenticated).unwrap(),
            ))
            .await
            .unwrap();

            ws.send(Message::Ping(vec![1, 2, 3])).await.unwrap();

            while let Some(frame) = ws.next().await {
                match frame.unwrap() {
                    Message::Ping(data) => {
                        ws.send(Message::Pong(data)).await.unwrap();
                    }
                    Message::Pong(data) if data == vec![1, 2, 3] => {
                        ws.close(None).await.unwrap();
                        break;
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        });

        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let (_outbound_tx, mut outbound_rx) = mpsc::channel(8);
        let (inbound_tx, _inbound_rx) = mpsc::channel(8);
        let callbacks = RecordingCallbacks::default();

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            connect_and_run(
                &format!("http://{}", addr),
                &wallet,
                &mut outbound_rx,
                &inbound_tx,
                &callbacks,
            ),
        )
        .await
        .expect("QP client test timed out");

        result.unwrap();
        server.await.unwrap();

        assert_eq!(callbacks.connected.load(Ordering::Relaxed), 1);
        assert!(
            callbacks.transport_activity.load(Ordering::Relaxed) >= 3,
            "expected connect, auth, and ping/pong activity callbacks"
        );
    }
}
