//! EIP-712 signing for every RFQ V2 action.
//!
//! The [`crate::rfq_v2`] transport methods take complete signed requests.
//! This module builds those requests. Each action has a pure message builder
//! (`rfq_v2_*_message`) that returns the exact EIP-712 struct the server
//! verifies, and a [`HypercallWallet`] method that signs it under the
//! Hypercall domain (name `Hypercall`, version `1`, the wallet's chain ID,
//! zero verifying contract) and returns the wire request.
//!
//! A remote signer that cannot use [`HypercallWallet`] signs
//! [`rfq_v2_signing_hash`] of the same message and wraps the 65-byte result
//! in [`hypercall_sdk_types::RfqV2Signature`].
//!
//! | Action | Message | Signer |
//! | --- | --- | --- |
//! | Create RFQ | [`AuthorizeRfqV2`] | Taker |
//! | Firm quote | [`AuthorizeRfqQuoteV2`] | Maker |
//! | Indicative quote | [`PublishRfqIndicativeQuoteV2`] | Maker |
//! | Firm-up request | [`RequestFirmRfqQuoteV2`] | Taker |
//! | Decline | [`DeclineRfqV2`] | Maker |
//! | Execute or sweep | [`ExecuteRfqV2`] | Taker |
//! | Cancel RFQ, withdraw quote | [`InvalidateRfqV2`] | Taker, maker |
//! | Bulk withdrawal | [`BulkWithdrawRfqV2`] | Maker |
//! | Cancel schedule | [`ScheduleRfqQuoteCancelV2`] | Maker |
//! | Replace RFQ or quote | [`ReplaceRfqV2`] | Taker, maker |
//!
//! These builders check only what changes the signed bytes or makes them
//! ambiguous. Economic policy (fill grids, deadlines, routing limits) is
//! the server's to enforce, and it answers a violation with a 4xx.
//!
//! ```rust,no_run
//! use hypercall_client::rfq_v2_signing::RfqV2ActionAuth;
//! use hypercall_client::{HypercallClient, HypercallWallet};
//! use uuid::Uuid;
//!
//! # async fn example(rfq_id: Uuid, quote_id: Uuid) -> hypercall_client::Result<()> {
//! let client = HypercallClient::new("https://api.hypercall.xyz");
//! let wallet = HypercallWallet::from_private_key("0xYOUR_PRIVATE_KEY", 999)?;
//! let now = client.server_now_ms().await?;
//! // Execute one package against a firm quote, with no builder fee.
//! let execution = wallet
//!     .sign_rfq_v2_execution(rfq_id, quote_id, 1_000_000, 0, RfqV2ActionAuth::new(now + 30_000, now))
//!     .await?;
//! client.execute_rfq_v2(rfq_id, &execution).await?;
//! # Ok(())
//! # }
//! ```

use alloy::{
    primitives::{keccak256, Address, FixedBytes, B256, U256},
    sol,
    sol_types::{Eip712Domain, SolStruct},
};
use uuid::Uuid;

use hypercall_sdk_types::{
    CreateRfqV2Request, DeclineRfqV2Request, PublishRfqV2FirmQuoteRequest,
    PublishRfqV2IndicativeQuoteRequest, RequestRfqV2FirmQuoteRequest, RfqV2BulkWithdrawalRequest,
    RfqV2BulkWithdrawalScope, RfqV2Digest, RfqV2ExecutionMode, RfqV2ExecutionRequest,
    RfqV2FeeLimits, RfqV2FillPolicy, RfqV2FirmQuoteTerms, RfqV2IndicativeQuoteTerms,
    RfqV2InvalidationRequest, RfqV2PackageLeg, RfqV2PricedLeg, RfqV2QuotePolicy,
    RfqV2QuoteReplacement, RfqV2ReplacementRequest, RfqV2Routing, RfqV2ScheduledQuoteCancelRequest,
    RfqV2Side, RfqV2SweepAllocation, RfqV2SweepRequest, RfqV2TakerIdentity, RfqV2Terms,
    RfqV2WithdrawalPolicy, WalletAddress,
};

use crate::{ClientError, HypercallWallet, Result};

/// Most quotes one execution or sweep may name.
pub const MAX_RFQ_V2_EXECUTION_ALLOCATIONS: usize = 1024;
/// Most exact quote targets one bulk withdrawal may name.
pub const MAX_RFQ_V2_BULK_WITHDRAWAL_QUOTES: usize = 64;

