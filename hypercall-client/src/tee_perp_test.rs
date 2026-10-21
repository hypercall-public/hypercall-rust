use super::*;
use crate::test_support::{json_body, read_http_request, serve_once, serve_sequence};
use alloy::primitives::Signature;
use hypercall_sdk_types::{PerpPreviewAction, TeePerpOrderStage};
use rust_decimal_macros::dec;
use std::str::FromStr;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

const PRIVATE_KEY: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const ACCOUNT: &str = "0x00000000000000000000000000000000000000a1";
const API_WALLET: &str = "0x00000000000000000000000000000000000000b2";
const NONCE: u64 = 1_785_672_000_000;

fn account() -> AccountAddress {
    ACCOUNT.parse().unwrap()
}

fn wallet() -> HypercallWallet {
    HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap()
}

fn digest(hex: &str) -> B256 {
    hex.parse().unwrap()
}

fn kat_trigger() -> TeePerpOrderWire {
    tee_perp_trigger_order_wire(
        0,
        Side::Sell,
        dec!(95000),
        dec!(76000),
        dec!(0.01),
        true,
        TeePerpTpsl::Sl,
        true,
        tee_perp_batch_cloid(NONCE, 0).unwrap(),
    )
    .unwrap()
}

fn kat_bracket() -> TeePerpOrderBatchAction {
    build_tee_perp_bracket_batch(
        TeePerpBracketEntry {
            asset: 3,
            side: Side::Buy,
            price: dec!(100000),
            size: dec!(0.01),
            tif: PerpTimeInForce::Gtc,
            reduce_only: false,
        },
        Some(TeePerpExit {
            trigger_price: dec!(110000),
            limit_price: dec!(88000),
        }),
        Some(TeePerpExit {
            trigger_price: dec!(90000),
            limit_price: dec!(72000),
        }),
        NONCE,
    )
    .unwrap()
}

fn recover(signature: &str, digest: B256) -> AccountAddress {
    AccountAddress::from(
        Signature::from_str(signature)
            .unwrap()
            .recover_address_from_prehash(&digest)
            .unwrap(),
    )
}

/// Digests computed independently with viem's `hashTypedData` over the same
/// typed-data definitions and inputs.
#[test]
fn signing_hashes_match_frontend_vectors() {
    let trigger = tee_perp_trigger_order_signing_hash(
        999,
        &TeePerpTriggerOrderSignature {
            account: account(),
            nonce: NONCE,
            order: kat_trigger(),
        },
    )
    .unwrap();
    assert_eq!(
        trigger,
        digest("0x8d66674dbca01100f0a0796718b3e36806f4ed060989511fa90dd15df13d5f5d")
    );

    let batch = kat_bracket();
    let batch_hash = tee_perp_order_batch_signing_hash(
        999,
        &TeePerpOrderBatchSignature {
            account: account(),
            nonce: NONCE,
            batch: &batch,
        },
    )
    .unwrap();
    assert_eq!(
        batch_hash,
        digest("0x5ab3a130019ebf6578451e6982581168da17ce9bac168ecee7365112ab17f951")
    );

    let leverage = tee_perp_leverage_signing_hash(
        999,
        &TeePerpLeverageSignature {
            account: account(),
            nonce: NONCE,
            action: TeePerpLeverageAction {
                asset: 0,
                is_cross: false,
                leverage: 5,
            },
        },
    )
    .unwrap();
    assert_eq!(
        leverage,
        digest("0xe44c44c973583b2c4701ad0d388a293db3abe4f45058eeb882c2bc428a44cba5")
    );

    let approval = tee_api_wallet_approval_signing_hash(
        999,
        &TeeApiWalletApprovalSignature {
            account: account(),
            nonce: 117_025_702_092_800_007,
            api_wallet_address: API_WALLET.parse().unwrap(),
            api_wallet_name: "hypercall-tee",
        },
    )
    .unwrap();
    assert_eq!(
        approval,
        digest("0x680a60d7486c579766782608c78a6fae5c4dbefb782349811bae3cf849295ffc")
    );
}

#[tokio::test]
async fn wallet_signatures_recover_the_signer() {
    let wallet = wallet();
    let trigger_payload = TeePerpTriggerOrderSignature {
        account: account(),
        nonce: NONCE,
        order: kat_trigger(),
    };
    let signature = wallet
        .sign_tee_perp_trigger_order_payload(trigger_payload)
        .await
        .unwrap();
    let hash = tee_perp_trigger_order_signing_hash(999, &trigger_payload).unwrap();
    assert_eq!(recover(&signature, hash), wallet.address);

    let batch = kat_bracket();
    let batch_payload = TeePerpOrderBatchSignature {
        account: account(),
        nonce: NONCE,
        batch: &batch,
    };
    let signature = wallet
        .sign_tee_perp_order_batch_payload(batch_payload)
        .await
        .unwrap();
    let hash = tee_perp_order_batch_signing_hash(999, &batch_payload).unwrap();
    assert_eq!(recover(&signature, hash), wallet.address);

    let approval = TeeApiWalletApprovalSignature {
        account: account(),
        nonce: 7,
        api_wallet_address: API_WALLET.parse().unwrap(),
        api_wallet_name: "hypercall-tee",
    };
    let signature = wallet
        .sign_tee_api_wallet_approval_payload(approval)
        .await
        .unwrap();
    let hash = tee_api_wallet_approval_signing_hash(999, &approval).unwrap();
    assert_eq!(recover(&signature, hash), wallet.address);
}

