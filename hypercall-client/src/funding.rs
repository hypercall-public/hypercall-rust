//! Direct HyperEVM USDC funding for Standard and Portfolio Margin accounts.

#![expect(
    clippy::result_large_err,
    reason = "these helpers preserve the established unboxed public ClientError contract"
)]

use std::{str::FromStr, time::Duration};

use alloy::eips::eip2718::Encodable2718;
use alloy::network::{Ethereum, EthereumWallet, Network, ReceiptResponse, TransactionBuilder};
use alloy::primitives::{Address, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol;
use hypercall_sdk_types::{
    MarginMode, PortfolioUsdcWithdrawalResponse, StandardUsdcWithdrawalResponse,
};
use reqwest::Method;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::api::HypercallClient;
use crate::error::{ClientError, Result};
use crate::wallet::{
    evm_submission_lock, AccountAddress, HypercallWallet, PortfolioUsdcWithdrawalSignature,
    StandardUsdcWithdrawalSignature, UsdcWithdrawalSigner,
};

const USDC_DECIMALS: u32 = 6;
const HYPERCORE_USDC_DECIMALS: u32 = 8;
const TRANSACTION_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(120);

type TransactionReceipt = <Ethereum as Network>::ReceiptResponse;
type TransactionRequest = <Ethereum as Network>::TransactionRequest;

sol! {
    #[sol(rpc)]
    interface HyperEvmUsdc {
        function decimals() external view returns (uint8);
        function balanceOf(address account) external view returns (uint256);
        function allowance(address owner, address spender) external view returns (uint256);
        function approve(address spender, uint256 amount) external returns (bool);
    }

    #[sol(rpc)]
    interface HypercallExchange {
        function usdc() external view returns (address);
        function depositUsdcFor(address account, uint256 amount) external;
    }

    #[sol(rpc)]
    interface HypercallDepositRouter {
        function USDC() external view returns (address);
        function deposit(address account, address token, uint256 amount, bool isPerp) external;
    }
}

/// Parameters shared by direct Standard and Portfolio Margin USDC deposits.
#[derive(Debug, Clone)]
pub struct DepositUsdcParams {
    /// HyperEVM JSON-RPC endpoint used for reads and transaction submission.
    pub rpc_url: String,
    /// Environment-specific HyperEVM USDC contract.
    pub usdc_address: AccountAddress,
    /// Standard account or canonical Portfolio Margin Account.sol to credit.
    pub account: AccountAddress,
    /// Positive USDC amount with at most six decimal places.
    pub amount: Decimal,
}

/// Confirmed direct USDC deposit transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepositUsdcResult {
    /// Present when the SDK had to increase the spender's allowance.
    pub approval_transaction_hash: Option<B256>,
    /// Successful deposit transaction hash.
    pub deposit_transaction_hash: B256,
    /// Block containing the successful deposit transaction.
    pub deposit_block_number: u64,
}

/// Parameters shared by Standard and Portfolio Margin USDC withdrawals.
#[derive(Debug, Clone, Copy)]
pub struct WithdrawUsdcParams {
    /// Standard account or canonical Portfolio Margin Account.sol to debit.
    pub account: AccountAddress,
    /// Nonzero HyperCore destination for withdrawn USDC.
    pub destination: AccountAddress,
    /// Positive USDC amount. Standard and new Portfolio Margin withdrawals require
    /// six decimal places (whole micro-USDC); `withdraw_portfolio_usdc_with_route`
    /// accepts up to eight decimal places to allow exact retries of legacy obligations.
    pub amount: Decimal,
    /// Explicit nonce for durable orchestration. Omitted values use the signer provider.
    pub nonce: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
enum DepositTarget {
    Exchange(Address),
    Router(Address),
}

struct DepositSubmitter {
    wallet: EthereumWallet,
    chain_id: u64,
    address: Address,
}

impl DepositSubmitter {
    fn new(wallet: &HypercallWallet) -> Self {
        Self {
            wallet: wallet.ethereum_wallet(),
            chain_id: wallet.chain_id(),
            address: wallet.address.as_wallet_address().inner(),
        }
    }
}

impl DepositTarget {
    fn address(self) -> Address {
        match self {
            Self::Exchange(address) | Self::Router(address) => address,
        }
    }
}

#[derive(Serialize)]
struct WithdrawUsdcRequest<'a> {
    wallet: AccountAddress,
    account: AccountAddress,
    destination: AccountAddress,
    amount: &'a str,
    nonce: u64,
    signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    destination_dex: Option<u32>,
}

