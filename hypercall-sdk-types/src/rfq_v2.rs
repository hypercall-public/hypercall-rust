//! Public wire contract for the native RFQ v2 API.
//!
//! These DTOs describe HTTP payloads only. Exact financial amounts are
//! decimal JSON strings at every magnitude.
//!
//! Compiled with the `rfq-v2` cargo feature (on by default since 0.3.0). An
//! environment that has not enabled RFQ V2 answers every route with HTTP 403
//! `rfq_v2_disabled`. Client-chosen operation IDs must be UUIDv7s at
//! most one hour old and at most 30 seconds in the future at the server, or
//! the server answers HTTP 400 `rfq_v2_invalid_operation_id`.

use alloy::primitives::B256;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::{CursorPage, WalletAddress};

mod json_u128 {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

mod optional_json_u128 {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &Option<u128>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => super::json_u128::serialize(value, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<u128>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<String>::deserialize(deserializer)?
            .map(|value| value.parse().map_err(D::Error::custom))
            .transpose()
    }
}

fn serialize_b256<S>(value: &B256, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&format!("{value:#x}"))
}

fn deserialize_b256<'de, D>(deserializer: D) -> Result<B256, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    let Some(hex) = value.strip_prefix("0x") else {
        return Err(serde::de::Error::custom(
            "bytes32 must be a 0x-prefixed hex string",
        ));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(serde::de::Error::custom(
            "bytes32 must contain exactly 64 hex characters",
        ));
    }
    value.parse().map_err(serde::de::Error::custom)
}

/// A recoverable ECDSA signature encoded as exactly 65 bytes on the V2 JSON wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RfqV2Signature([u8; 65]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[displaystr::display]
pub enum RfqV2SignatureError {
    InvalidEncoding = "signature must be 0x followed by 130 lowercase hex characters",
    InvalidRecoveryId = "signature recovery ID must be 0, 1, 27, or 28",
}

impl RfqV2Signature {
    pub fn from_bytes(bytes: [u8; 65]) -> Result<Self, RfqV2SignatureError> {
        if !matches!(bytes[64], 0 | 1 | 27 | 28) {
            return Err(RfqV2SignatureError::InvalidRecoveryId);
        }
        Ok(Self(bytes))
    }

    pub const fn as_bytes(&self) -> &[u8; 65] {
        &self.0
    }
}

impl std::str::FromStr for RfqV2Signature {
    type Err = RfqV2SignatureError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let hex = value
            .strip_prefix("0x")
            .ok_or(RfqV2SignatureError::InvalidEncoding)?;
        if hex.len() != 130
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(RfqV2SignatureError::InvalidEncoding);
        }
        let mut bytes = [0; 65];
        alloy::hex::decode_to_slice(hex, &mut bytes)
            .map_err(|_| RfqV2SignatureError::InvalidEncoding)?;
        Self::from_bytes(bytes)
    }
}

impl Serialize for RfqV2Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("0x{}", alloy::hex::encode(self.0)))
    }
}

impl<'de> Deserialize<'de> for RfqV2Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error as _;
        String::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

/// A reusable, short-lived wallet-signed private RFQ read grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RfqV2ReadGrant {
    pub account: WalletAddress,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub nonce: u64,
    pub signature: RfqV2Signature,
}

/// A bytes32 value represented as a `0x`-prefixed JSON string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RfqV2Digest(
    #[serde(
        serialize_with = "serialize_b256",
        deserialize_with = "deserialize_b256"
    )]
    pub B256,
);

