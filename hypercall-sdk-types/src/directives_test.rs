use super::*;
use crate::test_wallet;

#[test]
fn large_directive_integers_serialize_as_strings() {
    let action = HlLimitOrderAction {
        asset: 1,
        is_buy: true,
        limit_px: JSON_SAFE_INTEGER_MAX + 1,
        sz: 1,
        reduce_only: false,
        tif: PerpTimeInForce::Gtc,
        cloid: u128::MAX,
    };

    let value = serde_json::to_value(action).unwrap();
    assert_eq!(value["limitPx"], (JSON_SAFE_INTEGER_MAX + 1).to_string());
    assert_eq!(value["sz"], 1);
    assert_eq!(value["cloid"], u128::MAX.to_string());
}

#[test]
fn unsafe_json_numbers_fail_closed() {
    let value = serde_json::json!({
        "asset": 1,
        "isBuy": true,
        "limitPx": JSON_SAFE_INTEGER_MAX + 1,
        "sz": 1,
        "reduceOnly": false,
        "encodedTif": 2,
        "cloid": "1"
    });

    assert!(serde_json::from_value::<HlLimitOrderAction>(value).is_err());
}

#[test]
fn unsupported_tif_encoding_fails_closed() {
    let value = serde_json::json!({
        "asset": 1,
        "isBuy": true,
        "limitPx": 1,
        "sz": 1,
        "reduceOnly": false,
        "encodedTif": 4,
        "cloid": 1
    });

    assert!(serde_json::from_value::<HlLimitOrderAction>(value).is_err());
}

#[test]
fn account_abstraction_serializes_only_unified_mode() {
    let user = test_wallet(7);
    let action = HlSetAbstractionAction {
        user,
        abstraction: HypercoreAccountAbstraction::UnifiedAccount,
    };

    let value = serde_json::to_value(action).unwrap();
    assert_eq!(value, serde_json::json!({"user": user, "abstraction": 2}));

    for unsupported in [1, 3] {
        assert!(
            serde_json::from_value::<HlSetAbstractionAction>(serde_json::json!({
                "user": user,
                "abstraction": unsupported
            }))
            .is_err()
        );
    }
}

#[test]
fn api_wallet_update_serializes_bytes32_name_and_address() {
    let action = HcUpdateApiWalletAction {
        name: alloy::primitives::B256::repeat_byte(0x11),
        addr: test_wallet(7),
    };

    let value = serde_json::to_value(action).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "name": format!("{:#x}", action.name),
            "addr": action.addr,
        })
    );
    assert_eq!(
        serde_json::from_value::<HcUpdateApiWalletAction>(value).unwrap(),
        action
    );

    for invalid_name in ["11", "0x11", &format!("0x{}g", "1".repeat(63))] {
        assert!(
            serde_json::from_value::<HcUpdateApiWalletAction>(serde_json::json!({
                "name": invalid_name,
                "addr": action.addr,
            }))
            .is_err()
        );
    }
}

#[test]
fn directive_status_is_strictly_typed() {
    let status: DirectiveStatusResponse = serde_json::from_value(serde_json::json!({
        "directive_id": "d1",
        "action_key": "hl_limit_order",
        "domain_status": "pending_chain_effect",
        "delivery_status": "core_rejected",
        "tx_hash": null,
        "created_at": null
    }))
    .unwrap();

    assert_eq!(
        status.domain_status,
        DirectiveDomainStatus::PendingChainEffect
    );
    assert_eq!(
        status.delivery_status,
        DirectiveDeliveryStatus::CoreRejected
    );
}

#[test]
fn directive_status_decodes_pm_cash_withdrawal_states() {
    let states = [
        (
            "completed",
            "pending_core_delivery",
            DirectiveDomainStatus::Completed,
            DirectiveDeliveryStatus::PendingCoreDelivery,
        ),
        (
            "completed",
            "awaiting_portfolio_snapshot",
            DirectiveDomainStatus::Completed,
            DirectiveDeliveryStatus::AwaitingPortfolioSnapshot,
        ),
        (
            "pending_manual_review",
            "manual_review",
            DirectiveDomainStatus::PendingManualReview,
            DirectiveDeliveryStatus::ManualReview,
        ),
    ];

    for (domain, delivery, expected_domain, expected_delivery) in states {
        let status: DirectiveStatusResponse = serde_json::from_value(serde_json::json!({
            "directive_id": "withdrawal-1",
            "action_key": "hl_send_asset",
            "domain_status": domain,
            "delivery_status": delivery,
            "tx_hash": null,
            "created_at": null
        }))
        .unwrap();

        assert_eq!(status.domain_status, expected_domain);
        assert_eq!(status.delivery_status, expected_delivery);
    }
}
