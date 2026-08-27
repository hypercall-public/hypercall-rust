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
pub mod perp;
pub mod qp_client;
pub mod qp_scoped;
pub mod rfq;
pub mod wallet;
pub mod websocket;

pub use account::{CreateAccountParams, CreateAccountResult};
pub use api::{
    BulkOrderParams, BulkReplaceOrderParams, HypercallClient, OrderDecimalInput, OrderOptions,
    PlaceOrderParams, PublicLiquidationsQuery, ReplaceOrderParams, StandardMarginLiquidationParams,
};
pub use error::{ClientError, Result};
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
    QpWriteOperation,
};
pub use wallet::{
    AccountAddress, ApiWalletDirectiveSigner, AtomicNonceProvider, CancelOrderSignature,
    HypercallSigner, HypercallWallet, ManagerDirectiveSigner, NonceProvider,
    PerpCancelByCloidSignature, PerpCancelByOidSignature, PerpDirectiveSigner,
    PerpLimitOrderSignature, PlaceOrderSignature, ReplaceOrderSignature,
    SetAccountAbstractionSignature, StandardMarginLiquidationSignature, UpdateApiWalletSignature,
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
    PlaceOrderRequest, Portfolio, PortfolioPosition, PortfolioResponse, RfqLegRequest,
    SetAccountAbstractionRequest, Side, SpanMarginSummary, StandardMarginLiquidationOrderResponse,
    StandardMarginLiquidationPositionRequest, SubmitRfqRequest, TimeInForce,
};
