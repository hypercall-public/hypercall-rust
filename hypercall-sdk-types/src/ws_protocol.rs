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
    /// Portfolio update (authenticated)
    #[cfg_attr(feature = "schemars", schemars(with = "serde_json::Value"))]
    PortfolioUpdate(PortfolioUpdate),
    /// Position expiry notification (authenticated)
    PositionExpired(WsPositionExpired),
    /// Liquidation state change (authenticated)
    LiquidationStateChange(WsLiquidationStateChange),
    /// Identify the connection with a wallet address (replaces query-param ?wallet=).
    Authenticate { wallet: String },
    /// Server confirms wallet identification
    Authenticated { wallet: String },
    /// Error message from server
    Error { message: String },
    /// Subscription confirmed
    Subscribed { channel: String },
    /// Unsubscription confirmed
    Unsubscribed { channel: String },
    /// Indicative market data from aggregated QP quotes (public)
    IndicativeMarketData(WsIndicativeMarketData),
    /// RFQ quotes available for taker (authenticated)
    RfqQuotes(WsRfqQuotes),
    /// RFQ status update (authenticated)
    RfqStatusUpdate(WsRfqStatusUpdate),
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
    #[serde(other)]
    Other,
}

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
#[allow(clippy::large_enum_variant)]
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
