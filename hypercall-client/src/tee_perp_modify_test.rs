//! Modify, batch modify and TWAP: signing parity with the server and the
//! frontend, request building up to and past the batch cap, and HTTP shape.

use super::*;
use crate::test_support::{json_body, serve_once, serve_once_with_status};
use alloy::primitives::Signature;
use hypercall_sdk_types::{TeePerpErrorCode, TeePerpOrderStage};
use proptest::prelude::*;
use rust_decimal_macros::dec;
use std::str::FromStr;

const PRIVATE_KEY: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
/// Inputs shared with `frontend/src/lib/__tests__/teePerpSigningVectors.test.ts`
/// and the Rust API's `tee_perp_modify_twap_test.rs` /
/// `tee_perp_batch_modify_test.rs`.
const VECTOR_ACCOUNT: &str = "0x1111111111111111111111111111111111111111";
const VECTOR_NONCE: u64 = 1_800_000_000_001;
const VECTOR_BATCH_NONCE: u64 = 1_800_000_000_002;
const API_WALLET: &str = "0x00000000000000000000000000000000000000b2";

fn wallet() -> HypercallWallet {
    HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap()
}

fn vector_account() -> AccountAddress {
    VECTOR_ACCOUNT.parse().unwrap()
}

fn digest(hex: &str) -> B256 {
    hex.parse().unwrap()
}

fn recover(signature: &str, digest: B256) -> AccountAddress {
    AccountAddress::from(
        Signature::from_str(signature)
            .unwrap()
            .recover_address_from_prehash(&digest)
            .unwrap(),
    )
}

fn target(low: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(0x0199_9b5a_7c1e_7000_8000_0000_0000_0000 | low)
}

/// The first vector child, in human units.
fn vector_replacement_0() -> TeePerpOrderReplacement {
    TeePerpOrderReplacement {
        asset: 0,
        target_directive_id: target(0x42),
        cloid: 42,
        side: Side::Buy,
        price: dec!(100000),
        size: dec!(0.1),
        tif: PerpTimeInForce::Gtc,
        reduce_only: false,
    }
}

fn vector_replacement_1() -> TeePerpOrderReplacement {
    TeePerpOrderReplacement {
        asset: 1,
        target_directive_id: target(0x43),
        cloid: 43,
        side: Side::Sell,
        price: dec!(3500),
        size: dec!(2.5),
        tif: PerpTimeInForce::Alo,
        reduce_only: true,
    }
}

const MODIFY_VECTOR: &str = "0x33706da1ce92c746127c13de2a75ee7ec504e7b8ba3004f4ddc5ed4f09413bc6";
const TWAP_VECTOR: &str = "0x66c3df04fb316a5161a1508195a7aa8c997a5c2fa85f1432a804f301790986f2";
const TWAP_CANCEL_VECTOR: &str =
    "0x9b902505768171aa351637b7ff509e930b3f64bdb2d306589bad02e7d922c25d";
const BATCH_MODIFY_VECTOR: &str =
    "0x1d856dcf830bae6bbb26683a113148034520e617530a68c9f79cbce12362fa12";

fn vector_twap() -> TeePerpTwapAction {
    TeePerpTwapAction {
        asset: 0,
        is_buy: true,
        sz: 10_000_000,
        reduce_only: false,
        minutes: 30,
        randomize: true,
    }
}

