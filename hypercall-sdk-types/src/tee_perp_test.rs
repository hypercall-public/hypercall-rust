//! Wire fixtures follow the published OpenAPI schemas for the
//! `TEE Perp Trading` and `TEE API Wallets` tags.

use super::*;
use rust_decimal_macros::dec;
use serde_json::json;

const ACCOUNT: &str = "0x00000000000000000000000000000000000000a1";
const API_WALLET: &str = "0x00000000000000000000000000000000000000b2";

/// `PerpRejectionReasonCode.enum` from the published OpenAPI document.
const OPENAPI_REASON_CODES: [&str; 16] = [
    "insufficient_margin",
    "mode_change_with_position",
    "stale_position_snapshot",
    "market_not_supported",
    "unified_mode_required",
    "reduce_only_exceeds_position",
    "trading_halted",
    "account_not_pm",
    "venue_risk_unavailable",
    "price_unavailable",
    "liquidation_pending",
    "account_health_unavailable",
    "leverage_requires_flat_account",
    "action_not_supported",
    "invalid_order",
    "engine_rejected",
];

/// `TeePerpErrorCode.enum` from the published OpenAPI document.
const OPENAPI_ERROR_CODES: [&str; 29] = [
    "engine_admission_rejected",
    "duplicate_directive",
    "trading_unavailable",
    "execution_unavailable",
    "authorization_failed",
    "invalid_request",
    "wallet_not_admitted",
    "wallet_admission_mismatch",
    "wallet_admission_unavailable",
    "parent_credentials_unavailable",
    "parent_kms_request_invalid",
    "enclave_unavailable",
    "enclave_kms_request_invalid",
    "enclave_wallet_unavailable",
    "enclave_clock_unavailable",
    "enclave_policy_rejected",
    "enclave_authorization_rejected",
    "enclave_wallet_mismatch",
    "enclave_intent_expired",
    "enclave_intent_ttl_exceeded",
    "enclave_signing_failed",
    "enclave_attestation_failed",
    "enclave_response_invalid",
    "parent_response_invalid",
    "signed_request_invalid",
    "internal_unavailable",
    "venue_rejected",
    "builder_not_approved",
    "submission_outcome_unknown",
];

#[test]
fn reason_codes_match_the_server_list_in_order() {
    let ours: Vec<&str> = PerpRejectionReasonCode::ALL
        .iter()
        .map(|code| code.as_str())
        .collect();
    assert_eq!(ours, OPENAPI_REASON_CODES);
    for (code, wire) in PerpRejectionReasonCode::ALL
        .iter()
        .zip(OPENAPI_REASON_CODES)
    {
        assert_eq!(serde_json::to_value(code).unwrap(), json!(wire));
        let parsed: PerpRejectionReasonCode = serde_json::from_value(json!(wire)).unwrap();
        assert_eq!(parsed, *code);
    }
}

#[test]
fn unknown_codes_deserialize_as_unrecognized() {
    let reason: PerpRejectionReasonCode =
        serde_json::from_value(json!("brand_new_reason")).unwrap();
    assert_eq!(reason, PerpRejectionReasonCode::Unrecognized);
    let error: TeePerpErrorCode = serde_json::from_value(json!("brand_new_error")).unwrap();
    assert_eq!(error, TeePerpErrorCode::Unrecognized);
}

#[test]
fn every_server_error_code_is_known() {
    for wire in OPENAPI_ERROR_CODES {
        let parsed: TeePerpErrorCode = serde_json::from_value(json!(wire)).unwrap();
        assert_ne!(parsed, TeePerpErrorCode::Unrecognized, "{wire}");
        assert_eq!(serde_json::to_value(parsed).unwrap(), json!(wire));
    }
}

#[test]
fn trigger_order_wire_matches_openapi_shape() {
    let order = TeePerpOrderWire {
        asset: 0,
        is_buy: false,
        limit_px: 7_600_000_000_000,
        sz: 1_000_000,
        reduce_only: true,
        order_type: TeePerpOrderType::Trigger(TeePerpTriggerOrderType {
            trigger_px: 9_500_000_000_000,
            is_market: true,
            tpsl: TeePerpTpsl::Sl,
        }),
        cloid: 42,
    };
    assert_eq!(
        serde_json::to_value(order).unwrap(),
        json!({
            "asset": 0,
            "isBuy": false,
            "limitPx": 7_600_000_000_000_u64,
            "sz": 1_000_000,
            "reduceOnly": true,
            "orderType": {"trigger": {"triggerPx": 9_500_000_000_000_u64, "isMarket": true, "tpsl": "sl"}},
            "cloid": 42
        })
    );
}

