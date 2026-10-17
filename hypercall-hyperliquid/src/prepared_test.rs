use alloy::{primitives::address, signers::local::PrivateKeySigner};
use hypersdk::hypercore::types::{Action, OrderTypePlacement, TimeInForce, TpSl};
use rust_decimal::Decimal;

use crate::{
    HyperliquidChain, HyperliquidSubmissionClassification, PreparedHyperliquidAction,
    PreparedHyperliquidActionError, PreparedPerpCancelByCloid, PreparedPerpCancelByOid,
    PreparedPerpLeverageUpdate, PreparedPerpLimitOrder, PreparedPerpOrder,
    PreparedPerpOrderGrouping, PreparedPerpOrderType, PreparedPerpTriggerKind, Tif,
    PREPARED_HYPERLIQUID_ACTION_VERSION,
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
fn oid_cancel_and_schedule_cancel_preserve_exact_native_action() {
    let nonce = 1_800_000_000_001;
    let expiry = nonce + 999;
    let cancel = PreparedHyperliquidAction::prepare_cancel_by_oid(
        HyperliquidChain::Testnet,
        PreparedPerpCancelByOid { asset: 0, oid: 42 },
        nonce,
        None,
        expiry,
    )
    .unwrap();
    let bytes = cancel.canonical_bytes().unwrap();
    let decoded = PreparedHyperliquidAction::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(decoded.action_digest, cancel.action_digest);
    assert!(matches!(decoded.action, Action::Cancel(_)));
    assert!(decoded.cloid().is_err());

    for time in [Some(nonce + 60_000), None] {
        let schedule = PreparedHyperliquidAction::prepare_schedule_cancel(
            HyperliquidChain::Testnet,
            time,
            nonce,
            None,
            expiry,
        )
        .unwrap();
        let decoded =
            PreparedHyperliquidAction::from_canonical_bytes(&schedule.canonical_bytes().unwrap())
                .unwrap();
        assert!(matches!(decoded.action, Action::ScheduleCancel(_)));
        assert_eq!(decoded.action_digest, schedule.action_digest);
    }
    assert!(PreparedHyperliquidAction::prepare_cancel_by_oid(
        HyperliquidChain::Testnet,
        PreparedPerpCancelByOid { asset: 0, oid: 0 },
        nonce,
        None,
        expiry
    )
    .is_err());
}

#[test]
fn zero_fee_builder_is_bound_to_prepared_action() {
    let prepared = prepared();
    let builder = address!("1111111111111111111111111111111111111111");
    let attributed = PreparedHyperliquidAction::prepare_limit_order_with_builder(
        prepared.chain,
        prepared.limit_order().unwrap(),
        prepared.nonce,
        None,
        prepared.expires_after_ms,
        Some(builder),
    )
    .unwrap();
    let bytes = attributed.canonical_bytes().unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("\"f\":0"));
    assert_ne!(attributed.action_digest, prepared.action_digest);
    assert!(PreparedHyperliquidAction::from_canonical_bytes(&bytes).is_ok());

    let mut nonzero = attributed;
    if let Action::Order(batch) = &mut nonzero.action {
        batch.builder.as_mut().unwrap().fee = 1;
    }
    assert!(nonzero.validate().is_err());
}

#[test]
fn version_one_builder_action_remains_valid_for_replay() {
    let base = prepared();
    let builder = address!("1234567890123456789012345678901234567890");
    let mut legacy = PreparedHyperliquidAction::prepare_limit_order_with_builder(
        base.chain,
        base.limit_order().unwrap(),
        base.nonce,
        None,
        base.expires_after_ms,
        Some(builder),
    )
    .unwrap();
    let current_digest = legacy.action_digest;
    legacy.version = 1;
    let connection_id = legacy
        .action
        .hash(legacy.nonce, None, Some(legacy.expires_after_ms))
        .unwrap();
    legacy.action_digest = crate::prepared::agent_signing_hash(legacy.chain, connection_id);
    assert_ne!(legacy.action_digest, current_digest);
    let canonical = serde_json::to_vec(&legacy).unwrap();
    assert!(legacy.validate_journaled().is_ok());
    assert!(PreparedHyperliquidAction::from_canonical_bytes(&canonical).is_err());
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
    assert_eq!(
        error.to_string(),
        "invalid prepared Hyperliquid action: prepared Hyperliquid action is not canonically encoded"
    );
}

