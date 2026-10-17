//! Spot limit orders on the prepared Hyperliquid action path.
//!
//! A spot order is the same native `order` action as a perp order; only the
//! asset id differs (`10000 + spot pair index`). This module keeps the two
//! apart: a prepared perp order never carries a spot asset, a prepared spot
//! order never carries a perp asset, and a spot order is quantized to the
//! venue's spot rules before it is prepared.
//!
//! HyperCore spot rules (from the venue's tick and lot size documentation):
//!
//! - **Size:** a multiple of `10^-szDecimals` of the base token.
//! - **Price:** at most five significant figures and at most
//!   `8 - szDecimals` decimals. An integer price is always valid, whatever
//!   its number of significant figures.
//! - **Reduce-only:** HyperCore rejects reduce-only on spot ("Reduce-only is
//!   invalid for spot trading."), so a prepared spot order is never
//!   reduce-only.

use hypersdk::hypercore::{
    types::{
        Action, BatchOrder, OrderGrouping, OrderRequest, OrderTypePlacement,
        TimeInForce as HyperliquidTimeInForce,
    },
    Chain,
};
use rust_decimal::{Decimal, RoundingStrategy};

use crate::prepared::{
    action_digest, cloid_from_u128, PreparedHyperliquidAction, PreparedHyperliquidActionError,
    PREPARED_HYPERLIQUID_ACTION_VERSION,
};
use crate::Tif;

#[cfg(test)]
#[path = "prepared_spot_test.rs"]
mod tests;

type Result<T> = std::result::Result<T, PreparedHyperliquidActionError>;
use PreparedHyperliquidActionError as ClientError;

/// HyperCore spot asset ids are `10000 + spot pair index`.
pub const HYPERLIQUID_SPOT_ASSET_OFFSET: u32 = 10_000;
/// First asset id past the spot range. HIP-3 builder-deployed perps start
/// here (`100000 + dex * 10000 + index`).
pub const HYPERLIQUID_SPOT_ASSET_END: u32 = 100_000;
/// Spot prices carry at most this many decimals minus the token's
/// `szDecimals`.
pub const HYPERLIQUID_SPOT_MAX_PRICE_DECIMALS: u32 = 8;
/// Non-integer prices carry at most this many significant figures.
pub const HYPERLIQUID_MAX_PRICE_SIGNIFICANT_FIGURES: u32 = 5;

/// Whether `asset` is a HyperCore spot asset id.
pub const fn is_spot_asset(asset: u32) -> bool {
    asset >= HYPERLIQUID_SPOT_ASSET_OFFSET && asset < HYPERLIQUID_SPOT_ASSET_END
}

/// Economic fields of one spot limit order. The caller quantizes price and
/// size (see [`quantize_spot_price_down`] and [`quantize_spot_size_down`]);
/// preparation rejects a value the venue would not accept rather than
/// rounding it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedSpotLimitOrder {
    /// Index of the spot pair in HyperCore `spotMeta.universe`.
    pub pair_index: u32,
    pub is_buy: bool,
    pub limit_px: Decimal,
    /// Base token units.
    pub size: Decimal,
    pub tif: Tif,
    pub cloid: u128,
}

impl PreparedSpotLimitOrder {
    /// An immediate-or-cancel sell, the only spot order Hypercall prepares
    /// today (PM spot-sale recovery).
    pub fn ioc_sell(pair_index: u32, limit_px: Decimal, size: Decimal, cloid: u128) -> Self {
        Self {
            pair_index,
            is_buy: false,
            limit_px,
            size,
            tif: Tif::Ioc,
            cloid,
        }
    }

    /// The native asset id, `10000 + pair_index`, within the spot range.
    pub fn asset(&self) -> Result<u32> {
        HYPERLIQUID_SPOT_ASSET_OFFSET
            .checked_add(self.pair_index)
            .filter(|asset| is_spot_asset(*asset))
            .ok_or_else(|| {
                ClientError::InvalidInput(format!(
                    "spot pair index {} is outside the HyperCore spot asset range",
                    self.pair_index
                ))
            })
    }
}

