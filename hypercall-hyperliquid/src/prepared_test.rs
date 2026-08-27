use alloy::{primitives::address, signers::local::PrivateKeySigner};
use hypersdk::hypercore::types::{Action, OrderTypePlacement, TimeInForce, TpSl};
use rust_decimal::Decimal;

use crate::{
    HyperliquidChain, HyperliquidSubmissionClassification, PreparedHyperliquidAction,
    PreparedPerpCancelByCloid, PreparedPerpLimitOrder, Tif, PREPARED_HYPERLIQUID_ACTION_VERSION,
};

const TEST_PRIVATE_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

fn prepared() -> PreparedHyperliquidAction {
    PreparedHyperliquidAction::prepare_limit_order(
        HyperliquidChain::Testnet,
        PreparedPerpLimitOrder::new(
            0,
            true,
            Decimal::from(70_000u32),
            Decimal::new(1, 1),
            Tif::Ioc,
            42,
        ),
        1_800_000_000_001,
        None,
        1_800_000_001_000,
    )
    .unwrap()
}

#[test]
fn prepared_action_roundtrips_only_exact_canonical_bytes() {
    let prepared = prepared();
    let bytes = prepared.canonical_bytes().unwrap();
    let decoded = PreparedHyperliquidAction::from_canonical_bytes(&bytes).unwrap();

    assert_eq!(decoded.version, PREPARED_HYPERLIQUID_ACTION_VERSION);
    assert_eq!(decoded.action_digest, prepared.action_digest);
    assert_eq!(
        decoded.request_hash().unwrap(),
        prepared.request_hash().unwrap()
    );
    assert_eq!(decoded.cloid().unwrap().as_slice(), 42_u128.to_be_bytes());

    let mut noncanonical = bytes;
    noncanonical.push(b' ');
    let error = PreparedHyperliquidAction::from_canonical_bytes(&noncanonical).unwrap_err();
    assert!(
        error.to_string().contains("not canonically encoded"),
        "{error}"
    );
}

#[test]
fn reduce_only_is_frozen_into_canonical_order_bytes() {
    let prepared = PreparedHyperliquidAction::prepare_limit_order(
        HyperliquidChain::Testnet,
        PreparedPerpLimitOrder::new(
            0,
            false,
            Decimal::from(70_000u32),
            Decimal::new(1, 1),
            Tif::Ioc,
            43,
        )
        .reduce_only(true),
        1_800_000_000_002,
        None,
        1_800_000_001_001,
    )
    .unwrap();

    let decoded =
        PreparedHyperliquidAction::from_canonical_bytes(&prepared.canonical_bytes().unwrap())
            .unwrap();
    assert!(decoded.limit_order().unwrap().reduce_only);
}

#[test]
fn cancel_by_cloid_roundtrips_exact_canonical_bytes() {
    let prepared = PreparedHyperliquidAction::prepare_cancel_by_cloid(
        HyperliquidChain::Testnet,
        PreparedPerpCancelByCloid {
            asset: 3,
            cloid: 44,
        },
        1_800_000_000_003,
        None,
        1_800_000_001_002,
    )
    .unwrap();
    let bytes = prepared.canonical_bytes().unwrap();
    let decoded = PreparedHyperliquidAction::from_canonical_bytes(&bytes).unwrap();

    assert_eq!(
        decoded.cancel_by_cloid().unwrap(),
        PreparedPerpCancelByCloid {
            asset: 3,
            cloid: 44,
        }
    );
    assert_eq!(decoded.cloid().unwrap().as_slice(), 44_u128.to_be_bytes());
}

#[test]
fn cancel_by_cloid_rejects_zero_cloid() {
    let error = PreparedHyperliquidAction::prepare_cancel_by_cloid(
        HyperliquidChain::Testnet,
        PreparedPerpCancelByCloid { asset: 3, cloid: 0 },
        1,
        None,
        2,
    )
    .unwrap_err();

    assert!(error.to_string().contains("non-zero cloid"), "{error}");
}

#[test]
fn cloid_accessor_rejects_mutated_action_instead_of_panicking() {
    let mut prepared = prepared();
    let Action::Order(batch) = &mut prepared.action else {
        panic!("test fixture must be an order");
    };
    batch.orders.clear();

    let error = prepared.cloid().unwrap_err();
    assert!(error.to_string().contains("exactly one order"), "{error}");
}

#[test]
fn prepared_action_rejects_digest_drift() {
    let mut prepared = prepared();
    prepared.action_digest[0] ^= 1;

    let error = prepared.validate().unwrap_err();

    assert!(
        error
            .to_string()
            .contains("digest does not match frozen signing inputs"),
        "{error}"
    );
}

