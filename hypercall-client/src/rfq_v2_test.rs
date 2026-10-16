use alloy::primitives::B256;
use hypercall_sdk_types::*;
use serde_json::json;

use super::*;
use crate::test_support::{
    json_body, serve_once_with_status, serve_sequence, serve_status_sequence,
};

#[tokio::test]
async fn rfq_body_encoding_failure_is_definite_before_network_dispatch() {
    struct InvalidBody;

    impl serde::Serialize for InvalidBody {
        fn serialize<S>(&self, _serializer: S) -> std::result::Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(serde::ser::Error::custom("intentional encoding failure"))
        }
    }

    let request = reqwest::Client::new().post("http://127.0.0.1:1/private/rfqs");
    let error = send_rfq_v2_body::<serde_json::Value, _>(request, &InvalidBody, "create RFQ")
        .await
        .unwrap_err();
    assert!(matches!(error, ClientError::Json(_)));
    assert!(!ambiguous_rfq_v2_submission(&error));
}

fn signature_hex() -> String {
    format!("0x{}1b", "11".repeat(64))
}

fn signature() -> RfqV2Signature {
    signature_hex().parse().unwrap()
}

#[tokio::test]
async fn wallet_signs_reusable_private_read_grant_for_bearer_header() {
    let wallet = crate::HypercallWallet::from_private_key(
        "0x59c6995e998f97a5a0044966f094538e5b2f9f8c9360f0c2b3b8a4e1f2f8f96d",
        998,
    )
    .unwrap();
    let account = wallet.address.into_wallet_address();
    let grant = wallet
        .sign_rfq_v2_read_grant(account, 1_900_000_000_000, 1_900_000_030_000, 17)
        .await
        .unwrap();
    assert_eq!(grant.account, account);
    let bearer = encode_rfq_v2_read_grant(&grant).unwrap();
    let decoded =
        base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bearer).unwrap();
    let roundtrip: RfqV2ReadGrant = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(roundtrip, grant);
}

fn read_grant(expires_at_ms: u64) -> RfqV2ReadGrant {
    RfqV2ReadGrant {
        account: "0x00000000000000000000000000000000000000a1"
            .parse()
            .unwrap(),
        issued_at_ms: 1_900_000_000_000,
        expires_at_ms,
        nonce: 17,
        signature: signature(),
    }
}

#[tokio::test]
async fn change_watcher_drains_pages_without_waiting_for_the_next_poll() {
    let rfq_id = Uuid::from_u128(0x5306);
    let responses = vec![
        json!({
            "changes": [{"rfq_id": rfq_id, "sequence": 9}],
            "next_after_sequence": 9,
            "has_more": true
        }),
        json!({
            "changes": [{"rfq_id": rfq_id, "sequence": 12}],
            "next_after_sequence": 12,
            "has_more": false
        }),
    ];
    let (base_url, task) =
        serve_sequence(responses.into_iter().map(|page| page.to_string()).collect()).await;
    let client = HypercallClient::new(base_url);
    let mut pages = client
        .watch_rfq_v2_changes(&read_grant(1_900_000_030_000), 7, Duration::from_secs(30))
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(1), pages.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(Duration::from_secs(1), pages.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        (first.next_after_sequence, second.next_after_sequence),
        (9, 12)
    );
    drop(pages);
    let requests = task.await.unwrap();
    assert!(requests[0].contains("after_sequence=7"));
    assert!(requests[1].contains("after_sequence=9"));
}