/// Reject a spot price the venue would not accept for a token with
/// `sz_decimals`.
pub fn validate_spot_price(price: Decimal, sz_decimals: u32) -> Result<()> {
    validate_sz_decimals(sz_decimals)?;
    if price <= Decimal::ZERO {
        return Err(ClientError::InvalidInput(
            "spot price must be positive".to_string(),
        ));
    }
    let price = price.normalize();
    if price.fract().is_zero() {
        return Ok(());
    }
    let max_decimals = HYPERLIQUID_SPOT_MAX_PRICE_DECIMALS - sz_decimals;
    if price.scale() > max_decimals {
        return Err(ClientError::InvalidInput(format!(
            "spot price {price} has more than {max_decimals} decimals"
        )));
    }
    let significant_figures = price.mantissa().unsigned_abs().to_string().len();
    if significant_figures > HYPERLIQUID_MAX_PRICE_SIGNIFICANT_FIGURES as usize {
        return Err(ClientError::InvalidInput(format!(
            "spot price {price} has more than {HYPERLIQUID_MAX_PRICE_SIGNIFICANT_FIGURES} significant figures"
        )));
    }
    Ok(())
}

/// Reject a spot size the venue would not accept for a token with
/// `sz_decimals`.
pub fn validate_spot_size(size: Decimal, sz_decimals: u32) -> Result<()> {
    validate_sz_decimals(sz_decimals)?;
    if size <= Decimal::ZERO {
        return Err(ClientError::InvalidInput(
            "spot size must be positive".to_string(),
        ));
    }
    if size.normalize().scale() > sz_decimals {
        return Err(ClientError::InvalidInput(format!(
            "spot size {size} has more than {sz_decimals} decimals"
        )));
    }
    Ok(())
}

/// Round a price down to the nearest valid spot price for a token with
/// `sz_decimals`. Rounding a sell's limit down never raises it above the
/// caller's intended price. Non-positive prices round to zero.
pub fn quantize_spot_price_down(price: Decimal, sz_decimals: u32) -> Result<Decimal> {
    validate_sz_decimals(sz_decimals)?;
    if price <= Decimal::ZERO {
        return Ok(Decimal::ZERO);
    }
    let max_decimals = HYPERLIQUID_SPOT_MAX_PRICE_DECIMALS - sz_decimals;
    // The power of ten of the price's leading digit.
    let mut magnitude: i64 = 0;
    let mut scaled = price;
    while scaled >= Decimal::TEN {
        scaled /= Decimal::TEN;
        magnitude += 1;
    }
    while scaled < Decimal::ONE {
        scaled *= Decimal::TEN;
        magnitude -= 1;
    }
    let significant_decimals =
        (i64::from(HYPERLIQUID_MAX_PRICE_SIGNIFICANT_FIGURES) - 1 - magnitude).max(0);
    let decimals = u32::try_from(significant_decimals)
        .expect("a non-negative decimal count fits u32")
        .min(max_decimals);
    Ok(price
        .round_dp_with_strategy(decimals, RoundingStrategy::ToNegativeInfinity)
        .normalize())
}

/// Round a size down to the token's size increment.
pub fn quantize_spot_size_down(size: Decimal, sz_decimals: u32) -> Result<Decimal> {
    validate_sz_decimals(sz_decimals)?;
    Ok(size
        .round_dp_with_strategy(sz_decimals, RoundingStrategy::ToNegativeInfinity)
        .normalize())
}

fn validate_sz_decimals(sz_decimals: u32) -> Result<()> {
    if sz_decimals > HYPERLIQUID_SPOT_MAX_PRICE_DECIMALS {
        return Err(ClientError::InvalidInput(format!(
            "spot szDecimals {sz_decimals} exceeds {HYPERLIQUID_SPOT_MAX_PRICE_DECIMALS}"
        )));
    }
    Ok(())
}

