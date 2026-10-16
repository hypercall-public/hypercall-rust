use super::*;
use crate::test_support::{
    json_body, recording_rpc_client, serve_once, serve_once_then_without_response,
    serve_once_with_status, serve_sequence, FixedNonce, RecordedRequests,
};
use crate::{HypercallWallet, NonceProvider};
use alloy::consensus::{Transaction, TxEnvelope};
use alloy::eips::eip2718::Decodable2718;
use alloy::providers::ProviderBuilder;
use alloy::sol_types::{SolCall, SolValue};
use alloy::transports::mock::Asserter;
use hypercall_sdk_types::{
    DirectiveDeliveryStatus, ExchangeInfoResponse, Portfolio, SigningDomainInfo,
    UsdcWithdrawalDomainStatus, WalletAddress,
};
use rust_decimal_macros::dec;
use std::sync::Arc;

const PRIVATE_KEY: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const WALLET: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";
const ACCOUNT: &str = "0x0000000000000000000000000000000000000002";
const DESTINATION: &str = "0x0000000000000000000000000000000000000003";

fn account(value: &str) -> AccountAddress {
    value.parse().unwrap()
}

fn encoded_u8(value: u8) -> alloy::primitives::Bytes {
    let mut encoded = [0_u8; 32];
    encoded[31] = value;
    alloy::primitives::Bytes::copy_from_slice(&encoded)
}

fn encoded_address(value: Address) -> alloy::primitives::Bytes {
    alloy::primitives::Bytes::from(value.abi_encode())
}

fn push_transaction_signing_inputs(asserter: &Asserter) {
    asserter.push_success(&serde_json::json!("0x0"));
    asserter.push_success(&serde_json::json!({
        "oldestBlock": "0x1",
        "baseFeePerGas": ["0x1", "0x1"],
        "gasUsedRatio": [0.5],
        "reward": [["0x1"]]
    }));
    asserter.push_success(&serde_json::json!("0x5208"));
}

fn push_signed_transaction(
    asserter: &Asserter,
    rpc_transaction_hash: B256,
    receipt_value: serde_json::Value,
) {
    push_transaction_signing_inputs(asserter);
    asserter.push_success(&rpc_transaction_hash);
    asserter.push_success(&receipt_value);
    asserter.push_success(&receipt_value);
}

fn receipt(
    transaction_hash: B256,
    from: Address,
    to: Address,
    block_number: u64,
    success: bool,
) -> serde_json::Value {
    serde_json::json!({
        "transactionHash": transaction_hash,
        "transactionIndex": "0x0",
        "blockHash": B256::repeat_byte(0x44),
        "blockNumber": format!("0x{block_number:x}"),
        "from": from,
        "to": to,
        "cumulativeGasUsed": "0x5208",
        "gasUsed": "0x5208",
        "contractAddress": null,
        "logs": [],
        "logsBloom": format!("0x{}", "00".repeat(256)),
        "status": if success { "0x1" } else { "0x0" },
        "effectiveGasPrice": "0x1",
        "type": "0x0"
    })
}

fn sent_transactions(requests: &RecordedRequests) -> Vec<TxEnvelope> {
    requests
        .take_json()
        .into_iter()
        .filter(|request| request["method"] == "eth_sendRawTransaction")
        .map(|request| {
            let encoded = request["params"][0]
                .as_str()
                .expect("raw transaction parameter");
            let raw = hex::decode(encoded.strip_prefix("0x").unwrap_or(encoded)).unwrap();
            TxEnvelope::decode_2718(&mut raw.as_slice()).unwrap()
        })
        .collect()
}

fn portfolio(wallet_address: AccountAddress, margin_mode: &str) -> Portfolio {
    Portfolio {
        wallet_address: wallet_address.into_wallet_address(),
        positions: Vec::new(),
        total_margin_used: Decimal::ZERO,
        available_balance: dec!(100),
        withdrawable_usdc: None,
        portfolio_snapshot_timestamp_ms: None,
        span_margin: None,
        margin_mode: margin_mode.to_string(),
        margin_summary: None,
    }
}

fn exchange_info_json(router: Option<Address>, deposits_enabled: bool) -> String {
    serde_json::json!({
        "exchange_address": Address::repeat_byte(4),
        "factory_address": Address::repeat_byte(6),
        "deposit_router_address": router,
        "portfolio_margin_deposits_enabled": deposits_enabled,
        "chain_id": 999,
        "signing_domain": {
            "name": "Hypercall",
            "version": "1",
            "verifying_contract": Address::repeat_byte(4)
        }
    })
    .to_string()
}