sol! {
    // Every struct below must match the RFQ service's EIP-712 types exactly:
    // field names, order and widths are all part of the type hash.

    #[derive(Debug, PartialEq, Eq)]
    struct RfqV2SigningLeg {
        bytes32 instrumentId;
        uint8 takerSide;
        uint128 quantityMicrosPerUnit;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct RfqV2SigningFeeLimits {
        uint128 perExecutionMicros;
        uint128 cumulativeMicros;
    }

    /// Immutable taker terms. Its struct hash is the parent commitment every
    /// quote signs as `rfqTermsCommitment`.
    #[derive(Debug, PartialEq, Eq)]
    struct RfqV2SigningTerms {
        bytes16 rfqId;
        address takerAccount;
        RfqV2SigningLeg[] legs;
        uint128 targetUnits;
        uint8 fillPolicy;
        uint128 minimumUnits;
        uint128 stepUnits;
        uint8 executionMode;
        bool autoMakerReceivesPremium;
        uint128 autoPremiumLimitMicros;
        uint8 quotePolicy;
        uint8 takerIdentity;
        uint8 routingMode;
        address[] selectedMakers;
        uint64 quoteDeadlineMs;
        uint64 expiresAtMs;
        RfqV2SigningFeeLimits feeLimits;
        address builder;
        bytes32 privateTermsBlinding;
    }

    /// Taker authorization to admit one RFQ.
    #[derive(Debug, PartialEq, Eq)]
    struct AuthorizeRfqV2 {
        RfqV2SigningTerms terms;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct RfqV2SigningPriceLeg {
        bytes32 instrumentId;
        uint128 priceMicros;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct RfqFirmQuoteV2SigningTerms {
        bytes16 quoteId;
        bytes16 rfqId;
        bytes32 rfqTermsCommitment;
        address makerAccount;
        uint64 makerGeneration;
        uint64 rfqGeneration;
        RfqV2SigningPriceLeg[] pricedLegs;
        uint128 referenceQuantity;
        bool makerReceivesReferencePremium;
        uint128 referencePremiumMicros;
        uint128 capacity;
        uint8 fillPolicy;
        uint128 minimumUnits;
        uint128 stepUnits;
        uint8 withdrawalPolicy;
        RfqV2SigningFeeLimits feeLimits;
        uint64 expiresAtMs;
        bytes32 protectionScope;
        bool hasSourceIndicativeQuote;
        bytes16 sourceIndicativeQuoteId;
    }

    /// Maker authorization to admit one executable quote.
    #[derive(Debug, PartialEq, Eq)]
    struct AuthorizeRfqQuoteV2 {
        RfqFirmQuoteV2SigningTerms terms;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct RfqIndicativeQuoteV2SigningTerms {
        bytes16 quoteId;
        bytes16 rfqId;
        bytes32 rfqTermsCommitment;
        address makerAccount;
        RfqV2SigningPriceLeg[] pricedLegs;
        uint128 referenceQuantity;
        bool makerReceivesReferencePremium;
        uint128 referencePremiumMicros;
        uint128 indicatedCapacity;
        uint64 expiresAtMs;
    }

    /// Non-executable maker price discovery. The quote ID is the replay key,
    /// so there is no nonce.
    #[derive(Debug, PartialEq, Eq)]
    struct PublishRfqIndicativeQuoteV2 {
        RfqIndicativeQuoteV2SigningTerms terms;
    }

    /// One ordered allocation. Order is signed: a sweep fills in this order.
    #[derive(Debug, PartialEq, Eq)]
    struct RfqV2ExecutionAllocation {
        bytes16 quoteId;
        uint128 packageUnits;
    }

    /// Taker authorization for one execution or an atomic sweep.
    #[derive(Debug, PartialEq, Eq)]
    struct ExecuteRfqV2 {
        bytes16 rfqId;
        bytes16 operationId;
        RfqV2ExecutionAllocation[] allocations;
        uint128 maxFeeMicros;
        uint64 expiresAtMs;
        uint64 nonce;
    }

    /// One parent cancellation or quote withdrawal.
    #[derive(Debug, PartialEq, Eq)]
    struct InvalidateRfqV2 {
        bytes16 rfqId;
        bytes16 quoteId;
        uint8 scope;
        uint8 reason;
        bytes16 operationId;
        uint64 expiresAtMs;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct RfqV2BulkWithdrawalTarget {
        bytes16 rfqId;
        bytes16 quoteId;
    }

    /// One maker action over exact quote IDs, one RFQ, or all open quotes.
    #[derive(Debug, PartialEq, Eq)]
    struct BulkWithdrawRfqV2 {
        address makerAccount;
        uint8 scope;
        bytes16 rfqId;
        RfqV2BulkWithdrawalTarget[] targets;
        bytes16 operationId;
        uint64 expiresAtMs;
        uint64 nonce;
    }

    /// Renew or remove the maker's account-wide quote cancel deadline.
    /// `cancelAtMs` zero removes it.
    #[derive(Debug, PartialEq, Eq)]
    struct ScheduleRfqQuoteCancelV2 {
        bytes16 operationId;
        address makerAccount;
        uint64 cancelAtMs;
        uint64 expiresAtMs;
        uint64 nonce;
    }

    /// Atomic replacement of an RFQ or a quote by a newly signed successor.
    #[derive(Debug, PartialEq, Eq)]
    struct ReplaceRfqV2 {
        bytes16 rfqId;
        bytes16 oldObjectId;
        bytes16 newObjectId;
        uint8 scope;
        bytes16 operationId;
        uint64 expiresAtMs;
        uint64 nonce;
    }

    /// Taker asks the maker of one indicative quote for firm terms.
    #[derive(Debug, PartialEq, Eq)]
    struct RequestFirmRfqQuoteV2 {
        bytes16 operationId;
        bytes16 rfqId;
        bytes16 indicativeQuoteId;
        uint128 requestedUnits;
        uint64 expiresAtMs;
        uint64 nonce;
    }

    /// A routed maker declines to quote one RFQ.
    #[derive(Debug, PartialEq, Eq)]
    struct DeclineRfqV2 {
        bytes16 rfqId;
        address makerAccount;
        bytes16 operationId;
        uint64 expiresAtMs;
        uint64 nonce;
    }
}

/// The signed fields every RFQ V2 lifecycle action carries besides its
/// subject: a fresh UUIDv7 operation ID, the signature expiry, and a nonce.
///
/// Reuse an operation ID only to resend the identical signed request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RfqV2ActionAuth {
    pub operation_id: Uuid,
    pub expires_at_ms: u64,
    pub nonce: u64,
}

impl RfqV2ActionAuth {
    /// Mint a fresh UUIDv7 operation ID for a new action.
    pub fn new(expires_at_ms: u64, nonce: u64) -> Self {
        Self {
            operation_id: Uuid::now_v7(),
            expires_at_ms,
            nonce,
        }
    }
}

/// What an [`InvalidateRfqV2`] retires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RfqV2InvalidationTarget {
    /// The taker cancels its RFQ.
    CancelRfq { rfq_id: Uuid },
    /// The maker withdraws one of its quotes.
    WithdrawQuote { rfq_id: Uuid, quote_id: Uuid },
}

/// What a [`ReplaceRfqV2`] retires in favour of its successor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RfqV2ReplacementTarget {
    Rfq { rfq_id: Uuid },
    Quote { rfq_id: Uuid, quote_id: Uuid },
}

/// The EIP-712 domain RFQ V2 messages are signed under.
pub fn rfq_v2_domain(chain_id: u64) -> Eip712Domain {
    Eip712Domain {
        name: Some("Hypercall".into()),
        version: Some("1".into()),
        chain_id: Some(alloy::primitives::U256::from(chain_id)),
        verifying_contract: Some(Address::ZERO),
        salt: None,
    }
}

/// The digest a signer signs for `message` on `chain_id`.
pub fn rfq_v2_signing_hash<T: SolStruct>(message: &T, chain_id: u64) -> B256 {
    message.eip712_signing_hash(&rfq_v2_domain(chain_id))
}

/// Signed instrument identity: `keccak256("instrument" ‖ symbol)`.
pub fn rfq_v2_instrument_id(symbol: &str) -> B256 {
    let mut preimage = Vec::with_capacity(10 + symbol.len());
    preimage.extend_from_slice(b"instrument");
    preimage.extend_from_slice(symbol.as_bytes());
    keccak256(preimage)
}

/// The commitment quotes sign as `rfq_terms_commitment`. The server returns
/// the same value as `terms_commitment` when the RFQ is created; a maker that
/// cannot see the full terms reads it from the RFQ instead.
#[expect(
    clippy::result_large_err,
    reason = "public client methods preserve the established unboxed ClientError contract"
)]
pub fn rfq_v2_terms_commitment(terms: &RfqV2Terms) -> Result<RfqV2Digest> {
    Ok(RfqV2Digest(
        rfq_v2_signing_terms(terms)?.eip712_hash_struct(),
    ))
}