impl From<B256> for RfqV2Digest {
    fn from(value: B256) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfqV2TakerIdentity {
    Anonymous,
    Disclosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfqV2Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfqV2ExecutionMode {
    Manual,
    Auto,
}

/// Whether the parent accepts indicative quotes. Signed, so it is always
/// explicit on the wire: an omitted policy fails to decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfqV2QuotePolicy {
    FirmOnly,
    IndicativeAllowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfqV2WithdrawalPolicy {
    NotPermitted,
    Permitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum RfqV2FillPolicy {
    FullSizeOnce,
    OneSmallerPackage {
        #[serde(with = "json_u128")]
        minimum: u128,
        #[serde(with = "json_u128")]
        step: u128,
    },
    RepeatedPartial {
        #[serde(with = "json_u128")]
        minimum: u128,
        #[serde(with = "json_u128")]
        step: u128,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum RfqV2Routing {
    AllEligible,
    SelectedMakers { makers: Vec<WalletAddress> },
}

impl RfqV2Routing {
    /// Build the canonical signed representation of a maker-address group.
    pub fn selected_makers(
        mut makers: Vec<WalletAddress>,
        maximum: usize,
    ) -> Result<Self, RfqV2RoutingError> {
        if makers.is_empty() {
            return Err(RfqV2RoutingError::Empty);
        }
        if makers.len() > maximum {
            return Err(RfqV2RoutingError::TooMany {
                actual: makers.len(),
                maximum,
            });
        }
        if let Some(index) = makers.iter().position(|maker| maker.inner().is_zero()) {
            return Err(RfqV2RoutingError::ZeroAddress { index });
        }
        makers.sort_unstable();
        if let Some(pair) = makers.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(RfqV2RoutingError::Duplicate { address: pair[0] });
        }
        Ok(Self::SelectedMakers { makers })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[displaystr::display]
pub enum RfqV2RoutingError {
    Empty = "selected maker group must not be empty",
    TooMany {
        actual: usize,
        maximum: usize,
    } = "selected maker group contains {actual} addresses, maximum is {maximum}",
    ZeroAddress {
        index: usize,
    } = "selected maker at index {index} is the zero address",
    Duplicate {
        address: WalletAddress,
    } = "selected maker group contains duplicate {address}",
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2PackageLeg {
    /// Public canonical instrument symbol, resolved by the API at admission.
    pub instrument: String,
    pub taker_side: RfqV2Side,
    #[serde(with = "json_u128")]
    pub quantity_micros_per_unit: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2PricedLeg {
    pub instrument: String,
    #[serde(with = "json_u128")]
    pub price_micros: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2AutoExecutionLimit {
    pub maker_receives_premium: bool,
    #[serde(with = "json_u128")]
    pub premium_micros: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2FeeLimits {
    #[serde(with = "json_u128")]
    pub per_execution: u128,
    #[serde(with = "json_u128")]
    pub cumulative: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2BuilderAttribution {
    pub recipient: WalletAddress,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2Terms {
    pub id: Uuid,
    pub taker_account: WalletAddress,
    pub legs: Vec<RfqV2PackageLeg>,
    #[serde(with = "json_u128")]
    pub target: u128,
    pub fill_policy: RfqV2FillPolicy,
    pub execution_mode: RfqV2ExecutionMode,
    #[serde(default)]
    pub auto_execution_limit: Option<RfqV2AutoExecutionLimit>,
    pub quote_policy: RfqV2QuotePolicy,
    pub taker_identity: RfqV2TakerIdentity,
    pub routing: RfqV2Routing,
    pub quote_deadline_ms: u64,
    pub expires_at_ms: u64,
    pub fee_limits: RfqV2FeeLimits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builder: Option<RfqV2BuilderAttribution>,
    pub private_terms_blinding: RfqV2Digest,
}

/// Economic parent terms visible to a selected maker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2MakerTerms {
    pub id: Uuid,
    /// Present only when the signed taker identity policy is disclosed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub taker_account: Option<WalletAddress>,
    pub legs: Vec<RfqV2PackageLeg>,
    #[serde(with = "json_u128")]
    pub target: u128,
    pub fill_policy: RfqV2FillPolicy,
    pub execution_mode: RfqV2ExecutionMode,
    #[serde(default)]
    pub auto_execution_limit: Option<RfqV2AutoExecutionLimit>,
    pub quote_policy: RfqV2QuotePolicy,
    pub taker_identity: RfqV2TakerIdentity,
    pub quote_deadline_ms: u64,
    pub expires_at_ms: u64,
    pub fee_limits: RfqV2FeeLimits,
}

/// Role-aware parent payload returned by authorized RFQ reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "view", rename_all = "snake_case")]
pub enum RfqV2AuthorizedTerms {
    Taker { terms: RfqV2Terms },
    Maker { terms: RfqV2MakerTerms },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRfqV2Request {
    pub terms: RfqV2Terms,
    pub nonce: u64,
    pub signature: RfqV2Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRfqV2Response {
    pub rfq_id: Uuid,
    pub status: String,
    pub terms_commitment: RfqV2Digest,
    pub quote_deadline_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2IndicativeQuoteTerms {
    pub id: Uuid,
    pub rfq_id: Uuid,
    pub rfq_terms_commitment: RfqV2Digest,
    pub maker_account: WalletAddress,
    pub priced_legs: Vec<RfqV2PricedLeg>,
    #[serde(with = "json_u128")]
    pub reference_quantity: u128,
    pub maker_receives_reference_premium: bool,
    #[serde(with = "json_u128")]
    pub reference_premium_micros: u128,
    #[serde(with = "json_u128")]
    pub indicated_capacity: u128,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishRfqV2IndicativeQuoteRequest {
    pub terms: RfqV2IndicativeQuoteTerms,
    pub signature: RfqV2Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2FirmQuoteTerms {
    pub id: Uuid,
    pub rfq_id: Uuid,
    pub rfq_terms_commitment: RfqV2Digest,
    pub maker_account: WalletAddress,
    pub maker_generation: u64,
    pub rfq_generation: u64,
    pub priced_legs: Vec<RfqV2PricedLeg>,
    #[serde(with = "json_u128")]
    pub reference_quantity: u128,
    pub maker_receives_reference_premium: bool,
    #[serde(with = "json_u128")]
    pub reference_premium_micros: u128,
    #[serde(with = "json_u128")]
    pub capacity: u128,
    pub fill_policy: RfqV2FillPolicy,
    pub withdrawal_policy: RfqV2WithdrawalPolicy,
    pub fee_limits: RfqV2FeeLimits,
    pub expires_at_ms: u64,
    #[serde(default)]
    pub protection_scope: Option<RfqV2Digest>,
    #[serde(default)]
    pub source_indicative_quote_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishRfqV2FirmQuoteRequest {
    pub terms: RfqV2FirmQuoteTerms,
    pub nonce: u64,
    pub signature: RfqV2Signature,
}

/// Fresh signed quote carried by the quote-replacement endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RfqV2QuoteReplacement {
    Indicative(PublishRfqV2IndicativeQuoteRequest),
    Firm(PublishRfqV2FirmQuoteRequest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfqV2Firmness {
    Indicative,
    Firm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfqV2Role {
    Taker,
    Maker,
    Either,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfqV2PreflightStatus {
    Passed,
    UnavailableAdmitted,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2PreflightResult {
    pub status: RfqV2PreflightStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_revision: Option<u64>,
    pub checked_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "firmness", rename_all = "snake_case")]
pub enum PublishRfqV2QuoteResponse {
    Indicative {
        quote_id: Uuid,
        status: String,
        executable: bool,
        #[serde(with = "json_u128")]
        indicated_capacity: u128,
    },
    Firm {
        quote_id: Uuid,
        status: String,
        executable: bool,
        #[serde(with = "json_u128")]
        remaining_capacity: u128,
        preflight: RfqV2PreflightResult,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestRfqV2FirmQuoteRequest {
    pub operation_id: Uuid,
    #[serde(with = "json_u128")]
    pub requested_units: u128,
    pub expires_at_ms: u64,
    pub nonce: u64,
    pub signature: RfqV2Signature,
}

/// A routed maker's signed decline of one RFQ. Visible only to the taker and
/// the declining maker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclineRfqV2Request {
    pub operation_id: Uuid,
    pub maker_account: WalletAddress,
    pub expires_at_ms: u64,
    pub nonce: u64,
    pub signature: RfqV2Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2ExecutionRequest {
    pub operation_id: Uuid,
    pub quote_id: Uuid,
    #[serde(with = "json_u128")]
    pub package_units: u128,
    #[serde(with = "json_u128")]
    pub max_fee_micros: u128,
    pub expires_at_ms: u64,
    pub nonce: u64,
    pub signature: RfqV2Signature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2SweepAllocation {
    pub quote_id: Uuid,
    #[serde(with = "json_u128")]
    pub package_units: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2SweepRequest {
    pub operation_id: Uuid,
    pub allocations: Vec<RfqV2SweepAllocation>,
    #[serde(with = "json_u128")]
    pub max_fee_micros: u128,
    pub expires_at_ms: u64,
    pub nonce: u64,
    pub signature: RfqV2Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2ExecutionFill {
    pub execution_id: Uuid,
    pub quote_id: Uuid,
    /// The financial account that took the maker side.
    pub maker_account: WalletAddress,
    /// The registered provider whose enrollment routed `maker_account`.
    pub maker_provider: WalletAddress,
    /// The recovered signer of the executed quote.
    pub maker_signer: WalletAddress,
    #[serde(with = "json_u128")]
    pub package_units: u128,
    #[serde(with = "json_u128")]
    pub fee_micros: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2ExecutionResponse {
    pub operation_id: Uuid,
    pub status: String,
    pub executions: Vec<RfqV2ExecutionFill>,
    #[serde(with = "json_u128")]
    pub rfq_remaining_target: u128,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_json_u128"
    )]
    pub quote_remaining_capacity: Option<u128>,
}

/// Signed lifecycle action used for cancel and withdrawal operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2InvalidationRequest {
    pub operation_id: Uuid,
    pub expires_at_ms: u64,
    pub nonce: u64,
    pub signature: RfqV2Signature,
}

/// Identifies a firm quote within its parent RFQ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2QuoteRef {
    pub rfq_id: Uuid,
    pub quote_id: Uuid,
}

/// Atomic scope for retiring more than one open quote owned by a maker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum RfqV2BulkWithdrawalScope {
    QuoteIds { quote_ids: Vec<RfqV2QuoteRef> },
    Rfq { rfq_id: Uuid },
    AllOpen,
}

/// Signed maker operation that withdraws its resolved quote set atomically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2BulkWithdrawalRequest {
    pub operation_id: Uuid,
    pub maker_account: WalletAddress,
    pub scope: RfqV2BulkWithdrawalScope,
    pub expires_at_ms: u64,
    pub nonce: u64,
    pub signature: RfqV2Signature,
}

/// Maker-owned dead-man switch. Repeating this operation moves the deadline;
/// omitting it removes the schedule. The signature binds both choices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2ScheduledQuoteCancelRequest {
    pub operation_id: Uuid,
    pub maker_account: WalletAddress,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_at_ms: Option<u64>,
    pub expires_at_ms: u64,
    pub nonce: u64,
    pub signature: RfqV2Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2ScheduledQuoteCancelResponse {
    pub operation_id: Uuid,
    pub cancel_at_ms: Option<u64>,
}

/// Replacement couples a fresh signed object with authorization to retire the old one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2ReplacementRequest<T> {
    pub operation_id: Uuid,
    pub replacement: T,
    pub expires_at_ms: u64,
    pub nonce: u64,
    pub signature: RfqV2Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2OperationResponse {
    pub operation_id: Uuid,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2QuoteSummary {
    pub quote_id: Uuid,
    pub firmness: RfqV2Firmness,
    pub status: String,
    pub executable: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_json_u128"
    )]
    pub indicated_capacity: Option<u128>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_json_u128"
    )]
    pub original_capacity: Option<u128>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_json_u128"
    )]
    pub remaining_capacity: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_indicative_quote_id: Option<Uuid>,
}

/// Authorized full quote view returned by quote reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "firmness", rename_all = "snake_case")]
pub enum RfqV2QuoteView {
    Indicative {
        terms: RfqV2IndicativeQuoteTerms,
        status: String,
        executable: bool,
        /// The registered provider whose enrollment routed the maker account.
        provider: WalletAddress,
        /// The recovered quote signer, authorized for the maker account.
        signer: WalletAddress,
    },
    Firm {
        terms: RfqV2FirmQuoteTerms,
        status: String,
        executable: bool,
        /// The registered provider whose enrollment routed the maker account.
        provider: WalletAddress,
        /// The recovered quote signer, authorized for the maker account.
        signer: WalletAddress,
        #[serde(with = "json_u128")]
        remaining_capacity: u128,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preflight: Option<RfqV2PreflightResult>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2Response {
    pub rfq_id: Uuid,
    pub status: String,
    pub terms_commitment: RfqV2Digest,
    pub parent: RfqV2AuthorizedTerms,
    #[serde(with = "json_u128")]
    pub target: u128,
    #[serde(with = "json_u128")]
    pub remaining_target: u128,
    pub quote_policy: RfqV2QuotePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maker_quote_generation: Option<RfqV2MakerQuoteGeneration>,
    /// Makers that declined: every one for the taker, only itself for a maker.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declined_makers: Vec<WalletAddress>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2MakerQuoteGeneration {
    pub maker_generation: u64,
    pub rfq_generation: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListRfqV2Query {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<RfqV2Role>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListRfqV2QuotesQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmness: Option<RfqV2Firmness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_terminal: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRfqV2Response {
    pub rfqs: Vec<RfqV2Response>,
    pub page: CursorPage,
    pub as_of_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRfqV2QuotesResponse {
    pub quotes: Vec<RfqV2QuoteView>,
    pub page: CursorPage,
    pub as_of_sequence: u64,
}

/// Cursor through the changes visible to one signed read account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListRfqV2ChangesQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2ChangeHint {
    pub rfq_id: Uuid,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListRfqV2ChangesResponse {
    pub changes: Vec<RfqV2ChangeHint>,
    pub next_after_sequence: u64,
    pub has_more: bool,
}

/// Stable error body returned by disabled route skeletons and later V2 handlers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2ErrorResponse {
    pub code: String,
    pub message: String,
}

#[cfg(test)]
#[path = "rfq_v2_test.rs"]
mod tests;
