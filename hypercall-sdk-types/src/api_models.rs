//! API response types for the Hypercall REST API.
//!
//! These structs match the JSON payloads returned by public API endpoints.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{InstrumentKind, MarginMode, ParseSdkEnumError, Side, TradingModes, WalletAddress};

fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn decimal_or_zero<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Decimal, D::Error> {
    Option::<Decimal>::deserialize(d).map(|o| o.unwrap_or_default())
}

// ---- Generic API response ----

/// Generic envelope for all API responses.
///
/// Every endpoint returns `{ "success": bool, "data": T | null, "error": string | null }`.
/// Both `data` and `error` are always present in the JSON (possibly null).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    /// Whether the request succeeded.
    pub success: bool,
    /// Response payload, present on success and null on failure.
    pub data: Option<T>,
    /// Human-readable error message, present on failure and null on success.
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    /// Build a successful response wrapping `data`.
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    /// Build a successful response with no data (e.g. resource does not exist yet).
    pub fn success_empty() -> Self {
        Self {
            success: true,
            data: None,
            error: None,
        }
    }

    /// Build a failure response with the given error message.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message.into()),
        }
    }
}

// ---- Instrument status ----

/// Lifecycle state of a tradable instrument.
///
/// Serializes as `SCREAMING_SNAKE_CASE` (e.g. `"EXPIRED_PENDING_PRICE"`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InstrumentStatus {
    /// Instrument is open for trading.
    #[default]
    Active,
    /// Instrument has expired but the settlement price has not been finalized.
    ExpiredPendingPrice,
    /// Instrument has been settled and all PnL realized.
    Settled,
}

impl InstrumentStatus {
    /// Parse from a status string (case-insensitive).
    pub fn from_api_str(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "ACTIVE" => Some(Self::Active),
            "EXPIRED_PENDING_PRICE" => Some(Self::ExpiredPendingPrice),
            "SETTLED" => Some(Self::Settled),
            _ => None,
        }
    }

    /// Returns `true` if the instrument is actively trading.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }

    /// Return the canonical API string for this status.
    pub fn as_api_str(&self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::ExpiredPendingPrice => "EXPIRED_PENDING_PRICE",
            Self::Settled => "SETTLED",
        }
    }
}

impl std::fmt::Display for InstrumentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_api_str())
    }
}

// ---- Position types ----

/// A single open position for one instrument.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    /// Account wallet address (checksummed Ethereum address).
    pub wallet_address: WalletAddress,
    /// Instrument symbol (e.g. `"BTC-20260101-100000-C"`).
    pub symbol: String,
    /// Position size in contracts (positive = long, negative = short). Serialized as a string.
    pub amount: Decimal,
    /// Volume-weighted average entry price in USD. Serialized as a string.
    pub entry_price: Decimal,
    /// Margin currently locked against this position in USD. Serialized as a string.
    pub margin_posted: Decimal,
    /// Cumulative realized PnL in USD. Serialized as a string.
    pub realized_pnl: Decimal,
    /// Mark-to-market unrealized PnL in USD. Serialized as a string.
    pub unrealized_pnl: Decimal,
    /// Timestamp of the last position update (ISO 8601).
    pub updated_at: DateTime<Utc>,
}

/// Position enriched with derived risk metrics. Flattened in JSON (no nested `position` key).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionWithMetrics {
    /// Core position fields, flattened into the top-level JSON object.
    #[serde(flatten)]
    pub position: Position,
    /// Instrument family for this position (`"option"` or `"perp"`).
    pub instrument_type: String,
    /// Dollar notional value of the position. Serialized as a string.
    pub notional_value: Decimal,
    /// Maintenance margin requirement in USD. Serialized as a string.
    pub maintenance_margin: Decimal,
    /// Estimated liquidation price in USD. Serialized as a string.
    pub liquidation_price: Decimal,
    /// Margin utilization ratio (margin_used / equity). Serialized as a string.
    pub margin_ratio: Decimal,
}

impl PositionWithMetrics {
    pub fn instrument_kind(&self) -> Result<InstrumentKind, ParseSdkEnumError> {
        self.instrument_type.parse()
    }
}

/// USDC balance for a single trading account.
#[derive(Debug, Serialize, Deserialize)]
pub struct AccountBalance {
    /// Account wallet address (checksummed Ethereum address).
    pub wallet_address: WalletAddress,
    /// Current USDC balance. Serialized as a string.
    pub balance: Decimal,
    /// Timestamp of the last balance change (ISO 8601).
    pub updated_at: DateTime<Utc>,
}

// ---- Margin types ----

/// SPAN-based portfolio margin breakdown for an account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanMarginSummary {
    /// Total account equity (balance + unrealized PnL) in USD. Serialized as a string.
    pub equity: Decimal,
    /// Initial margin required for existing positions in USD. Serialized as a string.
    pub initial_margin_required: Decimal,
    /// Maintenance margin required for existing positions in USD. Serialized as a string.
    pub maintenance_margin_required: Decimal,
    /// Initial margin reserved for open (unfilled) orders. Defaults to zero. Serialized as a string.
    #[serde(default)]
    pub open_orders_initial_margin: Decimal,
    /// Options-specific margin requirement in USD. Serialized as a string.
    pub option_margin_required: Decimal,
    /// SPAN scanning risk (worst-case scenario loss) in USD. Serialized as a string.
    pub scanning_risk: Decimal,
    /// Minimum option margin floor (short option value * factor) in USD. Serialized as a string.
    pub option_floor: Decimal,
    /// Gamma/curvature overlay charge in USD. Serialized as a string.
    pub gamma_overlay: Decimal,
    /// Margin held on HyperCore for perp positions in USD. Serialized as a string.
    pub hypercore_margin_required: Decimal,
}

/// Re-export of the margin summary type from `crate::responses`.
pub use crate::responses::MarginSummary;

// ---- Portfolio ----