#[tokio::test]
async fn signed_request_accepts_only_expected_api_wallet() {
    let prepared = prepared();
    let signer: PrivateKeySigner = TEST_PRIVATE_KEY.parse().unwrap();
    let signature = prepared.sign(&signer).await.unwrap();
    let bytes = prepared
        .signed_request_bytes(signature, signer.address())
        .unwrap();
    let request: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(request["nonce"], prepared.nonce);
    assert_eq!(
        request["expiresAfter"],
        serde_json::Value::from(prepared.expires_after_ms)
    );
    assert!(request.get("signature").is_some());

    let signature = prepared.sign(&signer).await.unwrap();
    let error = prepared
        .signed_request_bytes(
            signature,
            address!("0x1111111111111111111111111111111111111111"),
        )
        .unwrap_err();
    assert!(error.to_string().contains("recovered"), "{error}");
}

#[test]
fn first_gateway_slice_rejects_alo_and_zero_cloid() {
    let alo = PreparedHyperliquidAction::prepare_limit_order(
        HyperliquidChain::Testnet,
        PreparedPerpLimitOrder::new(0, true, Decimal::ONE, Decimal::ONE, Tif::Alo, 42),
        1,
        None,
        2,
    )
    .unwrap_err();
    assert!(alo.to_string().contains("ALO"), "{alo}");

    let zero_cloid = PreparedHyperliquidAction::prepare_limit_order(
        HyperliquidChain::Testnet,
        PreparedPerpLimitOrder::new(0, true, Decimal::ONE, Decimal::ONE, Tif::Ioc, 0),
        1,
        None,
        2,
    )
    .unwrap_err();
    assert!(zero_cloid.to_string().contains("cloid"), "{zero_cloid}");

    let expired = PreparedHyperliquidAction::prepare_limit_order(
        HyperliquidChain::Testnet,
        PreparedPerpLimitOrder::new(0, true, Decimal::ONE, Decimal::ONE, Tif::Ioc, 42),
        2,
        None,
        2,
    )
    .unwrap_err();
    assert!(
        expired.to_string().contains("must be after nonce"),
        "{expired}"
    );
}

#[test]
fn first_gateway_slice_rejects_every_other_order_type_and_tif() {
    let mut frontend_market = prepared();
    let Action::Order(batch) = &mut frontend_market.action else {
        panic!("test fixture must be an order");
    };
    batch.orders[0].order_type = OrderTypePlacement::Limit {
        tif: TimeInForce::FrontendMarket,
    };
    let error = frontend_market.validate().unwrap_err();
    assert!(error.to_string().contains("GTC and IOC"), "{error}");

    let mut trigger = prepared();
    let Action::Order(batch) = &mut trigger.action else {
        panic!("test fixture must be an order");
    };
    batch.orders[0].order_type = OrderTypePlacement::Trigger {
        is_market: true,
        trigger_px: Decimal::ONE,
        tpsl: TpSl::Sl,
    };
    let error = trigger.validate().unwrap_err();
    assert!(error.to_string().contains("GTC and IOC"), "{error}");
}

#[test]
fn submission_response_requires_the_documented_order_shape() {
    let rejected_order = br#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"error":"bad nonce"}]}}}"#;
    assert_eq!(
        PreparedHyperliquidAction::classify_submission_response(
            br#"{"status":"ok","response":{"type":"cancel","data":{"statuses":["success"]}}}"#
        ),
        HyperliquidSubmissionClassification::Accepted
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_submission_response(
            br#"{"status":"ok","response":{"type":"cancel","data":{"statuses":[{"error":"order was never placed"}]}}}"#
        ),
        HyperliquidSubmissionClassification::Rejected
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_submission_response(
            br#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"resting":{"oid":1,"cloid":null}}]}}}"#
        ),
        HyperliquidSubmissionClassification::Accepted
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_submission_response(rejected_order),
        HyperliquidSubmissionClassification::Rejected
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_submission_response(
            br#"{"status":"ok","response":{"type":"order","data":{"statuses":["waitingForTrigger"]}}}"#
        ),
        HyperliquidSubmissionClassification::Unknown
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_submission_response(
            br#"{"status":"err","response":"rate limited"}"#
        ),
        HyperliquidSubmissionClassification::Rejected
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_submission_response(br#"{"status":"ok"}"#),
        HyperliquidSubmissionClassification::Unknown
    );
}
