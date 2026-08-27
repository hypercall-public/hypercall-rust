pub mod api_models;
pub mod directives;
pub mod enums;
pub mod requests;
pub mod responses;
pub mod wallet_address;
pub mod ws_protocol;

pub use api_models::{
    ApiResponse as CanonicalApiResponse, ExchangeInfoResponse, FillApiResponse, FillsResponse,
    Instrument, MarketInfo, MarketsResponse, Order, OrdersApiResponse,
    OrdersResponse as CanonicalOrdersResponse, Portfolio, Position, PositionWithMetrics,
    SigningDomainInfo, SpanMarginSummary,
};
pub use directives::{
    DirectiveDeliveryStatus, DirectiveDomainStatus, DirectiveFill, DirectiveRejection,
    DirectiveStage, DirectiveStatusResponse, DirectiveSubmitRequest, DirectiveSubmitResponse,
    HcUpdateApiWalletAction, HlCancelByCloidAction, HlCancelByOidAction, HlLimitOrderAction,
    HlSetAbstractionAction, HypercoreAccountAbstraction, PerpCancelByCloidRequest,
    PerpCancelByOidRequest, PerpLimitOrderRequest, SetAccountAbstractionRequest,
    UpdateApiWalletRequest,
};
pub use enums::{
    FillSource, InstrumentKind, MarginMode, MarketAction, MarketUpdateStatus, OptionType,
    OrderAction, OrderRoute, OrderStatus, OrderUpdateStatus, ParseSdkEnumError, PerpTimeInForce,
    QpStatus, RfqStatus, Side, TimeInForce, TradeSide, TradingModes, TransactionStatus,
};
pub use requests::{
    AcceptRfqRequest, ApproveAgentRequest, BulkCancelOrderRequest, BulkPlaceOrderRequest,
    CancelOrderByClientIdRequest, CancelOrderByCloidRequest, CancelOrderRequest, PlaceOrderRequest,
    ReplaceOrderRequest, RevokeAgentRequest, RevokeAllAgentsRequest, RfqLegRequest,
    SetMarginModeRequest, StandardMarginLiquidationOrderRequest,
    StandardMarginLiquidationPositionRequest, SubmitRfqRequest,
};
pub use responses::{
    ApiResponse, ApproveAgentResponse, AuthorizedAgentsResponse, BulkCancelOrderResponse,
    BulkOrderResult, BulkPlaceOrderResponse, CursorPage, Fill, FullLiquidationStatusData,
    HistoricalPnlInterval, HistoricalPnlPoint, HistoricalPnlResponse, HistoricalTheoInterval,
    HistoricalTheoPoint, HistoricalTheoResponse, InstrumentResponse, InstrumentSpecResponse,
    JsonRpcError, JsonRpcResponse, L2Message, L2Update, LiquidationHistoryEntry,
    LiquidationStatusData, LiquidationStatusResponse, MarginSummary, Market, MarketResponse,
    MarketUpdateMessage, OptionGreeks, OptionSummary, OrderBookGreeks, OrderBookResponse,
    OrderBookStats, OrderInfo, OrderMessage, OrderUpdateMessage, OrderbookUpdate, OrdersResponse,
    Pagination, PartialLiquidationStatusData, PortfolioPosition, PortfolioResponse,
    PublicLiquidationsResponse, RevokeAgentResponse, RevokeAllAgentsResponse, RfqAcceptResponse,
    RfqHistoryResponse, RfqLegResponse, RfqQuoteLegResponse, RfqQuoteResponse, RfqStatusResponse,
    StandardMarginLiquidationOrderResponse, TickSizeStep, TradeMessage,
    HISTORICAL_PNL_INTERVAL_1D_MS, HISTORICAL_PNL_INTERVAL_1H_MS, HISTORICAL_PNL_INTERVAL_5M_MS,
    HISTORICAL_THEO_INTERVAL_1D_MS, HISTORICAL_THEO_INTERVAL_1H_MS, HISTORICAL_THEO_INTERVAL_5M_MS,
};
pub use wallet_address::WalletAddress;
pub use ws_protocol::*;

pub const RFQ_SELF_TRADE_REJECTION_REASON: &str =
    "Self-trade prevention: taker wallet equals quote provider wallet";

#[cfg(any(test, feature = "test-utils"))]
pub use wallet_address::test_wallet;