/// The SDK signs byte-for-byte the digests the Rust API recomputes and the
/// frontend's viem `hashTypedData` produces for the same inputs.
#[test]
fn signing_hashes_match_the_shared_api_and_frontend_vectors() {
    let modify = vector_replacement_0().to_action().unwrap();
    assert_eq!(modify.limit_px, 10_000_000_000_000);
    assert_eq!(modify.sz, 10_000_000);
    assert_eq!(modify.tif.encoded(), 2);
    assert_eq!(
        modify.target_directive_id.to_string(),
        "01999b5a-7c1e-7000-8000-000000000042"
    );
    assert_eq!(
        tee_perp_modify_signing_hash(
            999,
            &TeePerpModifySignature {
                account: vector_account(),
                nonce: VECTOR_NONCE,
                action: modify,
            },
        )
        .unwrap(),
        digest(MODIFY_VECTOR)
    );

    assert_eq!(
        tee_perp_twap_signing_hash(
            999,
            &TeePerpTwapSignature {
                account: vector_account(),
                nonce: VECTOR_NONCE,
                action: vector_twap(),
            },
        )
        .unwrap(),
        digest(TWAP_VECTOR)
    );

    assert_eq!(
        tee_perp_twap_cancel_signing_hash(
            999,
            &TeePerpTwapCancelSignature {
                account: vector_account(),
                nonce: VECTOR_NONCE,
                action: TeePerpTwapCancelAction {
                    asset: 0,
                    twap_id: 77,
                },
            },
        )
        .unwrap(),
        digest(TWAP_CANCEL_VECTOR)
    );

    let batch =
        build_tee_perp_batch_modify(&[vector_replacement_0(), vector_replacement_1()], None)
            .unwrap();
    assert_eq!(batch.modifies[1].limit_px, 350_000_000_000);
    assert_eq!(batch.modifies[1].sz, 250_000_000);
    assert_eq!(batch.modifies[1].tif.encoded(), 1);
    let batch_hash = tee_perp_batch_modify_signing_hash(
        999,
        &TeePerpBatchModifySignature {
            account: vector_account(),
            nonce: VECTOR_BATCH_NONCE,
            batch: &batch,
        },
    )
    .unwrap();
    assert_eq!(batch_hash, digest(BATCH_MODIFY_VECTOR));

    // Child order is signed.
    let reversed =
        build_tee_perp_batch_modify(&[vector_replacement_1(), vector_replacement_0()], None)
            .unwrap();
    assert_ne!(
        tee_perp_batch_modify_signing_hash(
            999,
            &TeePerpBatchModifySignature {
                account: vector_account(),
                nonce: VECTOR_BATCH_NONCE,
                batch: &reversed,
            },
        )
        .unwrap(),
        batch_hash
    );
}

#[tokio::test]
async fn wallet_signatures_over_the_vectors_recover_the_signer() {
    let wallet = wallet();
    let signer = wallet.address();
    let modify = TeePerpModifySignature {
        account: vector_account(),
        nonce: VECTOR_NONCE,
        action: vector_replacement_0().to_action().unwrap(),
    };
    let signature = wallet.sign_tee_perp_modify_payload(modify).await.unwrap();
    assert_eq!(recover(&signature, digest(MODIFY_VECTOR)), signer);

    let batch =
        build_tee_perp_batch_modify(&[vector_replacement_0(), vector_replacement_1()], None)
            .unwrap();
    let signature = wallet
        .sign_tee_perp_batch_modify_payload(TeePerpBatchModifySignature {
            account: vector_account(),
            nonce: VECTOR_BATCH_NONCE,
            batch: &batch,
        })
        .await
        .unwrap();
    assert_eq!(recover(&signature, digest(BATCH_MODIFY_VECTOR)), signer);

    let signature = wallet
        .sign_tee_perp_twap_payload(TeePerpTwapSignature {
            account: vector_account(),
            nonce: VECTOR_NONCE,
            action: vector_twap(),
        })
        .await
        .unwrap();
    assert_eq!(recover(&signature, digest(TWAP_VECTOR)), signer);

    let signature = wallet
        .sign_tee_perp_twap_cancel_payload(TeePerpTwapCancelSignature {
            account: vector_account(),
            nonce: VECTOR_NONCE,
            action: TeePerpTwapCancelAction {
                asset: 0,
                twap_id: 77,
            },
        })
        .await
        .unwrap();
    assert_eq!(recover(&signature, digest(TWAP_CANCEL_VECTOR)), signer);
}

