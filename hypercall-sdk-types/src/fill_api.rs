//! The `GET /fills` row, kept beside `api_models` so that module stays
//! within the file-size ratchet. Re-exported as `api_models::FillApiResponse`.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{InstrumentKind, ParseSdkEnumError, WalletAddress};

/// A single fill (one side of a trade) for a specific wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillApiResponse {
    /// Unique fill identifier.
    pub fill_id: i64,
    /// Parent trade identifier that produced this fill.
    pub trade_id: i64,
    /// Wallet that received this fill (checksummed Ethereum address).
    pub wallet_address: WalletAddress,
    /// Instrument symbol that was filled.
    pub symbol: String,
    /// Execution price in USD. Serialized as a string.
    pub price: Decimal,
    /// Fill size in contracts. Serialized as a string.
    pub size: Decimal,
    /// Fee charged for this fill in USD. Serialized as a string.
    pub fee: Decimal,
    /// Wallet-specific fill side.
    pub side: crate::Side,
    /// Whether this fill was on the taker side.
    pub is_taker: bool,
    /// Fill timestamp in milliseconds since epoch.
    pub timestamp: i64,
    /// When the fill was persisted (ISO 8601).
    pub created_at: DateTime<Utc>,
    /// Builder/referral code wallet, if a builder code was used (checksummed Ethereum address).
    pub builder_code_address: Option<WalletAddress>,
    /// Fee rebate to the builder code wallet in USD. Serialized as a string.
    pub builder_code_fee: Option<Decimal>,
    /// Realized PnL from this fill in USD, if a position was reduced. Serialized as a string.
    pub realized_pnl: Option<Decimal>,
    /// Link to the on-chain transaction, if settled.
    pub explorer_url: Option<String>,
    /// Instrument family that produced this fill (`"option"` or `"perp"`).
    pub instrument_type: String,
    /// RFQ V2 attribution: the RFQ, firm quote, and execution that produced
    /// this fill. Present on every RFQ V2 leg fill (maker and taker rows),
    /// omitted on order-book and V1 RFQ fills. `execution_id` matches
    /// `GET /private/rfqs/executions` and the `fills` WebSocket message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rfq_id: Option<uuid::Uuid>,
    /// The executed firm quote. See `rfq_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_id: Option<uuid::Uuid>,
    /// The RFQ V2 execution (one maker allocation). See `rfq_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<uuid::Uuid>,
}

impl FillApiResponse {
    pub fn instrument_kind(&self) -> Result<InstrumentKind, ParseSdkEnumError> {
        self.instrument_type.parse()
    }
}