/// Full portfolio snapshot for a single account.
#[derive(Debug, Serialize, Deserialize)]
pub struct Portfolio {
    /// Account wallet address (checksummed Ethereum address).
    pub wallet_address: WalletAddress,
    /// All open positions with enriched risk metrics.
    pub positions: Vec<PositionWithMetrics>,
    /// Total margin currently in use across all positions in USD. Serialized as a string.
    pub total_margin_used: Decimal,
    /// Free collateral available for new trades in USD. Serialized as a string.
    pub available_balance: Decimal,
    /// Account.sol USDC currently eligible for a direct PM withdrawal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub withdrawable_usdc: Option<Decimal>,
    /// Source timestamp of the authoritative Hydromancer portfolio snapshot.
    pub portfolio_snapshot_timestamp_ms: Option<u64>,
    /// SPAN margin breakdown, present when the account uses portfolio margin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_margin: Option<SpanMarginSummary>,
    /// Margin mode for this account (`"standard"` or `"portfolio"`).
    pub margin_mode: String,
    /// Unified margin summary, present when computed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin_summary: Option<MarginSummary>,
}

impl Portfolio {
    pub fn margin_mode_kind(&self) -> Result<MarginMode, ParseSdkEnumError> {
        self.margin_mode.parse()
    }
}

// ---- Risk grid ----

/// A single SPAN risk-grid scenario with its computed PnL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskGridScenario {
    /// Unique scenario identifier (e.g. `"S1"`).
    pub id: String,
    /// Spot price shock as a fraction (e.g. `-0.15` = -15%). Serialized as a string.
    pub spot_shock_pct: Decimal,
    /// Implied volatility shock as a fraction (e.g. `0.30` = +30%). Serialized as a string.
    pub vol_shock_pct: Decimal,
    /// Weight applied to this scenario's PnL in the scanning risk calculation. Serialized as a string.
    pub pnl_weight: Decimal,
    /// Whether this is a tail/extreme scenario.
    pub is_tail: bool,
    /// Total portfolio PnL under this scenario in USD. Serialized as a string.
    pub total_pnl: Decimal,
}

/// Definition of a risk scenario (without computed PnL).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioDefinition {
    /// Unique scenario identifier.
    pub id: String,
    /// Spot price shock as a fraction. Serialized as a string.
    pub spot_shock_pct: Decimal,
    /// Implied volatility shock as a fraction. Serialized as a string.
    pub vol_shock_pct: Decimal,
    /// Weight applied to this scenario in the scanning risk calculation. Serialized as a string.
    pub pnl_weight: Decimal,
    /// Whether this is a tail/extreme scenario.
    pub is_tail: bool,
}

/// Per-instrument row in the extended risk matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstrumentRiskRowResponse {
    /// Instrument symbol.
    pub symbol: String,
    /// Underlying asset (e.g. `"BTC"`, `"ETH"`).
    pub underlying: String,
    /// Position size in contracts. Serialized as a string.
    pub amount: Decimal,
    /// Position size in base asset units. Serialized as a string.
    pub base_amount: Decimal,
    /// Current mark-to-market value of the position in USD. Serialized as a string.
    pub current_value: Decimal,
    /// PnL under each scenario, ordered to match `ExtendedRiskMatrixResponse::scenarios`. Serialized as strings.
    pub scenario_pnls: Vec<Decimal>,
}

/// Full risk matrix showing per-instrument PnL across all SPAN scenarios.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtendedRiskMatrixResponse {
    /// Scenario definitions (columns of the matrix).
    pub scenarios: Vec<ScenarioDefinition>,
    /// Per-instrument risk rows (rows of the matrix).
    pub instruments: Vec<InstrumentRiskRowResponse>,
    /// Aggregate portfolio PnL for each scenario. Serialized as strings.
    pub total_pnls: Vec<Decimal>,
    /// Index into `scenarios` of the worst-case scenario.
    pub worst_scenario_index: usize,
    /// PnL of the worst-case scenario in USD. Serialized as a string.
    pub worst_scenario_pnl: Decimal,
}

/// SPAN scenarios and aligned instrument matrix for one underlying.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnderlyingRiskGridResponse {
    /// Underlying symbol whose configured shocks produced this matrix.
    pub underlying: String,
    /// Scenario results using this underlying's exact configured shocks.
    pub scenarios: Vec<RiskGridScenario>,
    /// Per-instrument matrix aligned with `scenarios`.
    pub extended_risk_matrix: ExtendedRiskMatrixResponse,
}

/// Top-level SPAN risk grid response for an account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskGridResponse {
    /// Total account equity in USD. Serialized as a string.
    pub equity: Decimal,
    /// Initial margin for existing positions in USD. Serialized as a string.
    pub position_initial_margin: Decimal,
    /// Maintenance margin for existing positions in USD. Serialized as a string.
    pub position_maintenance_margin: Decimal,
    /// Direct additive price-band and perp-factor reservation for open orders in USD.
    /// Serialized as a string.
    pub open_orders_initial_margin: Decimal,
    /// Combined initial margin (positions + open orders) in USD. Serialized as a string.
    pub total_initial_margin: Decimal,
    /// SPAN scanning risk in USD. Serialized as a string.
    pub scanning_risk: Decimal,
    /// Option margin floor in USD. Serialized as a string.
    pub option_floor: Decimal,
    /// Gamma overlay charge in USD. Serialized as a string.
    pub gamma_overlay: Decimal,
    /// Independently configured executed-position scenario and instrument grids by underlying.
    pub underlyings: Vec<UnderlyingRiskGridResponse>,
}

// ---- Trade/Fill responses ----

/// A matched trade between a maker and a taker.
#[derive(Debug, Serialize, Deserialize)]
pub struct TradeApiResponse {
    /// Unique trade identifier.
    pub trade_id: i64,
    /// Instrument symbol that was traded.
    pub symbol: String,
    /// Execution price in USD. Serialized as a string.
    pub price: Decimal,
    /// Trade size in contracts. Serialized as a string.
    pub size: Decimal,
    /// Maker wallet address (checksummed Ethereum address).
    pub maker_address: WalletAddress,
    /// Taker wallet address (checksummed Ethereum address).
    pub taker_address: WalletAddress,
    /// Fee charged to the maker in USD. Serialized as a string.
    pub maker_fee: Decimal,
    /// Fee charged to the taker in USD. Serialized as a string.
    pub taker_fee: Decimal,
    /// Direction of the taker's fill.
    pub taker_side: Side,
    /// Trade timestamp in milliseconds since epoch.
    pub timestamp: i64,
    /// When the trade was persisted (ISO 8601).
    pub created_at: DateTime<Utc>,
}

