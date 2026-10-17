use alloy::signers::local::PrivateKeySigner;
use hypersdk::hypercore::types::Action;
use rust_decimal::Decimal;

use crate::{
    HyperliquidChain, HyperliquidSubmissionClassification as Class, PreparedHyperliquidAction,
    PreparedModifyTarget, PreparedPerpLimitOrder, PreparedPerpModify, PreparedPerpTwapCancel,
    PreparedPerpTwapOrder, Tif, TwapAction, TWAP_MAX_MINUTES, TWAP_MIN_MINUTES,
};

const KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const NONCE: u64 = 1_800_000_000_001;
const EXPIRY: u64 = NONCE + 9_000;

fn modify(target: PreparedModifyTarget) -> PreparedPerpModify {
    PreparedPerpModify {
        target,
        asset: 3,
        is_buy: true,
        limit_px: Decimal::from(70_500u32),
        size: Decimal::new(25, 2),
        reduce_only: false,
        tif: Tif::Gtc,
        cloid: 42,
    }
}

fn twap_order(minutes: u32) -> PreparedPerpTwapOrder {
    PreparedPerpTwapOrder {
        asset: 0,
        is_buy: true,
        size: Decimal::from(1u32),
        reduce_only: false,
        minutes,
        randomize: true,
    }
}

fn prepare_twap(minutes: u32) -> PreparedHyperliquidAction {
    PreparedHyperliquidAction::prepare_twap_order(
        HyperliquidChain::Testnet,
        twap_order(minutes),
        NONCE,
        None,
        EXPIRY,
    )
    .unwrap()
}

#[test]
fn modify_roundtrips_by_cloid_and_oid_and_keeps_its_cloid() {
    for target in [
        PreparedModifyTarget::Cloid(42),
        PreparedModifyTarget::Oid(777),
    ] {
        let prepared = PreparedHyperliquidAction::prepare_modify(
            HyperliquidChain::Testnet,
            modify(target),
            NONCE,
            None,
            EXPIRY,
        )
        .unwrap();
        let bytes = prepared.canonical_bytes().unwrap();
        let decoded = PreparedHyperliquidAction::from_canonical_bytes(&bytes).unwrap();
        assert!(matches!(decoded.action, Action::BatchModify(_)));
        assert_eq!(decoded.modify().unwrap(), modify(target));
        assert_eq!(decoded.cloid().unwrap().as_slice(), 42_u128.to_be_bytes());
        assert_eq!(decoded.order_builder(), None);
        assert!(decoded.twap.is_none());
    }
}

#[test]
fn modify_rejects_unsafe_shapes() {
    let prepare = |modify: PreparedPerpModify| {
        PreparedHyperliquidAction::prepare_modify(
            HyperliquidChain::Testnet,
            modify,
            NONCE,
            None,
            EXPIRY,
        )
    };
    assert!(prepare(modify(PreparedModifyTarget::Oid(0))).is_err());
    // A cloid-addressed modify must keep that cloid.
    let mut moved = modify(PreparedModifyTarget::Cloid(41));
    moved.cloid = 42;
    assert!(prepare(moved).is_err());
    let mut zero_cloid = modify(PreparedModifyTarget::Oid(1));
    zero_cloid.cloid = 0;
    assert!(prepare(zero_cloid).is_err());
    let mut zero_size = modify(PreparedModifyTarget::Oid(1));
    zero_size.size = Decimal::ZERO;
    assert!(prepare(zero_size).is_err());
    let mut spot = modify(PreparedModifyTarget::Oid(1));
    spot.asset = 10_000;
    assert!(prepare(spot).is_err());
}

#[test]
fn modify_digest_binds_every_economic_field() {
    let base = PreparedHyperliquidAction::prepare_modify(
        HyperliquidChain::Testnet,
        modify(PreparedModifyTarget::Cloid(42)),
        NONCE,
        None,
        EXPIRY,
    )
    .unwrap();
    let mut other = modify(PreparedModifyTarget::Cloid(42));
    other.limit_px = Decimal::from(70_501u32);
    let other = PreparedHyperliquidAction::prepare_modify(
        HyperliquidChain::Testnet,
        other,
        NONCE,
        None,
        EXPIRY,
    )
    .unwrap();
    assert_ne!(base.action_digest, other.action_digest);
}