/// The taker message for `POST /private/rfqs`.
#[expect(
    clippy::result_large_err,
    reason = "public client methods preserve the established unboxed ClientError contract"
)]
pub fn rfq_v2_create_message(terms: &RfqV2Terms, nonce: u64) -> Result<AuthorizeRfqV2> {
    Ok(AuthorizeRfqV2 {
        terms: rfq_v2_signing_terms(terms)?,
        nonce,
    })
}

/// The maker message for a firm quote.
pub fn rfq_v2_firm_quote_message(terms: &RfqV2FirmQuoteTerms, nonce: u64) -> AuthorizeRfqQuoteV2 {
    let (fill_policy, minimum_units, step_units) = fill_policy_fields(terms.fill_policy);
    AuthorizeRfqQuoteV2 {
        terms: RfqFirmQuoteV2SigningTerms {
            quoteId: uuid_bytes(terms.id),
            rfqId: uuid_bytes(terms.rfq_id),
            rfqTermsCommitment: terms.rfq_terms_commitment.0,
            makerAccount: terms.maker_account.inner(),
            makerGeneration: terms.maker_generation,
            rfqGeneration: terms.rfq_generation,
            pricedLegs: priced_legs(&terms.priced_legs),
            referenceQuantity: terms.reference_quantity,
            makerReceivesReferencePremium: terms.maker_receives_reference_premium,
            referencePremiumMicros: terms.reference_premium_micros,
            capacity: terms.capacity,
            fillPolicy: fill_policy,
            minimumUnits: minimum_units,
            stepUnits: step_units,
            withdrawalPolicy: match terms.withdrawal_policy {
                RfqV2WithdrawalPolicy::NotPermitted => 0,
                RfqV2WithdrawalPolicy::Permitted => 1,
            },
            feeLimits: fee_limits(terms.fee_limits),
            expiresAtMs: terms.expires_at_ms,
            protectionScope: terms.protection_scope.map_or(B256::ZERO, |scope| scope.0),
            hasSourceIndicativeQuote: terms.source_indicative_quote_id.is_some(),
            sourceIndicativeQuoteId: terms
                .source_indicative_quote_id
                .map_or(FixedBytes::ZERO, uuid_bytes),
        },
        nonce,
    }
}

/// The maker message for an indicative quote.
pub fn rfq_v2_indicative_quote_message(
    terms: &RfqV2IndicativeQuoteTerms,
) -> PublishRfqIndicativeQuoteV2 {
    PublishRfqIndicativeQuoteV2 {
        terms: RfqIndicativeQuoteV2SigningTerms {
            quoteId: uuid_bytes(terms.id),
            rfqId: uuid_bytes(terms.rfq_id),
            rfqTermsCommitment: terms.rfq_terms_commitment.0,
            makerAccount: terms.maker_account.inner(),
            pricedLegs: priced_legs(&terms.priced_legs),
            referenceQuantity: terms.reference_quantity,
            makerReceivesReferencePremium: terms.maker_receives_reference_premium,
            referencePremiumMicros: terms.reference_premium_micros,
            indicatedCapacity: terms.indicated_capacity,
            expiresAtMs: terms.expires_at_ms,
        },
    }
}