/// Prepared HyperCore recipient route for a USDC withdrawal.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub struct WithdrawUsdcRoute {
    pub destination: AccountAddress,
    pub destination_dex: u32,
}

impl HypercallClient {
    /// Deposit HyperEVM USDC into a Standard Margin account.
    pub async fn deposit_standard_usdc(
        &self,
        funder: &HypercallWallet,
        params: DepositUsdcParams,
    ) -> Result<DepositUsdcResult> {
        let amount_units = decimal_to_units(params.amount, USDC_DECIMALS, "deposit amount")?;
        validate_nonzero(params.account, "deposit account")?;
        validate_nonzero(params.usdc_address, "USDC address")?;

        let exchange_info = self.get_exchange_info().await?;
        validate_api_chain_id(exchange_info.chain_id, funder.chain_id())?;
        let portfolio = self.get_portfolio_snapshot(params.account).await?;
        validate_portfolio_identity(&portfolio, params.account, MarginMode::Standard)?;
        let exchange = parse_nonzero_address(&exchange_info.exchange_address, "Exchange address")?;

        self.execute_deposit(
            funder,
            params,
            U256::from(amount_units),
            DepositTarget::Exchange(exchange),
        )
        .await
    }

    /// Deposit HyperEVM USDC into a Portfolio Margin Account.sol through Router.
    pub async fn deposit_portfolio_usdc(
        &self,
        manager: &HypercallWallet,
        params: DepositUsdcParams,
    ) -> Result<DepositUsdcResult> {
        let amount_units = decimal_to_units(params.amount, USDC_DECIMALS, "deposit amount")?;
        validate_nonzero(params.account, "deposit account")?;
        validate_nonzero(params.usdc_address, "USDC address")?;

        let exchange_info = self.get_exchange_info().await?;
        validate_api_chain_id(exchange_info.chain_id, manager.chain_id())?;
        let router = portfolio_deposit_router(&exchange_info)?;
        let portfolio = self.get_portfolio_snapshot(manager.address).await?;
        validate_portfolio_identity(&portfolio, params.account, MarginMode::Portfolio)?;

        self.execute_deposit(
            manager,
            params,
            U256::from(amount_units),
            DepositTarget::Router(router),
        )
        .await
    }

    /// Submit a Standard Margin USDC withdrawal.
    ///
    /// The returned response is an acceptance receipt. Poll
    /// [`HypercallClient::get_directive_status`] for final delivery.
    pub async fn withdraw_standard_usdc<S>(
        &self,
        signer: &S,
        params: WithdrawUsdcParams,
    ) -> Result<StandardUsdcWithdrawalResponse>
    where
        S: UsdcWithdrawalSigner + ?Sized,
    {
        decimal_to_units(params.amount, USDC_DECIMALS, "withdrawal amount")?;
        validate_withdrawal_addresses(params.account, params.destination)?;
        let amount = canonical_decimal_string(params.amount);
        let nonce = params.nonce.unwrap_or_else(|| signer.next_nonce());
        let wallet = signer.address();
        let signature = signer
            .sign_standard_usdc_withdrawal_payload(StandardUsdcWithdrawalSignature {
                wallet,
                account: params.account,
                destination: params.destination,
                amount: &amount,
                nonce,
            })
            .await?;
        let request = WithdrawUsdcRequest {
            wallet,
            account: params.account,
            destination: params.destination,
            amount: &amount,
            nonce,
            signature,
            destination_dex: None,
        };
        self.submit_usdc_withdrawal(
            "/withdraw/usdc",
            &request,
            "Standard Margin USDC withdrawal",
        )
        .await
    }

