//! API response types.

use crate::{
    FillSource, MarketUpdateStatus, OptionType, OrderStatus, OrderUpdateStatus, RfqStatus, Side,
    TimeInForce, TradeSide, WalletAddress,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize};

fn default_true() -> bool {
    true
}

/// Deserialize a string as Decimal.
/// The API always returns Decimal values as JSON strings (e.g., "0.5", "10000").
fn string_decimal<'de, D>(deserializer: D) -> Result<Decimal, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de;

    struct StringDecimalVisitor;

    impl<'de> de::Visitor<'de> for StringDecimalVisitor {
        type Value = Decimal;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string representing Decimal")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            v.parse::<Decimal>().map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_str(StringDecimalVisitor)
}

/// Deserialize an optional string as Option<Decimal>.
/// The API always returns Decimal values as JSON strings (e.g., "0.5", "10000").
fn option_string_decimal<'de, D>(deserializer: D) -> Result<Option<Decimal>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de;

    struct OptionStringDecimalVisitor;

    impl<'de> de::Visitor<'de> for OptionStringDecimalVisitor {
        type Value = Option<Decimal>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("null or a string representing Decimal")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_some<D2>(self, deserializer: D2) -> Result<Self::Value, D2::Error>
        where
            D2: Deserializer<'de>,
        {
            string_decimal(deserializer).map(Some)
        }
    }

    deserializer.deserialize_option(OptionStringDecimalVisitor)
}

/// Generic API response wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandardMarginLiquidationOrderResponse {
    pub request_id: String,
    pub auction_id: String,
    pub liquidated_wallet: String,
    pub liquidator_wallet: String,
}

/// Cursor metadata for public liquidation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorPage {
    /// Page size used by the request.
    pub limit: usize,
    /// Opaque cursor for the next page, or null when no further page is known.
    pub next_cursor: Option<String>,
    /// Whether another page is available.
    pub has_more: bool,
}

/// Liquidation history transition entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidationHistoryEntry {
    /// History entry ID.
    pub id: i64,
    /// Wallet address.
    pub wallet: String,
    /// Previous liquidation state.
    pub previous_state: String,
    /// New liquidation state.
    pub new_state: String,
    /// Liquidation mode (`partial` or `full`) if applicable.
    pub liquidation_mode: Option<String>,
    /// Equity at time of transition.
    #[serde(with = "rust_decimal::serde::str")]
    pub equity: Decimal,
    /// MM required at time of transition.
    #[serde(with = "rust_decimal::serde::str")]
    pub mm_required: Decimal,
    /// Maintenance margin at time of transition.
    #[serde(with = "rust_decimal::serde::str")]
    pub maintenance_margin: Decimal,
    /// Shortfall at time of transition.
    #[serde(with = "rust_decimal::serde::str")]
    pub shortfall: Decimal,
    /// Auction ID, if applicable.
    pub auction_id: Option<String>,
    /// Request ID associated with the transition.
    pub request_id: Option<String>,
    /// Transaction hash associated with the transition.
    pub tx_hash: Option<String>,
    /// Margin needed for full liquidation, when applicable.
    #[serde(with = "rust_decimal::serde::str_option")]
    pub margin_needed: Option<Decimal>,
    /// Winning liquidator/manager, if resolved.
    pub winner_address: Option<String>,
    /// Bonus credited on resolution, if any.
    #[serde(with = "rust_decimal::serde::str_option")]
    pub bonus: Option<Decimal>,
    /// Additional status details.
    pub details: sonic_rs::Value,
    /// Timestamp of transition.
    pub timestamp: i64,
}

/// Public global liquidation history response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicLiquidationsResponse {
    /// Whether the request was successful.
    pub success: bool,
    /// Liquidation transition entries ordered newest first.
    pub data: Vec<LiquidationHistoryEntry>,
    /// Cursor pagination info.
    pub page: CursorPage,
    /// Error message, if any.
    pub error: Option<String>,
}

/// Wallet liquidation status response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidationStatusResponse {
    pub success: bool,
    pub data: Option<LiquidationStatusData>,
    pub error: Option<String>,
}