/// The taker message asking the maker of `indicative_quote_id` for firm terms.
pub fn rfq_v2_firm_up_message(
    rfq_id: Uuid,
    indicative_quote_id: Uuid,
    requested_units: u128,
    auth: RfqV2ActionAuth,
) -> RequestFirmRfqQuoteV2 {
    RequestFirmRfqQuoteV2 {
        operationId: uuid_bytes(auth.operation_id),
        rfqId: uuid_bytes(rfq_id),
        indicativeQuoteId: uuid_bytes(indicative_quote_id),
        requestedUnits: requested_units,
        expiresAtMs: auth.expires_at_ms,
        nonce: auth.nonce,
    }
}

/// The maker message declining to quote `rfq_id`.
pub fn rfq_v2_decline_message(
    rfq_id: Uuid,
    maker_account: WalletAddress,
    auth: RfqV2ActionAuth,
) -> DeclineRfqV2 {
    DeclineRfqV2 {
        rfqId: uuid_bytes(rfq_id),
        makerAccount: maker_account.inner(),
        operationId: uuid_bytes(auth.operation_id),
        expiresAtMs: auth.expires_at_ms,
        nonce: auth.nonce,
    }
}

/// The taker message for one execution (one allocation) or a sweep.
///
/// Refuses an empty list, more than [`MAX_RFQ_V2_EXECUTION_ALLOCATIONS`]
/// allocations, and a repeated quote ID, as the server does.
#[expect(
    clippy::result_large_err,
    reason = "public client methods preserve the established unboxed ClientError contract"
)]
pub fn rfq_v2_execution_message(
    rfq_id: Uuid,
    allocations: &[RfqV2SweepAllocation],
    max_fee_micros: u128,
    auth: RfqV2ActionAuth,
) -> Result<ExecuteRfqV2> {
    if allocations.is_empty() {
        return Err(invalid("RFQ V2 execution requires at least one allocation"));
    }
    if allocations.len() > MAX_RFQ_V2_EXECUTION_ALLOCATIONS {
        return Err(invalid(format!(
            "RFQ V2 execution names {} quotes, maximum is {MAX_RFQ_V2_EXECUTION_ALLOCATIONS}",
            allocations.len()
        )));
    }
    let mut seen = std::collections::HashSet::with_capacity(allocations.len());
    if let Some(repeat) = allocations
        .iter()
        .find(|allocation| !seen.insert(allocation.quote_id))
    {
        return Err(invalid(format!(
            "RFQ V2 execution repeats quote {}",
            repeat.quote_id
        )));
    }
    Ok(ExecuteRfqV2 {
        rfqId: uuid_bytes(rfq_id),
        operationId: uuid_bytes(auth.operation_id),
        allocations: allocations
            .iter()
            .map(|allocation| RfqV2ExecutionAllocation {
                quoteId: uuid_bytes(allocation.quote_id),
                packageUnits: allocation.package_units,
            })
            .collect(),
        maxFeeMicros: max_fee_micros,
        expiresAtMs: auth.expires_at_ms,
        nonce: auth.nonce,
    })
}

/// The message cancelling an RFQ (taker) or withdrawing a quote (maker).
pub fn rfq_v2_invalidation_message(
    target: RfqV2InvalidationTarget,
    auth: RfqV2ActionAuth,
) -> InvalidateRfqV2 {
    // Scope and reason are signed so a withdrawal cannot be replayed as a
    // cancellation. Reason 2 (replaced) is signed only through ReplaceRfqV2.
    let (rfq_id, quote_id, scope, reason) = match target {
        RfqV2InvalidationTarget::CancelRfq { rfq_id } => (rfq_id, FixedBytes::ZERO, 0, 0),
        RfqV2InvalidationTarget::WithdrawQuote { rfq_id, quote_id } => {
            (rfq_id, uuid_bytes(quote_id), 1, 1)
        }
    };
    InvalidateRfqV2 {
        rfqId: uuid_bytes(rfq_id),
        quoteId: quote_id,
        scope,
        reason,
        operationId: uuid_bytes(auth.operation_id),
        expiresAtMs: auth.expires_at_ms,
        nonce: auth.nonce,
    }
}