#[tokio::test]
async fn authentication_message_is_eip191_over_the_hypercall_text() {
    let wallet = wallet();
    let signature = wallet
        .sign_authentication_payload(1_704_067_200)
        .await
        .unwrap();
    let recovered = Signature::from_str(&signature)
        .unwrap()
        .recover_address_from_msg(
            "Sign this message to authenticate for Hypercall\nTimestamp: 1704067200",
        )
        .unwrap();
    assert_eq!(AccountAddress::from(recovered), wallet.address);
}

#[test]
fn a_limit_order_is_not_a_trigger_submission() {
    let limit = tee_perp_limit_order_wire(
        0,
        Side::Buy,
        dec!(100),
        dec!(1),
        PerpTimeInForce::Gtc,
        false,
        1,
    )
    .unwrap();
    assert!(tee_perp_trigger_order_signing_hash(
        999,
        &TeePerpTriggerOrderSignature {
            account: account(),
            nonce: NONCE,
            order: limit,
        }
    )
    .is_err());
}

#[test]
fn market_prices_match_the_order_panel() {
    assert_eq!(
        perp_market_limit_price(dec!(1515.4), Side::Buy, 4, PERP_MARKET_SLIPPAGE_BPS).unwrap(),
        dec!(1818.4)
    );
    assert_eq!(
        perp_market_limit_price(dec!(1514.8), Side::Sell, 4, PERP_MARKET_SLIPPAGE_BPS).unwrap(),
        dec!(1211.9)
    );
    assert_eq!(
        perp_market_limit_price(dec!(67626), Side::Buy, 5, 0).unwrap(),
        dec!(67626)
    );
    assert_eq!(
        perp_market_limit_price(dec!(67626), Side::Sell, 5, 0).unwrap(),
        dec!(67626)
    );
    // szDecimals 5 leaves one price decimal: no tick inside [0.12, 0.15].
    assert!(perp_market_limit_price(dec!(0.15), Side::Sell, 5, PERP_MARKET_SLIPPAGE_BPS).is_err());
    assert!(perp_market_limit_price(dec!(0.15), Side::Buy, 5, PERP_MARKET_SLIPPAGE_BPS).is_err());
    assert!(perp_market_limit_price(dec!(100), Side::Buy, 2, 10_000).is_err());
}