/// Current liquidation state for one wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidationStatusData {
    pub wallet: String,
    pub state: String,
    pub liquidation_mode: Option<String>,
    pub margin_mode: String,
    #[serde(deserialize_with = "string_decimal")]
    pub equity: Decimal,
    #[serde(deserialize_with = "string_decimal")]
    pub mm_required: Decimal,
    #[serde(deserialize_with = "string_decimal")]
    pub maintenance_margin: Decimal,
    #[serde(deserialize_with = "string_decimal")]
    pub shortfall: Decimal,
    pub partial_liquidation: Option<PartialLiquidationStatusData>,
    pub full_liquidation: Option<FullLiquidationStatusData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialLiquidationStatusData {
    pub entered_at: i64,
    #[serde(deserialize_with = "string_decimal")]
    pub target_equity: Decimal,
    #[serde(deserialize_with = "string_decimal")]
    pub mm_shortfall: Decimal,
    pub escalation_deadline: i64,
    pub last_reprice_at: Option<i64>,
    pub active_order_request_ids: Vec<String>,
    pub active_order_client_ids: Vec<String>,
    pub bonus_bps: i32,
    pub pending_full_auction_id: Option<String>,
    pub pending_full_request_id: Option<String>,
    pub pending_full_tx_hash: Option<String>,
    #[serde(default, deserialize_with = "option_string_decimal")]
    pub pending_full_margin_needed: Option<Decimal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullLiquidationStatusData {
    pub auction_id: Option<String>,
    pub request_id: Option<String>,
    pub tx_hash: Option<String>,
    pub started_at: Option<i64>,
    pub chain_start_time: Option<i64>,
    #[serde(default, deserialize_with = "option_string_decimal")]
    pub margin_needed: Option<Decimal>,
    pub stop_request_id: Option<String>,
    pub stop_tx_hash: Option<String>,
    pub liquidated_at: Option<i64>,
    pub winner: Option<String>,
    #[serde(default, deserialize_with = "option_string_decimal")]
    pub bonus: Option<Decimal>,
    pub resolution_tx_hash: Option<String>,
    #[serde(default, deserialize_with = "option_string_decimal")]
    pub current_required_bid_usdc: Option<Decimal>,
    #[serde(default, deserialize_with = "option_string_decimal")]
    pub current_equity: Option<Decimal>,
    #[serde(default, deserialize_with = "option_string_decimal")]
    pub current_mm_required: Option<Decimal>,
    #[serde(default, deserialize_with = "option_string_decimal")]
    pub current_maintenance_margin: Option<Decimal>,
    pub current_positions: Option<Vec<crate::requests::StandardMarginLiquidationPositionRequest>>,
    pub current_portfolio_hash: Option<String>,
    pub current_auction_terms_hash: Option<String>,
    pub current_auction_version: Option<u64>,
    pub current_valuation_timestamp_ms: Option<u64>,
}

/// Order information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderInfo {
    /// Instrument symbol
    pub symbol: String,
    /// Order price
    #[serde(deserialize_with = "string_decimal")]
    pub price: Decimal,
    /// Order size in raw units
    #[serde(deserialize_with = "string_decimal")]
    pub size: Decimal,
    /// Order side
    pub side: Side,
    /// Time in force
    pub tif: TimeInForce,
    /// Client-provided order ID
    pub client_id: Option<String>,
    /// Exchange-assigned order ID
    pub order_id: Option<u64>,
    /// Whether this is a perp order (required - must be explicitly set)
    pub is_perp: bool,
    /// Underlying asset for perp orders
    pub underlying: Option<String>,
    /// Reduce-only flag for perp orders
    pub reduce_only: Option<bool>,
    /// Nonce for signature verification
    pub nonce: Option<u64>,
    /// EIP-712 signature
    pub signature: Option<String>,
    /// Whether MMP is enabled
    #[serde(default)]
    pub mmp_enabled: bool,
    /// Optional builder code address for fee rebates
    pub builder_code_address: Option<WalletAddress>,
}

/// Order message (response from order placement).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderMessage {
    pub order_id: Option<u64>,
    #[serde(alias = "request")]
    pub info: OrderInfo,
    pub status: OrderStatus,
    pub timestamp: u64,
    pub reason: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_string_decimal"
    )]
    pub filled_size: Option<Decimal>,
    #[serde(alias = "account")]
    pub wallet_address: WalletAddress,
}

