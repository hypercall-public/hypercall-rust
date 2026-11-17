//! Managed vaults: read vaults, their settlement history and a holder's
//! position, create a vault, and queue deposits and withdrawals. Every write
//! is EIP-712 signed in the Hypercall domain by the owner or holder it names.
//!
//! Deposits and withdrawals queue and fill at the next keeper settlement, at
//! the vault's NAV. Poll [`HypercallClient::get_vault_holder`] for the fill.

#![expect(
    clippy::result_large_err,
    reason = "these helpers preserve the established unboxed public ClientError contract"
)]

use alloy::sol;
use reqwest::Method;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::api::HypercallClient;
use crate::error::{ClientError, Result};
use crate::wallet::{AccountAddress, HypercallWallet};

const API_ROUTE_VAULTS: &str = "/v1/vaults";

fn vault_route(vault_id: u64, suffix: &str) -> String {
    format!("{API_ROUTE_VAULTS}/{vault_id}{suffix}")
}

sol! {
    /// `POST /v1/vaults`, signed by the owner.
    #[derive(Debug, PartialEq, Eq)]
    struct CreateVault {
        address owner;
        uint64 salt;
        string initialSharePrice;
        string minSeedUsdc;
        string minDepositUsdc;
        string minWithdrawUsdc;
        uint64 cooldownMs;
        uint64 settleIntervalMs;
        uint64 withdrawalDeadlineMs;
        uint32 forcedUnwindSlippageBps;
        uint64 nonce;
    }

    /// `POST /v1/vaults/:id/deposit`, signed by the holder.
    #[derive(Debug, PartialEq, Eq)]
    struct VaultDeposit {
        address holder;
        uint64 vault;
        string amount;
        uint64 nonce;
    }

    /// `POST /v1/vaults/:id/withdraw`, signed by the holder.
    #[derive(Debug, PartialEq, Eq)]
    struct VaultWithdraw {
        address holder;
        uint64 vault;
        string shares;
        uint64 nonce;
    }

    /// `POST /v1/vaults/:id/cancel`, signed by the holder.
    #[derive(Debug, PartialEq, Eq)]
    struct VaultCancel {
        address holder;
        uint64 vault;
        uint64 nonce;
    }

    /// `POST /v1/vaults/:id/close`, signed by the owner.
    #[derive(Debug, PartialEq, Eq)]
    struct VaultClose {
        address owner;
        uint64 vault;
        uint64 nonce;
    }
}

/// The exact decimal string a request sends and signs: positive, normalized
/// (`60.50` signs as `60.5`).
fn signed_decimal(field: &str, value: Decimal) -> Result<String> {
    if value <= Decimal::ZERO {
        return Err(ClientError::InvalidInput(format!(
            "{field} must be positive"
        )));
    }
    Ok(value.normalize().to_string())
}

/// A vault's owner-signed terms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultConfigParams {
    pub initial_share_price: Decimal,
    pub min_seed_usdc: Decimal,
    pub min_deposit_usdc: Decimal,
    pub min_withdraw_usdc: Decimal,
    pub cooldown_ms: u64,
    pub settle_interval_ms: u64,
    pub withdrawal_deadline_ms: u64,
    pub forced_unwind_slippage_bps: u32,
}

#[derive(Debug, Serialize)]
struct VaultConfigBody {
    initial_share_price: String,
    min_seed_usdc: String,
    min_deposit_usdc: String,
    min_withdraw_usdc: String,
    cooldown_ms: u64,
    settle_interval_ms: u64,
    withdrawal_deadline_ms: u64,
    forced_unwind_slippage_bps: u32,
}

impl VaultConfigParams {
    fn body(&self) -> Result<VaultConfigBody> {
        Ok(VaultConfigBody {
            initial_share_price: signed_decimal("initial_share_price", self.initial_share_price)?,
            min_seed_usdc: signed_decimal("min_seed_usdc", self.min_seed_usdc)?,
            min_deposit_usdc: signed_decimal("min_deposit_usdc", self.min_deposit_usdc)?,
            min_withdraw_usdc: signed_decimal("min_withdraw_usdc", self.min_withdraw_usdc)?,
            cooldown_ms: self.cooldown_ms,
            settle_interval_ms: self.settle_interval_ms,
            withdrawal_deadline_ms: self.withdrawal_deadline_ms,
            forced_unwind_slippage_bps: self.forced_unwind_slippage_bps,
        })
    }
}

