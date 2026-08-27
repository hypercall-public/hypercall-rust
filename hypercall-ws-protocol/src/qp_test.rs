use super::{
    BorrowedIndicativeQuoteUpdate, BorrowedScopedIndicativeQuoteUpdate, IndicativeQuote,
    QpClientMessage, QpRfqLeg, QpServerMessage, ScopeId, CAP_SCOPED_INDICATIVE,
};
use std::borrow::Cow;

#[test]
fn indicative_snapshot_borrows_unescaped_strings() {
    let json = r#"{"type":"indicative_quote_update","quotes":[{"instrument":"BTC-20261225-50000-C","bid_price":"1.25","ask_price":"1.50","max_bid_size":"10","max_ask_size":"11"}]}"#;
    let snapshot: BorrowedIndicativeQuoteUpdate<'_> = serde_json::from_str(json).unwrap();
    let quote = &snapshot.quotes[0];

    assert!(matches!(quote.instrument, Cow::Borrowed(_)));
    assert!(matches!(quote.bid_price, Cow::Borrowed(_)));
    assert!(matches!(quote.ask_price, Cow::Borrowed(_)));
    assert!(matches!(quote.max_bid_size, Cow::Borrowed(_)));
    assert!(matches!(quote.max_ask_size, Cow::Borrowed(_)));
}

#[test]
fn indicative_snapshot_accepts_escaped_strings_as_owned() {
    let json = r#"{"type":"indicative_quote_update","quotes":[{"instrument":"BTC\u002d20261225-50000-C","bid_price":"1.25","ask_price":"1.50","max_bid_size":"10","max_ask_size":"11"}]}"#;
    let snapshot: BorrowedIndicativeQuoteUpdate<'_> = serde_json::from_str(json).unwrap();

    assert_eq!(snapshot.quotes[0].instrument, "BTC-20261225-50000-C");
    assert!(matches!(snapshot.quotes[0].instrument, Cow::Owned(_)));
}