/// The maker message withdrawing a set of its quotes atomically.
///
/// Refuses an empty or oversized exact-ID list, a nil or repeated target, and
/// a nil RFQ ID, as the server does.
#[expect(
    clippy::result_large_err,
    reason = "public client methods preserve the established unboxed ClientError contract"
)]
pub fn rfq_v2_bulk_withdrawal_message(
    maker_account: WalletAddress,
    scope: &RfqV2BulkWithdrawalScope,
    auth: RfqV2ActionAuth,
) -> Result<BulkWithdrawRfqV2> {
    let (scope_tag, rfq_id, targets) = match scope {
        RfqV2BulkWithdrawalScope::QuoteIds { quote_ids } => {
            if quote_ids.is_empty() {
                return Err(invalid(
                    "RFQ V2 bulk withdrawal requires at least one quote",
                ));
            }
            if quote_ids.len() > MAX_RFQ_V2_BULK_WITHDRAWAL_QUOTES {
                return Err(invalid(format!(
                    "RFQ V2 bulk withdrawal names {} quotes, maximum is {MAX_RFQ_V2_BULK_WITHDRAWAL_QUOTES}",
                    quote_ids.len()
                )));
            }
            let mut seen = std::collections::HashSet::with_capacity(quote_ids.len());
            if quote_ids.iter().any(|target| {
                target.rfq_id.is_nil()
                    || target.quote_id.is_nil()
                    || !seen.insert((target.rfq_id, target.quote_id))
            }) {
                return Err(invalid(
                    "RFQ V2 bulk withdrawal contains a nil or repeated quote",
                ));
            }
            let targets = quote_ids
                .iter()
                .map(|target| RfqV2BulkWithdrawalTarget {
                    rfqId: uuid_bytes(target.rfq_id),
                    quoteId: uuid_bytes(target.quote_id),
                })
                .collect();
            (0, FixedBytes::ZERO, targets)
        }
        RfqV2BulkWithdrawalScope::Rfq { rfq_id } => {
            if rfq_id.is_nil() {
                return Err(invalid("RFQ V2 bulk withdrawal names the nil RFQ"));
            }
            (1, uuid_bytes(*rfq_id), Vec::new())
        }
        RfqV2BulkWithdrawalScope::AllOpen => (2, FixedBytes::ZERO, Vec::new()),
    };
    Ok(BulkWithdrawRfqV2 {
        makerAccount: maker_account.inner(),
        scope: scope_tag,
        rfqId: rfq_id,
        targets,
        operationId: uuid_bytes(auth.operation_id),
        expiresAtMs: auth.expires_at_ms,
        nonce: auth.nonce,
    })
}

/// The maker message setting (`Some`) or removing (`None`) its account-wide
/// quote cancel deadline. `Some(0)` is refused: zero is the removal encoding.
#[expect(
    clippy::result_large_err,
    reason = "public client methods preserve the established unboxed ClientError contract"
)]
pub fn rfq_v2_quote_cancel_schedule_message(
    maker_account: WalletAddress,
    cancel_at_ms: Option<u64>,
    auth: RfqV2ActionAuth,
) -> Result<ScheduleRfqQuoteCancelV2> {
    if cancel_at_ms == Some(0) {
        return Err(invalid(
            "RFQ V2 quote cancel deadline must be nonzero; pass None to remove it",
        ));
    }
    Ok(ScheduleRfqQuoteCancelV2 {
        operationId: uuid_bytes(auth.operation_id),
        makerAccount: maker_account.inner(),
        cancelAtMs: cancel_at_ms.unwrap_or(0),
        expiresAtMs: auth.expires_at_ms,
        nonce: auth.nonce,
    })
}

/// The message retiring `target` in favour of `successor_id`, the ID inside
/// the replacement's own signed terms.
pub fn rfq_v2_replacement_message(
    target: RfqV2ReplacementTarget,
    successor_id: Uuid,
    auth: RfqV2ActionAuth,
) -> ReplaceRfqV2 {
    let (rfq_id, old_object_id, scope) = match target {
        RfqV2ReplacementTarget::Rfq { rfq_id } => (rfq_id, rfq_id, 0),
        RfqV2ReplacementTarget::Quote { rfq_id, quote_id } => (rfq_id, quote_id, 1),
    };
    ReplaceRfqV2 {
        rfqId: uuid_bytes(rfq_id),
        oldObjectId: uuid_bytes(old_object_id),
        newObjectId: uuid_bytes(successor_id),
        scope,
        operationId: uuid_bytes(auth.operation_id),
        expiresAtMs: auth.expires_at_ms,
        nonce: auth.nonce,
    }
}

/// The reference premium a quote must sign for `reference_quantity` package
/// units at `priced_legs`, as `(maker_receives_reference_premium,
/// reference_premium_micros)`.
///
/// The server recomputes this from the parent's legs and refuses a quote
/// whose signed premium differs. A taker buy leg pays the maker and a taker
/// sell leg pays the taker; a zero net premium is `(false, 0)`. Pass the
/// parent's fill `step` for partial-fill policies (`None` for
/// `full_size_once`): the premium of one step must also be a whole number of
/// micros.
#[expect(
    clippy::result_large_err,
    reason = "public client methods preserve the established unboxed ClientError contract"
)]
pub fn rfq_v2_reference_premium(
    parent_legs: &[RfqV2PackageLeg],
    priced_legs: &[RfqV2PricedLeg],
    reference_quantity: u128,
    fill_step: Option<u128>,
) -> Result<(bool, u128)> {
    if parent_legs.len() != priced_legs.len() {
        return Err(invalid(format!(
            "RFQ V2 quote prices {} legs, the RFQ has {}",
            priced_legs.len(),
            parent_legs.len()
        )));
    }
    if reference_quantity == 0 {
        return Err(invalid("RFQ V2 reference quantity must be nonzero"));
    }
    let overflow = || invalid("RFQ V2 reference premium overflows");
    let mut maker_receives = U256::ZERO;
    let mut taker_receives = U256::ZERO;
    for (leg, priced) in parent_legs.iter().zip(priced_legs) {
        if leg.instrument != priced.instrument {
            return Err(invalid(format!(
                "RFQ V2 quote prices {} where the RFQ has {}",
                priced.instrument, leg.instrument
            )));
        }
        let scaled = U256::from(priced.price_micros)
            .checked_mul(U256::from(leg.quantity_micros_per_unit))
            .and_then(|value| value.checked_mul(U256::from(reference_quantity)))
            .ok_or_else(overflow)?;
        let side_total = match leg.taker_side {
            RfqV2Side::Buy => &mut maker_receives,
            RfqV2Side::Sell => &mut taker_receives,
        };
        *side_total = side_total.checked_add(scaled).ok_or_else(overflow)?;
    }
    let (receives, scaled) = if maker_receives > taker_receives {
        (true, maker_receives - taker_receives)
    } else {
        (false, taker_receives - maker_receives)
    };
    // Prices and quantities are both micros, so the product carries 1e12.
    let denominator = U256::from(1_000_000_u64) * U256::from(1_000_000_u64);
    if let Some(step) = fill_step {
        let per_unit = scaled / U256::from(reference_quantity);
        let per_step = per_unit
            .checked_mul(U256::from(step))
            .ok_or_else(overflow)?;
        if per_step % denominator != U256::ZERO {
            return Err(invalid(
                "RFQ V2 premium for one fill step is not a whole number of micros",
            ));
        }
    }
    if scaled % denominator != U256::ZERO {
        return Err(invalid(
            "RFQ V2 reference premium is not a whole number of micros",
        ));
    }
    let premium = u128::try_from(scaled / denominator).map_err(|_| overflow())?;
    Ok((receives, premium))
}

