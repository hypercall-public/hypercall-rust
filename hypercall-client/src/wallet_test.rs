use super::*;

const TEST_PRIVATE_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

#[test]
fn invalid_private_key_preserves_the_public_error_text() {
    let invalid_private_key = "not-a-private-key";
    let source = PrivateKeySigner::from_str(invalid_private_key).unwrap_err();

    let error = HypercallWallet::from_private_key(invalid_private_key, 999).unwrap_err();

    assert!(matches!(
        &error,
        ClientError::Signing { operation, .. } if operation == "Invalid private key"
    ));
    assert_eq!(
        error.to_string(),
        format!("Signing error: Invalid private key: {source}")
    );
}

#[tokio::test]
async fn managed_account_directive_signatures_match_server_vectors() {
    let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 999).unwrap();
    let account: AccountAddress = "0x0000000000000000000000000000000000000002"
        .parse()
        .unwrap();
    let order = wallet
        .sign_perp_limit_order_payload(PerpLimitOrderSignature {
            account,
            nonce: 777,
            action: HlLimitOrderAction {
                asset: 3,
                is_buy: true,
                limit_px: 1_234_567_890,
                sz: 25_000_000,
                reduce_only: true,
                tif: hypercall_sdk_types::PerpTimeInForce::Ioc,
                cloid: 9_007_199_254_740_992,
            },
        })
        .await
        .unwrap();
    let oid = wallet
        .sign_perp_cancel_by_oid_payload(PerpCancelByOidSignature {
            account,
            nonce: 901,
            action: HlCancelByOidAction {
                asset: 4,
                oid: 9_007_199_254_740_992,
            },
        })
        .await
        .unwrap();
    let cloid = wallet
        .sign_perp_cancel_by_cloid_payload(PerpCancelByCloidSignature {
            account,
            nonce: 902,
            action: HlCancelByCloidAction {
                asset: 4,
                cloid: u128::MAX,
            },
        })
        .await
        .unwrap();
    let abstraction = wallet
        .sign_set_account_abstraction_payload(SetAccountAbstractionSignature {
            account,
            nonce: 778,
            action: HlSetAbstractionAction {
                user: account.into_wallet_address(),
                abstraction: hypercall_sdk_types::HypercoreAccountAbstraction::UnifiedAccount,
            },
        })
        .await
        .unwrap();
    let api_wallet_name = alloy::primitives::keccak256("primary-api-wallet");
    let api_wallet = wallet
        .sign_update_api_wallet_payload(UpdateApiWalletSignature {
            account,
            nonce: 779,
            action: HcUpdateApiWalletAction {
                name: api_wallet_name,
                addr: "0x0000000000000000000000000000000000000003"
                    .parse()
                    .unwrap(),
            },
        })
        .await
        .unwrap();

    assert_eq!(order, "0xf7e977300163d25cc71bead927b972b123a7ece0a4919b5ea109c4e4086898c3035318f89ce51a7a3386b7ac271ed10e7548f028c7cd2c01657e4938bfd546541c");
    assert_eq!(oid, "0x1a589059b0575e0b31a4e91741f88d7fa074617dcfa312503d9d854f2cef789263953415fc602d5611636c5f691d07c297f0473353753a2e303a16403df559b81b");
    assert_eq!(cloid, "0xf9cc6026ece7939e888f470909912ed9fd2afd661ce789a8bd36bceac9681fe43f4238a663aaecb3ef7f9373a8ba8ede95164587c9657e161752d578152007251b");
    assert_eq!(abstraction, "0x5f57e524f719d2ca6d5b917504b0d6450d14a407c0d59019c76fa0fae3c952346bac0ac4f1a1885bd66e2adb2d72060ba83999f27d801e88851fbb25767ef29f1c");
    assert_eq!(api_wallet, "0x5bb52141c7ee549345fa1db677cae5e0aa0fe4d04105d11d3a773b0b740ed43617e60c01f0f4e153ee832f6b3089271d907b173eb1c6137e44212302909f2bf21b");
}

#[tokio::test]
async fn usdc_withdrawal_signatures_match_server_vectors() {
    let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 999).unwrap();
    let account: AccountAddress = "0x0000000000000000000000000000000000000002"
        .parse()
        .unwrap();
    let destination: AccountAddress = "0x0000000000000000000000000000000000000003"
        .parse()
        .unwrap();
    let standard = wallet
        .sign_standard_usdc_withdrawal_payload(StandardUsdcWithdrawalSignature {
            wallet: wallet.address,
            account,
            destination,
            amount: "10.23",
            nonce: 777,
        })
        .await
        .unwrap();
    let portfolio = wallet
        .sign_portfolio_usdc_withdrawal_payload(PortfolioUsdcWithdrawalSignature {
            account,
            destination,
            amount_wei: 1_012_345_678,
            destination_dex: 0,
            nonce: 778,
        })
        .await
        .unwrap();

    assert_eq!(standard, "0xa4e75a273e3b6ddcb8a1545e6a365260398fb25a8cee5ebdd2e586e6b23e3773101cc18e3119f6b3f0dd9dcca9b28e115e6466ac5c032e6fe17ffc57988f20bb1b");
    let message = HLSendAsset {
        account: account.as_wallet_address().inner(),
        nonce: 778,
        action: SendAsset {
            destination: destination.as_wallet_address().inner(),
            subAccount: Address::ZERO,
            srcDex: u32::MAX,
            dstDex: 0,
            token: 0,
            amountWei: 1_012_345_678,
        },
    };
    let signature: alloy::primitives::Signature = portfolio.parse().unwrap();
    let hash =
        message.eip712_signing_hash(&wallet.directive_domain("HypercallManagerSign").unwrap());
    assert_eq!(
        signature.recover_address_from_prehash(&hash).unwrap(),
        wallet.address.as_wallet_address().inner()
    );
}

#[tokio::test]
async fn redeem_pool_shares_signs_the_server_type() {
    // hypercall-auth recovers exactly this type; keep them identical.
    assert_eq!(
        RedeemPoolShares::eip712_encode_type(),
        "RedeemPoolShares(address wallet,string shares,uint64 nonce)"
    );
    let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 999).unwrap();
    let signature = wallet.sign_redeem_pool_shares("100.5", 45).await.unwrap();
    let message = RedeemPoolShares {
        wallet: wallet.address.as_wallet_address().inner(),
        shares: "100.5".to_string(),
        nonce: 45,
    };
    let hash = message.eip712_signing_hash(&hypercall_domain(999));
    let recovered = alloy::primitives::Signature::from_str(&signature)
        .unwrap()
        .recover_address_from_prehash(&hash)
        .unwrap();
    assert_eq!(recovered, wallet.address.as_wallet_address().inner());
}

#[test]
fn cancel_pool_redemptions_signs_the_server_type() {
    assert_eq!(
        CancelPoolRedemptions::eip712_encode_type(),
        "CancelPoolRedemptions(address wallet,string requestIds,uint64 nonce)"
    );
}