#[test]
fn scoped_indicative_snapshot_borrows_and_types_scope() {
    let json = r#"{"type":"scoped_indicative_quote_update","scope":"4d534654000000000000000000000000","quotes":[{"instrument":"MSFT-20260731-450-C","bid_price":"3.0","ask_price":"3.2","max_bid_size":"1","max_ask_size":"1"}]}"#;
    let snapshot: BorrowedScopedIndicativeQuoteUpdate<'_> = serde_json::from_str(json).unwrap();

    assert_eq!(snapshot.scope, ScopeId::from_label("MSFT").unwrap());
    assert!(matches!(snapshot.quotes[0].instrument, Cow::Borrowed(_)));

    let wrong_type = r#"{"type":"indicative_quote_update","scope":"4d534654000000000000000000000000","quotes":[]}"#;
    assert!(serde_json::from_str::<BorrowedScopedIndicativeQuoteUpdate<'_>>(wrong_type).is_err());
}

#[test]
fn rfq_request_deserializes_without_auto_accept_limit() {
    let json = r#"{"type":"rfq_request","rfq_id":"abc","legs":[],"taker_wallet":"0x123","request_timestamp":1,"response_deadline_ms":5000,"auto_execute":false}"#;
    let msg: QpServerMessage = serde_json::from_str(json).unwrap();

    match msg {
        QpServerMessage::RfqRequest {
            auto_accept_limit,
            auto_execute,
            taker_limit_price,
            reference_price,
            min_improvement_tick,
            auction_deadline_ms,
            requires_price_improvement,
            ..
        } => {
            assert_eq!(auto_accept_limit, None);
            assert!(!auto_execute);
            assert_eq!(taker_limit_price, None);
            assert_eq!(reference_price, None);
            assert_eq!(min_improvement_tick, None);
            assert_eq!(auction_deadline_ms, None);
            assert!(!requires_price_improvement);
        }
        _ => panic!("expected rfq_request"),
    }
}

#[test]
fn rfq_request_serializes_auto_accept_limit_when_present() {
    let msg = QpServerMessage::RfqRequest {
        rfq_id: "abc".to_string(),
        legs: vec![QpRfqLeg {
            instrument: "BTC-20260501-90000-C".to_string(),
            side: "buy".to_string(),
            size: "1".to_string(),
        }],
        taker_wallet: "0x123".to_string(),
        request_timestamp: 1,
        response_deadline_ms: 5000,
        auto_accept_limit: Some("3999".to_string()),
        auto_execute: true,
        taker_limit_price: Some("3999".to_string()),
        reference_price: Some("3999".to_string()),
        min_improvement_tick: Some("0.0001".to_string()),
        auction_deadline_ms: Some(2000),
        requires_price_improvement: true,
    };

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""auto_accept_limit":"3999""#));
    assert!(json.contains(r#""auto_execute":true"#));
    assert!(json.contains(r#""taker_limit_price":"3999""#));
    assert!(json.contains(r#""reference_price":"3999""#));
    assert!(json.contains(r#""min_improvement_tick":"0.0001""#));
    assert!(json.contains(r#""auction_deadline_ms":2000"#));
    assert!(json.contains(r#""requires_price_improvement":true"#));
}

#[test]
fn gateway_resume_has_distinct_wire_type() {
    let msg = QpClientMessage::GatewayResumeQuoteProvider {
        wallet: "0x123".to_string(),
        timestamp: "42".to_string(),
        nonce: 7,
        signature: "0xsig".to_string(),
    };

    assert_eq!(
        serde_json::to_string(&msg).unwrap(),
        r#"{"type":"gateway_resume_quote_provider","wallet":"0x123","timestamp":"42","nonce":7,"signature":"0xsig"}"#
    );
}

// ---- ScopeId ----

#[test]
fn scope_id_hex_round_trips() {
    let id = ScopeId([
        0x53, 0x50, 0x43, 0x58, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ]);
    let hex = id.to_hex();
    assert_eq!(hex, "53504358000000000000000000000000");
    assert_eq!(ScopeId::try_from(hex.as_str()).unwrap(), id);
}

#[test]
fn scope_id_from_label_pads_and_round_trips() {
    // Client convention helper: a short ASCII label zero-padded into the
    // 16 bytes. "SPCX" and "SPCX\0..." must be the same identity.
    let id = ScopeId::from_label("SPCX").unwrap();
    assert_eq!(id.to_hex(), "53504358000000000000000000000000");
    // Too-long labels are rejected, not truncated (truncation would
    // silently collide distinct scopes).
    assert!(ScopeId::from_label("a-label-longer-than-16-bytes").is_none());
}

#[test]
fn scope_id_rejects_malformed_hex() {
    // Wrong length.
    assert!(ScopeId::try_from("abcd").is_err());
    assert!(ScopeId::try_from("").is_err());
    assert!(ScopeId::try_from("53504358000000000000000000000000ff").is_err());
    // Non-hex characters.
    assert!(ScopeId::try_from("5350435800000000000000000000000g").is_err());
    // Uppercase is rejected: the wire form is canonical lowercase so a
    // scope has exactly one wire representation.
    assert!(ScopeId::try_from("53504358000000000000000000000ABC").is_err());
}

#[test]
fn scope_id_serde_is_lowercase_hex_string() {
    let id = ScopeId::from_label("MSFT").unwrap();
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, r#""4d534654000000000000000000000000""#);
    let back: ScopeId = serde_json::from_str(&json).unwrap();
    assert_eq!(back, id);
}

#[test]
fn scope_id_deserialize_rejects_malformed() {
    assert!(serde_json::from_str::<ScopeId>(r#""short""#).is_err());
    assert!(serde_json::from_str::<ScopeId>(r#""53504358000000000000000000000ABC""#).is_err());
    assert!(serde_json::from_str::<ScopeId>("42").is_err());
}

// ---- ScopedIndicativeQuoteUpdate ----

#[test]
fn scoped_indicative_update_wire_shape() {
    let msg = QpClientMessage::ScopedIndicativeQuoteUpdate {
        scope: ScopeId::from_label("MSFT").unwrap(),
        quotes: vec![IndicativeQuote {
            instrument: "MSFT-20260731-450-C".to_string(),
            bid_price: "3.0".to_string(),
            ask_price: "3.2".to_string(),
            max_bid_size: "100000000".to_string(),
            max_ask_size: "100000000".to_string(),
        }],
    };

    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.starts_with(r#"{"type":"scoped_indicative_quote_update""#));
    assert!(json.contains(r#""scope":"4d534654000000000000000000000000""#));

    let back: QpClientMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back, msg);
}

#[test]
fn scoped_indicative_update_with_empty_quotes_round_trips() {
    // An empty scoped snapshot is meaningful: "pull this scope now".
    let msg = QpClientMessage::ScopedIndicativeQuoteUpdate {
        scope: ScopeId::from_label("SPX").unwrap(),
        quotes: Vec::new(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: QpClientMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back, msg);
}

#[test]
fn scoped_indicative_update_rejects_malformed_scope() {
    let json = r#"{"type":"scoped_indicative_quote_update","scope":"nothex","quotes":[]}"#;
    assert!(serde_json::from_str::<QpClientMessage>(json).is_err());
}

// ---- Authenticated capabilities ----

#[test]
fn authenticated_without_capabilities_defaults_empty() {
    // An old server's frame has no capabilities field; new clients must
    // read it as "no capabilities" and fall back to full snapshots.
    let json = r#"{"type":"authenticated","wallet":"0x123"}"#;
    let msg: QpServerMessage = serde_json::from_str(json).unwrap();
    match msg {
        QpServerMessage::Authenticated {
            wallet,
            capabilities,
        } => {
            assert_eq!(wallet, "0x123");
            assert!(capabilities.is_empty());
        }
        _ => panic!("expected authenticated"),
    }
}

#[test]
fn authenticated_with_capabilities_round_trips() {
    let msg = QpServerMessage::Authenticated {
        wallet: "0x123".to_string(),
        capabilities: vec![CAP_SCOPED_INDICATIVE.to_string()],
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""capabilities":["scoped_indicative"]"#));
    let back: QpServerMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back, msg);
}

#[test]
fn authenticated_empty_capabilities_omitted_on_wire() {
    // Serializing with no capabilities emits the legacy shape, so a new
    // server that advertises nothing is byte-identical to an old server.
    let msg = QpServerMessage::Authenticated {
        wallet: "0x123".to_string(),
        capabilities: Vec::new(),
    };
    assert_eq!(
        serde_json::to_string(&msg).unwrap(),
        r#"{"type":"authenticated","wallet":"0x123"}"#
    );
}
