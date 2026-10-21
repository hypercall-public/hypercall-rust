//! Wire types for TEE perp trading (`/v1/tee-perp/*`) and TEE API wallet
//! enrollment (`/v1/tee-api-wallet/*`).
//!
//! Signed submissions reuse [`DirectiveSubmitRequest`] (`account`, `nonce`,
//! `action`, `signature`). Prices and sizes on the signed routes are
//! 1e8-scaled integers; the read-only preview takes plain decimals.
//!
//! [`DirectiveSubmitRequest`]: crate::DirectiveSubmitRequest

use alloy::primitives::B256;
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::directives::{
    deserialize_b256, deserialize_perp_tif, deserialize_u128, deserialize_u64, serialize_b256,
    serialize_perp_tif, serialize_safe_u128, serialize_safe_u64,
};
use crate::{DirectiveSubmitRequest, HlCancelByCloidAction, HlCancelByOidAction};
use crate::{HlLimitOrderAction, PerpTimeInForce, WalletAddress};

/// `POST /v1/tee-perp/orders/submit`: one plain limit order (GTC, IOC, ALO).
pub const TEE_PERP_ORDER_SUBMIT_ROUTE: &str = "/v1/tee-perp/orders/submit";
/// `POST /v1/tee-perp/orders/submit-trigger`: one native stop or take-profit order.
pub const TEE_PERP_TRIGGER_ORDER_SUBMIT_ROUTE: &str = "/v1/tee-perp/orders/submit-trigger";
/// `POST /v1/tee-perp/orders/submit-batch`: up to 100 orders under one grouping.
pub const TEE_PERP_ORDER_BATCH_SUBMIT_ROUTE: &str = "/v1/tee-perp/orders/submit-batch";
/// `POST /v1/tee-perp/cancels/submit`: cancel by client order ID.
pub const TEE_PERP_CANCEL_SUBMIT_ROUTE: &str = "/v1/tee-perp/cancels/submit";
/// `POST /v1/tee-perp/cancels/submit-by-oid`: cancel by Hyperliquid order ID.
pub const TEE_PERP_CANCEL_BY_OID_SUBMIT_ROUTE: &str = "/v1/tee-perp/cancels/submit-by-oid";
/// `POST /v1/tee-perp/leverage/submit`: leverage and margin-mode change.
pub const TEE_PERP_LEVERAGE_SUBMIT_ROUTE: &str = "/v1/tee-perp/leverage/submit";
/// `POST /v1/tee-perp/orders/modify`: replace one resting limit order
/// (Hyperliquid `modify`, sent as a one-entry `batchModify`).
pub const TEE_PERP_ORDER_MODIFY_ROUTE: &str = "/v1/tee-perp/orders/modify";
/// `POST /v1/tee-perp/orders/batch-modify`: replace several resting limit
/// orders under one signature and one venue nonce (Hyperliquid `batchModify`).
pub const TEE_PERP_ORDER_BATCH_MODIFY_ROUTE: &str = "/v1/tee-perp/orders/batch-modify";
/// `POST /v1/tee-perp/twap/submit`: start a native Hyperliquid TWAP.
pub const TEE_PERP_TWAP_SUBMIT_ROUTE: &str = "/v1/tee-perp/twap/submit";
/// `POST /v1/tee-perp/twap/cancel`: cancel a running native TWAP.
pub const TEE_PERP_TWAP_CANCEL_ROUTE: &str = "/v1/tee-perp/twap/cancel";
/// `POST /v1/tee-perp/preview`: read-only admission preview.
pub const TEE_PERP_PREVIEW_ROUTE: &str = "/v1/tee-perp/preview";
/// `GET /v1/tee-perp/markets`: configured TEE perp markets.
pub const TEE_PERP_MARKETS_ROUTE: &str = "/v1/tee-perp/markets";
/// `POST /v1/tee-api-wallet/approval-request`: allocate the TEE API wallet.
pub const TEE_API_WALLET_APPROVAL_REQUEST_ROUTE: &str = "/v1/tee-api-wallet/approval-request";
/// `POST /v1/tee-api-wallet/approval`: submit the manager's approval signature.
pub const TEE_API_WALLET_APPROVAL_ROUTE: &str = "/v1/tee-api-wallet/approval";

/// Hyperliquid accepts at most this many children in one order batch.
pub const MAX_TEE_PERP_ORDER_BATCH_CHILDREN: usize = 100;
/// Children a batch modify may carry on the wire (the enclave's structural
/// bound). No server instance accepts more.
pub const MAX_TEE_PERP_BATCH_MODIFY_CHILDREN: usize = 100;
/// Default per-instance batch-modify cap (`api.tee_perp_batch_modify_max_children`).
/// An instance may configure anything from 1 to
/// [`MAX_TEE_PERP_BATCH_MODIFY_CHILDREN`]; a larger batch is refused with
/// `400 invalid_field` before it is charged against the rate limit.
pub const DEFAULT_TEE_PERP_BATCH_MODIFY_MAX_CHILDREN: usize = 50;
/// Shortest native TWAP Hyperliquid runs, in minutes.
pub const TEE_PERP_TWAP_MIN_MINUTES: u32 = 5;
/// Longest native TWAP Hyperliquid runs, in minutes (24 hours).
pub const TEE_PERP_TWAP_MAX_MINUTES: u32 = 1_440;
/// The server derives the venue expiry as `nonce + TEE_PERP_VENUE_TTL_MS`.
pub const TEE_PERP_VENUE_TTL_MS: u64 = 9_000;
/// The derived venue expiry must fall within this window after server "now".
pub const TEE_PERP_MAX_VENUE_EXPIRY_WINDOW_MS: u64 = 10_000;

