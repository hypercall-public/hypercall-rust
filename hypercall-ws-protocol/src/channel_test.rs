use super::WsChannel;
use std::collections::HashSet;

#[test]
fn every_channel_round_trips_and_names_are_unique() {
    let mut names = HashSet::new();
    for channel in WsChannel::ALL {
        assert_eq!(WsChannel::parse(channel.as_str()), Some(channel));
        assert!(names.insert(channel.as_str()), "{channel} listed twice");
    }
}

#[test]
fn the_batched_indicative_channel_name_is_stable() {
    assert_eq!(
        WsChannel::IndicativeMarketDataV2.as_str(),
        "indicative_market_data_v2"
    );
}

#[cfg(feature = "rfq-v2")]
#[test]
fn the_private_rfq_v2_channel_is_subscribable() {
    // The API and fanout reject any name that does not parse, so a missing
    // entry silently disables RFQ V2 private hints for every client.
    assert_eq!(WsChannel::parse("rfq_v2"), Some(WsChannel::RfqV2));
}

#[cfg(not(feature = "rfq-v2"))]
#[test]
fn the_rfq_v2_channel_needs_the_preview_feature() {
    // Without `rfq-v2` a server built on this crate must not accept an RFQ V2
    // subscription it cannot serve.
    assert_eq!(WsChannel::parse("rfq_v2"), None);
}

#[test]
fn unknown_or_differently_cased_names_do_not_parse() {
    for name in [
        "",
        "orders",
        "candles:BTC:1m",
        "Orderbook",
        "competition",
        "indicative_market_data_v3",
    ] {
        assert_eq!(WsChannel::parse(name), None, "{name:?}");
    }
}
