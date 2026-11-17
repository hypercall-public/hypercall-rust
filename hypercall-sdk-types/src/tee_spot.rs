//! Wire types for TEE user spot orders (`/v1/tee-spot/*`).
//!
//! Signed submissions reuse [`DirectiveSubmitRequest`] (`account`, `nonce`,
//! `action`, `signature`). Prices and sizes are 1e8-scaled integers, as on
//! the TEE perp routes. The account's API wallet signs `HCSpotOrder` or
//! `HCSpotCancel` in the `HypercallApiSign` domain.
//!
//! [`DirectiveSubmitRequest`]: crate::DirectiveSubmitRequest

use serde::{Deserialize, Serialize};

use crate::directives::{
    deserialize_perp_tif, deserialize_u128, deserialize_u64, serialize_perp_tif,
    serialize_safe_u128, serialize_safe_u64,
};
use crate::tee_perp::{TeePerpErrorCode, TeePerpOrderStage};
use crate::{PerpTimeInForce, WalletAddress};

/// `POST /v1/tee-spot/orders/submit`: one spot limit order (GTC or IOC).
pub const TEE_SPOT_ORDER_SUBMIT_ROUTE: &str = "/v1/tee-spot/orders/submit";
/// `POST /v1/tee-spot/cancels/submit`: cancel an open spot order by cloid.
pub const TEE_SPOT_CANCEL_SUBMIT_ROUTE: &str = "/v1/tee-spot/cancels/submit";

/// HyperCore spot asset ids are `10000 + spot pair index`.
pub const TEE_SPOT_ASSET_OFFSET: u32 = 10_000;

/// The signed spot order (`SpotOrder` in `HCSpotOrder`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeeSpotOrderAction {
    /// `10000 + spot pair index`. Only USDC-quoted pairs trade.
    pub asset: u32,
    pub is_buy: bool,
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub limit_px: u64,
    /// Base token units, 1e8-scaled.
    #[serde(
        serialize_with = "serialize_safe_u64",
        deserialize_with = "deserialize_u64"
    )]
    pub sz: u64,
    /// GTC or IOC. ALO is refused.
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

/// The signed spot cancel (`SpotCancel` in `HCSpotCancel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeeSpotCancelAction {
    pub asset: u32,
    #[serde(
        serialize_with = "serialize_safe_u128",
        deserialize_with = "deserialize_u128"
    )]
    pub cloid: u128,
}

/// Response of [`TEE_SPOT_ORDER_SUBMIT_ROUTE`] and
/// [`TEE_SPOT_CANCEL_SUBMIT_ROUTE`].
///
/// `accepted` means HyperCore took the action. Whether an order rested,
/// filled or was canceled is read from HyperCore (`orderStatus`); Hypercall
/// journals the same facts and releases the order's reservation once a
/// portfolio snapshot reflects them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeSpotResponse {
    pub stage: TeePerpOrderStage,
    #[serde(default)]
    pub error_code: Option<TeePerpErrorCode>,
    pub request_id: String,
    pub account: WalletAddress,
    pub api_wallet: WalletAddress,
    pub cloid: String,
    #[serde(
        default,
        serialize_with = "crate::tee_perp::serialize_optional_safe_u64",
        deserialize_with = "crate::tee_perp::deserialize_optional_u64"
    )]
    pub engine_seq: Option<u64>,
    #[serde(default)]
    pub venue_response_hash: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

impl TeeSpotResponse {
    /// Whether HyperCore accepted the action.
    pub fn is_accepted(&self) -> bool {
        self.stage == TeePerpOrderStage::Accepted
    }
}

#[cfg(test)]
#[path = "tee_spot_test.rs"]
mod tests;
