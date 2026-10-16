use alloy::primitives::B256;
use hypercall_sdk_types::*;
use serde_json::json;

use super::*;
use crate::test_support::{serve_sequence, serve_status_sequence};

const MAKER_A: u8 = 1;
const MAKER_B: u8 = 2;

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn signature() -> RfqV2Signature {
    format!("0x{}1b", "11".repeat(64)).parse().unwrap()
}

fn commitment() -> RfqV2Digest {
    B256::repeat_byte(0x4a).into()
}

fn parent_terms(rfq_id: Uuid) -> RfqV2Terms {
    RfqV2Terms {
        id: rfq_id,
        taker_account: test_wallet(10),
        legs: vec![RfqV2PackageLeg {
            instrument: "BTC-20261225-100000-C".into(),
            taker_side: RfqV2Side::Buy,
            quantity_micros_per_unit: 1_000_000,
        }],
        target: 5_000_000,
        fill_policy: RfqV2FillPolicy::FullSizeOnce,
        execution_mode: RfqV2ExecutionMode::Manual,
        auto_execution_limit: None,
        quote_policy: RfqV2QuotePolicy::FirmOnly,
        taker_identity: RfqV2TakerIdentity::Disclosed,
        routing: RfqV2Routing::AllEligible,
        quote_deadline_ms: 1_900_000_020_000,
        expires_at_ms: 1_900_000_060_000,
        fee_limits: RfqV2FeeLimits {
            per_execution: 0,
            cumulative: 0,
        },
        builder: None,
        private_terms_blinding: B256::ZERO.into(),
    }
}

fn parent_json(rfq_id: Uuid, status: &str, remaining_target: u128) -> serde_json::Value {
    json!({
        "rfq_id": rfq_id,
        "status": status,
        "terms_commitment": commitment(),
        "parent": {"view": "taker", "terms": parent_terms(rfq_id)},
        "target": "5000000",
        "remaining_target": remaining_target.to_string(),
        "quote_policy": "firm_only",
    })
}

fn parent(rfq_id: Uuid, status: &str, remaining_target: u128) -> RfqV2Response {
    serde_json::from_value(parent_json(rfq_id, status, remaining_target)).unwrap()
}

fn firm_terms(rfq_id: Uuid, quote_id: Uuid, maker: u8) -> RfqV2FirmQuoteTerms {
    RfqV2FirmQuoteTerms {
        id: quote_id,
        rfq_id,
        rfq_terms_commitment: commitment(),
        maker_account: test_wallet(maker),
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
        fill_policy: RfqV2FillPolicy::FullSizeOnce,
        withdrawal_policy: RfqV2WithdrawalPolicy::Permitted,
        fee_limits: RfqV2FeeLimits {
            per_execution: 0,
            cumulative: 0,
        },
        expires_at_ms: 1_900_000_045_000,
        protection_scope: None,
        source_indicative_quote_id: None,
    }
}

fn firm(
    rfq_id: Uuid,
    quote_id: Uuid,
    status: &str,
    executable: bool,
    remaining_capacity: u128,
) -> RfqV2QuoteView {
    RfqV2QuoteView::Firm {
        terms: firm_terms(rfq_id, quote_id, MAKER_A),
        // An unmanaged maker is its own provider and signer.
        provider: test_wallet(MAKER_A),
        signer: test_wallet(MAKER_A),
        status: status.into(),
        executable,
        remaining_capacity,
        preflight: None,
    }
}

fn quote_page(quotes: &[RfqV2QuoteView], next_cursor: Option<&str>, as_of: u64) -> String {
    json!({
        "quotes": quotes,
        "page": {"limit": 100, "next_cursor": next_cursor, "has_more": next_cursor.is_some()},
        "as_of_sequence": as_of,
    })
    .to_string()
}

fn execution_request() -> RfqV2ExecutionRequest {
    RfqV2ExecutionRequest {
        operation_id: id(0x0e),
        quote_id: id(0x0b),
        package_units: 5_000_000,
        max_fee_micros: 0,
        expires_at_ms: 1_900_000_030_000,
        nonce: 77,
        signature: signature(),
    }
}

fn execution_response() -> String {
    json!({
        "operation_id": id(0x0e),
        "status": "filled",
        "executions": [{
            "execution_id": id(0xe0),
            "quote_id": id(0x0b),
            "maker_account": test_wallet(MAKER_A),
            "maker_provider": test_wallet(MAKER_A),
            "maker_signer": test_wallet(MAKER_A),
            "package_units": "5000000",
            "fee_micros": "0"
        }],
        "rfq_remaining_target": "0",
        "quote_remaining_capacity": "0"
    })
    .to_string()
}

fn request_body(request: &str) -> &str {
    request.split_once("\r\n\r\n").unwrap().1
}

fn policy(max_resubmissions: u32) -> RfqV2ReconcilePolicy {
    RfqV2ReconcilePolicy {
        retry_interval: Duration::from_millis(1),
        max_resubmissions,
    }
}

#[tokio::test]
async fn reconcile_resends_identical_signed_bytes_until_the_original_outcome() {
    let request = execution_request();
    let (url, server) = serve_status_sequence(vec![
        (
            "503 Service Unavailable",
            json!({"code": "rfq_v2_authority_stalled", "message": "stalled"}).to_string(),
        ),
        ("200 OK", execution_response()),
    ])
    .await;
    let client = HypercallClient::new(url);
    let reconciled = client
        .reconcile_rfq_v2_execution(id(0x0a), &request, policy(3))
        .await
        .unwrap();
    assert_eq!(reconciled.resubmissions, 2);
    assert_eq!(reconciled.response.operation_id, request.operation_id);
    let original = String::from_utf8(sonic_rs::to_vec(&request).unwrap()).unwrap();
    for sent in server.await.unwrap() {
        assert!(sent.starts_with(&format!("POST /private/rfqs/{}/executions ", id(0x0a))));
        assert_eq!(request_body(&sent), original, "never new signed bytes");
    }
}

