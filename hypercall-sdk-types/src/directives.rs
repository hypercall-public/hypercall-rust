//! Public request and response types for managed-account directives.

use alloy::primitives::B256;

use crate::{PerpTimeInForce, WalletAddress};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
    if hex.len() != 64 {
        return Err(serde::de::Error::custom(
            "bytes32 must be exactly 32 bytes (64 hex chars)",
        ));
    }
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(serde::de::Error::custom("bytes32 contains invalid hex"));
    }
    value.parse().map_err(serde::de::Error::custom)
}

pub const JSON_SAFE_INTEGER_MAX: u64 = 9_007_199_254_740_991;

fn serialize_safe_u64<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if *value <= JSON_SAFE_INTEGER_MAX {
        serializer.serialize_u64(*value)
    } else {
        serializer.serialize_str(&value.to_string())
    }
}

fn serialize_safe_u128<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if *value <= u128::from(JSON_SAFE_INTEGER_MAX) {
        serializer.serialize_u64(*value as u64)
    } else {
        serializer.serialize_str(&value.to_string())
    }
}

fn deserialize_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    if let Some(number) = value.as_u64() {
        if number > JSON_SAFE_INTEGER_MAX {
            return Err(serde::de::Error::custom(
                "JSON number exceeds the safe integer range; provide a decimal string",
            ));
        }
        return Ok(number);
    }
    value
        .as_str()
        .ok_or_else(|| {
            serde::de::Error::custom("expected a non-negative integer or decimal string")
        })?
        .parse::<u64>()
        .map_err(serde::de::Error::custom)
}

fn deserialize_u128<'de, D>(deserializer: D) -> Result<u128, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    if let Some(number) = value.as_u64() {
        if number > JSON_SAFE_INTEGER_MAX {
            return Err(serde::de::Error::custom(
                "JSON number exceeds the safe integer range; provide a decimal string",
            ));
        }
        return Ok(u128::from(number));
    }
    value
        .as_str()
        .ok_or_else(|| {
            serde::de::Error::custom("expected a non-negative integer or decimal string")
        })?
        .parse::<u128>()
        .map_err(serde::de::Error::custom)
}

fn serialize_perp_tif<S>(value: &PerpTimeInForce, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_u8(value.encoded())
}

fn deserialize_perp_tif<'de, D>(deserializer: D) -> Result<PerpTimeInForce, D::Error>
where
    D: Deserializer<'de>,
{
    match u8::deserialize(deserializer)? {
        1 => Ok(PerpTimeInForce::Alo),
        2 => Ok(PerpTimeInForce::Gtc),
        3 => Ok(PerpTimeInForce::Ioc),
        value => Err(serde::de::Error::custom(format!(
            "unsupported perp time-in-force encoding: {value}"
        ))),
    }
}

/// Signed request accepted by one `/v1/actions/{action_key}` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectiveSubmitRequest<T> {
    pub account: WalletAddress,
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub nonce: u64,
    pub action: T,
    pub signature: String,
}

/// Managed HyperCore perp limit order action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HlLimitOrderAction {
    pub asset: u32,
    pub is_buy: bool,
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
    #[serde(
        rename = "encodedTif",
        serialize_with = "serialize_perp_tif",
        deserialize_with = "deserialize_perp_tif"
    )]
    pub tif: PerpTimeInForce,
    #[serde(
        serialize_with = "serialize_safe_u128",
        deserialize_with = "deserialize_u128"
    )]
    pub cloid: u128,
}

/// Managed HyperCore cancellation by exchange order ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HlCancelByOidAction {
    pub asset: u32,
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub oid: u64,
}

/// Managed HyperCore cancellation by client order ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HlCancelByCloidAction {
    pub asset: u32,
    #[serde(
        serialize_with = "serialize_safe_u128",
        deserialize_with = "deserialize_u128"
    )]
    pub cloid: u128,
}

/// HyperCore account abstraction modes supported by managed Account directives.
///
/// Hypercall currently permits only the forward transition to unified-account
/// mode. Additional variants must not be added until the server and Account
/// contract accept the corresponding action value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HypercoreAccountAbstraction {
    UnifiedAccount = 2,
}

impl HypercoreAccountAbstraction {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

impl Serialize for HypercoreAccountAbstraction {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(self.as_u8())
    }
}

impl<'de> Deserialize<'de> for HypercoreAccountAbstraction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match u8::deserialize(deserializer)? {
            2 => Ok(Self::UnifiedAccount),
            value => Err(serde::de::Error::custom(format!(
                "unsupported managed account abstraction: {value}"
            ))),
        }
    }
}

/// Manager-authorized action that changes a managed Account's HyperCore mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HlSetAbstractionAction {
    pub user: WalletAddress,
    pub abstraction: HypercoreAccountAbstraction,
}

/// Manager-authorized action that adds, replaces, or removes an API wallet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HcUpdateApiWalletAction {
    /// bytes32 identifier used to select the API-wallet slot.
    #[serde(
        serialize_with = "serialize_b256",
        deserialize_with = "deserialize_b256"
    )]
    pub name: B256,
    /// New API-wallet address. The zero address removes the named wallet.
    pub addr: WalletAddress,
}

/// Signed managed-perp limit order request body.
pub type PerpLimitOrderRequest = DirectiveSubmitRequest<HlLimitOrderAction>;
/// Signed managed-perp cancellation request body for an exchange order ID.
pub type PerpCancelByOidRequest = DirectiveSubmitRequest<HlCancelByOidAction>;
/// Signed managed-perp cancellation request body for a client order ID.
pub type PerpCancelByCloidRequest = DirectiveSubmitRequest<HlCancelByCloidAction>;
/// Signed managed-account abstraction request body.
pub type SetAccountAbstractionRequest = DirectiveSubmitRequest<HlSetAbstractionAction>;
/// Signed managed-account API-wallet update request body.
pub type UpdateApiWalletRequest = DirectiveSubmitRequest<HcUpdateApiWalletAction>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectiveStage {
    Rejected,
    Enqueued,
    Submitted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectiveRejection {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectiveFill {
    pub coin: String,
    pub side: String,
    pub size: String,
    pub price: String,
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub time: u64,
}

/// Honest result returned from a directive submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectiveSubmitResponse {
    pub stage: DirectiveStage,
    pub directive_id: String,
    pub action_key: String,
    pub account: WalletAddress,
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub nonce: u64,
    pub recovered_signer: Option<WalletAddress>,
    pub tx_hash: Option<String>,
    pub rejection: Option<DirectiveRejection>,
    pub fills: Option<Vec<DirectiveFill>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectiveDomainStatus {
    Accepted,
    Rejected,
    PendingChainEffect,
    PendingManualReview,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectiveDeliveryStatus {
    Pending,
    Broadcasted,
    Included,
    PendingCoreDelivery,
    AwaitingPortfolioSnapshot,
    ManualReview,
    CoreRejected,
    Finalized,
    Reverted,
    Expired,
    DeadLettered,
}

/// Delivery status returned from `/v1/directives/{directive_id}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectiveStatusResponse {
    pub directive_id: String,
    pub action_key: String,
    pub domain_status: DirectiveDomainStatus,
    pub delivery_status: DirectiveDeliveryStatus,
    pub tx_hash: Option<String>,
    pub created_at: Option<String>,
}

#[cfg(test)]
#[path = "directives_test.rs"]
mod tests;
