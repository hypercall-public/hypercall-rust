//! # Hypercall Client
//!
//! Rust SDK for trading on Hypercall public HTTP and websocket APIs.
//!
//! ## Quick Start: Place an Options Order
//!
//! ```rust,no_run
//! use hypercall_client::{HypercallClient, HypercallWallet};
//! use hypercall_sdk_types::{Side, TimeInForce};
//! use rust_decimal::Decimal;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let api = HypercallClient::new("https://api.hypercall.xyz");
//! let wallet = HypercallWallet::from_private_key("0xYOUR_PRIVATE_KEY", 999)?;
//!
//! // Place a BTC call buy (options use typed args)
//! let resp = api.place_order(&wallet, "BTC-20260501-76000-C", Side::Buy, Decimal::new(2000, 0), Decimal::new(5, 0), TimeInForce::IOC).await?;
//! println!("Order ID: {:?}", resp);
//! # Ok(())
//! # }
//! ```
//!
//! ## Crate Features
//!
pub mod account;
pub mod api;
pub mod error;
pub mod funding;
pub mod indicative;
pub mod perp;
pub mod qp_client;
pub mod qp_scoped;
pub mod rfq;
#[cfg(feature = "rfq-v2")]
pub mod rfq_v2;
#[cfg(feature = "rfq-v2")]
pub mod rfq_v2_recovery;
#[cfg(feature = "rfq-v2")]
pub mod rfq_v2_signing;
pub mod tee_perp;
pub mod wallet;
pub mod websocket;

#[cfg(test)]
mod test_support;

pub use account::{CreateAccountParams, CreateAccountResult};
pub use api::{
    BulkOrderParams, BulkReplaceOrderParams, HypercallClient, OrderDecimalInput, OrderOptions,
    PlaceOrderParams, PublicLiquidationsQuery, ReplaceOrderParams, StandardMarginLiquidationParams,
};
pub use error::{ClientError, Result};
pub use funding::{DepositUsdcParams, DepositUsdcResult, WithdrawUsdcParams};
pub use hypercall_sdk_types::ws_protocol::WsMessage;
pub use hypercall_sdk_types::{CursorPage, LiquidationHistoryEntry, PublicLiquidationsResponse};
pub use hypercall_ws_protocol::{
    WsDeliveryClass, WsPressureCause, WsRecoveryAction, WsSlowConsumerCloseReason,
};
pub use perp::{
    PerpCancelByCloidParams, PerpCancelByOidParams, PerpLimitOrderParams,
    SetAccountAbstractionParams, UpdateApiWalletParams,
};
pub use qp_client::{
    NoopCallbacks, QpClientCallbacks, QpClientConfig, QpDisconnectReason, QpWriteFailure,
    QpWriteOperation, RfqDropReason,
};
pub use tee_perp::{
    build_perp_scale_order_legs, build_tee_perp_batch_modify, build_tee_perp_bracket_batch,
    build_tee_perp_position_tpsl_batch, build_tee_perp_scale_batch, perp_market_limit_price,
    perp_preview_batch_action, perp_preview_order, resolve_hypercall_builder_address,
    round_perp_price_to_tick, tee_api_wallet_approval_progress, tee_perp_batch_cloid,
    tee_perp_batch_modify_signing_hash, tee_perp_limit_order_wire, tee_perp_modify_signing_hash,
    tee_perp_nonce_in_window, tee_perp_trigger_order_wire, tee_perp_twap_cancel_signing_hash,
    tee_perp_twap_signing_hash, tee_perp_venue_expires_after_ms, ApproveHypercallBuilderParams,
    HyperliquidInfoClient, HyperliquidLeverage, HyperliquidUserRole, PerpScaleLeg,
    PerpScaleOrderSpec, TeeApiWalletApprovalChallenge, TeeApiWalletApprovalProgress,
    TeeApiWalletApprovalSignature, TeeApiWalletEnrollment, TeeApiWalletEnrollmentPolling,
    TeeApiWalletManagerSigner, TeePerpBatchModifyParams, TeePerpBatchModifySignature,
    TeePerpBracketEntry, TeePerpBracketOrderParams, TeePerpExit, TeePerpLeverageParams,
    TeePerpLeverageSignature, TeePerpMarketOrderParams, TeePerpModifyParams,
    TeePerpModifySignature, TeePerpOrderBatchParams, TeePerpOrderBatchSignature,
    TeePerpOrderReplacement, TeePerpPositionTpslParams, TeePerpScaleOrderParams, TeePerpSigner,
    TeePerpTriggerExecution, TeePerpTriggerOrderParams, TeePerpTriggerOrderSignature,
    TeePerpTwapCancelParams, TeePerpTwapCancelSignature, TeePerpTwapParams, TeePerpTwapSignature,
    PERP_MARKET_SLIPPAGE_BPS,
};
pub use wallet::{
    AccountAddress, ApiWalletDirectiveSigner, AtomicNonceProvider, CancelOrderSignature,
    HypercallSigner, HypercallWallet, ManagerDirectiveSigner, NonceProvider,
    PerpCancelByCloidSignature, PerpCancelByOidSignature, PerpDirectiveSigner,
    PerpLimitOrderSignature, PlaceOrderSignature, PortfolioUsdcWithdrawalSignature,
    ReplaceOrderSignature, SetAccountAbstractionSignature, StandardMarginLiquidationSignature,
    StandardUsdcWithdrawalSignature, UpdateApiWalletSignature, UsdcWithdrawalSigner,
};
pub use websocket::{
    WsClient, WsClientConfig, WsConnectionState, WsDisconnectReason, WsRecoveryPlan,
};

