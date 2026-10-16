use hypersdk::hypercore::types::Action;
use rust_decimal::Decimal;
use std::str::FromStr;

use super::*;
use crate::{HyperliquidChain, PreparedPerpLimitOrder};

fn d(value: &str) -> Decimal {
    Decimal::from_str(value).unwrap()
}

const NONCE: u64 = 1_800_000_000_001;
const EXPIRES_AFTER_MS: u64 = 1_800_000_005_000;

fn ubtc_sell() -> PreparedHyperliquidAction {
    PreparedHyperliquidAction::prepare_spot_limit_order(
        HyperliquidChain::Mainnet,
        PreparedSpotLimitOrder::ioc_sell(142, d("94000"), d("0.01234"), 7),
        5,
        NONCE,
        EXPIRES_AFTER_MS,
    )
    .unwrap()
}

#[test]
fn spot_order_uses_the_native_spot_asset_id_and_round_trips() {
    let prepared = ubtc_sell();
    let bytes = prepared.canonical_bytes().unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    // The native wire is HyperLiquid's ordinary order with a = 10000 + pair.
    assert!(text.contains("\"a\":10142"), "{text}");
    assert!(text.contains("\"b\":false"), "{text}");
    assert!(text.contains("\"r\":false"), "{text}");
    assert!(text.contains("\"tif\":\"Ioc\""), "{text}");
    assert!(!text.contains("\"builder\""), "{text}");

    let decoded = PreparedHyperliquidAction::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(
        decoded.spot_limit_order().unwrap(),
        PreparedSpotLimitOrder::ioc_sell(142, d("94000"), d("0.01234"), 7)
    );
    // A spot order is not a perp order.
    assert!(decoded.limit_order().is_err());
}

#[test]
fn a_perp_order_is_not_a_spot_order_and_cannot_name_a_spot_asset() {
    let perp = PreparedHyperliquidAction::prepare_limit_order(
        HyperliquidChain::Mainnet,
        PreparedPerpLimitOrder::new(0, false, d("94000"), d("0.1"), Tif::Ioc, 7),
        NONCE,
        None,
        EXPIRES_AFTER_MS,
    )
    .unwrap();
    assert!(perp.spot_limit_order().is_err());
    // A HIP-3 builder perp (100000 and up) is still a perp.
    let builder_perp = PreparedHyperliquidAction::prepare_limit_order(
        HyperliquidChain::Mainnet,
        PreparedPerpLimitOrder::new(110_008, false, d("500"), d("1"), Tif::Ioc, 7),
        NONCE,
        None,
        EXPIRES_AFTER_MS,
    )
    .unwrap();
    assert!(builder_perp.spot_limit_order().is_err());
    assert!(builder_perp.limit_order().is_ok());

    let spot_as_perp = PreparedHyperliquidAction::prepare_limit_order(
        HyperliquidChain::Mainnet,
        PreparedPerpLimitOrder::new(10_142, false, d("94000"), d("0.01"), Tif::Ioc, 7),
        NONCE,
        None,
        EXPIRES_AFTER_MS,
    );
    assert!(spot_as_perp.is_err());
}

#[test]
fn spot_orders_are_never_reduce_only() {
    let mut prepared = ubtc_sell();
    if let Action::Order(batch) = &mut prepared.action {
        batch.orders[0].reduce_only = true;
    }
    // The digest no longer matches, and even a consistent digest is refused.
    assert!(prepared.spot_limit_order().is_err());
}

#[test]
fn spot_price_allows_five_significant_figures_and_size_decimals() {
    // UBTC: szDecimals 5, so at most 3 price decimals.
    for valid in [d("94000"), d("94123"), d("123456"), d("9412.3")] {
        validate_spot_price(valid, 5).unwrap();
    }
    for invalid in [d("94123.5"), d("9412.34"), d("0"), d("-1")] {
        assert!(validate_spot_price(invalid, 5).is_err(), "{invalid}");
    }
    // HYPE: szDecimals 2, so at most 6 price decimals and 5 significant figures.
    validate_spot_price(d("41.234"), 2).unwrap();
    assert!(validate_spot_price(d("41.2345"), 2).is_err());
    // A token with szDecimals 0 accepts 8 decimals within five figures.
    validate_spot_price(d("0.00012345"), 0).unwrap();
    assert!(validate_spot_price(d("0.000123456"), 0).is_err());

    validate_spot_size(d("0.01234"), 5).unwrap();
    assert!(validate_spot_size(d("0.012345"), 5).is_err());
    assert!(validate_spot_size(Decimal::ZERO, 5).is_err());
    assert!(validate_spot_size(d("1"), 9).is_err());
}

#[test]
fn quantization_rounds_down_to_a_valid_spot_order() {
    assert_eq!(
        quantize_spot_price_down(d("94123.789"), 5).unwrap(),
        d("94123")
    );
    assert_eq!(
        quantize_spot_price_down(d("9412.3789"), 5).unwrap(),
        d("9412.3")
    );
    assert_eq!(
        quantize_spot_price_down(d("3.987654"), 4).unwrap(),
        d("3.9876")
    );
    assert_eq!(
        quantize_spot_price_down(d("41.23456"), 2).unwrap(),
        d("41.234")
    );
    assert_eq!(quantize_spot_price_down(d("-5"), 2).unwrap(), Decimal::ZERO);
    assert_eq!(
        quantize_spot_size_down(d("0.0123456"), 5).unwrap(),
        d("0.01234")
    );
    for (price, sz) in [(d("94123.789"), 5), (d("3.987654"), 4), (d("41.23456"), 2)] {
        validate_spot_price(quantize_spot_price_down(price, sz).unwrap(), sz).unwrap();
    }

    let unquantized = PreparedHyperliquidAction::prepare_spot_limit_order(
        HyperliquidChain::Mainnet,
        PreparedSpotLimitOrder::ioc_sell(142, d("94000.5"), d("0.01"), 7),
        5,
        NONCE,
        EXPIRES_AFTER_MS,
    );
    assert!(unquantized.is_err());
}

#[test]
fn spot_pair_index_must_stay_in_the_spot_range() {
    let order = PreparedSpotLimitOrder::ioc_sell(90_000, d("1"), d("1"), 7);
    assert!(order.asset().is_err());
    assert!(is_spot_asset(10_000));
    assert!(is_spot_asset(99_999));
    assert!(!is_spot_asset(9_999));
    assert!(!is_spot_asset(100_000));
}