impl PreparedHyperliquidAction {
    /// Prepare one exact spot limit order for a token with `sz_decimals`.
    ///
    /// The native action is the venue's ordinary `order` action with
    /// `a = 10000 + pair index`, never reduce-only, with no builder and no
    /// vault. Price and size must already be valid for the token.
    pub fn prepare_spot_limit_order(
        chain: Chain,
        order: PreparedSpotLimitOrder,
        sz_decimals: u32,
        nonce: u64,
        expires_after_ms: u64,
    ) -> Result<Self> {
        let asset = order.asset()?;
        validate_spot_price(order.limit_px, sz_decimals)?;
        validate_spot_size(order.size, sz_decimals)?;
        let tif = match order.tif {
            Tif::Gtc => HyperliquidTimeInForce::Gtc,
            Tif::Ioc => HyperliquidTimeInForce::Ioc,
            Tif::Alo => {
                return Err(ClientError::InvalidInput(
                    "ALO is not supported for prepared spot orders".to_string(),
                ));
            }
        };
        let asset = usize::try_from(asset).map_err(|_| {
            ClientError::InvalidInput("Hyperliquid asset index does not fit usize".to_string())
        })?;
        let action = Action::Order(BatchOrder {
            orders: vec![OrderRequest {
                asset,
                is_buy: order.is_buy,
                limit_px: order.limit_px.normalize(),
                sz: order.size.normalize(),
                reduce_only: false,
                order_type: OrderTypePlacement::Limit { tif },
                cloid: cloid_from_u128(order.cloid),
            }],
            grouping: OrderGrouping::Na,
            builder: None,
        });
        let action_digest = action_digest(&action, chain, nonce, None, expires_after_ms)?;
        let prepared = Self {
            version: PREPARED_HYPERLIQUID_ACTION_VERSION,
            chain,
            action,
            twap: None,
            nonce,
            vault_address: None,
            expires_after_ms,
            action_digest,
        };
        prepared.validate()?;
        Ok(prepared)
    }

    /// Return the exact spot order encoded by the validated native action.
    /// A perp asset, a builder or a reduce-only flag is not a spot order.
    pub fn spot_limit_order(&self) -> Result<PreparedSpotLimitOrder> {
        self.validate()?;
        let Action::Order(batch) = &self.action else {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action is not a spot limit order".to_string(),
            ));
        };
        let order = &batch.orders[0];
        let asset = u32::try_from(order.asset)
            .ok()
            .filter(|asset| is_spot_asset(*asset))
            .ok_or_else(|| {
                ClientError::InvalidInput(
                    "prepared Hyperliquid order is not for a spot asset".to_string(),
                )
            })?;
        if order.reduce_only {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid spot order must not be reduce-only".to_string(),
            ));
        }
        if batch.builder.is_some() {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid spot order must not name a builder".to_string(),
            ));
        }
        let tif = match order.order_type {
            OrderTypePlacement::Limit {
                tif: HyperliquidTimeInForce::Gtc,
            } => Tif::Gtc,
            OrderTypePlacement::Limit {
                tif: HyperliquidTimeInForce::Ioc,
            } => Tif::Ioc,
            _ => {
                return Err(ClientError::InvalidInput(
                    "only GTC and IOC spot limit orders are supported".to_string(),
                ));
            }
        };
        let mut cloid = [0_u8; 16];
        cloid.copy_from_slice(order.cloid.as_slice());
        Ok(PreparedSpotLimitOrder {
            pair_index: asset - HYPERLIQUID_SPOT_ASSET_OFFSET,
            is_buy: order.is_buy,
            limit_px: order.limit_px,
            size: order.sz,
            tif,
            cloid: u128::from_be_bytes(cloid),
        })
    }
}