fn portfolio_json(wallet_address: AccountAddress, margin_mode: &str) -> String {
    serde_json::json!({
        "success": true,
        "data": {
            "wallet_address": wallet_address,
            "positions": [],
            "total_margin_used": "0",
            "available_balance": "100",
            "portfolio_snapshot_timestamp_ms": null,
            "margin_mode": margin_mode
        },
        "error": null
    })
    .to_string()
}

#[test]
fn usdc_units_are_exact_and_positive() {
    assert_eq!(
        decimal_to_units(dec!(1.234567), 6, "amount").unwrap(),
        1_234_567
    );
    assert_eq!(
        decimal_to_units(dec!(1.23456789), 8, "amount").unwrap(),
        123_456_789
    );
    assert!(decimal_to_units(Decimal::ZERO, 6, "amount").is_err());
    assert!(decimal_to_units(dec!(-1), 6, "amount").is_err());
    assert!(decimal_to_units(dec!(0.0000001), 6, "amount").is_err());
    assert!(decimal_to_units(dec!(0.000000001), 8, "amount").is_err());
    assert!(decimal_to_units(Decimal::MAX, 8, "amount").is_err());
}

#[test]
fn amount_and_addresses_are_canonicalized_or_rejected() {
    assert_eq!(canonical_decimal_string(dec!(100.230000)), "100.23");
    assert!(validate_nonzero(AccountAddress::from(Address::ZERO), "account").is_err());
    assert!(
        validate_withdrawal_addresses(account(ACCOUNT), AccountAddress::from(Address::ZERO))
            .is_err()
    );
    assert!(parse_nonzero_address("invalid", "Router").is_err());
    assert!(parse_nonzero_address(&Address::ZERO.to_string(), "Router").is_err());
}

#[test]
fn chain_ids_must_match() {
    assert!(validate_api_chain_id(999, 999).is_ok());
    assert!(validate_rpc_chain_id(999, 999).is_ok());
    assert!(validate_api_chain_id(998, 999).is_err());
    assert!(validate_rpc_chain_id(998, 999).is_err());
}

#[test]
fn account_identity_and_margin_mode_are_exact() {
    let expected = account(ACCOUNT);
    assert!(validate_portfolio_identity(
        &portfolio(expected, "standard"),
        expected,
        MarginMode::Standard,
    )
    .is_ok());
    assert!(validate_portfolio_identity(
        &portfolio(expected, "portfolio"),
        expected,
        MarginMode::Standard,
    )
    .is_err());
    assert!(validate_portfolio_identity(
        &portfolio(account(DESTINATION), "standard"),
        expected,
        MarginMode::Standard,
    )
    .is_err());
}

#[test]
fn portfolio_router_must_be_enabled_available_and_nonzero() {
    let router = Address::repeat_byte(5);
    let mut exchange_info = ExchangeInfoResponse {
        exchange_address: Address::repeat_byte(4).to_string(),
        factory_address: Address::repeat_byte(6).to_string(),
        deposit_router_address: Some(router.to_string()),
        portfolio_margin_deposits_enabled: true,
        chain_id: 999,
        signing_domain: SigningDomainInfo {
            name: "Hypercall".to_string(),
            version: "1".to_string(),
            verifying_contract: Address::repeat_byte(4).to_string(),
        },
        hl_builder_address: None,
    };
    assert_eq!(portfolio_deposit_router(&exchange_info).unwrap(), router);

    exchange_info.portfolio_margin_deposits_enabled = false;
    assert!(portfolio_deposit_router(&exchange_info).is_err());
    exchange_info.portfolio_margin_deposits_enabled = true;
    exchange_info.deposit_router_address = None;
    assert!(portfolio_deposit_router(&exchange_info).is_err());
    exchange_info.deposit_router_address = Some(Address::ZERO.to_string());
    assert!(portfolio_deposit_router(&exchange_info).is_err());
}

