use crate::api_models::{
    OptionsChainStrikeRow, PortfolioGreeksAggregate, PositionGreeksLeg, PositionWithMetrics,
    SpanMarginSummary,
};
use crate::{InstrumentKind, MarginSummary, ParseSdkEnumError, Side, WalletAddress};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsOrderRequest {
    pub price: Decimal,
    pub size: Decimal,
    pub symbol: String,
    pub side: Side,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tif: Option<crate::TimeInForce>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsOrderMessage {
    pub order_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    pub request: WsOrderRequest,
    pub status: crate::OrderUpdateStatus,
    /// Cumulative quantity filled across the order's executions, not the latest fill delta.
    pub filled_size: Option<Decimal>,
    pub timestamp: u64,
    pub reason: Option<String>,
    pub wallet_address: WalletAddress,
    pub instrument_type: String,
}

impl WsOrderMessage {
    pub fn instrument_kind(&self) -> Result<InstrumentKind, ParseSdkEnumError> {
        self.instrument_type.parse()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(tag = "type")]
#[cfg_attr(feature = "asyncapi", derive(asyncapi_rust::ToAsyncApiMessage))]
pub enum WsMessage {
    /// Request an API-clock sample for transport-delay upper bounds.
    ClockSync { nonce: String },
    /// API-clock sample returned on the same WebSocket connection.
    ClockSynced { nonce: String, server_at: u64 },
    /// Subscribe to a data channel
    Subscribe {
        channel: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        symbols: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expiry: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        option_type: Option<String>,
    },
    /// Unsubscribe from a data channel
    Unsubscribe {
        channel: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        symbols: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expiry: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        option_type: Option<String>,
    },
    /// Order status update (authenticated)
    #[cfg_attr(feature = "schemars", schemars(with = "serde_json::Value"))]
    OrderUpdate(WsOrderMessage),
    /// Opt-in private order lifecycle with opaque IDs and native combo parents.
    ExperimentalOrderUpdate(WsExperimentalOrderUpdate),
    /// Opt-in public package grouping; real legs remain in Trade.
    ComboTrade(WsComboTrade),
    /// Opt-in private package grouping, scoped to the authenticated wallet.
    ComboFill(WsComboFill),
    /// L2 orderbook snapshot/update
    OrderbookUpdate(WsOrderbookUpdate),
    /// Fill notification (authenticated)
    Fill(WsFillUpdate),
    /// Public trade event
    Trade(WsTradeUpdate),
    /// Market listing change
    MarketUpdate(WsMarketUpdate),
    /// Incremental options chain update
    OptionsChainUpdate(WsOptionsChainUpdate),
    /// Real-time index/spot price update for all underlyings
    IndexPriceUpdate(WsIndexPriceUpdate),
    /// Position expiry notification (authenticated)
    PositionExpired(WsPositionExpired),
    /// Liquidation state change (authenticated)
    LiquidationStateChange(WsLiquidationStateChange),
    /// Identify the connection with a wallet address (replaces query-param ?wallet=).
    Authenticate { wallet: String },
    #[cfg(feature = "rfq-v2")]
    /// Bind the RFQ V2 channel to a signed private-read bearer.
    AuthenticateRfqV2 { token: String },
    /// Server confirms wallet identification
    Authenticated { wallet: String },
    /// Server confirms the verified RFQ V2 account.
    #[cfg(feature = "rfq-v2")]
    RfqV2Authenticated {
        #[cfg_attr(feature = "schemars", schemars(with = "String"))]
        account: WalletAddress,
    },
    /// Error message from server
    Error {
        message: String,
        /// Stable machine-readable reason, when the error has one, for
        /// example `unknown_channel`. Match on this, not on `message`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        /// The channel a rejected `Subscribe` named.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<String>,
    },
    /// Subscription confirmed
    Subscribed {
        channel: String,
        /// `true` when the channel is delivered in batch frames, which today
        /// means `indicative_market_data_v2`. Servers that predate the v2
        /// channel accept any channel name and confirm it without this
        /// field, so a client must treat a confirmation without
        /// `batch: true` as "v2 unsupported" and fall back to
        /// `indicative_market_data`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        batch: Option<bool>,
    },
    /// Unsubscription confirmed
    Unsubscribed { channel: String },
    /// Indicative market data from aggregated QP quotes (public)
    IndicativeMarketData(WsIndicativeMarketData),
    /// Several `IndicativeMarketData` rows coalesced into one frame. Sent on
    /// the `indicative_market_data_v2` channel (public)
    IndicativeMarketDataBatch(WsIndicativeMarketDataBatch),
    /// RFQ quotes available for taker (authenticated)
    RfqQuotes(WsRfqQuotes),
    /// RFQ status update (authenticated)
    RfqStatusUpdate(WsRfqStatusUpdate),
    #[cfg(feature = "rfq-v2")]
    /// Opt-in RFQ V2 change hint; read the change feed through private reads.
    RfqV2Changed(WsRfqV2Changed),
    /// Submit an RFQ request via WebSocket (authenticated)
    SubmitRfq {
        rfq_id: String,
        legs: Vec<WsRfqLegRequest>,
        wallet_address: String,
        nonce: u64,
        signature: String,
    },
    /// Submit an RFQ with auto-execute via WebSocket (authenticated).
    /// The taker pre-authorizes execution with a directional `limit_price`.
    SubmitAutoExecuteRfq {
        rfq_id: String,
        legs: Vec<WsRfqLegRequest>,
        wallet_address: String,
        /// Directional premium limit as a decimal string. Buy RFQs use this
        /// as a max debit. Sell RFQs use it as a min credit.
        limit_price: String,
        nonce: u64,
        signature: String,
    },
    /// Accept an RFQ quote via WebSocket (authenticated)
    AcceptRfqQuote {
        rfq_id: String,
        quote_id: String,
        wallet_address: String,
        nonce: u64,
        signature: String,
    },
    /// RFQ accept result pushed back to the client
    RfqAcceptResult {
        rfq_id: String,
        quote_id: String,
        status: String,
        fill_id: Option<String>,
        /// Used for wallet-based filtering in `publish_to_channel` so
        /// the result is only delivered to the taker, not all rfq
        /// subscribers. Excluded from the wire format.
        #[serde(skip, default)]
        #[cfg_attr(feature = "schemars", schemars(skip))]
        taker_wallet: Option<WalletAddress>,
    },
    /// Place an order via WebSocket (authenticated)
    PlaceOrder {
        wallet: String,
        symbol: String,
        side: String,
        size: String,
        price: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tif: Option<String>,
        /// Optional route during the deprecation window.
        /// Omitted route keeps the legacy WebSocket orderbook path for now. It will not be
        /// required before July 4, 2026, but may become required later.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        route: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
        nonce: u64,
        signature: String,
        #[serde(default, skip_serializing_if = "crate::requests::is_false")]
        reduce_only: bool,
        #[serde(default)]
        mmp_enabled: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        builder_code_address: Option<String>,
    },
    /// Order placement result pushed back to the client
    OrderResult(WsOrderResult),
    /// Portfolio update (authenticated). The frame's only `type` is the
    /// update kind: `Initial`, `PositionUpdate`, `BalanceUpdate`,
    /// `MarginUpdate`, or `GreeksUpdate`.
    #[serde(untagged)]
    #[cfg_attr(feature = "schemars", schemars(with = "serde_json::Value"))]
    PortfolioUpdate(PortfolioUpdate),
    /// Any frame whose `type` is not one of `KNOWN_WS_MESSAGE_TAGS`, so
    /// newer servers can add message types. Serializes as `{"type":"Other"}`.
    #[serde(
        untagged,
        deserialize_with = "deserialize_unknown_message",
        serialize_with = "serialize_unknown_message"
    )]
    #[cfg_attr(feature = "schemars", schemars(with = "UnknownMessageSchema"))]
    Other,
}