    /// Submit a Portfolio Margin USDC withdrawal.
    ///
    /// The returned response is an acceptance receipt. Poll
    /// [`HypercallClient::get_directive_status`] for final delivery.
    pub async fn withdraw_portfolio_usdc<S>(
        &self,
        signer: &S,
        params: WithdrawUsdcParams,
    ) -> Result<PortfolioUsdcWithdrawalResponse>
    where
        S: UsdcWithdrawalSigner + ?Sized,
    {
        decimal_to_units(params.amount, USDC_DECIMALS, "portfolio withdrawal amount")?;
        let route = self
            .prepare_usdc_withdrawal_route(params.destination)
            .await?;
        self.withdraw_portfolio_usdc_with_route(signer, params, route)
            .await
    }

    /// Resolve and validate the current HyperCore route for a recipient.
    pub async fn prepare_usdc_withdrawal_route(
        &self,
        destination: AccountAddress,
    ) -> Result<WithdrawUsdcRoute> {
        let route: WithdrawUsdcRoute = Self::send_reqwest_json(
            self.request(Method::GET, "/withdraw/usdc/route")
                .query(&[("destination", destination.to_string())]),
            "prepare USDC withdrawal route",
        )
        .await?;
        if route.destination != destination {
            return Err(ClientError::InvalidInput(
                "withdrawal route response changed the destination".to_string(),
            ));
        }
        validate_destination_dex(route.destination_dex)?;
        Ok(route)
    }

    /// Submit a Portfolio Margin withdrawal using a previously prepared route.
    ///
    /// Retain this route with the nonce after an ambiguous outcome so an exact
    /// retry does not depend on the recipient's current abstraction mode.
    pub async fn withdraw_portfolio_usdc_with_route<S>(
        &self,
        signer: &S,
        params: WithdrawUsdcParams,
        route: WithdrawUsdcRoute,
    ) -> Result<PortfolioUsdcWithdrawalResponse>
    where
        S: UsdcWithdrawalSigner + ?Sized,
    {
        let amount_wei = decimal_to_units(
            params.amount,
            HYPERCORE_USDC_DECIMALS,
            "portfolio withdrawal amount",
        )?;
        let amount_wei = u64::try_from(amount_wei).map_err(|_| {
            ClientError::InvalidInput(
                "portfolio withdrawal amount exceeds HyperCore uint64 units".to_string(),
            )
        })?;
        validate_withdrawal_addresses(params.account, params.destination)?;
        if route.destination != params.destination {
            return Err(ClientError::InvalidInput(
                "prepared withdrawal route does not match the destination".to_string(),
            ));
        }
        validate_destination_dex(route.destination_dex)?;
        let amount = canonical_decimal_string(params.amount);
        let nonce = params.nonce.unwrap_or_else(|| signer.next_nonce());
        let wallet = signer.address();
        let signature = signer
            .sign_portfolio_usdc_withdrawal_payload(PortfolioUsdcWithdrawalSignature {
                account: params.account,
                destination: params.destination,
                amount_wei,
                destination_dex: route.destination_dex,
                nonce,
            })
            .await?;
        let request = WithdrawUsdcRequest {
            wallet,
            account: params.account,
            destination: params.destination,
            amount: &amount,
            nonce,
            signature,
            destination_dex: Some(route.destination_dex),
        };
        self.submit_usdc_withdrawal(
            "/withdraw/pm-usdc",
            &request,
            "Portfolio Margin USDC withdrawal",
        )
        .await
    }

