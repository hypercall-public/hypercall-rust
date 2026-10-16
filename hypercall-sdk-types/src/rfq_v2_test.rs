use std::str::FromStr;

use alloy::primitives::B256;
use serde_json::json;

use super::*;
use crate::test_wallet;

fn signature_hex() -> String {
    format!("0x{}1b", "11".repeat(64))
}

fn signature() -> RfqV2Signature {
    signature_hex().parse().unwrap()
}

#[test]
fn scheduled_quote_cancel_omits_deadline_to_remove_switch() {
    let request = RfqV2ScheduledQuoteCancelRequest {
        operation_id: Uuid::from_u128(0x5001),
        maker_account: test_wallet(1),
        cancel_at_ms: None,
        expires_at_ms: 1_900_000_020_000,
        nonce: 77,
        signature: signature(),
    };
    let value = serde_json::to_value(&request).unwrap();
    assert!(value.get("cancel_at_ms").is_none());
    assert_eq!(
        serde_json::from_value::<RfqV2ScheduledQuoteCancelRequest>(value).unwrap(),
        request
    );
}

#[test]
fn v2_signature_requires_canonical_recoverable_bytes() {
    let valid = signature_hex();
    let parsed: RfqV2Signature = valid.parse().unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), valid);
    assert_eq!(parsed.as_bytes()[64], 27);
    for invalid in [
        "0x11",
        &valid.to_uppercase(),
        &format!("0x{}ff", "11".repeat(64)),
    ] {
        assert!(serde_json::from_value::<RfqV2Signature>(json!(invalid)).is_err());
    }
}

fn id(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap()
}

#[test]
fn v2_side_accepts_only_lowercase_wire_values() {
    assert_eq!(serde_json::to_value(RfqV2Side::Buy).unwrap(), "buy");
    assert_eq!(serde_json::to_value(RfqV2Side::Sell).unwrap(), "sell");
    assert_eq!(
        serde_json::from_value::<RfqV2Side>(json!("buy")).unwrap(),
        RfqV2Side::Buy
    );
    assert!(serde_json::from_value::<RfqV2Side>(json!("Buy")).is_err());
    assert!(serde_json::from_value::<RfqV2Side>(json!("BUY")).is_err());
}

#[test]
fn selected_maker_groups_are_canonicalized_before_signing() {
    let first = test_wallet(1);
    let second = test_wallet(2);

    assert_eq!(
        RfqV2Routing::selected_makers(vec![second, first], 8).unwrap(),
        RfqV2Routing::SelectedMakers {
            makers: vec![first, second]
        }
    );
    assert_eq!(
        RfqV2Routing::selected_makers(vec![first, first], 8),
        Err(RfqV2RoutingError::Duplicate { address: first })
    );
    assert_eq!(
        RfqV2Routing::selected_makers(Vec::new(), 8),
        Err(RfqV2RoutingError::Empty)
    );
}

#[test]
fn create_request_matches_the_documented_json_contract() {
    let request = CreateRfqV2Request {
        terms: RfqV2Terms {
            id: id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce"),
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
            builder: Some(RfqV2BuilderAttribution {
                recipient: test_wallet(11),
            }),
            private_terms_blinding: B256::repeat_byte(0x8b).into(),
        },
        nonce: 42,
        signature: signature(),
    };

    let value = serde_json::to_value(request).unwrap();
    assert_eq!(value["terms"]["legs"][0]["taker_side"], "buy");
    assert_eq!(value["terms"]["routing"]["mode"], "selected_makers");
    assert_eq!(
        value["terms"]["private_terms_blinding"],
        format!("{:#x}", B256::repeat_byte(0x8b))
    );
    assert_eq!(value["terms"]["auto_execution_limit"], json!(null));
    assert_eq!(value["terms"]["target"], "5000000");
    assert_eq!(value["terms"]["fee_limits"]["per_execution"], "25000");
    assert_eq!(value["nonce"], 42);
}

#[test]
fn bytes32_values_reject_noncanonical_json() {
    assert!(serde_json::from_value::<RfqV2Digest>(json!("11")).is_err());
    assert!(serde_json::from_value::<RfqV2Digest>(json!("0x11")).is_err());
    assert!(
        serde_json::from_value::<RfqV2Digest>(json!(format!("0x{}", "AA".repeat(32)))).is_err()
    );
    assert!(
        serde_json::from_value::<RfqV2Digest>(json!(format!("0x{}", "zz".repeat(32)))).is_err()
    );

    let digest = RfqV2Digest(B256::from_str(&format!("0x{}", "11".repeat(32))).unwrap());
    assert_eq!(
        serde_json::from_value::<RfqV2Digest>(serde_json::to_value(digest).unwrap()).unwrap(),
        digest
    );
}