#[test]
fn prepared_action_errors_preserve_the_legacy_display_contract() {
    let decode_error = PreparedHyperliquidAction::from_canonical_bytes(b"{").unwrap_err();
    let decode_source = match &decode_error {
        PreparedHyperliquidActionError::Serialization { operation, source } => {
            assert_eq!(*operation, "invalid prepared Hyperliquid action encoding");
            source.to_string()
        }
        error => panic!("malformed JSON must retain its typed source, got {error:?}"),
    };
    assert_eq!(
        decode_error.to_string(),
        format!(
            "invalid prepared Hyperliquid action: invalid prepared Hyperliquid action encoding: {decode_source}"
        )
    );

    let signing_error = PreparedHyperliquidActionError::Signing {
        operation: "sign prepared Hyperliquid action digest",
        source: Box::new(std::io::Error::other("signer unavailable")),
    };
    assert_eq!(
        signing_error.to_string(),
        "prepared Hyperliquid action signing failed: failed to sign prepared Hyperliquid action digest: signer unavailable"
    );

    let hash_error = PreparedHyperliquidActionError::Hash {
        source: Box::new(std::io::Error::other("hash unavailable")),
    };
    assert_eq!(
        hash_error.to_string(),
        "invalid prepared Hyperliquid action: failed to hash prepared Hyperliquid action: hash unavailable"
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
fn leverage_update_roundtrips_exact_request_and_has_no_cloid() {
    let update = PreparedPerpLeverageUpdate {
        asset: 3,
        is_cross: false,
        leverage: 7,
    };
    let prepared = PreparedHyperliquidAction::prepare_leverage_update(
        HyperliquidChain::Testnet,
        update.clone(),
        1_800_000_000_004,
        1_800_000_001_003,
    )
    .unwrap();
    let decoded =
        PreparedHyperliquidAction::from_canonical_bytes(&prepared.canonical_bytes().unwrap())
            .unwrap();
    assert_eq!(decoded.leverage_update().unwrap(), update);
    assert!(decoded.cloid().is_err());

    let mut changed = decoded;
    if let Action::UpdateLeverage(ref mut action) = changed.action {
        action.leverage = 8;
    }
    assert!(changed.validate().is_err());
}

#[test]
fn leverage_response_requires_exact_default_ack() {
    assert_eq!(
        PreparedHyperliquidAction::classify_leverage_response(
            br#"{"status":"ok","response":{"type":"default"}}"#,
        ),
        HyperliquidSubmissionClassification::Accepted,
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_leverage_response(b"{}"),
        HyperliquidSubmissionClassification::Unknown,
    );
}

#[test]
fn cloid_accessor_rejects_mutated_action_instead_of_panicking() {
    let mut prepared = prepared();
    let Action::Order(batch) = &mut prepared.action else {
        panic!("test fixture must be an order");
    };
    batch.orders.clear();

    let error = prepared.cloid().unwrap_err();
    assert!(error.to_string().contains("1 to 100 orders"), "{error}");
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
    let expected_api_wallet = address!("0x1111111111111111111111111111111111111111");
    let error = prepared
        .signed_request_bytes(signature, expected_api_wallet)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "prepared Hyperliquid action signing failed: prepared Hyperliquid signature recovered {}, expected {expected_api_wallet}",
            signer.address()
        )
    );
}

#[tokio::test]
async fn text_builder_request_recovers_expected_signer() {
    let base = prepared();
    let attributed = PreparedHyperliquidAction::prepare_limit_order_with_builder(
        base.chain,
        base.limit_order().unwrap(),
        base.nonce,
        None,
        base.expires_after_ms,
        Some(address!("0x1234567890123456789012345678901234567890")),
    )
    .unwrap();
    let signer: PrivateKeySigner = TEST_PRIVATE_KEY.parse().unwrap();
    let signature = attributed.sign(&signer).await.unwrap();
    let bytes = attributed
        .signed_request_bytes(signature, signer.address())
        .unwrap();
    let request: hypersdk::hypercore::types::api::ActionRequest =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        crate::recover_action_request(&request, attributed.chain).unwrap(),
        signer.address()
    );
    assert_ne!(request.recover(attributed.chain).unwrap(), signer.address());
}