/// Order update message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderUpdateMessage {
    /// Timestamp in milliseconds
    pub timestamp: u64,
    /// Order information
    pub info: OrderInfo,
    /// Current order status
    pub status: OrderUpdateStatus,
    /// Reason for rejection or cancellation
    pub reason: Option<String>,
    /// Amount filled so far
    #[serde(deserialize_with = "string_decimal")]
    pub filled_size: Decimal,
    /// Exchange-assigned order ID
    pub order_id: Option<u64>,
    /// Wallet address
    #[serde(alias = "wallet", alias = "account")]
    pub wallet_address: WalletAddress,
    /// Whether MMP triggered this update
    #[serde(default)]
    pub mmp_triggered: bool,
    /// Request ID for correlating this update with the original command.
    /// Populated from the triggering OrderActionMessage's request_id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Market information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Market {
    /// Market symbol (e.g., "BTC-20250131-100000-C")
    pub symbol: String,
    /// Underlying asset (e.g., "BTC")
    pub underlying: String,
    /// Expiry timestamp
    pub expiry: u64,
    /// Strike price
    #[serde(deserialize_with = "string_decimal")]
    pub strike: Decimal,
    /// Option type (Call or Put)
    pub option_type: OptionType,
}

/// Market update message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketUpdateMessage {
    /// Market information
    pub market: Market,
    /// Update status
    pub status: MarketUpdateStatus,
    /// Timestamp in milliseconds
    pub timestamp: u64,
    /// Reason for failure (if status is MarketCreationFailed or MarketDeletionFailed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Simple market response (for create/delete).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketResponse {
    pub success: bool,
    pub message: String,
}

/// Fill (trade execution) information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fill {
    pub trade_id: u64,
    pub taker_order_id: u64,
    pub maker_order_id: u64,
    pub symbol: String,
    #[serde(deserialize_with = "string_decimal")]
    pub price: Decimal,
    /// Fill size in contract units.
    #[serde(deserialize_with = "string_decimal")]
    pub size: Decimal,
    pub taker_side: Side,
    pub taker_wallet_address: WalletAddress,
    pub maker_wallet_address: WalletAddress,
    #[serde(deserialize_with = "string_decimal")]
    pub fee: Decimal,
    pub is_taker: bool,
    pub timestamp: u64,
    /// Optional builder code address for fee rebates
    pub builder_code_address: Option<WalletAddress>,
    /// Fee paid to builder code (deducted from platform revenue)
    #[serde(default)]
    pub builder_code_fee: Option<Decimal>,
    /// Source of the fill (orderbook or RFQ). Defaults to Orderbook for backward compatibility.
    #[serde(default)]
    pub source: FillSource,
    /// Realized PnL for the taker from this fill (set by journal side-effect calculation).
    /// Only present for position-reducing fills; None for position-opening fills.
    #[serde(default)]
    pub taker_realized_pnl: Option<Decimal>,
    /// Realized PnL for the maker from this fill.
    #[serde(default)]
    pub maker_realized_pnl: Option<Decimal>,
    /// Underlying notional captured at fill time for option analytics.
    #[serde(default)]
    pub underlying_notional: Option<Decimal>,
}

/// Orderbook snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderbookUpdate {
    pub symbol: String,
    pub bids: Vec<(Decimal, Decimal)>,
    pub asks: Vec<(Decimal, Decimal)>,
    pub timestamp: u64,
}

/// L2 update (price level change).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L2Update {
    #[serde(deserialize_with = "string_decimal")]
    pub price: Decimal,
    #[serde(deserialize_with = "string_decimal")]
    pub size: Decimal, // Size remaining at this price level, 0 to remove the level
}

/// L2 message (orderbook delta).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L2Message {
    pub symbol: String,
    pub bid_updates: Vec<L2Update>,
    pub ask_updates: Vec<L2Update>,
    pub timestamp: u64,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<i64>,
}

/// Trade message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeMessage {
    pub symbol: String,
    #[serde(deserialize_with = "string_decimal")]
    pub price: Decimal,
    #[serde(deserialize_with = "string_decimal")]
    pub size: Decimal,
    pub side: TradeSide,
    pub timestamp: u64,
}

/// Result for a single order in a bulk operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkOrderResult {
    pub index: usize,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<OrderMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Response from bulk order placement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkPlaceOrderResponse {
    pub results: Vec<BulkOrderResult>,
}

/// Response from bulk cancel operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkCancelOrderResponse {
    pub results: Vec<BulkOrderResult>,
}

/// Pagination info for list responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pagination {
    pub limit: usize,
    pub offset: usize,
    pub count: usize,
}

/// Orders list response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrdersResponse {
    pub success: bool,
    pub data: Vec<OrderInfo>,
    pub pagination: Pagination,
}

