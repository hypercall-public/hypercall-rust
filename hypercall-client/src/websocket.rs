//! WebSocket client for real-time market data and account-scoped updates.
#![expect(
    clippy::result_large_err,
    reason = "websocket methods preserve the established unboxed public ClientError contract"
)]
//!
//! Connects to the Hypercall WS endpoint (`/ws`) for streaming:
//! - **Public**: orderbook, trades, index prices, options chain
//! - **Account-scoped**: order updates, fills, portfolio, position changes
//!
//! # Example
//!
//! ```rust,no_run
//! use hypercall_client::WsClient;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let ws = WsClient::new();
//! ws.connect("https://api.hypercall.xyz", None).await?;
//!
//! // Subscribe to fills (options + perps, distinguished by instrument_type)
//! ws.subscribe(vec!["fills", "order_updates"]).await?;
//!
//! // Wait for a message
//! let msg = ws.wait_for_message(|_| true, 5000).await;
//! println!("{:?}", msg);
//! # Ok(())
//! # }
//! ```
//!
//! # Channels
//!
//! | Channel | Auth | Description |
//! |---------|------|-------------|
//! | `orderbook` | No | L2 orderbook updates (filterable by symbols) |
//! | `trades` | No | Public trade events |
//! | `index_prices` | No | Spot/index prices for all underlyings |
//! | `indicative_market_data` | No | Aggregated quote-provider prices, one row per frame |
//! | `indicative_market_data_v2` | No | The same prices in batch frames, see [`WsClient::subscribe_indicative_market_data`] |
//! | `order_updates` | Yes | Order status changes (includes `instrument_type`) |
//! | `fills` | Yes | Fill notifications (includes `instrument_type`) |
//! | `portfolio` | Yes | Portfolio balance + position updates |
//!
//! # Compression
//!
//! The client offers RFC 7692 `permessage-deflate` on every connection. When the
//! server accepts, compressed frames are inflated transparently, with the same
//! size limit as uncompressed messages; when it does not, the connection runs
//! uncompressed with identical JSON messages. Opt out with
//! [`WsClient::with_permessage_deflate`].
//!
//! # Batched indicative market data
//!
//! [`WsClient::subscribe_indicative_market_data`] subscribes to
//! `indicative_market_data_v2`, which delivers `IndicativeMarketDataBatch`
//! frames, each carrying the rows of one short batch window. Servers that
//! predate that channel confirm any channel name, so the client checks the
//! confirmation: only `{"type":"Subscribed","channel":"indicative_market_data_v2","batch":true}`
//! means the server batches. Any other confirmation makes the client switch
//! that subscription to `indicative_market_data` (one row per frame) for the
//! rest of the client's life. [`crate::indicative`] decodes both. Opt out
//! with [`WsClient::with_batched_indicative`].

use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex as StdMutex,
};

use futures::{SinkExt, StreamExt};
use sonic_rs::JsonValueTrait;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch, Mutex, RwLock};
use tokio::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{client_async, tungstenite, WebSocketStream};
use tracing::{debug, info, warn};

use crate::error::{ClientError, Result};
#[cfg(feature = "rfq-v2")]
use crate::rfq_v2::encode_rfq_v2_read_grant;
use crate::wallet::AccountAddress;
#[cfg(feature = "rfq-v2")]
use hypercall_sdk_types::rfq_v2::RfqV2ReadGrant;
use hypercall_sdk_types::ws_protocol::WsMessage;
use hypercall_ws_protocol::{WsChannel, WsSlowConsumerCloseReason};

/// Original indicative channel: one `IndicativeMarketData` frame per row.
const INDICATIVE_CHANNEL: &str = WsChannel::IndicativeMarketData.as_str();
/// Batched indicative channel: `IndicativeMarketDataBatch` frames.
const INDICATIVE_V2_CHANNEL: &str = WsChannel::IndicativeMarketDataV2.as_str();

#[path = "websocket_deflate_io.rs"]
mod deflate_io;

use deflate_io::{InflatingStream, PERMESSAGE_DEFLATE_OFFER};

const MAX_CONTROL_FRAME_PAYLOAD_LEN: usize = 125;
const OUTBOUND_CONTROL_CAPACITY: usize = 256;
/// Tungstenite's default frame limit. Inflated messages are re-framed in
/// frames no larger than this.
const MAX_FRAME_LEN: usize = 16 << 20;
/// Tungstenite's default message limit, which also caps inflated messages.
const MAX_MESSAGE_LEN: usize = 64 << 20;

/// The byte stream under the WebSocket: plain TCP or native-tls over TCP.
trait ClientIo: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> ClientIo for T {}

type ClientSocket = WebSocketStream<InflatingStream<Box<dyn ClientIo>>>;

/// Reconnection and handshake policy for [`WsClient`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WsClientConfig {
    pub reconnect_initial_delay: Duration,
    pub reconnect_max_delay: Duration,
    pub reconnect_reset_after: Duration,
    pub authentication_timeout: Duration,
    pub subscription_timeout: Duration,
}

impl Default for WsClientConfig {
    fn default() -> Self {
        Self {
            reconnect_initial_delay: Duration::from_millis(500),
            reconnect_max_delay: Duration::from_secs(30),
            reconnect_reset_after: Duration::from_secs(30),
            authentication_timeout: Duration::from_secs(5),
            subscription_timeout: Duration::from_secs(5),
        }
    }
}

impl WsClientConfig {
    fn validate(self) -> Result<()> {
        if self.reconnect_initial_delay.is_zero() {
            return Err(ClientError::InvalidInput(
                "reconnect_initial_delay must be positive".to_string(),
            ));
        }
        if self.reconnect_max_delay < self.reconnect_initial_delay {
            return Err(ClientError::InvalidInput(
                "reconnect_max_delay must be at least reconnect_initial_delay".to_string(),
            ));
        }
        if self.reconnect_reset_after.is_zero() {
            return Err(ClientError::InvalidInput(
                "reconnect_reset_after must be positive".to_string(),
            ));
        }
        if self.authentication_timeout.is_zero() || self.subscription_timeout.is_zero() {
            return Err(ClientError::InvalidInput(
                "WebSocket handshake timeouts must be positive".to_string(),
            ));
        }
        Ok(())
    }
}

/// Application work required before state is continuous after a reconnect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WsRecoveryPlan {
    /// Fetch an authoritative snapshot before consuming ordered public deltas.
    pub snapshot_resubscribe: bool,
    /// Reconcile durable private events through REST.
    pub rest_reconcile: bool,
    /// Refetch the current portfolio snapshot.
    pub portfolio_refetch: bool,
    /// Reconcile an unrecognized channel according to application semantics.
    pub application_reconcile: bool,
}

impl WsRecoveryPlan {
    pub const fn is_empty(self) -> bool {
        !self.snapshot_resubscribe
            && !self.rest_reconcile
            && !self.portfolio_refetch
            && !self.application_reconcile
    }

    fn include_channel(&mut self, channel: &str) {
        match channel {
            "orderbook" | "trades" | "options_chain" => self.snapshot_resubscribe = true,
            "portfolio" => {
                self.portfolio_refetch = true;
                self.rest_reconcile = true;
            }
            "order_updates" | "fills" | "liquidation" | "rfq" => self.rest_reconcile = true,
            #[cfg(feature = "rfq-v2")]
            "rfq_v2" => self.rest_reconcile = true,
            "indicative_market_data"
            | "indicative_market_data_v2"
            | "index_prices"
            | "market_updates" => {}
            channel if channel.starts_with("candles") => {}
            _ => self.application_reconcile = true,
        }
    }
}

/// Why an established WebSocket session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsDisconnectReason {
    SlowConsumer(WsSlowConsumerCloseReason),
    CloseFrame { code: u16, reason: String },
    Transport(String),
    Protocol(String),
    EndOfStream,
}

/// Current lifecycle state of the client-managed WebSocket session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsConnectionState {
    Disconnected,
    Connecting { attempt: u32 },
    Authenticating,
    Resubscribing { channels: usize },
    Reconnecting { attempt: u32, delay: Duration },
    RecoveryRequired(WsRecoveryPlan),
    Ready,
}

impl WsConnectionState {
    pub const fn transport_connected(&self) -> bool {
        matches!(self, Self::Ready | Self::RecoveryRequired(_))
    }