#[test]
fn single_modify_and_twap_terms_are_held_to_the_server_rules() {
    let mut nil = vector_replacement_0();
    nil.target_directive_id = uuid::Uuid::nil();
    assert!(nil.to_action().is_err());
    let mut zero_cloid = vector_replacement_0();
    zero_cloid.cloid = 0;
    assert!(zero_cloid.to_action().is_err());
    let mut zero_size = vector_replacement_0();
    zero_size.size = Decimal::ZERO;
    assert!(zero_size.to_action().is_err());
    let mut fine = vector_replacement_0();
    fine.price = dec!(0.000000001);
    assert!(fine.to_action().is_err(), "more than 8 decimals");

    for (minutes, ok) in [
        (4, false),
        (5, true),
        (1440, true),
        (1441, false),
        (0, false),
    ] {
        let action = TeePerpTwapAction {
            minutes,
            ..vector_twap()
        };
        assert_eq!(
            validate_twap_action(&action).is_ok(),
            ok,
            "minutes {minutes}"
        );
    }
    assert!(validate_twap_action(&TeePerpTwapAction {
        sz: 0,
        ..vector_twap()
    })
    .is_err());
    assert!(tee_perp_twap_cancel_signing_hash(
        999,
        &TeePerpTwapCancelSignature {
            account: vector_account(),
            nonce: VECTOR_NONCE,
            action: TeePerpTwapCancelAction {
                asset: 0,
                twap_id: 0,
            },
        },
    )
    .is_err());
}

