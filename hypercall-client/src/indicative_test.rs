use super::{decode_indicative_rows, indicative_rows_from_value};

const ROW: &str = r#"{"instrument":"BTC-20261002-87000-C","best_bid":"14.7915","best_ask":"133.8835","indicative_bid_size":"20.161007","indicative_ask_size":"20.161007","num_providers":1,"rfq_provider_quotes":[{"wallet":"0x0000000000000000000000000000000000000001","bid_price":"14.7915","ask_price":"133.8835","max_bid_size":"20.161007","max_ask_size":"20.161007","updated_at":1790793244754}],"published_at":1790793244759,"timestamp":1790793244754}"#;

#[test]
fn unbatched_frame_yields_one_row() {
    let frame = format!(r#"{{"type":"IndicativeMarketData",{}"#, &ROW[1..]);
    let rows = decode_indicative_rows(&frame).unwrap().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].instrument, "BTC-20261002-87000-C");
}

#[test]
fn batch_frame_yields_rows_in_order() {
    let second = ROW.replace("87000", "88000");
    let frame = format!(
        r#"{{"type":"IndicativeMarketDataBatch","flushed_at":1790793244780,"updates":[{ROW},{second}]}}"#
    );
    let rows = decode_indicative_rows(&frame).unwrap().unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row.instrument.as_str())
            .collect::<Vec<_>>(),
        vec!["BTC-20261002-87000-C", "BTC-20261002-88000-C"]
    );
    assert_eq!(rows[0].rfq_provider_quotes.as_ref().map(Vec::len), Some(1));

    let value: sonic_rs::Value = sonic_rs::from_str(&frame).unwrap();
    assert_eq!(
        indicative_rows_from_value(&value).unwrap().unwrap().len(),
        2
    );
}

#[test]
fn other_messages_are_not_indicative() {
    let rows =
        decode_indicative_rows(r#"{"type":"Subscribed","channel":"indicative_market_data"}"#)
            .unwrap();
    assert!(rows.is_none());
}

#[test]
fn hostile_nesting_is_rejected_by_the_serde_recursion_guard() {
    let depth = 10_000;
    let frame = format!(
        r#"{{"type":"IndicativeMarketDataBatch","flushed_at":1,"updates":[],"extra":{}{}}}"#,
        "[".repeat(depth),
        "]".repeat(depth)
    );
    let error = decode_indicative_rows(&frame).expect_err("deep nesting must be rejected");
    let crate::ClientError::InboundJson(inner) = error else {
        panic!("expected the concrete Serde decode error, got {error:?}");
    };
    assert!(inner.to_string().contains("recursion limit"), "{inner}");
}