#[tokio::test]
async fn standard_deposit_rejects_wrong_mode_from_canonical_portfolio_preflight() {
    let requested_account = account(ACCOUNT);
    let (base_url, request_task) = serve_sequence(vec![
        exchange_info_json(None, false),
        portfolio_json(requested_account, "portfolio"),
    ])
    .await;
    let client = HypercallClient::new(base_url);
    let wallet = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();

    let error = client
        .deposit_standard_usdc(
            &wallet,
            DepositUsdcParams {
                rpc_url: "http://unused.invalid".to_string(),
                usdc_address: account(DESTINATION),
                account: requested_account,
                amount: dec!(1),
            },
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("expected standard margin"));
    let requests = request_task.await.unwrap();
    assert!(requests[0].starts_with("GET /exchange-info HTTP/1.1\r\n"));
    assert!(requests[1].starts_with("GET /portfolio?wallet="));
}

#[tokio::test]
async fn portfolio_deposit_rejects_manager_account_mismatch_before_rpc() {
    let requested_account = account(ACCOUNT);
    let canonical_account = account(DESTINATION);
    let (base_url, request_task) = serve_sequence(vec![
        exchange_info_json(Some(Address::repeat_byte(5)), true),
        portfolio_json(canonical_account, "portfolio"),
    ])
    .await;
    let client = HypercallClient::new(base_url);
    let manager = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();

    let error = client
        .deposit_portfolio_usdc(
            &manager,
            DepositUsdcParams {
                rpc_url: "http://unused.invalid".to_string(),
                usdc_address: account(DESTINATION),
                account: requested_account,
                amount: dec!(1),
            },
        )
        .await
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("does not match requested account"));
    let requests = request_task.await.unwrap();
    assert!(requests[0].starts_with("GET /exchange-info HTTP/1.1\r\n"));
    assert!(requests[1].starts_with(&format!("GET /portfolio?wallet={} ", manager.address)));
}

#[tokio::test]
async fn deposit_rejects_token_that_is_not_the_funding_contract_usdc() {
    let asserter = Asserter::new();
    let wallet = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();
    asserter.push_success(&encoded_address(Address::repeat_byte(9)));
    let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());

    let error = execute_deposit_on_provider(
        &provider,
        DepositSubmitter::new(&wallet),
        Address::repeat_byte(2),
        Address::repeat_byte(3),
        U256::from(1_000_000_u64),
        DepositTarget::Router(Address::repeat_byte(4)),
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("does not match"));
    assert!(error.to_string().contains("canonical USDC"));
    assert!(asserter.read_q().is_empty());
}

#[tokio::test]
async fn deposit_rejects_non_usdc_decimals_before_spending_funds() {
    let asserter = Asserter::new();
    let wallet = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();
    let usdc = Address::repeat_byte(2);
    asserter.push_success(&encoded_address(usdc));
    asserter.push_success(&encoded_u8(18));
    let provider = ProviderBuilder::new().connect_mocked_client(asserter);

    let error = execute_deposit_on_provider(
        &provider,
        DepositSubmitter::new(&wallet),
        usdc,
        Address::repeat_byte(3),
        U256::from(1_000_000_u64),
        DepositTarget::Exchange(Address::repeat_byte(4)),
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("reports 18 decimals"));
}

#[tokio::test]
async fn deposit_rejects_insufficient_balance_before_reading_allowance() {
    let asserter = Asserter::new();
    let wallet = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();
    let usdc = Address::repeat_byte(2);
    asserter.push_success(&encoded_address(usdc));
    asserter.push_success(&encoded_u8(6));
    asserter.push_success(&alloy::primitives::Bytes::from(
        U256::from(999_999_u64).abi_encode(),
    ));
    let provider = ProviderBuilder::new().connect_mocked_client(asserter);

    let error = execute_deposit_on_provider(
        &provider,
        DepositSubmitter::new(&wallet),
        usdc,
        Address::repeat_byte(3),
        U256::from(1_000_000_u64),
        DepositTarget::Exchange(Address::repeat_byte(4)),
    )
    .await
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("insufficient HyperEVM USDC balance"));
}

#[tokio::test]
async fn ambiguous_submission_error_includes_locally_computed_hash() {
    let asserter = Asserter::new();
    let wallet = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();
    let usdc = Address::repeat_byte(2);
    let exchange = Address::repeat_byte(4);
    let amount = U256::from(1_000_000_u64);
    asserter.push_success(&encoded_address(usdc));
    asserter.push_success(&encoded_u8(6));
    asserter.push_success(&alloy::primitives::Bytes::from(amount.abi_encode()));
    asserter.push_success(&alloy::primitives::Bytes::from(amount.abi_encode()));
    push_transaction_signing_inputs(&asserter);
    asserter.push_failure_msg("connection closed after write");
    let provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .connect_mocked_client(asserter.clone());

    let error = execute_deposit_on_provider(
        &provider,
        DepositSubmitter::new(&wallet),
        usdc,
        Address::repeat_byte(3),
        amount,
        DepositTarget::Exchange(exchange),
    )
    .await
    .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("submission outcome is unknown"));
    assert!(message.contains("hash=0x"));
    assert!(message.contains("reconcile this transaction before retrying"));
    assert!(asserter.read_q().is_empty());
}

