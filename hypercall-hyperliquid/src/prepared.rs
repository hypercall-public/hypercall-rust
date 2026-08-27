use alloy::{
    primitives::{keccak256, Address, FixedBytes},
    signers::{Signature, Signer},
};
use hypersdk::hypercore::{
    signing::agent_signing_hash,
    types::{
        Action, ActionRequest, BatchCancelCloid, BatchOrder, CancelByCloid, OkResponse,
        OrderGrouping, OrderRequest, OrderResponseStatus, OrderTypePlacement, Response,
        TimeInForce as HyperliquidTimeInForce,
    },
    Chain, Cloid,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::Tif;

#[cfg(test)]
#[path = "prepared_test.rs"]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum PreparedHyperliquidActionError {
    #[error("invalid prepared Hyperliquid action: {0}")]
    InvalidInput(String),
    #[error("prepared Hyperliquid action signing failed: {0}")]
    Signing(String),
}

type Result<T> = std::result::Result<T, PreparedHyperliquidActionError>;
use PreparedHyperliquidActionError as ClientError;

pub const PREPARED_HYPERLIQUID_ACTION_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HyperliquidSubmissionClassification {
    Accepted,
    Rejected,
    Unknown,
}

/// Economic fields for the first direct API execution slice.
///
/// The caller resolves the asset index and quantizes price and size before
/// preparation. Once prepared, none of these fields can be rebuilt by the
/// gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPerpLimitOrder {
    pub asset: u32,
    pub is_buy: bool,
    pub limit_px: Decimal,
    pub size: Decimal,
    pub reduce_only: bool,
    pub tif: Tif,
    pub cloid: u128,
}

/// Exact venue identity for cancelling one perpetual order by client order ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPerpCancelByCloid {
    pub asset: u32,
    pub cloid: u128,
}

impl PreparedPerpLimitOrder {
    pub fn new(
        asset: u32,
        is_buy: bool,
        limit_px: Decimal,
        size: Decimal,
        tif: Tif,
        cloid: u128,
    ) -> Self {
        Self {
            asset,
            is_buy,
            limit_px,
            size,
            reduce_only: false,
            tif,
            cloid,
        }
    }

    pub fn reduce_only(mut self, reduce_only: bool) -> Self {
        self.reduce_only = reduce_only;
        self
    }
}

/// Immutable unsigned Hyperliquid request.
///
/// `canonical_bytes` is the journal and gateway transport representation.
/// The gateway must decode those exact bytes, verify canonical encoding and
/// `action_digest`, attach only the enclave signature, and submit the resulting
/// `ActionRequest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedHyperliquidAction {
    pub version: u16,
    pub chain: Chain,
    pub action: Action,
    pub nonce: u64,
    pub vault_address: Option<Address>,
    pub expires_after_ms: u64,
    pub action_digest: FixedBytes<32>,
}

impl PreparedHyperliquidAction {
    /// Classifies only the documented Hyperliquid order response shape.
    ///
    /// HTTP status alone cannot prove whether venue admission occurred. An
    /// unrecognized or contradictory response therefore remains unknown.
    pub fn classify_submission_response(body: &[u8]) -> HyperliquidSubmissionClassification {
        match serde_json::from_slice::<Response>(body) {
            Ok(Response::Err(_)) => HyperliquidSubmissionClassification::Rejected,
            Ok(Response::Ok(OkResponse::Order { statuses })) if statuses.len() == 1 => {
                match &statuses[0] {
                    OrderResponseStatus::Success
                    | OrderResponseStatus::WaitingForFill
                    | OrderResponseStatus::Resting { .. }
                    | OrderResponseStatus::Filled { .. } => {
                        HyperliquidSubmissionClassification::Accepted
                    }
                    OrderResponseStatus::Error(_) => HyperliquidSubmissionClassification::Rejected,
                    OrderResponseStatus::WaitingForTrigger => {
                        HyperliquidSubmissionClassification::Unknown
                    }
                }
            }
            Ok(Response::Ok(OkResponse::Cancel { statuses })) if statuses.len() == 1 => {
                match &statuses[0] {
                    OrderResponseStatus::Success => HyperliquidSubmissionClassification::Accepted,
                    OrderResponseStatus::Error(_) => HyperliquidSubmissionClassification::Rejected,
                    _ => HyperliquidSubmissionClassification::Unknown,
                }
            }
            _ => HyperliquidSubmissionClassification::Unknown,
        }
    }