#[tokio::test]
async fn reconcile_returns_a_definite_rejection_and_keeps_unknown_outcomes_ambiguous() {
    let request = execution_request();
    let (url, _server) = serve_status_sequence(vec![(
        "409 Conflict",
        json!({"code": "rfq_v2_idempotency_conflict", "message": "different bytes"}).to_string(),
    )])
    .await;
    let error = HypercallClient::new(url)
        .reconcile_rfq_v2_execution(id(0x0a), &request, policy(3))
        .await
        .unwrap_err();
    assert!(
        matches!(error, ClientError::RfqV2Api { status: 409, ref code, .. } if code == "rfq_v2_idempotency_conflict")
    );

    let unavailable = json!({"code": "rfq_v2_authority_timeout", "message": "queued"}).to_string();
    let (url, server) = serve_status_sequence(vec![
        ("503 Service Unavailable", unavailable.clone()),
        ("503 Service Unavailable", unavailable),
    ])
    .await;
    let error = HypercallClient::new(url)
        .reconcile_rfq_v2_execution(id(0x0a), &request, policy(2))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ClientError::RfqV2SubmissionAmbiguous { operation_id, .. } if operation_id == request.operation_id
    ));
    assert_eq!(server.await.unwrap().len(), 2);
}

#[test]
fn change_cursor_dedupes_hints_and_accepts_global_sequence_gaps() {
    let mut cursor = RfqV2ChangeCursor::new(10);
    assert_eq!(
        cursor.observe_hint(10),
        RfqV2HintDisposition::AlreadyApplied
    );
    // 11..=13 belong to other recipients: a gap is not a lost frame.
    assert_eq!(
        cursor.observe_hint(14),
        RfqV2HintDisposition::ReplayRequired
    );
    assert_eq!(cursor.observe_hint(14), RfqV2HintDisposition::Duplicate);
    assert_eq!(
        cursor.observe_hint(20),
        RfqV2HintDisposition::ReplayRequired
    );
    cursor.commit(14);
    assert_eq!(cursor.last_applied_sequence(), 14);
    assert_eq!(cursor.pending_hints(), 1);
    assert_eq!(
        cursor.observe_hint(14),
        RfqV2HintDisposition::AlreadyApplied
    );
    let restored: RfqV2ChangeCursor =
        serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
    assert_eq!(restored, cursor);
}

/// The feed is unavailable while hints keep arriving. The cursor keeps only
/// the highest unread sequence, so it does not grow, and every lower or
/// repeated hint is covered by the feed read that one requires.
#[test]
fn change_cursor_keeps_one_high_water_hint_under_sustained_traffic() {
    let mut cursor = RfqV2ChangeCursor::new(10);
    for sequence in 11..=10_010 {
        assert_eq!(
            cursor.observe_hint(sequence),
            RfqV2HintDisposition::ReplayRequired
        );
    }
    assert_eq!(cursor.pending_hints(), 1);
    assert_eq!(cursor.pending_high_water(), Some(10_010));
    // A late or repeated lower hint is covered by the pending feed read.
    assert_eq!(cursor.observe_hint(500), RfqV2HintDisposition::Duplicate);
    let persisted = serde_json::to_value(&cursor).unwrap();
    assert_eq!(
        persisted,
        json!({"last_applied_sequence": 10, "pending": [10_010]})
    );
    // A feed read that stops short keeps the hint pending; one past it clears it.
    cursor.commit(9_000);
    assert_eq!(cursor.pending_high_water(), Some(10_010));
    cursor.commit(10_010);
    assert_eq!(cursor.pending_high_water(), None);
}

/// A cursor persisted by an earlier release holds every pending sequence.
/// It loads as the same pending read: the highest sequence above the
/// applied cursor.
#[test]
fn change_cursor_reads_a_persisted_pending_set_as_its_high_water_mark() {
    let restored: RfqV2ChangeCursor = serde_json::from_value(json!({
        "last_applied_sequence": 10,
        "pending": [12, 14, 20]
    }))
    .unwrap();
    assert_eq!(restored.last_applied_sequence(), 10);
    assert_eq!(restored.pending_high_water(), Some(20));
    let covered: RfqV2ChangeCursor = serde_json::from_value(json!({
        "last_applied_sequence": 30,
        "pending": [12, 20]
    }))
    .unwrap();
    assert_eq!(covered.pending_high_water(), None);
}