fn replacements(count: usize) -> Vec<TeePerpOrderReplacement> {
    (0..count)
        .map(|index| TeePerpOrderReplacement {
            asset: u32::try_from(index % 4).unwrap(),
            target_directive_id: target(index as u128 + 1),
            cloid: index as u128 + 1,
            side: if index % 2 == 0 {
                Side::Buy
            } else {
                Side::Sell
            },
            price: Decimal::from(1_000 + index as u64),
            size: dec!(0.01),
            tif: PerpTimeInForce::Gtc,
            reduce_only: index % 3 == 0,
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Every batch of 1 to the default cap builds, keeps request order,
    /// round-trips its wire form, and signs a digest that depends on order.
    #[test]
    fn batches_up_to_the_default_cap_build_in_order(
        count in 1usize..=DEFAULT_TEE_PERP_BATCH_MODIFY_MAX_CHILDREN,
        nonce in 1u64..=4_000_000_000_000,
    ) {
        let input = replacements(count);
        let batch = build_tee_perp_batch_modify(&input, None).unwrap();
        prop_assert_eq!(batch.modifies.len(), count);
        for (action, replacement) in batch.modifies.iter().zip(&input) {
            prop_assert_eq!(action.target_directive_id, replacement.target_directive_id);
            prop_assert_eq!(action.cloid, replacement.cloid);
        }
        let wire = serde_json::to_value(&batch).unwrap();
        let decoded: TeePerpBatchModifyAction = serde_json::from_value(wire).unwrap();
        prop_assert_eq!(&decoded, &batch);

        let hash = |batch: &TeePerpBatchModifyAction| {
            tee_perp_batch_modify_signing_hash(
                999,
                &TeePerpBatchModifySignature { account: vector_account(), nonce, batch },
            )
            .unwrap()
        };
        prop_assert_eq!(hash(&batch), hash(&decoded));
        if count > 1 {
            let mut reversed = batch.clone();
            reversed.modifies.reverse();
            prop_assert_ne!(hash(&reversed), hash(&batch));
        }
    }

    /// Past the default cap the builder refuses unless the caller names a
    /// higher instance cap, and nothing passes the 100-child wire bound.
    #[test]
    fn batches_over_the_cap_are_refused(
        count in (DEFAULT_TEE_PERP_BATCH_MODIFY_MAX_CHILDREN + 1)..=(MAX_TEE_PERP_BATCH_MODIFY_CHILDREN + 20),
    ) {
        let input = replacements(count);
        prop_assert!(build_tee_perp_batch_modify(&input, None).is_err());
        prop_assert!(
            build_tee_perp_batch_modify(&input, Some(DEFAULT_TEE_PERP_BATCH_MODIFY_MAX_CHILDREN))
                .is_err()
        );
        let at_wire_cap = build_tee_perp_batch_modify(
            &input,
            Some(MAX_TEE_PERP_BATCH_MODIFY_CHILDREN),
        );
        prop_assert_eq!(at_wire_cap.is_ok(), count <= MAX_TEE_PERP_BATCH_MODIFY_CHILDREN);
        prop_assert!(
            build_tee_perp_batch_modify(&input, Some(MAX_TEE_PERP_BATCH_MODIFY_CHILDREN + 1))
                .is_err()
        );
    }

    /// A lowered instance cap is honoured exactly.
    #[test]
    fn a_lowered_cap_is_exact(cap in 1usize..=MAX_TEE_PERP_BATCH_MODIFY_CHILDREN) {
        prop_assert!(build_tee_perp_batch_modify(&replacements(cap), Some(cap)).is_ok());
        prop_assert!(build_tee_perp_batch_modify(&replacements(cap + 1), Some(cap)).is_err());
    }

    /// One repeated target or cloid anywhere in the batch refuses it.
    #[test]
    fn duplicate_targets_or_cloids_are_refused(
        count in 2usize..=DEFAULT_TEE_PERP_BATCH_MODIFY_MAX_CHILDREN,
        pick in any::<prop::sample::Index>(),
        duplicate_cloid in any::<bool>(),
    ) {
        let mut input = replacements(count);
        let from = pick.index(count - 1);
        let to = from + 1;
        if duplicate_cloid {
            input[to].cloid = input[from].cloid;
        } else {
            input[to].target_directive_id = input[from].target_directive_id;
        }
        prop_assert!(build_tee_perp_batch_modify(&input, None).is_err());
    }
}

#[test]
fn empty_batches_and_zero_caps_are_refused() {
    assert!(build_tee_perp_batch_modify(&[], None).is_err());
    assert!(build_tee_perp_batch_modify(&replacements(1), Some(0)).is_err());
    // The signer itself refuses a hand-built batch past the wire bound.
    let oversized = TeePerpBatchModifyAction {
        modifies: replacements(MAX_TEE_PERP_BATCH_MODIFY_CHILDREN + 1)
            .iter()
            .map(|replacement| replacement.to_action().unwrap())
            .collect(),
    };
    assert!(tee_perp_batch_modify_signing_hash(
        999,
        &TeePerpBatchModifySignature {
            account: vector_account(),
            nonce: VECTOR_NONCE,
            batch: &oversized,
        },
    )
    .is_err());
}

fn request_line(request: &str) -> &str {
    request.lines().next().unwrap()
}

#[tokio::test]
async fn batch_modify_posts_the_signed_children_and_returns_typed_child_outcomes() {
    let response = format!(
        r#"{{"stage":"unknown","errorCode":"submission_outcome_unknown","requestId":"0199e000-0000-7000-8000-000000000001","account":"{VECTOR_ACCOUNT}","apiWallet":"{API_WALLET}","engineSeq":"9007199254740993","venueResponseHash":null,"message":"one child timed out","reasonCode":null,"children":[
            {{"index":0,"requestId":"0199e000-0000-7000-8000-000000000010","targetDirectiveId":"01999b5a-7c1e-7000-8000-000000000042","cloid":"0x0000000000000000000000000000002a","stage":"accepted"}},
            {{"index":1,"requestId":"0199e000-0000-7000-8000-000000000011","targetDirectiveId":"01999b5a-7c1e-7000-8000-000000000043","cloid":"0x0000000000000000000000000000002b","stage":"unknown"}}
        ]}}"#
    );
    let (base_url, request) = serve_once(response).await;
    let client = HypercallClient::new(&base_url);
    let wallet = wallet();
    let result = client
        .batch_modify_tee_perp_orders(
            &wallet,
            TeePerpBatchModifyParams {
                account: vector_account(),
                replacements: vec![vector_replacement_0(), vector_replacement_1()],
                max_children: None,
                nonce: Some(VECTOR_BATCH_NONCE),
            },
        )
        .await
        .unwrap();
    assert_eq!(result.engine_seq, Some(9_007_199_254_740_993));
    assert!(result.is_mixed());
    assert!(result.child(0).unwrap().is_accepted());
    assert_eq!(
        result.child_for_target(&target(0x43)).unwrap().stage,
        TeePerpOrderStage::Unknown
    );
    let to_reconcile: Vec<u32> = result
        .children_to_reconcile()
        .map(|child| child.index)
        .collect();
    assert_eq!(to_reconcile, [1]);

    let request = request.await.unwrap();
    assert_eq!(
        request_line(&request),
        "POST /v1/tee-perp/orders/batch-modify HTTP/1.1"
    );
    let body = json_body(&request);
    assert_eq!(body["account"], VECTOR_ACCOUNT);
    assert_eq!(body["nonce"], VECTOR_BATCH_NONCE);
    let children = body["action"]["modifies"].as_array().unwrap();
    assert_eq!(children.len(), 2);
    assert_eq!(
        children[1],
        serde_json::json!({
            "asset": 1,
            "targetDirectiveId": "01999b5a-7c1e-7000-8000-000000000043",
            "isBuy": false,
            "limitPx": 350_000_000_000u64,
            "sz": 250_000_000u64,
            "reduceOnly": true,
            "encodedTif": 1,
            "cloid": 43,
        })
    );
    assert_eq!(
        recover(
            body["signature"].as_str().unwrap(),
            digest(BATCH_MODIFY_VECTOR)
        ),
        wallet.address()
    );
}