/// Signed plain limit order body for [`TEE_PERP_ORDER_SUBMIT_ROUTE`].
pub type TeePerpOrderSubmitRequest = DirectiveSubmitRequest<HlLimitOrderAction>;
/// Signed trigger order body for [`TEE_PERP_TRIGGER_ORDER_SUBMIT_ROUTE`].
pub type TeePerpTriggerSubmitRequest = DirectiveSubmitRequest<TeePerpOrderWire>;
/// Signed order batch body for [`TEE_PERP_ORDER_BATCH_SUBMIT_ROUTE`].
pub type TeePerpOrderBatchSubmitRequest = DirectiveSubmitRequest<TeePerpOrderBatchAction>;
/// Signed cancel-by-cloid body for [`TEE_PERP_CANCEL_SUBMIT_ROUTE`].
pub type TeePerpCancelSubmitRequest = DirectiveSubmitRequest<HlCancelByCloidAction>;
/// Signed cancel-by-oid body for [`TEE_PERP_CANCEL_BY_OID_SUBMIT_ROUTE`].
pub type TeePerpCancelByOidSubmitRequest = DirectiveSubmitRequest<HlCancelByOidAction>;
/// Signed leverage body for [`TEE_PERP_LEVERAGE_SUBMIT_ROUTE`].
pub type TeePerpLeverageSubmitRequest = DirectiveSubmitRequest<TeePerpLeverageAction>;
/// Signed single-modify body for [`TEE_PERP_ORDER_MODIFY_ROUTE`].
pub type TeePerpModifySubmitRequest = DirectiveSubmitRequest<TeePerpModifyAction>;
/// Signed batch-modify body for [`TEE_PERP_ORDER_BATCH_MODIFY_ROUTE`].
pub type TeePerpBatchModifySubmitRequest = DirectiveSubmitRequest<TeePerpBatchModifyAction>;
/// Signed TWAP body for [`TEE_PERP_TWAP_SUBMIT_ROUTE`].
pub type TeePerpTwapSubmitRequest = DirectiveSubmitRequest<TeePerpTwapAction>;
/// Signed TWAP-cancel body for [`TEE_PERP_TWAP_CANCEL_ROUTE`].
pub type TeePerpTwapCancelSubmitRequest = DirectiveSubmitRequest<TeePerpTwapCancelAction>;

/// Trigger direction, as Hyperliquid names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TeePerpTpsl {
    /// Take profit.
    Tp,
    /// Stop loss.
    Sl,
}

impl TeePerpTpsl {
    /// The `tpsl` byte the app key signs: 1 = take profit, 2 = stop loss.
    pub const fn signed(self) -> u8 {
        match self {
            Self::Tp => 1,
            Self::Sl => 2,
        }
    }
}

/// Limit order type of one [`TeePerpOrderWire`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeePerpLimitOrderType {
    /// Hypercall TIF byte on the wire: 1 = ALO, 2 = GTC, 3 = IOC.
    #[serde(
        rename = "encodedTif",
        serialize_with = "serialize_perp_tif",
        deserialize_with = "deserialize_perp_tif"
    )]
    pub tif: PerpTimeInForce,
}

/// Trigger order type of one [`TeePerpOrderWire`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeePerpTriggerOrderType {
    /// Trigger price in 1e8 units. Must be positive.
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub trigger_px: u64,
    /// Execute as a market order bounded by `limitPx` once triggered.
    pub is_market: bool,
    pub tpsl: TeePerpTpsl,
}

/// Exactly one of `limit` or `trigger`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TeePerpOrderType {
    Limit(TeePerpLimitOrderType),
    Trigger(TeePerpTriggerOrderType),
}

/// One native order on the trigger and batch routes. Prices and sizes are
/// 1e8-scaled integers; `cloid` is a non-zero u128.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeePerpOrderWire {
    pub asset: u32,
    pub is_buy: bool,
    /// Limit price in 1e8 units. For a market trigger this is the worst
    /// execution price the order may fill at.
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub limit_px: u64,
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub sz: u64,
    pub reduce_only: bool,
    pub order_type: TeePerpOrderType,
    #[serde(
        serialize_with = "serialize_safe_u128",
        deserialize_with = "deserialize_u128"
    )]
    pub cloid: u128,
}

/// Native grouping of an order batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TeePerpOrderGrouping {
    /// Independent orders, for example the legs of a scale order.
    Na,
    /// An entry (first child) with reduce-only exits that activate on fill.
    NormalTpsl,
    /// Reduce-only exits on an existing position.
    PositionTpsl,
}

impl TeePerpOrderGrouping {
    /// The `grouping` byte the app key signs: 0 = na, 1 = normalTpsl, 2 = positionTpsl.
    pub const fn signed(self) -> u8 {
        match self {
            Self::Na => 0,
            Self::NormalTpsl => 1,
            Self::PositionTpsl => 2,
        }
    }
}

/// Order batch action for [`TEE_PERP_ORDER_BATCH_SUBMIT_ROUTE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeePerpOrderBatchAction {
    /// One to 100 children with distinct cloids.
    pub orders: Vec<TeePerpOrderWire>,
    pub grouping: TeePerpOrderGrouping,
}

