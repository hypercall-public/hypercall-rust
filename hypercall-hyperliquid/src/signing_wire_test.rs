use alloy::primitives::{address, b256};
use hypersdk::hypercore::types::{
    Action, BatchOrder, Builder, OrderGrouping, OrderRequest, OrderTypePlacement, TimeInForce,
};
use rust_decimal::Decimal;

use super::action_connection_id;

fn btc_order(builder: Option<Builder>) -> Action {
    Action::Order(BatchOrder {
        orders: vec![OrderRequest {
            asset: 0,
            is_buy: true,
            limit_px: Decimal::from(41_700),
            sz: Decimal::new(3, 4),
            reduce_only: false,
            order_type: OrderTypePlacement::Limit {
                tif: TimeInForce::Gtc,
            },
            cloid: Default::default(),
        }],
        grouping: OrderGrouping::Na,
        builder,
    })
}

#[test]
fn builder_order_hash_matches_hyperliquid_python_wire() {
    // Reference: msgpack.packb(order_wires_to_order_action(...)) from the
    // official Python SDK, with nonce bytes and the no-vault marker appended.
    let action = btc_order(Some(Builder {
        builder_address: address!("406cebfc7063e55761007d1c4b66b29308016602"),
        fee: 0,
    }));
    let nonce = 1_800_000_000_001;
    assert_eq!(
        action_connection_id(&action, nonce, None, None).unwrap(),
        b256!("530b8d8e09e2bece044aaf9af584b2c44e078f66419ee4a38457411b857f920d")
    );
    assert_ne!(
        action.hash(nonce, None, None).unwrap(),
        action_connection_id(&action, nonce, None, None).unwrap(),
        "hypersdk currently hashes the builder address as raw MessagePack bytes"
    );
}

#[test]
fn orders_without_builders_keep_the_sdk_hash() {
    let action = btc_order(None);
    let nonce = 1_800_000_000_001;
    assert_eq!(
        action_connection_id(&action, nonce, None, None).unwrap(),
        action.hash(nonce, None, None).unwrap()
    );
}

#[test]
fn schedule_cancel_hashes_match_the_official_sdk() {
    use hypersdk::hypercore::types::ScheduleCancel;
    // Vectors from hyperliquid-python-sdk `action_hash` with nonce
    // 1_800_000_000_001 and expiresAfter 1_800_000_009_001. Clearing sends
    // `{"type":"scheduleCancel"}` with no `time` key; hashing `time: nil`
    // instead recovers a different signer at the venue (seen on testnet).
    let clear = Action::ScheduleCancel(ScheduleCancel { time: None });
    assert_eq!(
        action_connection_id(&clear, 1_800_000_000_001, None, Some(1_800_000_009_001)).unwrap(),
        b256!("065b24b47b6c3ab804ab87e361a253fe2c6e1dead7ffcc6a43953771e4489c58")
    );
    let set = Action::ScheduleCancel(ScheduleCancel {
        time: Some(1_800_000_060_000),
    });
    assert_eq!(
        action_connection_id(&set, 1_800_000_000_001, None, Some(1_800_000_009_001)).unwrap(),
        b256!("a5ff51cae99a4b975d9d24baf5aeab38ab01b1a2d76f471c4289428eee84edce")
    );
}