/// Option summary (for fetching underlying price).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OptionSummary {
    pub underlying_price: f64,
    pub option_token_address: Option<WalletAddress>,
    #[serde(default)]
    pub greeks: Option<OptionGreeks>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OptionGreeks {
    pub delta: f64,
    pub gamma: f64,
    pub theta: f64,
    pub vega: f64,
    pub rho: f64,
}

/// JSON-RPC error information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code
    pub code: i32,
    /// Error message
    pub message: String,
    /// Additional error data
    pub data: Option<serde_json::Value>,
}

/// JSON-RPC style response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse<T> {
    /// JSON-RPC version
    pub jsonrpc: String,
    /// Result data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    /// Error information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    /// Whether this is testnet
    pub testnet: bool,
    /// Processing time in microseconds
    #[serde(rename = "usDiff")]
    pub us_diff: i64,
    /// Request received timestamp in microseconds
    #[serde(rename = "usIn")]
    pub us_in: i64,
    /// Response sent timestamp in microseconds
    #[serde(rename = "usOut")]
    pub us_out: i64,
}

/// Response for approving an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproveAgentResponse {
    /// Whether the request succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
}

/// Response for revoking an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeAgentResponse {
    /// Whether the request succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
}

/// Response for revoking every agent authorized by a wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeAllAgentsResponse {
    /// Whether the request succeeded
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
}

/// Response listing authorized agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizedAgentsResponse {
    /// List of authorized agent wallet addresses
    pub agents: Vec<WalletAddress>,
}

/// Tick size step for instrument pricing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TickSizeStep {
    /// Tick size at this level
    pub tick_size: f64,
    /// Price above which this tick size applies
    pub above_price: f64,
}

/// Instrument information response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentResponse {
    /// Price index name
    pub price_index: String,
    /// RFQ enabled
    pub rfq: bool,
    /// Orderbook enabled
    #[serde(default = "default_true")]
    pub orderbook: bool,
    /// Instrument kind
    pub kind: String,
    /// Instrument name/symbol
    pub instrument_name: String,
    /// Option token contract address
    pub option_token_address: Option<WalletAddress>,
    /// Maker commission rate
    pub maker_commission: f64,
    /// Taker commission rate
    pub taker_commission: f64,
    /// Instrument type
    pub instrument_type: String,
    /// Expiration timestamp
    pub expiration_timestamp: i64,
    /// Creation timestamp
    pub creation_timestamp: i64,
    /// Whether instrument is active
    pub is_active: bool,
    /// Option type (call/put)
    pub option_type: String,
    /// Contract size
    pub contract_size: f64,
    /// Tick size
    pub tick_size: f64,
    /// Strike price
    pub strike: f64,
    /// Instrument ID
    pub instrument_id: i32,
    /// Settlement period
    pub settlement_period: String,
    /// Minimum trade amount
    pub min_trade_amount: f64,
    /// Block trade commission
    pub block_trade_commission: f64,
    /// Block trade minimum amount
    pub block_trade_min_trade_amount: f64,
    /// Block trade tick size
    pub block_trade_tick_size: f64,
    /// Settlement currency
    pub settlement_currency: String,
    /// Base currency
    pub base_currency: String,
    /// Counter currency
    pub counter_currency: String,
    /// Quote currency
    pub quote_currency: String,
    /// Tick size steps
    pub tick_size_steps: Vec<TickSizeStep>,
}