// Re-export commonly needed types from hypercall-sdk-types
pub use hypercall_sdk_types::{
    AcceptRfqRequest, ApiResponse, BulkCancelOrderResponse, BulkPlaceOrderResponse,
    CancelOrderRequest, CanonicalOrdersResponse, DirectiveDeliveryStatus, DirectiveDomainStatus,
    DirectiveStage, DirectiveStatusResponse, DirectiveSubmitRequest, DirectiveSubmitResponse,
    ExchangeInfoResponse, Fill, FillApiResponse, FillsResponse, HcUpdateApiWalletAction,
    HistoricalPnlInterval, HistoricalPnlPoint, HistoricalPnlResponse, HistoricalTheoInterval,
    HistoricalTheoPoint, HistoricalTheoResponse, HlSetAbstractionAction,
    HypercoreAccountAbstraction, InstrumentKind, InstrumentSpecResponse, MarginMode, MarginSummary,
    Market, OptionType, Order, OrderInfo, OrderMessage, OrderStatus, OrdersApiResponse,
    PerpCancelByCloidRequest, PerpCancelByOidRequest, PerpLimitOrderRequest, PerpTimeInForce,
    PlaceOrderRequest, Portfolio, PortfolioPosition, PortfolioResponse,
    PortfolioUsdcWithdrawalResponse, RfqLegRequest, SetAccountAbstractionRequest, Side,
    SpanMarginSummary, StandardMarginLiquidationOrderResponse,
    StandardMarginLiquidationPositionRequest, StandardUsdcWithdrawalResponse, SubmitRfqRequest,
    TimeInForce, UsdcWithdrawalDomainStatus,
};
pub use hypercall_sdk_types::{
    PerpLiquidationBinding, PerpPreviewAction, PerpPreviewDecision, PerpPreviewLeverageSetting,
    PerpPreviewLimit, PerpPreviewMargin, PerpPreviewMaxSize, PerpPreviewOrder,
    PerpPreviewOrderType, PerpPreviewRequest, PerpPreviewResponse, PerpPreviewTif,
    PerpPreviewTrigger, PerpRejectionReasonCode, TeeApiWalletApprovalPackage,
    TeePerpCancelResponse, TeePerpErrorCode, TeePerpLeverageAction, TeePerpLeverageResponse,
    TeePerpMarketEntry, TeePerpMarketsResponse, TeePerpModifyVenueOrder, TeePerpOrderBatchAction,
    TeePerpOrderBatchResponse, TeePerpOrderGrouping, TeePerpOrderResponse, TeePerpOrderStage,
    TeePerpOrderType, TeePerpOrderWire, TeePerpTpsl, TeePerpVenueActionResponse,
};
pub use hypercall_sdk_types::{
    TeePerpBatchModifyAction, TeePerpBatchModifyChildResponse, TeePerpBatchModifyResponse,
    TeePerpModifyAction, TeePerpModifyResponse, TeePerpTwapAction, TeePerpTwapCancelAction,
    TeePerpTwapResponse, DEFAULT_TEE_PERP_BATCH_MODIFY_MAX_CHILDREN,
    MAX_TEE_PERP_BATCH_MODIFY_CHILDREN, TEE_PERP_TWAP_MAX_MINUTES, TEE_PERP_TWAP_MIN_MINUTES,
};