/// Leverage and margin-mode change for [`TEE_PERP_LEVERAGE_SUBMIT_ROUTE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeePerpLeverageAction {
    pub asset: u32,
    pub is_cross: bool,
    /// Must be at least 1.
    pub leverage: u32,
}

/// Replacement terms for one resting limit order, for
/// [`TEE_PERP_ORDER_MODIFY_ROUTE`] and as one child of
/// [`TeePerpBatchModifyAction`].
///
/// The order is addressed by its cloid, which the modify preserves, and by
/// the engine `directiveId` its submit call returned (for a batch child,
/// that child's `directiveId`). The engine rejects the modify unless the
/// target's cloid equals `cloid`. Prices and sizes are 1e8-scaled integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeePerpModifyAction {
    pub asset: u32,
    /// `directiveId` of the order being replaced. Must not be nil.
    pub target_directive_id: uuid::Uuid,
    pub is_buy: bool,
    /// New limit price in 1e8 units. Must be positive.
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub limit_px: u64,
    /// New size in 1e8 units. Must be positive.
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub sz: u64,
    pub reduce_only: bool,
    /// Hypercall TIF byte on the wire: 1 = ALO, 2 = GTC, 3 = IOC.
    #[serde(
        rename = "encodedTif",
        serialize_with = "serialize_perp_tif",
        deserialize_with = "deserialize_perp_tif"
    )]
    pub tif: PerpTimeInForce,
    /// Cloid of the order being modified. Non-zero; the modify keeps it.
    #[serde(
        serialize_with = "serialize_safe_u128",
        deserialize_with = "deserialize_u128"
    )]
    pub cloid: u128,
}

/// Batch modify action for [`TEE_PERP_ORDER_BATCH_MODIFY_ROUTE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeePerpBatchModifyAction {
    /// One entry per order, in the order the venue applies and answers them.
    /// Targets and cloids must be distinct; 1 to the instance cap
    /// ([`DEFAULT_TEE_PERP_BATCH_MODIFY_MAX_CHILDREN`] by default).
    pub modifies: Vec<TeePerpModifyAction>,
}

/// Native TWAP for [`TEE_PERP_TWAP_SUBMIT_ROUTE`]. Hyperliquid runs it as
/// slices over `minutes`, each capped at 3% slippage; `randomize` varies the
/// slice timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeePerpTwapAction {
    pub asset: u32,
    pub is_buy: bool,
    /// Total size in 1e8 units. Must be positive.
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub sz: u64,
    pub reduce_only: bool,
    /// Duration, [`TEE_PERP_TWAP_MIN_MINUTES`] to [`TEE_PERP_TWAP_MAX_MINUTES`].
    pub minutes: u32,
    pub randomize: bool,
}

/// TWAP cancel for [`TEE_PERP_TWAP_CANCEL_ROUTE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeePerpTwapCancelAction {
    pub asset: u32,
    /// Venue TWAP id from [`TeePerpTwapResponse::twap_id`]. Must not be zero.
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub twap_id: u64,
}

/// Outcome stage of a same-call TEE perp submission.
///
/// Values the server adds after this SDK release deserialize as
/// [`TeePerpOrderStage::Unrecognized`], so one new child stage never makes a
/// whole batch response unreadable. Treat it like [`TeePerpOrderStage::Unknown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TeePerpOrderStage {
    /// The venue accepted the action.
    Accepted,
    /// The engine or venue rejected the action.
    Rejected,
    /// The action never reached the venue.
    NotSubmitted,
    /// The venue may have received the action. Reconcile before retrying
    /// with a new nonce; never resubmit through another route.
    Unknown,
    /// Batch modify only: the venue answered every child definitively, some
    /// accepted and some rejected. Each child's own stage says which.
    Partial,
    /// A stage this SDK version does not know. Reconcile as for `Unknown`.
    #[serde(other)]
    Unrecognized,
}

impl TeePerpOrderStage {
    /// Whether the outcome is definitive (`accepted`, `rejected`,
    /// `not_submitted`, or a batch's `partial`). Anything else may have
    /// reached the venue and must be reconciled from venue state before
    /// acting on the same order again.
    pub const fn is_definitive(self) -> bool {
        matches!(
            self,
            Self::Accepted | Self::Rejected | Self::NotSubmitted | Self::Partial
        )
    }
}

/// Stable, non-sensitive failure classification of a TEE perp response.
///
/// Values the server adds after this SDK release deserialize as
/// [`TeePerpErrorCode::Unrecognized`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TeePerpErrorCode {
    EngineAdmissionRejected,
    DuplicateDirective,
    TradingUnavailable,
    ExecutionUnavailable,
    AuthorizationFailed,
    InvalidRequest,
    WalletNotAdmitted,
    WalletAdmissionMismatch,
    WalletAdmissionUnavailable,
    ParentCredentialsUnavailable,
    ParentKmsRequestInvalid,
    EnclaveUnavailable,
    EnclaveKmsRequestInvalid,
    EnclaveWalletUnavailable,
    EnclaveClockUnavailable,
    EnclavePolicyRejected,
    EnclaveAuthorizationRejected,
    EnclaveWalletMismatch,
    EnclaveIntentExpired,
    EnclaveIntentTtlExceeded,
    EnclaveSigningFailed,
    EnclaveAttestationFailed,
    EnclaveResponseInvalid,
    ParentResponseInvalid,
    SignedRequestInvalid,
    InternalUnavailable,
    VenueRejected,
    /// The account has not approved the Hypercall builder on Hyperliquid.
    /// Approve it once with `Account.approveZeroFeeBuilder(builder)`.
    BuilderNotApproved,
    SubmissionOutcomeUnknown,
    /// A code this SDK version does not know.
    #[serde(other)]
    Unrecognized,
}