#[tokio::test]
async fn an_over_cap_batch_is_refused_before_any_request() {
    // No server: a request would fail with a connection error instead.
    let client = HypercallClient::new("http://127.0.0.1:9");
    let error = client
        .batch_modify_tee_perp_orders(
            &wallet(),
            TeePerpBatchModifyParams {
                account: vector_account(),
                replacements: replacements(DEFAULT_TEE_PERP_BATCH_MODIFY_MAX_CHILDREN + 1),
                max_children: None,
                nonce: Some(VECTOR_BATCH_NONCE),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ClientError::InvalidInput(_)), "{error}");
}

#[tokio::test]
async fn single_modify_posts_to_the_modify_route() {
    let response = format!(
        r#"{{"stage":"rejected","errorCode":"venue_rejected","requestId":"0199e000-0000-7000-8000-000000000002","targetDirectiveId":"01999b5a-7c1e-7000-8000-000000000042","account":"{VECTOR_ACCOUNT}","apiWallet":"{API_WALLET}","cloid":"0x0000000000000000000000000000002a","engineSeq":3,"venueResponseHash":"0x01","message":"Order was never placed","reasonCode":null}}"#
    );
    let (base_url, request) = serve_once(response).await;
    let client = HypercallClient::new(&base_url);
    let wallet = wallet();
    let result = client
        .modify_tee_perp_order(
            &wallet,
            TeePerpModifyParams {
                account: vector_account(),
                replacement: vector_replacement_0(),
                nonce: Some(VECTOR_NONCE),
            },
        )
        .await
        .unwrap();
    assert_eq!(result.stage, TeePerpOrderStage::Rejected);
    assert!(result.stage.is_definitive());

    let request = request.await.unwrap();
    assert_eq!(
        request_line(&request),
        "POST /v1/tee-perp/orders/modify HTTP/1.1"
    );
    let body = json_body(&request);
    assert_eq!(body["action"]["encodedTif"], 2);
    assert_eq!(body["action"]["limitPx"], 10_000_000_000_000u64);
    assert_eq!(
        recover(body["signature"].as_str().unwrap(), digest(MODIFY_VECTOR)),
        wallet.address()
    );
}