#[test]
fn batch_request_round_trips_with_string_integers() {
    let wire = json!({
        "account": ACCOUNT,
        "nonce": "1785672000000",
        "action": {
            "grouping": "normalTpsl",
            "orders": [
                {"asset": 3, "isBuy": true, "limitPx": "10000000000000", "sz": "1000000",
                 "reduceOnly": false, "orderType": {"limit": {"encodedTif": 2}}, "cloid": "457131520000001"},
                {"asset": 3, "isBuy": false, "limitPx": "8800000000000", "sz": "1000000",
                 "reduceOnly": true, "orderType": {"trigger": {"triggerPx": "11000000000000", "isMarket": true, "tpsl": "tp"}},
                 "cloid": "340282366920938463463374607431768211455"}
            ]
        },
        "signature": "0x00"
    });
    let request: TeePerpOrderBatchSubmitRequest = serde_json::from_value(wire).unwrap();
    assert_eq!(request.nonce, 1_785_672_000_000);
    assert_eq!(request.action.grouping, TeePerpOrderGrouping::NormalTpsl);
    assert_eq!(request.action.orders[1].cloid, u128::MAX);
    assert_eq!(
        request.action.orders[0].order_type,
        TeePerpOrderType::Limit(TeePerpLimitOrderType {
            tif: PerpTimeInForce::Gtc
        })
    );
    let reencoded = serde_json::to_value(&request).unwrap();
    assert_eq!(
        reencoded["action"]["orders"][1]["cloid"],
        json!(u128::MAX.to_string())
    );
    assert_eq!(reencoded["nonce"], json!(1_785_672_000_000_u64));
}

#[test]
fn leverage_request_matches_openapi_shape() {
    let request = TeePerpLeverageSubmitRequest {
        account: ACCOUNT.parse().unwrap(),
        nonce: 7,
        action: TeePerpLeverageAction {
            asset: 0,
            is_cross: false,
            leverage: 5,
        },
        signature: "0xsig".into(),
    };
    let value = serde_json::to_value(request).unwrap();
    assert_eq!(
        value["action"],
        json!({"asset": 0, "isCross": false, "leverage": 5})
    );
}

#[test]
fn order_response_parses_numeric_or_string_engine_seq() {
    for engine_seq in [json!(12), json!("12")] {
        let response: TeePerpOrderResponse = serde_json::from_value(json!({
            "stage": "rejected",
            "errorCode": "engine_admission_rejected",
            "directiveId": "0195f3c4-0000-7000-8000-000000000001",
            "account": ACCOUNT,
            "apiWallet": API_WALLET,
            "cloid": "0x0000000000000000000000000000002a",
            "engineSeq": engine_seq,
            "venueResponseHash": null,
            "message": "Rejected: Insufficient margin: required=10, equity=5, shortfall=5",
            "reasonCode": "insufficient_margin"
        }))
        .unwrap();
        assert_eq!(response.engine_seq, Some(12));
        assert_eq!(
            response.reason_code,
            Some(PerpRejectionReasonCode::InsufficientMargin)
        );
        assert!(!response.is_accepted());
    }
}

#[test]
fn builder_not_approved_is_detected() {
    let response: TeePerpOrderBatchResponse = serde_json::from_value(json!({
        "stage": "rejected",
        "errorCode": "builder_not_approved",
        "batchId": "0195f3c4-0000-7000-8000-000000000002",
        "account": ACCOUNT,
        "apiWallet": API_WALLET,
        "grouping": "na",
        "children": [{"directiveId": "0195f3c4-0000-7000-8000-000000000003", "cloid": "0x01"}],
        "engineSeq": null,
        "venueResponseHash": null,
        "message": "builder fee has not been approved",
        "reasonCode": null
    }))
    .unwrap();
    assert!(response.requires_builder_approval());
    assert_eq!(response.children.len(), 1);
}

#[test]
fn venue_action_and_leverage_responses_parse() {
    let cancel: TeePerpVenueActionResponse = serde_json::from_value(json!({
        "stage": "accepted",
        "errorCode": null,
        "requestId": "0195f3c4-0000-8000-8000-000000000004",
        "account": ACCOUNT,
        "apiWallet": API_WALLET,
        "actionKind": "cancel_by_oid",
        "engineSeq": 99,
        "venueResponseHash": "0x1111111111111111111111111111111111111111111111111111111111111111",
        "message": null
    }))
    .unwrap();
    assert!(cancel.is_accepted());

    let leverage: TeePerpLeverageResponse = serde_json::from_value(json!({
        "stage": "rejected",
        "errorCode": "engine_admission_rejected",
        "requestId": "0195f3c4-0000-8000-8000-000000000005",
        "account": ACCOUNT,
        "apiWallet": API_WALLET,
        "asset": 0,
        "isCross": false,
        "leverage": 5,
        "engineSeq": null,
        "venueResponseHash": null,
        "message": "Rejected: margin mode cannot change while a position is open",
        "reasonCode": "mode_change_with_position"
    }))
    .unwrap();
    assert_eq!(
        leverage.reason_code,
        Some(PerpRejectionReasonCode::ModeChangeWithPosition)
    );
}