#[test]
fn omitted_quote_policy_is_rejected() {
    let mut terms = json!({
        "id": "f7c1ee82-c235-4bef-bc4b-af719f73d3ce",
        "taker_account": test_wallet(10),
        "legs": [{
            "instrument": "BTC-20261225-100000-C",
            "taker_side": "buy",
            "quantity_micros_per_unit": "1000000"
        }],
        "target": "1000000",
        "fill_policy": { "mode": "full_size_once" },
        "execution_mode": "manual",
        "taker_identity": "disclosed",
        "routing": { "mode": "all_eligible" },
        "quote_deadline_ms": 1900000020000_u64,
        "expires_at_ms": 1900000060000_u64,
        "fee_limits": { "per_execution": "0", "cumulative": "0" },
        "private_terms_blinding": format!("{:#x}", B256::ZERO)
    });
    terms.as_object_mut().unwrap().remove("quote_policy");

    // The policy decides whether indicative quotes are allowed. A payload
    // without it never decodes with an invented value.
    let error = serde_json::from_value::<RfqV2Terms>(terms).unwrap_err();
    assert!(error.to_string().contains("quote_policy"), "{error}");
}

#[test]
fn exact_amounts_always_use_decimal_strings() {
    let amount = 9_007_199_254_740_992_u128;
    let allocation = RfqV2SweepAllocation {
        quote_id: id("6889f9c6-cc85-40d9-86bb-df012ea74e2a"),
        package_units: amount,
    };

    let value = serde_json::to_value(allocation).unwrap();
    assert_eq!(value["package_units"], amount.to_string());
    let decoded: RfqV2SweepAllocation = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.package_units, amount);

    let small = RfqV2SweepAllocation {
        quote_id: allocation.quote_id,
        package_units: 1,
    };
    assert_eq!(serde_json::to_value(small).unwrap()["package_units"], "1");

    assert!(serde_json::from_value::<RfqV2SweepAllocation>(json!({
        "quote_id": "6889f9c6-cc85-40d9-86bb-df012ea74e2a",
        "package_units": 9_007_199_254_740_992_u64
    }))
    .is_err());
    assert!(serde_json::from_value::<RfqV2SweepAllocation>(json!({
        "quote_id": "6889f9c6-cc85-40d9-86bb-df012ea74e2a",
        "package_units": 1
    }))
    .is_err());
}