    pub fn prepare_limit_order(
        chain: Chain,
        order: PreparedPerpLimitOrder,
        nonce: u64,
        vault_address: Option<Address>,
        expires_after_ms: u64,
    ) -> Result<Self> {
        let asset = usize::try_from(order.asset).map_err(|_| {
            ClientError::InvalidInput("Hyperliquid asset index does not fit usize".to_string())
        })?;
        let action = Action::Order(BatchOrder {
            orders: vec![OrderRequest {
                asset,
                is_buy: order.is_buy,
                limit_px: order.limit_px,
                sz: order.size,
                reduce_only: order.reduce_only,
                order_type: OrderTypePlacement::Limit {
                    tif: match order.tif {
                        Tif::Gtc => HyperliquidTimeInForce::Gtc,
                        Tif::Ioc => HyperliquidTimeInForce::Ioc,
                        Tif::Alo => {
                            return Err(ClientError::InvalidInput(
                                "ALO is not supported by the first account-action gateway slice"
                                    .to_string(),
                            ));
                        }
                    },
                },
                cloid: cloid_from_u128(order.cloid),
            }],
            grouping: OrderGrouping::Na,
            builder: None,
        });
        let action_digest = action_digest(&action, chain, nonce, vault_address, expires_after_ms)?;
        let prepared = Self {
            version: PREPARED_HYPERLIQUID_ACTION_VERSION,
            chain,
            action,
            nonce,
            vault_address,
            expires_after_ms,
            action_digest,
        };
        prepared.validate()?;
        Ok(prepared)
    }

    pub fn prepare_cancel_by_cloid(
        chain: Chain,
        cancel: PreparedPerpCancelByCloid,
        nonce: u64,
        vault_address: Option<Address>,
        expires_after_ms: u64,
    ) -> Result<Self> {
        let action = Action::CancelByCloid(BatchCancelCloid {
            cancels: vec![CancelByCloid {
                asset: cancel.asset,
                cloid: cloid_from_u128(cancel.cloid),
            }],
        });
        let action_digest = action_digest(&action, chain, nonce, vault_address, expires_after_ms)?;
        let prepared = Self {
            version: PREPARED_HYPERLIQUID_ACTION_VERSION,
            chain,
            action,
            nonce,
            vault_address,
            expires_after_ms,
            action_digest,
        };
        prepared.validate()?;
        Ok(prepared)
    }

