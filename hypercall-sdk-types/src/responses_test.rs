use super::*;
use rust_decimal_macros::dec;

#[test]
fn withdrawal_responses_decode_exact_status_and_balance_shapes() {
    let standard: StandardUsdcWithdrawalResponse = serde_json::from_value(serde_json::json!({
        "success": true,
        "request_id": "request-1",
        "directive_id": "directive-1",
        "domain_status": "accepted",
        "delivery_status": "pending",
        "balance_after": "89.77",
        "message": "USDC withdrawal accepted"
    }))
    .unwrap();
    assert_eq!(standard.balance_after, dec!(89.77));
    assert_eq!(
        standard.domain_status,
        crate::UsdcWithdrawalDomainStatus::Accepted
    );

    let portfolio: PortfolioUsdcWithdrawalResponse = serde_json::from_value(serde_json::json!({
        "success": true,
        "request_id": "request-2",
        "directive_id": "request-2",
        "domain_status": "pending",
        "delivery_status": "pending",
        "message": "Portfolio-margin USDC withdrawal accepted"
    }))
    .unwrap();
    assert_eq!(
        portfolio.domain_status,
        crate::UsdcWithdrawalDomainStatus::Pending
    );
}

#[test]
fn portfolio_withdrawal_response_decodes_exact_retry_statuses() {
    let response: crate::PortfolioUsdcWithdrawalResponse =
        serde_json::from_value(serde_json::json!({
            "success": true,
            "request_id": "retry-request",
            "directive_id": "retry-request",
            "domain_status": "confirmed",
            "delivery_status": "submitted",
            "message": "Portfolio-margin USDC withdrawal already accepted"
        }))
        .unwrap();

    assert_eq!(
        response.domain_status,
        crate::UsdcWithdrawalDomainStatus::Confirmed
    );
    assert_eq!(
        response.delivery_status,
        crate::DirectiveDeliveryStatus::Submitted
    );
}

fn margin_summary(mode: &str) -> MarginSummary {
    MarginSummary {
        mode: mode.to_owned(),
        equity: dec!(5000),
        position_im: dec!(250),
        open_orders_im: dec!(10),
        initial_margin: dec!(4740),
        maintenance_margin: dec!(4800),
        open_orders_premium_reserved: None,
        standard_margin: Some(StandardMarginBreakdown {
            cash_margin: dec!(1000),
            initial_margin_headroom: dec!(740),
            maintenance_margin_headroom: dec!(800),
            oracle_contingency_im: dec!(0),
            quote_depeg_im: dec!(0),
            option_groups: Vec::new(),
        }),
    }
}

#[test]
fn standard_margin_helpers_use_cash_and_never_long_mark_credit() {
    let summary = margin_summary("standard");
    assert_eq!(summary.risk_maintenance_margin_required(), Some(dec!(200)));
    assert_eq!(summary.risk_margin_utilization(), Some(dec!(0.2)));
    let missing = MarginSummary {
        standard_margin: None,
        ..summary
    };
    assert_eq!(missing.risk_maintenance_margin_required(), None);
    assert_eq!(missing.risk_margin_utilization(), None);
}

#[test]
fn portfolio_helpers_continue_to_use_marked_equity() {
    let summary = MarginSummary {
        standard_margin: None,
        ..margin_summary("portfolio")
    };
    assert_eq!(summary.maintenance_margin_required(), dec!(200));
    assert_eq!(summary.margin_utilization(), dec!(0.04));
}

#[test]
fn standard_breakdown_round_trips_strict_decimal_wire_values() {
    let mut summary = margin_summary("standard");
    summary
        .standard_margin
        .as_mut()
        .unwrap()
        .option_groups
        .push(StandardOptionExpiryMargin {
            underlying: "ETH".to_owned(),
            expiry_ts: 1800000000,
            symbols: vec!["call-1".to_owned(), "call-2".to_owned()],
            isolated_im: dec!(325),
            isolated_mm: dec!(212.5),
            payoff_cash_requirement: dec!(100),
            bounded_max_loss: Some(dec!(100)),
            unpaired_call_im: dec!(0),
            unpaired_call_mm: dec!(0),
            initial_margin: dec!(100),
            maintenance_margin: dec!(100),
        });
    let wire = serde_json::to_value(&summary).unwrap();
    assert_eq!(wire["standard_margin"]["cash_margin"], "1000");
    assert_eq!(
        wire["standard_margin"]["option_groups"][0]["bounded_max_loss"],
        "100"
    );
    let decoded: MarginSummary = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(
        decoded.standard_margin.unwrap().option_groups[0].initial_margin,
        dec!(100)
    );
    let mut numeric = wire;
    numeric["standard_margin"]["cash_margin"] = serde_json::json!(1000);
    assert!(serde_json::from_value::<MarginSummary>(numeric).is_err());
}

#[test]
fn existing_nav_accessors_keep_their_values_and_source_signatures() {
    let summary = margin_summary("standard");
    let required: Decimal = summary.maintenance_margin_required();
    let utilization: Decimal = summary.margin_utilization();
    assert_eq!(required, dec!(200));
    assert_eq!(utilization, dec!(0.04));
    assert_eq!(summary.initial_margin_headroom(), Some(dec!(740)));
    assert_eq!(summary.maintenance_margin_headroom(), Some(dec!(800)));
    // Old responses deserialize, but cannot provide authoritative cash risk.
    let mut old_wire = serde_json::to_value(&summary).unwrap();
    old_wire.as_object_mut().unwrap().remove("standard_margin");
    let old: MarginSummary = serde_json::from_value(old_wire).unwrap();
    assert_eq!(old.maintenance_margin_required(), required);
    assert_eq!(old.initial_margin_headroom(), None);
    assert_eq!(old.maintenance_margin_headroom(), None);
}

#[test]
fn empty_portfolio_account_headroom_fields_are_not_requirements() {
    // Prod 2026-10-06: an empty PM account with 40 equity returned
    // initial_margin = maintenance_margin = 40. Those are headroom; the
    // requirement is zero and must be derived, never read from these fields.
    let summary = MarginSummary {
        mode: "portfolio".to_owned(),
        equity: dec!(40),
        position_im: dec!(0),
        open_orders_im: dec!(0),
        initial_margin: dec!(40),
        maintenance_margin: dec!(40),
        open_orders_premium_reserved: None,
        standard_margin: None,
    };
    assert_eq!(summary.initial_margin_headroom(), Some(dec!(40)));
    assert_eq!(summary.maintenance_margin_headroom(), Some(dec!(40)));
    assert_eq!(summary.risk_maintenance_margin_required(), Some(dec!(0)));
    assert_eq!(summary.risk_margin_utilization(), Some(dec!(0)));
    assert_eq!(summary.position_im + summary.open_orders_im, dec!(0));
}