    pub const fn state_recovered(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

#[derive(Debug, Clone)]
struct DesiredSubscription {
    channel: String,
    symbols: Option<Vec<String>>,
    expiry: Option<String>,
    option_type: Option<String>,
    /// Owned by [`WsClient::subscribe_indicative_market_data`], so it may live
    /// on either indicative channel after a fallback and is released by
    /// [`WsClient::unsubscribe_indicative_market_data`].
    logical_indicative: bool,
    /// Owned by a direct subscribe ([`WsClient::subscribe_with_options`] and
    /// friends) and released by [`WsClient::unsubscribe`]. A fallback can make
    /// one entry carry both claims; it is removed when neither remains.
    direct: bool,
    subscribe: String,
    unsubscribe: String,
}

fn canonical_symbols(symbols: Option<&[String]>) -> Option<std::collections::BTreeSet<String>> {
    let set: std::collections::BTreeSet<String> = symbols?
        .iter()
        .map(|symbol| symbol.trim().to_uppercase())
        .filter(|symbol| !symbol.is_empty())
        .collect();
    (!set.is_empty()).then_some(set)
}

impl DesiredSubscription {
    /// Take over the ownership claims of an equivalent entry it replaces.
    fn absorb(&mut self, other: &Self) {
        self.logical_indicative |= other.logical_indicative;
        self.direct |= other.direct;
    }

    /// Whether both entries describe the same channel and filter.
    ///
    /// Symbols compare the way the server stores them: trimmed, upper-cased,
    /// as a set, with an empty list meaning no filter.
    fn same_filter(&self, other: &Self) -> bool {
        self.channel == other.channel
            && canonical_symbols(self.symbols.as_deref())
                == canonical_symbols(other.symbols.as_deref())
            && self.expiry == other.expiry
            && self.option_type == other.option_type
    }

    fn new(
        channel: &str,
        symbols: Option<Vec<String>>,
        expiry: Option<String>,
        option_type: Option<String>,
    ) -> Result<Self> {
        let subscribe = sonic_rs::to_string(&WsMessage::Subscribe {
            channel: channel.to_string(),
            symbols: symbols.clone(),
            expiry: expiry.clone(),
            option_type: option_type.clone(),
        })?;
        let unsubscribe = sonic_rs::to_string(&WsMessage::Unsubscribe {
            channel: channel.to_string(),
            symbols: symbols.clone(),
            expiry: expiry.clone(),
            option_type: option_type.clone(),
        })?;
        Ok(Self {
            channel: channel.to_string(),
            symbols,
            expiry,
            option_type,
            logical_indicative: false,
            direct: false,
            subscribe,
            unsubscribe,
        })
    }
}

struct WsClientShared {
    messages: Arc<Mutex<Vec<sonic_rs::Value>>>,
    pongs: Arc<Mutex<Vec<Vec<u8>>>>,
    tx: StdMutex<Option<mpsc::Sender<tungstenite::Message>>>,
    subscriptions: RwLock<BTreeMap<String, DesiredSubscription>>,
    state: StdMutex<WsConnectionState>,
    state_tx: watch::Sender<WsConnectionState>,
    last_disconnect: StdMutex<Option<WsDisconnectReason>>,
    started: AtomicBool,
    /// Prefer `indicative_market_data_v2` for indicative subscriptions.
    batched_indicative: AtomicBool,
    /// Set once a server confirmed v2 without the batch marker: it does not
    /// know the channel. Sticky for the client's life.
    v2_unsupported: AtomicBool,
    /// A v2 confirmation without the marker arrived and the switch to v1 has
    /// not been applied yet.
    v2_fallback_pending: AtomicBool,
}

/// What [`WsClient::release_where`] does with one stored subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Release {
    Keep,
    Remove,
}

/// What the client offers in the upgrade request.
#[derive(Debug, Clone, Copy)]
struct ConnectOptions {
    permessage_deflate: bool,
}

/// WebSocket client for Hypercall.
pub struct WsClient {
    shared: Arc<WsClientShared>,
    config: WsClientConfig,
    options: ConnectOptions,
    shutdown_tx: StdMutex<Option<watch::Sender<bool>>>,
}

#[cfg(feature = "rfq-v2")]
struct RfqV2SocketGrant {
    token: String,
    account: String,
    expires_at_ms: u64,
}

/// The identity a session presents after every (re)connect.
struct SessionCredentials {
    wallet: Option<String>,
    #[cfg(feature = "rfq-v2")]
    rfq_v2_grant: Option<RfqV2SocketGrant>,
}

impl SessionCredentials {
    fn wallet(wallet: Option<String>) -> Self {
        Self {
            wallet,
            #[cfg(feature = "rfq-v2")]
            rfq_v2_grant: None,
        }
    }

    /// Wall-clock expiry of the session's RFQ V2 read grant, if it has one.
    fn rfq_v2_expires_at_ms(&self) -> Option<u64> {
        #[cfg(feature = "rfq-v2")]
        {
            self.rfq_v2_grant.as_ref().map(|grant| grant.expires_at_ms)
        }
        #[cfg(not(feature = "rfq-v2"))]
        {
            None
        }
    }
}

fn current_unix_ms() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
}

impl WsClient {
    /// Create a new WebSocket client (not yet connected).
    pub fn new() -> Self {
        Self::with_config(WsClientConfig::default())
    }

    /// Create a WebSocket client with an explicit reconnection policy.
    pub fn with_config(config: WsClientConfig) -> Self {
        let (state_tx, _) = watch::channel(WsConnectionState::Disconnected);
        Self {
            shared: Arc::new(WsClientShared {
                messages: Arc::new(Mutex::new(Vec::new())),
                pongs: Arc::new(Mutex::new(Vec::new())),
                tx: StdMutex::new(None),
                subscriptions: RwLock::new(BTreeMap::new()),
                state: StdMutex::new(WsConnectionState::Disconnected),
                state_tx,
                last_disconnect: StdMutex::new(None),
                started: AtomicBool::new(false),
                batched_indicative: AtomicBool::new(true),
                v2_unsupported: AtomicBool::new(false),
                v2_fallback_pending: AtomicBool::new(false),
            }),
            config,
            options: ConnectOptions {
                permessage_deflate: true,
            },
            shutdown_tx: StdMutex::new(None),
        }
    }

    /// Choose whether to offer RFC 7692 `permessage-deflate` when connecting.
    ///
    /// Enabled by default. The server decides whether to accept, and the client
    /// works either way: compressed frames are inflated transparently, and the
    /// JSON messages are identical. Disable it to save the CPU spent inflating,
    /// or to rule compression out while debugging. The setting applies to the
    /// initial connection and every reconnect.
    pub fn with_permessage_deflate(mut self, enabled: bool) -> Self {
        self.options.permessage_deflate = enabled;
        self
    }

    /// Choose whether [`WsClient::subscribe_indicative_market_data`] uses
    /// the batched `indicative_market_data_v2` channel.
    ///
    /// Enabled by default: fewer frames, rows up to one short batch window
    /// later, and much better compression. Disable it to receive one
    /// `IndicativeMarketData` frame per row from `indicative_market_data`.
    pub fn with_batched_indicative(self, enabled: bool) -> Self {
        self.shared
            .batched_indicative
            .store(enabled, Ordering::Release);
        self
    }

    /// Whether indicative subscriptions are currently delivered in batches:
    /// batching is enabled and no server has shown it lacks the v2 channel.
    pub fn indicative_batched(&self) -> bool {
        self.shared.batched_indicative.load(Ordering::Acquire)
            && !self.shared.v2_unsupported.load(Ordering::Acquire)
    }

    /// Connect to the WebSocket endpoint.
    ///
    /// If a wallet is provided, sends an `Authenticate` message after connecting
    /// and waits for the `Authenticated` confirmation before returning. After
    /// the initial connection succeeds, the client reconnects, reauthenticates,
    /// and restores desired subscriptions until [`WsClient::disconnect`] is called
    /// or the client is dropped.
    pub async fn connect(&self, base_url: &str, wallet: Option<&AccountAddress>) -> Result<()> {
        self.start_session(
            base_url,
            SessionCredentials::wallet(wallet.map(ToString::to_string)),
        )
        .await
    }

    /// Connect using a signed private RFQ read grant. The grant is sent again
    /// before subscriptions on reconnect. Once it expires, sign a fresh grant
    /// and create a new session; no unsigned wallet identity grants RFQ access.
    #[cfg(feature = "rfq-v2")]
    pub async fn connect_rfq_v2(&self, base_url: &str, grant: &RfqV2ReadGrant) -> Result<()> {
        self.start_session(
            base_url,
            SessionCredentials {
                wallet: None,
                rfq_v2_grant: Some(RfqV2SocketGrant {
                    token: encode_rfq_v2_read_grant(grant)?,
                    account: grant.account.to_string(),
                    expires_at_ms: grant.expires_at_ms,
                }),
            },
        )
        .await
    }