/// Paginated list of trades.
#[derive(Debug, Serialize, Deserialize)]
pub struct TradesResponse {
    /// Whether the request succeeded.
    pub success: bool,
    /// Trade records for the current page.
    pub data: Vec<TradeApiResponse>,
    /// Pagination metadata.
    pub pagination: crate::Pagination,
}

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
}

impl FillApiResponse {
    pub fn instrument_kind(&self) -> Result<InstrumentKind, ParseSdkEnumError> {
        self.instrument_type.parse()
    }
}

/// Paginated list of fills.
#[derive(Debug, Serialize, Deserialize)]
pub struct FillsResponse {
    /// Whether the request succeeded.
    pub success: bool,
    /// Fill records for the current page.
    pub data: Vec<FillApiResponse>,
    /// Pagination metadata.
    pub pagination: crate::Pagination,
}

// ---- Order ----

/// A resting or historical order on the matching engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    /// Unique order identifier assigned by the engine.
    pub order_id: i64,
    /// Owner wallet address (checksummed Ethereum address).
    pub wallet_address: WalletAddress,
    /// Instrument symbol the order is placed on.
    pub symbol: String,
    /// Order side (`"buy"` or `"sell"`).
    pub side: String,
    /// Limit price in USD. Serialized as a string.
    pub price: Decimal,
    /// Order size in contracts. Serialized as a string.
    pub size: Decimal,
    /// Time-in-force policy (e.g. `"GTC"`, `"IOC"`, `"FOK"`), when the source exposes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tif: Option<String>,
    /// Current order status (e.g. `"open"`, `"filled"`, `"cancelled"`).
    pub status: Option<String>,
    /// Order creation timestamp in milliseconds since epoch.
    pub created_at: i64,
    /// Timestamp of the last status change (ISO 8601).
    pub updated_at: Option<DateTime<Utc>>,
    /// Cumulative filled size in contracts. Serialized as a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filled_size: Option<Decimal>,
    /// Client-assigned order identifier, when one was supplied.
    #[serde(deserialize_with = "required_option")]
    pub client_id: Option<String>,
    /// Whether the order can only reduce an existing position.
    ///
    /// Historical option records may not contain this field. `None` preserves
    /// that absence instead of inventing order intent.
    #[serde(deserialize_with = "required_option")]
    pub reduce_only: Option<bool>,
    /// Whether Market Maker Protection is enabled for this order.
    #[serde(default)]
    pub mmp_enabled: bool,
    /// Instrument family that produced this order (`"option"` or `"perp"`).
    pub instrument_type: String,
}

impl Order {
    pub fn instrument_kind(&self) -> Result<InstrumentKind, ParseSdkEnumError> {
        self.instrument_type.parse()
    }
}

/// Canonical paginated `/orders` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrdersResponse {
    pub success: bool,
    pub data: Vec<Order>,
    pub pagination: crate::Pagination,
}

/// Unambiguous root-level alias for the canonical orders response.
pub type OrdersApiResponse = OrdersResponse;

// ---- Instrument ----

/// Public representation of a tradable option instrument.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instrument {
    /// Numeric instrument identifier.
    #[serde(default)]
    pub instrument_id: i32,
    /// Human-readable instrument symbol (e.g. `"BTC-20260101-100000-C"`).
    pub id: String,
    /// Underlying asset (e.g. `"BTC"`, `"ETH"`).
    pub underlying: String,
    /// Strike price in USD. Serialized as a string.
    pub strike: Decimal,
    /// Expiry timestamp in seconds since epoch.
    pub expiry: u64,
    /// Option type: `"call"` or `"put"`.
    pub option_type: String,
    /// On-chain option token contract address, if deployed (checksummed Ethereum address).
    pub option_token_address: Option<WalletAddress>,
    /// Mark implied volatility as a decimal (e.g. `0.70` = 70%). Serialized as a string.
    pub mark_iv: Option<Decimal>,
    /// Rolling 24-hour traded volume in contracts. Serialized as a string.
    pub volume_24h: Decimal,
    /// Total open interest in contracts. Serialized as a string.
    pub open_interest: Decimal,
    /// Last time this instrument's data was refreshed (ISO 8601).
    #[serde(default = "chrono::Utc::now")]
    pub updated_at: DateTime<Utc>,
    /// Current lifecycle state of the instrument.
    #[serde(default)]
    pub status: InstrumentStatus,
    /// Trading mode flags for this instrument.
    #[serde(default)]
    pub trading_mode: TradingModes,
}

// ---- Market data ----

/// Summary of a single expiry's market data for one underlying.
#[derive(Debug, Serialize, Deserialize)]
pub struct MarketInfo {
    /// Underlying asset (e.g. `"BTC"`).
    pub underlying: String,
    /// Expiry timestamp in seconds since epoch.
    pub expiry: u64,
    /// Current spot/index price of the underlying in USD. Serialized as a string.
    /// Defaults to zero if missing or null.
    #[serde(default, deserialize_with = "decimal_or_zero")]
    pub index_price: Decimal,
    /// At-the-money implied volatility as a decimal, if available. Serialized as a string.
    pub atm_vol: Option<Decimal>,
    /// All instruments listed under this underlying/expiry pair.
    pub instruments: Vec<Instrument>,
    /// Aggregate 24-hour traded volume across all instruments in contracts. Serialized as a string.
    pub total_volume_24h: Decimal,
    /// Aggregate open interest across all instruments in contracts. Serialized as a string.
    pub total_open_interest: Decimal,
    /// Previous day's closing index price in USD, if available. Serialized as a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_day_price: Option<Decimal>,
}

/// Response containing all available markets.
#[derive(Debug, Serialize, Deserialize)]
pub struct MarketsResponse {
    /// Whether the request succeeded.
    pub success: bool,
    /// List of markets grouped by underlying and expiry.
    pub data: Vec<MarketInfo>,
}

// ---- Options chain ----