#[tokio::test]
async fn deposit_skips_approval_and_returns_confirmed_receipt() {
    let asserter = Asserter::new();
    let wallet = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();
    let funder = wallet.address.as_wallet_address().inner();
    let usdc = Address::repeat_byte(2);
    let account = Address::repeat_byte(3);
    let exchange = Address::repeat_byte(4);
    let amount = U256::from(1_000_000_u64);
    let deposit_hash = B256::repeat_byte(0x22);
    asserter.push_success(&encoded_address(usdc));
    asserter.push_success(&encoded_u8(6));
    asserter.push_success(&alloy::primitives::Bytes::from(
        U256::from(2_000_000_u64).abi_encode(),
    ));
    asserter.push_success(&alloy::primitives::Bytes::from(amount.abi_encode()));
    push_signed_transaction(
        &asserter,
        deposit_hash,
        receipt(deposit_hash, funder, exchange, 42, true),
    );
    let (rpc_client, requests) = recording_rpc_client(asserter.clone());
    let provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .connect_client(rpc_client);

    let result = execute_deposit_on_provider(
        &provider,
        DepositSubmitter::new(&wallet),
        usdc,
        account,
        amount,
        DepositTarget::Exchange(exchange),
    )
    .await
    .unwrap();

    assert_eq!(result.approval_transaction_hash, None);
    assert_ne!(result.deposit_transaction_hash, B256::ZERO);
    assert_eq!(result.deposit_block_number, 42);
    assert!(asserter.read_q().is_empty());
    let transactions = sent_transactions(&requests);
    assert_eq!(transactions.len(), 1);
    assert_eq!(transactions[0].to(), Some(exchange));
    assert_eq!(
        transactions[0].input().as_ref(),
        HypercallExchange::depositUsdcForCall { account, amount }
            .abi_encode()
            .as_slice()
    );
}

#[tokio::test]
async fn deposit_confirms_exact_approval_before_router_submission() {
    let asserter = Asserter::new();
    let wallet = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();
    let funder = wallet.address.as_wallet_address().inner();
    let usdc = Address::repeat_byte(2);
    let account = Address::repeat_byte(3);
    let router = Address::repeat_byte(4);
    let amount = U256::from(1_000_000_u64);
    let approval_hash = B256::repeat_byte(0x11);
    let deposit_hash = B256::repeat_byte(0x22);
    asserter.push_success(&encoded_address(usdc));
    asserter.push_success(&encoded_u8(6));
    asserter.push_success(&alloy::primitives::Bytes::from(
        U256::from(2_000_000_u64).abi_encode(),
    ));
    asserter.push_success(&alloy::primitives::Bytes::from(U256::ZERO.abi_encode()));
    push_signed_transaction(
        &asserter,
        approval_hash,
        receipt(approval_hash, funder, usdc, 41, true),
    );
    push_signed_transaction(
        &asserter,
        deposit_hash,
        receipt(deposit_hash, funder, router, 42, true),
    );
    let (rpc_client, requests) = recording_rpc_client(asserter.clone());
    let provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .connect_client(rpc_client);

    let result = execute_deposit_on_provider(
        &provider,
        DepositSubmitter::new(&wallet),
        usdc,
        account,
        amount,
        DepositTarget::Router(router),
    )
    .await
    .unwrap();

    assert!(result.approval_transaction_hash.is_some());
    assert_ne!(result.deposit_transaction_hash, B256::ZERO);
    assert_eq!(result.deposit_block_number, 42);
    assert!(asserter.read_q().is_empty());
    let transactions = sent_transactions(&requests);
    assert_eq!(transactions.len(), 2);
    assert_eq!(transactions[0].to(), Some(usdc));
    assert_eq!(
        transactions[0].input().as_ref(),
        HyperEvmUsdc::approveCall {
            spender: router,
            amount,
        }
        .abi_encode()
        .as_slice()
    );
    assert_eq!(transactions[1].to(), Some(router));
    assert_eq!(
        transactions[1].input().as_ref(),
        HypercallDepositRouter::depositCall {
            account,
            token: usdc,
            amount,
            isPerp: true,
        }
        .abi_encode()
        .as_slice()
    );
}

