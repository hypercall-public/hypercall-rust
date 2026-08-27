//! API request types.

use crate::{OrderRoute, Side, TimeInForce, WalletAddress};
use serde::{Deserialize, Serialize};

pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}

/// Request body for placing an order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceOrderRequest {
    pub wallet: WalletAddress,
    /// Price as string (must match signed value exactly)
    pub price: String,
    /// Size as string (must match signed value exactly)
    pub size: String,
    pub symbol: String,
    pub side: Side,
    #[serde(default)]
    pub tif: TimeInForce,
    /// Optional signed route preference during the deprecation window.
    /// Omitted route defaults to best_execution for now. It will not be required before
    /// July 4, 2026, but may become required later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<OrderRoute>,
    pub client_id: Option<String>,
    pub nonce: u64,
    pub signature: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reduce_only: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mmp_enabled: bool,
    /// Optional builder code address for fee rebates
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builder_code_address: Option<WalletAddress>,
}

/// Request to cancel an order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelOrderRequest {
    pub wallet: WalletAddress,
    pub order_id: u64,
    pub nonce: u64,
    pub signature: String,
}

/// Request to cancel an order by client ID.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelOrderByClientIdRequest {
    pub wallet: WalletAddress,
    pub client_id: String,
    pub nonce: u64,
    pub signature: String,
}

/// Request to cancel an order by client ID through the public cloid endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CancelOrderByCloidRequest {
    pub wallet: WalletAddress,
    pub client_id: String,
    pub nonce: u64,
    pub signature: String,
}

/// Request to set margin mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetMarginModeRequest {
    pub wallet: WalletAddress,
    pub margin_mode: String,
    pub nonce: u64,
    pub signature: String,
}

/// Bulk order placement request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkPlaceOrderRequest {
    pub orders: Vec<PlaceOrderRequest>,
}

/// Bulk cancel request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkCancelOrderRequest {
    pub cancels: Vec<CancelOrderRequest>,
}

/// Request to approve an agent wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproveAgentRequest {
    /// Agent wallet address to authorize
    pub agent: WalletAddress,
    /// Nonce for replay protection
    pub nonce: u64,
    /// EIP-712 signature from wallet owner
    pub signature: String,
}

/// Request to revoke an agent wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeAgentRequest {
    /// Agent wallet address to revoke
    pub agent: WalletAddress,
    /// Nonce for replay protection
    pub nonce: u64,
    /// EIP-712 signature from wallet owner
    pub signature: String,
}

/// Request to revoke every agent wallet authorized by the signer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeAllAgentsRequest {
    /// Nonce for replay protection
    pub nonce: u64,
    /// EIP-712 signature from wallet owner
    pub signature: String,
}

/// Request to atomically cancel an existing order and place a new one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplaceOrderRequest {
    pub wallet: WalletAddress,
    /// ID of the order to cancel
    pub order_id: u64,
    /// New order price as string (must match signed value exactly)
    pub price: String,
    /// New order size as string (must match signed value exactly)
    pub size: String,
    /// New order symbol
    pub symbol: String,
    /// New order side
    pub side: Side,
    /// New order time-in-force
    #[serde(default)]
    pub tif: TimeInForce,
    /// Optional client-provided order ID for the new order
    pub client_id: Option<String>,
    pub nonce: u64,
    pub signature: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub reduce_only: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mmp_enabled: bool,
    /// Optional builder code address for fee rebates
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builder_code_address: Option<WalletAddress>,
}

// RFQ Requests

/// A single leg in an RFQ request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RfqLegRequest {
    pub instrument: String,
    pub side: Side,
    pub size: String,
}

/// Request to submit an RFQ.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitRfqRequest {
    pub rfq_id: String,
    pub legs: Vec<RfqLegRequest>,
    pub wallet_address: WalletAddress,
    pub nonce: u64,
    pub signature: String,
    /// When set, the RFQ auto-executes the first quote satisfying the taker's
    /// directional limit. Buy RFQs use this as a max debit. Sell RFQs use it
    /// as a min credit. The taker must sign the `SubmitAutoExecuteRfq`
    /// EIP-712 type instead of `SubmitRFQ`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_accept_limit: Option<String>,
}

/// Request to accept an RFQ quote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptRfqRequest {
    pub rfq_id: String,
    pub quote_id: String,
    pub wallet_address: WalletAddress,
    pub nonce: u64,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandardMarginLiquidationPositionRequest {
    pub symbol: String,
    pub quantity: String,
    pub entry_price: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandardMarginLiquidationOrderRequest {
    pub wallet: WalletAddress,
    pub liquidated_wallet: WalletAddress,
    pub request_id: uuid::Uuid,
    pub auction_id: String,
    pub bid_usdc: String,
    pub positions: Vec<StandardMarginLiquidationPositionRequest>,
    pub portfolio_hash: String,
    pub auction_terms_hash: String,
    pub auction_version: u64,
    pub valuation_timestamp_ms: u64,
    pub bid_intent_hash: String,
    pub nonce: u64,
    pub signature: String,
}