#[tokio::test]
async fn twap_submit_and_cancel_use_their_routes() {
    let response = format!(
        r#"{{"stage":"accepted","errorCode":null,"directiveId":"0199e000-0000-7000-8000-000000000003","account":"{VECTOR_ACCOUNT}","apiWallet":"{API_WALLET}","twapId":77,"engineSeq":4,"venueResponseHash":"0x02","message":null,"reasonCode":null}}"#
    );
    let (base_url, request) = serve_once(response).await;
    let client = HypercallClient::new(&base_url);
    let wallet = wallet();
    let twap = client
        .place_tee_perp_twap_order(
            &wallet,
            TeePerpTwapParams {
                account: vector_account(),
                asset: 0,
                side: Side::Buy,
                size: dec!(0.1),
                reduce_only: false,
                minutes: 30,
                randomize: true,
                nonce: Some(VECTOR_NONCE),
            },
        )
        .await
        .unwrap();
    assert_eq!(twap.twap_id, Some(77));
    let request = request.await.unwrap();
    assert_eq!(
        request_line(&request),
        "POST /v1/tee-perp/twap/submit HTTP/1.1"
    );
    let body = json_body(&request);
    assert_eq!(
        body["action"],
        serde_json::json!({
            "asset": 0, "isBuy": true, "sz": 10_000_000u64, "reduceOnly": false,
            "minutes": 30, "randomize": true,
        })
    );
    assert_eq!(
        recover(body["signature"].as_str().unwrap(), digest(TWAP_VECTOR)),
        wallet.address()
    );

    let response = format!(
        r#"{{"stage":"accepted","errorCode":null,"requestId":"0199e000-0000-7000-8000-000000000004","account":"{VECTOR_ACCOUNT}","apiWallet":"{API_WALLET}","actionKind":"twap_cancel","engineSeq":5,"venueResponseHash":null,"message":null}}"#
    );
    let (base_url, request) = serve_once(response).await;
    let client = HypercallClient::new(&base_url);
    let cancel = client
        .cancel_tee_perp_twap(
            &wallet,
            TeePerpTwapCancelParams {
                account: vector_account(),
                asset: 0,
                twap_id: 77,
                nonce: Some(VECTOR_NONCE),
            },
        )
        .await
        .unwrap();
    assert_eq!(cancel.action_kind, "twap_cancel");
    let request = request.await.unwrap();
    assert_eq!(
        request_line(&request),
        "POST /v1/tee-perp/twap/cancel HTTP/1.1"
    );
    let body = json_body(&request);
    assert_eq!(
        body["action"],
        serde_json::json!({ "asset": 0, "twapId": 77 })
    );
    assert_eq!(
        recover(
            body["signature"].as_str().unwrap(),
            digest(TWAP_CANCEL_VECTOR)
        ),
        wallet.address()
    );
}

#[tokio::test]
async fn a_disabled_twap_surfaces_as_unsupported_action() {
    let (base_url, _request) = serve_once_with_status(
        "400 Bad Request",
        r#"{"error":"unsupported_action","code":"unsupported_action","message":"TWAP is not enabled by every admitted TEE release"}"#,
    )
    .await;
    let client = HypercallClient::new(&base_url);
    let error = client
        .place_tee_perp_twap_order(
            &wallet(),
            TeePerpTwapParams {
                account: vector_account(),
                asset: 0,
                side: Side::Sell,
                size: dec!(1),
                reduce_only: true,
                minutes: 60,
                randomize: false,
                nonce: Some(VECTOR_NONCE),
            },
        )
        .await
        .unwrap_err();
    assert!(error.is_unsupported_action(), "{error}");
}

fn twap_params() -> TeePerpTwapParams {
    TeePerpTwapParams {
        account: vector_account(),
        asset: 0,
        side: Side::Buy,
        size: dec!(0.1),
        reduce_only: false,
        minutes: 30,
        randomize: true,
        nonce: None,
    }
}