/// Stable machine code for why a perp action is (or would be) rejected. The
/// preview and the real submit responses carry the same code for the same
/// rejection.
///
/// Values the server adds after this SDK release deserialize as
/// [`PerpRejectionReasonCode::Unrecognized`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PerpRejectionReasonCode {
    /// Portfolio margin does not cover the action.
    InsufficientMargin,
    /// A cross/isolated switch was requested on an open position.
    ModeChangeWithPosition,
    /// The HyperCore position or equity observation is missing or too old.
    StalePositionSnapshot,
    /// The asset is not a configured collateral perp.
    MarketNotSupported,
    /// Risk-increasing perp orders require HyperCore unified account mode.
    UnifiedModeRequired,
    /// A reduce-only order would increase or flip the position.
    ReduceOnlyExceedsPosition,
    /// Trading is halted globally or for the market.
    TradingHalted,
    /// Perp admission requires a portfolio-margin account.
    AccountNotPm,
    /// The HyperCore venue risk observation is missing or mismatched.
    VenueRiskUnavailable,
    /// The valuation spot price is missing or stale.
    PriceUnavailable,
    /// A full liquidation of the account is pending.
    LiquidationPending,
    /// The account's health is unavailable, so risk-increasing orders are disabled.
    AccountHealthUnavailable,
    /// Leverage changes currently require no open perp positions or orders.
    LeverageRequiresFlatAccount,
    /// The current TEE policy does not enable this action kind.
    ActionNotSupported,
    /// The order terms are invalid (precision, grouping, identity).
    InvalidOrder,
    /// Any other engine rejection; the response text carries the reason.
    EngineRejected,
    /// A code this SDK version does not know.
    #[serde(other)]
    Unrecognized,
}

impl PerpRejectionReasonCode {
    /// Every code the server documents, in server order.
    pub const ALL: [Self; 16] = [
        Self::InsufficientMargin,
        Self::ModeChangeWithPosition,
        Self::StalePositionSnapshot,
        Self::MarketNotSupported,
        Self::UnifiedModeRequired,
        Self::ReduceOnlyExceedsPosition,
        Self::TradingHalted,
        Self::AccountNotPm,
        Self::VenueRiskUnavailable,
        Self::PriceUnavailable,
        Self::LiquidationPending,
        Self::AccountHealthUnavailable,
        Self::LeverageRequiresFlatAccount,
        Self::ActionNotSupported,
        Self::InvalidOrder,
        Self::EngineRejected,
    ];

    /// The wire (snake_case) form of the code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InsufficientMargin => "insufficient_margin",
            Self::ModeChangeWithPosition => "mode_change_with_position",
            Self::StalePositionSnapshot => "stale_position_snapshot",
            Self::MarketNotSupported => "market_not_supported",
            Self::UnifiedModeRequired => "unified_mode_required",
            Self::ReduceOnlyExceedsPosition => "reduce_only_exceeds_position",
            Self::TradingHalted => "trading_halted",
            Self::AccountNotPm => "account_not_pm",
            Self::VenueRiskUnavailable => "venue_risk_unavailable",
            Self::PriceUnavailable => "price_unavailable",
            Self::LiquidationPending => "liquidation_pending",
            Self::AccountHealthUnavailable => "account_health_unavailable",
            Self::LeverageRequiresFlatAccount => "leverage_requires_flat_account",
            Self::ActionNotSupported => "action_not_supported",
            Self::InvalidOrder => "invalid_order",
            Self::EngineRejected => "engine_rejected",
            Self::Unrecognized => "unrecognized",
        }
    }

    /// Whether the condition is a transient read (venue observation, price,
    /// or account health) and the same action may pass when re-checked
    /// shortly. The Hypercall order panel re-runs the preview once for these.
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::StalePositionSnapshot
                | Self::VenueRiskUnavailable
                | Self::PriceUnavailable
                | Self::AccountHealthUnavailable
        )
    }
}

fn serialize_optional_safe_u64<S>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        Some(value) => serialize_safe_u64(value, serializer),
        None => serializer.serialize_none(),
    }
}

fn deserialize_optional_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Wrapped(#[serde(deserialize_with = "deserialize_u64")] u64);
    Ok(Option::<Wrapped>::deserialize(deserializer)?.map(|Wrapped(value)| value))
}

macro_rules! tee_perp_outcome_helpers {
    ($name:ident) => {
        impl $name {
            /// Whether the venue accepted the action.
            pub fn is_accepted(&self) -> bool {
                self.stage == TeePerpOrderStage::Accepted
            }

            /// Whether the venue refused the order because the account has not
            /// approved the Hypercall builder. Fix with a one-time
            /// `Account.approveZeroFeeBuilder(builder)` from the manager wallet.
            pub fn requires_builder_approval(&self) -> bool {
                self.error_code == Some(TeePerpErrorCode::BuilderNotApproved)
            }
        }
    };
}

/// Response of [`TEE_PERP_ORDER_SUBMIT_ROUTE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeePerpOrderResponse {
    pub stage: TeePerpOrderStage,
    #[serde(default)]
    pub error_code: Option<TeePerpErrorCode>,
    pub directive_id: String,
    pub account: WalletAddress,
    pub api_wallet: WalletAddress,
    /// The cloid as 0x-prefixed 16-byte hex.
    pub cloid: String,
    /// WAL offset of a newly committed directive. Exact retries return null.
    #[serde(
        default,
        serialize_with = "serialize_optional_safe_u64",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub engine_seq: Option<u64>,
    #[serde(default)]
    pub venue_response_hash: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    /// Same code `POST /v1/tee-perp/preview` returns for the same rejection.
    #[serde(default)]
    pub reason_code: Option<PerpRejectionReasonCode>,
}
tee_perp_outcome_helpers!(TeePerpOrderResponse);

