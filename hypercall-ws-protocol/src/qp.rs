//! Quote Provider WebSocket protocol for `/ws/quotes`.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Capability string advertised in [`QpServerMessage::Authenticated`] when
/// the server accepts [`QpClientMessage::ScopedIndicativeQuoteUpdate`].
/// Clients must not send scoped updates without seeing this capability.
pub const CAP_SCOPED_INDICATIVE: &str = "scoped_indicative";

/// Opaque scope identifier for scoped indicative updates: exactly 16 bytes,
/// contents client-defined. The server never interprets the contents; it
/// only uses the id as a partition key for snapshot-omission eviction.
///
/// Wire form is a 32-character canonical lowercase hex JSON string. How a
/// client derives the bytes is its own convention: a zero-padded ASCII
/// label ([`ScopeId::from_label`]), raw UUID bytes (a UUID is already 128
/// bits), or a stable 128-bit hash of a longer name. Collisions only
/// matter within one wallet's scopes, so non-cryptographic derivations
/// are fine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeId(pub [u8; 16]);

impl ScopeId {
    /// Canonical wire form: 32 lowercase hex characters.
    pub fn to_hex(self) -> String {
        let mut out = String::with_capacity(32);
        for b in self.0 {
            use std::fmt::Write;
            write!(out, "{b:02x}").expect("writing hex to String cannot fail");
        }
        out
    }

    /// Convenience derivation: an ASCII label of at most 16 bytes,
    /// zero-padded. Returns `None` for longer labels rather than
    /// truncating, because truncation would silently collide distinct
    /// scopes.
    pub fn from_label(label: &str) -> Option<Self> {
        let bytes = label.as_bytes();
        if bytes.is_empty() || bytes.len() > 16 {
            return None;
        }
        let mut buf = [0u8; 16];
        buf[..bytes.len()].copy_from_slice(bytes);
        Some(Self(buf))
    }
}

impl std::fmt::Display for ScopeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl TryFrom<&str> for ScopeId {
    type Error = ScopeIdParseError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let bytes = s.as_bytes();
        if bytes.len() != 32 {
            return Err(ScopeIdParseError::Length(bytes.len()));
        }
        let mut buf = [0u8; 16];
        for (i, chunk) in bytes.chunks_exact(2).enumerate() {
            let hi = hex_nibble(chunk[0]).ok_or(ScopeIdParseError::NotLowercaseHex)?;
            let lo = hex_nibble(chunk[1]).ok_or(ScopeIdParseError::NotLowercaseHex)?;
            buf[i] = (hi << 4) | lo;
        }
        Ok(Self(buf))
    }
}

/// Canonical lowercase only: each scope has exactly one wire
/// representation, so byte-level dedup/compare of frames stays valid.
fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeIdParseError {
    /// Wire scope must be exactly 32 hex characters (16 bytes).
    Length(usize),
    /// Wire scope must be canonical lowercase hex.
    NotLowercaseHex,
}

impl std::fmt::Display for ScopeIdParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Length(n) => write!(f, "scope must be 32 lowercase hex chars, got length {n}"),
            Self::NotLowercaseHex => f.write_str("scope must be canonical lowercase hex"),
        }
    }
}

impl std::error::Error for ScopeIdParseError {}

impl Serialize for ScopeId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ScopeId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Cow borrows when the deserializer can lend (in-memory JSON with
        // no escapes, the hot server path) and owns otherwise (readers).
        let s = std::borrow::Cow::<str>::deserialize(deserializer)?;
        Self::try_from(s.as_ref()).map_err(serde::de::Error::custom)
    }
}

/// Gateway-authenticated reconnect request for an already-authenticated QP.
///
/// Direct public QP connections use `ConnectQuoteProvider` and strict nonce
/// checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayResumeQuoteProvider {
    pub wallet: String,
    pub timestamp: String,
    pub nonce: u64,
    pub signature: String,
}