#[test]
fn gateway_preserves_alo_and_rejects_zero_cloid() {
    let alo = PreparedHyperliquidAction::prepare_limit_order(
        HyperliquidChain::Testnet,
        PreparedPerpLimitOrder::new(
            0,
            true,
            Decimal::from(70_000),
            Decimal::new(1, 1),
            Tif::Alo,
            42,
        ),
        1_800_000_000_001,
        None,
        1_800_000_001_000,
    )
    .unwrap();
    assert_eq!(alo.limit_order().unwrap().tif, Tif::Alo);
    let native: serde_json::Value =
        serde_json::from_slice(&alo.canonical_bytes().unwrap()).unwrap();
    assert_eq!(native["action"]["orders"][0]["t"]["limit"]["tif"], "Alo");

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
fn native_order_types_reject_frontend_market_and_accept_trigger_batches() {
    let mut frontend_market = prepared();
    let Action::Order(batch) = &mut frontend_market.action else {
        panic!("test fixture must be an order");
    };
    batch.orders[0].order_type = OrderTypePlacement::Limit {
        tif: TimeInForce::FrontendMarket,
    };
    let error = frontend_market.validate().unwrap_err();
    assert!(
        error.to_string().contains("unsupported or invalid"),
        "{error}"
    );

    let builder = address!("0x1234567890123456789012345678901234567890");
    let trigger = PreparedHyperliquidAction::prepare_order_batch_with_builder(
        HyperliquidChain::Testnet,
        vec![
            PreparedPerpOrder {
                asset: 0,
                is_buy: true,
                limit_px: Decimal::from(70_000),
                size: Decimal::new(1, 1),
                reduce_only: false,
                order_type: PreparedPerpOrderType::Limit(Tif::Gtc),
                cloid: 42,
            },
            PreparedPerpOrder {
                asset: 0,
                is_buy: false,
                limit_px: Decimal::from(69_000),
                size: Decimal::new(1, 1),
                reduce_only: true,
                order_type: PreparedPerpOrderType::Trigger {
                    is_market: false,
                    trigger_px: Decimal::from(69_500),
                    tpsl: PreparedPerpTriggerKind::StopLoss,
                },
                cloid: 43,
            },
        ],
        PreparedPerpOrderGrouping::NormalTpsl,
        1_800_000_000_001,
        None,
        1_800_000_001_000,
        builder,
    )
    .unwrap();
    let bytes = trigger.canonical_bytes().unwrap();
    let decoded = PreparedHyperliquidAction::from_canonical_bytes(&bytes).unwrap();
    let Action::Order(batch) = decoded.action else {
        panic!("expected native order batch");
    };
    assert_eq!(batch.orders.len(), 2);
    assert_eq!(batch.builder.unwrap().builder_address, builder);
    assert!(matches!(
        batch.orders[1].order_type,
        OrderTypePlacement::Trigger {
            is_market: false,
            tpsl: TpSl::Sl,
            ..
        }
    ));
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
        HyperliquidSubmissionClassification::Accepted
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

#[test]
fn schedule_cancel_accepts_only_default_exchange_success() {
    let default = br#"{"status":"ok","response":{"type":"default"}}"#;
    assert_eq!(
        PreparedHyperliquidAction::classify_schedule_cancel_response(default),
        HyperliquidSubmissionClassification::Accepted
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_submission_response(default),
        HyperliquidSubmissionClassification::Unknown
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_schedule_cancel_response(
            br#"{"status":"ok","response":{"type":"order","data":{"statuses":["success"]}}}"#
        ),
        HyperliquidSubmissionClassification::Unknown
    );
}

#[test]
fn order_batch_rejects_repeated_cloid() {
    let order = PreparedPerpOrder {
        asset: 0,
        is_buy: true,
        limit_px: Decimal::from(70_000),
        size: Decimal::new(1, 1),
        reduce_only: false,
        order_type: PreparedPerpOrderType::Limit(Tif::Gtc),
        cloid: 42,
    };
    let result = PreparedHyperliquidAction::prepare_order_batch_with_builder(
        HyperliquidChain::Testnet,
        vec![order.clone(), order],
        PreparedPerpOrderGrouping::None,
        1_800_000_000_001,
        None,
        1_800_000_001_000,
        address!("1234567890123456789012345678901234567890"),
    );
    assert!(result.is_err());
}

#[test]
fn clearing_schedule_cancel_request_omits_time() {
    let signer: PrivateKeySigner = TEST_PRIVATE_KEY.parse().unwrap();
    let prepared = PreparedHyperliquidAction::prepare_schedule_cancel(
        HyperliquidChain::Testnet,
        None,
        1_800_000_000_001,
        None,
        1_800_000_009_001,
    )
    .unwrap();
    let signature =
        alloy::signers::SignerSync::sign_hash_sync(&signer, &prepared.action_digest).unwrap();
    let body: serde_json::Value = serde_json::from_slice(
        &prepared
            .signed_request_bytes(signature, signer.address())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        body["action"],
        serde_json::json!({"type": "scheduleCancel"})
    );
}

fn bracket_batch() -> PreparedHyperliquidAction {
    PreparedHyperliquidAction::prepare_order_batch_with_builder(
        HyperliquidChain::Testnet,
        vec![
            PreparedPerpOrder {
                asset: 0,
                is_buy: true,
                limit_px: Decimal::from(70_000),
                size: Decimal::new(1, 1),
                reduce_only: false,
                order_type: PreparedPerpOrderType::Limit(Tif::Gtc),
                cloid: 42,
            },
            PreparedPerpOrder {
                asset: 0,
                is_buy: false,
                limit_px: Decimal::from(80_000),
                size: Decimal::new(1, 1),
                reduce_only: true,
                order_type: PreparedPerpOrderType::Trigger {
                    is_market: true,
                    trigger_px: Decimal::from(75_000),
                    tpsl: PreparedPerpTriggerKind::TakeProfit,
                },
                cloid: 43,
            },
            PreparedPerpOrder {
                asset: 0,
                is_buy: false,
                limit_px: Decimal::from(60_000),
                size: Decimal::new(1, 1),
                reduce_only: true,
                order_type: PreparedPerpOrderType::Trigger {
                    is_market: false,
                    trigger_px: Decimal::from(65_000),
                    tpsl: PreparedPerpTriggerKind::StopLoss,
                },
                cloid: 44,
            },
        ],
        PreparedPerpOrderGrouping::NormalTpsl,
        1_800_000_000_001,
        None,
        1_800_000_001_000,
        address!("0x1234567890123456789012345678901234567890"),
    )
    .unwrap()
}

#[test]
fn order_batch_accessor_returns_every_child_and_grouping() {
    let bracket = bracket_batch();
    let batch = bracket.order_batch().unwrap();
    assert_eq!(batch.grouping, PreparedPerpOrderGrouping::NormalTpsl);
    assert_eq!(
        batch
            .orders
            .iter()
            .map(|order| order.cloid)
            .collect::<Vec<_>>(),
        vec![42, 43, 44]
    );
    assert_eq!(
        batch.orders[1].order_type,
        PreparedPerpOrderType::Trigger {
            is_market: true,
            trigger_px: Decimal::from(75_000),
            tpsl: PreparedPerpTriggerKind::TakeProfit,
        }
    );
    // A bracket is not one standalone limit order, so the single-order
    // accessor that HlLimitOrder signing relies on must keep refusing it.
    assert!(bracket.limit_order().is_err());
    assert_eq!(prepared().order_batch().unwrap().orders.len(), 1);
    let cancel = PreparedHyperliquidAction::prepare_cancel_by_cloid(
        HyperliquidChain::Testnet,
        PreparedPerpCancelByCloid { asset: 0, cloid: 9 },
        1_800_000_000_001,
        None,
        1_800_000_001_000,
    )
    .unwrap();
    assert!(cancel.order_batch().is_err());
}

#[test]
fn order_batch_response_requires_one_status_per_child() {
    let all_accepted = br#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"resting":{"oid":1}},"waitingForFill","waitingForFill"]}}}"#;
    let all_rejected = br#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"error":"a"},{"error":"b"}]}}}"#;
    let mixed = br#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"resting":{"oid":1}},{"error":"b"}]}}}"#;
    assert_eq!(
        PreparedHyperliquidAction::classify_order_batch_response(all_accepted, 3),
        HyperliquidSubmissionClassification::Accepted
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_order_batch_response(all_accepted, 2),
        HyperliquidSubmissionClassification::Unknown
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_order_batch_response(all_rejected, 2),
        HyperliquidSubmissionClassification::Rejected
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_order_batch_response(mixed, 2),
        HyperliquidSubmissionClassification::Unknown
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_order_batch_response(
            br#"{"status":"err","response":"Invalid nonce"}"#,
            2
        ),
        HyperliquidSubmissionClassification::Rejected
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_order_batch_response(
            br#"{"status":"ok","response":{"type":"default"}}"#,
            1
        ),
        HyperliquidSubmissionClassification::Unknown
    );
}