impl HypercallWallet {
    /// Sign RFQ terms for [`crate::HypercallClient::create_rfq_v2`].
    pub async fn sign_create_rfq_v2(
        &self,
        terms: RfqV2Terms,
        nonce: u64,
    ) -> Result<CreateRfqV2Request> {
        let message = rfq_v2_create_message(&terms, nonce)?;
        let signature = self.sign_rfq_v2_typed_data(&message, "RFQ V2").await?;
        Ok(CreateRfqV2Request {
            terms,
            nonce,
            signature,
        })
    }

    /// Sign a firm quote for
    /// [`crate::HypercallClient::publish_rfq_v2_firm_quote`].
    pub async fn sign_rfq_v2_firm_quote(
        &self,
        terms: RfqV2FirmQuoteTerms,
        nonce: u64,
    ) -> Result<PublishRfqV2FirmQuoteRequest> {
        let message = rfq_v2_firm_quote_message(&terms, nonce);
        let signature = self
            .sign_rfq_v2_typed_data(&message, "RFQ V2 firm quote")
            .await?;
        Ok(PublishRfqV2FirmQuoteRequest {
            terms,
            nonce,
            signature,
        })
    }

    /// Sign an indicative quote for
    /// [`crate::HypercallClient::publish_rfq_v2_indicative_quote`].
    pub async fn sign_rfq_v2_indicative_quote(
        &self,
        terms: RfqV2IndicativeQuoteTerms,
    ) -> Result<PublishRfqV2IndicativeQuoteRequest> {
        let message = rfq_v2_indicative_quote_message(&terms);
        let signature = self
            .sign_rfq_v2_typed_data(&message, "RFQ V2 indicative quote")
            .await?;
        Ok(PublishRfqV2IndicativeQuoteRequest { terms, signature })
    }

    /// Sign a firm-up request for
    /// [`crate::HypercallClient::request_rfq_v2_firm_quote`].
    pub async fn sign_rfq_v2_firm_up(
        &self,
        rfq_id: Uuid,
        indicative_quote_id: Uuid,
        requested_units: u128,
        auth: RfqV2ActionAuth,
    ) -> Result<RequestRfqV2FirmQuoteRequest> {
        let message = rfq_v2_firm_up_message(rfq_id, indicative_quote_id, requested_units, auth);
        let signature = self
            .sign_rfq_v2_typed_data(&message, "RFQ V2 firm-up request")
            .await?;
        Ok(RequestRfqV2FirmQuoteRequest {
            operation_id: auth.operation_id,
            requested_units,
            expires_at_ms: auth.expires_at_ms,
            nonce: auth.nonce,
            signature,
        })
    }

    /// Sign a maker decline for [`crate::HypercallClient::decline_rfq_v2`].
    pub async fn sign_rfq_v2_decline(
        &self,
        rfq_id: Uuid,
        maker_account: WalletAddress,
        auth: RfqV2ActionAuth,
    ) -> Result<DeclineRfqV2Request> {
        let message = rfq_v2_decline_message(rfq_id, maker_account, auth);
        let signature = self
            .sign_rfq_v2_typed_data(&message, "RFQ V2 decline")
            .await?;
        Ok(DeclineRfqV2Request {
            operation_id: auth.operation_id,
            maker_account,
            expires_at_ms: auth.expires_at_ms,
            nonce: auth.nonce,
            signature,
        })
    }

    /// Sign one execution against one firm quote for
    /// [`crate::HypercallClient::execute_rfq_v2`].
    pub async fn sign_rfq_v2_execution(
        &self,
        rfq_id: Uuid,
        quote_id: Uuid,
        package_units: u128,
        max_fee_micros: u128,
        auth: RfqV2ActionAuth,
    ) -> Result<RfqV2ExecutionRequest> {
        let allocation = RfqV2SweepAllocation {
            quote_id,
            package_units,
        };
        let message = rfq_v2_execution_message(rfq_id, &[allocation], max_fee_micros, auth)?;
        let signature = self
            .sign_rfq_v2_typed_data(&message, "RFQ V2 execution")
            .await?;
        Ok(RfqV2ExecutionRequest {
            operation_id: auth.operation_id,
            quote_id,
            package_units,
            max_fee_micros,
            expires_at_ms: auth.expires_at_ms,
            nonce: auth.nonce,
            signature,
        })
    }

