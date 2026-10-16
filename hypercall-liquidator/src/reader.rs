use hypercall_client::PortfolioResponse;
use thiserror::Error;

use crate::types::MarginSnapshot;

#[derive(Debug, Error, PartialEq, Eq)]
#[displaystr::display]
pub enum ReaderError {
    MissingMarginSummary { wallet: String } =
        "portfolio response for {wallet} is missing margin_summary",
    UnavailableMaintenanceRequirement { wallet: String } =
        "portfolio response for {wallet} has unavailable maintenance margin basis",
}

/// Extract standard margin facts from a public Hypercall portfolio response.
///
/// This function intentionally fails if `margin_summary` is missing. A
/// liquidator must not synthesize equity or maintenance margin from partial
/// portfolio fields.
pub fn margin_snapshot_from_portfolio(
    portfolio: &PortfolioResponse,
) -> Result<MarginSnapshot, ReaderError> {
    let summary =
        portfolio
            .margin_summary
            .as_ref()
            .ok_or_else(|| ReaderError::MissingMarginSummary {
                wallet: portfolio.wallet_address.to_string(),
            })?;

    Ok(MarginSnapshot {
        mode: summary.mode.clone(),
        equity: summary.equity,
        initial_margin_required: summary.position_im + summary.open_orders_im,
        maintenance_margin_required: summary.risk_maintenance_margin_required().ok_or_else(
            || ReaderError::UnavailableMaintenanceRequirement {
                wallet: portfolio.wallet_address.to_string(),
            },
        )?,
        maintenance_margin: summary.maintenance_margin_headroom().ok_or_else(|| {
            ReaderError::UnavailableMaintenanceRequirement {
                wallet: portfolio.wallet_address.to_string(),
            }
        })?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypercall_client::MarginSummary;
    use hypercall_sdk_types::{StandardMarginBreakdown, WalletAddress};
    use rust_decimal_macros::dec;

    fn wallet() -> WalletAddress {
        "0x0000000000000000000000000000000000000001"
            .parse()
            .unwrap()
    }

    #[test]
    fn extracts_margin_summary_without_recomputing_values() {
        let portfolio = PortfolioResponse {
            wallet_address: wallet(),
            positions: Vec::new(),
            total_margin_used: dec!(300),
            available_balance: dec!(1000),
            withdrawable_usdc: None,
            portfolio_snapshot_timestamp_ms: None,
            margin_mode: "standard".to_string(),
            margin_summary: Some(MarginSummary {
                mode: "standard".to_string(),
                equity: dec!(950),
                position_im: dec!(700),
                open_orders_im: dec!(50),
                initial_margin: dec!(200),
                maintenance_margin: dec!(-75),
                open_orders_premium_reserved: None,
                standard_margin: Some(StandardMarginBreakdown {
                    cash_margin: dec!(1000),
                    initial_margin_headroom: dec!(200),
                    maintenance_margin_headroom: dec!(-25),
                    oracle_contingency_im: dec!(0),
                    quote_depeg_im: dec!(0),
                    option_groups: Vec::new(),
                }),
            }),
        };

        let snapshot = margin_snapshot_from_portfolio(&portfolio).unwrap();
        assert_eq!(snapshot.mode, "standard");
        assert_eq!(snapshot.equity, dec!(950));
        assert_eq!(snapshot.initial_margin_required, dec!(750));
        assert_eq!(snapshot.maintenance_margin_required, dec!(1025));
        assert_eq!(snapshot.maintenance_excess(), dec!(-25));
    }

    #[test]
    fn fails_if_margin_summary_is_missing() {
        let portfolio = PortfolioResponse {
            wallet_address: wallet(),
            positions: Vec::new(),
            total_margin_used: dec!(0),
            available_balance: dec!(0),
            withdrawable_usdc: None,
            portfolio_snapshot_timestamp_ms: None,
            margin_mode: "standard".to_string(),
            margin_summary: None,
        };

        let error = margin_snapshot_from_portfolio(&portfolio).unwrap_err();
        assert_eq!(
            error,
            ReaderError::MissingMarginSummary {
                wallet: "0x0000000000000000000000000000000000000001".to_string()
            }
        );
    }
    #[test]
    fn missing_standard_cash_context_fails_closed() {
        let portfolio = PortfolioResponse {
            wallet_address: wallet(),
            positions: Vec::new(),
            total_margin_used: dec!(0),
            available_balance: dec!(0),
            withdrawable_usdc: None,
            portfolio_snapshot_timestamp_ms: None,
            margin_mode: "standard".to_owned(),
            margin_summary: Some(MarginSummary {
                mode: "standard".to_owned(),
                equity: dec!(1000),
                position_im: dec!(100),
                open_orders_im: dec!(0),
                initial_margin: dec!(900),
                maintenance_margin: dec!(900),
                open_orders_premium_reserved: None,
                standard_margin: None,
            }),
        };
        assert!(matches!(
            margin_snapshot_from_portfolio(&portfolio),
            Err(ReaderError::UnavailableMaintenanceRequirement { .. })
        ));
    }
}