/// Schema of the `{"type":"Other"}` form that [`WsMessage::Other`] serializes to.
#[cfg(feature = "schemars")]
#[derive(schemars::JsonSchema)]
#[expect(dead_code, reason = "schema-only type, never constructed")]
struct UnknownMessageSchema {
    #[serde(rename = "type")]
    tag: UnknownMessageTag,
}

#[cfg(feature = "schemars")]
#[derive(schemars::JsonSchema)]
#[expect(dead_code, reason = "schema-only type, never constructed")]
enum UnknownMessageTag {
    Other,
}

fn serialize_unknown_message<S>(serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(Some(1))?;
    map.serialize_entry("type", "Other")?;
    map.end()
}

/// Accept a frame as [`WsMessage::Other`] only when its `type` is unknown.
/// A malformed frame with a known `type` stays a decode error.
fn deserialize_unknown_message<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Tagged {
        #[serde(rename = "type")]
        tag: String,
    }
    let Tagged { tag } = Tagged::deserialize(deserializer)?;
    if KNOWN_WS_MESSAGE_TAGS.contains(&tag.as_str()) {
        return Err(serde::de::Error::custom(format!(
            "malformed `{tag}` WebSocket message"
        )));
    }
    Ok(())
}

/// Wire `type` values of [`PortfolioUpdate`] frames.
pub const PORTFOLIO_UPDATE_KINDS: &[&str] = &[
    "Initial",
    "PositionUpdate",
    "BalanceUpdate",
    "MarginUpdate",
    "GreeksUpdate",
];