#[test]
fn preview_actions_serialize_with_kind_tags() {
    let order = PerpPreviewOrder {
        asset: 0,
        is_buy: true,
        limit_px: dec!(100000),
        sz: dec!(0.01),
        reduce_only: false,
        order_type: PerpPreviewOrderType::Limit(PerpPreviewLimit {
            tif: PerpPreviewTif::Ioc,
        }),
    };
    let request = PerpPreviewRequest {
        account: ACCOUNT.parse().unwrap(),
        action: PerpPreviewAction::Order {
            order,
            leverage: Some(PerpPreviewLeverageSetting {
                is_cross: true,
                value: 10,
            }),
        },
    };
    assert_eq!(
        serde_json::to_value(&request).unwrap()["action"],
        json!({
            "kind": "order",
            "asset": 0,
            "isBuy": true,
            "limitPx": "100000",
            "sz": "0.01",
            "reduceOnly": false,
            "orderType": {"limit": {"tif": "Ioc"}},
            "leverage": {"isCross": true, "value": 10}
        })
    );

    let batch = PerpPreviewAction::OrderBatch {
        grouping: TeePerpOrderGrouping::PositionTpsl,
        orders: vec![PerpPreviewOrder {
            order_type: PerpPreviewOrderType::Trigger(PerpPreviewTrigger {
                trigger_px: dec!(90000),
                is_market: true,
                tpsl: TeePerpTpsl::Sl,
            }),
            reduce_only: true,
            is_buy: false,
            ..order
        }],
        leverage: None,
    };
    let value = serde_json::to_value(&batch).unwrap();
    assert_eq!(value["kind"], json!("order_batch"));
    assert_eq!(value["grouping"], json!("positionTpsl"));
    assert_eq!(
        value["orders"][0]["orderType"],
        json!({"trigger": {"triggerPx": "90000", "isMarket": true, "tpsl": "sl"}})
    );
    assert!(value.get("leverage").is_none());

    let leverage = PerpPreviewAction::Leverage {
        asset: 3,
        is_cross: false,
        leverage: 4,
    };
    assert_eq!(
        serde_json::to_value(&leverage).unwrap(),
        json!({"kind": "leverage", "asset": 3, "isCross": false, "leverage": 4})
    );
    let round_trip: PerpPreviewAction =
        serde_json::from_value(serde_json::to_value(&batch).unwrap()).unwrap();
    assert_eq!(round_trip, batch);
}

#[test]
fn preview_response_parses_full_payload() {
    let response: PerpPreviewResponse = serde_json::from_value(json!({
        "decision": "rejected",
        "reasonCode": "insufficient_margin",
        "reason": "Rejected: Insufficient margin: required=120.5, equity=100, shortfall=20.5",
        "margin": {
            "equity": "100",
            "initialRequiredBefore": "10",
            "initialRequiredAfter": "120.5",
            "initialAvailableAfter": "-20.5",
            "maintenanceRequiredAfter": "60",
            "shortfall": "20.5"
        },
        "liquidationPxBefore": null,
        "liquidationPxAfter": "81234.5",
        "liquidationBinding": "portfolio_margin",
        "venueLiquidation": {
            "enforced": false,
            "position_initial_margin": "50",
            "compartments": [
                {"underlyings": ["BTC"], "hedge_survives": false, "liquidated_in": "venue", "collateral_loss": "12.5"}
            ]
        },
        "maxSize": {"sz": "0.008", "notional": "800"},
        "notes": ["Liquidation price input unavailable: no venue schedule"]
    }))
    .unwrap();
    assert!(!response.is_ok());
    assert_eq!(
        response.margin.as_ref().unwrap().shortfall,
        Some(dec!(20.5))
    );
    assert_eq!(response.liquidation_px_after, Some(dec!(81234.5)));
    assert_eq!(
        response.liquidation_binding,
        Some(PerpLiquidationBinding::PortfolioMargin)
    );
    let venue = response.venue_liquidation.as_ref().unwrap();
    assert_eq!(
        venue.compartments[0].liquidated_in,
        Some(VenueLiquidationBasis::Venue)
    );
    assert_eq!(response.max_size.as_ref().unwrap().sz, dec!(0.008));
    assert!(response.liquidation_price_unavailable());

    let minimal: PerpPreviewResponse =
        serde_json::from_value(json!({"decision": "ok", "notes": []})).unwrap();
    assert!(minimal.is_ok());
    assert!(minimal.margin.is_none());
}