/// Absolute (per-contract) option Greeks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct OptionsChainGreeksAbs {
    /// Delta: rate of change of option price with respect to underlying price.
    pub delta: f64,
    /// Gamma: rate of change of delta with respect to underlying price.
    pub gamma: f64,
    /// Theta: time decay per day.
    pub theta: f64,
    /// Vega: sensitivity to 1-point change in implied volatility.
    pub vega: f64,
}

/// Cash-denominated option Greeks (dollar impact per unit move).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct OptionsChainGreeksCash {
    /// Dollar PnL for a 1% move in the underlying.
    pub delta_1pct_usd: f64,
    /// Dollar gamma impact for a 1% move in the underlying.
    pub gamma_1pct_usd: f64,
    /// Dollar theta decay over one day.
    pub theta_1d_usd: f64,
    /// Dollar vega for a 1-vol-point move.
    pub vega_1vol_usd: f64,
}

/// A single call or put leg in the options chain, including top-of-book quotes and Greeks.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct OptionsChainLeg {
    /// Instrument symbol (e.g. `"BTC-20260101-100000-C"`).
    pub symbol: String,
    /// On-chain option token address, if deployed (checksummed Ethereum address).
    #[cfg_attr(feature = "schemars", schemars(with = "Option<String>"))]
    pub option_token_address: Option<WalletAddress>,
    /// Best bid price in USD.
    pub bid_price_usd: Option<f64>,
    /// Implied volatility at the best bid.
    pub bid_iv: Option<f64>,
    /// Size at the best bid in contracts.
    pub bid_size_contracts: Option<f64>,
    /// Notional value of the best bid in USD.
    pub bid_size_usd_notional: Option<f64>,
    /// Best ask price in USD.
    pub ask_price_usd: Option<f64>,
    /// Implied volatility at the best ask.
    pub ask_iv: Option<f64>,
    /// Size at the best ask in contracts.
    pub ask_size_contracts: Option<f64>,
    /// Notional value of the best ask in USD.
    pub ask_size_usd_notional: Option<f64>,
    /// Per-contract (absolute) Greeks.
    pub greeks_abs: Option<OptionsChainGreeksAbs>,
    /// Cash-denominated Greeks.
    pub greeks_cash: Option<OptionsChainGreeksCash>,
}

/// A single strike row in the options chain, pairing call and put legs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct OptionsChainStrikeRow {
    /// Strike price in USD.
    pub strike: f64,
    /// Call leg at this strike, if listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call: Option<OptionsChainLeg>,
    /// Put leg at this strike, if listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub put: Option<OptionsChainLeg>,
}

/// Full options chain snapshot for one underlying and expiry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptionsChainSnapshotResponse {
    /// Underlying currency (e.g. `"BTC"`).
    pub currency: String,
    /// Expiry timestamp in seconds since epoch.
    pub expiry: u64,
    /// Strike rows ordered by strike price.
    pub rows: Vec<OptionsChainStrikeRow>,
}

// ---- Greeks ----

/// A hypothetical order used for simulating portfolio Greeks impact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulatedGreeksOrder {
    /// Instrument symbol to simulate.
    pub symbol: String,
    /// Order side (buy or sell).
    pub side: Side,
    /// Simulated order size in contracts. Serialized as a string.
    pub size: Decimal,
}

/// Greeks for a single position leg in a portfolio.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionGreeksLeg {
    /// Instrument symbol.
    pub symbol: String,
    /// Position size in contracts (positive = long, negative = short). Serialized as a string.
    pub quantity: Decimal,
    /// Delta of this leg.
    pub delta: f64,
    /// Gamma of this leg.
    pub gamma: f64,
    /// Theta (daily time decay) of this leg.
    pub theta: f64,
    /// Vega of this leg.
    pub vega: f64,
    /// Implied volatility used for this leg's Greeks.
    pub iv: f64,
}

/// Aggregate Greeks across all positions in a portfolio.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioGreeksAggregate {
    /// Net portfolio delta.
    pub delta: f64,
    /// Net portfolio gamma.
    pub gamma: f64,
    /// Net portfolio theta (daily).
    pub theta: f64,
    /// Net portfolio vega.
    pub vega: f64,
    /// Weighted-average implied volatility, if computable.
    pub iv: Option<f64>,
}

/// Full portfolio Greeks breakdown: per-leg detail and aggregate totals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioGreeksResponse {
    /// Account wallet address (checksummed Ethereum address).
    pub wallet_address: WalletAddress,
    /// Greeks for each individual position leg.
    #[serde(default)]
    pub per_leg: Vec<PositionGreeksLeg>,
    /// Aggregate Greeks across all legs, if the portfolio is non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<PortfolioGreeksAggregate>,
}

// ---- Health ----

/// Simple health-check response from `GET /health`.
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Health status string (e.g. `"ok"`).
    pub status: String,
}

/// Build version and git metadata returned by `GET /version`.
#[derive(Debug, Serialize, Deserialize)]
pub struct VersionResponse {
    /// Semantic version of the running binary.
    pub version: String,
    /// Short git commit SHA.
    pub commit: String,
    /// Git ref (branch or tag) from which the binary was built. Serialized as `"ref"`.
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// Build timestamp string.
    pub build_time: String,
    /// Chain ID used for the EIP-712 option order signing domain.
    /// May be omitted when the signing domain is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signing_chain_id: Option<u64>,
}

/// Readiness status of one service component.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReadinessComponentReport {
    /// Component name.
    pub name: String,
    /// Whether the component is ready to serve traffic.
    pub ready: bool,
    /// Optional detail string explaining the current state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Aggregated readiness probe response from `GET /ready`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ReadyResponse {
    /// Overall readiness status (e.g. `"ready"` or `"not_ready"`).
    pub status: String,
    /// Human-readable message if not all components are ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Per-component readiness reports.
    pub components: Vec<ReadinessComponentReport>,
}

// ---- MMP ----