#[derive(Debug, Serialize)]
struct CreateVaultRequest {
    owner: AccountAddress,
    salt: u64,
    config: VaultConfigBody,
    nonce: u64,
    signature: String,
}

#[derive(Debug, Serialize)]
struct VaultHolderRequest {
    holder: AccountAddress,
    #[serde(skip_serializing_if = "Option::is_none")]
    amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shares: Option<String>,
    nonce: u64,
    signature: String,
}

/// `POST /v1/vaults` response: the derived account and the owner's
/// `VaultManager` calldata.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CreateVaultResponse {
    pub success: bool,
    pub request_id: String,
    pub vault_id: Option<u64>,
    pub account: AccountAddress,
    pub vault_manager: AccountAddress,
    /// Send with the HYPE creation deposit as `msg.value`.
    pub create_vault_calldata: String,
    /// Send once HyperCore has processed the creation deposit.
    pub activate_vault_calldata: String,
}

/// A queued deposit, withdrawal or cancel.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VaultRequestResponse {
    pub success: bool,
    pub request_id: String,
    pub vault_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VaultSettlement {
    pub seq: u64,
    pub at_ms: u64,
    pub nav: Decimal,
    pub share_price: Decimal,
    pub total_shares: Decimal,
    pub paged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VaultMetadata {
    pub name: String,
    pub description: String,
    pub website_url: Option<String>,
    pub logo_url: Option<String>,
    pub updated_at_ms: u64,
}

/// One vault, from the engine snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Vault {
    pub vault_id: u64,
    pub account: AccountAddress,
    pub owner: AccountAddress,
    /// `deploying`, `active`, `deposits_paused`, `halted`, `frozen` or
    /// `closed`.
    pub status: String,
    pub config: VaultConfigParams,
    pub total_shares: Decimal,
    pub holder_count: usize,
    pub queued_deposits: usize,
    pub queued_withdrawals: usize,
    pub last_settlement: Option<VaultSettlement>,
    pub metadata: Option<VaultMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VaultQueuedRequest {
    pub request_id: String,
    /// `deposit` or `withdrawal`.
    pub kind: String,
    /// USDC for a deposit, shares for a withdrawal.
    pub amount: Decimal,
    pub at_ms: u64,
}

/// One holder's shares and queued requests.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VaultHolder {
    pub vault_id: u64,
    pub wallet: AccountAddress,
    pub free_shares: Decimal,
    pub locked_shares: Decimal,
    pub last_deposit_fill_ms: Option<u64>,
    pub queued: Vec<VaultQueuedRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VaultSettlementsPage {
    pub vault_id: u64,
    pub settlements: Vec<VaultSettlement>,
    pub next_before_seq: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VaultFill {
    pub seq: u64,
    pub at_ms: u64,
    pub request_id: String,
    /// `redemption`, `mint`, `release` or `close_redemption`.
    pub kind: String,
    pub usdc: Decimal,
    pub shares: Decimal,
    pub share_price: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VaultHistoryPage {
    pub vault_id: u64,
    pub wallet: AccountAddress,
    pub fills: Vec<VaultFill>,
    pub next_before_seq: Option<u64>,
}

#[derive(Debug, Serialize)]
struct VaultCloseRequest {
    owner: AccountAddress,
    nonce: u64,
    signature: String,
}

/// One status transition (pause, resume, freeze, close).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VaultStatusChange {
    pub at_ms: u64,
    pub from: String,
    pub to: String,
    /// `admin`, `keeper` or `owner`.
    pub signer: String,
    pub signer_wallet: Option<AccountAddress>,
    pub cursor: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VaultStatusChangesPage {
    pub vault_id: u64,
    pub changes: Vec<VaultStatusChange>,
    /// Pass as `before` for the next page.
    pub next_before: Option<u64>,
}

/// A page of history: older than `before_seq`, at most `limit` settlements.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VaultPage {
    pub before_seq: Option<u64>,
    pub limit: Option<u32>,
}

impl VaultPage {
    fn query(&self) -> Vec<(&'static str, String)> {
        let mut query = Vec::new();
        if let Some(before_seq) = self.before_seq {
            query.push(("before_seq", before_seq.to_string()));
        }
        if let Some(limit) = self.limit {
            query.push(("limit", limit.to_string()));
        }
        query
    }
}

impl HypercallWallet {
    /// Sign a `CreateVault` for this wallet as owner.
    pub async fn sign_create_vault(
        &self,
        salt: u64,
        config: &VaultConfigParams,
        nonce: u64,
    ) -> Result<String> {
        let body = config.body()?;
        let message = CreateVault {
            owner: self.address.as_wallet_address().inner(),
            salt,
            initialSharePrice: body.initial_share_price,
            minSeedUsdc: body.min_seed_usdc,
            minDepositUsdc: body.min_deposit_usdc,
            minWithdrawUsdc: body.min_withdraw_usdc,
            cooldownMs: body.cooldown_ms,
            settleIntervalMs: body.settle_interval_ms,
            withdrawalDeadlineMs: body.withdrawal_deadline_ms,
            forcedUnwindSlippageBps: body.forced_unwind_slippage_bps,
            nonce,
        };
        self.sign_hypercall_action(&message, "CreateVault").await
    }
}

impl HypercallClient {
    /// Every vault, in id order.
    pub async fn list_vaults(&self) -> Result<Vec<Vault>> {
        Self::send_reqwest_json(self.request(Method::GET, API_ROUTE_VAULTS), "list vaults").await
    }

    /// One vault.
    pub async fn get_vault(&self, vault_id: u64) -> Result<Vault> {
        Self::send_reqwest_json(
            self.request(Method::GET, &vault_route(vault_id, "")),
            "get vault",
        )
        .await
    }

    /// One holder's shares and queued requests.
    pub async fn get_vault_holder(
        &self,
        vault_id: u64,
        wallet: AccountAddress,
    ) -> Result<VaultHolder> {
        Self::send_reqwest_json(
            self.request(Method::GET, &vault_route(vault_id, "/me"))
                .query(&[("wallet", wallet.to_string())]),
            "get vault holder",
        )
        .await
    }

    /// A vault's settlements (NAV and share price per page), newest first.
    pub async fn get_vault_settlements(
        &self,
        vault_id: u64,
        page: VaultPage,
    ) -> Result<VaultSettlementsPage> {
        Self::send_reqwest_json(
            self.request(Method::GET, &vault_route(vault_id, "/settlements"))
                .query(&page.query()),
            "get vault settlements",
        )
        .await
    }

    /// One holder's settlement fills in a vault, newest first.
    pub async fn get_vault_history(
        &self,
        vault_id: u64,
        wallet: AccountAddress,
        page: VaultPage,
    ) -> Result<VaultHistoryPage> {
        let mut query = page.query();
        query.push(("wallet", wallet.to_string()));
        Self::send_reqwest_json(
            self.request(Method::GET, &vault_route(vault_id, "/history"))
                .query(&query),
            "get vault history",
        )
        .await
    }

    /// Create a vault owned by `owner`. Send the returned calldata from the
    /// owner to `VaultManager` to deploy and then activate it.
    pub async fn create_vault(
        &self,
        owner: &HypercallWallet,
        salt: u64,
        config: &VaultConfigParams,
    ) -> Result<CreateVaultResponse> {
        let nonce = owner.next_nonce();
        let signature = owner.sign_create_vault(salt, config, nonce).await?;
        let request = CreateVaultRequest {
            owner: owner.address,
            salt,
            config: config.body()?,
            nonce,
            signature,
        };
        Self::send_json_body(
            self.request(Method::POST, API_ROUTE_VAULTS),
            &request,
            "create vault",
        )
        .await
    }

    /// Queue a deposit of `amount` USDC of the holder's free engine cash.
    pub async fn deposit_to_vault(
        &self,
        holder: &HypercallWallet,
        vault_id: u64,
        amount: Decimal,
    ) -> Result<VaultRequestResponse> {
        let amount = signed_decimal("amount", amount)?;
        let nonce = holder.next_nonce();
        let message = VaultDeposit {
            holder: holder.address.as_wallet_address().inner(),
            vault: vault_id,
            amount: amount.clone(),
            nonce,
        };
        let signature = holder
            .sign_hypercall_action(&message, "VaultDeposit")
            .await?;
        self.post_holder_request(
            vault_id,
            "/deposit",
            VaultHolderRequest {
                holder: holder.address,
                amount: Some(amount),
                shares: None,
                nonce,
                signature,
            },
        )
        .await
    }

    /// Queue a withdrawal of `shares`.
    pub async fn withdraw_from_vault(
        &self,
        holder: &HypercallWallet,
        vault_id: u64,
        shares: Decimal,
    ) -> Result<VaultRequestResponse> {
        let shares = signed_decimal("shares", shares)?;
        let nonce = holder.next_nonce();
        let message = VaultWithdraw {
            holder: holder.address.as_wallet_address().inner(),
            vault: vault_id,
            shares: shares.clone(),
            nonce,
        };
        let signature = holder
            .sign_hypercall_action(&message, "VaultWithdraw")
            .await?;
        self.post_holder_request(
            vault_id,
            "/withdraw",
            VaultHolderRequest {
                holder: holder.address,
                amount: None,
                shares: Some(shares),
                nonce,
                signature,
            },
        )
        .await
    }

    /// A vault's status transitions, newest first. `page.before_seq` is a
    /// previous page's `next_before`; this route names it `before`.
    pub async fn get_vault_status_changes(
        &self,
        vault_id: u64,
        page: VaultPage,
    ) -> Result<VaultStatusChangesPage> {
        let mut query = Vec::new();
        if let Some(before) = page.before_seq {
            query.push(("before", before.to_string()));
        }
        if let Some(limit) = page.limit {
            query.push(("limit", limit.to_string()));
        }
        Self::send_reqwest_json(
            self.request(Method::GET, &vault_route(vault_id, "/status-changes"))
                .query(&query),
            "get vault status changes",
        )
        .await
    }

    /// Request that a vault close, as its owner. A deploying vault closes at
    /// once; a live one freezes, is unwound, and redeems every holder. Repeat
    /// the request to advance a frozen vault's close pages.
    pub async fn close_vault(
        &self,
        owner: &HypercallWallet,
        vault_id: u64,
    ) -> Result<VaultRequestResponse> {
        let nonce = owner.next_nonce();
        let message = VaultClose {
            owner: owner.address.as_wallet_address().inner(),
            vault: vault_id,
            nonce,
        };
        let signature = owner.sign_hypercall_action(&message, "VaultClose").await?;
        Self::send_json_body(
            self.request(Method::POST, &vault_route(vault_id, "/close")),
            &VaultCloseRequest {
                owner: owner.address,
                nonce,
                signature,
            },
            "close vault",
        )
        .await
    }

    /// Release every queued deposit and withdrawal the holder has in a vault.
    pub async fn cancel_vault_requests(
        &self,
        holder: &HypercallWallet,
        vault_id: u64,
    ) -> Result<VaultRequestResponse> {
        let nonce = holder.next_nonce();
        let message = VaultCancel {
            holder: holder.address.as_wallet_address().inner(),
            vault: vault_id,
            nonce,
        };
        let signature = holder
            .sign_hypercall_action(&message, "VaultCancel")
            .await?;
        self.post_holder_request(
            vault_id,
            "/cancel",
            VaultHolderRequest {
                holder: holder.address,
                amount: None,
                shares: None,
                nonce,
                signature,
            },
        )
        .await
    }

    async fn post_holder_request(
        &self,
        vault_id: u64,
        suffix: &str,
        request: VaultHolderRequest,
    ) -> Result<VaultRequestResponse> {
        Self::send_json_body(
            self.request(Method::POST, &vault_route(vault_id, suffix)),
            &request,
            "vault request",
        )
        .await
    }
}

#[cfg(test)]
#[path = "vaults_test.rs"]
mod tests;
