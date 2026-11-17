//! Portfolio Margin financing: read an account's PM loans and unpaid
//! settlement obligations, and repay them from its HyperCore USDC.

#![expect(
    clippy::result_large_err,
    reason = "these helpers preserve the established unboxed public ClientError contract"
)]

use alloy::sol;
use reqwest::Method;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::api::HypercallClient;
use crate::error::{ClientError, Result};
use crate::wallet::{AccountAddress, HypercallWallet};

const API_ROUTE_PM_FINANCING: &str = "/v1/pm-financing";
const API_ROUTE_PM_FINANCING_REPAY: &str = "/v1/pm-financing/repay";
/// The signed `amount` that repays everything owed.
const REPAY_ALL: &str = "all";
/// Account.sol moves whole micro-USDC.
const USDC_DECIMALS: u32 = 6;

sol! {
    /// Repay a Portfolio Margin account's PM financing from its HyperCore
    /// USDC (`POST /v1/pm-financing/repay`), signed by the Account.sol
    /// manager `wallet`. `amount` is the exact decimal string the request
    /// carries, or `all`.
    #[derive(Debug, PartialEq, Eq)]
    struct RepayPmLoan {
        address wallet;
        address account;
        string underlying;
        string amount;
        uint64 nonce;
    }
}

/// How much PM financing to repay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepayAmount {
    /// An exact positive USDC amount with at most six decimal places.
    Usdc(Decimal),
    /// Everything owed on the loan, plus every unpaid settlement obligation.
    All,
}

impl RepayAmount {
    /// The `amount` string the request sends and the manager signs: `all`,
    /// or the normalized decimal (`60.50` signs as `60.5`).
    pub fn signed_amount(&self) -> Result<String> {
        match self {
            Self::All => Ok(REPAY_ALL.to_string()),
            Self::Usdc(amount) => {
                let amount = amount.normalize();
                if amount <= Decimal::ZERO {
                    return Err(ClientError::InvalidInput(
                        "repay amount must be positive".to_string(),
                    ));
                }
                if amount.scale() > USDC_DECIMALS {
                    return Err(ClientError::InvalidInput(
                        "repay amount must have at most 6 decimal places".to_string(),
                    ));
                }
                Ok(amount.to_string())
            }
        }
    }
}

/// A voluntary repayment of PM financing.
#[derive(Debug, Clone)]
pub struct RepayPmLoanParams {
    /// The manager's Portfolio Margin Account.sol.
    pub account: AccountAddress,
    /// The loan's underlying, exactly as [`PmFinancingItem::underlying`]
    /// lists it. Unpaid settlement obligations are always repaid first.
    pub underlying: String,
    pub amount: RepayAmount,
    /// Millisecond time-bound nonce. Defaults to the wallet's next nonce.
    /// Keep it to retry an ambiguous submission: an exact retry (same
    /// account, underlying, amount and nonce) returns the original request.
    pub nonce: Option<u64>,
}

/// `POST /v1/pm-financing/repay` acceptance. The USDC is reserved; the
/// repayment lands once HyperCore confirms the transfer. Track it by
/// `plan_id` in [`PmFinancingRepayment::requests`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PmLoanRepayResponse {
    pub account: AccountAddress,
    /// The request's id; an exact retry returns the same id.
    pub request_id: String,
    pub plan_id: String,
    pub nonce: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PmFinancingItemKind {
    Loan,
    Settlement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PmFinancingItemStatus {
    /// A floating loan: no deadline.
    Open,
    /// An unpaid settlement inside its cure period.
    GracePeriod,
    /// An unpaid settlement past its cure deadline.
    Missed,
}

/// One loan or unpaid settlement obligation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PmFinancingItem {
    pub kind: PmFinancingItemKind,
    pub underlying: String,
    pub expiry_ts_ms: Option<i64>,
    pub principal_usdc: Decimal,
    pub interest_usdc: Decimal,
    pub paid_usdc: Option<Decimal>,
    pub owed_usdc: Decimal,
    pub since_ms: Option<u64>,
    pub deadline_ms: Option<u64>,
    pub status: PmFinancingItemStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PmFinancingTotals {
    pub loans_owed_usdc: Decimal,
    pub settlements_owed_usdc: Decimal,
    pub total_owed_usdc: Decimal,
    pub missed_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PmFinancingMargin {
    pub equity_usdc: Decimal,
    pub maintenance_requirement_usdc: Decimal,
    /// Requirement over equity; `None` when equity is not positive.
    pub margin_utilization: Option<Decimal>,
}

/// What a voluntary repayment may use now, and the latest requests.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PmFinancingRepayment {
    /// HyperCore USDC a new repayment may use: withdrawable minus active
    /// withdrawal and recovery reservations. `None` before the engine has
    /// the account's balance.
    pub unreserved_withdrawable_usdc: Option<Decimal>,
    /// Why a new request would be refused now; `None` when it would not.
    /// [`PmRepaymentBlockedReason::rejection`] names the error such a
    /// request gets.
    pub blocked_reason: Option<PmRepaymentBlockedReason>,
    /// The newest requests, newest first.
    pub requests: Vec<PmLoanRepaymentRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PmLoanRepaymentRequest {
    pub plan_id: String,
    pub underlying: String,
    pub amount_usdc: Decimal,
    pub status: PmRepaymentStatus,
    pub updated_at_ms: u64,
}

/// A voluntary repayment's recovery plan status, as the engine names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum PmRepaymentStatus {
    /// The account's HyperCore USDC is reserved; nothing has moved yet.
    Planned,
    /// The transfer was sent to HyperCore.
    Submitted,
    /// HyperCore confirmed the transfer; the repayment applies next.
    Confirmed,
    /// Transient: the recovery worker cancels the plan next.
    Failed,
    /// Transient: the recovery worker cancels the plan next.
    TimedOut,
    /// Nothing moved, or the transfer failed. Sign a new request.
    Canceled,
    /// Handed to operators.
    EscalatedManual,
    /// Part of the amount was applied.
    PartiallyRepaid,
    /// The repayment was applied.
    Repaid,
    /// A status this client version does not know.
    #[serde(other)]
    Unknown,
}

impl PmRepaymentStatus {
    /// Whether the request is finished: `Repaid`, `Canceled` or
    /// `EscalatedManual`. Poll [`HypercallClient::get_pm_financing`] until
    /// this holds.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Repaid | Self::Canceled | Self::EscalatedManual)
    }
}

