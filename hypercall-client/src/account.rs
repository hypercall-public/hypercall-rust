//! Managed Account.sol creation through the Hypercall Factory contract.

use alloy::primitives::{aliases::U96, Address, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol;

use crate::api::HypercallClient;
use crate::error::{ClientError, Result};
use crate::wallet::{AccountAddress, HypercallWallet};

const BPS_DENOMINATOR: u64 = 10_000;
const CREATION_DEPOSIT_BUFFER_BPS: u64 = 100;
const GAS_LIMIT_BUFFER_BPS: u64 = 2_000;
const MAX_U96: u128 = (1_u128 << 96) - 1;

sol! {
    #[sol(rpc)]
    interface AccountFactory {
        event AccountCreated(address indexed account, address indexed manager);

        function getCreationDeposit() external view returns (uint256);
        function createAccount(uint96 salt) external payable returns (address account);
    }
}

/// Parameters for deploying and registering an Account.sol through Factory.
#[derive(Debug, Clone)]
pub struct CreateAccountParams {
    /// HyperEVM JSON-RPC endpoint used for contract reads and transaction submission.
    pub rpc_url: String,
    /// Factory contract that is authorized to register accounts with Exchange.
    pub factory_address: AccountAddress,
    /// Optional CREATE2 salt. Omitted values use the frontend-compatible zero salt.
    pub salt: Option<u128>,
}

/// Confirmed Account.sol creation result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreateAccountResult {
    /// Account.sol address registered by the Factory.
    pub account: AccountAddress,
    /// Confirmed account-creation transaction hash.
    pub transaction_hash: B256,
    /// HYPE value sent to Factory and deposited into the new account.
    pub hype_value: U256,
}

impl HypercallClient {
    /// Deploy and register a managed Account.sol through Factory.
    ///
    /// This call is not idempotent. Reusing a manager and salt after the
    /// corresponding account exists causes the Factory transaction to revert.
    pub async fn create_account(
        &self,
        manager: &HypercallWallet,
        params: CreateAccountParams,
    ) -> Result<CreateAccountResult> {
        let rpc_url = params.rpc_url.parse().map_err(|error| {
            ClientError::InvalidInput(format!("invalid HyperEVM RPC URL: {error}"))
        })?;
        let salt = validate_salt(params.salt)?;
        let factory_address = params.factory_address.as_wallet_address().inner();
        let provider = ProviderBuilder::new()
            .wallet(manager.ethereum_wallet())
            .connect_http(rpc_url);

        let rpc_chain_id = provider
            .get_chain_id()
            .await
            .map_err(|error| onchain_error("read HyperEVM chain ID", error))?;
        validate_chain_id(rpc_chain_id, manager.chain_id())?;

        create_account_on_provider(&provider, manager.address, factory_address, salt).await
    }
}

async fn create_account_on_provider<P>(
    provider: &P,
    manager: AccountAddress,
    factory_address: Address,
    salt: U96,
) -> Result<CreateAccountResult>
where
    P: Provider,
{
    let factory = AccountFactory::new(factory_address, provider);
    let creation_deposit = factory
        .getCreationDeposit()
        .call()
        .await
        .map_err(|error| onchain_error("read Factory creation deposit", error))?;
    let hype_value = buffered_u256(creation_deposit, CREATION_DEPOSIT_BUFFER_BPS)?;

    let predicted_account = factory
        .createAccount(salt)
        .value(hype_value)
        .from(manager.as_wallet_address().inner())
        .call()
        .await
        .map_err(|error| onchain_error("simulate Factory account creation", error))?;
    let estimated_gas = factory
        .createAccount(salt)
        .value(hype_value)
        .from(manager.as_wallet_address().inner())
        .estimate_gas()
        .await
        .map_err(|error| onchain_error("estimate Factory account creation gas", error))?;
    let gas_limit = buffered_u64(estimated_gas, GAS_LIMIT_BUFFER_BPS)?;

    let receipt = factory
        .createAccount(salt)
        .value(hype_value)
        .gas(gas_limit)
        .from(manager.as_wallet_address().inner())
        .send()
        .await
        .map_err(|error| onchain_error("submit Factory account creation", error))?
        .get_receipt()
        .await
        .map_err(|error| onchain_error("confirm Factory account creation", error))?;

    if !receipt.status() {
        return Err(ClientError::Other(
            "Factory account creation transaction reverted".to_string(),
        ));
    }

    let event = receipt
        .logs()
        .iter()
        .filter(|log| log.address() == factory_address)
        .find_map(|log| log.log_decode::<AccountFactory::AccountCreated>().ok());
    let account = validate_account_created_event(
        event.as_ref().map(|event| event.data()),
        manager.as_wallet_address().inner(),
        predicted_account,
    )?;

    Ok(CreateAccountResult {
        account,
        transaction_hash: receipt.transaction_hash,
        hype_value,
    })
}

fn validate_chain_id(rpc_chain_id: u64, wallet_chain_id: u64) -> Result<()> {
    if rpc_chain_id == wallet_chain_id {
        return Ok(());
    }
    Err(ClientError::InvalidInput(format!(
        "HyperEVM RPC chain ID {rpc_chain_id} does not match wallet chain ID {wallet_chain_id}"
    )))
}

fn validate_account_created_event(
    event: Option<&AccountFactory::AccountCreated>,
    manager_address: Address,
    predicted_account: Address,
) -> Result<AccountAddress> {
    let event = event.ok_or_else(|| {
        ClientError::Other("confirmed Factory transaction did not emit AccountCreated".to_string())
    })?;
    if event.manager != manager_address {
        return Err(ClientError::Other(format!(
            "AccountCreated manager {} does not match submitting manager {}",
            event.manager, manager_address
        )));
    }
    if event.account != predicted_account {
        return Err(ClientError::Other(format!(
            "AccountCreated account {} does not match simulated account {}",
            event.account, predicted_account
        )));
    }
    Ok(AccountAddress::from(event.account))
}

fn validate_salt(salt: Option<u128>) -> Result<U96> {
    let salt = salt.unwrap_or_default();
    if salt > MAX_U96 {
        return Err(ClientError::InvalidInput(format!(
            "account salt {salt} exceeds uint96"
        )));
    }
    Ok(U96::from(salt))
}

fn buffered_u256(value: U256, buffer_bps: u64) -> Result<U256> {
    let multiplier = U256::from(BPS_DENOMINATOR + buffer_bps);
    value
        .checked_mul(multiplier)
        .and_then(|scaled| scaled.checked_add(U256::from(BPS_DENOMINATOR - 1)))
        .map(|rounded| rounded / U256::from(BPS_DENOMINATOR))
        .ok_or_else(|| ClientError::Other("account creation deposit buffer overflow".to_string()))
}

fn buffered_u64(value: u64, buffer_bps: u64) -> Result<u64> {
    let rounded = u128::from(value)
        .checked_mul(u128::from(BPS_DENOMINATOR + buffer_bps))
        .and_then(|scaled| scaled.checked_add(u128::from(BPS_DENOMINATOR - 1)))
        .map(|scaled| scaled / u128::from(BPS_DENOMINATOR))
        .ok_or_else(|| ClientError::Other("account creation gas buffer overflow".to_string()))?;
    u64::try_from(rounded)
        .map_err(|_| ClientError::Other("account creation gas buffer overflow".to_string()))
}

fn onchain_error(context: &str, error: impl std::fmt::Display) -> ClientError {
    ClientError::Other(format!("failed to {context}: {error}"))
}

#[cfg(test)]
#[path = "account_test.rs"]
mod tests;
