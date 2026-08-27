//! Core enums used throughout Hypercall.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseSdkEnumError {
    kind: &'static str,
    value: String,
}

impl ParseSdkEnumError {
    fn new(kind: &'static str, value: impl Into<String>) -> Self {
        Self {
            kind,
            value: value.into(),
        }
    }
}

impl fmt::Display for ParseSdkEnumError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown {}: {}", self.kind, self.value)
    }
}

impl std::error::Error for ParseSdkEnumError {}

/// Margin regime selected for an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarginMode {
    Standard,
    Portfolio,
}

impl MarginMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Portfolio => "portfolio",
        }
    }

    pub const fn is_portfolio(self) -> bool {
        matches!(self, Self::Portfolio)
    }
}

impl fmt::Display for MarginMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for MarginMode {
    type Err = ParseSdkEnumError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "standard" => Ok(Self::Standard),
            "portfolio" => Ok(Self::Portfolio),
            _ => Err(ParseSdkEnumError::new("margin mode", value)),
        }
    }
}

/// Instrument family represented by a portfolio, order, fill, or WebSocket row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstrumentKind {
    Option,
    Perp,
}

impl InstrumentKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Option => "option",
            Self::Perp => "perp",
        }
    }
}

impl fmt::Display for InstrumentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for InstrumentKind {
    type Err = ParseSdkEnumError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "option" => Ok(Self::Option),
            "perp" => Ok(Self::Perp),
            _ => Err(ParseSdkEnumError::new("instrument kind", value)),
        }
    }
}

/// HyperCore time-in-force encoding used by managed perp directives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PerpTimeInForce {
    Alo,
    Gtc,
    Ioc,
}

#[cfg(test)]
#[path = "enums_test.rs"]
mod tests;

impl PerpTimeInForce {
    pub const fn encoded(self) -> u8 {
        match self {
            Self::Alo => 1,
            Self::Gtc => 2,
            Self::Ioc => 3,
        }
    }
}

/// Order side (buy or sell).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// Buy side
    Buy,
    /// Sell side
    Sell,
}

/// Time in force for orders.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeInForce {
    /// Good Till Cancelled
    #[serde(rename = "gtc")]
    #[default]
    GTC,
    /// Immediate or Cancel
    #[serde(rename = "ioc")]
    IOC,
    /// Fill or Kill
    #[serde(rename = "fok")]
    FOK,
}

/// Order routing preference for public order submission.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderRoute {
    /// Try eligible price improvement before falling back to the orderbook.
    #[default]
    BestExecution,
    /// Skip RPI/RFQ discovery and route directly to the orderbook.
    BookOnly,
    /// Only execute through RFQ/RPI. Do not rest or fill on the orderbook.
    RfqOnly,
}

impl OrderRoute {
    pub const fn as_signed_str(self) -> &'static str {
        match self {
            Self::BestExecution => "best_execution",
            Self::BookOnly => "book_only",
            Self::RfqOnly => "rfq_only",
        }
    }
}

/// Option type (call or put).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OptionType {
    /// Call option
    #[serde(rename = "CALL", alias = "Call", alias = "call")]
    Call,
    /// Put option
    #[serde(rename = "PUT", alias = "Put", alias = "put")]
    Put,
}

impl OptionType {
    pub const fn as_api_str(self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::Put => "put",
        }
    }

    pub const fn is_call(self) -> bool {
        matches!(self, Self::Call)
    }
}

impl fmt::Display for OptionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_api_str())
    }
}

impl FromStr for OptionType {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.eq_ignore_ascii_case("call") {
            return Ok(Self::Call);
        }
        if value.eq_ignore_ascii_case("put") {
            return Ok(Self::Put);
        }
        Err(())
    }
}

/// Order status in the order lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    /// Order acknowledged.
    #[serde(rename = "ACKED", alias = "ACK")]
    Acked,
    /// Order is open on the book
    #[serde(rename = "OPEN_ORDER", alias = "OPEN")]
    OpenOrder,
    /// Order was rejected
    #[serde(
        rename = "REJECT_ORDER",
        alias = "REJECT",
        alias = "REJECTED",
        alias = "Rejected"
    )]
    RejectOrder,
    /// Order fully filled
    #[serde(rename = "FILLED", alias = "Filled")]
    Filled,
    /// Order partially filled
    #[serde(rename = "PARTIALLY_FILLED", alias = "PartiallyFilled")]
    PartiallyFilled,
    /// Order was canceled
    #[serde(rename = "CANCELED", alias = "CANCELLED")]
    Canceled,
}

/// Order update status (used in order update messages).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderUpdateStatus {
    /// Order acknowledged.
    #[serde(rename = "ACKED")]
    Acked,
    /// Order is open on the book
    #[serde(rename = "OPEN")]
    Open,
    /// Order partially filled
    #[serde(rename = "PARTIALLY_FILLED")]
    PartiallyFilled,
    /// Order fully filled
    #[serde(rename = "FILLED")]
    Filled,
    /// Order was canceled
    #[serde(rename = "CANCELED")]
    Canceled,
    /// Order was rejected
    #[serde(rename = "REJECTED")]
    Rejected,
}

/// Trade side for trade messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradeSide {
    #[serde(rename = "BUY", alias = "Buy", alias = "buy")]
    Buy,
    #[serde(rename = "SELL", alias = "Sell", alias = "sell")]
    Sell,
}