/// Why `GET /v1/pm-financing` says a new repayment would be refused now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PmRepaymentBlockedReason {
    /// Another recovery plan of the account is still active.
    RecoveryInProgress,
    /// The account is below maintenance or in a liquidation state.
    AccountUnhealthy,
    /// A reason this client version does not know.
    #[serde(other)]
    Unknown,
}

impl PmRepaymentBlockedReason {
    /// The rejection a repay request gets while this reason holds.
    pub fn rejection(self) -> Option<PmRepayRejection> {
        match self {
            Self::RecoveryInProgress => Some(PmRepayRejection::RecoveryInProgress),
            Self::AccountUnhealthy => Some(PmRepayRejection::AccountUnhealthy),
            Self::Unknown => None,
        }
    }
}

/// A stable error code `POST /v1/pm-financing/repay` returns. Read it from a
/// [`ClientError`] with [`PmRepayRejection::from_error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmRepayRejection {
    /// Voluntary repayment is off on this deployment (503).
    Unavailable,
    /// The nonce is outside the engine's time bounds (409). Sign again.
    NonceOutOfBounds,
    /// The nonce is bound to a different signed request (409).
    NonceReused,
    /// `underlying` names none of the account's loans or unpaid settlements
    /// (400).
    UnknownUnderlying,
    /// The signer is not the account's engine-indexed manager (403).
    Unauthorized,
    /// The account is below maintenance or being liquidated (409). Matches
    /// [`PmRepaymentBlockedReason::AccountUnhealthy`].
    AccountUnhealthy,
    /// Another recovery of the account is active (409). Matches
    /// [`PmRepaymentBlockedReason::RecoveryInProgress`].
    RecoveryInProgress,
    /// Nothing is owed (409).
    NothingOwed,
    /// The amount is not a positive whole micro-USDC amount (400).
    InvalidAmount,
    /// An exact amount below the instance minimum that does not repay
    /// everything owed (400).
    BelowMinimum,
    /// The amount exceeds what is owed (409).
    ExceedsOwed,
    /// The amount exceeds the unreserved withdrawable USDC (409).
    InsufficientWithdrawable,
    /// The engine refused it without a more specific code (422).
    Rejected,
    /// The signer is not the account's current Account.sol manager (403).
    NotManager,
    /// The API's nonce time-bound check failed (409). Sign again.
    InvalidNonce,
    /// The signature does not recover to `wallet`.
    SignatureVerificationFailed,
}

impl PmRepayRejection {
    pub const ALL: [Self; 16] = [
        Self::Unavailable,
        Self::NonceOutOfBounds,
        Self::NonceReused,
        Self::UnknownUnderlying,
        Self::Unauthorized,
        Self::AccountUnhealthy,
        Self::RecoveryInProgress,
        Self::NothingOwed,
        Self::InvalidAmount,
        Self::BelowMinimum,
        Self::ExceedsOwed,
        Self::InsufficientWithdrawable,
        Self::Rejected,
        Self::NotManager,
        Self::InvalidNonce,
        Self::SignatureVerificationFailed,
    ];