#[test]
fn markets_response_parses_and_looks_up() {
    let markets: TeePerpMarketsResponse = serde_json::from_value(json!({
        "markets": [
            {"assetId": 0, "symbol": "BTC-PERP", "underlying": "BTC", "hyperliquidCoin": "BTC",
             "dex": null, "supported": true, "reasonCode": null, "missing": null, "message": null},
            {"assetId": 110003, "symbol": "SNDK-PERP", "underlying": "SNDK", "hyperliquidCoin": "xyz:SNDK",
             "dex": "xyz", "supported": false, "reasonCode": "market_not_supported",
             "missing": "price_source", "message": "perp asset 110003 (SNDK-PERP) has no engine spot price for its underlying"}
        ]
    }))
    .unwrap();
    assert!(markets.by_asset_id(0).unwrap().supported);
    let sndk = markets.by_hyperliquid_coin("xyz:SNDK").unwrap();
    assert_eq!(sndk.dex.as_deref(), Some("xyz"));
    assert_eq!(sndk.missing.as_deref(), Some("price_source"));
}

#[test]
fn approval_package_accepts_string_nonce_beyond_json_safe_range() {
    let package: TeeApiWalletApprovalPackage = serde_json::from_value(json!({
        "teeWalletId": "0x2222222222222222222222222222222222222222222222222222222222222222",
        "apiWalletAddress": API_WALLET,
        "apiWalletName": "hypercall-tee",
        "nonce": "117025702092800007",
        "issuedAtMs": 1785672000000_u64,
        "expiresAtMs": 1785672300000_u64,
        "typedData": {"primaryType": "HLAddApiWallet"}
    }))
    .unwrap();
    assert_eq!(package.nonce, 117_025_702_092_800_007);

    let submit = TeeApiWalletApprovalSubmitRequest {
        account: ACCOUNT.parse().unwrap(),
        manager: ACCOUNT.parse().unwrap(),
        timestamp: 1_785_672_000,
        authentication_signature: "0xauth".into(),
        tee_wallet_id: package.tee_wallet_id,
        nonce: package.nonce,
        signature: "0xsig".into(),
    };
    let value = serde_json::to_value(submit).unwrap();
    assert_eq!(value["nonce"], json!("117025702092800007"));
    assert_eq!(
        value["teeWalletId"],
        json!("0x2222222222222222222222222222222222222222222222222222222222222222")
    );
    let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 7);
}

fn batch_modify_response(stages: &[&str]) -> serde_json::Value {
    let children: Vec<_> = stages
        .iter()
        .enumerate()
        .map(|(index, stage)| {
            json!({
                "index": index,
                "requestId": format!("0199e000-0000-7000-8000-0000000000{index:02}"),
                "targetDirectiveId": format!("01999b5a-7c1e-7000-8000-0000000000{:02x}", 0x42 + index),
                "cloid": format!("0x{:032x}", 42 + index),
                "stage": stage,
            })
        })
        .collect();
    json!({
        "stage": "unknown",
        "errorCode": "submission_outcome_unknown",
        "requestId": "0199e000-0000-7000-8000-000000000001",
        "account": ACCOUNT,
        "apiWallet": API_WALLET,
        "engineSeq": "12",
        "venueResponseHash": null,
        "message": null,
        "reasonCode": null,
        "children": children,
    })
}

#[test]
fn batch_modify_response_parses_mixed_child_stages() {
    let response: TeePerpBatchModifyResponse = serde_json::from_value(batch_modify_response(&[
        "accepted",
        "rejected",
        "not_submitted",
        "unknown",
    ]))
    .unwrap();
    assert_eq!(response.engine_seq, Some(12));
    assert_eq!(
        response
            .children
            .iter()
            .map(|child| child.stage)
            .collect::<Vec<_>>(),
        [
            TeePerpOrderStage::Accepted,
            TeePerpOrderStage::Rejected,
            TeePerpOrderStage::NotSubmitted,
            TeePerpOrderStage::Unknown,
        ]
    );
    assert!(response.is_mixed());
    assert_eq!(response.accepted_children().count(), 1);
    assert_eq!(
        response
            .children_to_reconcile()
            .map(|child| child.index)
            .collect::<Vec<_>>(),
        [3]
    );
    assert_eq!(
        response.child(2).unwrap().stage,
        TeePerpOrderStage::NotSubmitted
    );
    assert_eq!(
        response
            .child_for_target(&"01999b5a-7c1e-7000-8000-000000000043".parse().unwrap())
            .unwrap()
            .index,
        1
    );
    assert!(response.child(4).is_none());
}