#[tokio::test]
async fn change_watcher_stops_on_nonprogressing_cursor_and_rejects_expired_grants() {
    let (base_url, task) = serve_once_with_status(
        "200 OK",
        json!({"changes": [], "next_after_sequence": 7, "has_more": true}).to_string(),
    )
    .await;
    let client = HypercallClient::new(base_url);
    let mut pages = client
        .watch_rfq_v2_changes(&read_grant(1_900_000_030_000), 7, Duration::from_millis(10))
        .unwrap();
    assert!(matches!(
        pages.recv().await.unwrap(),
        Err(ClientError::RfqV2ChangeFeedCursor {
            after_sequence: 7,
            next_after_sequence: 7,
            has_more: true,
        })
    ));
    assert!(pages.recv().await.is_none());
    task.await.unwrap();
    assert!(matches!(
        client.watch_rfq_v2_changes(&read_grant(1), 7, Duration::from_millis(10)),
        Err(ClientError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn change_watcher_rejects_a_change_page_that_does_not_advance_its_cursor() {
    let (base_url, server) = serve_once_with_status(
        "200 OK",
        json!({
            "changes": [{"rfq_id": Uuid::from_u128(0x530d), "sequence": 9}],
            "next_after_sequence": 7,
            "has_more": false
        })
        .to_string(),
    )
    .await;
    let client = HypercallClient::new(base_url);
    let mut pages = client
        .watch_rfq_v2_changes(&read_grant(1_900_000_030_000), 7, Duration::from_secs(30))
        .unwrap();
    assert!(matches!(
        pages.recv().await.unwrap(),
        Err(ClientError::RfqV2ChangeFeedCursor {
            after_sequence: 7,
            next_after_sequence: 7,
            has_more: false,
        })
    ));
    assert!(pages.recv().await.is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn change_watcher_retries_transient_failure_from_the_same_cursor() {
    let rfq_id = Uuid::from_u128(0x5307);
    let (base_url, task) = serve_status_sequence(vec![
        (
            "503 Service Unavailable",
            json!({"code": "temporarily_unavailable", "message": "retry"}).to_string(),
        ),
        (
            "200 OK",
            json!({
                "changes": [{"rfq_id": rfq_id, "sequence": 9}],
                "next_after_sequence": 9,
                "has_more": false
            })
            .to_string(),
        ),
    ])
    .await;
    let client = HypercallClient::new(base_url);
    let mut pages = client
        .watch_rfq_v2_changes(&read_grant(1_900_000_030_000), 7, Duration::from_millis(20))
        .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), pages.recv())
            .await
            .unwrap()
            .unwrap(),
        Err(ClientError::RfqV2Api { status: 503, .. })
    ));
    let recovered = tokio::time::timeout(Duration::from_secs(1), pages.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(recovered.changes[0].rfq_id, rfq_id);
    drop(pages);
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests
        .iter()
        .all(|request| request.contains("after_sequence=7")));
}

#[tokio::test]
async fn change_watcher_closes_at_grant_expiry_without_waiting_for_next_poll() {
    let (base_url, task) = serve_once_with_status(
        "200 OK",
        json!({"changes": [{"rfq_id": Uuid::from_u128(0x530a), "sequence": 9}], "next_after_sequence": 9, "has_more": false}).to_string(),
    )
    .await;
    let client = HypercallClient::new(base_url);
    let now_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let mut pages = client
        .watch_rfq_v2_changes(&read_grant(now_ms + 2_000), 7, Duration::from_secs(30))
        .unwrap();
    assert_eq!(pages.recv().await.unwrap().unwrap().next_after_sequence, 9);
    assert!(tokio::time::timeout(Duration::from_secs(4), pages.recv())
        .await
        .unwrap()
        .is_none());
    task.await.unwrap();
}

#[tokio::test]
async fn change_watcher_skips_noop_polls_and_keeps_reading_for_later_changes() {
    let rfq_id = Uuid::from_u128(0x530b);
    let (base_url, server) = serve_sequence(vec![
        json!({"changes": [], "next_after_sequence": 7, "has_more": false}).to_string(),
        json!({"changes": [], "next_after_sequence": 7, "has_more": false}).to_string(),
        json!({
            "changes": [{"rfq_id": rfq_id, "sequence": 9}],
            "next_after_sequence": 9,
            "has_more": false
        })
        .to_string(),
    ])
    .await;
    let client = HypercallClient::new(base_url);
    let mut pages = client
        .watch_rfq_v2_changes(&read_grant(1_900_000_030_000), 7, Duration::from_millis(20))
        .unwrap();
    let page = tokio::time::timeout(Duration::from_secs(1), pages.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(page.changes[0].rfq_id, rfq_id);
    drop(pages);
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests
        .iter()
        .all(|request| request.contains("after_sequence=7")));
}

#[tokio::test]
async fn change_watcher_does_not_deliver_queued_catchup_after_grant_expiry() {
    let rfq_id = Uuid::from_u128(0x5309);
    let responses = vec![
        json!({
            "changes": [{"rfq_id": rfq_id, "sequence": 9}],
            "next_after_sequence": 9,
            "has_more": true
        }),
        json!({
            "changes": [{"rfq_id": rfq_id, "sequence": 10}],
            "next_after_sequence": 10,
            "has_more": false
        }),
    ];
    let (base_url, server) =
        serve_sequence(responses.into_iter().map(|page| page.to_string()).collect()).await;
    let now_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    let client = HypercallClient::new(base_url);
    let mut pages = client
        .watch_rfq_v2_changes(&read_grant(now_ms + 2_000), 7, Duration::from_secs(30))
        .unwrap();
    let requests = tokio::time::timeout(Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(requests.len(), 2);
    // The one-slot receiver is still full with the first page. A second page
    // may already have been read, but it must not be emitted after expiry.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(pages.recv().await.unwrap().unwrap().next_after_sequence, 9);
    assert!(tokio::time::timeout(Duration::from_secs(1), pages.recv())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn change_watcher_replays_unapplied_page_from_persisted_cursor() {
    let rfq_id = Uuid::from_u128(0x5308);
    let body = json!({
        "changes": [{"rfq_id": rfq_id, "sequence": 9}],
        "next_after_sequence": 9,
        "has_more": false
    })
    .to_string();
    let (base_url, task) = serve_sequence(vec![body.clone(), body]).await;
    let client = HypercallClient::new(base_url);
    let grant = read_grant(1_900_000_030_000);
    let mut first = client
        .watch_rfq_v2_changes(&grant, 7, Duration::from_secs(30))
        .unwrap();
    assert_eq!(first.recv().await.unwrap().unwrap().next_after_sequence, 9);
    // The consumer could not apply the parent read, so it did not persist 9.
    drop(first);
    let mut restarted = client
        .watch_rfq_v2_changes(&grant, 7, Duration::from_secs(30))
        .unwrap();
    assert_eq!(
        restarted.recv().await.unwrap().unwrap().changes[0].rfq_id,
        rfq_id
    );
    drop(restarted);
    let requests = task.await.unwrap();
    assert!(requests
        .iter()
        .all(|request| request.contains("after_sequence=7")));
}

#[tokio::test]
async fn scheduled_quote_cancel_client_posts_signed_deadline() {
    let operation_id = Uuid::from_u128(0x5101);
    let deadline = 1_900_000_030_000_u64;
    let response = serde_json::json!({"operation_id": operation_id, "cancel_at_ms": deadline});
    let (base_url, task) = serve_once_with_status("200 OK", response.to_string()).await;
    let client = HypercallClient::new(base_url);
    let request = RfqV2ScheduledQuoteCancelRequest {
        operation_id,
        maker_account: test_wallet(1),
        cancel_at_ms: Some(deadline),
        expires_at_ms: deadline - 1_000,
        nonce: 77,
        signature: signature(),
    };
    assert_eq!(
        client
            .schedule_rfq_v2_quote_cancel(&request)
            .await
            .unwrap()
            .cancel_at_ms,
        Some(deadline)
    );
    let raw = task.await.unwrap();
    assert!(raw.starts_with("POST /private/rfq-quote-cancel-schedule "));
    assert!(raw.contains("\"cancel_at_ms\":1900000030000"));
}

fn id(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap()
}

fn create_request(rfq_id: Uuid) -> CreateRfqV2Request {
    CreateRfqV2Request {
        terms: RfqV2Terms {
            id: rfq_id,
            taker_account: test_wallet(10),
            legs: vec![RfqV2PackageLeg {
                instrument: "BTC-20261225-100000-C".into(),
                taker_side: RfqV2Side::Buy,
                quantity_micros_per_unit: 1_000_000,
            }],
            target: 5_000_000,
            fill_policy: RfqV2FillPolicy::RepeatedPartial {
                minimum: 1_000_000,
                step: 1_000_000,
            },
            execution_mode: RfqV2ExecutionMode::Manual,
            auto_execution_limit: None,
            quote_policy: RfqV2QuotePolicy::IndicativeAllowed,
            taker_identity: RfqV2TakerIdentity::Anonymous,
            routing: RfqV2Routing::SelectedMakers {
                makers: vec![test_wallet(1), test_wallet(2)],
            },
            quote_deadline_ms: 1_900_000_020_000,
            expires_at_ms: 1_900_000_060_000,
            fee_limits: RfqV2FeeLimits {
                per_execution: 25_000,
                cumulative: 100_000,
            },
            builder: None,
            private_terms_blinding: B256::repeat_byte(0x8b).into(),
        },
        nonce: 42,
        signature: signature(),
    }
}

fn indicative_request(rfq_id: Uuid, quote_id: Uuid) -> PublishRfqV2IndicativeQuoteRequest {
    PublishRfqV2IndicativeQuoteRequest {
        terms: RfqV2IndicativeQuoteTerms {
            id: quote_id,
            rfq_id,
            rfq_terms_commitment: B256::repeat_byte(0x4a).into(),
            maker_account: test_wallet(1),
            priced_legs: vec![RfqV2PricedLeg {
                instrument: "BTC-20261225-100000-C".into(),
                price_micros: 825_000_000,
            }],
            reference_quantity: 1_000_000,
            maker_receives_reference_premium: true,
            reference_premium_micros: 825_000_000,
            indicated_capacity: 5_000_000,
            expires_at_ms: 1_900_000_015_000,
        },
        signature: signature(),
    }
}

fn firm_request(rfq_id: Uuid, quote_id: Uuid) -> PublishRfqV2FirmQuoteRequest {
    PublishRfqV2FirmQuoteRequest {
        terms: RfqV2FirmQuoteTerms {
            id: quote_id,
            rfq_id,
            rfq_terms_commitment: B256::repeat_byte(0x4a).into(),
            maker_account: test_wallet(1),
            maker_generation: 0,
            rfq_generation: 0,
            priced_legs: vec![RfqV2PricedLeg {
                instrument: "BTC-20261225-100000-C".into(),
                price_micros: 830_000_000,
            }],
            reference_quantity: 1_000_000,
            maker_receives_reference_premium: true,
            reference_premium_micros: 830_000_000,
            capacity: 5_000_000,
            fill_policy: RfqV2FillPolicy::RepeatedPartial {
                minimum: 1_000_000,
                step: 1_000_000,
            },
            withdrawal_policy: RfqV2WithdrawalPolicy::Permitted,
            fee_limits: RfqV2FeeLimits {
                per_execution: 25_000,
                cumulative: 100_000,
            },
            expires_at_ms: 1_900_000_045_000,
            protection_scope: None,
            source_indicative_quote_id: None,
        },
        nonce: 91,
        signature: signature(),
    }
}

fn create_response(rfq_id: Uuid) -> String {
    json!({
        "rfq_id": rfq_id,
        "status": "open",
        "terms_commitment": format!("{:#x}", B256::repeat_byte(0x4a)),
        "quote_deadline_ms": 1_900_000_020_000_u64,
        "expires_at_ms": 1_900_000_060_000_u64
    })
    .to_string()
}

fn quote_response(quote_id: Uuid, firmness: &str, executable: bool) -> String {
    let mut response = json!({
        "quote_id": quote_id,
        "firmness": firmness,
        "status": "open",
        "executable": executable,
    });
    if executable {
        response["remaining_capacity"] = json!("5000000");
        response["preflight"] = json!({
            "status": "passed",
            "snapshot_revision": 12,
            "checked_at_ms": 1_900_000_000_000_u64
        });
    } else {
        response["indicated_capacity"] = json!("5000000");
    }
    response.to_string()
}

fn operation_response(operation_id: Uuid) -> String {
    json!({"operation_id": operation_id, "status": "accepted"}).to_string()
}

fn execution_response(operation_id: Uuid, quote_id: Uuid) -> String {
    json!({
        "operation_id": operation_id,
        "status": "filled",
        "executions": [{
            "execution_id": "ed954878-ce46-4182-9b47-260058e2f55b",
            "quote_id": quote_id,
            "maker_account": test_wallet(1),
            "maker_provider": test_wallet(1),
            "maker_signer": test_wallet(1),
            "package_units": "2000000",
            "fee_micros": "25000"
        }],
        "rfq_remaining_target": "3000000",
        "quote_remaining_capacity": "3000000"
    })
    .to_string()
}

/// A parent policy that admits any non-zero size, so a test isolates the
/// quote's own constraints.
const ANY_PARENT_SIZE: RfqV2FillPolicy = RfqV2FillPolicy::RepeatedPartial {
    minimum: 1,
    step: 1,
};

fn firm_view(
    rfq_id: Uuid,
    quote_id: Uuid,
    premium: u128,
    reference_quantity: u128,
    remaining_capacity: u128,
    maker_receives: bool,
) -> RfqV2QuoteView {
    let mut terms = firm_request(rfq_id, quote_id).terms;
    terms.reference_premium_micros = premium;
    terms.reference_quantity = reference_quantity;
    terms.maker_receives_reference_premium = maker_receives;
    RfqV2QuoteView::Firm {
        provider: terms.maker_account,
        signer: terms.maker_account,
        terms,
        status: "open".into(),
        executable: true,
        remaining_capacity,
        preflight: Some(RfqV2PreflightResult {
            status: RfqV2PreflightStatus::Passed,
            snapshot_revision: Some(12),
            checked_at_ms: 1_900_000_000_000,
        }),
    }
}

#[tokio::test]
async fn client_covers_the_complete_proposed_route_inventory() {
    let rfq_id = id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce");
    let indicative_id = id("169a869d-9fa7-490b-b68e-d78005740d4c");
    let firm_id = id("6889f9c6-cc85-40d9-86bb-df012ea74e2a");
    let operation_id = id("0b762028-6074-49ca-8881-e0f20362dd28");
    let replacement_id = id("1378a869-9fa7-490b-b68e-d78005740d4c");
    let parent_terms = create_request(rfq_id).terms;
    let read_response = json!({
        "rfq_id": rfq_id,
        "status": "partially_filled",
        "terms_commitment": format!("{:#x}", B256::repeat_byte(0x4a)),
        "parent": {"view": "taker", "terms": parent_terms},
        "target": "5000000",
        "remaining_target": "3000000",
        "quote_policy": "indicative_allowed",
        "quotes": []
    })
    .to_string();
    let terms = indicative_request(rfq_id, indicative_id).terms;
    let quote_view = RfqV2QuoteView::Indicative {
        provider: terms.maker_account,
        signer: terms.maker_account,
        terms,
        status: "open".into(),
        executable: false,
    };
    let rfq_list_response = json!({
        "rfqs": [],
        "page": {"limit": 50, "next_cursor": null, "has_more": false},
        "as_of_sequence": 812044
    })
    .to_string();
    let quote_list_response = serde_json::to_string(&ListRfqV2QuotesResponse {
        quotes: vec![quote_view.clone()],
        page: CursorPage {
            limit: 50,
            next_cursor: None,
            has_more: false,
        },
        as_of_sequence: 812_044,
    })
    .unwrap();
    let change_list_response = serde_json::to_string(&ListRfqV2ChangesResponse {
        changes: vec![RfqV2ChangeHint {
            rfq_id,
            sequence: 812_045,
        }],
        next_after_sequence: 812_045,
        has_more: false,
    })
    .unwrap();
    let quote_read_response = serde_json::to_string(&quote_view).unwrap();
    let responses = vec![
        create_response(rfq_id),
        quote_response(indicative_id, "indicative", false),
        quote_response(firm_id, "firm", true),
        operation_response(operation_id),
        execution_response(operation_id, firm_id),
        execution_response(operation_id, firm_id),
        operation_response(operation_id),
        operation_response(operation_id),
        operation_response(operation_id),
        create_response(replacement_id),
        quote_response(replacement_id, "indicative", false),
        read_response,
        rfq_list_response,
        change_list_response,
        quote_list_response,
        quote_read_response,
    ];
    let (base_url, request_task) = serve_sequence(responses).await;
    let client = HypercallClient::new(base_url);
    let create = create_request(rfq_id);
    let indicative = indicative_request(rfq_id, indicative_id);
    let firm = firm_request(rfq_id, firm_id);
    let invalidation = RfqV2InvalidationRequest {
        operation_id,
        expires_at_ms: 1_900_000_030_000,
        nonce: 44,
        signature: signature(),
    };

    client.create_rfq_v2(&create).await.unwrap();
    client
        .publish_rfq_v2_indicative_quote(rfq_id, &indicative)
        .await
        .unwrap();
    client
        .publish_rfq_v2_firm_quote(rfq_id, &firm)
        .await
        .unwrap();
    client
        .request_rfq_v2_firm_quote(
            rfq_id,
            indicative_id,
            &RequestRfqV2FirmQuoteRequest {
                operation_id,
                requested_units: 2_000_000,
                expires_at_ms: 1_900_000_012_000,
                nonce: 43,
                signature: signature(),
            },
        )
        .await
        .unwrap();
    client
        .execute_rfq_v2(
            rfq_id,
            &RfqV2ExecutionRequest {
                operation_id,
                quote_id: firm_id,
                package_units: 2_000_000,
                max_fee_micros: 25_000,
                expires_at_ms: 1_900_000_030_000,
                nonce: 44,
                signature: signature(),
            },
        )
        .await
        .unwrap();
    client
        .sweep_rfq_v2(
            rfq_id,
            &RfqV2SweepRequest {
                operation_id,
                allocations: vec![RfqV2SweepAllocation {
                    quote_id: firm_id,
                    package_units: 2_000_000,
                }],
                max_fee_micros: 25_000,
                expires_at_ms: 1_900_000_030_000,
                nonce: 45,
                signature: signature(),
            },
        )
        .await
        .unwrap();
    client.cancel_rfq_v2(rfq_id, &invalidation).await.unwrap();
    client
        .withdraw_rfq_v2_quote(rfq_id, firm_id, &invalidation)
        .await
        .unwrap();
    client
        .withdraw_rfq_v2_quotes(&RfqV2BulkWithdrawalRequest {
            operation_id,
            maker_account: test_wallet(1),
            scope: RfqV2BulkWithdrawalScope::Rfq { rfq_id },
            expires_at_ms: 1_900_000_030_000,
            nonce: 48,
            signature: signature(),
        })
        .await
        .unwrap();
    client
        .replace_rfq_v2(
            rfq_id,
            &RfqV2ReplacementRequest {
                operation_id,
                replacement: create_request(replacement_id),
                expires_at_ms: 1_900_000_030_000,
                nonce: 46,
                signature: signature(),
            },
        )
        .await
        .unwrap();
    client
        .replace_rfq_v2_quote(
            rfq_id,
            indicative_id,
            &RfqV2ReplacementRequest {
                operation_id,
                replacement: RfqV2QuoteReplacement::Indicative(indicative_request(
                    rfq_id,
                    replacement_id,
                )),
                expires_at_ms: 1_900_000_030_000,
                nonce: 47,
                signature: signature(),
            },
        )
        .await
        .unwrap();
    client
        .get_rfq_v2(
            rfq_id,
            RfqV2ReadCredentials {
                bearer_token: "test-session",
            },
        )
        .await
        .unwrap();
    client
        .list_rfqs_v2(
            &ListRfqV2Query {
                limit: Some(50),
                role: Some(RfqV2Role::Taker),
                status: Some("open".into()),
                ..ListRfqV2Query::default()
            },
            RfqV2ReadCredentials {
                bearer_token: "test-session",
            },
        )
        .await
        .unwrap();
    let changes = client
        .list_rfq_v2_changes(
            &ListRfqV2ChangesQuery {
                after_sequence: Some(812_044),
                limit: Some(50),
            },
            RfqV2ReadCredentials {
                bearer_token: "test-session",
            },
        )
        .await
        .unwrap();
    assert_eq!(changes.changes[0].sequence, 812_045);
    client
        .list_rfq_v2_quotes(
            rfq_id,
            &ListRfqV2QuotesQuery {
                limit: Some(50),
                after_sequence: Some(812_000),
                include_terminal: Some(true),
                ..ListRfqV2QuotesQuery::default()
            },
            RfqV2ReadCredentials {
                bearer_token: "test-session",
            },
        )
        .await
        .unwrap();
    client
        .get_rfq_v2_quote(
            rfq_id,
            indicative_id,
            RfqV2ReadCredentials {
                bearer_token: "test-session",
            },
        )
        .await
        .unwrap();

    let requests = request_task.await.unwrap();
    let expected = [
        "POST /private/rfqs HTTP/1.1".to_string(),
        format!("POST /private/rfqs/{rfq_id}/quotes/indicative HTTP/1.1"),
        format!("POST /private/rfqs/{rfq_id}/quotes/firm HTTP/1.1"),
        format!("POST /private/rfqs/{rfq_id}/quotes/{indicative_id}/firm-up-requests HTTP/1.1"),
        format!("POST /private/rfqs/{rfq_id}/executions HTTP/1.1"),
        format!("POST /private/rfqs/{rfq_id}/sweeps HTTP/1.1"),
        format!("POST /private/rfqs/{rfq_id}/cancellations HTTP/1.1"),
        format!("POST /private/rfqs/{rfq_id}/quotes/{firm_id}/withdrawals HTTP/1.1"),
        "POST /private/rfq-quote-withdrawals HTTP/1.1".to_string(),
        format!("POST /private/rfqs/{rfq_id}/replacements HTTP/1.1"),
        format!("POST /private/rfqs/{rfq_id}/quotes/{indicative_id}/replacements HTTP/1.1"),
        format!("GET /private/rfqs/{rfq_id}?view=authorized HTTP/1.1"),
        "GET /private/rfqs?limit=50&role=taker&status=open HTTP/1.1".to_string(),
        "GET /private/rfqs/changes?after_sequence=812044&limit=50 HTTP/1.1".to_string(),
        format!(
            "GET /private/rfqs/{rfq_id}/quotes?limit=50&after_sequence=812000&include_terminal=true HTTP/1.1"
        ),
        format!("GET /private/rfqs/{rfq_id}/quotes/{indicative_id} HTTP/1.1"),
    ];
    for (request, expected_line) in requests.iter().zip(expected) {
        assert!(request.starts_with(&expected_line), "{request}");
    }
    for request in &requests[11..] {
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer test-session\r\n"),
            "private read omitted its bearer credential: {request}"
        );
    }
    assert_eq!(json_body(&requests[0])["terms"]["id"], rfq_id.to_string());
    assert_eq!(
        json_body(&requests[5])["allocations"][0]["package_units"],
        "2000000"
    );
}

#[tokio::test]
async fn typed_not_enabled_response_is_not_treated_as_success() {
    let (base_url, request_task) = serve_once_with_status(
        "501 Not Implemented",
        json!({
            "code": "rfq_v2_not_enabled",
            "message": "RFQ v2 is not enabled"
        })
        .to_string(),
    )
    .await;
    let client = HypercallClient::new(base_url);
    let error = client
        .create_rfq_v2(&create_request(id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce")))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        crate::ClientError::RfqV2Api {
            status: 501,
            code,
            ..
        } if code == "rfq_v2_not_enabled"
    ));
    assert!(request_task
        .await
        .unwrap()
        .starts_with("POST /private/rfqs HTTP/1.1"));
}

#[tokio::test]
async fn execution_server_failure_keeps_reconciliation_ids() {
    let (base_url, request_task) = serve_once_with_status(
        "503 Service Unavailable",
        json!({"code":"AUTHORITY_UNAVAILABLE","message":"retry later"}).to_string(),
    )
    .await;
    let client = HypercallClient::new(base_url);
    let rfq_id = id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce");
    let operation_id = id("0b762028-6074-49ca-8881-e0f20362dd28");
    let request = RfqV2ExecutionRequest {
        operation_id,
        quote_id: id("6889f9c6-cc85-40d9-86bb-df012ea74e2a"),
        package_units: 1_000_000,
        max_fee_micros: 25_000,
        expires_at_ms: 1_900_000_030_000,
        nonce: 43,
        signature: signature(),
    };
    let error = client.execute_rfq_v2(rfq_id, &request).await.unwrap_err();
    assert!(
        matches!(error, crate::ClientError::RfqV2SubmissionAmbiguous {
        rfq_id: actual_rfq,
        operation_id: actual_operation,
        source,
    } if actual_rfq == Some(rfq_id) && actual_operation == operation_id
        && matches!(source.as_ref(), crate::ClientError::RfqV2Api {status: 503, code, ..} if code == "AUTHORITY_UNAVAILABLE"))
    );
    request_task.await.unwrap();
}

#[test]
fn best_quote_selection_normalizes_price_and_filters_ineligible_quotes() {
    let rfq_id = id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce");
    let worse_id = id("6889f9c6-cc85-40d9-86bb-df012ea74e2a");
    let best_id = id("169a869d-9fa7-490b-b68e-d78005740d4c");
    let too_small_id = id("1378a869-9fa7-490b-b68e-d78005740d4c");
    let quotes = vec![
        firm_view(rfq_id, worse_id, 500, 1, 5_000_000, true),
        firm_view(rfq_id, best_id, 900, 2, 5_000_000, true),
        firm_view(rfq_id, too_small_id, 1, 1, 500_000, true),
    ];
    let selected = select_best_rfq_v2_quote(
        &quotes,
        RfqV2BestQuoteCriteria {
            rfq_id,
            terms_commitment: B256::repeat_byte(0x4a).into(),
            requested_units: 1_000_000,
            parent_remaining_target: u128::MAX,
            parent_target: u128::MAX,
            parent_fill_policy: ANY_PARENT_SIZE,
            parent_unit_grid: RfqV2ParentUnitGrid::UNBOUNDED,
            parent_legs: &[],
            now_ms: 1_900_000_000_000,
            auto_execution_limit: None,
        },
    )
    .unwrap();

    let RfqV2QuoteView::Firm { terms, .. } = selected else {
        panic!("selector returned an indicative quote");
    };
    assert_eq!(terms.id, best_id);
}

#[test]
fn best_quote_selection_uses_stable_tie_breaks_and_credit_direction() {
    let rfq_id = id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce");
    let lower_id = id("169a869d-9fa7-490b-b68e-d78005740d4c");
    let higher_id = id("6889f9c6-cc85-40d9-86bb-df012ea74e2a");
    let criteria = RfqV2BestQuoteCriteria {
        rfq_id,
        terms_commitment: B256::repeat_byte(0x4a).into(),
        requested_units: 1_000_000,
        parent_remaining_target: u128::MAX,
        parent_target: u128::MAX,
        parent_fill_policy: ANY_PARENT_SIZE,
        parent_unit_grid: RfqV2ParentUnitGrid::UNBOUNDED,
        parent_legs: &[],
        now_ms: 1_900_000_000_000,
        auto_execution_limit: None,
    };
    let tied = vec![
        firm_view(rfq_id, higher_id, 500, 1, 5_000_000, false),
        firm_view(rfq_id, lower_id, 1_000, 2, 5_000_000, false),
    ];
    let RfqV2QuoteView::Firm { terms, .. } = select_best_rfq_v2_quote(&tied, criteria).unwrap()
    else {
        panic!("selector returned an indicative quote");
    };
    assert_eq!(terms.id, lower_id);

    let credits = vec![
        firm_view(rfq_id, lower_id, 500, 1, 5_000_000, false),
        firm_view(rfq_id, higher_id, 1_200, 2, 5_000_000, false),
    ];
    let RfqV2QuoteView::Firm { terms, .. } = select_best_rfq_v2_quote(&credits, criteria).unwrap()
    else {
        panic!("selector returned an indicative quote");
    };
    assert_eq!(terms.id, higher_id);
}

#[test]
fn best_quote_selection_compares_mixed_directions_and_enforces_auto_limit() {
    let rfq_id = id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce");
    let debit_id = id("169a869d-9fa7-490b-b68e-d78005740d4c");
    let credit_id = id("6889f9c6-cc85-40d9-86bb-df012ea74e2a");
    let quotes = vec![
        firm_view(rfq_id, debit_id, 100, 1_000_000, 5_000_000, true),
        firm_view(rfq_id, credit_id, 100, 1_000_000, 5_000_000, false),
    ];
    let criteria = RfqV2BestQuoteCriteria {
        rfq_id,
        terms_commitment: B256::repeat_byte(0x4a).into(),
        requested_units: 1_000_000,
        parent_remaining_target: u128::MAX,
        parent_target: u128::MAX,
        parent_fill_policy: ANY_PARENT_SIZE,
        parent_unit_grid: RfqV2ParentUnitGrid::UNBOUNDED,
        parent_legs: &[],
        now_ms: 1_900_000_000_000,
        auto_execution_limit: None,
    };
    let RfqV2QuoteView::Firm { terms, .. } = select_best_rfq_v2_quote(&quotes, criteria).unwrap()
    else {
        panic!("expected firm quote")
    };
    assert_eq!(terms.id, credit_id);

    let too_strict = RfqV2BestQuoteCriteria {
        auto_execution_limit: Some(RfqV2AutoExecutionLimit {
            maker_receives_premium: false,
            premium_micros: 200,
        }),
        ..criteria
    };
    assert_eq!(
        select_best_rfq_v2_quote(&quotes, too_strict),
        Err(RfqV2BestQuoteError::NoEligibleQuote)
    );
}

#[test]
fn one_smaller_package_is_not_selectable_after_a_partial_fill() {
    let rfq_id = id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce");
    let quote_id = id("6889f9c6-cc85-40d9-86bb-df012ea74e2a");
    let mut quote = firm_view(rfq_id, quote_id, 100, 1_000_000, 4_000_000, true);
    if let RfqV2QuoteView::Firm { terms, status, .. } = &mut quote {
        terms.fill_policy = RfqV2FillPolicy::OneSmallerPackage {
            minimum: 1_000_000,
            step: 1_000_000,
        };
        *status = "partially_filled".into();
    }
    assert_eq!(
        select_best_rfq_v2_quote(
            &[quote],
            RfqV2BestQuoteCriteria {
                rfq_id,
                terms_commitment: B256::repeat_byte(0x4a).into(),
                requested_units: 1_000_000,
                parent_remaining_target: u128::MAX,
                parent_target: u128::MAX,
                parent_fill_policy: ANY_PARENT_SIZE,
                parent_unit_grid: RfqV2ParentUnitGrid::UNBOUNDED,
                parent_legs: &[],
                now_ms: 1_900_000_000_000,
                auto_execution_limit: None,
            }
        ),
        Err(RfqV2BestQuoteError::NoEligibleQuote)
    );
}

#[test]
fn selector_skips_open_quote_when_parent_is_not_executable() {
    let rfq_id = id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce");
    let quote_id = id("6889f9c6-cc85-40d9-86bb-df012ea74e2a");
    let mut quote = firm_view(rfq_id, quote_id, 100, 1_000_000, 5_000_000, true);
    if let RfqV2QuoteView::Firm { executable, .. } = &mut quote {
        *executable = false;
    }
    assert_eq!(
        select_best_rfq_v2_quote(
            &[quote],
            RfqV2BestQuoteCriteria {
                rfq_id,
                terms_commitment: B256::repeat_byte(0x4a).into(),
                requested_units: 1_000_000,
                parent_remaining_target: u128::MAX,
                parent_target: u128::MAX,
                parent_fill_policy: ANY_PARENT_SIZE,
                parent_unit_grid: RfqV2ParentUnitGrid::UNBOUNDED,
                parent_legs: &[],
                now_ms: 1_900_000_000_000,
                auto_execution_limit: None,
            }
        ),
        Err(RfqV2BestQuoteError::NoEligibleQuote)
    );
}

/// Another quote can partially fill the parent. A quote whose own capacity
/// covers the request is still not eligible when the request exceeds the
/// parent's remaining target, because the execution would be refused.
#[test]
fn best_quote_selection_respects_the_parent_remaining_target() {
    let rfq_id = id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce");
    let quote_id = id("6889f9c6-cc85-40d9-86bb-df012ea74e2a");
    let quotes = vec![firm_view(
        rfq_id, quote_id, 100, 1_000_000, 5_000_000, false,
    )];
    let criteria = |parent_remaining_target| RfqV2BestQuoteCriteria {
        rfq_id,
        terms_commitment: B256::repeat_byte(0x4a).into(),
        requested_units: 2_000_000,
        parent_remaining_target,
        parent_target: u128::MAX,
        parent_fill_policy: ANY_PARENT_SIZE,
        parent_unit_grid: RfqV2ParentUnitGrid::UNBOUNDED,
        parent_legs: &[],
        now_ms: 1_900_000_000_000,
        auto_execution_limit: None,
    };
    assert!(select_best_rfq_v2_quote(&quotes, criteria(2_000_000)).is_ok());
    assert_eq!(
        select_best_rfq_v2_quote(&quotes, criteria(1_999_999)),
        Err(RfqV2BestQuoteError::NoEligibleQuote)
    );
}

/// Every fill must satisfy the parent's signed fill policy as well as the
/// quote's: a full-size-once parent targeting 10 refuses 5 even when a
/// repeated-partial quote admits it.
#[test]
fn best_quote_selection_applies_the_parent_fill_policy() {
    let rfq_id = id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce");
    let quote_id = id("6889f9c6-cc85-40d9-86bb-df012ea74e2a");
    let mut quote = firm_view(rfq_id, quote_id, 100, 1, 10, false);
    if let RfqV2QuoteView::Firm { terms, .. } = &mut quote {
        terms.capacity = 10;
        terms.fill_policy = RfqV2FillPolicy::RepeatedPartial {
            minimum: 1,
            step: 1,
        };
    }
    let quotes = [quote];
    let criteria = |requested_units| RfqV2BestQuoteCriteria {
        rfq_id,
        terms_commitment: B256::repeat_byte(0x4a).into(),
        requested_units,
        parent_remaining_target: 10,
        parent_target: 10,
        parent_fill_policy: RfqV2FillPolicy::FullSizeOnce,
        parent_unit_grid: RfqV2ParentUnitGrid::UNBOUNDED,
        parent_legs: &[],
        now_ms: 1_900_000_000_000,
        auto_execution_limit: None,
    };
    assert_eq!(
        select_best_rfq_v2_quote(&quotes, criteria(5)),
        Err(RfqV2BestQuoteError::NoEligibleQuote)
    );
    assert!(select_best_rfq_v2_quote(&quotes, criteria(10)).is_ok());
}

/// The leg grid admits a size exactly when every leg is a whole `u64` number
/// of contract micro-units, the rule an execution must satisfy.
#[test]
fn parent_unit_grid_matches_the_contract_unit_rule() {
    let leg_rule_admits = |quantities: &[u128], units: u128| {
        quantities.iter().all(|&quantity| {
            quantity.checked_mul(units).is_some_and(|micros| {
                micros % 1_000_000 == 0 && micros / 1_000_000 <= u128::from(u64::MAX)
            })
        })
    };
    let quantities = [
        1,
        3,
        7,
        250_000,
        333_333,
        500_000,
        1_000_000,
        1_500_000,
        2_000_000,
        999_999_999,
        u128::from(u64::MAX),
    ];
    let max = u128::from(u64::MAX) * 1_000_000;
    let mut sizes: Vec<u128> = (0..=64).collect();
    for base in [
        1_000_000,
        u128::from(u64::MAX),
        max / 2_000_000,
        max / 1_500_000,
        max / 1_000_000,
        max / 500_000,
        max / 250_000,
        max,
        u128::MAX / 2,
        u128::MAX,
    ] {
        for delta in 0..=4u128 {
            sizes.extend([base.saturating_sub(delta), base.saturating_add(delta)]);
        }
    }
    let mut groups: Vec<Vec<u128>> = vec![Vec::new()];
    for &a in &quantities {
        groups.push(vec![a]);
        for &b in &quantities {
            groups.push(vec![a, b]);
        }
    }
    for group in &groups {
        let grid = RfqV2ParentUnitGrid::of(
            &group
                .iter()
                .map(|&quantity_micros_per_unit| RfqV2PackageLeg {
                    instrument: "BTC-20261225-100000-C".into(),
                    taker_side: RfqV2Side::Buy,
                    quantity_micros_per_unit,
                })
                .collect::<Vec<_>>(),
        );
        for &units in &sizes {
            assert_eq!(
                grid.admits(units),
                leg_rule_admits(group, units),
                "{group:?} {units}: {grid:?}"
            );
        }
    }
}

/// A quote's signed fee limit is a ceiling, not a charge, so it
/// neither fails a premium-exact auto limit nor changes the ranking.
#[test]
fn best_quote_selection_prices_the_charged_fee_not_the_signed_ceiling() {
    let rfq_id = id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce");
    let capped_id = id("169a869d-9fa7-490b-b68e-d78005740d4c");
    let uncapped_id = id("6889f9c6-cc85-40d9-86bb-df012ea74e2a");
    let mut capped = firm_view(rfq_id, capped_id, 100, 1_000_000, 5_000_000, false);
    let mut uncapped = firm_view(rfq_id, uncapped_id, 100, 1_000_000, 5_000_000, false);
    if let RfqV2QuoteView::Firm { terms, .. } = &mut capped {
        terms.fee_limits.per_execution = 50_000;
        terms.fee_limits.cumulative = 50_000;
    }
    if let RfqV2QuoteView::Firm { terms, .. } = &mut uncapped {
        terms.fee_limits.per_execution = 0;
    }
    let criteria = RfqV2BestQuoteCriteria {
        rfq_id,
        terms_commitment: B256::repeat_byte(0x4a).into(),
        requested_units: 1_000_000,
        parent_remaining_target: u128::MAX,
        parent_target: u128::MAX,
        parent_fill_policy: ANY_PARENT_SIZE,
        parent_unit_grid: RfqV2ParentUnitGrid::UNBOUNDED,
        parent_legs: &[],
        now_ms: 1_900_000_000_000,
        auto_execution_limit: Some(RfqV2AutoExecutionLimit {
            maker_receives_premium: false,
            premium_micros: 100,
        }),
    };
    let quotes = [capped.clone()];
    let RfqV2QuoteView::Firm { terms, .. } = select_best_rfq_v2_quote(&quotes, criteria).unwrap()
    else {
        panic!("expected firm quote")
    };
    assert_eq!(terms.id, capped_id);

    // Equal premiums tie on economics and break on the lower quote id,
    // whatever either quote's fee ceiling.
    let quotes = [uncapped, capped];
    let RfqV2QuoteView::Firm { terms, .. } = select_best_rfq_v2_quote(&quotes, criteria).unwrap()
    else {
        panic!("expected firm quote")
    };
    assert_eq!(terms.id, capped_id);
}

/// Every stable-ID mutation, not only execution, can take effect before its
/// response is lost. A 5xx or timeout must surface as ambiguous
/// with the identity to reconcile, never as a plain failure.
#[tokio::test]
async fn every_mutation_reports_a_server_failure_as_ambiguous() {
    fn unavailable() -> (&'static str, String) {
        (
            "503 Service Unavailable",
            json!({"code":"rfq_v2_authority_unavailable","message":"retry"}).to_string(),
        )
    }
    let rfq_id = id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce");
    let quote_id = id("6889f9c6-cc85-40d9-86bb-df012ea74e2a");
    let operation_id = id("0190a5c4-7e1f-7a2b-8c3d-4e5f60718293");
    let (base_url, task) = serve_status_sequence(vec![unavailable(); 6]).await;
    let client = HypercallClient::new(base_url);

    let ambiguous = |error: crate::ClientError, rfq: Option<Uuid>, op: Uuid| {
        assert!(
            matches!(
                &error,
                crate::ClientError::RfqV2SubmissionAmbiguous { rfq_id, operation_id, .. }
                    if *rfq_id == rfq && *operation_id == op
            ),
            "{error:?}"
        );
    };

    let create = create_request(rfq_id);
    ambiguous(
        client.create_rfq_v2(&create).await.unwrap_err(),
        Some(rfq_id),
        rfq_id,
    );
    let firm = firm_request(rfq_id, quote_id);
    ambiguous(
        client
            .publish_rfq_v2_firm_quote(rfq_id, &firm)
            .await
            .unwrap_err(),
        Some(rfq_id),
        quote_id,
    );
    let invalidation = RfqV2InvalidationRequest {
        operation_id,
        expires_at_ms: 1_900_000_030_000,
        nonce: 5,
        signature: signature(),
    };
    ambiguous(
        client
            .cancel_rfq_v2(rfq_id, &invalidation)
            .await
            .unwrap_err(),
        Some(rfq_id),
        operation_id,
    );
    ambiguous(
        client
            .withdraw_rfq_v2_quote(rfq_id, quote_id, &invalidation)
            .await
            .unwrap_err(),
        Some(rfq_id),
        operation_id,
    );
    let bulk = RfqV2BulkWithdrawalRequest {
        operation_id,
        maker_account: test_wallet(1),
        scope: RfqV2BulkWithdrawalScope::AllOpen,
        expires_at_ms: 1_900_000_030_000,
        nonce: 6,
        signature: signature(),
    };
    ambiguous(
        client.withdraw_rfq_v2_quotes(&bulk).await.unwrap_err(),
        None,
        operation_id,
    );
    let schedule = RfqV2ScheduledQuoteCancelRequest {
        operation_id,
        maker_account: test_wallet(1),
        cancel_at_ms: Some(1_900_000_040_000),
        expires_at_ms: 1_900_000_030_000,
        nonce: 7,
        signature: signature(),
    };
    ambiguous(
        client
            .schedule_rfq_v2_quote_cancel(&schedule)
            .await
            .unwrap_err(),
        None,
        operation_id,
    );
    assert_eq!(task.await.unwrap().len(), 6);
}

/// A definite refusal stays definite: it is not reported as ambiguous.
#[tokio::test]
async fn a_definite_mutation_refusal_is_not_ambiguous() {
    let (base_url, task) = serve_once_with_status(
        "409 Conflict",
        json!({"code":"rfq_v2_idempotency_conflict","message":"different terms"}).to_string(),
    )
    .await;
    let client = HypercallClient::new(base_url);
    let error = client
        .create_rfq_v2(&create_request(id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce")))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, crate::ClientError::RfqV2Api { status: 409, code, .. } if code == "rfq_v2_idempotency_conflict"),
        "{error:?}"
    );
    task.await.unwrap();
}

/// An invalid URL fails while the request is built, before anything is sent.
/// That is a definite failure, never an ambiguous submission.
#[tokio::test]
async fn a_request_that_never_left_the_client_is_not_ambiguous() {
    let client = HypercallClient::new("not a url");
    let error = client
        .create_rfq_v2(&create_request(id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce")))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, crate::ClientError::Http(source) if source.is_builder()),
        "{error:?}"
    );
}

/// A 4xx whose body is cut off is still a refusal the server already made,
/// not an ambiguous submission.
#[tokio::test]
async fn a_refusal_with_an_unreadable_body_stays_definite() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = vec![0_u8; 64 * 1024];
        let _ = stream.read(&mut buffer).await.unwrap();
        // Promise 100 body bytes, send 5, then close.
        stream
            .write_all(b"HTTP/1.1 409 Conflict\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{\"cod")
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });
    let client = HypercallClient::new(base_url);
    let error = client
        .create_rfq_v2(&create_request(id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce")))
        .await
        .unwrap_err();
    let crate::ClientError::ApiSource {
        status: 409,
        source,
    } = &error
    else {
        panic!("{error:?}");
    };
    // The concrete transport error survives for callers to inspect.
    assert!(
        source
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_body),
        "{source:?}"
    );
    assert!(!crate::rfq_v2::ambiguous_rfq_v2_submission(&error));
    assert!(crate::rfq_v2::ambiguous_rfq_v2_submission(
        &crate::ClientError::api_source(503, std::io::Error::other("cut off"))
    ));
    server.await.unwrap();
}

/// Serve each raw HTTP response to one connection, in order.
async fn serve_raw(responses: Vec<&'static [u8]>) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        for response in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0_u8; 64 * 1024];
            let _ = stream.read(&mut buffer).await.unwrap();
            stream.write_all(response).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    (base_url, task)
}

/// A 5xx whose body is cut off is a transient failure: the watcher reports
/// it and keeps polling. An unreadable 401 or 403 still ends the watch.
#[tokio::test]
async fn change_watcher_retries_a_truncated_5xx_and_stops_on_a_truncated_401() {
    let (base_url, task) = serve_raw(vec![
        b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 100\r\n\r\n{\"cod",
        b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 55\r\n\r\n{\"changes\":[],\"next_after_sequence\":8,\"has_more\":false}",
    ])
    .await;
    let client = HypercallClient::new(base_url);
    let mut pages = client
        .watch_rfq_v2_changes(&read_grant(1_900_000_030_000), 7, Duration::from_millis(20))
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(1), pages.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(first, Err(ClientError::ApiSource { status: 503, .. })),
        "{first:?}"
    );
    let recovered = tokio::time::timeout(Duration::from_secs(1), pages.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(recovered.next_after_sequence, 8);
    drop(pages);
    task.await.unwrap();

    let (base_url, task) = serve_raw(vec![
        b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 100\r\n\r\n{\"cod",
    ])
    .await;
    let client = HypercallClient::new(base_url);
    let mut pages = client
        .watch_rfq_v2_changes(&read_grant(1_900_000_030_000), 7, Duration::from_millis(20))
        .unwrap();
    let denied = tokio::time::timeout(Duration::from_secs(1), pages.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(denied, Err(ClientError::ApiSource { status: 401, .. })),
        "{denied:?}"
    );
    assert!(tokio::time::timeout(Duration::from_secs(1), pages.recv())
        .await
        .unwrap()
        .is_none());
    task.await.unwrap();
}