/// Canonical instrument specification for discovery, quoting, and lifecycle
/// clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentSpecResponse {
    /// Canonical string identifier.
    pub instrument_id: String,
    /// Numeric instrument identifier.
    pub instrument_numeric_id: i32,
    /// Native venue symbol used in REST and WebSocket calls.
    pub exchange_symbol: String,
    /// Trading pair for grouping, e.g. `"BTC-USD"`.
    pub sym: String,
    /// Venue identifier, e.g. `"HYPERCALL"`.
    pub exchange: String,
    /// Instrument class: `"OPTION"`, `"PERP"`, `"SPOT"`, or `"FUTURE"`.
    pub instrument_kind: String,
    /// Option side, `"C"` or `"P"` when this is an option.
    pub option_kind: Option<String>,
    /// Delivery mode, e.g. `"CASH"` or `"PHYSICAL"`.
    pub delivery: Option<String>,
    /// Settlement asset for margin and PnL accounting.
    pub settle_asset: Option<String>,
    /// Base asset for cross-venue mapping.
    pub base_asset: String,
    /// Quote asset for cross-venue mapping.
    pub quote_asset: String,
    /// Strike price for derivatives. Serialized as a string.
    pub strike: Decimal,
    /// Expiry timestamp in nanoseconds since epoch.
    pub expiry_ns: i64,
    /// UTC hour of day at which this instrument expires and settles.
    pub settlement_hour_utc: Option<u8>,
    /// UTC time of day ("HH:MM") at which this instrument expires and
    /// settles. Authoritative alongside expiry_ns; per-underlying policy.
    pub settlement_time_utc: Option<String>,
    /// Contract size used for Greeks and notional scaling.
    pub contract_size: f64,
    /// Minimum trade size.
    pub min_trade_size: f64,
    /// Base tick size for price rounding.
    pub tick_size: f64,
    /// Decimal precision for wire price quantities.
    pub price_decimals: Option<u32>,
    /// Decimal precision for wire size quantities.
    pub size_decimals: Option<u32>,
    /// Stepped tick rules, when any.
    pub min_price_increment_bands: Vec<TickSizeStep>,
    /// Lifecycle state: `"OPEN"`, `"SETTLEMENT"`, or `"DELIVERED"`.
    pub state: String,
    /// Whether the instrument can currently accept new trading activity.
    pub is_tradable: bool,
    /// First-listed time in nanoseconds, if known.
    pub listed_time_ns: Option<i64>,
    /// Timestamp for this spec version in nanoseconds. Currently always
    /// null: no persisted spec-change timestamp exists yet, and cache
    /// rebuild times would read as false spec changes.
    pub event_ts_ns: Option<i64>,
    /// Maker fee in basis points, if configured per instrument.
    pub maker_fee_bps: Option<f64>,
    /// Taker fee in basis points, if configured per instrument.
    pub taker_fee_bps: Option<f64>,
    /// Initial margin fraction, if configured per instrument.
    pub initial_margin_fraction: Option<f64>,
    /// Maintenance margin fraction, if configured per instrument.
    pub maintenance_margin_fraction: Option<f64>,
    /// Per-instrument position limit, if configured.
    pub position_limit: Option<f64>,
    /// Hypercall option token contract address, if deployed.
    pub option_token_address: Option<WalletAddress>,
    /// Settlement oracle identifier, if modeled.
    pub settlement_oracle: Option<String>,
    /// External condition identifier, if modeled.
    pub condition_id: Option<String>,
    /// Source used to resolve the underlying at settlement, if modeled.
    pub underlying_resolution_source: Option<String>,
}

/// Order book statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBookStats {
    /// 24h high
    pub high: Option<f64>,
    /// 24h low
    pub low: Option<f64>,
    /// 24h price change
    pub price_change: Option<f64>,
    /// 24h volume
    pub volume: f64,
    /// 24h volume in USD
    pub volume_usd: f64,
}

/// Option Greeks for order book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBookGreeks {
    /// Delta
    pub delta: f64,
    /// Gamma
    pub gamma: f64,
    /// Vega
    pub vega: f64,
    /// Theta
    pub theta: f64,
    /// Rho
    pub rho: f64,
}

/// Orderbook response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBookResponse {
    /// Timestamp
    pub timestamp: i64,
    /// Market state
    pub state: String,
    /// Order book statistics
    pub stats: OrderBookStats,
    /// Option Greeks
    #[serde(skip_serializing_if = "Option::is_none")]
    pub greeks: Option<OrderBookGreeks>,
    /// Change ID for incremental updates
    pub change_id: i64,
    /// Index price
    pub index_price: f64,
    /// Instrument name
    pub instrument_name: String,
    /// Option token contract address
    pub option_token_address: Option<WalletAddress>,
    /// Bid orders [price, size], where size is in human-readable contracts.
    pub bids: Vec<[f64; 2]>,
    /// Ask orders [price, size], where size is in human-readable contracts.
    pub asks: Vec<[f64; 2]>,
    /// Last trade price
    pub last_price: Option<f64>,
    /// Settlement price
    pub settlement_price: f64,
    /// Minimum price
    pub min_price: f64,
    /// Maximum price
    pub max_price: f64,
    /// Open interest
    pub open_interest: f64,
    /// Mark price
    pub mark_price: f64,
    /// Theoretical option price derived from the vol oracle, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theoretical_price: Option<f64>,
    /// Best bid price
    pub best_bid_price: f64,
    /// Best ask price
    pub best_ask_price: f64,
    /// Theoretical implied volatility used for mark price and greeks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_iv: Option<f64>,
    /// Quote-derived ask-side implied volatility
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_iv: Option<f64>,
    /// Quote-derived bid-side implied volatility
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bid_iv: Option<f64>,
    /// Underlying price
    pub underlying_price: f64,
    /// Underlying index name
    pub underlying_index: String,
    /// Interest rate
    pub interest_rate: f64,
    /// Estimated delivery price
    pub estimated_delivery_price: f64,
    /// Best ask amount in human-readable contracts.
    pub best_ask_amount: f64,
    /// Best bid amount in human-readable contracts.
    pub best_bid_amount: f64,
}