    async fn submit_usdc_withdrawal<T>(
        &self,
        route: &str,
        request: &WithdrawUsdcRequest<'_>,
        action: &str,
    ) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        match Self::send_json_body(self.request(Method::POST, route), request, action).await {
            Err(error) if withdrawal_submission_is_ambiguous(&error) => {
                Err(ClientError::WithdrawalSubmissionAmbiguous {
                    action: action.to_owned(),
                    route: route.to_owned(),
                    account: request.account,
                    destination: request.destination,
                    destination_dex: request.destination_dex,
                    amount: request.amount.to_owned(),
                    nonce: request.nonce,
                    source: Box::new(error),
                })
            }
            result => result,
        }
    }

    async fn execute_deposit(
        &self,
        funder: &HypercallWallet,
        params: DepositUsdcParams,
        amount_units: U256,
        target: DepositTarget,
    ) -> Result<DepositUsdcResult> {
        let rpc_url = params
            .rpc_url
            .parse()
            .map_err(|source| ClientError::invalid_input("HyperEVM RPC URL", source))?;
        let provider = ProviderBuilder::new().connect_http(rpc_url);
        let rpc_chain_id = provider
            .get_chain_id()
            .await
            .map_err(|error| onchain_error("read HyperEVM chain ID", error))?;
        validate_rpc_chain_id(rpc_chain_id, funder.chain_id())?;

        let submission_lock = evm_submission_lock(funder.address.as_wallet_address().inner());
        let _submission_guard = submission_lock.lock().await;
        execute_deposit_on_provider(
            &provider,
            DepositSubmitter::new(funder),
            params.usdc_address.as_wallet_address().inner(),
            params.account.as_wallet_address().inner(),
            amount_units,
            target,
        )
        .await
    }
}

fn validate_destination_dex(destination_dex: u32) -> Result<()> {
    if matches!(destination_dex, 0 | u32::MAX) {
        Ok(())
    } else {
        Err(ClientError::InvalidInput(
            "withdrawal route response returned an unsupported destination dex".to_string(),
        ))
    }
}

async fn execute_deposit_on_provider<P>(
    provider: &P,
    submitter: DepositSubmitter,
    usdc_address: Address,
    account: Address,
    amount_units: U256,
    target: DepositTarget,
) -> Result<DepositUsdcResult>
where
    P: Provider,
{
    let DepositSubmitter {
        wallet,
        chain_id,
        address: funder,
    } = submitter;
    let canonical_usdc = match target {
        DepositTarget::Exchange(exchange) => HypercallExchange::new(exchange, provider)
            .usdc()
            .call()
            .await
            .map_err(|error| onchain_error("read Exchange canonical USDC", error))?,
        DepositTarget::Router(router) => HypercallDepositRouter::new(router, provider)
            .USDC()
            .call()
            .await
            .map_err(|error| onchain_error("read Router canonical USDC", error))?,
    };
    if canonical_usdc != usdc_address {
        return Err(ClientError::InvalidInput(format!(
            "supplied USDC address {usdc_address} does not match the funding contract's canonical USDC {canonical_usdc}"
        )));
    }

    let usdc = HyperEvmUsdc::new(usdc_address, provider);
    let decimals = usdc
        .decimals()
        .call()
        .await
        .map_err(|error| onchain_error("read USDC decimals", error))?;
    if decimals != USDC_DECIMALS as u8 {
        return Err(ClientError::InvalidInput(format!(
            "USDC contract reports {decimals} decimals, expected {USDC_DECIMALS}"
        )));
    }
    let balance = usdc
        .balanceOf(funder)
        .call()
        .await
        .map_err(|error| onchain_error("read HyperEVM USDC balance", error))?;
    if balance < amount_units {
        return Err(ClientError::InvalidInput(format!(
            "insufficient HyperEVM USDC balance: have {balance} base units, need {amount_units}"
        )));
    }

    let spender = target.address();
    let allowance = usdc
        .allowance(funder, spender)
        .call()
        .await
        .map_err(|error| onchain_error("read HyperEVM USDC allowance", error))?;
    let approval_transaction_hash = if allowance < amount_units {
        let request = usdc
            .approve(spender, amount_units)
            .from(funder)
            .into_transaction_request();
        let (tx_hash, receipt) = submit_contract_transaction(
            provider,
            &wallet,
            chain_id,
            funder,
            request,
            "HyperEVM USDC approval",
        )
        .await?;
        if !receipt.status() {
            return Err(ClientError::TransactionReverted {
                operation: "HyperEVM USDC approval",
                transaction_hash: Some(tx_hash),
            });
        }
        Some(tx_hash)
    } else {
        None
    };

    let request = match target {
        DepositTarget::Exchange(exchange) => HypercallExchange::new(exchange, provider)
            .depositUsdcFor(account, amount_units)
            .from(funder)
            .into_transaction_request(),
        DepositTarget::Router(router) => HypercallDepositRouter::new(router, provider)
            .deposit(account, usdc_address, amount_units, true)
            .from(funder)
            .into_transaction_request(),
    };
    let operation = approval_transaction_hash.map_or_else(
        || "Hypercall USDC deposit".to_string(),
        |tx_hash| format!("Hypercall USDC deposit after confirmed approval {tx_hash}"),
    );
    let (deposit_transaction_hash, receipt) =
        submit_contract_transaction(provider, &wallet, chain_id, funder, request, &operation)
            .await?;
    if !receipt.status() {
        return Err(ClientError::TransactionReverted {
            operation: "Hypercall USDC deposit",
            transaction_hash: Some(deposit_transaction_hash),
        });
    }
    let deposit_block_number =
        receipt
            .block_number()
            .ok_or(ClientError::MissingTransactionBlock {
                transaction_hash: deposit_transaction_hash,
            })?;

    Ok(DepositUsdcResult {
        approval_transaction_hash,
        deposit_transaction_hash,
        deposit_block_number,
    })
}