/// Market Maker Protection (MMP) configuration for a wallet and currency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MmpConfigData {
    /// Market maker wallet address (checksummed Ethereum address).
    pub wallet_address: WalletAddress,
    /// Underlying currency this config applies to (e.g. `"BTC"`).
    pub currency: String,
    /// Rolling window length in milliseconds for fill monitoring.
    pub interval_ms: i64,
    /// Duration in milliseconds to freeze quoting after a trigger.
    pub frozen_time_ms: i64,
    /// Maximum filled quantity in contracts within the interval. Serialized as a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qty_limit: Option<Decimal>,
    /// Maximum net delta filled within the interval. Serialized as a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_limit: Option<Decimal>,
    /// Maximum net vega filled within the interval. Serialized as a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vega_limit: Option<Decimal>,
    /// Whether MMP is currently active for this wallet/currency pair.
    pub enabled: bool,
    /// Highest committed signed nonce. The next mutation must use `nonce + 1`.
    pub nonce: u64,
}

/// Signed request to create or update an MMP configuration.
#[derive(Debug, Serialize, Deserialize)]
pub struct SetMmpConfigRequest {
    /// Market maker wallet address (checksummed Ethereum address).
    pub wallet: WalletAddress,
    /// Underlying currency (e.g. `"BTC"`).
    pub currency: String,
    /// Rolling window length in milliseconds.
    pub interval_ms: i64,
    /// Freeze duration in milliseconds after a trigger.
    pub frozen_time_ms: i64,
    /// Maximum filled quantity limit. Serialized as a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qty_limit: Option<Decimal>,
    /// Maximum net delta limit. Serialized as a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta_limit: Option<Decimal>,
    /// Maximum net vega limit. Serialized as a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vega_limit: Option<Decimal>,
    /// Replay-protection nonce, exactly one greater than the committed nonce.
    pub nonce: u64,
    /// EIP-712 signature authorizing this request.
    pub signature: String,
}

/// Signed request to delete an MMP configuration.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteMmpConfigRequest {
    /// Market maker wallet address (checksummed Ethereum address).
    pub wallet: WalletAddress,
    /// Underlying currency to delete MMP config for.
    pub currency: String,
    /// Replay-protection nonce, exactly one greater than the committed nonce.
    pub nonce: u64,
    /// EIP-712 signature authorizing this request.
    pub signature: String,
}

/// Signed request to reset (unfreeze) a triggered MMP state.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResetMmpRequest {
    /// Market maker wallet address (checksummed Ethereum address).
    pub wallet: WalletAddress,
    /// Underlying currency to reset MMP for.
    pub currency: String,
    /// Replay-protection nonce, exactly one greater than the committed nonce.
    pub nonce: u64,
    /// EIP-712 signature authorizing this request.
    pub signature: String,
}

/// Response listing MMP configurations for a wallet.
#[derive(Debug, Serialize, Deserialize)]
pub struct MmpConfigResponse {
    /// Whether the request succeeded.
    pub success: bool,
    /// MMP configurations, one per currency.
    pub data: Vec<MmpConfigData>,
}

// ---- User tier ----

/// A wallet's assigned fee/rate-limit tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserTierData {
    /// Account wallet address (checksummed Ethereum address).
    pub wallet_address: WalletAddress,
    /// Tier name (e.g. `"default"`, `"mm"`, `"vip"`).
    pub tier: String,
}

/// Rate and capacity limits for a given tier.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TradingLimits {
    /// Maximum number of concurrent open orders.
    pub max_open_orders: i32,
    /// Maximum number of concurrent open positions.
    pub max_open_positions: i32,
    /// Maximum order submissions per minute.
    pub orders_per_minute: i32,
    /// Maximum order cancellations per minute.
    pub cancels_per_minute: i32,
    /// Maximum total API requests per minute.
    pub api_requests_per_minute: i32,
}

impl Default for TradingLimits {
    fn default() -> Self {
        Self {
            max_open_orders: 100,
            max_open_positions: 50,
            orders_per_minute: 60,
            cancels_per_minute: 120,
            api_requests_per_minute: 600,
        }
    }
}

/// Response containing a wallet's current tier assignment.
#[derive(Debug, Serialize, Deserialize)]
pub struct UserTierResponse {
    /// Whether the request succeeded.
    pub success: bool,
    /// Tier data for the queried wallet.
    pub data: UserTierData,
}

// ---- Margin mode ----

/// Result of a margin mode switch.
#[derive(Debug, Serialize, Deserialize)]
pub struct MarginModeResponse {
    /// Wallet address that was updated.
    pub wallet: String,
    /// New margin mode after the switch.
    pub margin_mode: String,
    /// Margin mode before the switch.
    pub previous_mode: String,
}

/// API envelope for margin mode operations.
#[derive(Debug, Serialize, Deserialize)]
pub struct MarginModeApiResponse {
    /// Whether the request succeeded.
    pub success: bool,
    /// Margin mode change details, present on success.
    pub data: Option<MarginModeResponse>,
    /// Error message, present on failure.
    pub error: Option<String>,
}

/// Realized PnL breakdown for one instrument symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealizedPnlRow {
    /// Instrument symbol.
    pub symbol: String,
    /// Total realized PnL for this symbol in USD. Serialized as a string.
    pub realized_pnl: Decimal,
    /// Number of PnL events (fills, settlements) contributing to the total.
    pub event_count: i64,
}

/// Response containing per-symbol realized PnL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealizedPnlResponse {
    /// Whether the request succeeded.
    pub success: bool,
    /// Per-symbol realized PnL rows.
    pub data: Vec<RealizedPnlRow>,
}

/// Margin statistics shown on a user's profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileMarginStats {
    /// Margin currently locked in positions in USD. Serialized as a string.
    pub in_use: Decimal,
    /// Free margin available for new trades in USD. Serialized as a string.
    pub available: Decimal,
    /// Total account equity (in_use + available) in USD. Serialized as a string.
    pub total: Decimal,
    /// Lifetime deposit total in USD. Serialized as a string.
    pub deposits: Decimal,
    /// Lifetime withdrawal total in USD. Serialized as a string.
    pub withdraws: Decimal,
}

/// PnL statistics shown on a user's profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilePnlStats {
    /// Current mark-to-market unrealized PnL in USD. Serialized as a string.
    pub unrealized: Decimal,
    /// Realized PnL over the last 24 hours in USD. Serialized as a string.
    pub pnl_24h: Decimal,
    /// Lifetime cumulative realized PnL in USD. Serialized as a string.
    pub lifetime_realized: Decimal,
}