// =============================================================================
// Historical PnL / Equity Response Types
// =============================================================================

/// Supported historical equity snapshot intervals.
pub const HISTORICAL_PNL_INTERVAL_5M_MS: i64 = 5 * 60 * 1000;
/// Supported historical equity snapshot intervals.
pub const HISTORICAL_PNL_INTERVAL_1H_MS: i64 = 60 * 60 * 1000;
/// Supported historical equity snapshot intervals.
pub const HISTORICAL_PNL_INTERVAL_1D_MS: i64 = 24 * 60 * 60 * 1000;

/// Historical equity interval identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoricalPnlInterval {
    /// 5-minute snapshots.
    #[serde(rename = "5m")]
    FiveMinutes,
    /// 1-hour snapshots.
    #[serde(rename = "1h")]
    OneHour,
    /// 1-day snapshots.
    #[serde(rename = "1d")]
    OneDay,
}

impl HistoricalPnlInterval {
    /// Interval duration in milliseconds.
    pub fn as_ms(self) -> i64 {
        match self {
            Self::FiveMinutes => HISTORICAL_PNL_INTERVAL_5M_MS,
            Self::OneHour => HISTORICAL_PNL_INTERVAL_1H_MS,
            Self::OneDay => HISTORICAL_PNL_INTERVAL_1D_MS,
        }
    }

    /// Interval identifier used in query params and API responses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5m",
            Self::OneHour => "1h",
            Self::OneDay => "1d",
        }
    }
}

/// A single historical equity point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalPnlPoint {
    /// Interval bucket start timestamp in milliseconds since epoch.
    pub timestamp: i64,
    /// Total account equity at the bucket timestamp.
    #[serde(deserialize_with = "string_decimal")]
    pub equity: Decimal,
    /// Per-symbol PnL attribution. Keys are symbol names, values are [position, entry_price, realized, unrealized, total].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<std::collections::HashMap<String, [f64; 5]>>,
    /// Cumulative net deposits (deposits minus withdraws) as of this bucket's
    /// timestamp. The frontend uses this as the P&L baseline so multi-deposit
    /// wallets stay correctly anchored historically; the delta between
    /// consecutive points also surfaces deposit/withdraw events for annotation.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_string_decimal"
    )]
    pub net_deposits: Option<Decimal>,
}

/// Historical equity response for a wallet and interval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalPnlResponse {
    /// Account wallet address.
    pub wallet_address: WalletAddress,
    /// Historical interval identifier.
    pub interval: HistoricalPnlInterval,
    /// Returned points in ascending timestamp order.
    #[serde(default)]
    pub points: Vec<HistoricalPnlPoint>,
}

/// Supported historical theo snapshot intervals.
pub const HISTORICAL_THEO_INTERVAL_5M_MS: i64 = HISTORICAL_PNL_INTERVAL_5M_MS;
/// Supported historical theo snapshot intervals.
pub const HISTORICAL_THEO_INTERVAL_1H_MS: i64 = HISTORICAL_PNL_INTERVAL_1H_MS;
/// Supported historical theo snapshot intervals.
pub const HISTORICAL_THEO_INTERVAL_1D_MS: i64 = HISTORICAL_PNL_INTERVAL_1D_MS;

/// Historical theo interval identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoricalTheoInterval {
    /// 5-minute snapshots.
    #[serde(rename = "5m")]
    FiveMinutes,
    /// 1-hour snapshots.
    #[serde(rename = "1h")]
    OneHour,
    /// 1-day snapshots.
    #[serde(rename = "1d")]
    OneDay,
}