#[test]
fn twap_order_and_cancel_roundtrip_with_placeholder_action() {
    let order = prepare_twap(30);
    assert!(matches!(order.action, Action::Noop));
    let decoded =
        PreparedHyperliquidAction::from_canonical_bytes(&order.canonical_bytes().unwrap()).unwrap();
    assert_eq!(decoded.twap_order().unwrap(), twap_order(30));
    assert!(decoded.twap_cancel().is_err());
    assert!(decoded.cloid().is_err());
    assert_eq!(decoded.order_builder(), None);

    let cancel = PreparedHyperliquidAction::prepare_twap_cancel(
        HyperliquidChain::Testnet,
        PreparedPerpTwapCancel {
            asset: 0,
            twap_id: 9,
        },
        NONCE,
        None,
        EXPIRY,
    )
    .unwrap();
    let decoded =
        PreparedHyperliquidAction::from_canonical_bytes(&cancel.canonical_bytes().unwrap())
            .unwrap();
    assert_eq!(
        decoded.twap_cancel().unwrap(),
        PreparedPerpTwapCancel {
            asset: 0,
            twap_id: 9
        }
    );
    assert!(decoded.twap_order().is_err());
    assert!(PreparedHyperliquidAction::prepare_twap_cancel(
        HyperliquidChain::Testnet,
        PreparedPerpTwapCancel {
            asset: 0,
            twap_id: 0
        },
        NONCE,
        None,
        EXPIRY,
    )
    .is_err());
}

#[test]
fn twap_duration_is_bounded() {
    for minutes in [TWAP_MIN_MINUTES, 30, TWAP_MAX_MINUTES] {
        assert!(PreparedHyperliquidAction::prepare_twap_order(
            HyperliquidChain::Testnet,
            twap_order(minutes),
            NONCE,
            None,
            EXPIRY,
        )
        .is_ok());
    }
    for minutes in [0, TWAP_MIN_MINUTES - 1, TWAP_MAX_MINUTES + 1] {
        assert!(PreparedHyperliquidAction::prepare_twap_order(
            HyperliquidChain::Testnet,
            twap_order(minutes),
            NONCE,
            None,
            EXPIRY,
        )
        .is_err());
    }
    let mut empty = twap_order(30);
    empty.size = Decimal::ZERO;
    assert!(PreparedHyperliquidAction::prepare_twap_order(
        HyperliquidChain::Testnet,
        empty,
        NONCE,
        None,
        EXPIRY,
    )
    .is_err());
}

#[test]
fn twap_wire_msgpack_matches_the_documented_key_order() {
    let order = TwapAction::Order {
        twap: crate::TwapOrderWire {
            asset: 0,
            is_buy: true,
            size: Decimal::new(100, 2),
            reduce_only: false,
            minutes: 30,
            randomize: true,
        },
    };
    // {"type":"twapOrder","twap":{"a":0,"b":true,"s":"1","r":false,"m":30,"t":true}}
    let expected = hex_literal(
        "82a474797065a9747761704f72646572a4747761708 6a16100a162c3a173a131a172c2a16d1ea174c3"
            .replace(' ', "")
            .as_str(),
    );
    assert_eq!(rmp_serde::to_vec_named(&order).unwrap(), expected);

    let cancel = TwapAction::Cancel(crate::TwapCancelWire {
        asset: 0,
        twap_id: 5,
    });
    // {"type":"twapCancel","a":0,"t":5}
    let expected = hex_literal("83a474797065aa7477617043616e63656ca16100a17405");
    assert_eq!(rmp_serde::to_vec_named(&cancel).unwrap(), expected);
}

fn hex_literal(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect()
}

#[test]
fn existing_actions_keep_their_canonical_bytes() {
    let limit = PreparedHyperliquidAction::prepare_limit_order(
        HyperliquidChain::Testnet,
        PreparedPerpLimitOrder::new(
            0,
            true,
            Decimal::from(70_000u32),
            Decimal::new(1, 1),
            Tif::Ioc,
            42,
        ),
        NONCE,
        None,
        EXPIRY,
    )
    .unwrap();
    let json = String::from_utf8(limit.canonical_bytes().unwrap()).unwrap();
    assert!(!json.contains("twap"));
}

#[tokio::test]
async fn twap_signed_request_carries_the_hashed_wire_and_recovers_the_signer() {
    let signer: PrivateKeySigner = KEY.parse().unwrap();
    let prepared = prepare_twap(30);
    let signature = prepared.sign(&signer).await.unwrap();
    let body = prepared
        .signed_request_bytes(signature, signer.address())
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        value["action"],
        serde_json::json!({
            "type": "twapOrder",
            "twap": {"a": 0, "b": true, "s": "1", "r": false, "m": 30, "t": true}
        })
    );
    assert_eq!(value["nonce"], NONCE);
    assert_eq!(value["expiresAfter"], EXPIRY);

    let cancel = PreparedHyperliquidAction::prepare_twap_cancel(
        HyperliquidChain::Testnet,
        PreparedPerpTwapCancel {
            asset: 0,
            twap_id: 5,
        },
        NONCE,
        None,
        EXPIRY,
    )
    .unwrap();
    let signature = cancel.sign(&signer).await.unwrap();
    let body = cancel
        .signed_request_bytes(signature, signer.address())
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        value["action"],
        serde_json::json!({"type": "twapCancel", "a": 0, "t": 5})
    );
}