/// Aggregated profile data for a single user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileData {
    /// Requested identity wallet address (checksummed Ethereum address).
    pub wallet: WalletAddress,
    /// Resolved trading account whose financial statistics are returned.
    pub account_wallet: WalletAddress,
    /// Display username.
    pub username: String,
    /// Timestamp when the account was first observed (ms since epoch).
    pub account_first_seen_ts_ms: Option<i64>,
    /// Number of days since the account was first seen.
    pub account_age_days: Option<i64>,
    /// Margin statistics.
    pub margin: ProfileMarginStats,
    /// PnL statistics.
    pub pnl: ProfilePnlStats,
}

/// Response containing a user's full profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileResponse {
    /// Whether the request succeeded.
    pub success: bool,
    /// Profile data.
    pub data: ProfileData,
}

/// Paginated list of a user's trade fills, shown on their profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileTradesResponse {
    /// Whether the request succeeded.
    pub success: bool,
    /// Fill records for the current page.
    pub data: Vec<FillApiResponse>,
    /// Pagination metadata.
    pub pagination: crate::Pagination,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn api_response_preserves_null_fields() {
        let resp: ApiResponse<String> = ApiResponse::error("something broke".to_string());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["success"], false);
        assert!(
            json.get("data").is_some(),
            "data field must be present (null, not omitted)"
        );
        assert!(json["data"].is_null());
        assert_eq!(json["error"], "something broke");
    }

    #[test]
    fn api_response_success_includes_data() {
        let resp = ApiResponse::success("hello".to_string());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["data"], "hello");
        assert!(json["error"].is_null());
    }

    #[test]
    fn api_response_roundtrip() {
        let resp = ApiResponse::success(42u32);
        let json_str = serde_json::to_string(&resp).unwrap();
        let parsed: ApiResponse<u32> = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.data, Some(42));
    }

    #[test]
    fn risk_grid_serializes_only_per_underlying_matrices() {
        let response = RiskGridResponse {
            equity: dec!(100),
            position_initial_margin: dec!(10),
            position_maintenance_margin: dec!(5),
            open_orders_initial_margin: dec!(2),
            total_initial_margin: dec!(12),
            scanning_risk: dec!(8),
            option_floor: dec!(1),
            gamma_overlay: dec!(0),
            underlyings: vec![
                UnderlyingRiskGridResponse {
                    underlying: "BTC".to_string(),
                    scenarios: vec![],
                    extended_risk_matrix: ExtendedRiskMatrixResponse {
                        scenarios: vec![],
                        instruments: vec![],
                        total_pnls: vec![],
                        worst_scenario_index: 0,
                        worst_scenario_pnl: dec!(0),
                    },
                },
                UnderlyingRiskGridResponse {
                    underlying: "AAPL".to_string(),
                    scenarios: vec![],
                    extended_risk_matrix: ExtendedRiskMatrixResponse {
                        scenarios: vec![],
                        instruments: vec![],
                        total_pnls: vec![],
                        worst_scenario_index: 0,
                        worst_scenario_pnl: dec!(0),
                    },
                },
            ],
        };

        let json = serde_json::to_value(response).expect("risk grid should serialize");
        assert_eq!(json["underlyings"][0]["underlying"], "BTC");
        assert_eq!(json["underlyings"][1]["underlying"], "AAPL");
        assert!(json.get("scenarios").is_none());
        assert!(json.get("extended_risk_matrix").is_none());
    }

    #[test]
    fn decimal_fields_serialize_as_strings() {
        let position = Position {
            wallet_address: WalletAddress::default(),
            symbol: "BTC-20260101-100000-C".to_string(),
            amount: dec!(1.5),
            entry_price: dec!(2500),
            margin_posted: dec!(100),
            realized_pnl: dec!(-50),
            unrealized_pnl: dec!(200),
            updated_at: chrono::Utc::now(),
        };
        let json = serde_json::to_value(&position).unwrap();
        assert_eq!(json["amount"], "1.5");
        assert_eq!(json["entry_price"], "2500");
        assert_eq!(json["realized_pnl"], "-50");
    }

    #[test]
    fn decimal_fields_deserialize_from_strings() {
        let json = serde_json::json!({
            "wallet_address": "0x0000000000000000000000000000000000000000",
            "symbol": "BTC-20260101-100000-C",
            "amount": "1.5",
            "entry_price": "2500",
            "margin_posted": "100",
            "realized_pnl": "-50",
            "unrealized_pnl": "200",
            "updated_at": "2026-01-01T00:00:00Z"
        });
        let position: Position = serde_json::from_value(json).unwrap();
        assert_eq!(position.amount, dec!(1.5));
        assert_eq!(position.entry_price, dec!(2500));
    }

    #[test]
    fn decimal_fields_deserialize_from_serde_json_value() {
        let json = serde_json::json!({
            "wallet_address": "0x0000000000000000000000000000000000000000",
            "symbol": "BTC-20260101-100000-C",
            "amount": "1.5",
            "entry_price": "2500",
            "margin_posted": "100",
            "realized_pnl": "-50",
            "unrealized_pnl": "200",
            "updated_at": "2026-01-01T00:00:00Z"
        });
        let pos: Position = serde_json::from_value(json).unwrap();
        assert_eq!(pos.amount, dec!(1.5));
        assert_eq!(pos.entry_price, dec!(2500));
    }

    #[test]
    fn position_with_metrics_flattens_position_fields() {
        let pwm = PositionWithMetrics {
            position: Position {
                wallet_address: WalletAddress::default(),
                symbol: "ETH-20260301-5000-P".to_string(),
                amount: dec!(-3),
                entry_price: dec!(150),
                margin_posted: dec!(50),
                realized_pnl: dec!(0),
                unrealized_pnl: dec!(-10),
                updated_at: chrono::Utc::now(),
            },
            instrument_type: "option".to_string(),
            notional_value: dec!(-450),
            maintenance_margin: dec!(0),
            liquidation_price: dec!(0),
            margin_ratio: dec!(0),
        };
        let json = serde_json::to_value(&pwm).unwrap();
        // Flattened: Position fields at top level
        assert_eq!(json["symbol"], "ETH-20260301-5000-P");
        assert_eq!(json["amount"], "-3");
        // PositionWithMetrics fields also at top level
        assert_eq!(json["notional_value"], "-450");
        // No nested "position" key
        assert!(json.get("position").is_none());
    }

    #[test]
    fn position_with_metrics_roundtrip_from_flat_json() {
        let json = serde_json::json!({
            "wallet_address": "0x0000000000000000000000000000000000000001",
            "symbol": "BTC-20260101-100000-C",
            "amount": "2",
            "entry_price": "3000",
            "margin_posted": "100",
            "realized_pnl": "0",
            "unrealized_pnl": "50",
            "updated_at": "2026-01-01T00:00:00Z",
            "instrument_type": "option",
            "notional_value": "6000",
            "maintenance_margin": "0",
            "liquidation_price": "0",
            "margin_ratio": "0"
        });
        let pwm: PositionWithMetrics = serde_json::from_value(json).unwrap();
        assert_eq!(pwm.position.symbol, "BTC-20260101-100000-C");
        assert_eq!(pwm.position.amount, dec!(2));
        assert_eq!(pwm.instrument_kind().unwrap(), InstrumentKind::Option);
        assert_eq!(pwm.notional_value, dec!(6000));

        let missing_kind = serde_json::json!({
            "wallet_address": "0x0000000000000000000000000000000000000001",
            "symbol": "BTC-PERP",
            "amount": "2",
            "entry_price": "3000",
            "margin_posted": "100",
            "realized_pnl": "0",
            "unrealized_pnl": "50",
            "updated_at": "2026-01-01T00:00:00Z",
            "notional_value": "6000",
            "maintenance_margin": "0",
            "liquidation_price": "0",
            "margin_ratio": "0"
        });
        assert!(serde_json::from_value::<PositionWithMetrics>(missing_kind).is_err());
    }

    #[test]
    fn instrument_status_serde_roundtrip() {
        for status in [
            InstrumentStatus::Active,
            InstrumentStatus::ExpiredPendingPrice,
            InstrumentStatus::Settled,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let parsed: InstrumentStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn instrument_status_serializes_screaming_snake() {
        assert_eq!(
            serde_json::to_string(&InstrumentStatus::ExpiredPendingPrice).unwrap(),
            "\"EXPIRED_PENDING_PRICE\""
        );
    }

    #[test]
    fn portfolio_serialization_omits_none_fields() {
        let portfolio = Portfolio {
            wallet_address: WalletAddress::default(),
            positions: vec![],
            total_margin_used: dec!(0),
            available_balance: dec!(1000),
            withdrawable_usdc: None,
            portfolio_snapshot_timestamp_ms: None,
            span_margin: None,
            margin_mode: "standard".to_string(),
            margin_summary: None,
        };
        let json = serde_json::to_value(&portfolio).unwrap();
        assert!(
            json.get("span_margin").is_none(),
            "None span_margin should be omitted"
        );
        assert!(
            json.get("margin_summary").is_none(),
            "None margin_summary should be omitted"
        );
        assert_eq!(json["margin_mode"], "standard");
    }

    #[test]
    fn portfolio_requires_margin_mode() {
        let json = serde_json::json!({
            "wallet_address": "0x0000000000000000000000000000000000000000",
            "positions": [],
            "total_margin_used": "0",
            "available_balance": "1000"
        });
        assert!(serde_json::from_value::<Portfolio>(json).is_err());
    }

    #[test]
    fn markets_response_roundtrip() {
        let resp = MarketsResponse {
            success: true,
            data: vec![MarketInfo {
                underlying: "BTC".to_string(),
                expiry: 1735689600,
                index_price: dec!(95000),
                atm_vol: Some(dec!(0.65)),
                instruments: vec![Instrument {
                    instrument_id: 1,
                    id: "BTC-20260101-100000-C".to_string(),
                    underlying: "BTC".to_string(),
                    strike: dec!(100000),
                    expiry: 1735689600,
                    option_type: "call".to_string(),
                    option_token_address: None,
                    mark_iv: Some(dec!(0.70)),
                    volume_24h: dec!(1500),
                    open_interest: dec!(25000),
                    updated_at: chrono::Utc::now(),
                    status: InstrumentStatus::Active,
                    trading_mode: TradingModes::default(),
                }],
                total_volume_24h: dec!(1500),
                total_open_interest: dec!(25000),
                prev_day_price: None,
            }],
        };
        let json_str = serde_json::to_string(&resp).unwrap();
        let parsed: MarketsResponse = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.data.len(), 1);
        assert_eq!(parsed.data[0].instruments[0].strike, dec!(100000));
        assert_eq!(
            parsed.data[0].instruments[0].status,
            InstrumentStatus::Active
        );
    }

    #[test]
    fn risk_grid_decimal_precision_preserved() {
        let scenario = RiskGridScenario {
            id: "S1".to_string(),
            spot_shock_pct: dec!(-0.15),
            vol_shock_pct: dec!(0.30),
            pnl_weight: dec!(1.00),
            is_tail: false,
            total_pnl: dec!(-1234.56789),
        };
        let json = serde_json::to_value(&scenario).unwrap();
        assert_eq!(json["spot_shock_pct"], "-0.15");
        assert_eq!(json["total_pnl"], "-1234.56789");

        let parsed: RiskGridScenario = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.total_pnl, dec!(-1234.56789));
    }

    #[test]
    fn span_margin_summary_open_orders_defaults_to_zero() {
        let json = serde_json::json!({
            "equity": "10000",
            "initial_margin_required": "1500",
            "maintenance_margin_required": "1275",
            "option_margin_required": "1500",
            "scanning_risk": "1200",
            "option_floor": "1500",
            "gamma_overlay": "250",
            "hypercore_margin_required": "0"
        });
        let summary: SpanMarginSummary = serde_json::from_value(json).unwrap();
        assert_eq!(summary.open_orders_initial_margin, dec!(0));
    }

    #[test]
    fn fill_api_response_explorer_url_omission() {
        let fill = FillApiResponse {
            fill_id: 1,
            trade_id: 100,
            wallet_address: WalletAddress::default(),
            symbol: "BTC-20260101-100000-C".to_string(),
            price: dec!(2500),
            size: dec!(1),
            fee: dec!(2.5),
            side: crate::Side::Buy,
            is_taker: true,
            timestamp: 1700000000000,
            created_at: chrono::Utc::now(),
            builder_code_address: None,
            builder_code_fee: None,
            realized_pnl: Some(dec!(150)),
            explorer_url: None,
            instrument_type: "option".to_string(),
        };
        let json = serde_json::to_value(&fill).unwrap();
        assert_eq!(json["price"], "2500");
        assert_eq!(json["realized_pnl"], "150");
    }

    #[test]
    fn order_decodes_missing_tif_with_required_instrument_type() {
        let json = serde_json::json!({
            "order_id": 42,
            "wallet_address": WalletAddress::default(),
            "symbol": "BTC-PERP",
            "side": "Buy",
            "price": "100000",
            "size": "0.5",
            "status": "open",
            "created_at": 1700000000000_i64,
            "updated_at": null,
            "client_id": "perp-42",
            "reduce_only": true,
            "mmp_enabled": false,
            "instrument_type": "perp"
        });

        let order: Order = serde_json::from_value(json).unwrap();

        assert_eq!(order.tif, None);
        assert_eq!(order.instrument_type, "perp");
    }

    #[test]
    fn order_requires_instrument_type() {
        let json = serde_json::json!({
            "order_id": 42,
            "wallet_address": WalletAddress::default(),
            "symbol": "BTC-PERP",
            "side": "Buy",
            "price": "100000",
            "size": "0.5",
            "status": "open",
            "created_at": 1700000000000_i64,
            "updated_at": null,
            "client_id": null,
            "reduce_only": false,
            "mmp_enabled": false
        });

        assert!(serde_json::from_value::<Order>(json).is_err());
    }

    #[test]
    fn order_requires_nullable_metadata_keys() {
        let order = serde_json::json!({
            "order_id": 42,
            "wallet_address": WalletAddress::default(),
            "symbol": "BTC-PERP",
            "side": "Buy",
            "price": "100000",
            "size": "0.5",
            "tif": null,
            "status": "open",
            "created_at": 1700000000000_i64,
            "updated_at": null,
            "filled_size": null,
            "client_id": null,
            "reduce_only": null,
            "mmp_enabled": false,
            "instrument_type": "perp"
        });

        for required_key in ["client_id", "reduce_only"] {
            let mut missing_key = order.clone();
            missing_key
                .as_object_mut()
                .expect("order fixture must be an object")
                .remove(required_key);
            assert!(
                serde_json::from_value::<Order>(missing_key).is_err(),
                "missing {required_key} must fail canonical order decoding"
            );
        }
    }

    #[test]
    fn fill_requires_instrument_type() {
        let json = serde_json::json!({
            "fill_id": 1,
            "trade_id": 100,
            "wallet_address": WalletAddress::default(),
            "symbol": "BTC-PERP",
            "price": "100000",
            "size": "0.5",
            "fee": "1",
            "side": "Buy",
            "is_taker": true,
            "timestamp": 1700000000000_i64,
            "created_at": chrono::Utc::now(),
            "builder_code_address": null,
            "builder_code_fee": null,
            "realized_pnl": "0",
            "explorer_url": null
        });

        assert!(serde_json::from_value::<FillApiResponse>(json).is_err());
    }

    #[test]
    fn health_response_roundtrip() {
        let resp = HealthResponse {
            status: "ok".to_string(),
        };
        let json_str = serde_json::to_string(&resp).unwrap();
        let parsed: HealthResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.status, "ok");
    }

    #[test]
    fn options_chain_leg_all_none_fields() {
        let leg = OptionsChainLeg {
            symbol: "BTC-20260101-100000-C".to_string(),
            option_token_address: None,
            bid_price_usd: None,
            bid_iv: None,
            bid_size_contracts: None,
            bid_size_usd_notional: None,
            ask_price_usd: None,
            ask_iv: None,
            ask_size_contracts: None,
            ask_size_usd_notional: None,
            greeks_abs: None,
            greeks_cash: None,
        };
        let json = serde_json::to_value(&leg).unwrap();
        assert_eq!(json["symbol"], "BTC-20260101-100000-C");
        assert!(json["bid_price_usd"].is_null());
    }

    #[test]
    fn mmp_config_data_roundtrip() {
        let config = MmpConfigData {
            wallet_address: WalletAddress::default(),
            currency: "BTC".to_string(),
            interval_ms: 5000,
            frozen_time_ms: 10000,
            qty_limit: Some(dec!(100)),
            delta_limit: None,
            vega_limit: Some(dec!(50000)),
            enabled: true,
            nonce: 42,
        };
        let json_str = serde_json::to_string(&config).unwrap();
        let parsed: MmpConfigData = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed.qty_limit, Some(dec!(100)));
        assert_eq!(parsed.delta_limit, None);
        assert!(parsed.enabled);
        assert_eq!(parsed.nonce, 42);
    }
}

/// Public exchange configuration for frontend deposit/withdraw integration.
#[derive(Debug, Serialize, Deserialize)]
pub struct ExchangeInfoResponse {
    /// Exchange contract address on HyperEVM (deposit destination).
    pub exchange_address: String,
    /// Router used for direct Account.sol deposits when PM deposits are enabled.
    pub deposit_router_address: Option<String>,
    /// Whether direct portfolio-margin deposits are enabled in this environment.
    pub portfolio_margin_deposits_enabled: bool,
    /// Chain ID for EIP-712 signing (999 = mainnet, 998 = testnet).
    pub chain_id: u64,
    /// EIP-712 signing domain info.
    pub signing_domain: SigningDomainInfo,
}

/// EIP-712 domain parameters for frontend signing.
#[derive(Debug, Serialize, Deserialize)]
pub struct SigningDomainInfo {
    /// Domain name ("Hypercall").
    pub name: String,
    /// Domain version ("1").
    pub version: String,
}