/// Response of [`TEE_PERP_CANCEL_SUBMIT_ROUTE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeePerpCancelResponse {
    pub stage: TeePerpOrderStage,
    #[serde(default)]
    pub error_code: Option<TeePerpErrorCode>,
    pub directive_id: String,
    pub account: WalletAddress,
    pub api_wallet: WalletAddress,
    pub cloid: String,
    #[serde(
        default,
        serialize_with = "serialize_optional_safe_u64",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub engine_seq: Option<u64>,
    #[serde(default)]
    pub venue_response_hash: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}
tee_perp_outcome_helpers!(TeePerpCancelResponse);

/// Response of [`TEE_PERP_CANCEL_BY_OID_SUBMIT_ROUTE`] and
/// [`TEE_PERP_TWAP_CANCEL_ROUTE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeePerpVenueActionResponse {
    pub stage: TeePerpOrderStage,
    #[serde(default)]
    pub error_code: Option<TeePerpErrorCode>,
    pub request_id: String,
    pub account: WalletAddress,
    pub api_wallet: WalletAddress,
    /// For example `cancel_by_oid` or `twap_cancel`.
    pub action_kind: String,
    #[serde(
        default,
        serialize_with = "serialize_optional_safe_u64",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub engine_seq: Option<u64>,
    #[serde(default)]
    pub venue_response_hash: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}
tee_perp_outcome_helpers!(TeePerpVenueActionResponse);

/// One child of a [`TeePerpOrderBatchResponse`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeePerpOrderBatchChildResponse {
    /// Engine reservation identity of this child.
    pub directive_id: String,
    /// The child's cloid as 0x-prefixed 16-byte hex.
    pub cloid: String,
}

/// Response of [`TEE_PERP_TRIGGER_ORDER_SUBMIT_ROUTE`] and
/// [`TEE_PERP_ORDER_BATCH_SUBMIT_ROUTE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeePerpOrderBatchResponse {
    /// `accepted` only when every child was accepted. A partly rejected batch
    /// is `unknown`; its children are settled individually by reconciliation.
    pub stage: TeePerpOrderStage,
    #[serde(default)]
    pub error_code: Option<TeePerpErrorCode>,
    pub batch_id: String,
    pub account: WalletAddress,
    pub api_wallet: WalletAddress,
    pub grouping: TeePerpOrderGrouping,
    /// Children in submission order.
    pub children: Vec<TeePerpOrderBatchChildResponse>,
    #[serde(
        default,
        serialize_with = "serialize_optional_safe_u64",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub engine_seq: Option<u64>,
    #[serde(default)]
    pub venue_response_hash: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub reason_code: Option<PerpRejectionReasonCode>,
}
tee_perp_outcome_helpers!(TeePerpOrderBatchResponse);

/// Response of [`TEE_PERP_LEVERAGE_SUBMIT_ROUTE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeePerpLeverageResponse {
    pub stage: TeePerpOrderStage,
    #[serde(default)]
    pub error_code: Option<TeePerpErrorCode>,
    pub request_id: String,
    pub account: WalletAddress,
    pub api_wallet: WalletAddress,
    pub asset: u32,
    pub is_cross: bool,
    pub leverage: u32,
    #[serde(
        default,
        serialize_with = "serialize_optional_safe_u64",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub engine_seq: Option<u64>,
    #[serde(default)]
    pub venue_response_hash: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub reason_code: Option<PerpRejectionReasonCode>,
}
tee_perp_outcome_helpers!(TeePerpLeverageResponse);

/// What rests at the venue under a modified order's cloid after the call,
/// read from the Hyperliquid response the server verified against
/// `venueResponseHash`. The modify response is returned only after
/// Hyperliquid answered, so trust it over an immediate `orderStatus` read,
/// which can lag the venue by seconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
#[non_exhaustive]
pub enum TeePerpModifyVenueOrder {
    /// The replacement rests under this NEW order ID; the old one is gone.
    Resting { oid: u64 },
    /// The replacement filled on placement.
    #[serde(rename_all = "camelCase")]
    Filled {
        oid: u64,
        total_sz: String,
        avg_px: String,
    },
    /// Hyperliquid cancelled the original order and then failed to place the
    /// replacement: nothing rests under the cloid. `reason` is
    /// `modify rejected; venue cancelled original`.
    Cancelled { reason: String },
    /// A status this SDK version does not know.
    #[serde(other)]
    Unrecognized,
}