#[test]
fn an_unknown_child_stage_does_not_hide_its_siblings() {
    // A stage added by a newer server must not make the batch unreadable.
    let response: TeePerpBatchModifyResponse =
        serde_json::from_value(batch_modify_response(&["accepted", "pending_venue_reply"]))
            .unwrap();
    assert_eq!(response.children[0].stage, TeePerpOrderStage::Accepted);
    assert_eq!(response.children[1].stage, TeePerpOrderStage::Unrecognized);
    assert!(!TeePerpOrderStage::Unrecognized.is_definitive());
    assert_eq!(
        response
            .children_to_reconcile()
            .map(|child| child.index)
            .collect::<Vec<_>>(),
        [1]
    );

    let uniform: TeePerpBatchModifyResponse =
        serde_json::from_value(batch_modify_response(&["accepted", "accepted"])).unwrap();
    assert!(!uniform.is_mixed());
    assert_eq!(uniform.children_to_reconcile().count(), 0);
}

#[test]
fn modify_and_twap_actions_match_the_server_wire_shape() {
    let modify = TeePerpModifyAction {
        asset: 0,
        target_directive_id: "01999b5a-7c1e-7000-8000-000000000042".parse().unwrap(),
        is_buy: true,
        limit_px: 10_000_000_000_000,
        sz: 10_000_000,
        reduce_only: false,
        tif: PerpTimeInForce::Gtc,
        cloid: 42,
    };
    let wire = serde_json::to_value(modify).unwrap();
    assert_eq!(
        wire,
        json!({
            "asset": 0,
            "targetDirectiveId": "01999b5a-7c1e-7000-8000-000000000042",
            "isBuy": true,
            "limitPx": 10_000_000_000_000u64,
            "sz": 10_000_000,
            "reduceOnly": false,
            "encodedTif": 2,
            "cloid": 42,
        })
    );
    // The server accepts and emits string integers; so does the SDK.
    let mut stringly = wire.clone();
    stringly["limitPx"] = json!("10000000000000");
    stringly["cloid"] = json!("42");
    assert_eq!(
        serde_json::from_value::<TeePerpModifyAction>(stringly).unwrap(),
        modify
    );
    let mut unknown = wire;
    unknown["oid"] = json!(7);
    assert!(serde_json::from_value::<TeePerpModifyAction>(unknown).is_err());

    let twap = TeePerpTwapResponse {
        stage: TeePerpOrderStage::Accepted,
        error_code: None,
        directive_id: "0199e000-0000-7000-8000-000000000003".into(),
        account: ACCOUNT.parse().unwrap(),
        api_wallet: API_WALLET.parse().unwrap(),
        twap_id: Some(9_007_199_254_740_993),
        engine_seq: None,
        venue_response_hash: None,
        message: None,
        reason_code: None,
    };
    let wire = serde_json::to_value(&twap).unwrap();
    assert_eq!(wire["twapId"], json!("9007199254740993"));
    assert_eq!(
        serde_json::from_value::<TeePerpTwapResponse>(wire).unwrap(),
        twap
    );
    let pending: TeePerpTwapResponse = serde_json::from_value(json!({
        "stage": "not_submitted",
        "errorCode": "trading_unavailable",
        "directiveId": "0199e000-0000-7000-8000-000000000003",
        "account": ACCOUNT,
        "apiWallet": API_WALLET,
        "twapId": null,
    }))
    .unwrap();
    assert_eq!(pending.twap_id, None);
}

#[test]
fn twap_preview_serializes_with_its_kind_tag() {
    let request = PerpPreviewRequest {
        account: ACCOUNT.parse().unwrap(),
        action: PerpPreviewAction::Twap {
            asset: 0,
            is_buy: true,
            sz: dec!(0.5),
            reduce_only: false,
            leverage: None,
        },
    };
    let wire = serde_json::to_value(&request).unwrap();
    assert_eq!(
        wire["action"],
        json!({ "kind": "twap", "asset": 0, "isBuy": true, "sz": "0.5", "reduceOnly": false })
    );
    assert_eq!(
        serde_json::from_value::<PerpPreviewRequest>(wire).unwrap(),
        request
    );
}