/// Every `type` value a [`WsMessage`] variant other than `Other` can carry.
/// A frame with one of these tags that fails to decode is an error, never
/// [`WsMessage::Other`].
const KNOWN_WS_MESSAGE_TAGS: &[&str] = &[
    "ClockSync",
    "ClockSynced",
    "Subscribe",
    "Unsubscribe",
    "OrderUpdate",
    "ExperimentalOrderUpdate",
    "ComboTrade",
    "ComboFill",
    "OrderbookUpdate",
    "Fill",
    "Trade",
    "MarketUpdate",
    "OptionsChainUpdate",
    "IndexPriceUpdate",
    "Initial",
    "PositionUpdate",
    "BalanceUpdate",
    "MarginUpdate",
    "GreeksUpdate",
    "PositionExpired",
    "LiquidationStateChange",
    "Authenticate",
    "Authenticated",
    "Error",
    "Subscribed",
    "Unsubscribed",
    "IndicativeMarketData",
    "IndicativeMarketDataBatch",
    "RfqQuotes",
    "RfqStatusUpdate",
    #[cfg(feature = "rfq-v2")]
    "AuthenticateRfqV2",
    #[cfg(feature = "rfq-v2")]
    "RfqV2Authenticated",
    #[cfg(feature = "rfq-v2")]
    "RfqV2Changed",
    "SubmitRfq",
    "SubmitAutoExecuteRfq",
    "AcceptRfqQuote",
    "RfqAcceptResult",
    "PlaceOrder",
    "OrderResult",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsOrderResult {
    pub order_id: Option<u64>,
    pub status: String,
    pub symbol: String,
    pub side: String,
    pub price: String,
    pub size: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsIndicativeMarketData {
    pub instrument: String,
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_bid: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bid_iv: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask_iv: Option<f64>,
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_ask: Option<Decimal>,
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indicative_bid_size: Option<Decimal>,
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indicative_ask_size: Option<Decimal>,
    pub num_providers: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rfq_provider_quotes: Option<Vec<RfqProviderIndicativeQuote>>,
    /// API-clock timestamp when this WebSocket payload was published.
    ///
    /// Consumers must use this timestamp, rather than their local wall clock,
    /// to measure the age of provider quotes in `rfq_provider_quotes`.
    pub published_at: u64,
    pub timestamp: u64,
}

/// Batched `indicative_market_data` delivery.
///
/// `updates` holds the rows published during one batch window, oldest first,
/// each with the same fields as an `IndicativeMarketData` frame minus `type`.
/// Applying the rows in order leaves the same per-instrument state as
/// receiving them one frame at a time. Rows are never conflated, so one
/// instrument can appear more than once in a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsIndicativeMarketDataBatch {
    /// API-clock timestamp in milliseconds when the batch was flushed. The
    /// batching delay of a row is `flushed_at - published_at`.
    pub flushed_at: u64,
    pub updates: Vec<WsIndicativeMarketData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct RfqProviderIndicativeQuote {
    #[cfg_attr(feature = "utoipa", schema(value_type = String))]
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub wallet: WalletAddress,
    #[cfg_attr(feature = "utoipa", schema(value_type = String))]
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub bid_price: Decimal,
    #[cfg_attr(feature = "utoipa", schema(value_type = String))]
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub ask_price: Decimal,
    #[cfg_attr(feature = "utoipa", schema(value_type = String))]
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub max_bid_size: Decimal,
    #[cfg_attr(feature = "utoipa", schema(value_type = String))]
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub max_ask_size: Decimal,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsRfqQuotes {
    pub rfq_id: String,
    pub quotes: Vec<WsRfqQuoteEntry>,
    pub status: String,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub taker_wallet: WalletAddress,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsRfqQuoteEntry {
    pub quote_id: String,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub net_premium: Decimal,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsRfqStatusUpdate {
    pub rfq_id: String,
    pub status: String,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub taker_wallet: WalletAddress,
}

/// A private RFQ V2 change hint. Reconnecting clients recover missed
/// records from the durable sequence through authorized reads.
#[cfg(feature = "rfq-v2")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsRfqV2Changed {
    pub rfq_id: String,
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsRfqLegRequest {
    pub instrument: String,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub side: Side,
    pub size: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsOrderbookUpdate {
    pub symbol: String,
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub option_token_address: Option<WalletAddress>,
    #[cfg_attr(feature = "schemars", schemars(with = "Vec<(String, String)>"))]
    pub bids: Vec<(Decimal, Decimal)>,
    #[cfg_attr(feature = "schemars", schemars(with = "Vec<(String, String)>"))]
    pub asks: Vec<(Decimal, Decimal)>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsFillUpdate {
    pub order_id: i64,
    pub fill_id: i64,
    pub symbol: String,
    pub side: String,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub price: Decimal,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub size: Decimal,
    pub timestamp: i64,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub wallet_address: WalletAddress,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub fee: Decimal,
    pub trade_id: i64,
    pub is_taker: bool,
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub builder_code_address: Option<WalletAddress>,
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub builder_code_fee: Option<Decimal>,
    pub instrument_type: String,
}

impl WsFillUpdate {
    pub fn instrument_kind(&self) -> Result<InstrumentKind, ParseSdkEnumError> {
        self.instrument_type.parse()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsTradeUpdate {
    pub symbol: String,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub price: Decimal,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub size: Decimal,
    pub side: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PortfolioUpdate {
    Initial {
        positions: Vec<PositionWithMetrics>,
        timestamp: i64,
    },
    PositionUpdate {
        position: PositionWithMetrics,
        timestamp: i64,
    },
    BalanceUpdate {
        total_margin_used: Decimal,
        timestamp: i64,
    },
    MarginUpdate {
        span_margin: SpanMarginSummary,
        margin_summary: MarginSummary,
        margin_mode: String,
        total_margin_used: Decimal,
        available_balance: Decimal,
        timestamp: i64,
    },
    GreeksUpdate {
        per_leg: Vec<PositionGreeksLeg>,
        aggregate: Option<PortfolioGreeksAggregate>,
        timestamp: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsPositionExpired {
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub wallet_address: WalletAddress,
    pub symbol: String,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub position_size: Decimal,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub settlement_price: Decimal,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub settlement_value: Decimal,
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub settlement_entry_price: Option<Decimal>,
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub cost_basis: Option<Decimal>,
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub net_pnl: Option<Decimal>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsLiquidationStateChange {
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub wallet_address: WalletAddress,
    pub previous_state: String,
    pub new_state: String,
    pub liquidation_mode: Option<String>,
    pub margin_mode: String,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub equity: Decimal,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub mm_required: Decimal,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub maintenance_margin: Decimal,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub shortfall: Decimal,
    pub partial_liquidation: Option<WsPartialLiquidationState>,
    pub full_liquidation: Option<WsFullLiquidationState>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsPartialLiquidationState {
    pub entered_at: i64,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub target_equity: Decimal,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub mm_shortfall: Decimal,
    pub escalation_deadline: i64,
    pub last_reprice_at: Option<i64>,
    pub active_order_request_ids: Vec<String>,
    pub active_order_client_ids: Vec<String>,
    pub bonus_bps: i32,
    pub pending_full_auction_id: Option<String>,
    pub pending_full_request_id: Option<String>,
    pub pending_full_tx_hash: Option<String>,
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub pending_full_margin_needed: Option<Decimal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsFullLiquidationState {
    pub auction_id: Option<String>,
    pub request_id: Option<String>,
    pub tx_hash: Option<String>,
    pub started_at: Option<i64>,
    pub chain_start_time: Option<i64>,
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub margin_needed: Option<Decimal>,
    pub stop_request_id: Option<String>,
    pub stop_tx_hash: Option<String>,
    pub liquidated_at: Option<i64>,
    pub winner: Option<String>,
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub bonus: Option<Decimal>,
    pub resolution_tx_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct IndexPriceEntry {
    pub underlying: String,
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub price: Decimal,
    /// Source observation time for this underlying, as Unix milliseconds.
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsIndexPriceUpdate {
    pub prices: Vec<IndexPriceEntry>,
    /// Oldest source observation time included in `prices`, as Unix milliseconds.
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(tag = "action")]
pub enum WsMarketUpdate {
    ComboCreated {
        symbol: String,
        combo_id: String,
        legs: Vec<WsComboLeg>,
        timestamp: u64,
    },
    Created {
        symbol: String,
        #[cfg_attr(feature = "schemars", schemars(with = "String"))]
        strike: Decimal,
        is_call: bool,
        underlying: String,
        expiry: u32,
        timestamp: u64,
    },
    Deleted {
        symbol: String,
        timestamp: u64,
    },
    Expired {
        symbol: String,
        #[cfg_attr(feature = "schemars", schemars(with = "String"))]
        strike: Decimal,
        is_call: bool,
        underlying: String,
        expiry: u32,
        timestamp: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(tag = "action")]
#[expect(
    clippy::large_enum_variant,
    reason = "boxing the upsert row would change the public serde type"
)]
pub enum WsOptionsChainUpdate {
    Upsert {
        currency: String,
        expiry: u64,
        row: OptionsChainStrikeRow,
        timestamp: i64,
    },
    Remove {
        currency: String,
        expiry: u64,
        strike: f64,
        option_type: String,
        symbol: String,
        timestamp: i64,
    },
}

/// Experimental catalogue legs use opaque IDs, never option-like fake symbols.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsComboLeg {
    pub instrument_id: String,
    /// Exact signed integer text, including ratios outside JS's safe range.
    pub ratio: String,
}

/// Non-additive grouping of the real leg trades. It carries no account data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsComboTrade {
    pub execution_id: String,
    pub symbol: String,
    pub quantity: String,
    pub net_price: String,
    pub leg_trade_ids: Vec<String>,
    pub timestamp: u64,
}

/// The account projection of one package execution. Every copy names exactly
/// one wallet and includes only that wallet's parent order reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsComboFill {
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub wallet_address: WalletAddress,
    pub order_id: String,
    pub side: String,
    pub execution: WsComboTrade,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct WsExperimentalOrderUpdate {
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub wallet_address: WalletAddress,
    pub order_id: Option<String>,
    pub symbol: String,
    pub instrument_type: String,
    pub status: String,
    pub quantity: String,
    pub filled_quantity: String,
    pub net_price: String,
    pub timestamp: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonic_rs::JsonValueTrait;

    #[test]
    fn ws_order_request_decodes_missing_tif() {
        let json = serde_json::json!({
            "price": "100000",
            "size": "0.5",
            "symbol": "BTC-PERP",
            "side": "Buy"
        });

        let request: WsOrderRequest = serde_json::from_value(json).unwrap();

        assert_eq!(request.tif, None);
    }

    #[test]
    fn canonical_websocket_rows_require_instrument_type() {
        let order = serde_json::json!({
            "order_id": 1,
            "request": {
                "price": "100000",
                "size": "0.5",
                "symbol": "BTC-PERP",
                "side": "Buy"
            },
            "status": "OPEN",
            "timestamp": 1,
            "reason": null,
            "wallet_address": "0x0000000000000000000000000000000000000001"
        });
        assert!(serde_json::from_value::<WsOrderMessage>(order).is_err());

        let fill = serde_json::json!({
            "order_id": 1,
            "fill_id": 2,
            "symbol": "BTC-PERP",
            "side": "Buy",
            "price": "100000",
            "size": "0.5",
            "timestamp": 1,
            "wallet_address": "0x0000000000000000000000000000000000000001",
            "fee": "1",
            "trade_id": 3,
            "is_taker": true,
            "builder_code_address": null,
            "builder_code_fee": null
        });
        assert!(serde_json::from_value::<WsFillUpdate>(fill).is_err());
    }

    #[test]
    fn websocket_clock_sync_round_trips() {
        let request = WsMessage::ClockSync {
            nonce: "analytics-worker".to_string(),
        };
        let response = WsMessage::ClockSynced {
            nonce: "analytics-worker".to_string(),
            server_at: 1_785_052_800_000,
        };

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "type": "ClockSync",
                "nonce": "analytics-worker"
            })
        );
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            serde_json::json!({
                "type": "ClockSynced",
                "nonce": "analytics-worker",
                "server_at": 1_785_052_800_000_u64
            })
        );
    }

    #[test]
    fn portfolio_update_wire_carries_only_the_update_kind() {
        let message = WsMessage::PortfolioUpdate(PortfolioUpdate::Initial {
            positions: Vec::new(),
            timestamp: 123,
        });
        let expected = r#"{"type":"Initial","positions":[],"timestamp":123}"#;

        assert_eq!(serde_json::to_string(&message).unwrap(), expected);
        assert_eq!(sonic_rs::to_string(&message).unwrap(), expected);

        let sonic_value: sonic_rs::Value = sonic_rs::from_str(expected).unwrap();
        assert_eq!(
            sonic_value.get("type").and_then(|value| value.as_str()),
            Some("Initial")
        );
        for decoded in [
            serde_json::from_str::<WsMessage>(expected).unwrap(),
            sonic_rs::from_str::<WsMessage>(expected).unwrap(),
        ] {
            assert!(matches!(
                decoded,
                WsMessage::PortfolioUpdate(PortfolioUpdate::Initial { timestamp: 123, .. })
            ));
        }
    }

    #[test]
    fn unknown_message_type_decodes_as_other_but_malformed_known_type_errors() {
        let unknown = r#"{"type":"FutureMessage","payload":1}"#;
        assert!(matches!(
            serde_json::from_str::<WsMessage>(unknown),
            Ok(WsMessage::Other)
        ));
        assert!(matches!(
            sonic_rs::from_str::<WsMessage>(unknown),
            Ok(WsMessage::Other)
        ));
        for malformed in [r#"{"type":"Subscribed"}"#, r#"{"type":"MarginUpdate"}"#] {
            assert!(serde_json::from_str::<WsMessage>(malformed).is_err());
            assert!(sonic_rs::from_str::<WsMessage>(malformed).is_err());
        }
    }
}