    async fn start_session(&self, base_url: &str, credentials: SessionCredentials) -> Result<()> {
        self.config.validate()?;
        let ws_url = websocket_url(base_url)?;
        if self
            .shared
            .started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ClientError::WebSocket(
                "WebSocket client is already connected or connecting".to_string(),
            ));
        }

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        *self
            .shutdown_tx
            .lock()
            .expect("WebSocket shutdown mutex poisoned") = Some(shutdown_tx);
        let (initial_tx, initial_rx) = oneshot::channel();
        let shared = self.shared.clone();
        let config = self.config;
        let options = self.options;
        tokio::spawn(async move {
            run_ws_session(
                shared,
                config,
                ws_url,
                options,
                credentials,
                shutdown_rx,
                initial_tx,
            )
            .await;
        });

        initial_rx.await.map_err(|_| {
            ClientError::WebSocket("WebSocket connection task stopped before startup".to_string())
        })?
    }

    fn validate_control_payload(payload: &[u8]) -> Result<()> {
        if payload.len() > MAX_CONTROL_FRAME_PAYLOAD_LEN {
            return Err(ClientError::WebSocket(format!(
                "WebSocket control frame payload exceeds {} bytes",
                MAX_CONTROL_FRAME_PAYLOAD_LEN
            )));
        }

        Ok(())
    }

    async fn send_control_frame(&self, msg: tungstenite::Message) -> Result<()> {
        let tx = self
            .shared
            .tx
            .lock()
            .expect("WebSocket sender mutex poisoned")
            .clone()
            .ok_or(ClientError::WebSocket("Not connected".to_string()))?;

        tx.send(msg)
            .await
            .map_err(|e| ClientError::WebSocket(format!("Failed to send: {}", e)))
    }

    /// Send an empty WebSocket ping frame.
    pub async fn ping(&self) -> Result<()> {
        self.ping_with_payload(Vec::new()).await
    }

    /// Send a WebSocket ping frame with a payload.
    ///
    /// WebSocket control frame payloads are limited to 125 bytes by the protocol.
    pub async fn ping_with_payload(&self, payload: impl Into<Vec<u8>>) -> Result<()> {
        let payload = payload.into();
        Self::validate_control_payload(&payload)?;
        self.send_control_frame(tungstenite::Message::Ping(payload))
            .await
    }

    /// Send a WebSocket pong frame with a payload.
    ///
    /// This is rarely needed directly because `WsClient` automatically responds
    /// to server ping frames.
    pub async fn pong_with_payload(&self, payload: impl Into<Vec<u8>>) -> Result<()> {
        let payload = payload.into();
        Self::validate_control_payload(&payload)?;
        self.send_control_frame(tungstenite::Message::Pong(payload))
            .await
    }

    /// Subscribe to channels.
    pub async fn subscribe(&self, channels: Vec<&str>) -> Result<()> {
        for channel in channels {
            self.subscribe_with_options(channel, None, None, None)
                .await?;
        }

        Ok(())
    }

    /// Send a subscription request for private RFQ V2 change hints. `Ok` means
    /// the frame was sent; observe `Subscribed` or `Error` for server acceptance.
    /// Use a periodic signed change-feed read to recover hints lost while a
    /// socket stays connected.
    #[cfg(feature = "rfq-v2")]
    pub async fn subscribe_rfq_v2(&self) -> Result<()> {
        self.subscribe_with_options("rfq_v2", None, None, None)
            .await
    }

    /// Remove the RFQ V2 subscription, including its reconnect intent.
    #[cfg(feature = "rfq-v2")]
    pub async fn unsubscribe_rfq_v2(&self) -> Result<()> {
        self.unsubscribe(vec!["rfq_v2"]).await
    }

    /// Subscribe to a channel and retain its filters across reconnects.
    pub async fn subscribe_with_options(
        &self,
        channel: &str,
        symbols: Option<Vec<String>>,
        expiry: Option<String>,
        option_type: Option<String>,
    ) -> Result<()> {
        self.subscribe_desired(channel, symbols, expiry, option_type)
            .await
    }

    /// Subscribe to indicative market data and retain the subscription
    /// across reconnects.
    ///
    /// By default this uses `indicative_market_data_v2`, whose
    /// `IndicativeMarketDataBatch` frames carry the rows of one short batch
    /// window. If the server confirms v2 without `"batch": true` (it predates
    /// the channel), the client switches to `indicative_market_data`, one
    /// `IndicativeMarketData` frame per row. Read rows with
    /// [`crate::indicative::indicative_rows_from_value`], which handles both.
    ///
    /// The server keeps one symbol filter per channel per connection: every
    /// `Subscribe` adds symbols to it, and a filtered subscribe narrows an
    /// unfiltered one. If you also subscribe to `indicative_market_data`
    /// directly with a different filter, a fallback onto v1 shares that one
    /// server filter with your direct subscription. Use one or the other per
    /// connection when exact filters matter.
    pub async fn subscribe_indicative_market_data(
        &self,
        symbols: Option<Vec<String>>,
    ) -> Result<()> {
        let channel = if self.indicative_batched() {
            INDICATIVE_V2_CHANNEL
        } else {
            INDICATIVE_CHANNEL
        };
        self.subscribe_desired_with(channel, symbols, None, None, true)
            .await
    }

    /// Stop the subscriptions made by
    /// [`WsClient::subscribe_indicative_market_data`], on whichever channel
    /// each resolved to after a fallback, and stop restoring them after
    /// reconnects. Subscriptions made with [`WsClient::subscribe_with_options`]
    /// on either indicative channel are left alone.
    pub async fn unsubscribe_indicative_market_data(&self) -> Result<()> {
        self.release_where(|subscription| {
            if !subscription.logical_indicative {
                return Release::Keep;
            }
            subscription.logical_indicative = false;
            if subscription.direct {
                Release::Keep
            } else {
                Release::Remove
            }
        })
        .await
    }

    async fn subscribe_desired(
        &self,
        channel: &str,
        symbols: Option<Vec<String>>,
        expiry: Option<String>,
        option_type: Option<String>,
    ) -> Result<()> {
        self.subscribe_desired_with(channel, symbols, expiry, option_type, false)
            .await
    }

    async fn subscribe_desired_with(
        &self,
        channel: &str,
        symbols: Option<Vec<String>>,
        expiry: Option<String>,
        option_type: Option<String>,
        logical_indicative: bool,
    ) -> Result<()> {
        if !self.shared.started.load(Ordering::Acquire) {
            return Err(ClientError::WebSocket("Not connected".to_string()));
        }

        let mut desired = DesiredSubscription::new(channel, symbols, expiry, option_type)?;
        desired.logical_indicative = logical_indicative;
        desired.direct = !logical_indicative;
        let frame = tungstenite::Message::Text(desired.subscribe.clone());
        {
            let mut subscriptions = self.shared.subscriptions.write().await;
            // One logical subscription is one channel plus its filter fields.
            // Replace any equivalent earlier entry so a reconnect replays it
            // (and its subscribe-time snapshot) once.
            // Keep any ownership claim the equivalent entry already had.
            for existing in subscriptions.values() {
                if desired.same_filter(existing) {
                    desired.absorb(existing);
                }
            }
            subscriptions.retain(|_, existing| !desired.same_filter(existing));
            subscriptions.insert(desired.subscribe.clone(), desired);
        }

        let tx = self
            .shared
            .tx
            .lock()
            .expect("WebSocket sender mutex poisoned")
            .clone();
        if let Some(tx) = tx {
            debug!("Subscribing to: {}", channel);
            tx.send(frame)
                .await
                .map_err(|e| ClientError::WebSocket(format!("Failed to send: {}", e)))?;
        }

        Ok(())
    }

    /// Unsubscribe from channels and stop restoring them after reconnects.
    ///
    /// Channels match exactly, and only subscriptions made directly (for
    /// example with [`WsClient::subscribe_with_options`]) are released. To
    /// stop a subscription made by
    /// [`WsClient::subscribe_indicative_market_data`], which may have moved
    /// from `indicative_market_data_v2` to `indicative_market_data`, use
    /// [`WsClient::unsubscribe_indicative_market_data`]. If both own the same
    /// channel and filter, the server subscription ends when both release it.
    pub async fn unsubscribe(&self, channels: Vec<&str>) -> Result<()> {
        for channel in channels {
            // Releases the direct claim; an entry the indicative helper also
            // owns stays subscribed until that claim is released too.
            self.release_where(|subscription| {
                if subscription.channel != channel {
                    return Release::Keep;
                }
                subscription.direct = false;
                if subscription.logical_indicative {
                    Release::Keep
                } else {
                    Release::Remove
                }
            })
            .await?;
        }
        Ok(())
    }

    /// Apply `release` to every stored subscription and unsubscribe the ones
    /// it says to remove.
    async fn release_where(
        &self,
        release: impl Fn(&mut DesiredSubscription) -> Release,
    ) -> Result<()> {
        if !self.shared.started.load(Ordering::Acquire) {
            return Err(ClientError::WebSocket("Not connected".to_string()));
        }

        let removed = {
            let mut subscriptions = self.shared.subscriptions.write().await;
            let keys = subscriptions
                .iter_mut()
                .filter_map(|(key, subscription)| {
                    (release(subscription) == Release::Remove).then(|| key.clone())
                })
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| subscriptions.remove(&key))
                .collect::<Vec<_>>()
        };
        for removed in removed {
            let tx = self
                .shared
                .tx
                .lock()
                .expect("WebSocket sender mutex poisoned")
                .clone();
            if let Some(tx) = tx {
                tx.send(tungstenite::Message::Text(removed.unsubscribe))
                    .await
                    .map_err(|e| ClientError::WebSocket(format!("Failed to send: {}", e)))?;
            }
        }
        Ok(())
    }

    /// Stop reconnecting and close the current socket.
    pub async fn disconnect(&self) {
        let _ = self
            .send_control_frame(tungstenite::Message::Close(None))
            .await;
        if let Some(shutdown_tx) = self
            .shutdown_tx
            .lock()
            .expect("WebSocket shutdown mutex poisoned")
            .take()
        {
            let _ = shutdown_tx.send(true);
        }
    }

    /// Return the current connection and recovery state.
    pub fn connection_state(&self) -> WsConnectionState {
        self.shared
            .state
            .lock()
            .expect("WebSocket state mutex poisoned")
            .clone()
    }

    /// Watch connection and recovery state transitions.
    pub fn connection_state_receiver(&self) -> watch::Receiver<WsConnectionState> {
        self.shared.state_tx.subscribe()
    }

    /// Return the most recent established-session disconnect reason.
    pub fn last_disconnect_reason(&self) -> Option<WsDisconnectReason> {
        self.shared
            .last_disconnect
            .lock()
            .expect("WebSocket disconnect mutex poisoned")
            .clone()
    }

    /// Mark application-owned snapshot or REST recovery as complete.
    pub fn mark_recovered(&self) -> Result<()> {
        let mut state = self
            .shared
            .state
            .lock()
            .expect("WebSocket state mutex poisoned");
        if !matches!(*state, WsConnectionState::RecoveryRequired(_)) {
            return Err(ClientError::WebSocket(
                "No WebSocket recovery is currently required".to_string(),
            ));
        }
        *state = WsConnectionState::Ready;
        self.shared.state_tx.send_replace(WsConnectionState::Ready);
        Ok(())
    }

    /// Get all received WebSocket pong payloads.
    pub async fn get_pongs(&self) -> Vec<Vec<u8>> {
        self.shared.pongs.lock().await.clone()
    }

    /// Clear all received WebSocket pong payloads.
    pub async fn clear_pongs(&self) {
        self.shared.pongs.lock().await.clear();
    }

    /// Wait for a pong payload matching a predicate.
    pub async fn wait_for_pong<F>(&self, check: F, timeout_ms: u64) -> Option<Vec<u8>>
    where
        F: Fn(&[u8]) -> bool,
    {
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(timeout_ms);

        while start.elapsed() < timeout {
            let pongs = self.get_pongs().await;
            if let Some(pong) = pongs.iter().find(|payload| check(payload)) {
                return Some(pong.clone());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        None
    }

    /// Get all received messages.
    pub async fn get_messages(&self) -> Vec<sonic_rs::Value> {
        self.shared.messages.lock().await.clone()
    }

    /// Clear all received messages.
    pub async fn clear_messages(&self) {
        self.shared.messages.lock().await.clear();
    }

    /// Wait for a message matching a predicate.
    pub async fn wait_for_message<F>(&self, check: F, timeout_ms: u64) -> Option<sonic_rs::Value>
    where
        F: Fn(&sonic_rs::Value) -> bool,
    {
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(timeout_ms);

        while start.elapsed() < timeout {
            let messages = self.get_messages().await;
            if let Some(msg) = messages.iter().find(|m| check(m)) {
                return Some(msg.clone());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        None
    }

    /// Count messages matching a predicate.
    pub async fn count_messages<F>(&self, check: F) -> usize
    where
        F: Fn(&sonic_rs::Value) -> bool,
    {
        self.get_messages()
            .await
            .iter()
            .filter(|msg| check(msg))
            .count()
    }
}

impl Drop for WsClient {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self
            .shutdown_tx
            .lock()
            .expect("WebSocket shutdown mutex poisoned")
            .take()
        {
            let _ = shutdown_tx.send(true);
        }
    }
}

#[derive(Debug, Clone)]
enum SessionStop {
    Shutdown,
    Disconnected(WsDisconnectReason),
    NonReconnectable(WsDisconnectReason),
}

impl SessionStop {
    fn websocket_error(&self, context: &str) -> ClientError {
        match self {
            Self::Shutdown => ClientError::WebSocket(format!("{}: client stopped", context)),
            Self::Disconnected(reason) | Self::NonReconnectable(reason) => {
                ClientError::WebSocket(format!("{}: {:?}", context, reason))
            }
        }
    }
}

fn websocket_url(base_url: &str) -> Result<String> {
    let base_url = base_url.trim_end_matches('/');
    let ws_origin = if let Some(rest) = base_url.strip_prefix("https://") {
        format!("wss://{}", rest)
    } else if let Some(rest) = base_url.strip_prefix("http://") {
        format!("ws://{}", rest)
    } else if base_url.starts_with("wss://") || base_url.starts_with("ws://") {
        base_url.to_string()
    } else {
        return Err(ClientError::InvalidInput(
            "WebSocket base URL must use http, https, ws, or wss".to_string(),
        ));
    };

    if ws_origin.ends_with("/ws") {
        Ok(ws_origin)
    } else {
        Ok(format!("{}/ws", ws_origin))
    }
}

/// Why opening a WebSocket connection failed, with the source kept until the
/// session renders it at the existing `ClientError::WebSocket` boundary.
#[derive(Debug, thiserror::Error)]
#[displaystr::display]
enum ConnectError {
    Url(#[source] tungstenite::Error) = "invalid WebSocket URL: {_0}",
    MissingHost = "WebSocket URL has no host",
    UnsupportedScheme = "WebSocket URL must use ws or wss",
    Tcp(#[source] std::io::Error) = "IO error: {_0}",
    Tls(#[source] tokio_native_tls::native_tls::Error) = "TLS error: {_0}",
    /// The upgrade handshake stage. Named rather than transparent because it
    /// adds a stage to the Tungstenite error; the text is unchanged.
    Handshake(#[source] tungstenite::Error) = "{_0}",
}

/// Opens TCP, then native-tls for `wss` (the platform trust store, as before),
/// then runs the Tungstenite handshake over [`InflatingStream`].
async fn connect_socket(
    ws_url: &str,
    options: ConnectOptions,
) -> std::result::Result<(ClientSocket, tungstenite::handshake::client::Response), ConnectError> {
    let permessage_deflate = options.permessage_deflate;
    let mut request = ws_url.into_client_request().map_err(ConnectError::Url)?;
    if permessage_deflate {
        request.headers_mut().insert(
            "Sec-WebSocket-Extensions",
            HeaderValue::from_static(PERMESSAGE_DEFLATE_OFFER),
        );
    }
    let uri = request.uri();
    let host = uri.host().ok_or(ConnectError::MissingHost)?.to_string();
    let tls = match uri.scheme_str() {
        Some("wss") => true,
        Some("ws") => false,
        _ => return Err(ConnectError::UnsupportedScheme),
    };
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let tcp = TcpStream::connect(format!("{host}:{port}"))
        .await
        .map_err(ConnectError::Tcp)?;

    let io: Box<dyn ClientIo> = if tls {
        let domain = host.trim_start_matches('[').trim_end_matches(']');
        let connector =
            tokio_native_tls::native_tls::TlsConnector::new().map_err(ConnectError::Tls)?;
        let stream = tokio_native_tls::TlsConnector::from(connector)
            .connect(domain, tcp)
            .await
            .map_err(ConnectError::Tls)?;
        Box::new(stream)
    } else {
        Box::new(tcp)
    };

    let stream = InflatingStream::new(io, permessage_deflate, MAX_FRAME_LEN, MAX_MESSAGE_LEN);
    client_async(request, stream)
        .await
        .map_err(ConnectError::Handshake)
}

fn reconnect_delay(config: WsClientConfig, attempt: u32) -> Duration {
    let multiplier = 1_u32 << attempt.min(31);
    let ceiling = config
        .reconnect_initial_delay
        .saturating_mul(multiplier)
        .min(config.reconnect_max_delay);
    ceiling.mul_f64(0.5 + rand::random::<f64>() * 0.5)
}

fn set_state(shared: &WsClientShared, next: WsConnectionState) {
    *shared.state.lock().expect("WebSocket state mutex poisoned") = next.clone();
    shared.state_tx.send_replace(next);
}

fn finish_session(shared: &WsClientShared) {
    *shared.tx.lock().expect("WebSocket sender mutex poisoned") = None;
    shared.started.store(false, Ordering::Release);
    set_state(shared, WsConnectionState::Disconnected);
}

fn record_disconnect(shared: &WsClientShared, reason: WsDisconnectReason) {
    *shared
        .last_disconnect
        .lock()
        .expect("WebSocket disconnect mutex poisoned") = Some(reason);
}

async fn record_text(shared: &WsClientShared, text: &str) -> Option<sonic_rs::Value> {
    debug!("WS received: {}", text);
    let json = sonic_rs::from_str::<sonic_rs::Value>(text).ok()?;
    if is_unbatched_v2_confirmation(&json) {
        // The server accepted the name but does not know the channel, so no
        // data would ever arrive on it. Switch to v1 at the next safe point.
        warn!("server lacks indicative_market_data_v2; falling back to indicative_market_data");
        shared.v2_unsupported.store(true, Ordering::Release);
        shared.v2_fallback_pending.store(true, Ordering::Release);
    }
    shared.messages.lock().await.push(json.clone());
    Some(json)
}

/// The server cannot deliver `indicative_market_data_v2`: it confirmed the
/// channel without `"batch": true` (servers that predate the channel confirm
/// any name), or it rejected the channel as unknown.
fn is_unbatched_v2_confirmation(json: &sonic_rs::Value) -> bool {
    let channel_is_v2 =
        json.get("channel").and_then(|value| value.as_str()) == Some(INDICATIVE_V2_CHANNEL);
    match json.get("type").and_then(|value| value.as_str()) {
        Some("Subscribed") => {
            channel_is_v2 && json.get("batch").and_then(|value| value.as_bool()) != Some(true)
        }
        Some("Error") => {
            channel_is_v2
                && json.get("code").and_then(|value| value.as_str())
                    == Some(hypercall_ws_protocol::UNKNOWN_CHANNEL_CODE)
        }
        _ => false,
    }
}

/// Move every v2 subscription made by
/// [`WsClient::subscribe_indicative_market_data`] to v1 with the same filter.
/// Subscriptions made directly on `indicative_market_data_v2` are the
/// caller's to manage and are left alone. Returns
/// the `Unsubscribe` frames for the dead v2 subscriptions and the new v1
/// entries, which the caller subscribes (and, during a reconnect, waits for).
async fn take_v2_fallback(
    shared: &WsClientShared,
) -> std::result::Result<(Vec<String>, BTreeMap<String, DesiredSubscription>), SessionStop> {
    let mut subscriptions = shared.subscriptions.write().await;
    take_v2_fallback_locked(shared, &mut subscriptions)
}

/// [`take_v2_fallback`] under a guard the caller already holds, so it can keep
/// holding it while the replacements are subscribed.
fn take_v2_fallback_locked(
    shared: &WsClientShared,
    subscriptions: &mut BTreeMap<String, DesiredSubscription>,
) -> std::result::Result<(Vec<String>, BTreeMap<String, DesiredSubscription>), SessionStop> {
    let mut unsubscribes = Vec::new();
    let mut replacements = BTreeMap::new();
    if !shared.v2_fallback_pending.swap(false, Ordering::AcqRel) {
        return Ok((unsubscribes, replacements));
    }
    let v2_keys: Vec<String> = subscriptions
        .iter()
        .filter(|(_, desired)| {
            desired.channel == INDICATIVE_V2_CHANNEL && desired.logical_indicative
        })
        .map(|(key, _)| key.clone())
        .collect();
    for key in v2_keys {
        let Some(mut v2) = subscriptions.remove(&key) else {
            continue;
        };
        if v2.direct {
            // A direct claim on the same v2 entry is the caller's to manage:
            // keep it (and the server subscription) and move only the
            // helper's claim to v1.
            v2.logical_indicative = false;
            subscriptions.insert(key, v2.clone());
        } else {
            unsubscribes.push(v2.unsubscribe.clone());
        }
        let mut v1 = DesiredSubscription::new(
            INDICATIVE_CHANNEL,
            v2.symbols.clone(),
            v2.expiry.clone(),
            v2.option_type.clone(),
        )
        .map_err(|error| {
            SessionStop::Disconnected(WsDisconnectReason::Protocol(error.to_string()))
        })?;
        v1.logical_indicative = true;
        // A direct v1 subscription with the same filter keeps its claim: the
        // merged entry stays until both owners release it.
        for existing in subscriptions.values() {
            if v1.same_filter(existing) {
                v1.absorb(existing);
            }
        }
        subscriptions.retain(|_, existing| !v1.same_filter(existing));
        subscriptions.insert(v1.subscribe.clone(), v1.clone());
        replacements.insert(v1.subscribe.clone(), v1);
    }
    Ok((unsubscribes, replacements))
}

/// Steady-state fallback: switch v2 subscriptions to v1 on the live socket.
/// Their confirmations are recorded like any other message.
async fn apply_v2_fallback<S>(
    socket: &mut S,
    shared: &WsClientShared,
) -> std::result::Result<(), SessionStop>
where
    S: futures::Sink<tungstenite::Message, Error = tungstenite::Error> + Unpin,
{
    let (unsubscribes, replacements) = take_v2_fallback(shared).await?;
    let frames = unsubscribes.into_iter().chain(
        replacements
            .into_values()
            .map(|replacement| replacement.subscribe),
    );
    for frame in frames {
        socket
            .send(tungstenite::Message::Text(frame))
            .await
            .map_err(|error| {
                SessionStop::Disconnected(WsDisconnectReason::Transport(error.to_string()))
            })?;
    }
    Ok(())
}

fn disconnect_reason_from_close(
    frame: Option<tungstenite::protocol::CloseFrame<'_>>,
) -> WsDisconnectReason {
    let Some(frame) = frame else {
        return WsDisconnectReason::EndOfStream;
    };
    let reason = frame.reason.into_owned();
    if let Ok(slow_consumer) = serde_json::from_str::<WsSlowConsumerCloseReason>(&reason) {
        return WsDisconnectReason::SlowConsumer(slow_consumer);
    }

    WsDisconnectReason::CloseFrame {
        code: frame.code.into(),
        reason,
    }
}

async fn authenticate_socket<S>(
    socket: &mut S,
    shared: &WsClientShared,
    wallet: &str,
    shutdown_rx: &mut watch::Receiver<bool>,
    timeout: Duration,
) -> std::result::Result<(), SessionStop>
where
    S: futures::Sink<tungstenite::Message, Error = tungstenite::Error>
        + futures::Stream<Item = std::result::Result<tungstenite::Message, tungstenite::Error>>
        + Unpin,
{
    let auth_msg = sonic_rs::to_string(&WsMessage::Authenticate {
        wallet: wallet.to_string(),
    })
    .map_err(|error| {
        SessionStop::Disconnected(WsDisconnectReason::Protocol(format!(
            "failed to serialize authentication: {}",
            error
        )))
    })?;
    socket
        .send(tungstenite::Message::Text(auth_msg))
        .await
        .map_err(|error| {
            SessionStop::Disconnected(WsDisconnectReason::Transport(error.to_string()))
        })?;

    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    return Err(SessionStop::Shutdown);
                }
            }
            _ = &mut deadline => {
                return Err(SessionStop::Disconnected(WsDisconnectReason::Protocol(
                    "timed out waiting for authentication response".to_string(),
                )));
            }
            incoming = socket.next() => {
                match incoming {
                    Some(Ok(tungstenite::Message::Text(text))) => {
                        let Some(json) = record_text(shared, &text).await else {
                            continue;
                        };
                        match json.get("type").and_then(|value| value.as_str()) {
                            Some("Authenticated") => {
                                let authenticated_wallet = json
                                    .get("wallet")
                                    .and_then(|value| value.as_str())
                                    .ok_or_else(|| SessionStop::Disconnected(
                                        WsDisconnectReason::Protocol(
                                            "authentication response omitted wallet".to_string(),
                                        )
                                    ))?;
                                if !authenticated_wallet.eq_ignore_ascii_case(wallet) {
                                    return Err(SessionStop::Disconnected(
                                        WsDisconnectReason::Protocol(format!(
                                            "authenticated wallet mismatch: expected {}, received {}",
                                            wallet, authenticated_wallet
                                        )),
                                    ));
                                }
                                return Ok(());
                            }
                            Some("Error") => {
                                let message = json
                                    .get("message")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("authentication failed");
                                return Err(SessionStop::Disconnected(
                                    WsDisconnectReason::Protocol(message.to_string()),
                                ));
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(tungstenite::Message::Ping(payload))) => {
                        socket.send(tungstenite::Message::Pong(payload)).await.map_err(|error| {
                            SessionStop::Disconnected(WsDisconnectReason::Transport(error.to_string()))
                        })?;
                    }
                    Some(Ok(tungstenite::Message::Pong(payload))) => {
                        shared.pongs.lock().await.push(payload);
                    }
                    Some(Ok(tungstenite::Message::Close(frame))) => {
                        return Err(SessionStop::Disconnected(disconnect_reason_from_close(frame)));
                    }
                    Some(Err(error)) => {
                        return Err(SessionStop::Disconnected(WsDisconnectReason::Transport(
                            error.to_string(),
                        )));
                    }
                    None => return Err(SessionStop::Disconnected(WsDisconnectReason::EndOfStream)),
                    _ => {}
                }
            }
        }
    }
}

#[cfg(feature = "rfq-v2")]
async fn authenticate_rfq_v2_socket<S>(
    socket: &mut S,
    shared: &WsClientShared,
    grant: &RfqV2SocketGrant,
    shutdown_rx: &mut watch::Receiver<bool>,
    timeout: Duration,
) -> std::result::Result<(), SessionStop>
where
    S: futures::Sink<tungstenite::Message, Error = tungstenite::Error>
        + futures::Stream<Item = std::result::Result<tungstenite::Message, tungstenite::Error>>
        + Unpin,
{
    let auth_msg = sonic_rs::to_string(&WsMessage::AuthenticateRfqV2 {
        token: grant.token.clone(),
    })
    .map_err(|error| {
        SessionStop::NonReconnectable(WsDisconnectReason::Protocol(format!(
            "failed to serialize RFQ authentication: {error}"
        )))
    })?;
    socket
        .send(tungstenite::Message::Text(auth_msg))
        .await
        .map_err(|error| {
            SessionStop::Disconnected(WsDisconnectReason::Transport(error.to_string()))
        })?;

    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    return Err(SessionStop::Shutdown);
                }
            }
            _ = &mut deadline => {
                return Err(SessionStop::Disconnected(WsDisconnectReason::Protocol(
                    "timed out waiting for RFQ authentication response".to_string(),
                )));
            }
            incoming = socket.next() => {
                match incoming {
                    Some(Ok(tungstenite::Message::Text(text))) => {
                        let Some(json) = record_text(shared, &text).await else {
                            continue;
                        };
                        match json.get("type").and_then(|value| value.as_str()) {
                            Some("RfqV2Authenticated") => {
                                let account = json
                                    .get("account")
                                    .and_then(|value| value.as_str())
                                    .ok_or_else(|| SessionStop::NonReconnectable(
                                        WsDisconnectReason::Protocol(
                                            "RFQ authentication response omitted account".to_string(),
                                        )
                                    ))?;
                                if !account.eq_ignore_ascii_case(&grant.account) {
                                    return Err(SessionStop::NonReconnectable(
                                        WsDisconnectReason::Protocol(
                                            "RFQ authenticated account does not match signed grant".to_string(),
                                        ),
                                    ));
                                }
                                return Ok(());
                            }
                            Some("Error") => {
                                let message = json
                                    .get("message")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("RFQ authentication failed");
                                return Err(SessionStop::NonReconnectable(
                                    WsDisconnectReason::Protocol(message.to_string()),
                                ));
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(tungstenite::Message::Ping(payload))) => {
                        socket.send(tungstenite::Message::Pong(payload)).await.map_err(|error| {
                            SessionStop::Disconnected(WsDisconnectReason::Transport(error.to_string()))
                        })?;
                    }
                    Some(Ok(tungstenite::Message::Pong(payload))) => {
                        shared.pongs.lock().await.push(payload);
                    }
                    Some(Ok(tungstenite::Message::Close(frame))) => {
                        return Err(SessionStop::Disconnected(disconnect_reason_from_close(frame)));
                    }
                    Some(Err(error)) => {
                        return Err(SessionStop::Disconnected(WsDisconnectReason::Transport(error.to_string())));
                    }
                    None => return Err(SessionStop::Disconnected(WsDisconnectReason::EndOfStream)),
                    _ => {}
                }
            }
        }
    }
}

async fn replay_subscriptions<S>(
    socket: &mut S,
    shared: &WsClientShared,
    subscriptions: &BTreeMap<String, DesiredSubscription>,
    shutdown_rx: &mut watch::Receiver<bool>,
    timeout: Duration,
) -> std::result::Result<(), SessionStop>
where
    S: futures::Sink<tungstenite::Message, Error = tungstenite::Error>
        + futures::Stream<Item = std::result::Result<tungstenite::Message, tungstenite::Error>>
        + Unpin,
{
    let mut pending = BTreeMap::<String, usize>::new();
    for subscription in subscriptions.values() {
        socket
            .send(tungstenite::Message::Text(subscription.subscribe.clone()))
            .await
            .map_err(|error| {
                SessionStop::Disconnected(WsDisconnectReason::Transport(error.to_string()))
            })?;
        *pending.entry(subscription.channel.clone()).or_default() += 1;
    }

    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    while !pending.is_empty() {
        tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    return Err(SessionStop::Shutdown);
                }
            }
            _ = &mut deadline => {
                return Err(SessionStop::NonReconnectable(WsDisconnectReason::Protocol(format!(
                    "timed out restoring subscriptions: {}",
                    pending
                        .into_iter()
                        .map(|(channel, count)| format!("{} ({})", channel, count))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))));
            }
            incoming = socket.next() => {
                match incoming {
                    Some(Ok(tungstenite::Message::Text(text))) => {
                        let Some(json) = record_text(shared, &text).await else {
                            continue;
                        };
                        // Whether a v2 refusal (unmarked confirmation or
                        // `unknown_channel`) can be absorbed by the logical fallback.
                        // It cannot say which v2 request it answers, so any direct
                        // v2 subscription being restored makes it a failure.
                        let v2_refusal = is_unbatched_v2_confirmation(&json);
                        let v2_all_logical = subscriptions
                            .values()
                            .filter(|desired| desired.channel == INDICATIVE_V2_CHANNEL)
                            .all(|desired| desired.logical_indicative && !desired.direct);
                        if v2_refusal && !v2_all_logical {
                            return Err(SessionStop::NonReconnectable(
                                WsDisconnectReason::Protocol(format!(
                                    "server does not support {INDICATIVE_V2_CHANNEL}; \
                                     direct subscriptions to it cannot be restored"
                                )),
                            ));
                        }
                        match json.get("type").and_then(|value| value.as_str()) {
                            Some("Subscribed") => {
                                if let Some(channel) = json.get("channel").and_then(|value| value.as_str()) {
                                    if let Some(count) = pending.get_mut(channel) {
                                        *count -= 1;
                                        if *count == 0 {
                                            pending.remove(channel);
                                        }
                                    }
                                }
                            }
                            // A logical v2 subscription rejected by the server falls
                            // back to v1 once replay finishes.
                            Some("Error") if v2_refusal => {
                                if let Some(count) = pending.get_mut(INDICATIVE_V2_CHANNEL) {
                                    *count -= 1;
                                    if *count == 0 {
                                        pending.remove(INDICATIVE_V2_CHANNEL);
                                    }
                                }
                            }
                            Some("Error") => {
                                let message = json
                                    .get("message")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("subscription restoration failed");
                                return Err(SessionStop::NonReconnectable(
                                    WsDisconnectReason::Protocol(message.to_string()),
                                ));
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(tungstenite::Message::Ping(payload))) => {
                        socket.send(tungstenite::Message::Pong(payload)).await.map_err(|error| {
                            SessionStop::Disconnected(WsDisconnectReason::Transport(error.to_string()))
                        })?;
                    }
                    Some(Ok(tungstenite::Message::Pong(payload))) => {
                        shared.pongs.lock().await.push(payload);
                    }
                    Some(Ok(tungstenite::Message::Close(frame))) => {
                        return Err(SessionStop::Disconnected(disconnect_reason_from_close(frame)));
                    }
                    Some(Err(error)) => {
                        return Err(SessionStop::Disconnected(WsDisconnectReason::Transport(
                            error.to_string(),
                        )));
                    }
                    None => return Err(SessionStop::Disconnected(WsDisconnectReason::EndOfStream)),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

async fn run_connected_socket<S>(
    socket: &mut S,
    shared: &WsClientShared,
    outbound_rx: &mut mpsc::Receiver<tungstenite::Message>,
    shutdown_rx: &mut watch::Receiver<bool>,
    rfq_v2_expires_at_ms: Option<u64>,
) -> SessionStop
where
    S: futures::Sink<tungstenite::Message, Error = tungstenite::Error>
        + futures::Stream<Item = std::result::Result<tungstenite::Message, tungstenite::Error>>
        + Unpin,
{
    let grant_expiry = async {
        if let Some(expires_at_ms) = rfq_v2_expires_at_ms {
            let remaining_ms = current_unix_ms().map_or(0, |now| expires_at_ms.saturating_sub(now));
            tokio::time::sleep(Duration::from_millis(remaining_ms)).await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(grant_expiry);
    loop {
        tokio::select! {
            _ = &mut grant_expiry => {
                let _ = socket.send(tungstenite::Message::Close(None)).await;
                return SessionStop::NonReconnectable(WsDisconnectReason::Protocol(
                    "RFQ V2 read grant expired; sign a fresh grant".to_string(),
                ));
            }
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    let _ = socket.send(tungstenite::Message::Close(None)).await;
                    return SessionStop::Shutdown;
                }
            }
            outbound = outbound_rx.recv() => {
                let Some(outbound) = outbound else {
                    return SessionStop::Shutdown;
                };
                if let Err(error) = socket.send(outbound).await {
                    return SessionStop::Disconnected(WsDisconnectReason::Transport(
                        error.to_string(),
                    ));
                }
            }
            incoming = socket.next() => {
                match incoming {
                    Some(Ok(tungstenite::Message::Text(text))) => {
                        let _ = record_text(shared, &text).await;
                        if let Err(stop) = apply_v2_fallback(socket, shared).await {
                            return stop;
                        }
                    }
                    Some(Ok(tungstenite::Message::Ping(payload))) => {
                        if let Err(error) = socket.send(tungstenite::Message::Pong(payload)).await {
                            return SessionStop::Disconnected(WsDisconnectReason::Transport(
                                error.to_string(),
                            ));
                        }
                    }
                    Some(Ok(tungstenite::Message::Pong(payload))) => {
                        shared.pongs.lock().await.push(payload);
                    }
                    Some(Ok(tungstenite::Message::Close(frame))) => {
                        return SessionStop::Disconnected(disconnect_reason_from_close(frame));
                    }
                    Some(Err(error)) => {
                        return SessionStop::Disconnected(WsDisconnectReason::Transport(
                            error.to_string(),
                        ));
                    }
                    None => return SessionStop::Disconnected(WsDisconnectReason::EndOfStream),
                    _ => {}
                }
            }
        }
    }
}

async fn run_ws_session(
    shared: Arc<WsClientShared>,
    config: WsClientConfig,
    ws_url: String,
    options: ConnectOptions,
    credentials: SessionCredentials,
    mut shutdown_rx: watch::Receiver<bool>,
    initial_tx: oneshot::Sender<Result<()>>,
) {
    let mut initial_tx = Some(initial_tx);
    let mut reconnecting = false;
    let mut attempt = 0_u32;

    loop {
        if reconnecting {
            let delay = reconnect_delay(config, attempt);
            set_state(&shared, WsConnectionState::Reconnecting { attempt, delay });
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        finish_session(&shared);
                        return;
                    }
                }
                _ = tokio::time::sleep(delay) => {}
            }
        }

        if credentials
            .rfq_v2_expires_at_ms()
            .is_some_and(|expires_at_ms| current_unix_ms().is_none_or(|now| now >= expires_at_ms))
        {
            let reason = WsDisconnectReason::Protocol(
                "RFQ V2 read grant expired; sign a fresh grant".to_string(),
            );
            record_disconnect(&shared, reason.clone());
            finish_session(&shared);
            if let Some(initial_tx) = initial_tx.take() {
                let _ = initial_tx.send(Err(ClientError::WebSocket(format!("{reason:?}"))));
            }
            return;
        }

        set_state(&shared, WsConnectionState::Connecting { attempt });
        debug!("Connecting to WebSocket: {}", ws_url);
        let connect_result = tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    finish_session(&shared);
                    if let Some(initial_tx) = initial_tx.take() {
                        let _ = initial_tx.send(Err(ClientError::WebSocket(
                            "WebSocket connection stopped before startup".to_string(),
                        )));
                    }
                    return;
                }
                continue;
            }
            result = connect_socket(&ws_url, options) => result,
        };

        let (mut socket, response) = match connect_result {
            Ok(connection) => connection,
            Err(error) => {
                let reason = WsDisconnectReason::Transport(error.to_string());
                record_disconnect(&shared, reason.clone());
                if !reconnecting {
                    finish_session(&shared);
                    if let Some(initial_tx) = initial_tx.take() {
                        let _ = initial_tx.send(Err(ClientError::WebSocket(format!(
                            "Failed to connect: {}",
                            error
                        ))));
                    }
                    return;
                }
                attempt = attempt.saturating_add(1);
                continue;
            }
        };
        info!(
            permessage_deflate = socket.get_ref().deflate_negotiated(),
            "WebSocket connected, status: {}",
            response.status()
        );

        let handshake_result = async {
            if let Some(wallet) = credentials.wallet.as_deref() {
                set_state(&shared, WsConnectionState::Authenticating);
                authenticate_socket(
                    &mut socket,
                    &shared,
                    wallet,
                    &mut shutdown_rx,
                    config.authentication_timeout,
                )
                .await?;
                info!("WebSocket authenticated as wallet: {}", wallet);
            }

            #[cfg(feature = "rfq-v2")]
            if let Some(grant) = credentials.rfq_v2_grant.as_ref() {
                set_state(&shared, WsConnectionState::Authenticating);
                authenticate_rfq_v2_socket(
                    &mut socket,
                    &shared,
                    grant,
                    &mut shutdown_rx,
                    config.authentication_timeout,
                )
                .await?;
            }

            // One write guard from the first replay until the sender is
            // published: a subscribe or unsubscribe from another task waits
            // and then runs against the live socket, so nothing it changes is
            // missed by the replay or the v2 fallback below.
            let mut subscriptions = shared.subscriptions.write().await;
            if !subscriptions.is_empty() {
                set_state(
                    &shared,
                    WsConnectionState::Resubscribing {
                        channels: subscriptions.len(),
                    },
                );
                replay_subscriptions(
                    &mut socket,
                    &shared,
                    &subscriptions,
                    &mut shutdown_rx,
                    config.subscription_timeout,
                )
                .await?;
            }
            // A v2 confirmation without the marker during replay: switch
            // those subscriptions to v1, and wait for the v1 confirmations
            // like the rest of the replay, before reporting the session ready.
            let (unsubscribes, replacements) =
                take_v2_fallback_locked(&shared, &mut subscriptions)?;
            for frame in unsubscribes {
                socket
                    .send(tungstenite::Message::Text(frame))
                    .await
                    .map_err(|error| {
                        SessionStop::Disconnected(WsDisconnectReason::Transport(error.to_string()))
                    })?;
            }
            if !replacements.is_empty() {
                replay_subscriptions(
                    &mut socket,
                    &shared,
                    &replacements,
                    &mut shutdown_rx,
                    config.subscription_timeout,
                )
                .await?;
            }

            let mut recovery_plan = WsRecoveryPlan::default();
            for subscription in subscriptions.values() {
                recovery_plan.include_channel(&subscription.channel);
            }
            let (tx, outbound_rx) = mpsc::channel(OUTBOUND_CONTROL_CAPACITY);
            *shared.tx.lock().expect("WebSocket sender mutex poisoned") = Some(tx);
            if reconnecting && !recovery_plan.is_empty() {
                set_state(&shared, WsConnectionState::RecoveryRequired(recovery_plan));
            } else {
                set_state(&shared, WsConnectionState::Ready);
            }
            drop(subscriptions);
            Ok::<_, SessionStop>(outbound_rx)
        }
        .await;

        let mut outbound_rx = match handshake_result {
            Ok(outbound_rx) => outbound_rx,
            Err(SessionStop::Shutdown) => {
                finish_session(&shared);
                if let Some(initial_tx) = initial_tx.take() {
                    let _ = initial_tx.send(Err(ClientError::WebSocket(
                        "WebSocket connection stopped before startup".to_string(),
                    )));
                }
                return;
            }
            Err(SessionStop::NonReconnectable(reason)) => {
                record_disconnect(&shared, reason.clone());
                finish_session(&shared);
                if let Some(initial_tx) = initial_tx.take() {
                    let _ = initial_tx.send(Err(ClientError::WebSocket(format!(
                        "WebSocket handshake failed: {:?}",
                        reason
                    ))));
                }
                return;
            }
            Err(stop) => {
                if let SessionStop::Disconnected(reason) = &stop {
                    record_disconnect(&shared, reason.clone());
                }
                if !reconnecting {
                    finish_session(&shared);
                    if let Some(initial_tx) = initial_tx.take() {
                        let _ = initial_tx
                            .send(Err(stop.websocket_error("WebSocket handshake failed")));
                    }
                    return;
                }
                attempt = attempt.saturating_add(1);
                continue;
            }
        };

        let was_reconnect = reconnecting;
        if let Some(initial_tx) = initial_tx.take() {
            let _ = initial_tx.send(Ok(()));
        }
        reconnecting = true;
        let connected_at = tokio::time::Instant::now();

        match run_connected_socket(
            &mut socket,
            &shared,
            &mut outbound_rx,
            &mut shutdown_rx,
            credentials.rfq_v2_expires_at_ms(),
        )
        .await
        {
            SessionStop::Shutdown => {
                finish_session(&shared);
                return;
            }
            SessionStop::NonReconnectable(reason) => {
                record_disconnect(&shared, reason);
                finish_session(&shared);
                return;
            }
            SessionStop::Disconnected(reason) => {
                warn!(?reason, "WebSocket disconnected; scheduling reconnect");
                record_disconnect(&shared, reason);
                *shared.tx.lock().expect("WebSocket sender mutex poisoned") = None;
                if !was_reconnect || connected_at.elapsed() >= config.reconnect_reset_after {
                    attempt = 0;
                } else {
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    }
}

impl Default for WsClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "websocket_reconnect_test.rs"]
mod reconnect_tests;

#[cfg(test)]
#[path = "websocket_deflate_test.rs"]
mod deflate_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use sonic_rs::JsonValueTrait;

    #[test]
    fn test_ws_client_new() {
        let client = WsClient::new();
        // Client should be created successfully
        assert!(client.shared.messages.try_lock().is_ok());
        assert!(client.shared.pongs.try_lock().is_ok());
        assert!(client.shared.tx.try_lock().is_ok());
    }

    #[test]
    fn test_ws_client_default() {
        let client = WsClient::default();
        assert!(client.shared.messages.try_lock().is_ok());
    }

    #[tokio::test]
    async fn test_ws_client_get_messages_empty() {
        let client = WsClient::new();
        let messages = client.get_messages().await;
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_ws_client_clear_messages() {
        let client = WsClient::new();
        // Add a message manually
        client
            .shared
            .messages
            .lock()
            .await
            .push(sonic_rs::json!({"test": "value"}));
        assert_eq!(client.get_messages().await.len(), 1);

        // Clear messages
        client.clear_messages().await;
        assert!(client.get_messages().await.is_empty());
    }

    #[tokio::test]
    async fn test_ws_client_count_messages() {
        let client = WsClient::new();
        // Add some messages
        {
            let mut messages = client.shared.messages.lock().await;
            messages.push(sonic_rs::json!({"type": "order", "id": 1}));
            messages.push(sonic_rs::json!({"type": "trade", "id": 2}));
            messages.push(sonic_rs::json!({"type": "order", "id": 3}));
        }

        let order_count = client
            .count_messages(|m| m.get("type").and_then(|t| t.as_str()) == Some("order"))
            .await;
        assert_eq!(order_count, 2);

        let trade_count = client
            .count_messages(|m| m.get("type").and_then(|t| t.as_str()) == Some("trade"))
            .await;
        assert_eq!(trade_count, 1);
    }

    #[tokio::test]
    async fn test_ws_client_subscribe_not_connected() {
        let client = WsClient::new();
        let result = client.subscribe(vec!["orders"]).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::error::ClientError::WebSocket(_)
        ));
    }

    #[tokio::test]
    async fn test_ws_client_ping_not_connected() {
        let client = WsClient::new();
        let result = client.ping().await;
        assert!(matches!(
            result.unwrap_err(),
            crate::error::ClientError::WebSocket(_)
        ));
    }

    #[tokio::test]
    async fn test_ws_client_sends_ping_with_payload() {
        let client = WsClient::new();
        let (tx, mut rx) = mpsc::channel(1);
        *client
            .shared
            .tx
            .lock()
            .expect("WebSocket sender mutex poisoned") = Some(tx);

        client.ping_with_payload(b"health".to_vec()).await.unwrap();

        match rx.recv().await.unwrap() {
            tungstenite::Message::Ping(payload) => assert_eq!(payload, b"health"),
            other => panic!("Expected ping frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_ws_client_rejects_oversized_ping_payload() {
        let client = WsClient::new();
        let (tx, _rx) = mpsc::channel(1);
        *client
            .shared
            .tx
            .lock()
            .expect("WebSocket sender mutex poisoned") = Some(tx);

        let result = client.ping_with_payload(vec![0; 126]).await;
        assert!(matches!(
            result.unwrap_err(),
            crate::error::ClientError::WebSocket(_)
        ));
    }

    #[tokio::test]
    async fn test_ws_client_tracks_received_pongs() {
        let client = WsClient::new();
        client.shared.pongs.lock().await.push(b"health".to_vec());

        let pong = client
            .wait_for_pong(|payload| payload == b"health", 100)
            .await;
        assert_eq!(pong, Some(b"health".to_vec()));

        client.clear_pongs().await;
        assert!(client.get_pongs().await.is_empty());
    }

    #[tokio::test]
    async fn test_ws_client_responds_to_server_ping() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.send(tungstenite::Message::Ping(b"server".to_vec()))
                .await
                .unwrap();

            match tokio::time::timeout(Duration::from_secs(1), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
            {
                tungstenite::Message::Pong(payload) => assert_eq!(payload, b"server"),
                other => panic!("Expected pong frame, got {other:?}"),
            }
        });

        let client = WsClient::new();
        client
            .connect(&format!("http://{}", addr), None)
            .await
            .unwrap();

        server.await.unwrap();
    }

    #[tokio::test]
    async fn test_ws_client_wait_for_message_timeout() {
        let client = WsClient::new();

        // Wait for a message that doesn't exist with short timeout
        let result = client
            .wait_for_message(|m| m.get("never").is_some(), 100)
            .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_ws_client_wait_for_message_found() {
        let client = WsClient::new();

        // Add a message in a background task after a delay
        let messages = client.shared.messages.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            messages.lock().await.push(sonic_rs::json!({"found": true}));
        });

        let result = client
            .wait_for_message(|m| m.get("found").is_some(), 500)
            .await;
        assert!(result.is_some());
        assert_eq!(result.unwrap()["found"], true);
    }

    #[test]
    fn test_ws_message_subscribe_serialize() {
        let msg = WsMessage::Subscribe {
            channel: "orders".to_string(),
            symbols: None,
            expiry: None,
            option_type: None,
        };
        let json = sonic_rs::to_string(&msg).unwrap();
        assert!(json.contains("Subscribe"));
        assert!(json.contains("orders"));
    }

    #[test]
    fn test_ws_message_unsubscribe_serialize() {
        let msg = WsMessage::Unsubscribe {
            channel: "trades".to_string(),
            symbols: None,
            expiry: None,
            option_type: None,
        };
        let json = sonic_rs::to_string(&msg).unwrap();
        assert!(json.contains("Unsubscribe"));
        assert!(json.contains("trades"));
    }

    #[test]
    fn test_ws_message_subscribed_deserialize() {
        let json = r#"{"type": "Subscribed", "channel": "orders"}"#;
        let msg: WsMessage = sonic_rs::from_str(json).unwrap();
        match msg {
            WsMessage::Subscribed { channel, .. } => assert_eq!(channel, "orders"),
            _ => panic!("Expected Subscribed variant"),
        }
    }

    #[test]
    fn test_ws_message_unsubscribed_deserialize() {
        let json = r#"{"type": "Unsubscribed", "channel": "trades"}"#;
        let msg: WsMessage = sonic_rs::from_str(json).unwrap();
        match msg {
            WsMessage::Unsubscribed { channel } => assert_eq!(channel, "trades"),
            _ => panic!("Expected Unsubscribed variant"),
        }
    }

    #[test]
    fn test_ws_message_error_deserialize() {
        let json = r#"{"type": "Error", "message": "invalid channel"}"#;
        let msg: WsMessage = sonic_rs::from_str(json).unwrap();
        match msg {
            WsMessage::Error { message, .. } => assert_eq!(message, "invalid channel"),
            _ => panic!("Expected Error variant"),
        }
    }

    #[test]
    fn test_ws_message_unknown_type_deserialize() {
        let json = r#"{"type": "unknown_type", "data": "something"}"#;
        let msg: WsMessage = sonic_rs::from_str(json).unwrap();
        match msg {
            WsMessage::Other => {}
            _ => panic!("Expected Other variant"),
        }
    }

    #[test]
    fn test_ws_message_debug() {
        let msg = WsMessage::Subscribe {
            channel: "orders".to_string(),
            symbols: None,
            expiry: None,
            option_type: None,
        };
        let debug = format!("{:?}", msg);
        assert!(debug.contains("Subscribe"));
        assert!(debug.contains("orders"));
    }

    #[test]
    fn test_ws_message_clone() {
        let msg = WsMessage::Error {
            code: None,
            channel: None,
            message: "test error".to_string(),
        };
        let cloned = msg.clone();
        match cloned {
            WsMessage::Error { message, .. } => assert_eq!(message, "test error"),
            _ => panic!("Clone failed"),
        }
    }
}