/// Response of [`TEE_PERP_ORDER_MODIFY_ROUTE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeePerpModifyResponse {
    /// `accepted`: the venue applied the replacement. `rejected`: the venue
    /// (or engine) refused it; the earlier order still rests unless
    /// [`Self::venue_order`] is `Cancelled`. `not_submitted`: nothing reached
    /// the venue. `unknown`: reconcile.
    pub stage: TeePerpOrderStage,
    #[serde(default)]
    pub error_code: Option<TeePerpErrorCode>,
    /// Identity of this modify command.
    pub request_id: String,
    /// The reservation that was replaced; unchanged by the modify.
    pub target_directive_id: String,
    pub account: WalletAddress,
    pub api_wallet: WalletAddress,
    /// The preserved cloid as 0x-prefixed 16-byte hex.
    pub cloid: String,
    #[serde(
        default,
        serialize_with = "serialize_optional_safe_u64",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub engine_seq: Option<u64>,
    #[serde(default)]
    pub venue_response_hash: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub reason_code: Option<PerpRejectionReasonCode>,
    /// The replacement's new order ID, or the venue's cancel of the original.
    #[serde(default)]
    pub venue_order: Option<TeePerpModifyVenueOrder>,
}
tee_perp_outcome_helpers!(TeePerpModifyResponse);

impl TeePerpModifyResponse {
    /// Whether Hyperliquid cancelled the original order during a rejected
    /// modify, so nothing rests under the cloid any more.
    pub fn venue_cancelled_original(&self) -> bool {
        matches!(
            self.venue_order,
            Some(TeePerpModifyVenueOrder::Cancelled { .. })
        )
    }
}

/// One child of a [`TeePerpBatchModifyResponse`], in request order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeePerpBatchModifyChildResponse {
    /// Zero-based position of this child in the request.
    pub index: u32,
    /// Modify identity of this child. Its outcome, and reconciliation of an
    /// unknown one, are keyed by it.
    pub request_id: String,
    pub target_directive_id: String,
    /// The child's preserved cloid as 0x-prefixed 16-byte hex.
    pub cloid: String,
    /// This child's own outcome. `accepted`: the venue applied this modify.
    /// `rejected`: the venue refused it; the earlier order still rests unless
    /// [`Self::venue_order`] is `Cancelled`. `not_submitted`: nothing reached
    /// the venue. `unknown`: reconciled from venue state.
    pub stage: TeePerpOrderStage,
    /// The venue's error for a rejected child, verbatim.
    #[serde(default)]
    pub message: Option<String>,
    /// The replacement's new order ID, or the venue's cancel of the original.
    #[serde(default)]
    pub venue_order: Option<TeePerpModifyVenueOrder>,
}

impl TeePerpBatchModifyChildResponse {
    /// Whether the venue applied this child's replacement.
    pub fn is_accepted(&self) -> bool {
        self.stage == TeePerpOrderStage::Accepted
    }

    /// Whether Hyperliquid cancelled this child's original order before its
    /// replacement failed, so nothing rests under the cloid any more.
    pub fn venue_cancelled_original(&self) -> bool {
        matches!(
            self.venue_order,
            Some(TeePerpModifyVenueOrder::Cancelled { .. })
        )
    }
}

/// Response of [`TEE_PERP_ORDER_BATCH_MODIFY_ROUTE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeePerpBatchModifyResponse {
    /// Whole-action stage: `accepted` when every child was, `rejected` when
    /// every child was, `partial` when the venue answered every child with a
    /// mix, and `unknown` only when some child is genuinely unknown. Read
    /// [`Self::children`] for each child.
    pub stage: TeePerpOrderStage,
    #[serde(default)]
    pub error_code: Option<TeePerpErrorCode>,
    /// Identity of this batch modify command.
    pub request_id: String,
    pub account: WalletAddress,
    pub api_wallet: WalletAddress,
    #[serde(
        default,
        serialize_with = "serialize_optional_safe_u64",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub engine_seq: Option<u64>,
    #[serde(default)]
    pub venue_response_hash: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub reason_code: Option<PerpRejectionReasonCode>,
    /// One entry per requested modify, in request order.
    pub children: Vec<TeePerpBatchModifyChildResponse>,
}
tee_perp_outcome_helpers!(TeePerpBatchModifyResponse);

impl TeePerpBatchModifyResponse {
    /// The child for request position `index`.
    pub fn child(&self, index: usize) -> Option<&TeePerpBatchModifyChildResponse> {
        self.children
            .iter()
            .find(|child| usize::try_from(child.index).ok() == Some(index))
    }

    /// The child that targeted `target_directive_id`.
    pub fn child_for_target(
        &self,
        target_directive_id: &uuid::Uuid,
    ) -> Option<&TeePerpBatchModifyChildResponse> {
        self.children.iter().find(|child| {
            uuid::Uuid::parse_str(&child.target_directive_id).ok() == Some(*target_directive_id)
        })
    }

    /// Children whose replacement the venue applied.
    pub fn accepted_children(&self) -> impl Iterator<Item = &TeePerpBatchModifyChildResponse> {
        self.children.iter().filter(|child| child.is_accepted())
    }

    /// Children whose outcome is not definitive (`unknown`, or a stage this
    /// SDK does not know). Reconcile these from venue state before modifying
    /// or cancelling the same orders again.
    pub fn children_to_reconcile(&self) -> impl Iterator<Item = &TeePerpBatchModifyChildResponse> {
        self.children
            .iter()
            .filter(|child| !child.stage.is_definitive())
    }

    /// Whether the children did not all end in the same stage.
    pub fn is_mixed(&self) -> bool {
        self.children
            .split_first()
            .is_some_and(|(first, rest)| rest.iter().any(|child| child.stage != first.stage))
    }
}