#[test]
fn authorized_parent_views_redact_private_taker_fields() {
    let maker = RfqV2AuthorizedTerms::Maker {
        terms: RfqV2MakerTerms {
            id: id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce"),
            taker_account: None,
            legs: vec![],
            target: 5_000_000,
            fill_policy: RfqV2FillPolicy::FullSizeOnce,
            execution_mode: RfqV2ExecutionMode::Manual,
            auto_execution_limit: None,
            quote_policy: RfqV2QuotePolicy::FirmOnly,
            taker_identity: RfqV2TakerIdentity::Anonymous,
            quote_deadline_ms: 1_900_000_020_000,
            expires_at_ms: 1_900_000_060_000,
            fee_limits: RfqV2FeeLimits {
                per_execution: 25_000,
                cumulative: 100_000,
            },
        },
    };
    let value = serde_json::to_value(maker).unwrap();
    assert_eq!(value["view"], "maker");
    assert!(value["terms"].get("taker_account").is_none());
    assert!(value["terms"].get("routing").is_none());
    assert!(value["terms"].get("private_terms_blinding").is_none());

    let disclosed = serde_json::from_value::<RfqV2AuthorizedTerms>(json!({
        "view": "maker",
        "terms": {
            "id": "f7c1ee82-c235-4bef-bc4b-af719f73d3ce",
            "taker_account": test_wallet(10),
            "legs": [],
            "target": "5000000",
            "fill_policy": {"mode": "full_size_once"},
            "execution_mode": "manual",
            "quote_policy": "firm_only",
            "taker_identity": "disclosed",
            "quote_deadline_ms": 1900000020000_u64,
            "expires_at_ms": 1900000060000_u64,
            "fee_limits": {"per_execution": "25000", "cumulative": "100000"}
        }
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(disclosed).unwrap()["terms"]["taker_account"],
        json!(test_wallet(10))
    );
}

#[test]
fn firm_publication_response_requires_preflight_classification() {
    let response = json!({
        "firmness": "firm",
        "quote_id": "6889f9c6-cc85-40d9-86bb-df012ea74e2a",
        "status": "open",
        "executable": true,
        "remaining_capacity": "5000000",
        "preflight": null
    });
    assert!(serde_json::from_value::<PublishRfqV2QuoteResponse>(response).is_err());
}

#[test]
fn rfq_list_role_supports_either() {
    assert_eq!(
        serde_json::to_value(RfqV2Role::Either).unwrap(),
        json!("either")
    );
}

#[test]
fn quote_reads_keep_firmness_structural() {
    let view = RfqV2QuoteView::Indicative {
        terms: RfqV2IndicativeQuoteTerms {
            id: id("169a869d-9fa7-490b-b68e-d78005740d4c"),
            rfq_id: id("f7c1ee82-c235-4bef-bc4b-af719f73d3ce"),
            rfq_terms_commitment: B256::repeat_byte(0x4a).into(),
            maker_account: test_wallet(1),
            priced_legs: vec![],
            reference_quantity: 1_000_000,
            maker_receives_reference_premium: false,
            reference_premium_micros: 0,
            indicated_capacity: 5_000_000,
            expires_at_ms: 1_900_000_015_000,
        },
        status: "open".into(),
        executable: false,
        provider: test_wallet(2),
        signer: test_wallet(3),
    };

    let value = serde_json::to_value(&view).unwrap();
    assert_eq!(value["firmness"], "indicative");
    assert_eq!(
        value["provider"],
        serde_json::to_value(test_wallet(2)).unwrap()
    );
    assert_eq!(
        value["signer"],
        serde_json::to_value(test_wallet(3)).unwrap()
    );
    assert!(value.get("remaining_capacity").is_none());
    assert_eq!(
        serde_json::from_value::<RfqV2QuoteView>(value).unwrap(),
        view
    );
}

#[test]
fn unavailable_preflight_does_not_invent_a_revision() {
    let result: RfqV2PreflightResult = serde_json::from_value(json!({
        "status": "unavailable_admitted",
        "checked_at_ms": 1_900_000_000_000_u64
    }))
    .unwrap();
    assert_eq!(result.snapshot_revision, None);
}

#[test]
fn quote_polling_query_uses_cursor_and_sequence_fields() {
    let query = ListRfqV2QuotesQuery {
        cursor: Some("opaque".into()),
        limit: Some(50),
        firmness: Some(RfqV2Firmness::Firm),
        status: Some("open".into()),
        after_sequence: Some(812_000),
        include_terminal: Some(true),
    };
    let value = serde_json::to_value(query).unwrap();
    assert_eq!(value["cursor"], "opaque");
    assert_eq!(value["firmness"], "firm");
    assert_eq!(value["after_sequence"], 812_000);
    assert_eq!(value["include_terminal"], true);
}

#[test]
fn bulk_withdrawal_scope_is_explicitly_tagged() {
    let request = RfqV2BulkWithdrawalRequest {
        operation_id: id("0b762028-6074-49ca-8881-e0f20362dd28"),
        maker_account: test_wallet(1),
        scope: RfqV2BulkWithdrawalScope::QuoteIds {
            quote_ids: vec![RfqV2QuoteRef {
                rfq_id: id("1c3bd3c6-c7c8-4ac4-a18c-ad1515ce145f"),
                quote_id: id("6889f9c6-cc85-40d9-86bb-df012ea74e2a"),
            }],
        },
        expires_at_ms: 1_900_000_030_000,
        nonce: 48,
        signature: signature(),
    };

    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(value["scope"]["mode"], "quote_ids");
    assert_eq!(
        value["scope"]["quote_ids"][0]["quote_id"],
        "6889f9c6-cc85-40d9-86bb-df012ea74e2a"
    );
    assert_eq!(
        value["scope"]["quote_ids"][0]["rfq_id"],
        "1c3bd3c6-c7c8-4ac4-a18c-ad1515ce145f"
    );
}
