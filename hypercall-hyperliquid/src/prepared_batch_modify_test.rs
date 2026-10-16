use hypersdk::hypercore::types::Action;
use rust_decimal::Decimal;

use crate::{
    HyperliquidChain, HyperliquidSubmissionClassification as Class, PreparedHyperliquidAction,
    PreparedModifyTarget, PreparedPerpModify, Tif, MAX_BATCH_MODIFY_CHILDREN,
};

const NONCE: u64 = 1_800_000_000_001;
const EXPIRY: u64 = NONCE + 9_000;

fn child(index: u32, target: PreparedModifyTarget) -> PreparedPerpModify {
    PreparedPerpModify {
        target,
        asset: index % 4,
        is_buy: index.is_multiple_of(2),
        limit_px: Decimal::from(70_000 + index),
        size: Decimal::new(i64::from(index) + 1, 2),
        reduce_only: index.is_multiple_of(3),
        tif: Tif::Gtc,
        cloid: 1_000 + u128::from(index),
    }
}

fn children(count: u32) -> Vec<PreparedPerpModify> {
    (0..count)
        .map(|index| {
            child(
                index,
                PreparedModifyTarget::Cloid(1_000 + u128::from(index)),
            )
        })
        .collect()
}

fn prepare(modifies: Vec<PreparedPerpModify>) -> Result<PreparedHyperliquidAction, String> {
    PreparedHyperliquidAction::prepare_batch_modify(
        HyperliquidChain::Testnet,
        modifies,
        NONCE,
        None,
        EXPIRY,
    )
    .map_err(|error| error.to_string())
}

#[test]
fn every_child_roundtrips_in_signed_order() {
    for count in [1, 2, 7, MAX_BATCH_MODIFY_CHILDREN as u32] {
        let mut modifies = children(count);
        if count > 1 {
            modifies[1].target = PreparedModifyTarget::Oid(555);
        }
        let prepared = prepare(modifies.clone()).unwrap();
        let bytes = prepared.canonical_bytes().unwrap();
        let decoded = PreparedHyperliquidAction::from_canonical_bytes(&bytes).unwrap();
        let Action::BatchModify(batch) = &decoded.action else {
            panic!("not a batchModify");
        };
        assert_eq!(batch.modifies.len(), count as usize);
        assert_eq!(decoded.batch_modifies().unwrap(), modifies);
        assert_eq!(
            decoded.cloid().unwrap().as_slice(),
            1_000_u128.to_be_bytes()
        );
        assert_eq!(decoded.order_builder(), None);
        // A single-child accessor never sees only the first child of a batch.
        assert_eq!(decoded.modify().is_ok(), count == 1);
    }
}

#[test]
fn the_one_child_batch_is_the_single_modify() {
    let single = PreparedHyperliquidAction::prepare_modify(
        HyperliquidChain::Testnet,
        children(1).remove(0),
        NONCE,
        None,
        EXPIRY,
    )
    .unwrap();
    let batch = prepare(children(1)).unwrap();
    assert_eq!(
        single.canonical_bytes().unwrap(),
        batch.canonical_bytes().unwrap()
    );
}

#[test]
fn the_digest_binds_child_order() {
    let forward = prepare(children(3)).unwrap();
    let mut reversed = children(3);
    reversed.reverse();
    let reversed = prepare(reversed).unwrap();
    assert_ne!(forward.action_digest, reversed.action_digest);
}

#[test]
fn unsafe_batches_are_refused() {
    assert!(prepare(Vec::new()).unwrap_err().contains("1 to 100"));
    assert!(prepare(children(MAX_BATCH_MODIFY_CHILDREN as u32 + 1))
        .unwrap_err()
        .contains("1 to 100"));
    let mut repeated_cloid = children(2);
    repeated_cloid[1].cloid = repeated_cloid[0].cloid;
    repeated_cloid[1].target = PreparedModifyTarget::Cloid(repeated_cloid[0].cloid);
    assert!(prepare(repeated_cloid)
        .unwrap_err()
        .contains("repeats a cloid"));
    let mut repeated_oid = children(2);
    repeated_oid[0].target = PreparedModifyTarget::Oid(9);
    repeated_oid[1].target = PreparedModifyTarget::Oid(9);
    assert!(prepare(repeated_oid)
        .unwrap_err()
        .contains("repeats an oid"));
    let mut changed_cloid = children(2);
    changed_cloid[1].target = PreparedModifyTarget::Cloid(77);
    assert!(prepare(changed_cloid)
        .unwrap_err()
        .contains("preserve its cloid"));
    let mut spot = children(2);
    spot[1].asset = 10_001;
    assert!(prepare(spot).unwrap_err().contains("spot asset"));
    let mut zero = children(2);
    zero[1].size = Decimal::ZERO;
    assert!(prepare(zero).unwrap_err().contains("positive"));
}

fn classify(body: &str, expected: usize) -> (Class, Vec<Class>) {
    let result =
        PreparedHyperliquidAction::classify_batch_modify_response(body.as_bytes(), expected);
    (result.batch, result.children)
}

#[test]
fn each_child_takes_its_own_venue_status() {
    let mixed = r#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"resting":{"oid":5}},{"error":"Cannot modify canceled or filled order"},{"filled":{"totalSz":"0.1","avgPx":"70000","oid":6}}]}}}"#;
    assert_eq!(
        classify(mixed, 3),
        (
            Class::Unknown,
            vec![Class::Accepted, Class::Rejected, Class::Accepted]
        )
    );
    let accepted = r#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"resting":{"oid":5}},{"resting":{"oid":6}}]}}}"#;
    assert_eq!(
        classify(accepted, 2),
        (Class::Accepted, vec![Class::Accepted; 2])
    );
    let rejected = r#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"error":"a"},{"error":"b"}]}}}"#;
    assert_eq!(
        classify(rejected, 2),
        (Class::Rejected, vec![Class::Rejected; 2])
    );
    // A waiting status is not a modify outcome.
    let waiting = r#"{"status":"ok","response":{"type":"order","data":{"statuses":["waitingForFill",{"resting":{"oid":6}}]}}}"#;
    assert_eq!(
        classify(waiting, 2),
        (Class::Unknown, vec![Class::Unknown, Class::Accepted])
    );
}

#[test]
fn whole_action_responses_apply_to_every_child() {
    assert_eq!(
        classify(r#"{"status":"err","response":"Invalid nonce"}"#, 3),
        (Class::Rejected, vec![Class::Rejected; 3])
    );
    // A miscounted status list matches no child.
    let one = r#"{"status":"ok","response":{"type":"order","data":{"statuses":[{"resting":{"oid":5}}]}}}"#;
    assert_eq!(classify(one, 2), (Class::Unknown, vec![Class::Unknown; 2]));
    // A bare response carries no per-child status: only a one-child batch
    // reads it as the single modify does.
    let bare = r#"{"status":"ok","response":{"type":"default"}}"#;
    assert_eq!(classify(bare, 2), (Class::Unknown, vec![Class::Unknown; 2]));
    assert_eq!(classify(bare, 1), (Class::Accepted, vec![Class::Accepted]));
    assert_eq!(
        classify("garbage", 2),
        (Class::Unknown, vec![Class::Unknown; 2])
    );
    assert_eq!(classify(one, 0), (Class::Unknown, Vec::new()));
}