impl HistoricalTheoInterval {
    /// Interval duration in milliseconds.
    pub fn as_ms(self) -> i64 {
        match self {
            Self::FiveMinutes => HISTORICAL_THEO_INTERVAL_5M_MS,
            Self::OneHour => HISTORICAL_THEO_INTERVAL_1H_MS,
            Self::OneDay => HISTORICAL_THEO_INTERVAL_1D_MS,
        }
    }

    /// Interval identifier used in query params and API responses.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5m",
            Self::OneHour => "1h",
            Self::OneDay => "1d",
        }
    }
}

/// A single historical theoretical-price point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalTheoPoint {
    /// Interval bucket start timestamp in milliseconds since epoch.
    pub timestamp: i64,
    /// Theoretical option price at the bucket timestamp.
    pub theoretical_price: f64,
}

/// Historical theo response for an instrument and interval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoricalTheoResponse {
    /// Option instrument symbol.
    pub instrument_name: String,
    /// Historical interval identifier.
    pub interval: HistoricalTheoInterval,
    /// Returned points in ascending timestamp order.
    #[serde(default)]
    pub points: Vec<HistoricalTheoPoint>,
}

// =============================================================================
// Portfolio Response Types
// =============================================================================

/// Unified margin summary that works for both Standard and Portfolio modes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct MarginSummary {
    /// Margin mode: "standard" or "portfolio"
    pub mode: String,
    /// Total account equity (balance + unrealized PnL)
    #[serde(deserialize_with = "string_decimal")]
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub equity: Decimal,
    /// Initial Margin required from positions
    #[serde(deserialize_with = "string_decimal")]
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub position_im: Decimal,
    /// Initial Margin from open orders
    #[serde(deserialize_with = "string_decimal")]
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub open_orders_im: Decimal,
    /// Excess Initial Margin (equity - position_im - open_orders_im)
    #[serde(deserialize_with = "string_decimal")]
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub initial_margin: Decimal,
    /// Excess Maintenance Margin (equity - position_mm)
    #[serde(deserialize_with = "string_decimal")]
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub maintenance_margin: Decimal,
    /// (Standard mode only) USDC premium reserved for open BUY orders
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option_string_decimal"
    )]
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub open_orders_premium_reserved: Option<Decimal>,
}

impl MarginSummary {
    /// Calculate the maintenance margin required (position_mm).
    /// Since the API returns excess MM (equity - position_mm), we derive position_mm.
    pub fn maintenance_margin_required(&self) -> Decimal {
        // maintenance_margin = equity - position_mm
        // Therefore: position_mm = equity - maintenance_margin
        (self.equity - self.maintenance_margin).max(Decimal::ZERO)
    }

    /// Calculate margin utilization (0-1 scale).
    /// Returns the ratio of maintenance margin required to equity.
    pub fn margin_utilization(&self) -> Decimal {
        if self.equity <= Decimal::ZERO {
            return Decimal::ONE;
        }
        (self.maintenance_margin_required() / self.equity).min(Decimal::ONE)
    }
}

/// Position information in portfolio response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioPosition {
    /// Wallet address
    pub wallet_address: WalletAddress,
    /// Option symbol (e.g., "BTC-20260115-100000-C")
    pub symbol: String,
    /// Position size in contracts (positive = long, negative = short)
    #[serde(deserialize_with = "string_decimal")]
    pub amount: Decimal,
    /// Average entry price
    #[serde(deserialize_with = "string_decimal")]
    pub entry_price: Decimal,
    /// Margin posted for this position
    #[serde(deserialize_with = "string_decimal")]
    pub margin_posted: Decimal,
    /// Realized profit/loss from closed positions
    #[serde(deserialize_with = "string_decimal")]
    pub realized_pnl: Decimal,
    /// Unrealized profit/loss from mark-to-market
    #[serde(deserialize_with = "string_decimal")]
    pub unrealized_pnl: Decimal,
    /// Notional value of position
    #[serde(default, deserialize_with = "option_string_decimal_default")]
    pub notional_value: Decimal,
    /// Required maintenance margin (deprecated, use portfolio-level margin)
    #[serde(default, deserialize_with = "option_string_decimal_default")]
    pub maintenance_margin: Decimal,
    /// Liquidation price (deprecated, use portfolio-level SPAN)
    #[serde(default, deserialize_with = "option_string_decimal_default")]
    pub liquidation_price: Decimal,
    /// Current margin ratio
    #[serde(default, deserialize_with = "option_string_decimal_default")]
    pub margin_ratio: Decimal,
}

