use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::LiquidationConfig;
use crate::types::LiquidationCandidate;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateDecision {
    Eligible {
        maintenance_excess: Decimal,
        required_buffered_shortfall: Decimal,
    },
    Skip(SkipReason),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkipReason {
    LiquidationDisabled,
    NotStandardMargin {
        mode: String,
    },
    NotBelowBufferedMaintenance {
        maintenance_excess: Decimal,
        required_buffered_shortfall: Decimal,
    },
    BidAboveCap {
        bid: Decimal,
        cap: Decimal,
    },
    ProfitabilityNotAssumed,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[displaystr::display]
pub enum CandidateEvaluationError {
    NonPositiveMaxBid = "liquidation max_bid_usdc must be positive",
}

pub fn evaluate_candidate(
    config: &LiquidationConfig,
    candidate: &LiquidationCandidate,
) -> Result<CandidateDecision, CandidateEvaluationError> {
    if config.max_bid_usdc <= Decimal::ZERO {
        return Err(CandidateEvaluationError::NonPositiveMaxBid);
    }
    if !config.enabled {
        return Ok(CandidateDecision::Skip(SkipReason::LiquidationDisabled));
    }
    if !candidate.margin.is_standard_margin() {
        return Ok(CandidateDecision::Skip(SkipReason::NotStandardMargin {
            mode: candidate.margin.mode.clone(),
        }));
    }
    if !config.assume_penalty_profitability {
        return Ok(CandidateDecision::Skip(SkipReason::ProfitabilityNotAssumed));
    }
    if candidate.current_bid_usdc > config.max_bid_usdc {
        return Ok(CandidateDecision::Skip(SkipReason::BidAboveCap {
            bid: candidate.current_bid_usdc,
            cap: config.max_bid_usdc,
        }));
    }

    let maintenance_excess = candidate.margin.maintenance_excess();
    let required_buffered_shortfall = buffered_shortfall(
        candidate.margin.maintenance_margin_required,
        config.min_maintenance_buffer_bps,
    );
    if maintenance_excess > -required_buffered_shortfall {
        return Ok(CandidateDecision::Skip(
            SkipReason::NotBelowBufferedMaintenance {
                maintenance_excess,
                required_buffered_shortfall,
            },
        ));
    }

    Ok(CandidateDecision::Eligible {
        maintenance_excess,
        required_buffered_shortfall,
    })
}

fn buffered_shortfall(maintenance_margin_required: Decimal, buffer_bps: u32) -> Decimal {
    maintenance_margin_required * Decimal::from(buffer_bps) / Decimal::from(10_000u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HypercallLiquidatorConfig;
    use crate::types::MarginSnapshot;
    use rust_decimal_macros::dec;

    fn config() -> LiquidationConfig {
        HypercallLiquidatorConfig::from_toml_str(include_str!(
            "../examples/liquidator.example.toml"
        ))
        .unwrap()
        .liquidation
    }

    fn candidate(equity: Decimal, mm_required: Decimal) -> LiquidationCandidate {
        LiquidationCandidate {
            account: "0x0000000000000000000000000000000000000001".to_string(),
            margin: MarginSnapshot {
                mode: "standard".to_string(),
                equity,
                initial_margin_required: dec!(1500),
                maintenance_margin_required: mm_required,
                maintenance_margin: equity - mm_required,
            },
            current_bid_usdc: dec!(1000),
            positions: Vec::new(),
            standard_margin_terms: None,
        }
    }

    #[test]
    fn eligible_only_after_buffered_mm_breach() {
        let cfg = config();

        let shallow = evaluate_candidate(&cfg, &candidate(dec!(995), dec!(1000))).unwrap();
        assert!(matches!(
            shallow,
            CandidateDecision::Skip(SkipReason::NotBelowBufferedMaintenance { .. })
        ));

        let deep = evaluate_candidate(&cfg, &candidate(dec!(940), dec!(1000))).unwrap();
        assert!(matches!(deep, CandidateDecision::Eligible { .. }));
    }

    #[test]
    fn skips_portfolio_margin_accounts() {
        let mut c = candidate(dec!(900), dec!(1000));
        c.margin.mode = "portfolio".to_string();
        let decision = evaluate_candidate(&config(), &c).unwrap();
        assert_eq!(
            decision,
            CandidateDecision::Skip(SkipReason::NotStandardMargin {
                mode: "portfolio".to_string()
            })
        );
    }

    #[test]
    fn skips_bid_above_cap() {
        let mut c = candidate(dec!(900), dec!(1000));
        c.current_bid_usdc = dec!(100000);
        let decision = evaluate_candidate(&config(), &c).unwrap();
        assert!(matches!(
            decision,
            CandidateDecision::Skip(SkipReason::BidAboveCap { .. })
        ));
    }
}