    /// The `code` the server sends.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Unavailable => "pm_voluntary_repayment_unavailable",
            Self::NonceOutOfBounds => "pm_voluntary_repayment_nonce_out_of_bounds",
            Self::NonceReused => "pm_voluntary_repayment_nonce_reused",
            Self::UnknownUnderlying => "pm_voluntary_repayment_unknown_underlying",
            Self::Unauthorized => "pm_voluntary_repayment_unauthorized",
            Self::AccountUnhealthy => "pm_voluntary_repayment_account_unhealthy",
            Self::RecoveryInProgress => "pm_voluntary_repayment_recovery_in_progress",
            Self::NothingOwed => "pm_voluntary_repayment_nothing_owed",
            Self::InvalidAmount => "pm_voluntary_repayment_invalid_amount",
            Self::BelowMinimum => "pm_voluntary_repayment_below_minimum",
            Self::ExceedsOwed => "pm_voluntary_repayment_exceeds_owed",
            Self::InsufficientWithdrawable => "pm_voluntary_repayment_insufficient_withdrawable",
            Self::Rejected => "pm_voluntary_repayment_rejected",
            Self::NotManager => "forbidden",
            Self::InvalidNonce => "invalid_nonce",
            Self::SignatureVerificationFailed => "signature_verification_failed",
        }
    }

    /// The rejection a server `code` names, if it is one of these.
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|rejection| rejection.code() == code)
    }

    /// The rejection a failed [`HypercallClient::repay_pm_loan`] carries, if
    /// the server named one.
    pub fn from_error(error: &ClientError) -> Option<Self> {
        Self::from_code(&error.api_error_code()?)
    }

    /// Whether signing the same request again with a fresh nonce may succeed
    /// without anything else changing.
    pub fn is_resignable(self) -> bool {
        matches!(self, Self::NonceOutOfBounds | Self::InvalidNonce)
    }
}

/// `GET /v1/pm-financing`: one wallet's PM borrowing.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct PmFinancingResponse {
    pub wallet: AccountAddress,
    /// The wallet's Portfolio Margin Account.sol, if it has one.
    pub account: Option<AccountAddress>,
    pub is_pm_account: bool,
    pub measured_at_ms: u64,
    pub borrow_apr: Decimal,
    pub items: Vec<PmFinancingItem>,
    pub totals: PmFinancingTotals,
    pub margin: Option<PmFinancingMargin>,
    /// Financed PM boxes, as the server returns them.
    #[serde(default)]
    pub box_financing: Option<serde_json::Value>,
    /// Voluntary repayment capacity and requests. `None` for a Standard
    /// account, or from a server without voluntary repayment.
    #[serde(default)]
    pub repayment: Option<PmFinancingRepayment>,
}

#[derive(Serialize)]
struct RepayPmLoanRequest<'a> {
    wallet: AccountAddress,
    account: AccountAddress,
    underlying: &'a str,
    amount: &'a str,
    nonce: u64,
    signature: String,
}

impl HypercallWallet {
    /// Sign a RepayPmLoan action. `amount` must be the exact string the
    /// request sends (see [`RepayAmount::signed_amount`]).
    pub async fn sign_repay_pm_loan(
        &self,
        account: AccountAddress,
        underlying: &str,
        amount: &str,
        nonce: u64,
    ) -> Result<String> {
        let message = RepayPmLoan {
            wallet: self.address.as_wallet_address().inner(),
            account: account.as_wallet_address().inner(),
            underlying: underlying.to_string(),
            amount: amount.to_string(),
            nonce,
        };
        self.sign_hypercall_action(&message, "RepayPmLoan").await
    }
}

impl HypercallClient {
    /// Read `wallet`'s PM loans, unpaid settlement obligations, margin and
    /// voluntary repayment status. `wallet` is the manager or its Account.sol.
    pub async fn get_pm_financing(&self, wallet: AccountAddress) -> Result<PmFinancingResponse> {
        Self::send_reqwest_json(
            self.request(Method::GET, API_ROUTE_PM_FINANCING)
                .query(&[("wallet", wallet.to_string())]),
            "get PM financing",
        )
        .await
    }

    /// Repay PM financing from the account's HyperCore USDC. `manager` must
    /// be the account's Account.sol manager; API wallets and agents are
    /// refused.
    ///
    /// Acceptance reserves the USDC; poll [`Self::get_pm_financing`] until
    /// the request reaches `Repaid` or `Canceled`. Rejections surface as
    /// [`ClientError::Api`] carrying a `pm_voluntary_repayment_*` code.
    pub async fn repay_pm_loan(
        &self,
        manager: &HypercallWallet,
        params: RepayPmLoanParams,
    ) -> Result<PmLoanRepayResponse> {
        if params.underlying.is_empty() {
            return Err(ClientError::InvalidInput(
                "underlying must name the loan's underlying".to_string(),
            ));
        }
        let amount = params.amount.signed_amount()?;
        let nonce = params.nonce.unwrap_or_else(|| manager.next_nonce());
        let signature = manager
            .sign_repay_pm_loan(params.account, &params.underlying, &amount, nonce)
            .await?;
        let request = RepayPmLoanRequest {
            wallet: manager.address,
            account: params.account,
            underlying: &params.underlying,
            amount: &amount,
            nonce,
            signature,
        };
        Self::send_json_body(
            self.request(Method::POST, API_ROUTE_PM_FINANCING_REPAY),
            &request,
            "repay PM loan",
        )
        .await
    }
}

#[cfg(test)]
#[path = "pm_financing_test.rs"]
mod tests;