#[tokio::test]
async fn reverted_deposit_receipt_includes_transaction_hash() {
    let asserter = Asserter::new();
    let wallet = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();
    let funder = wallet.address.as_wallet_address().inner();
    let usdc = Address::repeat_byte(2);
    let account = Address::repeat_byte(3);
    let exchange = Address::repeat_byte(4);
    let amount = U256::from(1_000_000_u64);
    let deposit_hash = B256::repeat_byte(0x22);
    asserter.push_success(&encoded_address(usdc));
    asserter.push_success(&encoded_u8(6));
    asserter.push_success(&alloy::primitives::Bytes::from(amount.abi_encode()));
    asserter.push_success(&alloy::primitives::Bytes::from(amount.abi_encode()));
    push_signed_transaction(
        &asserter,
        deposit_hash,
        receipt(deposit_hash, funder, exchange, 42, false),
    );
    let provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .connect_mocked_client(asserter);

    let error = execute_deposit_on_provider(
        &provider,
        DepositSubmitter::new(&wallet),
        usdc,
        account,
        amount,
        DepositTarget::Exchange(exchange),
    )
    .await
    .unwrap_err();

    let transaction_hash = match &error {
        ClientError::TransactionReverted {
            operation: "Hypercall USDC deposit",
            transaction_hash: Some(transaction_hash),
        } => *transaction_hash,
        other => panic!("expected a typed deposit-reverted error, got {other:?}"),
    };
    assert_ne!(transaction_hash, B256::ZERO);
    assert_eq!(
        error.to_string(),
        format!("Hypercall USDC deposit transaction {transaction_hash} reverted")
    );
}

#[tokio::test]
async fn reverted_approval_receipt_stops_before_deposit_and_includes_hash() {
    let asserter = Asserter::new();
    let wallet = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();
    let funder = wallet.address.as_wallet_address().inner();
    let usdc = Address::repeat_byte(2);
    let account = Address::repeat_byte(3);
    let exchange = Address::repeat_byte(4);
    let amount = U256::from(1_000_000_u64);
    let approval_hash = B256::repeat_byte(0x11);
    asserter.push_success(&encoded_address(usdc));
    asserter.push_success(&encoded_u8(6));
    asserter.push_success(&alloy::primitives::Bytes::from(amount.abi_encode()));
    asserter.push_success(&alloy::primitives::Bytes::from(U256::ZERO.abi_encode()));
    push_signed_transaction(
        &asserter,
        approval_hash,
        receipt(approval_hash, funder, usdc, 41, false),
    );
    let provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .connect_mocked_client(asserter.clone());

    let error = execute_deposit_on_provider(
        &provider,
        DepositSubmitter::new(&wallet),
        usdc,
        account,
        amount,
        DepositTarget::Exchange(exchange),
    )
    .await
    .unwrap_err();

    let transaction_hash = match &error {
        ClientError::TransactionReverted {
            operation: "HyperEVM USDC approval",
            transaction_hash: Some(transaction_hash),
        } => *transaction_hash,
        other => panic!("expected a typed approval-reverted error, got {other:?}"),
    };
    assert_ne!(transaction_hash, B256::ZERO);
    assert_eq!(
        error.to_string(),
        format!("HyperEVM USDC approval transaction {transaction_hash} reverted")
    );
    assert!(asserter.read_q().is_empty());
}

#[tokio::test]
async fn standard_withdrawal_uses_canonical_body_and_explicit_nonce() {
    let response = serde_json::json!({
        "success": true,
        "request_id": "00000000-0000-0000-0000-000000000001",
        "directive_id": "00000000-0000-0000-0000-000000000002",
        "domain_status": "accepted",
        "delivery_status": "pending",
        "balance_after": "89.77",
        "message": "USDC withdrawal accepted"
    })
    .to_string();
    let (base_url, request_task) = serve_once(response).await;
    let client = HypercallClient::new(base_url);
    let signer = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();

    let result = client
        .withdraw_standard_usdc(
            &signer,
            WithdrawUsdcParams {
                account: account(ACCOUNT),
                destination: account(DESTINATION),
                amount: dec!(10.230000),
                nonce: Some(777),
            },
        )
        .await
        .unwrap();

    assert_eq!(result.balance_after, dec!(89.77));
    assert_eq!(result.domain_status, UsdcWithdrawalDomainStatus::Accepted);
    let request = request_task.await.unwrap();
    assert!(request.starts_with("POST /withdraw/usdc HTTP/1.1\r\n"));
    let body = json_body(&request);
    assert_eq!(body["wallet"], WALLET);
    assert_eq!(body["account"], ACCOUNT);
    assert_eq!(body["destination"], DESTINATION);
    assert_eq!(body["amount"], "10.23");
    assert_eq!(body["nonce"], 777);
    assert!(body["signature"].as_str().unwrap().starts_with("0x"));
}

