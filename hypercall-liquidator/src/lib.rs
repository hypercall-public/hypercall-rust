//! Public standard margin liquidator proof-of-concept.
//!
//! This crate intentionally keeps Hypercall liquidation eligibility separate from
//! external Hyperliquid hedging. Hypercall standard margin facts decide whether
//! an account can be liquidated. Hyperliquid orders only manage the operator's
//! risk after a liquidation is chosen.

pub mod auction;
pub mod config;
pub mod cycle;
pub mod hedge;
pub mod keys;
pub mod planner;
pub mod reader;
pub mod types;
pub mod ui;

pub use auction::{
    auction_terms_hash, verify_published_terms_hash, AuctionBidRule, AuctionTermsError,
    SUPPORTED_AUCTION_VERSIONS,
};
pub use config::{ConfigError, HypercallLiquidatorConfig, KeyConfig, KeyKind};
pub use cycle::{
    discover_standard_margin_liquidation_cycles, execute_hedge_after_liquidation,
    execute_standard_margin_liquidation_cycle, plan_liquidation_cycle,
    read_standard_margin_liquidation_cycle, CycleDecision, CycleError, CycleSkipReason,
    HedgeExecution, LiquidationExecution, PostLiquidationHedgeSkip,
    StandardMarginLiquidationReceipt,
};
pub use hedge::{
    plan_delta_hedge, submit_hedge_plan, DeltaHedgeInput, HedgePlan, HedgePlanDecision,
};
pub use keys::{hypercall_wallet_from_key_config, plaintext_private_key_from_env, KeyError};
pub use planner::{evaluate_candidate, CandidateDecision, CandidateEvaluationError};
pub use reader::{margin_snapshot_from_portfolio, ReaderError};
pub use types::{AuctionStatus, LiquidationCandidate, MarginSnapshot, PositionDelta};
#[cfg(feature = "ui")]
pub use ui::LiquidatorDashboard;
pub use ui::{
    CollateralPrompt, DashboardSnapshot, HedgePanel, KillSwitch, LiquidationPanel, MarginPanel,
};
