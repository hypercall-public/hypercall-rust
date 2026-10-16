use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Engine-priced standard-margin full-liquidation terms.
///
/// These fields must be read as one Hypercall status snapshot and submitted
/// together. Mixing fields across status reads can produce an invalid auction
/// terms hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StandardMarginLiquidationBidTerms {
    pub auction_id: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub bid_usdc: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub equity: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub mm_required: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub maintenance_margin: Decimal,
    pub positions: Vec<hypercall_sdk_types::StandardMarginLiquidationPositionRequest>,
    pub portfolio_hash: String,
    pub auction_terms_hash: String,
    pub auction_version: u64,
    pub valuation_timestamp_ms: u64,
}

/// Active standard-margin liquidation terms shown to the planner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuctionStatus {
    pub account: String,
    pub start_time: u64,
    #[serde(with = "rust_decimal::serde::str")]
    pub current_bid_usdc: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub current_cost_usdc: Decimal,
}

impl AuctionStatus {
    pub fn is_active(&self) -> bool {
        self.start_time > 0
    }
}

/// Standard margin facts for one account.
///
/// These values must come from Hypercall state. External hedge balances or PnL
/// must not be included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarginSnapshot {
    pub mode: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub equity: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub initial_margin_required: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub maintenance_margin_required: Decimal,
    /// Authoritative maintenance headroom; SM cannot derive this from NAV.
    #[serde(with = "rust_decimal::serde::str")]
    pub maintenance_margin: Decimal,
}

impl MarginSnapshot {
    pub fn is_standard_margin(&self) -> bool {
        self.mode.eq_ignore_ascii_case("standard")
    }

    pub fn maintenance_excess(&self) -> Decimal {
        self.maintenance_margin
    }
}

/// Option delta exposure for a single position or aggregated symbol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionDelta {
    pub symbol: String,
    pub underlying: String,
    pub delta: f64,
}

/// A candidate account assembled from public Hypercall reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiquidationCandidate {
    pub account: String,
    pub margin: MarginSnapshot,
    #[serde(with = "rust_decimal::serde::str")]
    pub current_bid_usdc: Decimal,
    #[serde(default)]
    pub positions: Vec<PositionDelta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard_margin_terms: Option<StandardMarginLiquidationBidTerms>,
}
