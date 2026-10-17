//! Native Hyperliquid TWAP wire (`twapOrder` and `twapCancel`).
//!
//! `hypersdk` 0.2.13 has no TWAP action, so the wire is defined here. The
//! MessagePack key order (`type`, then `twap{a,b,s,r,m,t}` or `a`, `t`) is
//! part of the signed hash and must not change. Hyperliquid documents no
//! builder field for either action.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Shortest TWAP duration this crate prepares. Unverified against the live
/// venue, which rejects out-of-range durations with "Invalid TWAP duration".
pub const TWAP_MIN_MINUTES: u32 = 5;
/// Longest TWAP duration this crate prepares (24 hours).
pub const TWAP_MAX_MINUTES: u32 = 1440;

/// One Hyperliquid TWAP action in its exact signed and submitted shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum TwapAction {
    #[serde(rename = "twapOrder")]
    Order { twap: TwapOrderWire },
    #[serde(rename = "twapCancel")]
    Cancel(TwapCancelWire),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TwapOrderWire {
    #[serde(rename = "a")]
    pub asset: u32,
    #[serde(rename = "b")]
    pub is_buy: bool,
    #[serde(rename = "s", with = "decimal_normalized")]
    pub size: Decimal,
    #[serde(rename = "r")]
    pub reduce_only: bool,
    #[serde(rename = "m")]
    pub minutes: u32,
    #[serde(rename = "t")]
    pub randomize: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TwapCancelWire {
    #[serde(rename = "a")]
    pub asset: u32,
    #[serde(rename = "t")]
    pub twap_id: u64,
}

mod decimal_normalized {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Decimal, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.normalize().to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Decimal, D::Error> {
        let value = String::deserialize(deserializer)?;
        Decimal::from_str(&value)
            .map(|decimal| decimal.normalize())
            .map_err(serde::de::Error::custom)
    }
}

/// Exact economic fields of one TWAP order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPerpTwapOrder {
    pub asset: u32,
    pub is_buy: bool,
    pub size: Decimal,
    pub reduce_only: bool,
    pub minutes: u32,
    pub randomize: bool,
}

/// Exact identity of one running TWAP to cancel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPerpTwapCancel {
    pub asset: u32,
    pub twap_id: u64,
}

/// Which order a modify replaces. A cloid target must equal the replacement
/// cloid so the order keeps its client identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparedModifyTarget {
    Oid(u64),
    Cloid(u128),
}

/// Exact replacement terms for one resting limit order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPerpModify {
    pub target: PreparedModifyTarget,
    pub asset: u32,
    pub is_buy: bool,
    pub limit_px: Decimal,
    pub size: Decimal,
    pub reduce_only: bool,
    pub tif: crate::Tif,
    pub cloid: u128,
}

#[cfg(test)]
#[path = "prepared_twap_test.rs"]
mod tests;