#[tokio::test]
async fn portfolio_withdrawal_uses_generated_nonce_and_pm_route() {
    let response = serde_json::json!({
        "success": true,
        "request_id": "00000000-0000-0000-0000-000000000003",
        "directive_id": "00000000-0000-0000-0000-000000000003",
        "domain_status": "pending_chain_effect",
        "delivery_status": "pending_core_delivery",
        "message": "Portfolio-margin USDC withdrawal accepted"
    })
    .to_string();
    let route = serde_json::json!({
        "destination": DESTINATION,
        "destination_dex": u32::MAX,
    })
    .to_string();
    let (base_url, request_task) = serve_sequence(vec![route, response]).await;
    let client = HypercallClient::new(base_url);
    let nonce_provider: Arc<dyn NonceProvider> = Arc::new(FixedNonce(778));
    let signer = HypercallWallet::from_private_key(PRIVATE_KEY, 999)
        .unwrap()
        .with_nonce_provider(nonce_provider);

    let result = client
        .withdraw_portfolio_usdc(
            &signer,
            WithdrawUsdcParams {
                account: account(ACCOUNT),
                destination: account(DESTINATION),
                amount: dec!(10.123456),
                nonce: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        result.domain_status,
        UsdcWithdrawalDomainStatus::PendingChainEffect
    );
    assert_eq!(
        result.delivery_status,
        DirectiveDeliveryStatus::PendingCoreDelivery
    );
    let requests = request_task.await.unwrap();
    assert!(requests[0].starts_with("GET /withdraw/usdc/route?destination="));
    assert!(requests[1].starts_with("POST /withdraw/pm-usdc HTTP/1.1\r\n"));
    let body = json_body(&requests[1]);
    assert_eq!(body["amount"], "10.123456");
    assert_eq!(body["destination_dex"], u32::MAX);
    assert_eq!(body["nonce"], 778);
    assert!(body["signature"].as_str().unwrap().starts_with("0x"));
}

#[tokio::test]
async fn portfolio_withdrawal_can_retry_with_a_pinned_route() {
    let response = serde_json::json!({
        "success": true,
        "request_id": "00000000-0000-0000-0000-000000000003",
        "directive_id": "00000000-0000-0000-0000-000000000003",
        "domain_status": "pending_chain_effect",
        "delivery_status": "pending_core_delivery",
        "message": "Portfolio-margin USDC withdrawal accepted"
    })
    .to_string();
    let (base_url, request_task) = serve_once(response).await;
    let client = HypercallClient::new(base_url);
    let signer = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();
    let destination = account(DESTINATION);

    client
        .withdraw_portfolio_usdc_with_route(
            &signer,
            WithdrawUsdcParams {
                account: account(ACCOUNT),
                destination,
                amount: dec!(10.123456),
                nonce: Some(778),
            },
            WithdrawUsdcRoute {
                destination,
                destination_dex: u32::MAX,
            },
        )
        .await
        .unwrap();

    let request = request_task.await.unwrap();
    assert!(request.starts_with("POST /withdraw/pm-usdc HTTP/1.1\r\n"));
    let body = json_body(&request);
    assert_eq!(body["amount"], "10.123456");
    assert_eq!(body["destination_dex"], u32::MAX);
    assert_eq!(body["nonce"], 778);
}

#[tokio::test]
async fn portfolio_withdrawal_with_route_accepts_legacy_eight_decimal_amounts_for_retry() {
    // withdraw_portfolio_usdc_with_route is the pinned-route retry path. It accepts
    // up to eight decimal places so callers can re-submit an already-accepted
    // obligation (e.g. dec!(1.22985018)) after an ambiguous HTTP outcome. The
    // server's pm_cash_withdrawal_status check bypasses require_evm_usdc_precision
    // for such retries; new eight-decimal submissions are rejected server-side.
    let response = serde_json::json!({
        "success": true,
        "request_id": "00000000-0000-0000-0000-000000000003",
        "directive_id": "00000000-0000-0000-0000-000000000003",
        "domain_status": "pending_chain_effect",
        "delivery_status": "pending_core_delivery",
        "message": "Portfolio-margin USDC withdrawal accepted"
    })
    .to_string();
    let destination = account(DESTINATION);
    let route = WithdrawUsdcRoute {
        destination,
        destination_dex: u32::MAX,
    };

    for amount in [dec!(1.22985018), dec!(10.12345678), dec!(0.71281536)] {
        let (base_url, request_task) = serve_once(response.clone()).await;
        let client = HypercallClient::new(base_url);
        let signer = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();

        client
            .withdraw_portfolio_usdc_with_route(
                &signer,
                WithdrawUsdcParams {
                    account: account(ACCOUNT),
                    destination,
                    amount,
                    nonce: Some(778),
                },
                route,
            )
            .await
            .unwrap_or_else(|e| panic!("expected success for legacy amount {amount}: {e:?}"));

        let request = request_task.await.unwrap();
        assert!(
            request.starts_with("POST /withdraw/pm-usdc HTTP/1.1\r\n"),
            "expected POST for legacy amount {amount}"
        );
    }
}

#[tokio::test]
async fn portfolio_withdrawal_rejects_sub_micro_amounts_before_route_preflight() {
    // withdraw_portfolio_usdc validates to six decimal places before issuing
    // the route GET, so no HTTP request is dispatched for sub-micro amounts.
    let response = serde_json::json!({
        "success": true,
        "request_id": "00000000-0000-0000-0000-000000000003",
        "directive_id": "00000000-0000-0000-0000-000000000003",
        "domain_status": "pending_chain_effect",
        "delivery_status": "pending_core_delivery",
        "message": "Portfolio-margin USDC withdrawal accepted"
    })
    .to_string();
    let destination = account(DESTINATION);

    for amount in [dec!(0.71281536), dec!(10.12345678), dec!(0.0000001)] {
        let (base_url, request_task) = serve_once(response.clone()).await;
        let client = HypercallClient::new(base_url);
        let signer = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();

        let error = client
            .withdraw_portfolio_usdc(
                &signer,
                WithdrawUsdcParams {
                    account: account(ACCOUNT),
                    destination,
                    amount,
                    nonce: Some(778),
                },
            )
            .await
            .unwrap_err();

        let ClientError::InvalidInput(message) = &error else {
            panic!("expected a client-side InvalidInput error for {amount}, got {error:?}");
        };
        assert!(
            message.contains("at most 6 decimal places"),
            "unexpected precision message for {amount}: {message}"
        );

        assert!(
            tokio::time::timeout(Duration::from_millis(100), request_task)
                .await
                .is_err(),
            "portfolio withdrawal of {amount} dispatched an HTTP request before validation",
        );
    }
}

#[tokio::test]
async fn portfolio_withdrawal_rejects_unsupported_server_route() {
    let route = serde_json::json!({
        "destination": DESTINATION,
        "destination_dex": 7,
    })
    .to_string();
    let (base_url, request_task) = serve_once(route).await;
    let client = HypercallClient::new(base_url);
    let signer = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();

    let error = client
        .withdraw_portfolio_usdc(
            &signer,
            WithdrawUsdcParams {
                account: account(ACCOUNT),
                destination: account(DESTINATION),
                amount: dec!(10),
                nonce: Some(778),
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        ClientError::InvalidInput(message)
            if message == "withdrawal route response returned an unsupported destination dex"
    ));
    let request = request_task.await.unwrap();
    assert!(request.starts_with("GET /withdraw/usdc/route?destination="));
}

#[tokio::test]
async fn withdrawal_surfaces_api_errors_without_route_fallback() {
    let (base_url, request_task) = serve_once_with_status(
        "400 Bad Request",
        serde_json::json!({"error": "insufficient withdrawable balance"}).to_string(),
    )
    .await;
    let client = HypercallClient::new(base_url);
    let signer = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();

    let error = client
        .withdraw_standard_usdc(
            &signer,
            WithdrawUsdcParams {
                account: account(ACCOUNT),
                destination: account(DESTINATION),
                amount: dec!(10),
                nonce: Some(777),
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(error, ClientError::Api { status: 400, .. }));
    let request = request_task.await.unwrap();
    assert!(request.starts_with("POST /withdraw/usdc HTTP/1.1\r\n"));
}

#[tokio::test]
async fn generated_withdrawal_nonce_is_exposed_when_submission_is_ambiguous() {
    let route = serde_json::json!({
        "destination": DESTINATION,
        "destination_dex": u32::MAX,
    })
    .to_string();
    let (base_url, request_task) = serve_once_then_without_response(route).await;
    let client = HypercallClient::new(base_url);
    let nonce_provider: Arc<dyn NonceProvider> = Arc::new(FixedNonce(779));
    let signer = HypercallWallet::from_private_key(PRIVATE_KEY, 999)
        .unwrap()
        .with_nonce_provider(nonce_provider);

    let error = client
        .withdraw_portfolio_usdc(
            &signer,
            WithdrawUsdcParams {
                account: account(ACCOUNT),
                destination: account(DESTINATION),
                amount: dec!(10.23000000),
                nonce: None,
            },
        )
        .await
        .unwrap_err();

    let ClientError::WithdrawalSubmissionAmbiguous {
        action,
        route,
        account: actual_account,
        destination: actual_destination,
        destination_dex,
        amount,
        nonce,
        source,
    } = &error
    else {
        panic!("expected a typed ambiguous-withdrawal error, got {error}");
    };
    assert_eq!(action, "Portfolio Margin USDC withdrawal");
    assert_eq!(route, "/withdraw/pm-usdc");
    assert_eq!(*actual_account, account(ACCOUNT));
    assert_eq!(*actual_destination, account(DESTINATION));
    assert_eq!(*destination_dex, Some(u32::MAX));
    assert_eq!(amount, "10.23");
    assert_eq!(*nonce, 779);
    assert!(matches!(source.as_ref(), ClientError::Http(_)));
    assert_eq!(
        error.to_string(),
        format!(
            "Portfolio Margin USDC withdrawal submission outcome is unknown: route=/withdraw/pm-usdc, account={actual_account}, destination={actual_destination}, destination_dex=Some({}), amount=10.23, nonce=779, submission_error={source}; reconcile or retry only with these identical fields",
            u32::MAX
        )
    );
    let requests = request_task.await.unwrap();
    assert!(requests[0].starts_with("GET /withdraw/usdc/route?destination="));
    assert!(requests[1].starts_with("POST /withdraw/pm-usdc HTTP/1.1\r\n"));
}

#[tokio::test]
async fn generated_withdrawal_nonce_is_exposed_for_ambiguous_server_error() {
    let (base_url, request_task) = serve_once_with_status(
        "503 Service Unavailable",
        serde_json::json!({"error": "node response unavailable"}).to_string(),
    )
    .await;
    let client = HypercallClient::new(base_url);
    let nonce_provider: Arc<dyn NonceProvider> = Arc::new(FixedNonce(780));
    let signer = HypercallWallet::from_private_key(PRIVATE_KEY, 999)
        .unwrap()
        .with_nonce_provider(nonce_provider);

    let error = client
        .withdraw_standard_usdc(
            &signer,
            WithdrawUsdcParams {
                account: account(ACCOUNT),
                destination: account(DESTINATION),
                amount: dec!(10.230000),
                nonce: None,
            },
        )
        .await
        .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("submission outcome is unknown"));
    assert!(message.contains("route=/withdraw/usdc"));
    assert!(message.contains("amount=10.23"));
    assert!(message.contains("nonce=780"));
    assert!(message.contains("API error (503)"));
    let request = request_task.await.unwrap();
    assert!(request.starts_with("POST /withdraw/usdc HTTP/1.1\r\n"));
}

#[test]
fn wallet_constant_matches_private_key() {
    let wallet = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();
    assert_eq!(wallet.address.as_hex(), WALLET);
    let wallet_address: WalletAddress = WALLET.parse().unwrap();
    assert_eq!(wallet.address.into_wallet_address(), wallet_address);
}

#[test]
fn evm_submission_locks_are_shared_per_sender() {
    let funder = Address::repeat_byte(0x91);
    let first = evm_submission_lock(funder);
    let second = evm_submission_lock(funder);
    let other = evm_submission_lock(Address::repeat_byte(0x92));

    assert!(Arc::ptr_eq(&first, &second));
    assert!(!Arc::ptr_eq(&first, &other));
    let _guard = first.try_lock().unwrap();
    assert!(second.try_lock().is_err());
    assert!(other.try_lock().is_ok());
}