    /// Sign an ordered, atomic sweep for
    /// [`crate::HypercallClient::sweep_rfq_v2`].
    pub async fn sign_rfq_v2_sweep(
        &self,
        rfq_id: Uuid,
        allocations: Vec<RfqV2SweepAllocation>,
        max_fee_micros: u128,
        auth: RfqV2ActionAuth,
    ) -> Result<RfqV2SweepRequest> {
        let message = rfq_v2_execution_message(rfq_id, &allocations, max_fee_micros, auth)?;
        let signature = self
            .sign_rfq_v2_typed_data(&message, "RFQ V2 sweep")
            .await?;
        Ok(RfqV2SweepRequest {
            operation_id: auth.operation_id,
            allocations,
            max_fee_micros,
            expires_at_ms: auth.expires_at_ms,
            nonce: auth.nonce,
            signature,
        })
    }

    /// Sign a taker cancel for [`crate::HypercallClient::cancel_rfq_v2`] or a
    /// maker withdrawal for [`crate::HypercallClient::withdraw_rfq_v2_quote`].
    pub async fn sign_rfq_v2_invalidation(
        &self,
        target: RfqV2InvalidationTarget,
        auth: RfqV2ActionAuth,
    ) -> Result<RfqV2InvalidationRequest> {
        let message = rfq_v2_invalidation_message(target, auth);
        let signature = self
            .sign_rfq_v2_typed_data(&message, "RFQ V2 invalidation")
            .await?;
        Ok(RfqV2InvalidationRequest {
            operation_id: auth.operation_id,
            expires_at_ms: auth.expires_at_ms,
            nonce: auth.nonce,
            signature,
        })
    }

    /// Sign a bulk withdrawal for
    /// [`crate::HypercallClient::withdraw_rfq_v2_quotes`].
    pub async fn sign_rfq_v2_bulk_withdrawal(
        &self,
        maker_account: WalletAddress,
        scope: RfqV2BulkWithdrawalScope,
        auth: RfqV2ActionAuth,
    ) -> Result<RfqV2BulkWithdrawalRequest> {
        let message = rfq_v2_bulk_withdrawal_message(maker_account, &scope, auth)?;
        let signature = self
            .sign_rfq_v2_typed_data(&message, "RFQ V2 bulk withdrawal")
            .await?;
        Ok(RfqV2BulkWithdrawalRequest {
            operation_id: auth.operation_id,
            maker_account,
            scope,
            expires_at_ms: auth.expires_at_ms,
            nonce: auth.nonce,
            signature,
        })
    }

    /// Sign the maker dead-man switch for
    /// [`crate::HypercallClient::schedule_rfq_v2_quote_cancel`]. `None`
    /// removes the schedule.
    pub async fn sign_rfq_v2_quote_cancel_schedule(
        &self,
        maker_account: WalletAddress,
        cancel_at_ms: Option<u64>,
        auth: RfqV2ActionAuth,
    ) -> Result<RfqV2ScheduledQuoteCancelRequest> {
        let message = rfq_v2_quote_cancel_schedule_message(maker_account, cancel_at_ms, auth)?;
        let signature = self
            .sign_rfq_v2_typed_data(&message, "RFQ V2 quote cancel schedule")
            .await?;
        Ok(RfqV2ScheduledQuoteCancelRequest {
            operation_id: auth.operation_id,
            maker_account,
            cancel_at_ms,
            expires_at_ms: auth.expires_at_ms,
            nonce: auth.nonce,
            signature,
        })
    }

    /// Sign the replacement of `rfq_id` by an already signed successor RFQ,
    /// for [`crate::HypercallClient::replace_rfq_v2`].
    pub async fn sign_rfq_v2_replacement(
        &self,
        rfq_id: Uuid,
        replacement: CreateRfqV2Request,
        auth: RfqV2ActionAuth,
    ) -> Result<RfqV2ReplacementRequest<CreateRfqV2Request>> {
        let message = rfq_v2_replacement_message(
            RfqV2ReplacementTarget::Rfq { rfq_id },
            replacement.terms.id,
            auth,
        );
        let signature = self
            .sign_rfq_v2_typed_data(&message, "RFQ V2 replacement")
            .await?;
        Ok(RfqV2ReplacementRequest {
            operation_id: auth.operation_id,
            replacement,
            expires_at_ms: auth.expires_at_ms,
            nonce: auth.nonce,
            signature,
        })
    }

