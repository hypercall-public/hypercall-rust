//! Managed-account perp directive helpers.

use alloy::primitives::B256;
use hypercall_sdk_types::{
    DirectiveStatusResponse, DirectiveSubmitRequest, DirectiveSubmitResponse,
    HcUpdateApiWalletAction, HlCancelByCloidAction, HlCancelByOidAction, HlLimitOrderAction,
    HlSetAbstractionAction, HypercoreAccountAbstraction, PerpTimeInForce, Side,
};
use reqwest::Method;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use crate::api::HypercallClient;
use crate::error::{ClientError, Result};
use crate::wallet::{
    AccountAddress, ApiWalletDirectiveSigner, ManagerDirectiveSigner, PerpCancelByCloidSignature,
    PerpCancelByOidSignature, PerpDirectiveSigner, PerpLimitOrderSignature,
    SetAccountAbstractionSignature, UpdateApiWalletSignature,
};

const PERP_UNIT_SCALE: u64 = 100_000_000;

#[derive(Debug, Clone)]
pub struct PerpLimitOrderParams {
    pub account: AccountAddress,
    pub asset: u32,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub tif: PerpTimeInForce,
    pub reduce_only: bool,
    pub client_order_id: Option<u128>,
    pub nonce: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub struct PerpCancelByOidParams {
    pub account: AccountAddress,
    pub asset: u32,
    pub order_id: u64,
    pub nonce: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub struct PerpCancelByCloidParams {
    pub account: AccountAddress,
    pub asset: u32,
    pub client_order_id: u128,
    pub nonce: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub struct SetAccountAbstractionParams {
    pub account: AccountAddress,
    pub abstraction: HypercoreAccountAbstraction,
    pub nonce: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub struct UpdateApiWalletParams {
    /// Managed Account.sol whose API-wallet authorization will change.
    pub account: AccountAddress,
    /// bytes32 identifier for the API-wallet slot.
    pub name: B256,
    /// API wallet to add or replace. The zero address removes the named wallet.
    pub api_wallet: AccountAddress,
    /// Optional directive nonce. Omitted values use the manager signer's next nonce.
    pub nonce: Option<u64>,
}

fn to_perp_units(value: Decimal, field: &str) -> Result<u64> {
    if value <= Decimal::ZERO {
        return Err(ClientError::InvalidInput(format!(
            "{field} must be positive"
        )));
    }
    let scaled = value
        .checked_mul(Decimal::from(PERP_UNIT_SCALE))
        .ok_or_else(|| ClientError::InvalidInput(format!("{field} overflows 1e8 scaling")))?;
    if scaled.fract() != Decimal::ZERO {
        return Err(ClientError::InvalidInput(format!(
            "{field} supports at most 8 decimal places"
        )));
    }
    scaled
        .to_u64()
        .ok_or_else(|| ClientError::InvalidInput(format!("{field} exceeds u64 perp units")))
}

impl HypercallClient {
    /// Add, replace, or remove an API wallet on a managed Account.sol.
    pub async fn update_api_wallet<S>(
        &self,
        manager: &S,
        params: UpdateApiWalletParams,
    ) -> Result<DirectiveSubmitResponse>
    where
        S: ApiWalletDirectiveSigner + ?Sized,
    {
        let nonce = params.nonce.unwrap_or_else(|| manager.next_nonce());
        let action = HcUpdateApiWalletAction {
            name: params.name,
            addr: params.api_wallet.into_wallet_address(),
        };
        let signature = manager
            .sign_update_api_wallet_payload(UpdateApiWalletSignature {
                account: params.account,
                nonce,
                action,
            })
            .await?;
        self.submit_directive(
            "hc_update_api_wallet",
            params.account,
            nonce,
            action,
            signature,
        )
        .await
    }

    /// Change a managed Account's HyperCore abstraction mode.
    ///
    /// This manager-authorized directive currently supports only the forward
    /// transition to unified-account mode.
    pub async fn set_account_abstraction<S>(
        &self,
        signer: &S,
        params: SetAccountAbstractionParams,
    ) -> Result<DirectiveSubmitResponse>
    where
        S: ManagerDirectiveSigner + ?Sized,
    {
        let nonce = params.nonce.unwrap_or_else(|| signer.next_nonce());
        let action = HlSetAbstractionAction {
            user: params.account.into_wallet_address(),
            abstraction: params.abstraction,
        };
        let signature = signer
            .sign_set_account_abstraction_payload(SetAccountAbstractionSignature {
                account: params.account,
                nonce,
                action,
            })
            .await?;
        self.submit_directive(
            "hl_set_abstraction",
            params.account,
            nonce,
            action,
            signature,
        )
        .await
    }

    pub async fn place_perp_limit_order<S>(
        &self,
        signer: &S,
        params: PerpLimitOrderParams,
    ) -> Result<DirectiveSubmitResponse>
    where
        S: PerpDirectiveSigner + ?Sized,
    {
        let nonce = params.nonce.unwrap_or_else(|| signer.next_nonce());
        let action = HlLimitOrderAction {
            asset: params.asset,
            is_buy: matches!(params.side, Side::Buy),
            limit_px: to_perp_units(params.price, "perp limit price")?,
            sz: to_perp_units(params.size, "perp size")?,
            reduce_only: params.reduce_only,
            tif: params.tif,
            cloid: params.client_order_id.unwrap_or(u128::from(nonce)),
        };
        let signature = signer
            .sign_perp_limit_order_payload(PerpLimitOrderSignature {
                account: params.account,
                nonce,
                action,
            })
            .await?;
        self.submit_directive("hl_limit_order", params.account, nonce, action, signature)
            .await
    }

    pub async fn cancel_perp_order_by_oid<S>(
        &self,
        signer: &S,
        params: PerpCancelByOidParams,
    ) -> Result<DirectiveSubmitResponse>
    where
        S: PerpDirectiveSigner + ?Sized,
    {
        let nonce = params.nonce.unwrap_or_else(|| signer.next_nonce());
        let action = HlCancelByOidAction {
            asset: params.asset,
            oid: params.order_id,
        };
        let signature = signer
            .sign_perp_cancel_by_oid_payload(PerpCancelByOidSignature {
                account: params.account,
                nonce,
                action,
            })
            .await?;
        self.submit_directive("hl_cancel_by_oid", params.account, nonce, action, signature)
            .await
    }

    pub async fn cancel_perp_order_by_cloid<S>(
        &self,
        signer: &S,
        params: PerpCancelByCloidParams,
    ) -> Result<DirectiveSubmitResponse>
    where
        S: PerpDirectiveSigner + ?Sized,
    {
        let nonce = params.nonce.unwrap_or_else(|| signer.next_nonce());
        let action = HlCancelByCloidAction {
            asset: params.asset,
            cloid: params.client_order_id,
        };
        let signature = signer
            .sign_perp_cancel_by_cloid_payload(PerpCancelByCloidSignature {
                account: params.account,
                nonce,
                action,
            })
            .await?;
        self.submit_directive(
            "hl_cancel_by_cloid",
            params.account,
            nonce,
            action,
            signature,
        )
        .await
    }

    pub async fn get_directive_status(
        &self,
        directive_id: &str,
    ) -> Result<DirectiveStatusResponse> {
        let directive_id = uuid::Uuid::parse_str(directive_id).map_err(|error| {
            ClientError::InvalidInput(format!("invalid directive ID {directive_id}: {error}"))
        })?;
        let route = format!("/v1/directives/{directive_id}");
        Self::send_reqwest_json(self.request(Method::GET, &route), "get directive status").await
    }

    async fn submit_directive<T>(
        &self,
        action_key: &str,
        account: AccountAddress,
        nonce: u64,
        action: T,
        signature: String,
    ) -> Result<DirectiveSubmitResponse>
    where
        T: serde::Serialize,
    {
        let route = format!("/v1/actions/{action_key}");
        let request = DirectiveSubmitRequest {
            account: account.into_wallet_address(),
            nonce,
            action,
            signature,
        };
        Self::send_json_body(
            self.request(Method::POST, &route),
            &request,
            "submit managed account directive",
        )
        .await
    }
}

#[cfg(test)]
#[path = "perp_test.rs"]
mod tests;