#[test]
fn ws_hint_decoding_ignores_other_frames() {
    let hint: sonic_rs::Value = sonic_rs::from_str(
        r#"{"type":"RfqV2Changed","rfq_id":"00000000-0000-0000-0000-00000000000a","sequence":7}"#,
    )
    .unwrap();
    let decoded = rfq_v2_change_hint(&hint).unwrap();
    assert_eq!(decoded.sequence, 7);
    let other: sonic_rs::Value =
        sonic_rs::from_str(r#"{"type":"Subscribed","channel":"rfq_v2"}"#).unwrap();
    assert!(rfq_v2_change_hint(&other).is_none());
}

#[tokio::test]
async fn mirror_replays_from_the_applied_cursor_and_records_each_transition_once() {
    let rfq_id = id(0x0a);
    let quote_id = id(0x0b);
    let mut mirror = RfqV2PrivateMirror::new(RfqV2ChangeCursor::new(4));
    let hint = |sequence| WsRfqV2Changed {
        rfq_id: rfq_id.to_string(),
        sequence,
        quote_id: None,
    };
    assert_eq!(
        mirror.observe_hint(&hint(9)),
        RfqV2HintDisposition::ReplayRequired
    );
    assert_eq!(
        mirror.observe_hint(&hint(9)),
        RfqV2HintDisposition::Duplicate
    );
    let (url, server) = serve_sequence(vec![
        json!({"changes": [{"rfq_id": rfq_id, "sequence": 6}, {"rfq_id": rfq_id, "sequence": 9}],
            "next_after_sequence": 9, "has_more": false})
        .to_string(),
        parent_json(rfq_id, "filled", 0).to_string(),
        quote_page(&[firm(rfq_id, quote_id, "filled", false, 0)], None, 9),
    ])
    .await;
    let refreshed = HypercallClient::new(url)
        .sync_rfq_v2_mirror(
            &mut mirror,
            RfqV2ReadCredentials {
                bearer_token: "grant",
            },
        )
        .await
        .unwrap();
    assert_eq!(refreshed, BTreeSet::from([rfq_id]));
    let requests = server.await.unwrap();
    assert!(requests[0].starts_with("GET /private/rfqs/changes?after_sequence=4&limit=100 "));
    assert!(requests[2].contains("include_terminal=true"));
    assert_eq!(mirror.cursor().last_applied_sequence(), 9);
    assert_eq!(mirror.cursor().pending_hints(), 0);
    assert_eq!(mirror.parent(rfq_id).unwrap().status, "filled");
    assert_eq!(mirror.quotes(rfq_id).unwrap().len(), 1);
    let stats = mirror.stats();
    assert_eq!(stats.duplicate_hints, 1);
    assert_eq!(stats.duplicate_transitions, 0);
    assert_eq!(stats.rfq_refreshes, 1, "one refresh per named RFQ per page");
    assert_eq!(
        mirror.observe_hint(&hint(9)),
        RfqV2HintDisposition::AlreadyApplied
    );
}

#[test]
fn mirror_counts_a_regressed_lifecycle_as_a_duplicate_transition() {
    let rfq_id = id(0x0a);
    let mut mirror = RfqV2PrivateMirror::default();
    for status in ["open", "filled", "open"] {
        let remaining = if status == "open" { 5_000_000 } else { 0 };
        mirror
            .apply_refresh(parent(rfq_id, status, remaining), BTreeMap::new())
            .unwrap();
    }
    assert_eq!(mirror.stats().duplicate_transitions, 1);
    assert_eq!(mirror.transitions().len(), 3);
}

#[test]
fn mirror_rejects_a_refresh_that_drops_retained_quote_history() {
    let rfq_id = id(0x0a);
    let quote_id = id(0x0b);
    let mut mirror = RfqV2PrivateMirror::default();
    mirror
        .apply_refresh(
            parent(rfq_id, "open", 5_000_000),
            BTreeMap::from([(quote_id, firm(rfq_id, quote_id, "open", true, 5_000_000))]),
        )
        .unwrap();
    // The rejected page also moves the parent and lists another quote. None
    // of it may reach the persisted mirror, transition history included.
    let before = mirror.clone();
    let other = id(0x0c);
    let error = mirror
        .apply_refresh(
            parent(rfq_id, "filled", 0),
            BTreeMap::from([(other, firm(rfq_id, other, "filled", false, 0))]),
        )
        .unwrap_err();
    assert!(matches!(error, ClientError::RfqV2ProtocolViolation(_)));
    assert_eq!(mirror, before);
}

#[tokio::test]
async fn quote_history_resumes_mid_scan_and_after_completion_without_duplicates() {
    let rfq_id = id(0x0a);
    let (a, b, c) = (id(0x0b), id(0x0c), id(0x0d));
    let (url, server) = serve_sequence(vec![
        quote_page(&[firm(rfq_id, a, "open", true, 5_000_000)], Some("3"), 3),
        quote_page(&[firm(rfq_id, b, "withdrawn", false, 5_000_000)], None, 5),
        quote_page(&[firm(rfq_id, c, "open", true, 5_000_000)], None, 8),
    ])
    .await;
    let credentials = || RfqV2ReadCredentials {
        bearer_token: "grant",
    };
    let mut history = RfqV2QuoteHistory::new(rfq_id, 1).unwrap();
    let first = HypercallClient::new(url.clone());
    first
        .next_rfq_v2_quote_page(&mut history, credentials())
        .await
        .unwrap();
    assert!(!history.complete());
    // Disconnect: persist, drop the client, restore into a new one.
    let persisted = serde_json::to_string(&history).unwrap();
    drop(first);
    let mut history: RfqV2QuoteHistory = serde_json::from_str(&persisted).unwrap();
    let second = HypercallClient::new(url);
    second
        .sync_rfq_v2_quote_history(&mut history, credentials())
        .await
        .unwrap();
    assert!(history.complete());
    assert_eq!(history.as_of_sequence(), 5);
    second
        .sync_rfq_v2_quote_history(&mut history, credentials())
        .await
        .unwrap();
    assert_eq!(
        history.quotes().keys().copied().collect::<Vec<_>>(),
        vec![a, b, c]
    );
    let requests = server.await.unwrap();
    assert!(requests[0].contains("limit=1"));
    assert!(requests[1].contains("cursor=3"));
    assert!(!requests[1].contains("after_sequence"));
    assert!(requests[2].contains("after_sequence=5"));
}

#[tokio::test]
async fn quote_history_rejects_a_quote_delivered_on_two_pages() {
    let rfq_id = id(0x0a);
    let a = id(0x0b);
    let (url, _server) = serve_sequence(vec![
        quote_page(&[firm(rfq_id, a, "open", true, 5_000_000)], Some("3"), 3),
        quote_page(&[firm(rfq_id, a, "open", true, 5_000_000)], None, 4),
    ])
    .await;
    let mut history = RfqV2QuoteHistory::new(rfq_id, 1).unwrap();
    let error = HypercallClient::new(url)
        .sync_rfq_v2_quote_history(
            &mut history,
            RfqV2ReadCredentials {
                bearer_token: "grant",
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ClientError::RfqV2ProtocolViolation(_)));
    assert!(RfqV2QuoteHistory::new(rfq_id, 0).is_err());
}

#[test]
fn execution_action_ignores_a_stale_capacity_observation() {
    let rfq_id = id(0x0a);
    let quote_id = id(0x0b);
    let stale = firm(rfq_id, quote_id, "open", true, 5_000_000);
    let current = firm(rfq_id, quote_id, "filled", false, 0);
    let action = rfq_v2_execution_action(
        &parent(rfq_id, "filled", 0),
        &current,
        5_000_000,
        1_900_000_000_000,
        Some(&stale),
    );
    assert!(!action.enabled);
    assert!(action.observation_stale);
    assert_eq!(action.block, Some(RfqV2ExecutionBlock::ParentNotExecutable));
    assert_eq!(action.remaining_capacity, 0);
    assert_eq!(action.quote_status, "filled");

    let open = rfq_v2_execution_action(
        &parent(rfq_id, "open", 5_000_000),
        &stale,
        5_000_000,
        1_900_000_000_000,
        Some(&stale),
    );
    assert!(open.enabled);
    assert!(!open.observation_stale);
    let partial = rfq_v2_execution_action(
        &parent(rfq_id, "open", 5_000_000),
        &stale,
        1_000_000,
        1_900_000_000_000,
        None,
    );
    assert_eq!(partial.block, Some(RfqV2ExecutionBlock::FillPolicy));
    let expired = rfq_v2_execution_action(
        &parent(rfq_id, "open", 5_000_000),
        &stale,
        5_000_000,
        1_900_000_045_000,
        None,
    );
    assert_eq!(expired.block, Some(RfqV2ExecutionBlock::Expired));
}

/// The review case: an open full-size parent with target 5 and a repeated
/// partial quote. The quote alone admits a 1-unit fill, but every fill must
/// satisfy the parent's signed policy too, so only the full target enables.
#[test]
fn execution_action_enforces_the_parent_fill_policy() {
    let rfq_id = id(0x0a);
    let quote_id = id(0x0b);
    let RfqV2QuoteView::Firm {
        mut terms,
        provider,
        signer,
        ..
    } = firm(rfq_id, quote_id, "open", true, 5_000_000)
    else {
        unreachable!()
    };
    terms.fill_policy = RfqV2FillPolicy::RepeatedPartial {
        minimum: 1_000_000,
        step: 1_000_000,
    };
    let quote = RfqV2QuoteView::Firm {
        terms,
        provider,
        signer,
        status: "open".into(),
        executable: true,
        remaining_capacity: 5_000_000,
        preflight: None,
    };
    let full_size_parent = parent(rfq_id, "open", 5_000_000);
    let partial = rfq_v2_execution_action(
        &full_size_parent,
        &quote,
        1_000_000,
        1_900_000_000_000,
        None,
    );
    assert!(!partial.enabled);
    assert_eq!(partial.block, Some(RfqV2ExecutionBlock::FillPolicy));
    let full = rfq_v2_execution_action(
        &full_size_parent,
        &quote,
        5_000_000,
        1_900_000_000_000,
        None,
    );
    assert!(full.enabled, "{full:?}");

    // A one-smaller-package parent that already filled once admits nothing.
    let mut used = parent_json(rfq_id, "partially_filled", 4_000_000);
    used["parent"]["terms"]["fill_policy"] = json!({
        "mode": "one_smaller_package",
        "minimum": "1000000",
        "step": "1000000",
    });
    let used: RfqV2Response = serde_json::from_value(used).unwrap();
    assert!(matches!(
        used.parent,
        RfqV2AuthorizedTerms::Taker {
            terms: RfqV2Terms {
                fill_policy: RfqV2FillPolicy::OneSmallerPackage { .. },
                ..
            }
        }
    ));
    let after_fill = rfq_v2_execution_action(&used, &quote, 1_000_000, 1_900_000_000_000, None);
    assert_eq!(after_fill.block, Some(RfqV2ExecutionBlock::FillPolicy));
}

/// A quote with the given policy, signed capacity and remaining capacity.
fn firm_with(
    rfq_id: Uuid,
    quote_id: Uuid,
    fill_policy: RfqV2FillPolicy,
    capacity: u128,
    remaining_capacity: u128,
) -> RfqV2QuoteView {
    let mut terms = firm_terms(rfq_id, quote_id, MAKER_A);
    terms.fill_policy = fill_policy;
    terms.capacity = capacity;
    RfqV2QuoteView::Firm {
        terms,
        provider: test_wallet(MAKER_A),
        signer: test_wallet(MAKER_A),
        status: if remaining_capacity == capacity {
            "open"
        } else {
            "partially_filled"
        }
        .into(),
        executable: true,
        remaining_capacity,
        preflight: None,
    }
}

/// A taker parent with the given legs' quantities, policy and targets.
fn parent_with(
    rfq_id: Uuid,
    quantities: &[u128],
    fill_policy: RfqV2FillPolicy,
    target: u128,
    remaining_target: u128,
) -> RfqV2Response {
    let mut terms = parent_terms(rfq_id);
    terms.legs = quantities
        .iter()
        .map(|&quantity_micros_per_unit| RfqV2PackageLeg {
            instrument: "BTC-20261225-100000-C".into(),
            taker_side: RfqV2Side::Buy,
            quantity_micros_per_unit,
        })
        .collect();
    terms.fill_policy = fill_policy;
    terms.target = target;
    let mut json = parent_json(
        rfq_id,
        if remaining_target == target {
            "open"
        } else {
            "partially_filled"
        },
        remaining_target,
    );
    json["parent"]["terms"] = serde_json::to_value(terms).unwrap();
    json["target"] = json!(target.to_string());
    serde_json::from_value(json).unwrap()
}

/// A fill is `units * quantity / 1e6` contract micro-units per leg, and a
/// fractional or beyond-`u64` quantity is refused. A manual parent whose
/// policy grid is finer than that leg grid must not enable such a size.
#[test]
fn execution_action_rejects_engine_unrepresentable_sizes() {
    let rfq_id = id(0x0a);
    let quote_id = id(0x0b);
    let any = RfqV2FillPolicy::RepeatedPartial {
        minimum: 1,
        step: 1,
    };
    // 0.5 contracts per unit: 3 units would book 1.5 contracts.
    let half = parent_with(rfq_id, &[500_000], any, 3, 3);
    let quote = firm_with(rfq_id, quote_id, any, 3, 3);
    let odd = rfq_v2_execution_action(&half, &quote, 3, 1_900_000_000_000, None);
    assert!(!odd.enabled);
    assert_eq!(odd.block, Some(RfqV2ExecutionBlock::UnrepresentableSize));
    let even = rfq_v2_execution_action(&half, &quote, 2, 1_900_000_000_000, None);
    assert!(even.enabled, "{even:?}");

    // Whole contracts per unit, but one past u64 contract units.
    let past = u128::from(u64::MAX) + 1;
    let large = parent_with(rfq_id, &[1_000_000], any, past, past);
    let quote = firm_with(rfq_id, quote_id, any, past, past);
    let overflow = rfq_v2_execution_action(&large, &quote, past, 1_900_000_000_000, None);
    assert_eq!(
        overflow.block,
        Some(RfqV2ExecutionBlock::UnrepresentableSize)
    );
    // The grid admits the largest u64 size, but at 1.8e13 contracts it is
    // outside the engine's record range, which refuses it next.
    let at_ceiling = rfq_v2_execution_action(&large, &quote, past - 1, 1_900_000_000_000, None);
    assert_eq!(
        at_ceiling.block,
        Some(RfqV2ExecutionBlock::OutsideRecordRange)
    );

    // Any leg can refuse the whole package.
    let mixed = parent_with(rfq_id, &[1_000_000, 250_000], any, 8, 8);
    let quote = firm_with(rfq_id, quote_id, any, 8, 8);
    for units in 1..=8 {
        let action = rfq_v2_execution_action(&mixed, &quote, units, 1_900_000_000_000, None);
        assert_eq!(action.enabled, units % 4 == 0, "{units}: {action:?}");
    }
}

/// Best-quote selection and the execution action decide the same sizes for
/// the same fresh reads: across every combination of parent and
/// quote policy, signed and remaining amounts, leg grid, and request,
/// selection picks the quote exactly when the action is enabled.
#[test]
fn best_quote_selection_agrees_with_the_execution_action() {
    let rfq_id = id(0x0a);
    let quote_id = id(0x0b);
    let now = 1_900_000_000_000;
    let policies = [
        RfqV2FillPolicy::FullSizeOnce,
        RfqV2FillPolicy::OneSmallerPackage {
            minimum: 2,
            step: 2,
        },
        RfqV2FillPolicy::RepeatedPartial {
            minimum: 1,
            step: 1,
        },
        RfqV2FillPolicy::RepeatedPartial {
            minimum: 3,
            step: 3,
        },
    ];
    let leg_grids: [&[u128]; 4] = [&[1_000_000], &[500_000], &[250_000, 1_000_000], &[]];
    let mut checked = 0usize;
    for parent_policy in policies {
        for quote_policy in policies {
            for legs in leg_grids {
                for (target, remaining_target) in [(6, 6), (6, 4), (6, 0)] {
                    for (capacity, remaining_capacity) in [(6, 6), (6, 3), (4, 4)] {
                        let parent =
                            parent_with(rfq_id, legs, parent_policy, target, remaining_target);
                        let quote =
                            firm_with(rfq_id, quote_id, quote_policy, capacity, remaining_capacity);
                        for units in 0..=7 {
                            let action = rfq_v2_execution_action(&parent, &quote, units, now, None);
                            let selected = crate::rfq_v2::select_best_rfq_v2_quote(
                                std::slice::from_ref(&quote),
                                crate::rfq_v2::RfqV2BestQuoteCriteria::for_parent(
                                    &parent, units, now,
                                ),
                            )
                            .is_ok();
                            assert_eq!(
                                selected, action.enabled,
                                "parent {parent_policy:?} {legs:?} {target}/{remaining_target}, \
                                 quote {quote_policy:?} {capacity}/{remaining_capacity}, \
                                 units {units}: {action:?}"
                            );
                            checked += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checked, 4 * 4 * 4 * 3 * 3 * 8);
}

/// The engine refuses a fill whose leg size, leg price, leg premium or
/// package premium reaches 1e12, its fill record range. The execution action
/// and best-quote selection refuse the same fills, at the exact boundary,
/// instead of offering one the server rejects.
#[test]
fn execution_action_and_selection_apply_the_engine_record_range() {
    const E12: u128 = 1_000_000_000_000;
    const E17: u128 = 100_000 * E12;
    let rfq_id = id(0x0a);
    let quote_id = id(0x0b);
    let now = 1_900_000_000_000;
    let units = 5_000_000;
    const BUY: RfqV2Side = RfqV2Side::Buy;
    const SELL: RfqV2Side = RfqV2Side::Sell;
    // (legs as (taker side, quantity micros per unit), leg price micros, in range)
    type Case = (&'static [(RfqV2Side, u128)], &'static [u128], bool);
    let cases: [Case; 10] = [
        (&[(BUY, 1_000_000)], &[830_000_000], true),
        // Leg price: 1e12 - 1e-6 fits, 1e12 does not (size 0.5 keeps the
        // premium inside).
        (&[(BUY, 100_000)], &[E12 * 1_000_000 - 1], true),
        (&[(BUY, 100_000)], &[E12 * 1_000_000], false),
        // Leg premium: 5 contracts at 2e11 is exactly 1e12.
        (&[(BUY, 1_000_000)], &[2 * E17 - 1], true),
        (&[(BUY, 1_000_000)], &[2 * E17], false),
        // Leg size: 5 units of 2e17 micros is exactly 1e12 contracts.
        (&[(BUY, 2 * E17 - 1)], &[0], true),
        (&[(BUY, 2 * E17)], &[0], false),
        // Package premium: two legs of 6e11 each fit, their 1.2e12 sum does
        // not, unless one leg is sold and offsets the other.
        (
            &[(BUY, 1_000_000), (BUY, 1_000_000)],
            &[12 * E17 / 10, 12 * E17 / 10],
            false,
        ),
        (
            &[(SELL, 1_000_000), (SELL, 1_000_000)],
            &[12 * E17 / 10, 12 * E17 / 10],
            false,
        ),
        (
            &[(BUY, 1_000_000), (SELL, 1_000_000)],
            &[12 * E17 / 10, 12 * E17 / 10],
            true,
        ),
    ];
    for (legs, prices, in_range) in cases {
        let mut parent = parent_with(
            rfq_id,
            &legs.iter().map(|leg| leg.1).collect::<Vec<_>>(),
            RfqV2FillPolicy::FullSizeOnce,
            units,
            units,
        );
        let RfqV2AuthorizedTerms::Taker { terms } = &mut parent.parent else {
            unreachable!("parent_with builds a taker view");
        };
        for (leg, (side, _)) in terms.legs.iter_mut().zip(legs) {
            leg.taker_side = *side;
        }
        let mut quote = firm(rfq_id, quote_id, "open", true, units);
        let RfqV2QuoteView::Firm { terms, .. } = &mut quote else {
            unreachable!("firm builds a firm view");
        };
        terms.priced_legs = prices
            .iter()
            .map(|&price_micros| RfqV2PricedLeg {
                instrument: "BTC-20261225-100000-C".into(),
                price_micros,
            })
            .collect();

        let action = rfq_v2_execution_action(&parent, &quote, units, now, None);
        let selected = crate::rfq_v2::select_best_rfq_v2_quote(
            std::slice::from_ref(&quote),
            crate::rfq_v2::RfqV2BestQuoteCriteria::for_parent(&parent, units, now),
        )
        .is_ok();
        assert_eq!(action.enabled, in_range, "{legs:?} {prices:?}: {action:?}");
        assert_eq!(selected, in_range, "{legs:?} {prices:?}");
        if !in_range {
            assert_eq!(action.block, Some(RfqV2ExecutionBlock::OutsideRecordRange));
        }
    }
}

fn hold() -> RfqV2MakerHold {
    RfqV2MakerHold::for_firm_quote(&firm_terms(id(0x0a), id(0x0b), MAKER_A), 12)
}

#[test]
fn maker_hold_stays_unresolved_without_conclusive_identity_bound_evidence() {
    let rfq_id = id(0x0a);
    let quote_id = id(0x0b);
    let now = 1_900_000_000_000;
    let mut hold = hold();
    let filled_parent = parent(rfq_id, "filled", 0);
    let untouched = firm(rfq_id, quote_id, "open", false, 5_000_000);

    // A 404 never proves a loss, even when a competitor in fact won.
    for evidence in [
        hold.evaluate(None, Some(&untouched), now),
        hold.evaluate(Some(&filled_parent), None, now),
        hold.evaluate(None, None, now),
    ] {
        assert_eq!(evidence, RfqV2MakerHoldEvidence::Missing);
        hold.apply(&evidence);
        assert!(!hold.released());
    }
    let live = firm(rfq_id, quote_id, "open", true, 5_000_000);
    assert_eq!(
        hold.evaluate(Some(&parent(rfq_id, "open", 5_000_000)), Some(&live), now),
        RfqV2MakerHoldEvidence::Inconclusive
    );
    let other_maker = RfqV2QuoteView::Firm {
        terms: firm_terms(rfq_id, quote_id, MAKER_B),
        provider: test_wallet(MAKER_B),
        signer: test_wallet(MAKER_B),
        status: "open".into(),
        executable: false,
        remaining_capacity: 5_000_000,
        preflight: None,
    };
    assert_eq!(
        hold.evaluate(Some(&filled_parent), Some(&other_maker), now),
        RfqV2MakerHoldEvidence::IdentityMismatch
    );
    let other_rfq = parent(id(0x0c), "filled", 0);
    assert_eq!(
        hold.evaluate(Some(&other_rfq), Some(&untouched), now),
        RfqV2MakerHoldEvidence::IdentityMismatch
    );
    // Expiry counts only after the hold's own response window closed.
    let expired = firm(rfq_id, quote_id, "expired", false, 5_000_000);
    let open_parent = parent(rfq_id, "open", 5_000_000);
    assert_eq!(
        hold.evaluate(Some(&open_parent), Some(&expired), now),
        RfqV2MakerHoldEvidence::Inconclusive
    );
    assert_eq!(
        hold.evaluate(
            Some(&open_parent),
            Some(&expired),
            hold.response_window_ends_at_ms
        ),
        RfqV2MakerHoldEvidence::Conclusive(RfqV2MakerHoldProof::TerminalUnfilled {
            status: "expired".into()
        })
    );
}

#[test]
fn maker_hold_releases_on_competing_winner_or_own_fill_and_survives_persistence() {
    let rfq_id = id(0x0a);
    let quote_id = id(0x0b);
    let now = 1_900_000_000_000;
    let mut hold = hold();
    let restored: RfqV2MakerHold =
        serde_json::from_str(&serde_json::to_string(&hold).unwrap()).unwrap();
    assert_eq!(restored, hold);

    let competing = hold.evaluate(
        Some(&parent(rfq_id, "filled", 0)),
        Some(&firm(rfq_id, quote_id, "open", false, 5_000_000)),
        now,
    );
    assert_eq!(
        competing,
        RfqV2MakerHoldEvidence::Conclusive(RfqV2MakerHoldProof::CompetingWinner)
    );
    hold.apply(&competing);
    assert!(hold.released());
    // A released hold is final.
    hold.apply(&RfqV2MakerHoldEvidence::Missing);
    assert_eq!(
        hold.state,
        RfqV2MakerHoldState::Released {
            proof: RfqV2MakerHoldProof::CompetingWinner
        }
    );

    let own = self::hold().evaluate(
        Some(&parent(rfq_id, "filled", 0)),
        Some(&firm(rfq_id, quote_id, "filled", false, 0)),
        now,
    );
    assert_eq!(
        own,
        RfqV2MakerHoldEvidence::Conclusive(RfqV2MakerHoldProof::OwnFill {
            filled_units: 5_000_000
        })
    );
}

/// A partly used quote that can no longer execute (a spent one-shot policy)
/// stays `partially_filled` but never fills again, so the hold releases.
#[test]
fn maker_hold_releases_an_exhausted_partial_fill() {
    let rfq_id = id(0x0a);
    let quote_id = id(0x0b);
    let now = 1_900_000_000_000;
    let open_parent = parent(rfq_id, "partially_filled", 4_000_000);
    assert_eq!(
        hold().evaluate(
            Some(&open_parent),
            Some(&firm(
                rfq_id,
                quote_id,
                "partially_filled",
                false,
                4_000_000
            )),
            now,
        ),
        RfqV2MakerHoldEvidence::Conclusive(RfqV2MakerHoldProof::OwnFill {
            filled_units: 1_000_000
        })
    );
    // While it can still execute, the remainder stays held.
    assert_eq!(
        hold().evaluate(
            Some(&open_parent),
            Some(&firm(rfq_id, quote_id, "partially_filled", true, 4_000_000)),
            now,
        ),
        RfqV2MakerHoldEvidence::Inconclusive
    );
}

#[tokio::test]
async fn maker_hold_reconcile_treats_404_as_unknown_and_advances_its_cursor() {
    let not_found = json!({"code": "rfq_v2_not_found", "message": "gone"}).to_string();
    let (url, server) = serve_status_sequence(vec![
        (
            "200 OK",
            json!({"changes": [{"rfq_id": id(0x0a), "sequence": 15}],
                "next_after_sequence": 15, "has_more": false})
            .to_string(),
        ),
        ("404 Not Found", not_found.clone()),
        ("404 Not Found", not_found),
    ])
    .await;
    let mut hold = hold();
    let evidence = HypercallClient::new(url)
        .reconcile_rfq_v2_maker_hold(
            &mut hold,
            RfqV2ReadCredentials {
                bearer_token: "grant",
            },
        )
        .await
        .unwrap();
    assert_eq!(evidence, RfqV2MakerHoldEvidence::Missing);
    assert!(!hold.released());
    assert_eq!(hold.last_cursor, 15);
    let requests = server.await.unwrap();
    assert!(requests[0].contains("after_sequence=12"));
    assert!(requests[1].starts_with(&format!("GET /private/rfqs/{}?view=authorized ", id(0x0a))));
    assert!(requests[2].starts_with(&format!(
        "GET /private/rfqs/{}/quotes/{} ",
        id(0x0a),
        id(0x0b)
    )));
}

/// Another quote partly filled a parent whose policy then admits no further
/// fill: this unused quote can never execute, so its hold releases as a loss.
#[test]
fn maker_hold_releases_when_a_competitor_exhausts_the_parent() {
    let rfq_id = id(0x0a);
    let quote_id = id(0x0b);
    let now = 1_900_000_000_000;
    let exhausted = parent(rfq_id, "partially_filled", 4_000_000);
    assert_eq!(
        hold().evaluate(
            Some(&exhausted),
            Some(&firm(rfq_id, quote_id, "open", false, 5_000_000)),
            now,
        ),
        RfqV2MakerHoldEvidence::Conclusive(RfqV2MakerHoldProof::CompetingWinner)
    );
    // An untouched parent proves nothing about a non-executable quote.
    assert_eq!(
        hold().evaluate(
            Some(&parent(rfq_id, "open", 5_000_000)),
            Some(&firm(rfq_id, quote_id, "open", false, 5_000_000)),
            now,
        ),
        RfqV2MakerHoldEvidence::Inconclusive
    );
}

/// No change event announces an expiry, so a periodic sync rereads a cached
/// live RFQ whose signed deadline has passed even when the feed is empty.
#[tokio::test]
async fn mirror_sync_rereads_a_live_rfq_past_its_deadline_without_a_hint() {
    let rfq_id = id(0x0a);
    let expired_terms = |status: &str, remaining: u128| {
        let mut parent = parent_json(rfq_id, status, remaining);
        parent["parent"]["terms"]["expires_at_ms"] = json!(1_000);
        parent["parent"]["terms"]["quote_deadline_ms"] = json!(500);
        parent
    };
    let mut mirror = RfqV2PrivateMirror::new(RfqV2ChangeCursor::new(4));
    mirror
        .apply_refresh(
            serde_json::from_value(expired_terms("open", 5_000_000)).unwrap(),
            BTreeMap::new(),
        )
        .unwrap();
    let (url, server) = serve_sequence(vec![
        json!({"changes": [], "next_after_sequence": 4, "has_more": false}).to_string(),
        expired_terms("expired", 5_000_000).to_string(),
        quote_page(&[], None, 4),
    ])
    .await;
    let refreshed = HypercallClient::new(url)
        .sync_rfq_v2_mirror(
            &mut mirror,
            RfqV2ReadCredentials {
                bearer_token: "grant",
            },
        )
        .await
        .unwrap();
    assert_eq!(refreshed, BTreeSet::from([rfq_id]));
    assert_eq!(server.await.unwrap().len(), 3);
    assert_eq!(mirror.parent(rfq_id).unwrap().status, "expired");
}

/// The premium bound a selection enforces is the one the taker signed. A
/// caller cannot widen it, drop it, or substitute another.
#[test]
fn best_quote_criteria_take_the_auto_limit_from_the_signed_parent() {
    let rfq_id = id(0x0c);
    let limit = RfqV2AutoExecutionLimit {
        maker_receives_premium: true,
        premium_micros: 123,
    };
    let mut terms = parent_terms(rfq_id);
    terms.execution_mode = RfqV2ExecutionMode::Auto;
    terms.auto_execution_limit = Some(limit);
    let mut json = parent_json(rfq_id, "open", terms.target);
    json["parent"]["terms"] = serde_json::to_value(terms).unwrap();
    let parent: RfqV2Response = serde_json::from_value(json).unwrap();

    let criteria = crate::rfq_v2::RfqV2BestQuoteCriteria::for_parent(&parent, 1, 0);
    assert_eq!(criteria.auto_execution_limit, Some(limit));

    let manual_parent = parent_with(rfq_id, &[1_000_000], RfqV2FillPolicy::FullSizeOnce, 1, 1);
    let manual = crate::rfq_v2::RfqV2BestQuoteCriteria::for_parent(&manual_parent, 1, 0);
    assert_eq!(manual.auto_execution_limit, None);
}

/// On an automatic parent, a quote priced outside the taker's signed premium
/// bound is not an action the taker authorized, and the execution guard
/// agrees with best-quote selection on that.
#[test]
fn execution_action_enforces_the_signed_auto_limit() {
    let rfq_id = id(0x0d);
    let quote_id = id(0x0e);
    let mut terms = parent_terms(rfq_id);
    terms.execution_mode = RfqV2ExecutionMode::Auto;
    let mut quote = firm_with(
        rfq_id,
        quote_id,
        RfqV2FillPolicy::FullSizeOnce,
        5_000_000,
        5_000_000,
    );
    let RfqV2QuoteView::Firm {
        terms: quote_terms, ..
    } = &mut quote
    else {
        unreachable!()
    };
    quote_terms.maker_receives_reference_premium = true;
    quote_terms.reference_quantity = 5_000_000;
    quote_terms.reference_premium_micros = 5_000_000;
    let now = 1_900_000_000_000;

    for (premium_limit, enabled) in [(999_999, false), (1_000_000, true)] {
        terms.auto_execution_limit = Some(RfqV2AutoExecutionLimit {
            maker_receives_premium: true,
            premium_micros: premium_limit,
        });
        let mut json = parent_json(rfq_id, "open", terms.target);
        json["parent"]["terms"] = serde_json::to_value(&terms).unwrap();
        let parent: RfqV2Response = serde_json::from_value(json).unwrap();

        let action = rfq_v2_execution_action(&parent, &quote, 5_000_000, now, None);
        assert_eq!(action.enabled, enabled, "limit {premium_limit}: {action:?}");
        if !enabled {
            assert_eq!(action.block, Some(RfqV2ExecutionBlock::OutsideAutoLimit));
        }
        let selected = crate::rfq_v2::select_best_rfq_v2_quote(
            std::slice::from_ref(&quote),
            crate::rfq_v2::RfqV2BestQuoteCriteria::for_parent(&parent, 5_000_000, now),
        )
        .is_ok();
        assert_eq!(selected, enabled, "limit {premium_limit}");
    }
}
