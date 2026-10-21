pub mod api_models;
pub mod directives;
pub mod enums;
pub mod requests;
pub mod responses;
#[cfg(feature = "rfq-v2")]
pub mod rfq_v2;
pub mod tee_perp;
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
    PortfolioUsdcWithdrawalResponse, PublicLiquidationsResponse, RevokeAgentResponse,
    RevokeAllAgentsResponse, RfqAcceptResponse, RfqHistoryResponse, RfqLegResponse,
    RfqQuoteLegResponse, RfqQuoteResponse, RfqStatusResponse, StandardMarginBreakdown,
    StandardMarginLiquidationOrderResponse, StandardOptionExpiryMargin,
    StandardUsdcWithdrawalResponse, TickSizeStep, TradeMessage, UsdcWithdrawalDomainStatus,
    HISTORICAL_PNL_INTERVAL_1D_MS, HISTORICAL_PNL_INTERVAL_1H_MS, HISTORICAL_PNL_INTERVAL_5M_MS,
    HISTORICAL_THEO_INTERVAL_1D_MS, HISTORICAL_THEO_INTERVAL_1H_MS, HISTORICAL_THEO_INTERVAL_5M_MS,
};
#[cfg(feature = "rfq-v2")]
pub use rfq_v2::*;
pub use tee_perp::{
    PerpLiquidationBinding, PerpPreviewAction, PerpPreviewDecision, PerpPreviewLeverageSetting,
    PerpPreviewLimit, PerpPreviewMargin, PerpPreviewMaxSize, PerpPreviewOrder,
    PerpPreviewOrderType, PerpPreviewRequest, PerpPreviewResponse, PerpPreviewTif,
    PerpPreviewTrigger, PerpRejectionReasonCode, TeeApiWalletApprovalPackage,
    TeeApiWalletApprovalRequest, TeeApiWalletApprovalSubmitRequest,
    TeePerpCancelByOidSubmitRequest, TeePerpCancelResponse, TeePerpCancelSubmitRequest,
    TeePerpErrorCode, TeePerpLeverageAction, TeePerpLeverageResponse, TeePerpLeverageSubmitRequest,
    TeePerpLimitOrderType, TeePerpMarketEntry, TeePerpMarketsResponse, TeePerpModifyVenueOrder,
    TeePerpOrderBatchAction, TeePerpOrderBatchChildResponse, TeePerpOrderBatchResponse,
    TeePerpOrderBatchSubmitRequest, TeePerpOrderGrouping, TeePerpOrderResponse, TeePerpOrderStage,
    TeePerpOrderSubmitRequest, TeePerpOrderType, TeePerpOrderWire, TeePerpTpsl,
    TeePerpTriggerOrderType, TeePerpTriggerSubmitRequest, TeePerpVenueActionResponse,
    VenueCompartmentRisk, VenueLiquidationBasis, VenueLiquidationRisk,
    MAX_TEE_PERP_ORDER_BATCH_CHILDREN, TEE_PERP_VENUE_TTL_MS,
};
pub use tee_perp::{
    TeePerpBatchModifyAction, TeePerpBatchModifyChildResponse, TeePerpBatchModifyResponse,
    TeePerpBatchModifySubmitRequest, TeePerpModifyAction, TeePerpModifyResponse,
    TeePerpModifySubmitRequest, TeePerpTwapAction, TeePerpTwapCancelAction,
    TeePerpTwapCancelSubmitRequest, TeePerpTwapResponse, TeePerpTwapSubmitRequest,
    DEFAULT_TEE_PERP_BATCH_MODIFY_MAX_CHILDREN, MAX_TEE_PERP_BATCH_MODIFY_CHILDREN,
    TEE_PERP_TWAP_MAX_MINUTES, TEE_PERP_TWAP_MIN_MINUTES,
};
pub use wallet_address::WalletAddress;
pub use ws_protocol::*;

pub const RFQ_SELF_TRADE_REJECTION_REASON: &str =
    "Self-trade prevention: taker wallet equals quote provider wallet";

#[cfg(any(test, feature = "test-utils"))]
pub use wallet_address::test_wallet;
