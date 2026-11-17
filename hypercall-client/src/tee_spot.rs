//! TEE user spot orders: HyperCore spot orders signed by the
//! account's app key (an active Account API wallet) and placed through
//! Hypercall's TEE-held Hyperliquid API wallet, on a portfolio-margin account.
//!
//! Setup is the TEE perp setup (register the app key, enroll the TEE API
//! wallet, approve the Hypercall builder); see [`crate::tee_perp`]. Nonces
//! follow the same server-clock window.
//!
//! A buy holds its whole cost at the limit back from PM cash until the order
//! ends and a portfolio snapshot reflects it; a sell holds its coin. Proceeds
//! and bought tokens count once a snapshot shows them.

#![expect(
    clippy::result_large_err,
    reason = "these helpers preserve the established unboxed public ClientError contract"
)]

use alloy::primitives::B256;
use alloy::sol;
use alloy::sol_types::SolStruct;
use hypercall_sdk_types::{
    DirectiveSubmitRequest, PerpTimeInForce, TeeSpotCancelAction, TeeSpotOrderAction,
    TeeSpotResponse, TEE_SPOT_ASSET_OFFSET, TEE_SPOT_CANCEL_SUBMIT_ROUTE,
    TEE_SPOT_ORDER_SUBMIT_ROUTE,
};
use rust_decimal::Decimal;

use crate::api::HypercallClient;
use crate::error::{ClientError, Result};
use crate::perp::to_perp_units;
use crate::tee_perp::{nonzero_cloid, signing_domain, HYPERCALL_API_DOMAIN};
use crate::wallet::{AccountAddress, HypercallSigner, HypercallWallet};
use hypercall_sdk_types::Side;

sol! {
    struct SpotOrder {
        uint32 asset;
        bool isBuy;
        uint64 limitPx;
        uint64 sz;
        uint8 encodedTif;
        uint128 cloid;
    }

    struct HCSpotOrder {
        address account;
        uint64 nonce;
        SpotOrder action;
    }

    struct SpotCancel {
        uint32 asset;
        uint128 cloid;
    }

    struct HCSpotCancel {
        address account;
        uint64 nonce;
        SpotCancel action;
    }
}

/// What the app key signs for one spot order.
#[derive(Debug, Clone, Copy)]
pub struct TeeSpotOrderSignature {
    pub account: AccountAddress,
    pub nonce: u64,
    pub action: TeeSpotOrderAction,
}

/// What the app key signs for one spot cancel.
#[derive(Debug, Clone, Copy)]
pub struct TeeSpotCancelSignature {
    pub account: AccountAddress,
    pub nonce: u64,
    pub action: TeeSpotCancelAction,
}

fn order_message(payload: &TeeSpotOrderSignature) -> Result<HCSpotOrder> {
    let action = payload.action;
    if action.asset < TEE_SPOT_ASSET_OFFSET {
        return Err(ClientError::InvalidInput(
            "spot asset must be 10000 + pair index".into(),
        ));
    }
    let encoded_tif = match action.tif {
        PerpTimeInForce::Gtc => 2,
        PerpTimeInForce::Ioc => 3,
        PerpTimeInForce::Alo => {
            return Err(ClientError::InvalidInput(
                "spot orders are GTC or IOC".into(),
            ))
        }
    };
    Ok(HCSpotOrder {
        account: payload.account.as_wallet_address().inner(),
        nonce: payload.nonce,
        action: SpotOrder {
            asset: action.asset,
            isBuy: action.is_buy,
            limitPx: action.limit_px,
            sz: action.sz,
            encodedTif: encoded_tif,
            cloid: nonzero_cloid(action.cloid)?,
        },
    })
}

fn cancel_message(payload: &TeeSpotCancelSignature) -> Result<HCSpotCancel> {
    Ok(HCSpotCancel {
        account: payload.account.as_wallet_address().inner(),
        nonce: payload.nonce,
        action: SpotCancel {
            asset: payload.action.asset,
            cloid: nonzero_cloid(payload.action.cloid)?,
        },
    })
}

/// EIP-712 digest the app key signs for a TEE spot order.
pub fn tee_spot_order_signing_hash(chain_id: u64, payload: &TeeSpotOrderSignature) -> Result<B256> {
    Ok(order_message(payload)?
        .eip712_signing_hash(&signing_domain(HYPERCALL_API_DOMAIN, chain_id)?))
}

/// EIP-712 digest the app key signs for a TEE spot cancel.
pub fn tee_spot_cancel_signing_hash(
    chain_id: u64,
    payload: &TeeSpotCancelSignature,
) -> Result<B256> {
    Ok(cancel_message(payload)?
        .eip712_signing_hash(&signing_domain(HYPERCALL_API_DOMAIN, chain_id)?))
}