/// Response of [`TEE_PERP_TWAP_SUBMIT_ROUTE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeePerpTwapResponse {
    pub stage: TeePerpOrderStage,
    #[serde(default)]
    pub error_code: Option<TeePerpErrorCode>,
    /// Engine reservation identity of the TWAP. The engine reserves the full
    /// size at admission and releases it as venue fills arrive.
    pub directive_id: String,
    pub account: WalletAddress,
    pub api_wallet: WalletAddress,
    /// Venue TWAP id, present once the venue accepted the TWAP. Needed to
    /// cancel it.
    #[serde(
        default,
        serialize_with = "serialize_optional_safe_u64",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub twap_id: Option<u64>,
    #[serde(
        default,
        serialize_with = "serialize_optional_safe_u64",
        deserialize_with = "deserialize_optional_u64"
    )]
    pub engine_seq: Option<u64>,
    #[serde(default)]
    pub venue_response_hash: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub reason_code: Option<PerpRejectionReasonCode>,
}
tee_perp_outcome_helpers!(TeePerpTwapResponse);

// ---------------------------------------------------------------------------
// Markets
// ---------------------------------------------------------------------------

/// One configured perp market and whether the TEE routes accept it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeePerpMarketEntry {
    /// Hyperliquid asset id the signed order names.
    pub asset_id: u32,
    /// Engine perp symbol, e.g. `BTC-PERP`.
    pub symbol: String,
    pub underlying: String,
    /// Exact, case-sensitive Hyperliquid coin, e.g. `BTC` or `xyz:SNDK`.
    pub hyperliquid_coin: String,
    /// HIP-3 dex name for builder-deployed markets.
    #[serde(default)]
    pub dex: Option<String>,
    /// Whether market-level requirements hold. Per-account venue schedule
    /// observation is checked at order time.
    pub supported: bool,
    /// `market_not_supported` when `supported` is false.
    #[serde(default)]
    pub reason_code: Option<String>,
    /// The missing requirement, e.g. `price_source`.
    #[serde(default)]
    pub missing: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

/// Response of [`TEE_PERP_MARKETS_ROUTE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeePerpMarketsResponse {
    pub markets: Vec<TeePerpMarketEntry>,
}

impl TeePerpMarketsResponse {
    /// The market for a Hyperliquid asset id.
    pub fn by_asset_id(&self, asset_id: u32) -> Option<&TeePerpMarketEntry> {
        self.markets
            .iter()
            .find(|market| market.asset_id == asset_id)
    }

    /// The market for an exact Hyperliquid coin, e.g. `BTC` or `xyz:SNDK`.
    pub fn by_hyperliquid_coin(&self, coin: &str) -> Option<&TeePerpMarketEntry> {
        self.markets
            .iter()
            .find(|market| market.hyperliquid_coin == coin)
    }
}

// ---------------------------------------------------------------------------
// Preview
// ---------------------------------------------------------------------------

/// Native time in force of a previewed limit order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PerpPreviewTif {
    Gtc,
    Ioc,
    /// Add liquidity only. Admission reserves it as resting GTC risk.
    Alo,
}

impl From<PerpTimeInForce> for PerpPreviewTif {
    fn from(value: PerpTimeInForce) -> Self {
        match value {
            PerpTimeInForce::Alo => Self::Alo,
            PerpTimeInForce::Gtc => Self::Gtc,
            PerpTimeInForce::Ioc => Self::Ioc,
        }
    }
}

/// Previewed limit order type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerpPreviewLimit {
    pub tif: PerpPreviewTif,
}

/// Previewed trigger order type. Prices are USDC decimals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerpPreviewTrigger {
    pub trigger_px: Decimal,
    pub is_market: bool,
    pub tpsl: TeePerpTpsl,
}

/// Exactly one of `limit` or `trigger`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PerpPreviewOrderType {
    Limit(PerpPreviewLimit),
    Trigger(PerpPreviewTrigger),
}

/// One previewed order. Prices are USDC and sizes are coins, as decimals
/// (not the 1e8-scaled integers of the signed submit wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerpPreviewOrder {
    pub asset: u32,
    pub is_buy: bool,
    pub limit_px: Decimal,
    pub sz: Decimal,
    #[serde(default)]
    pub reduce_only: bool,
    pub order_type: PerpPreviewOrderType,
}

/// Leverage the preview should assume instead of the account's current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerpPreviewLeverageSetting {
    pub is_cross: bool,
    pub value: u32,
}

/// The action to preview, tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PerpPreviewAction {
    /// One limit or trigger order.
    Order {
        #[serde(flatten)]
        order: PerpPreviewOrder,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        leverage: Option<PerpPreviewLeverageSetting>,
    },
    /// A native order batch (scale orders, brackets, position TP/SL).
    OrderBatch {
        grouping: TeePerpOrderGrouping,
        orders: Vec<PerpPreviewOrder>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        leverage: Option<PerpPreviewLeverageSetting>,
    },
    /// A leverage or margin-mode change.
    Leverage {
        asset: u32,
        is_cross: bool,
        leverage: u32,
    },
    /// A TWAP, evaluated as admission reserves it: its full size at the worst
    /// slice price (the mark moved by the venue slippage cap), even when
    /// reduce-only.
    Twap {
        asset: u32,
        is_buy: bool,
        /// Total size in coins.
        sz: Decimal,
        #[serde(default)]
        reduce_only: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        leverage: Option<PerpPreviewLeverageSetting>,
    },
}

/// Request body of [`TEE_PERP_PREVIEW_ROUTE`]. Read-only; no signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerpPreviewRequest {
    /// Trading account (or its manager wallet).
    pub account: WalletAddress,
    pub action: PerpPreviewAction,
}

/// Whether the previewed action would be admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerpPreviewDecision {
    Ok,
    Rejected,
}