    /// Decodes only the current canonical representation.
    ///
    /// Semantically equivalent JSON with different bytes is rejected because
    /// the engine authorization binds one exact prepared-request hash.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self> {
        let prepared: Self = serde_json::from_slice(bytes).map_err(|error| {
            ClientError::InvalidInput(format!(
                "invalid prepared Hyperliquid action encoding: {error}"
            ))
        })?;
        prepared.validate()?;
        let canonical = prepared.canonical_bytes()?;
        if canonical != bytes {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action is not canonically encoded".to_string(),
            ));
        }
        Ok(prepared)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != PREPARED_HYPERLIQUID_ACTION_VERSION {
            return Err(ClientError::InvalidInput(format!(
                "unsupported prepared Hyperliquid action version {}, expected {}",
                self.version, PREPARED_HYPERLIQUID_ACTION_VERSION
            )));
        }
        if self.nonce == 0 {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid nonce must not be zero".to_string(),
            ));
        }
        if self.expires_after_ms == 0 {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid expires_after_ms must not be zero".to_string(),
            ));
        }
        if self.expires_after_ms <= self.nonce {
            return Err(ClientError::InvalidInput(format!(
                "prepared Hyperliquid expires_after_ms {} must be after nonce {}",
                self.expires_after_ms, self.nonce
            )));
        }
        match &self.action {
            Action::Order(batch) => {
                if batch.orders.len() != 1 {
                    return Err(ClientError::InvalidInput(
                        "prepared Hyperliquid action must contain exactly one order".to_string(),
                    ));
                }
                if !matches!(batch.grouping, OrderGrouping::Na) || batch.builder.is_some() {
                    return Err(ClientError::InvalidInput(
                        "prepared Hyperliquid order must use na grouping without a builder"
                            .to_string(),
                    ));
                }
                let order = &batch.orders[0];
                if order.limit_px <= Decimal::ZERO || order.sz <= Decimal::ZERO {
                    return Err(ClientError::InvalidInput(
                        "prepared Hyperliquid price and size must be positive".to_string(),
                    ));
                }
                match order.order_type {
                    OrderTypePlacement::Limit {
                        tif: HyperliquidTimeInForce::Gtc | HyperliquidTimeInForce::Ioc,
                    } => {}
                    _ => {
                        return Err(ClientError::InvalidInput(
                            "only GTC and IOC limit orders are supported by the first account-action gateway slice"
                                .to_string(),
                        ));
                    }
                }
                if order.cloid == Cloid::ZERO {
                    return Err(ClientError::InvalidInput(
                        "prepared Hyperliquid cloid must not be zero".to_string(),
                    ));
                }
            }
            Action::CancelByCloid(batch) => {
                if batch.cancels.len() != 1 || batch.cancels[0].cloid == Cloid::ZERO {
                    return Err(ClientError::InvalidInput(
                        "prepared Hyperliquid cancel must contain exactly one non-zero cloid"
                            .to_string(),
                    ));
                }
            }
            _ => {
                return Err(ClientError::InvalidInput(
                    "only Hyperliquid perp limit orders and cancel-by-cloid actions are supported"
                        .to_string(),
                ));
            }
        }
        let expected = action_digest(
            &self.action,
            self.chain,
            self.nonce,
            self.vault_address,
            self.expires_after_ms,
        )?;
        if self.action_digest != expected {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action digest does not match frozen signing inputs"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| {
            ClientError::InvalidInput(format!(
                "failed to encode prepared Hyperliquid action: {error}"
            ))
        })
    }

    pub fn request_hash(&self) -> Result<FixedBytes<32>> {
        Ok(keccak256(self.canonical_bytes()?))
    }

    pub fn cloid(&self) -> Result<FixedBytes<16>> {
        self.validate()?;
        match &self.action {
            Action::Order(batch) => Ok(batch.orders[0].cloid),
            Action::CancelByCloid(batch) => Ok(batch.cancels[0].cloid),
            _ => unreachable!("validated prepared action kind"),
        }
    }

    /// Return the exact economic order encoded by the validated native action.
    pub fn limit_order(&self) -> Result<PreparedPerpLimitOrder> {
        self.validate()?;
        let Action::Order(batch) = &self.action else {
            return Err(ClientError::InvalidInput(
                "only Hyperliquid perp limit orders are supported".to_string(),
            ));
        };
        let order = &batch.orders[0];
        let asset = u32::try_from(order.asset).map_err(|_| {
            ClientError::InvalidInput(
                "prepared Hyperliquid asset index does not fit u32".to_string(),
            )
        })?;
        let tif = match order.order_type {
            OrderTypePlacement::Limit {
                tif: HyperliquidTimeInForce::Gtc,
            } => Tif::Gtc,
            OrderTypePlacement::Limit {
                tif: HyperliquidTimeInForce::Ioc,
            } => Tif::Ioc,
            _ => {
                return Err(ClientError::InvalidInput(
                    "only GTC and IOC limit orders are supported".to_string(),
                ));
            }
        };
        let mut cloid = [0_u8; 16];
        cloid.copy_from_slice(order.cloid.as_slice());
        Ok(PreparedPerpLimitOrder {
            asset,
            is_buy: order.is_buy,
            limit_px: order.limit_px,
            size: order.sz,
            reduce_only: order.reduce_only,
            tif,
            cloid: u128::from_be_bytes(cloid),
        })
    }

    /// Return the exact cancel identity encoded by the validated native action.
    pub fn cancel_by_cloid(&self) -> Result<PreparedPerpCancelByCloid> {
        self.validate()?;
        let Action::CancelByCloid(batch) = &self.action else {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action is not cancel-by-cloid".to_string(),
            ));
        };
        let mut cloid = [0_u8; 16];
        cloid.copy_from_slice(batch.cancels[0].cloid.as_slice());
        Ok(PreparedPerpCancelByCloid {
            asset: batch.cancels[0].asset,
            cloid: u128::from_be_bytes(cloid),
        })
    }

    /// Convenience helper for non-production tests and direct callers.
    ///
    /// Production gateway signing is performed by the enclave over the same
    /// `action_digest`, then passed to `signed_request_bytes`.
    pub async fn sign<S>(&self, signer: &S) -> Result<Signature>
    where
        S: Signer + Send + Sync,
    {
        self.validate()?;
        signer
            .sign_hash(&self.action_digest)
            .await
            .map_err(|error| {
                ClientError::Signing(format!(
                    "failed to sign prepared Hyperliquid action digest: {error}"
                ))
            })
    }

    /// Attaches a signature without rebuilding any frozen signing input.
    pub fn signed_request_bytes(
        &self,
        signature: Signature,
        expected_api_wallet: Address,
    ) -> Result<Vec<u8>> {
        self.validate()?;
        let recovered = signature
            .recover_address_from_prehash(&self.action_digest)
            .map_err(|error| {
                ClientError::Signing(format!(
                    "failed to recover prepared Hyperliquid action signer: {error}"
                ))
            })?;
        if recovered != expected_api_wallet {
            return Err(ClientError::Signing(format!(
                "prepared Hyperliquid signature recovered {recovered}, expected {expected_api_wallet}"
            )));
        }
        let request = ActionRequest {
            action: self.action.clone(),
            nonce: self.nonce,
            signature: signature.into(),
            vault_address: self.vault_address,
            expires_after: Some(self.expires_after_ms),
        };
        serde_json::to_vec(&request).map_err(|error| {
            ClientError::InvalidInput(format!(
                "failed to encode signed Hyperliquid request: {error}"
            ))
        })
    }
}

fn action_digest(
    action: &Action,
    chain: Chain,
    nonce: u64,
    vault_address: Option<Address>,
    expires_after_ms: u64,
) -> Result<FixedBytes<32>> {
    let connection_id = action
        .hash(nonce, vault_address, Some(expires_after_ms))
        .map_err(|error| {
            ClientError::InvalidInput(format!(
                "failed to hash prepared Hyperliquid action: {error}"
            ))
        })?;
    Ok(agent_signing_hash(chain, connection_id))
}

fn cloid_from_u128(cloid: u128) -> Cloid {
    Cloid::from(cloid.to_be_bytes())
}
