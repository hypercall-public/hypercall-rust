use super::*;
use alloy::providers::ProviderBuilder;
use alloy::sol_types::{SolCall, SolValue};
use alloy::transports::mock::Asserter;

#[test]
fn account_salt_defaults_to_zero() {
    assert_eq!(validate_salt(None).unwrap(), U96::ZERO);
}

#[test]
fn account_salt_accepts_uint96_maximum() {
    assert_eq!(validate_salt(Some(MAX_U96)).unwrap(), U96::MAX);
    assert_eq!(validate_salt(Some(42)).unwrap(), U96::from(42));
}

#[test]
fn account_salt_rejects_value_above_uint96() {
    let invalid_salt = 1_u128 << 96;
    let error = validate_salt(Some(invalid_salt)).unwrap_err();
    assert!(matches!(
        &error,
        ClientError::InvalidInput(message)
            if message == &format!("account salt {invalid_salt} exceeds uint96")
    ));
    assert_eq!(
        error.to_string(),
        format!("invalid input: account salt {invalid_salt} exceeds uint96")
    );
}

#[tokio::test]
async fn invalid_rpc_url_preserves_the_public_error_text() {
    let invalid_rpc_url = "://";
    let source = invalid_rpc_url.parse::<reqwest::Url>().unwrap_err();
    let client = HypercallClient::new("http://unused.invalid");
    let manager = HypercallWallet::random(999);

    let error = client
        .create_account(
            &manager,
            CreateAccountParams {
                rpc_url: invalid_rpc_url.to_owned(),
                factory_address: AccountAddress::from(Address::repeat_byte(5)),
                salt: None,
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(
        &error,
        ClientError::InvalidInputSource { context, .. } if context == "HyperEVM RPC URL"
    ));
    assert_eq!(
        error.to_string(),
        format!("invalid input: invalid HyperEVM RPC URL: {source}")
    );
}

#[test]
fn creation_deposit_buffer_matches_frontend_rounding() {
    assert_eq!(
        buffered_u256(U256::from(100), CREATION_DEPOSIT_BUFFER_BPS).unwrap(),
        U256::from(101)
    );
    assert_eq!(
        buffered_u256(U256::from(101), CREATION_DEPOSIT_BUFFER_BPS).unwrap(),
        U256::from(103)
    );
    assert_eq!(
        buffered_u256(U256::ZERO, CREATION_DEPOSIT_BUFFER_BPS).unwrap(),
        U256::ZERO
    );
}

#[test]
fn gas_limit_buffer_matches_frontend_rounding() {
    assert_eq!(buffered_u64(100, GAS_LIMIT_BUFFER_BPS).unwrap(), 120);
    assert_eq!(buffered_u64(101, GAS_LIMIT_BUFFER_BPS).unwrap(), 122);
}

#[test]
fn creation_deposit_buffer_fails_on_overflow() {
    let error = buffered_u256(U256::MAX, CREATION_DEPOSIT_BUFFER_BPS).unwrap_err();
    assert!(matches!(
        &error,
        ClientError::ArithmeticOverflow {
            operation: "account creation deposit buffer"
        }
    ));
    assert_eq!(
        error.to_string(),
        "account creation deposit buffer overflow"
    );
}

#[test]
fn chain_id_mismatch_fails_closed() {
    assert!(validate_chain_id(999, 999).is_ok());
    let error = validate_chain_id(998, 999).unwrap_err();
    assert!(error.to_string().contains("does not match wallet chain ID"));
}

#[test]
fn account_created_event_is_required() {
    let manager = Address::repeat_byte(1);
    let account = Address::repeat_byte(2);
    let error = validate_account_created_event(None, manager, account).unwrap_err();
    assert!(matches!(error, ClientError::MissingAccountCreatedEvent));
}

#[test]
fn account_created_event_must_match_manager_and_simulation() {
    let manager = Address::repeat_byte(1);
    let account = Address::repeat_byte(2);
    let event = AccountFactory::AccountCreated { account, manager };
    assert_eq!(
        validate_account_created_event(Some(&event), manager, account).unwrap(),
        AccountAddress::from(account)
    );

    let error =
        validate_account_created_event(Some(&event), Address::repeat_byte(3), account).unwrap_err();
    assert!(matches!(
        error,
        ClientError::AccountCreatedManagerMismatch {
            observed,
            expected,
        } if observed == manager && expected == Address::repeat_byte(3)
    ));

    let error =
        validate_account_created_event(Some(&event), manager, Address::repeat_byte(4)).unwrap_err();
    assert!(matches!(
        error,
        ClientError::AccountCreatedAddressMismatch {
            observed,
            expected,
        } if observed == account && expected == Address::repeat_byte(4)
    ));
}

#[test]
fn create_account_transaction_uses_expected_calldata_value_and_gas() {
    let provider = ProviderBuilder::new().connect_mocked_client(Asserter::new());
    let factory_address = Address::repeat_byte(5);
    let manager = Address::repeat_byte(1);
    let salt = U96::from(42);
    let hype_value = U256::from(103);
    let gas_limit = 122;
    let request = AccountFactory::new(factory_address, provider)
        .createAccount(salt)
        .value(hype_value)
        .gas(gas_limit)
        .from(manager)
        .into_transaction_request();

    assert_eq!(request.value, Some(hype_value));
    assert_eq!(request.gas, Some(gas_limit));
    assert_eq!(
        request.input.input.expect("createAccount calldata"),
        alloy::primitives::Bytes::from(AccountFactory::createAccountCall { salt }.abi_encode())
    );
}

#[tokio::test]
async fn contract_read_failure_is_contextualized() {
    let asserter = Asserter::new();
    asserter.push_failure_msg("creation deposit unavailable");
    let provider = ProviderBuilder::new().connect_mocked_client(asserter);

    let error = create_account_on_provider(
        &provider,
        AccountAddress::from(Address::repeat_byte(1)),
        Address::repeat_byte(5),
        U96::ZERO,
    )
    .await
    .unwrap_err();

    assert!(matches!(
        &error,
        ClientError::Onchain { operation, .. }
            if operation == "read Factory creation deposit"
    ));
    assert!(std::error::Error::source(&error).is_some());
}

#[tokio::test]
async fn simulation_failure_is_contextualized() {
    let asserter = Asserter::new();
    let creation_deposit = U256::from(100).abi_encode();
    asserter.push_success(&alloy::primitives::Bytes::from(creation_deposit));
    asserter.push_failure_msg("account already exists");
    let provider = ProviderBuilder::new().connect_mocked_client(asserter);

    let error = create_account_on_provider(
        &provider,
        AccountAddress::from(Address::repeat_byte(1)),
        Address::repeat_byte(5),
        U96::ZERO,
    )
    .await
    .unwrap_err();

    assert!(matches!(
        &error,
        ClientError::Onchain { operation, .. }
            if operation == "simulate Factory account creation"
    ));
    assert!(std::error::Error::source(&error).is_some());
}