/// Portfolio margin before and after the action, as admission charges it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerpPreviewMargin {
    pub equity: Decimal,
    pub initial_required_before: Decimal,
    pub initial_required_after: Decimal,
    pub initial_available_after: Decimal,
    pub maintenance_required_after: Decimal,
    /// Initial margin after minus equity when positive, otherwise null.
    #[serde(default)]
    pub shortfall: Option<Decimal>,
}

/// Largest same-side size that still passes the real admission check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerpPreviewMaxSize {
    pub sz: Decimal,
    pub notional: Decimal,
}

/// The liquidation reached first as the market's price moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerpLiquidationBinding {
    /// The Hyperliquid compartment reaches venue maintenance.
    Hyperliquid,
    /// Hypercall portfolio-margin equity reaches maintenance margin.
    PortfolioMargin,
}

/// The price basis in which a venue compartment breaches maintenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VenueLiquidationBasis {
    /// Hyperliquid's own marks and collateral.
    Venue,
    /// The same observation rebased onto Hypercall's valuation spot.
    Valuation,
}

/// Survival of one HyperCore collateral compartment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueCompartmentRisk {
    pub underlyings: Vec<String>,
    pub hedge_survives: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liquidated_in: Option<VenueLiquidationBasis>,
    pub collateral_loss: Decimal,
}

/// Requirement with venue liquidation and reversal charged. While `enforced`
/// is false it is reported alongside, not used for, admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VenueLiquidationRisk {
    pub enforced: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_initial_margin: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_maintenance_margin: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanning_risk: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compartments: Vec<VenueCompartmentRisk>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

/// Response of [`TEE_PERP_PREVIEW_ROUTE`] (the `data` of its envelope).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerpPreviewResponse {
    pub decision: PerpPreviewDecision,
    #[serde(default)]
    pub reason_code: Option<PerpRejectionReasonCode>,
    /// Exactly the text the real submit would return.
    #[serde(default)]
    pub reason: Option<String>,
    /// Null when the action never reaches margin evaluation.
    #[serde(default)]
    pub margin: Option<PerpPreviewMargin>,
    /// Current projected liquidation price of the action's market: the
    /// stricter of the Hyperliquid and the portfolio-margin liquidation.
    #[serde(default)]
    pub liquidation_px_before: Option<Decimal>,
    /// The same price after the action.
    #[serde(default)]
    pub liquidation_px_after: Option<Decimal>,
    /// Which liquidation binds `liquidation_px_after`.
    #[serde(default)]
    pub liquidation_binding: Option<PerpLiquidationBinding>,
    #[serde(default)]
    pub venue_liquidation: Option<VenueLiquidationRisk>,
    /// Plain limit orders only.
    #[serde(default)]
    pub max_size: Option<PerpPreviewMaxSize>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl PerpPreviewResponse {
    /// Whether the previewed action would be admitted.
    pub fn is_ok(&self) -> bool {
        self.decision == PerpPreviewDecision::Ok
    }

    /// Whether a null liquidation price means "could not be computed" rather
    /// than "never reached".
    pub fn liquidation_price_unavailable(&self) -> bool {
        self.notes.iter().any(|note| {
            note.starts_with("Liquidation price unavailable")
                || note.starts_with("Liquidation price input unavailable")
        })
    }
}

// ---------------------------------------------------------------------------
// TEE API wallet enrollment
// ---------------------------------------------------------------------------

/// Body of [`TEE_API_WALLET_APPROVAL_REQUEST_ROUTE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeeApiWalletApprovalRequest {
    /// Portfolio-margin Account.sol whose manager will authorize the wallet.
    pub account: WalletAddress,
    /// Current manager of the account.
    pub manager: WalletAddress,
    /// Unix seconds included in the EIP-191 authentication message.
    pub timestamp: i64,
    /// Manager personal-sign signature over the authentication message.
    pub signature: String,
}

/// Response of [`TEE_API_WALLET_APPROVAL_REQUEST_ROUTE`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeApiWalletApprovalPackage {
    /// Opaque TEE wallet identity. Echo it when submitting the signature.
    #[serde(
        serialize_with = "serialize_b256",
        deserialize_with = "deserialize_b256"
    )]
    pub tee_wallet_id: B256,
    /// Hyperliquid API wallet address controlled by the TEE.
    pub api_wallet_address: WalletAddress,
    /// Stable name registered for the API wallet on Hyperliquid.
    pub api_wallet_name: String,
    /// Nonce of the EIP-712 approval payload. Copy it without modification.
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub nonce: u64,
    /// Server issuance time encoded into `nonce`, as Unix milliseconds.
    pub issued_at_ms: u64,
    /// Last Unix millisecond at which the signed approval may be submitted.
    pub expires_at_ms: u64,
    /// EIP-712 `HLAddApiWallet` payload for the manager to sign.
    pub typed_data: serde_json::Value,
}

/// Body of [`TEE_API_WALLET_APPROVAL_ROUTE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeeApiWalletApprovalSubmitRequest {
    pub account: WalletAddress,
    pub manager: WalletAddress,
    /// The same timestamp used for the approval request.
    pub timestamp: i64,
    pub authentication_signature: String,
    #[serde(
        serialize_with = "serialize_b256",
        deserialize_with = "deserialize_b256"
    )]
    pub tee_wallet_id: B256,
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub nonce: u64,
    /// Manager EIP-712 signature over the returned typed data.
    pub signature: String,
}

#[cfg(test)]
#[path = "tee_perp_test.rs"]
mod tests;