/// A `5xx` or an undecodable `2xx` may hide an accepted action: the error
/// carries the exact signed bytes, and resending them sends the same bytes
/// (same nonce, same signature), never a freshly signed action.
#[tokio::test]
async fn ambiguous_submissions_return_the_exact_signed_request() {
    let wallet = wallet();
    for (status, body) in [
        ("503 Service Unavailable", "upstream timed out"),
        ("200 OK", "<html>proxy</html>"),
    ] {
        let (base_url, request) = serve_once_with_status(status, body).await;
        let client = HypercallClient::new(&base_url);
        let error = client
            .place_tee_perp_twap_order(
                &wallet,
                TeePerpTwapParams {
                    nonce: Some(VECTOR_NONCE),
                    ..twap_params()
                },
            )
            .await
            .unwrap_err();
        let sent = request.await.unwrap();
        let ClientError::TeeSubmissionAmbiguous {
            route,
            nonce,
            request_json,
            ..
        } = &error
        else {
            panic!("{status}: expected an ambiguous submission, got {error}");
        };
        assert_eq!(route, TEE_PERP_TWAP_SUBMIT_ROUTE);
        assert_eq!(*nonce, VECTOR_NONCE);
        assert!(sent.ends_with(request_json.as_str()), "{status}");

        let accepted = format!(
            r#"{{"stage":"unknown","errorCode":"duplicate_directive","directiveId":"0199e000-0000-7000-8000-000000000003","account":"{VECTOR_ACCOUNT}","apiWallet":"{API_WALLET}","twapId":null,"engineSeq":null}}"#
        );
        let (base_url, resent) = serve_once(accepted).await;
        let response: TeePerpTwapResponse = HypercallClient::new(&base_url)
            .resend_tee_submission(route, request_json)
            .await
            .unwrap();
        assert_eq!(
            response.error_code,
            Some(TeePerpErrorCode::DuplicateDirective)
        );
        let resent = resent.await.unwrap();
        assert_eq!(
            request_line(&resent),
            "POST /v1/tee-perp/twap/submit HTTP/1.1"
        );
        assert!(resent.ends_with(request_json.as_str()));
    }
}

#[tokio::test]
async fn definitive_refusals_are_not_ambiguous() {
    let (base_url, _request) = serve_once_with_status(
        "429 Too Many Requests",
        r#"{"code":"rate_limited","message":"slow down"}"#,
    )
    .await;
    let error = HypercallClient::new(&base_url)
        .place_tee_perp_twap_order(
            &wallet(),
            TeePerpTwapParams {
                nonce: Some(VECTOR_NONCE),
                ..twap_params()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, ClientError::Api { status: 429, .. }),
        "{error}"
    );
}

/// The original request may have executed, so a resend refused as expired
/// (a `4xx` that would be definitive on a first send) stays ambiguous.
#[tokio::test]
async fn refused_resends_stay_ambiguous() {
    let request_json = format!(r#"{{"nonce":"{VECTOR_NONCE}","action":{{}}}}"#);
    let (base_url, _request) = serve_once_with_status(
        "400 Bad Request",
        r#"{"code":"expired","message":"signed action expired"}"#,
    )
    .await;
    let error = HypercallClient::new(&base_url)
        .resend_tee_submission::<TeePerpTwapResponse>(TEE_PERP_TWAP_SUBMIT_ROUTE, &request_json)
        .await
        .unwrap_err();
    let ClientError::TeeSubmissionAmbiguous {
        nonce,
        request_json: echoed,
        ..
    } = &error
    else {
        panic!("expected an ambiguous resend, got {error}");
    };
    assert_eq!(*nonce, VECTOR_NONCE);
    assert_eq!(echoed, &request_json);
}

#[tokio::test]
async fn request_timeouts_are_ambiguous() {
    let (base_url, _request) =
        serve_once_with_status("408 Request Timeout", r#"{"message":"timeout"}"#).await;
    let error = HypercallClient::new(&base_url)
        .place_tee_perp_twap_order(
            &wallet(),
            TeePerpTwapParams {
                nonce: Some(VECTOR_NONCE),
                ..twap_params()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, ClientError::TeeSubmissionAmbiguous { .. }),
        "{error}"
    );
}
