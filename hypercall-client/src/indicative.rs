//! Decoding `indicative_market_data` frames, batched or not.
#![expect(
    clippy::result_large_err,
    reason = "decoders return the crate's established unboxed public ClientError"
)]
//!
//! [`crate::WsClient::subscribe_indicative_market_data`] subscribes to
//! `indicative_market_data_v2`, whose `IndicativeMarketDataBatch` frames each
//! hold several rows oldest first. With batching disabled, or against a server
//! that predates the v2 channel, it uses `indicative_market_data`, where each
//! row arrives as its own `IndicativeMarketData` frame. The helpers here return the rows in delivery order either way, so
//! applying them in order gives the same per-instrument state regardless of
//! framing.
//!
//! ```rust
//! use hypercall_client::indicative::decode_indicative_rows;
//!
//! let frame = r#"{"type":"IndicativeMarketDataBatch","flushed_at":2,"updates":[
//!     {"instrument":"BTC-20261002-87000-C","best_bid":"14.79","best_ask":"133.88",
//!      "num_providers":1,"published_at":1,"timestamp":1}]}"#;
//! let rows = decode_indicative_rows(frame).unwrap().expect("indicative frame");
//! assert_eq!(rows[0].instrument, "BTC-20261002-87000-C");
//! ```

use crate::error::Result;
use hypercall_sdk_types::ws_protocol::{WsIndicativeMarketData, WsMessage};

/// Rows carried by one server message, or `None` if it is not indicative
/// market data.
pub fn indicative_rows(message: WsMessage) -> Option<Vec<WsIndicativeMarketData>> {
    match message {
        WsMessage::IndicativeMarketData(row) => Some(vec![row]),
        WsMessage::IndicativeMarketDataBatch(batch) => Some(batch.updates),
        _ => None,
    }
}

/// Decode one text frame and return its indicative rows, or `None` for any
/// other message type.
///
/// Frames come from the network, so they are decoded with Serde, whose
/// default recursion limit rejects hostile nesting (the repository JSON
/// policy: Serde incoming, Sonic outgoing).
pub fn decode_indicative_rows(frame: &str) -> Result<Option<Vec<WsIndicativeMarketData>>> {
    let message: WsMessage = serde_json::from_str(frame)?;
    Ok(indicative_rows(message))
}

/// [`decode_indicative_rows`] for a message already collected by
/// [`crate::WsClient`] (see [`crate::WsClient::get_messages`]).
pub fn indicative_rows_from_value(
    value: &sonic_rs::Value,
) -> Result<Option<Vec<WsIndicativeMarketData>>> {
    // Re-encode and decode through the same guarded Serde path rather than
    // converting the Sonic value directly.
    decode_indicative_rows(&sonic_rs::to_string(value)?)
}

#[cfg(test)]
#[path = "indicative_test.rs"]
mod tests;