/// A key that signs TEE spot orders and cancels.
#[expect(
    async_fn_in_trait,
    reason = "signers are used generically, never as Send-bound trait objects"
)]
pub trait TeeSpotSigner: HypercallSigner {
    async fn sign_tee_spot_order_payload(&self, payload: TeeSpotOrderSignature) -> Result<String>;
    async fn sign_tee_spot_cancel_payload(&self, payload: TeeSpotCancelSignature)
        -> Result<String>;
}

impl TeeSpotSigner for HypercallWallet {
    async fn sign_tee_spot_order_payload(&self, payload: TeeSpotOrderSignature) -> Result<String> {
        let message = order_message(&payload)?;
        self.sign_directive(&message, "HCSpotOrder", HYPERCALL_API_DOMAIN)
            .await
    }

    async fn sign_tee_spot_cancel_payload(
        &self,
        payload: TeeSpotCancelSignature,
    ) -> Result<String> {
        let message = cancel_message(&payload)?;
        self.sign_directive(&message, "HCSpotCancel", HYPERCALL_API_DOMAIN)
            .await
    }
}

/// One spot limit order. `price` is USDC per base token, `size` base tokens.
#[derive(Debug, Clone)]
pub struct TeeSpotOrderParams {
    pub account: AccountAddress,
    /// HyperCore spot pair index (the asset id is `10000 + pair_index`).
    pub pair_index: u32,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    /// GTC or IOC.
    pub tif: PerpTimeInForce,
    /// Defaults to the nonce.
    pub client_order_id: Option<u128>,
    /// Defaults to [`HypercallClient::next_tee_perp_nonce`].
    pub nonce: Option<u64>,
}

/// Cancel one open spot order by its client order id.
#[derive(Debug, Clone)]
pub struct TeeSpotCancelParams {
    pub account: AccountAddress,
    pub pair_index: u32,
    pub client_order_id: u128,
    pub nonce: Option<u64>,
}

fn spot_asset(pair_index: u32) -> Result<u32> {
    TEE_SPOT_ASSET_OFFSET
        .checked_add(pair_index)
        .ok_or_else(|| ClientError::InvalidInput("spot pair index overflows".into()))
}

impl HypercallClient {
    /// Place one spot limit order through the TEE. Prices and sizes must
    /// respect the pair's HyperCore tick and lot rules.
    pub async fn place_tee_spot_order<S>(
        &self,
        signer: &S,
        params: TeeSpotOrderParams,
    ) -> Result<TeeSpotResponse>
    where
        S: TeeSpotSigner + ?Sized,
    {
        let nonce = self
            .resolve_tee_nonce(signer.address(), params.nonce)
            .await?;
        let action = TeeSpotOrderAction {
            asset: spot_asset(params.pair_index)?,
            is_buy: matches!(params.side, Side::Buy),
            limit_px: to_perp_units(params.price, "spot limit price")?,
            sz: to_perp_units(params.size, "spot size")?,
            tif: params.tif,
            cloid: nonzero_cloid(params.client_order_id.unwrap_or(u128::from(nonce)))?,
        };
        let signature = signer
            .sign_tee_spot_order_payload(TeeSpotOrderSignature {
                account: params.account,
                nonce,
                action,
            })
            .await?;
        let request = DirectiveSubmitRequest {
            account: params.account.into_wallet_address(),
            nonce,
            action,
            signature,
        };
        self.post_tee_signed(
            TEE_SPOT_ORDER_SUBMIT_ROUTE,
            &request,
            "submit TEE spot order",
        )
        .await
    }

    /// Cancel one open spot order by client order id.
    pub async fn cancel_tee_spot_order<S>(
        &self,
        signer: &S,
        params: TeeSpotCancelParams,
    ) -> Result<TeeSpotResponse>
    where
        S: TeeSpotSigner + ?Sized,
    {
        let nonce = self
            .resolve_tee_nonce(signer.address(), params.nonce)
            .await?;
        let action = TeeSpotCancelAction {
            asset: spot_asset(params.pair_index)?,
            cloid: nonzero_cloid(params.client_order_id)?,
        };
        let signature = signer
            .sign_tee_spot_cancel_payload(TeeSpotCancelSignature {
                account: params.account,
                nonce,
                action,
            })
            .await?;
        let request = DirectiveSubmitRequest {
            account: params.account.into_wallet_address(),
            nonce,
            action,
            signature,
        };
        self.post_tee_signed(
            TEE_SPOT_CANCEL_SUBMIT_ROUTE,
            &request,
            "submit TEE spot cancel",
        )
        .await
    }
}

#[cfg(test)]
#[path = "tee_spot_test.rs"]
mod tests;