/// Portfolio response from the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioResponse {
    /// Account wallet address
    pub wallet_address: WalletAddress,
    /// List of positions
    #[serde(default)]
    pub positions: Vec<PortfolioPosition>,
    /// Total margin used (position IM + open orders IM)
    #[serde(deserialize_with = "string_decimal")]
    pub total_margin_used: Decimal,
    /// Available balance for new positions
    #[serde(deserialize_with = "string_decimal")]
    pub available_balance: Decimal,
    /// Account.sol USDC currently eligible for a direct PM withdrawal.
    #[serde(default, deserialize_with = "option_string_decimal")]
    pub withdrawable_usdc: Option<Decimal>,
    /// Source timestamp of the authoritative Hydromancer portfolio snapshot.
    #[serde(default)]
    pub portfolio_snapshot_timestamp_ms: Option<u64>,
    /// Margin mode: "standard" or "portfolio"
    #[serde(default = "default_margin_mode")]
    pub margin_mode: String,
    /// Unified margin summary
    pub margin_summary: Option<MarginSummary>,
}

fn default_margin_mode() -> String {
    "standard".to_string()
}

impl PortfolioResponse {
    /// Get equity from margin summary.
    pub fn equity(&self) -> Option<Decimal> {
        self.margin_summary.as_ref().map(|m| m.equity)
    }

    /// Get maintenance margin required from margin summary.
    pub fn maintenance_margin_required(&self) -> Option<Decimal> {
        self.margin_summary
            .as_ref()
            .map(|m| m.maintenance_margin_required())
    }

    /// Calculate margin utilization (0-1 scale).
    pub fn margin_utilization(&self) -> Option<Decimal> {
        self.margin_summary.as_ref().map(|m| m.margin_utilization())
    }

    /// Get position IM from margin summary.
    pub fn position_im(&self) -> Option<Decimal> {
        self.margin_summary.as_ref().map(|m| m.position_im)
    }

    /// Get open orders IM from margin summary.
    pub fn open_orders_im(&self) -> Option<Decimal> {
        self.margin_summary.as_ref().map(|m| m.open_orders_im)
    }
}

/// Deserialize a string as Decimal, defaulting to zero if missing or null.
fn option_string_decimal_default<'de, D>(deserializer: D) -> Result<Decimal, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de;

    struct OptStringDecimalVisitor;

    impl<'de> de::Visitor<'de> for OptStringDecimalVisitor {
        type Value = Decimal;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string representing Decimal or null")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Decimal::ZERO)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Decimal::ZERO)
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            v.parse::<Decimal>().map_err(de::Error::custom)
        }

        fn visit_some<D2>(self, deserializer: D2) -> Result<Self::Value, D2::Error>
        where
            D2: Deserializer<'de>,
        {
            deserializer.deserialize_str(OptStringDecimalVisitor)
        }
    }

    deserializer.deserialize_any(OptStringDecimalVisitor)
}

// RFQ Response Types

/// Response for an RFQ quote leg.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RfqQuoteLegResponse {
    pub instrument: String,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
}

/// A single quote from a QP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RfqQuoteResponse {
    pub quote_id: String,
    pub net_premium: Decimal,
    pub legs: Vec<RfqQuoteLegResponse>,
    pub expires_at: u64,
}

/// RFQ leg in status response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RfqLegResponse {
    pub instrument: String,
    pub side: Side,
    pub size: Decimal,
}

/// Full RFQ status response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RfqStatusResponse {
    pub rfq_id: String,
    pub status: RfqStatus,
    pub underlying: String,
    pub legs: Vec<RfqLegResponse>,
    pub quotes: Vec<RfqQuoteResponse>,
    pub created_at: u64,
    pub expires_at: u64,
}

/// Response after accepting an RFQ quote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RfqAcceptResponse {
    pub rfq_id: String,
    pub quote_id: String,
    pub status: RfqStatus,
    pub fill_id: String,
}

/// RFQ history response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RfqHistoryResponse {
    pub rfqs: Vec<RfqStatusResponse>,
}

// Backward-compatible re-exports. These types were moved to api_models as the
// canonical Decimal-based definitions. Code that imported them from responses
// still compiles via these aliases.
pub use crate::api_models::Instrument;
pub use crate::api_models::MarketInfo;
pub use crate::api_models::MarketsResponse;