fn withdrawal_submission_is_ambiguous(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Http(_)
            | ClientError::Api {
                status: 500..=599,
                ..
            }
    )
}

/// Sign before broadcasting so an ambiguous RPC failure includes the hash that
/// callers must reconcile before retrying.
async fn submit_contract_transaction<P>(
    provider: &P,
    wallet: &EthereumWallet,
    chain_id: u64,
    sender: Address,
    mut request: TransactionRequest,
    operation: &str,
) -> Result<(B256, TransactionReceipt)>
where
    P: Provider,
{
    request.from = Some(sender);
    request.chain_id = Some(chain_id);
    request.nonce = Some(
        provider
            .get_transaction_count(sender)
            .pending()
            .await
            .map_err(|error| {
                onchain_error(format!("resolve pending nonce for {operation}"), error)
            })?,
    );
    let fees = provider
        .estimate_eip1559_fees()
        .await
        .map_err(|error| onchain_error(format!("estimate EIP-1559 fees for {operation}"), error))?;
    request.max_fee_per_gas = Some(fees.max_fee_per_gas);
    request.max_priority_fee_per_gas = Some(fees.max_priority_fee_per_gas);
    request.gas = Some(
        provider
            .estimate_gas(request.clone())
            .await
            .map_err(|error| onchain_error(format!("estimate gas for {operation}"), error))?,
    );

    let signed = request
        .build(wallet)
        .await
        .map_err(|error| onchain_error(format!("sign {operation}"), error))?;
    let tx_hash = *signed.tx_hash();
    let mut raw_transaction = Vec::with_capacity(signed.encode_2718_len());
    signed.encode_2718(&mut raw_transaction);

    let pending = provider
        .send_raw_transaction(&raw_transaction)
        .await
        .map_err(|error| ClientError::TransactionSubmissionUnknown {
            operation: operation.to_owned(),
            transaction_hash: tx_hash,
            source: Box::new(error),
        })?;
    let receipt = pending
        .with_timeout(Some(TRANSACTION_CONFIRMATION_TIMEOUT))
        .get_receipt()
        .await
        .map_err(|error| ClientError::TransactionConfirmationUnknown {
            operation: operation.to_owned(),
            transaction_hash: tx_hash,
            source: Box::new(error),
        })?;
    Ok((tx_hash, receipt))
}