/// Messages sent by a QP client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QpClientMessage {
    /// First frame for direct QP connections.
    ConnectQuoteProvider {
        wallet: String,
        timestamp: String,
        nonce: u64,
        signature: String,
    },
    /// First frame for gateway-managed reconnects.
    ///
    /// Public QP clients must not send it directly.
    GatewayResumeQuoteProvider {
        wallet: String,
        timestamp: String,
        nonce: u64,
        signature: String,
    },
    /// Periodic indicative quote update: a full snapshot for the wallet.
    /// Instruments this wallet previously quoted but omits here are
    /// evicted server-side.
    IndicativeQuoteUpdate { quotes: Vec<IndicativeQuote> },
    /// Scoped indicative quote update: a full snapshot of one
    /// client-defined scope (see [`ScopeId`]). Eviction-by-omission is
    /// limited to instruments this wallet previously published under the
    /// same scope; other scopes are untouched. Empty `quotes` explicitly
    /// clears the scope.
    ///
    /// Clients must only send this after the server advertised
    /// [`CAP_SCOPED_INDICATIVE`] in [`QpServerMessage::Authenticated`];
    /// older servers reject the unknown message type.
    ScopedIndicativeQuoteUpdate {
        scope: ScopeId,
        quotes: Vec<IndicativeQuote>,
    },
    /// Firm quote response to an RFQ.
    RfqResponse {
        rfq_id: String,
        /// "quote" or "decline".
        action: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        legs: Option<Vec<QpResponseLeg>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        net_premium: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        valid_for_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        nonce: Option<u64>,
    },
}

/// Messages sent to a QP client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QpServerMessage {
    Authenticated {
        wallet: String,
        /// Optional protocol capabilities this server supports (e.g.
        /// [`CAP_SCOPED_INDICATIVE`]). Absent/empty on older servers, and
        /// omitted from the wire when empty so a server advertising
        /// nothing is byte-identical to a pre-capability server. Clients
        /// must treat unknown strings as inert.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        capabilities: Vec<String>,
    },
    AuthFailed {
        reason: String,
    },
    RfqRequest {
        rfq_id: String,
        legs: Vec<QpRfqLeg>,
        taker_wallet: String,
        request_timestamp: u64,
        response_deadline_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auto_accept_limit: Option<String>,
        #[serde(default)]
        auto_execute: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        taker_limit_price: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference_price: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_improvement_tick: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auction_deadline_ms: Option<u64>,
        #[serde(default)]
        requires_price_improvement: bool,
    },
    QpMarginRejection {
        rfq_id: String,
        quote_id: String,
        reason: String,
    },
    RfqAlreadyFilled {
        rfq_id: String,
        filled_by_quote_id: String,
    },
}

/// Backwards-compatible names for API-side code.
pub type QpInboundMessage = QpClientMessage;
pub type QpOutboundMessage = QpServerMessage;

/// A single indicative quote for an instrument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndicativeQuote {
    pub instrument: String,
    pub bid_price: String,
    pub ask_price: String,
    pub max_bid_size: String,
    pub max_ask_size: String,
}

/// Borrowing view of an indicative snapshot used by allocation-sensitive ingress.
///
/// Unescaped JSON strings point into the original WebSocket frame. Escaped strings
/// fall back to an owned value so this remains wire-compatible with the ordinary
/// [`QpClientMessage`] decoder.
#[derive(Debug, PartialEq, Eq, Deserialize)]
pub struct BorrowedIndicativeQuoteUpdate<'a> {
    #[serde(rename = "type")]
    _message_type: IndicativeQuoteUpdateType,
    #[serde(borrow)]
    pub quotes: Vec<BorrowedIndicativeQuote<'a>>,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IndicativeQuoteUpdateType {
    IndicativeQuoteUpdate,
}

/// One quote whose string fields borrow from the snapshot frame when possible.
#[derive(Debug, PartialEq, Eq, Deserialize)]
pub struct BorrowedIndicativeQuote<'a> {
    #[serde(borrow)]
    pub instrument: Cow<'a, str>,
    #[serde(borrow)]
    pub bid_price: Cow<'a, str>,
    #[serde(borrow)]
    pub ask_price: Cow<'a, str>,
    #[serde(borrow)]
    pub max_bid_size: Cow<'a, str>,
    #[serde(borrow)]
    pub max_ask_size: Cow<'a, str>,
}

/// Borrowing view of a SCOPED indicative snapshot, the scoped counterpart
/// of [`BorrowedIndicativeQuoteUpdate`]. Same borrowing rules; the scope
/// id is fixed-size and always owned.
#[derive(Debug, PartialEq, Eq, Deserialize)]
pub struct BorrowedScopedIndicativeQuoteUpdate<'a> {
    #[serde(rename = "type")]
    _message_type: ScopedIndicativeQuoteUpdateType,
    pub scope: ScopeId,
    #[serde(borrow)]
    pub quotes: Vec<BorrowedIndicativeQuote<'a>>,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ScopedIndicativeQuoteUpdateType {
    ScopedIndicativeQuoteUpdate,
}

/// A leg in a QP's firm quote response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QpResponseLeg {
    pub instrument: String,
    pub side: String,
    pub price: String,
    pub size: String,
}

/// A leg in an RFQ request sent to QPs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QpRfqLeg {
    pub instrument: String,
    pub side: String,
    pub size: String,
}

#[cfg(test)]
#[path = "qp_test.rs"]
mod tests;