/// Market action (create, delete, or expire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarketAction {
    #[serde(rename = "CREATE_MARKET")]
    CreateMarket,
    #[serde(rename = "DELETE_MARKET")]
    DeleteMarket,
    #[serde(rename = "EXPIRE_MARKET")]
    ExpireMarket,
}

/// Market update status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarketUpdateStatus {
    /// Market was created
    #[serde(rename = "MARKET_CREATED")]
    MarketCreated,
    /// Market already exists (idempotent - not an error)
    #[serde(rename = "MARKET_ALREADY_EXISTS")]
    MarketAlreadyExists,
    /// Market was deleted
    #[serde(rename = "MARKET_DELETED")]
    MarketDeleted,
    /// Market expired
    #[serde(rename = "MARKET_EXPIRED")]
    MarketExpired,
    /// Market transitioned to expired pending settlement (price unavailable or settlement deferred)
    #[serde(rename = "MARKET_PENDING_SETTLEMENT")]
    MarketPendingSettlement,
    /// Market creation failed
    #[serde(rename = "MARKET_CREATION_FAILED")]
    MarketCreationFailed,
    /// Market deletion failed
    #[serde(rename = "MARKET_DELETION_FAILED")]
    MarketDeletionFailed,
}

/// Order action (create, cancel, or replace).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderAction {
    #[serde(rename = "CREATE_ORDER")]
    CreateOrder,
    #[serde(rename = "CANCEL_ORDER")]
    CancelOrder,
    #[serde(rename = "REPLACE_ORDER")]
    ReplaceOrder,
}

/// Transaction status for on-chain transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionStatus {
    #[serde(rename = "PENDING")]
    Pending,
    #[serde(rename = "SUBMITTED")]
    Submitted,
    #[serde(rename = "CONFIRMED")]
    Confirmed,
    #[serde(rename = "FAILED")]
    Failed,
    #[serde(rename = "EXPIRED")]
    Expired,
}

/// RFQ lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfqStatus {
    Created,
    SentToQps,
    QuotesReceived,
    NoQuotes,
    Expired,
    Accepted,
    Executed,
    Failed,
}

impl RfqStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RfqStatus::NoQuotes | RfqStatus::Expired | RfqStatus::Executed | RfqStatus::Failed
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            RfqStatus::Created => "created",
            RfqStatus::SentToQps => "sent_to_qps",
            RfqStatus::QuotesReceived => "quotes_received",
            RfqStatus::NoQuotes => "no_quotes",
            RfqStatus::Expired => "expired",
            RfqStatus::Accepted => "accepted",
            RfqStatus::Executed => "executed",
            RfqStatus::Failed => "failed",
        }
    }
}

impl std::fmt::Display for RfqStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Quote provider status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QpStatus {
    Active,
    Suspended,
}

/// Source of a fill (orderbook matching or RFQ execution).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FillSource {
    #[default]
    Orderbook,
    Rfq {
        rfq_id: String,
        quote_id: String,
    },
}

bitflags::bitflags! {
    /// Set of trading modes enabled for an instrument. Empty means the
    /// instrument is fully disabled. New variants can be added without
    /// the combinatorial explosion of an enum.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct TradingModes: u8 {
        const ORDERBOOK = 0b0000_0001;
        const RFQ       = 0b0000_0010;
    }
}

impl Default for TradingModes {
    fn default() -> Self {
        TradingModes::ORDERBOOK
    }
}

impl TradingModes {
    pub fn allows_orderbook(&self) -> bool {
        self.contains(Self::ORDERBOOK)
    }

    pub fn allows_rfq(&self) -> bool {
        self.contains(Self::RFQ)
    }

    /// Canonical API representation: pipe-joined lowercase tokens
    /// (e.g. `"orderbook"`, `"rfq"`, `"orderbook|rfq"`, `""` for empty).
    pub fn as_api_str(&self) -> String {
        let mut parts = Vec::with_capacity(2);
        if self.contains(Self::ORDERBOOK) {
            parts.push("orderbook");
        }
        if self.contains(Self::RFQ) {
            parts.push("rfq");
        }
        parts.join("|")
    }

    /// Parse from API wire format. Accepts the canonical pipe-delimited form
    /// and older single-value aliases (`orderbook_only`, `rfq_only`, `both`).
    pub fn from_api_str(s: &str) -> Self {
        match s {
            "" => return Self::empty(),
            "orderbook_only" => return Self::ORDERBOOK,
            "rfq_only" => return Self::RFQ,
            "both" => return Self::ORDERBOOK | Self::RFQ,
            _ => {}
        }
        let mut modes = Self::empty();
        for part in s.split('|') {
            match part.trim() {
                "" => {}
                "orderbook" => modes |= Self::ORDERBOOK,
                "rfq" => modes |= Self::RFQ,
                other => {
                    // Log loudly and ignore the bad token. Boundary parsing
                    // should not crash callers.
                    tracing::error!(
                        "TradingModes::from_api_str: unknown token '{}' in '{}', \
                         ignoring; may indicate public API drift or invalid input",
                        other,
                        s
                    );
                }
            }
        }
        modes
    }
}

impl std::fmt::Display for TradingModes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_api_str())
    }
}

impl Serialize for TradingModes {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_api_str())
    }
}

impl<'de> Deserialize<'de> for TradingModes {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_api_str(&s))
    }
}