fn decimal_to_units(amount: Decimal, decimals: u32, name: &str) -> Result<u128> {
    if amount <= Decimal::ZERO {
        return Err(ClientError::InvalidInput(format!(
            "{name} must be positive"
        )));
    }
    let normalized = amount.normalize();
    if normalized.scale() > decimals {
        return Err(ClientError::InvalidInput(format!(
            "{name} supports at most {decimals} decimal places"
        )));
    }
    let multiplier = Decimal::from(10_u64.pow(decimals));
    normalized
        .checked_mul(multiplier)
        .and_then(|scaled| scaled.to_u128())
        .ok_or_else(|| ClientError::InvalidInput(format!("{name} is too large")))
}

fn canonical_decimal_string(amount: Decimal) -> String {
    amount.normalize().to_string()
}

fn validate_nonzero(address: AccountAddress, name: &str) -> Result<()> {
    if address.as_wallet_address().inner() == Address::ZERO {
        return Err(ClientError::InvalidInput(format!("{name} must be nonzero")));
    }
    Ok(())
}

fn validate_withdrawal_addresses(
    account: AccountAddress,
    destination: AccountAddress,
) -> Result<()> {
    validate_nonzero(account, "withdrawal account")?;
    validate_nonzero(destination, "withdrawal destination")
}

fn parse_nonzero_address(value: &str, name: &str) -> Result<Address> {
    let address = Address::from_str(value)
        .map_err(|source| ClientError::invalid_input(name.to_owned(), source))?;
    if address == Address::ZERO {
        return Err(ClientError::InvalidInput(format!("{name} must be nonzero")));
    }
    Ok(address)
}

fn validate_api_chain_id(api_chain_id: u64, wallet_chain_id: u64) -> Result<()> {
    if api_chain_id == wallet_chain_id {
        return Ok(());
    }
    Err(ClientError::InvalidInput(format!(
        "API chain ID {api_chain_id} does not match wallet chain ID {wallet_chain_id}"
    )))
}

fn validate_rpc_chain_id(rpc_chain_id: u64, wallet_chain_id: u64) -> Result<()> {
    if rpc_chain_id == wallet_chain_id {
        return Ok(());
    }
    Err(ClientError::InvalidInput(format!(
        "HyperEVM RPC chain ID {rpc_chain_id} does not match wallet chain ID {wallet_chain_id}"
    )))
}

fn validate_portfolio_identity(
    portfolio: &hypercall_sdk_types::Portfolio,
    expected_account: AccountAddress,
    expected_mode: MarginMode,
) -> Result<()> {
    if portfolio.wallet_address != expected_account.into_wallet_address() {
        return Err(ClientError::InvalidInput(format!(
            "resolved account {} does not match requested account {expected_account}",
            portfolio.wallet_address
        )));
    }
    let actual_mode = portfolio
        .margin_mode_kind()
        .map_err(|source| ClientError::invalid_input("account margin mode", source))?;
    if actual_mode != expected_mode {
        return Err(ClientError::InvalidInput(format!(
            "account {} uses {} margin, expected {} margin",
            portfolio.wallet_address,
            actual_mode.as_str(),
            expected_mode.as_str()
        )));
    }
    Ok(())
}

fn portfolio_deposit_router(
    exchange_info: &hypercall_sdk_types::ExchangeInfoResponse,
) -> Result<Address> {
    if !exchange_info.portfolio_margin_deposits_enabled {
        return Err(ClientError::InvalidInput(
            "portfolio-margin deposits are disabled".to_string(),
        ));
    }
    exchange_info
        .deposit_router_address
        .as_deref()
        .ok_or_else(|| {
            ClientError::InvalidInput("portfolio-margin deposit Router is unavailable".to_string())
        })
        .and_then(|address| parse_nonzero_address(address, "deposit Router address"))
}

fn onchain_error(
    operation: impl Into<String>,
    source: impl std::error::Error + Send + Sync + 'static,
) -> ClientError {
    ClientError::Onchain {
        operation: operation.into(),
        source: Box::new(source),
    }
}

#[cfg(test)]
#[path = "funding_test.rs"]
mod tests;