#[test]
fn scale_legs_match_the_order_panel() {
    let spec = PerpScaleOrderSpec {
        start_price: dec!(100),
        end_price: dec!(110),
        total_size: dec!(1),
        order_count: 5,
        size_skew: dec!(1),
        side: Side::Buy,
        size_decimals: 2,
    };
    let legs = build_perp_scale_order_legs(spec).unwrap();
    let expected = [
        (dec!(100), dec!(0.2)),
        (dec!(102.5), dec!(0.2)),
        (dec!(105), dec!(0.2)),
        (dec!(107.5), dec!(0.2)),
        (dec!(110), dec!(0.2)),
    ];
    assert_eq!(
        legs.iter()
            .map(|leg| (leg.price, leg.size))
            .collect::<Vec<_>>(),
        expected
    );

    let skewed = build_perp_scale_order_legs(PerpScaleOrderSpec {
        end_price: dec!(90),
        order_count: 4,
        size_skew: dec!(3),
        side: Side::Sell,
        ..spec
    })
    .unwrap();
    assert_eq!(
        skewed
            .iter()
            .map(|leg| (leg.price, leg.size))
            .collect::<Vec<_>>(),
        [
            (dec!(100), dec!(0.12)),
            (dec!(96.667), dec!(0.2)),
            (dec!(93.334), dec!(0.3)),
            (dec!(90), dec!(0.38)),
        ]
    );

    let inverse = build_perp_scale_order_legs(PerpScaleOrderSpec {
        order_count: 3,
        size_skew: dec!(0.5),
        ..spec
    })
    .unwrap();
    assert_eq!(
        inverse.iter().map(|leg| leg.size).collect::<Vec<_>>(),
        [dec!(0.44), dec!(0.33), dec!(0.23)]
    );

    let fine = PerpScaleOrderSpec {
        start_price: dec!(0.123456),
        end_price: dec!(0.13),
        total_size: dec!(1000),
        order_count: 3,
        size_skew: dec!(1),
        side: Side::Buy,
        size_decimals: 0,
    };
    let buys = build_perp_scale_order_legs(fine).unwrap();
    assert_eq!(
        buys.iter()
            .map(|leg| (leg.price, leg.size))
            .collect::<Vec<_>>(),
        [
            (dec!(0.12345), dec!(333)),
            (dec!(0.12672), dec!(333)),
            (dec!(0.13), dec!(334)),
        ]
    );
    let sells = build_perp_scale_order_legs(PerpScaleOrderSpec {
        side: Side::Sell,
        ..fine
    })
    .unwrap();
    assert_eq!(
        sells.iter().map(|leg| leg.price).collect::<Vec<_>>(),
        [dec!(0.12346), dec!(0.12673), dec!(0.13)]
    );

    for invalid in [
        PerpScaleOrderSpec {
            order_count: 1,
            ..spec
        },
        PerpScaleOrderSpec {
            order_count: 101,
            ..spec
        },
        PerpScaleOrderSpec {
            size_skew: dec!(0),
            ..spec
        },
        PerpScaleOrderSpec {
            size_skew: dec!(100.5),
            ..spec
        },
        PerpScaleOrderSpec {
            end_price: dec!(100),
            ..spec
        },
        PerpScaleOrderSpec {
            total_size: dec!(0.04),
            ..spec
        },
        PerpScaleOrderSpec {
            total_size: dec!(1.001),
            ..spec
        },
    ] {
        assert!(build_perp_scale_order_legs(invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn batch_cloids_match_the_order_panel() {
    assert_eq!(
        tee_perp_batch_cloid(1_788_000_000_000, 0).unwrap(),
        457_728_000_000_001
    );
    assert_eq!(
        tee_perp_batch_cloid(1_788_000_000_000, 254).unwrap(),
        457_728_000_000_255
    );
    assert!(tee_perp_batch_cloid(1, 255).is_err());
    assert!(tee_perp_batch_cloid(0, 0).is_err());
}

#[test]
fn bracket_and_position_tpsl_batches_have_the_panel_shape() {
    let bracket = kat_bracket();
    assert_eq!(bracket.grouping, TeePerpOrderGrouping::NormalTpsl);
    assert_eq!(bracket.orders.len(), 3);
    assert!(bracket.orders[0].is_buy && !bracket.orders[0].reduce_only);
    for (child, tpsl) in bracket.orders[1..]
        .iter()
        .zip([TeePerpTpsl::Tp, TeePerpTpsl::Sl])
    {
        assert!(!child.is_buy && child.reduce_only);
        assert_eq!(child.sz, bracket.orders[0].sz);
        let TeePerpOrderType::Trigger(trigger) = child.order_type else {
            panic!("exit must be a trigger");
        };
        assert!(trigger.is_market);
        assert_eq!(trigger.tpsl, tpsl);
    }

    let position = build_tee_perp_position_tpsl_batch(
        0,
        Side::Sell,
        dec!(0.5),
        None,
        Some(TeePerpExit::market(dec!(105000), Side::Buy, 5, 500).unwrap()),
        NONCE,
    )
    .unwrap();
    assert_eq!(position.grouping, TeePerpOrderGrouping::PositionTpsl);
    assert_eq!(position.orders.len(), 1);
    assert!(position.orders[0].is_buy && position.orders[0].reduce_only);
    assert_eq!(position.orders[0].limit_px, 11_025_000_000_000);
    assert!(build_tee_perp_position_tpsl_batch(0, Side::Buy, dec!(1), None, None, NONCE).is_err());

    let preview = perp_preview_batch_action(&bracket);
    let PerpPreviewAction::OrderBatch {
        orders, grouping, ..
    } = preview
    else {
        panic!("expected a batch preview");
    };
    assert_eq!(grouping, TeePerpOrderGrouping::NormalTpsl);
    assert_eq!(orders[0].limit_px, dec!(100000));
    assert_eq!(orders[0].sz, dec!(0.01));
}

#[test]
fn http_dates_and_clock_offsets_parse() {
    assert_eq!(
        parse_http_date_ms("Sun, 06 Nov 1994 08:49:37 GMT"),
        Some(784_111_777_000)
    );
    assert_eq!(
        parse_http_date_ms("Sat, 01 Aug 2026 12:00:00 GMT"),
        Some(1_785_585_600_000)
    );
    assert_eq!(parse_http_date_ms("Sun, 06 Nov 1994 08:49:37 UTC"), None);
    assert_eq!(parse_http_date_ms("not a date"), None);
    // Server stamped 12:00:00 (+500 ms mid-second) while we measured 1000..1200.
    assert_eq!(
        server_clock_offset_ms("Sat, 01 Aug 2026 12:00:00 GMT", 1_000, 1_200),
        Some(1_785_585_600_000 + 500 - 1_100)
    );
    assert_eq!(
        server_clock_offset_ms("Sat, 01 Aug 2026 12:00:00 GMT", 1_200, 1_000),
        None
    );
}

#[test]
fn nonces_respect_the_server_window_and_increase() {
    assert!(tee_perp_nonce_in_window(NONCE, NONCE));
    assert!(tee_perp_nonce_in_window(NONCE - 8_999, NONCE));
    assert!(!tee_perp_nonce_in_window(NONCE - 9_000, NONCE));
    assert!(tee_perp_nonce_in_window(NONCE + 1_000, NONCE));
    assert!(!tee_perp_nonce_in_window(NONCE + 1_001, NONCE));
    assert!(!tee_perp_nonce_in_window(0, NONCE));
    assert_eq!(tee_perp_venue_expires_after_ms(NONCE), Some(NONCE + 9_000));

    let signer = Address::repeat_byte(0x5a);
    let first = next_nonce_at(signer, NONCE);
    let second = next_nonce_at(signer, NONCE);
    assert_eq!(first, NONCE + 1);
    assert_eq!(second, NONCE + 2);
    assert_eq!(next_nonce_at(signer, NONCE + 100), NONCE + 101);
}

fn order_response() -> String {
    format!(
        r#"{{"stage":"accepted","errorCode":null,"directiveId":"0195f3c4-0000-7000-8000-000000000001","account":"{ACCOUNT}","apiWallet":"{API_WALLET}","cloid":"0x0000000000000000000000000000002a","engineSeq":"7","venueResponseHash":null,"message":null,"reasonCode":null}}"#
    )
}

fn batch_response(grouping: &str) -> String {
    format!(
        r#"{{"stage":"accepted","errorCode":null,"batchId":"0195f3c4-0000-7000-8000-000000000002","account":"{ACCOUNT}","apiWallet":"{API_WALLET}","grouping":"{grouping}","children":[],"engineSeq":8,"venueResponseHash":null,"message":null,"reasonCode":null}}"#
    )
}

fn request_line(request: &str) -> &str {
    request.lines().next().unwrap()
}

#[tokio::test]
async fn limit_order_posts_the_hl_order_schema_to_the_tee_route() {
    let (base_url, request) = serve_once(order_response()).await;
    let client = HypercallClient::new(&base_url);
    let wallet = wallet();
    let response = client
        .place_tee_perp_limit_order(
            &wallet,
            PerpLimitOrderParams {
                account: account(),
                asset: 0,
                side: Side::Buy,
                price: dec!(100000),
                size: dec!(0.01),
                tif: PerpTimeInForce::Alo,
                reduce_only: false,
                client_order_id: None,
                nonce: Some(NONCE),
            },
        )
        .await
        .unwrap();
    assert!(response.is_accepted());
    assert_eq!(response.engine_seq, Some(7));

    let request = request.await.unwrap();
    assert_eq!(
        request_line(&request),
        "POST /v1/tee-perp/orders/submit HTTP/1.1"
    );
    let body = json_body(&request);
    assert_eq!(
        body["action"],
        serde_json::json!({
            "asset": 0, "isBuy": true, "limitPx": 10_000_000_000_000_u64, "sz": 1_000_000,
            "reduceOnly": false, "encodedTif": 1, "cloid": NONCE
        })
    );
    assert_eq!(body["nonce"], serde_json::json!(NONCE));
    let action = HlLimitOrderAction {
        asset: 0,
        is_buy: true,
        limit_px: 10_000_000_000_000,
        sz: 1_000_000,
        reduce_only: false,
        tif: PerpTimeInForce::Alo,
        cloid: u128::from(NONCE),
    };
    let expected = wallet
        .sign_perp_limit_order_payload(PerpLimitOrderSignature {
            account: account(),
            nonce: NONCE,
            action,
        })
        .await
        .unwrap();
    assert_eq!(body["signature"], serde_json::json!(expected));
}

#[tokio::test]
async fn market_order_signs_a_bounded_ioc() {
    let (base_url, request) = serve_once(order_response()).await;
    let client = HypercallClient::new(&base_url);
    client
        .place_tee_perp_market_order(
            &wallet(),
            TeePerpMarketOrderParams {
                account: account(),
                asset: 1,
                side: Side::Sell,
                size: dec!(0.5),
                reference_price: dec!(1514.8),
                size_decimals: 4,
                slippage_bps: None,
                reduce_only: true,
                client_order_id: Some(99),
                nonce: Some(NONCE),
            },
        )
        .await
        .unwrap();
    let body = json_body(&request.await.unwrap());
    assert_eq!(body["action"]["encodedTif"], serde_json::json!(3));
    assert_eq!(
        body["action"]["limitPx"],
        serde_json::json!(121_190_000_000_u64)
    );
    assert_eq!(body["action"]["reduceOnly"], serde_json::json!(true));
    assert_eq!(body["action"]["cloid"], serde_json::json!(99));
}

#[tokio::test]
async fn trigger_order_posts_one_trigger_with_a_recoverable_signature() {
    let (base_url, request) = serve_once(batch_response("na")).await;
    let client = HypercallClient::new(&base_url);
    let wallet = wallet();
    let response = client
        .place_tee_perp_trigger_order(
            &wallet,
            TeePerpTriggerOrderParams {
                account: account(),
                asset: 0,
                side: Side::Sell,
                size: dec!(0.01),
                trigger_price: dec!(95000),
                tpsl: TeePerpTpsl::Sl,
                execution: TeePerpTriggerExecution::Limit { price: dec!(76000) },
                reduce_only: true,
                client_order_id: None,
                nonce: Some(NONCE),
            },
        )
        .await
        .unwrap();
    assert_eq!(response.engine_seq, Some(8));

    let request = request.await.unwrap();
    assert_eq!(
        request_line(&request),
        "POST /v1/tee-perp/orders/submit-trigger HTTP/1.1"
    );
    let body = json_body(&request);
    let order: TeePerpOrderWire = serde_json::from_value(body["action"].clone()).unwrap();
    assert_eq!(order.cloid, tee_perp_batch_cloid(NONCE, 0).unwrap());
    assert_eq!(
        order.order_type,
        TeePerpOrderType::Trigger(TeePerpTriggerOrderType {
            trigger_px: 9_500_000_000_000,
            is_market: false,
            tpsl: TeePerpTpsl::Sl,
        })
    );
    let hash = tee_perp_trigger_order_signing_hash(
        999,
        &TeePerpTriggerOrderSignature {
            account: account(),
            nonce: NONCE,
            order,
        },
    )
    .unwrap();
    assert_eq!(
        recover(body["signature"].as_str().unwrap(), hash),
        wallet.address
    );
}

#[tokio::test]
async fn bracket_scale_and_raw_batches_post_to_submit_batch() {
    let (base_url, requests) =
        serve_sequence(vec![batch_response("normalTpsl"), batch_response("na")]).await;
    let client = HypercallClient::new(&base_url);
    let wallet = wallet();
    client
        .place_tee_perp_bracket_order(
            &wallet,
            TeePerpBracketOrderParams {
                account: account(),
                entry: TeePerpBracketEntry {
                    asset: 3,
                    side: Side::Buy,
                    price: dec!(100000),
                    size: dec!(0.01),
                    tif: PerpTimeInForce::Gtc,
                    reduce_only: false,
                },
                take_profit: Some(TeePerpExit {
                    trigger_price: dec!(110000),
                    limit_price: dec!(88000),
                }),
                stop_loss: Some(TeePerpExit {
                    trigger_price: dec!(90000),
                    limit_price: dec!(72000),
                }),
                nonce: Some(NONCE),
            },
        )
        .await
        .unwrap();
    let legs = build_perp_scale_order_legs(PerpScaleOrderSpec {
        start_price: dec!(100),
        end_price: dec!(110),
        total_size: dec!(1),
        order_count: 5,
        size_skew: dec!(1),
        side: Side::Buy,
        size_decimals: 2,
    })
    .unwrap();
    client
        .place_tee_perp_scale_order(
            &wallet,
            TeePerpScaleOrderParams {
                account: account(),
                asset: 5,
                side: Side::Buy,
                legs,
                tif: PerpTimeInForce::Gtc,
                reduce_only: false,
                nonce: Some(NONCE + 1),
            },
        )
        .await
        .unwrap();

    let requests = requests.await.unwrap();
    for request in &requests {
        assert_eq!(
            request_line(request),
            "POST /v1/tee-perp/orders/submit-batch HTTP/1.1"
        );
    }
    let bracket = json_body(&requests[0]);
    let action: TeePerpOrderBatchAction =
        serde_json::from_value(bracket["action"].clone()).unwrap();
    assert_eq!(action, kat_bracket());
    let hash = tee_perp_order_batch_signing_hash(
        999,
        &TeePerpOrderBatchSignature {
            account: account(),
            nonce: NONCE,
            batch: &action,
        },
    )
    .unwrap();
    assert_eq!(
        hash,
        digest("0x5ab3a130019ebf6578451e6982581168da17ce9bac168ecee7365112ab17f951")
    );
    assert_eq!(
        recover(bracket["signature"].as_str().unwrap(), hash),
        wallet.address
    );

    let scale = json_body(&requests[1]);
    assert_eq!(scale["action"]["grouping"], serde_json::json!("na"));
    assert_eq!(scale["action"]["orders"].as_array().unwrap().len(), 5);

    let duplicate = TeePerpOrderBatchAction {
        orders: vec![kat_trigger(), kat_trigger()],
        grouping: TeePerpOrderGrouping::Na,
    };
    assert!(client
        .place_tee_perp_order_batch(
            &wallet,
            TeePerpOrderBatchParams {
                account: account(),
                nonce: NONCE,
                batch: duplicate,
            },
        )
        .await
        .is_err());
}

#[tokio::test]
async fn cancels_and_leverage_use_their_tee_routes() {
    let cancel_by_oid = format!(
        r#"{{"stage":"accepted","errorCode":null,"requestId":"0195f3c4-0000-8000-8000-000000000004","account":"{ACCOUNT}","apiWallet":"{API_WALLET}","actionKind":"cancel_by_oid","engineSeq":1,"venueResponseHash":null,"message":null}}"#
    );
    let cancel_by_cloid = format!(
        r#"{{"stage":"unknown","errorCode":"submission_outcome_unknown","directiveId":"0195f3c4-0000-7000-8000-000000000005","account":"{ACCOUNT}","apiWallet":"{API_WALLET}","cloid":"0x01","engineSeq":null,"venueResponseHash":null,"message":"timed out"}}"#
    );
    let leverage = format!(
        r#"{{"stage":"accepted","errorCode":null,"requestId":"0195f3c4-0000-8000-8000-000000000006","account":"{ACCOUNT}","apiWallet":"{API_WALLET}","asset":0,"isCross":false,"leverage":5,"engineSeq":2,"venueResponseHash":null,"message":null,"reasonCode":null}}"#
    );
    let (base_url, requests) = serve_sequence(vec![cancel_by_oid, cancel_by_cloid, leverage]).await;
    let client = HypercallClient::new(&base_url);
    let wallet = wallet();

    client
        .cancel_tee_perp_order_by_oid(
            &wallet,
            PerpCancelByOidParams {
                account: account(),
                asset: 0,
                order_id: 123,
                nonce: Some(NONCE),
            },
        )
        .await
        .unwrap();
    let unknown = client
        .cancel_tee_perp_order_by_cloid(
            &wallet,
            PerpCancelByCloidParams {
                account: account(),
                asset: 0,
                client_order_id: 1,
                nonce: Some(NONCE + 1),
            },
        )
        .await
        .unwrap();
    assert_eq!(unknown.stage, TeePerpOrderStage::Unknown);
    let leverage = client
        .update_tee_perp_leverage(
            &wallet,
            TeePerpLeverageParams {
                account: account(),
                asset: 0,
                is_cross: false,
                leverage: 5,
                nonce: Some(NONCE),
            },
        )
        .await
        .unwrap();
    assert!(leverage.is_accepted());

    let requests = requests.await.unwrap();
    assert_eq!(
        request_line(&requests[0]),
        "POST /v1/tee-perp/cancels/submit-by-oid HTTP/1.1"
    );
    assert_eq!(
        json_body(&requests[0])["action"],
        serde_json::json!({"asset": 0, "oid": 123})
    );
    assert_eq!(
        request_line(&requests[1]),
        "POST /v1/tee-perp/cancels/submit HTTP/1.1"
    );
    assert_eq!(
        request_line(&requests[2]),
        "POST /v1/tee-perp/leverage/submit HTTP/1.1"
    );
    let body = json_body(&requests[2]);
    assert_eq!(
        body["action"],
        serde_json::json!({"asset": 0, "isCross": false, "leverage": 5})
    );
    let hash = tee_perp_leverage_signing_hash(
        999,
        &TeePerpLeverageSignature {
            account: account(),
            nonce: NONCE,
            action: TeePerpLeverageAction {
                asset: 0,
                is_cross: false,
                leverage: 5,
            },
        },
    )
    .unwrap();
    assert_eq!(
        recover(body["signature"].as_str().unwrap(), hash),
        wallet.address
    );
}

#[tokio::test]
async fn preview_unwraps_its_envelope_and_markets_parse() {
    let preview = r#"{"success":true,"data":{"decision":"rejected","reasonCode":"trading_halted","reason":"Rejected: trading is halted","margin":null,"liquidationPxBefore":null,"liquidationPxAfter":null,"liquidationBinding":null,"venueLiquidation":null,"maxSize":null,"notes":[]}}"#;
    let failed = r#"{"success":false,"error":"account not found"}"#;
    let markets = r#"{"markets":[{"assetId":0,"symbol":"BTC-PERP","underlying":"BTC","hyperliquidCoin":"BTC","dex":null,"supported":true,"reasonCode":null,"missing":null,"message":null}]}"#;
    let (base_url, requests) =
        serve_sequence(vec![preview.into(), failed.into(), markets.into()]).await;
    let client = HypercallClient::new(&base_url);
    let request = PerpPreviewRequest {
        account: ACCOUNT.parse().unwrap(),
        action: PerpPreviewAction::Leverage {
            asset: 0,
            is_cross: true,
            leverage: 3,
        },
    };

    let response = client.preview_tee_perp_action(&request).await.unwrap();
    assert_eq!(
        response.reason_code,
        Some(hypercall_sdk_types::PerpRejectionReasonCode::TradingHalted)
    );
    let error = client.preview_tee_perp_action(&request).await.unwrap_err();
    assert!(error.to_string().contains("account not found"));
    let markets = client.get_tee_perp_markets().await.unwrap();
    assert!(markets.by_hyperliquid_coin("BTC").unwrap().supported);

    let requests = requests.await.unwrap();
    assert_eq!(
        request_line(&requests[0]),
        "POST /v1/tee-perp/preview HTTP/1.1"
    );
    assert_eq!(
        json_body(&requests[0])["action"],
        serde_json::json!({"kind": "leverage", "asset": 0, "isCross": true, "leverage": 3})
    );
    assert_eq!(
        request_line(&requests[2]),
        "GET /v1/tee-perp/markets HTTP/1.1"
    );
}

/// One-shot server whose response carries a `Date` header.
async fn serve_with_date(
    date: &'static str,
    body: String,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_http_request(&mut stream).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\ndate: {date}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        request
    });
    (format!("http://{address}"), task)
}

#[tokio::test]
async fn server_clock_comes_from_the_health_date_header() {
    let (base_url, request) = serve_with_date("Sat, 01 Aug 2026 12:00:00 GMT", "{}".into()).await;
    let client = HypercallClient::new(&base_url);
    let now = client.server_now_ms().await.unwrap();
    // Within a generous bound of the stamped second (+500 ms mid-second).
    assert!(
        (1_785_585_599_000..=1_785_585_602_000).contains(&now),
        "{now}"
    );
    assert!(request_line(&request.await.unwrap()).starts_with("GET /health "));
    // The offset is cached: no second request is made.
    let again = client.server_now_ms().await.unwrap();
    assert!(again >= now);
}

#[tokio::test]
async fn enrollment_validates_the_typed_data_before_signing() {
    let package = serde_json::json!({
        "teeWalletId": "0x2222222222222222222222222222222222222222222222222222222222222222",
        "apiWalletAddress": API_WALLET,
        "apiWalletName": "hypercall-tee",
        "nonce": "117025702092800007",
        "issuedAtMs": 1_785_672_000_000_u64,
        "expiresAtMs": 1_785_672_300_000_u64,
        "typedData": {
            "domain": {"name": "HypercallManagerSign", "version": "1", "chainId": 999,
                       "verifyingContract": "0x0000000000000000000000000000000000000000"},
            "primaryType": "HLAddApiWallet",
            "message": {"account": ACCOUNT, "nonce": "117025702092800007",
                        "action": {"apiWalletAddress": API_WALLET, "apiWalletName": "hypercall-tee"}}
        }
    });
    let wallet = wallet();
    let challenge = TeeApiWalletApprovalChallenge {
        account: account(),
        manager: wallet.address,
        timestamp: 1_785_672_000,
        authentication_signature: "0xauth".into(),
        package: serde_json::from_value(package).unwrap(),
    };

    let mut tampered = challenge.clone();
    tampered.package.typed_data["message"]["action"]["apiWalletAddress"] =
        serde_json::json!("0x00000000000000000000000000000000000000c3");
    let client = HypercallClient::new("http://127.0.0.1:9");
    let error = client
        .submit_tee_api_wallet_approval(&wallet, &tampered)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("apiWalletAddress"));

    let submitted = format!(
        r#"{{"stage":"enqueued","directiveId":"d-1","actionKey":"hl_add_api_wallet","account":"{ACCOUNT}","nonce":"117025702092800007","recoveredSigner":null,"txHash":null,"rejection":null,"fills":null}}"#
    );
    let (base_url, request) = serve_once(submitted).await;
    let client = HypercallClient::new(&base_url);
    client
        .submit_tee_api_wallet_approval(&wallet, &challenge)
        .await
        .unwrap();
    let request = request.await.unwrap();
    assert_eq!(
        request_line(&request),
        "POST /v1/tee-api-wallet/approval HTTP/1.1"
    );
    let body = json_body(&request);
    assert_eq!(body["nonce"], serde_json::json!("117025702092800007"));
    assert_eq!(body["authenticationSignature"], serde_json::json!("0xauth"));
    let hash = tee_api_wallet_approval_signing_hash(
        999,
        &TeeApiWalletApprovalSignature {
            account: account(),
            nonce: 117_025_702_092_800_007,
            api_wallet_address: API_WALLET.parse().unwrap(),
            api_wallet_name: "hypercall-tee",
        },
    )
    .unwrap();
    assert_eq!(
        recover(body["signature"].as_str().unwrap(), hash),
        wallet.address
    );
}

#[test]
fn approval_progress_and_roles_classify() {
    let status = |domain: DirectiveDomainStatus, delivery: DirectiveDeliveryStatus| {
        DirectiveStatusResponse {
            directive_id: "d".into(),
            action_key: "hl_add_api_wallet".into(),
            domain_status: domain,
            delivery_status: delivery,
            tx_hash: None,
            created_at: None,
        }
    };
    assert_eq!(
        tee_api_wallet_approval_progress(&status(
            DirectiveDomainStatus::Completed,
            DirectiveDeliveryStatus::Finalized
        )),
        TeeApiWalletApprovalProgress::Finalized
    );
    assert_eq!(
        tee_api_wallet_approval_progress(&status(
            DirectiveDomainStatus::Accepted,
            DirectiveDeliveryStatus::CoreRejected
        )),
        TeeApiWalletApprovalProgress::Failed
    );
    assert_eq!(
        tee_api_wallet_approval_progress(&status(
            DirectiveDomainStatus::Accepted,
            DirectiveDeliveryStatus::Submitted
        )),
        TeeApiWalletApprovalProgress::Pending
    );

    let role: HyperliquidUserRole =
        serde_json::from_value(serde_json::json!({"role": "agent", "data": {"user": ACCOUNT}}))
            .unwrap();
    assert!(role.is_agent_for(account()));
    assert!(!role.is_agent_for(API_WALLET.parse().unwrap()));
    let missing: HyperliquidUserRole =
        serde_json::from_value(serde_json::json!({"role": "missing"})).unwrap();
    assert!(!missing.is_agent_for(account()));
}

#[tokio::test]
async fn hyperliquid_leverage_readback_checks_the_echo() {
    let response = format!(
        r#"{{"user":"{ACCOUNT}","coin":"BTC","leverage":{{"type":"isolated","value":5,"rawUsd":"-1"}},"maxTradeSzs":["1","1"],"availableToTrade":["1","1"],"markPx":"100000"}}"#
    );
    let (url, request) = serve_once(response).await;
    let info = HyperliquidInfoClient::new(url);
    let leverage = info.active_asset_leverage(account(), "BTC").await.unwrap();
    assert_eq!(
        leverage,
        HyperliquidLeverage {
            is_cross: false,
            value: 5
        }
    );
    let body = json_body(&request.await.unwrap());
    assert_eq!(
        body,
        serde_json::json!({"type": "activeAssetData", "user": ACCOUNT, "coin": "BTC"})
    );

    let (url, _) = serve_once(format!(
        r#"{{"user":"{ACCOUNT}","coin":"ETH","leverage":{{"type":"cross","value":5}}}}"#
    ))
    .await;
    assert!(HyperliquidInfoClient::new(url)
        .active_asset_leverage(account(), "BTC")
        .await
        .is_err());
}

#[test]
fn builder_address_comes_only_from_exchange_info() {
    let mut info: ExchangeInfoResponse = serde_json::from_value(serde_json::json!({
        "exchange_address": "0x0000000000000000000000000000000000000001",
        "factory_address": "0x0000000000000000000000000000000000000002",
        "deposit_router_address": null,
        "portfolio_margin_deposits_enabled": true,
        "chain_id": 999,
        "signing_domain": {"name": "Hypercall", "version": "1",
                           "verifying_contract": "0x0000000000000000000000000000000000000000"},
        "hl_builder_address": "0x00000000000000000000000000000000000000d4"
    }))
    .unwrap();
    assert_eq!(
        resolve_hypercall_builder_address(&info).unwrap(),
        "0x00000000000000000000000000000000000000d4"
            .parse::<AccountAddress>()
            .unwrap()
    );
    info.hl_builder_address = Some("0x0000000000000000000000000000000000000000".into());
    assert!(resolve_hypercall_builder_address(&info).is_err());
    info.hl_builder_address = None;
    assert!(resolve_hypercall_builder_address(&info).is_err());
}

/// A price past the 12-decimal working scale is refused, not floored: a
/// floored sell could round to a tick below the caller's bound.
#[test]
fn prices_past_the_working_scale_are_refused_not_floored() {
    let past = Decimal::from_str("1000.0000000000001").unwrap();
    assert!(round_perp_price_to_tick(past, Side::Sell, 5).is_err());
    assert!(round_perp_price_to_tick(past, Side::Buy, 5).is_err());
    // Trailing zeros past the scale carry no value and are accepted.
    let zeros = Decimal::from_str("1000.0000000000000").unwrap();
    assert_eq!(
        round_perp_price_to_tick(zeros, Side::Sell, 5).unwrap(),
        dec!(1000)
    );
    // Sells still round up to the next tick, buys down.
    let fine = Decimal::from_str("1000.000000000001").unwrap();
    assert!(round_perp_price_to_tick(fine, Side::Sell, 5).unwrap() > fine);
    assert!(round_perp_price_to_tick(fine, Side::Buy, 5).unwrap() < fine);
    assert!(build_perp_scale_order_legs(PerpScaleOrderSpec {
        side: Side::Sell,
        total_size: dec!(1),
        start_price: past,
        end_price: dec!(1100),
        order_count: 2,
        size_skew: dec!(1),
        size_decimals: 2,
    })
    .is_err());
}