    /// Sign the replacement of quote `quote_id` by an already signed
    /// successor quote, for [`crate::HypercallClient::replace_rfq_v2_quote`].
    pub async fn sign_rfq_v2_quote_replacement(
        &self,
        rfq_id: Uuid,
        quote_id: Uuid,
        replacement: RfqV2QuoteReplacement,
        auth: RfqV2ActionAuth,
    ) -> Result<RfqV2ReplacementRequest<RfqV2QuoteReplacement>> {
        let successor_id = match &replacement {
            RfqV2QuoteReplacement::Indicative(quote) => quote.terms.id,
            RfqV2QuoteReplacement::Firm(quote) => quote.terms.id,
        };
        let message = rfq_v2_replacement_message(
            RfqV2ReplacementTarget::Quote { rfq_id, quote_id },
            successor_id,
            auth,
        );
        let signature = self
            .sign_rfq_v2_typed_data(&message, "RFQ V2 quote replacement")
            .await?;
        Ok(RfqV2ReplacementRequest {
            operation_id: auth.operation_id,
            replacement,
            expires_at_ms: auth.expires_at_ms,
            nonce: auth.nonce,
            signature,
        })
    }
}

#[expect(
    clippy::result_large_err,
    reason = "public client methods preserve the established unboxed ClientError contract"
)]
fn rfq_v2_signing_terms(terms: &RfqV2Terms) -> Result<RfqV2SigningTerms> {
    let (fill_policy, minimum_units, step_units) = fill_policy_fields(terms.fill_policy);
    let (routing_mode, selected_makers) = match &terms.routing {
        RfqV2Routing::AllEligible => (0, Vec::new()),
        RfqV2Routing::SelectedMakers { makers } => {
            // The server signs the canonical (sorted, unique) set. Build it
            // with `RfqV2Routing::selected_makers` rather than by hand.
            if makers.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(invalid(
                    "RFQ V2 selected makers must be sorted ascending without duplicates; \
                     build them with RfqV2Routing::selected_makers",
                ));
            }
            (1, makers.iter().map(|maker| maker.inner()).collect())
        }
    };
    let (auto_maker_receives_premium, auto_premium_limit_micros) =
        match (terms.execution_mode, terms.auto_execution_limit) {
            (RfqV2ExecutionMode::Manual, None) => (false, 0),
            (RfqV2ExecutionMode::Auto, Some(limit)) => {
                (limit.maker_receives_premium, limit.premium_micros)
            }
            (RfqV2ExecutionMode::Manual, Some(_)) => {
                return Err(invalid(
                    "RFQ V2 manual execution must not carry an auto_execution_limit",
                ))
            }
            (RfqV2ExecutionMode::Auto, None) => {
                return Err(invalid(
                    "RFQ V2 auto execution requires an auto_execution_limit",
                ))
            }
        };
    Ok(RfqV2SigningTerms {
        rfqId: uuid_bytes(terms.id),
        takerAccount: terms.taker_account.inner(),
        legs: terms
            .legs
            .iter()
            .map(|leg| RfqV2SigningLeg {
                instrumentId: rfq_v2_instrument_id(&leg.instrument),
                takerSide: match leg.taker_side {
                    RfqV2Side::Buy => 0,
                    RfqV2Side::Sell => 1,
                },
                quantityMicrosPerUnit: leg.quantity_micros_per_unit,
            })
            .collect(),
        targetUnits: terms.target,
        fillPolicy: fill_policy,
        minimumUnits: minimum_units,
        stepUnits: step_units,
        executionMode: match terms.execution_mode {
            RfqV2ExecutionMode::Manual => 0,
            RfqV2ExecutionMode::Auto => 1,
        },
        autoMakerReceivesPremium: auto_maker_receives_premium,
        autoPremiumLimitMicros: auto_premium_limit_micros,
        quotePolicy: match terms.quote_policy {
            RfqV2QuotePolicy::FirmOnly => 0,
            RfqV2QuotePolicy::IndicativeAllowed => 1,
        },
        takerIdentity: match terms.taker_identity {
            RfqV2TakerIdentity::Anonymous => 0,
            RfqV2TakerIdentity::Disclosed => 1,
        },
        routingMode: routing_mode,
        selectedMakers: selected_makers,
        quoteDeadlineMs: terms.quote_deadline_ms,
        expiresAtMs: terms.expires_at_ms,
        feeLimits: fee_limits(terms.fee_limits),
        builder: terms
            .builder
            .map_or(Address::ZERO, |builder| builder.recipient.inner()),
        privateTermsBlinding: terms.private_terms_blinding.0,
    })
}

fn fill_policy_fields(policy: RfqV2FillPolicy) -> (u8, u128, u128) {
    match policy {
        RfqV2FillPolicy::FullSizeOnce => (0, 0, 0),
        RfqV2FillPolicy::OneSmallerPackage { minimum, step } => (1, minimum, step),
        RfqV2FillPolicy::RepeatedPartial { minimum, step } => (2, minimum, step),
    }
}

fn fee_limits(limits: RfqV2FeeLimits) -> RfqV2SigningFeeLimits {
    RfqV2SigningFeeLimits {
        perExecutionMicros: limits.per_execution,
        cumulativeMicros: limits.cumulative,
    }
}

fn priced_legs(legs: &[RfqV2PricedLeg]) -> Vec<RfqV2SigningPriceLeg> {
    legs.iter()
        .map(|leg| RfqV2SigningPriceLeg {
            instrumentId: rfq_v2_instrument_id(&leg.instrument),
            priceMicros: leg.price_micros,
        })
        .collect()
}

fn uuid_bytes(id: Uuid) -> FixedBytes<16> {
    FixedBytes::from(*id.as_bytes())
}

fn invalid(message: impl Into<String>) -> ClientError {
    ClientError::InvalidInput(message.into())
}

#[cfg(test)]
#[path = "rfq_v2_signing_test.rs"]
mod tests;