#[tokio::test]
async fn modify_signed_request_uses_the_native_batch_modify_shape() {
    let signer: PrivateKeySigner = KEY.parse().unwrap();
    let prepared = PreparedHyperliquidAction::prepare_modify(
        HyperliquidChain::Testnet,
        modify(PreparedModifyTarget::Cloid(42)),
        NONCE,
        None,
        EXPIRY,
    )
    .unwrap();
    let signature = prepared.sign(&signer).await.unwrap();
    let body = prepared
        .signed_request_bytes(signature, signer.address())
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let action = &value["action"];
    assert_eq!(action["type"], "batchModify");
    let entry = &action["modifies"][0];
    assert_eq!(entry["oid"], "0x0000000000000000000000000000002a");
    assert_eq!(entry["order"]["c"], "0x0000000000000000000000000000002a");
    assert_eq!(entry["order"]["p"], "70500");
    assert_eq!(entry["order"]["s"], "0.25");
}

#[test]
fn tampered_twap_is_rejected() {
    let mut prepared = prepare_twap(30);
    if let Some(TwapAction::Order { twap }) = &mut prepared.twap {
        twap.minutes = 31;
    }
    assert!(prepared.validate().is_err());
    let mut placeholder = prepare_twap(30);
    placeholder.action =
        Action::ScheduleCancel(hypersdk::hypercore::types::ScheduleCancel { time: Some(1) });
    assert!(placeholder.validate().is_err());
}

#[test]
fn response_classification() {
    let modify_ok = br#"{"status":"ok","response":{"type":"modify","data":{}}}"#;
    assert_eq!(
        PreparedHyperliquidAction::classify_modify_response(modify_ok),
        Class::Accepted
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_modify_response(
            br#"{"status":"ok","response":{"type":"default"}}"#
        ),
        Class::Accepted
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_modify_response(
            br#"{"status":"err","response":{"type":"default"}}"#
        ),
        Class::Rejected
    );
    let modify_status = br#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"resting":{"oid":5}}]}}}"#;
    assert_eq!(
        PreparedHyperliquidAction::classify_modify_response(modify_status),
        Class::Accepted
    );
    let modify_error = br#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"error":"Order not found"}]}}}"#;
    assert_eq!(
        PreparedHyperliquidAction::classify_modify_response(modify_error),
        Class::Rejected
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_modify_response(br#"{"status":"err","response":"x"}"#),
        Class::Rejected
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_modify_response(b"garbage"),
        Class::Unknown
    );

    let running = br#"{"status":"ok","response":{"type":"twapOrder","data":{"status":{"running":{"twapId":77}}}}}"#;
    assert_eq!(
        PreparedHyperliquidAction::classify_twap_order_response(running),
        Class::Accepted
    );
    assert_eq!(
        PreparedHyperliquidAction::twap_id_from_order_response(running),
        Some(77)
    );
    let failed = br#"{"status":"ok","response":{"type":"twapOrder","data":{"status":{"error":"Invalid TWAP duration: 1 min(s)"}}}}"#;
    assert_eq!(
        PreparedHyperliquidAction::classify_twap_order_response(failed),
        Class::Rejected
    );
    assert_eq!(
        PreparedHyperliquidAction::twap_id_from_order_response(failed),
        None
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_twap_order_response(
            br#"{"status":"ok","response":{"type":"order","data":{}}}"#
        ),
        Class::Unknown
    );

    let cancelled =
        br#"{"status":"ok","response":{"type":"twapCancel","data":{"status":"success"}}}"#;
    assert_eq!(
        PreparedHyperliquidAction::classify_twap_cancel_response(cancelled),
        Class::Accepted
    );
    let cancel_error = br#"{"status":"ok","response":{"type":"twapCancel","data":{"status":{"error":"TWAP not found"}}}}"#;
    assert_eq!(
        PreparedHyperliquidAction::classify_twap_cancel_response(cancel_error),
        Class::Rejected
    );
    assert_eq!(
        PreparedHyperliquidAction::classify_twap_cancel_response(b"{}"),
        Class::Unknown
    );
}
