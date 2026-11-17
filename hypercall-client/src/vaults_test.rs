use super::*;
use crate::test_support::{json_body, serve_once, FixedNonce};
use crate::NonceProvider;
use alloy::sol_types::SolStruct;
use rust_decimal_macros::dec;
use std::str::FromStr;
use std::sync::Arc;

const PRIVATE_KEY: &str = "0x59c6995e998f97a5a0044966f0945389dc9e86dae2f36eb873f6fb9d8e20f6f8";
const OTHER_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
const CHAIN_ID: u32 = 999;
const NONCE: u64 = 1_761_000_000_000;

fn wallet(key: &str) -> HypercallWallet {
    let nonce_provider: Arc<dyn NonceProvider> = Arc::new(FixedNonce(NONCE));
    HypercallWallet::from_private_key(key, CHAIN_ID)
        .unwrap()
        .with_nonce_provider(nonce_provider)
}

fn recover<T: SolStruct>(signature: &str, message: &T) -> alloy::primitives::Address {
    let hash = message.eip712_signing_hash(&crate::wallet::hypercall_domain(CHAIN_ID));
    alloy::primitives::Signature::from_str(signature)
        .unwrap()
        .recover_address_from_prehash(&hash)
        .unwrap()
}

fn config() -> VaultConfigParams {
    VaultConfigParams {
        initial_share_price: dec!(1.00),
        min_seed_usdc: dec!(100),
        min_deposit_usdc: dec!(10.50),
        min_withdraw_usdc: dec!(10),
        cooldown_ms: 3_600_000,
        settle_interval_ms: 3_600_000,
        withdrawal_deadline_ms: 86_400_000,
        forced_unwind_slippage_bps: 200,
    }
}

#[test]
fn signed_types_match_the_server() {
    // hypercall-auth pins exactly these types; a change breaks every signature.
    assert_eq!(
        CreateVault::eip712_encode_type(),
        "CreateVault(address owner,uint64 salt,string initialSharePrice,string minSeedUsdc,string minDepositUsdc,string minWithdrawUsdc,uint64 cooldownMs,uint64 settleIntervalMs,uint64 withdrawalDeadlineMs,uint32 forcedUnwindSlippageBps,uint64 nonce)"
    );
    assert_eq!(
        VaultDeposit::eip712_encode_type(),
        "VaultDeposit(address holder,uint64 vault,string amount,uint64 nonce)"
    );
    assert_eq!(
        VaultWithdraw::eip712_encode_type(),
        "VaultWithdraw(address holder,uint64 vault,string shares,uint64 nonce)"
    );
    assert_eq!(
        VaultCancel::eip712_encode_type(),
        "VaultCancel(address holder,uint64 vault,uint64 nonce)"
    );
}

#[tokio::test]
async fn create_vault_posts_the_owner_signed_terms() {
    let response = serde_json::json!({
        "success": true,
        "request_id": "0192b1c4-7d2e-7a10-8000-000000000001",
        "vault_id": null,
        "account": "0xb2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2",
        "vault_manager": "0xc3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3",
        "create_vault_calldata": "0x01",
        "activate_vault_calldata": "0x02",
    });
    let (base_url, request_task) = serve_once(response.to_string()).await;
    let owner = wallet(PRIVATE_KEY);
    let created = HypercallClient::new(base_url)
        .create_vault(&owner, 42, &config())
        .await
        .unwrap();
    assert_eq!(created.vault_id, None);
    let request = request_task.await.unwrap();
    assert!(request.starts_with("POST /v1/vaults HTTP/1.1\r\n"));
    let body = json_body(&request);
    assert_eq!(body["owner"], owner.address.to_string());
    assert_eq!(body["config"]["initial_share_price"], "1");
    assert_eq!(body["config"]["min_deposit_usdc"], "10.5");
    let signed = CreateVault {
        owner: owner.address.as_wallet_address().inner(),
        salt: 42,
        initialSharePrice: "1".to_string(),
        minSeedUsdc: "100".to_string(),
        minDepositUsdc: "10.5".to_string(),
        minWithdrawUsdc: "10".to_string(),
        cooldownMs: 3_600_000,
        settleIntervalMs: 3_600_000,
        withdrawalDeadlineMs: 86_400_000,
        forcedUnwindSlippageBps: 200,
        nonce: NONCE,
    };
    assert_eq!(
        recover(body["signature"].as_str().unwrap(), &signed),
        owner.address.as_wallet_address().inner()
    );
}

#[tokio::test]
async fn holder_writes_post_signatures_that_only_the_holder_recovers() {
    let holder = wallet(PRIVATE_KEY);
    let other = wallet(OTHER_KEY).address.as_wallet_address().inner();
    let accepted = serde_json::json!({
        "success": true,
        "request_id": "0192b1c4-7d2e-7a10-8000-000000000002",
        "vault_id": 3,
    })
    .to_string();

    let (base_url, request_task) = serve_once(accepted.clone()).await;
    HypercallClient::new(base_url)
        .deposit_to_vault(&holder, 3, dec!(250.50))
        .await
        .unwrap();
    let request = request_task.await.unwrap();
    assert!(request.starts_with("POST /v1/vaults/3/deposit HTTP/1.1\r\n"));
    let body = json_body(&request);
    assert_eq!(body["amount"], "250.5");
    assert!(body.get("shares").is_none());
    let deposit = VaultDeposit {
        holder: holder.address.as_wallet_address().inner(),
        vault: 3,
        amount: "250.5".to_string(),
        nonce: NONCE,
    };
    let signature = body["signature"].as_str().unwrap();
    assert_eq!(recover(signature, &deposit), deposit.holder);
    assert_ne!(recover(signature, &deposit), other);
    // The signature binds the vault.
    assert_ne!(
        recover(
            signature,
            &VaultDeposit {
                vault: 4,
                ..deposit.clone()
            }
        ),
        deposit.holder
    );

    let (base_url, request_task) = serve_once(accepted.clone()).await;
    HypercallClient::new(base_url)
        .withdraw_from_vault(&holder, 3, dec!(12))
        .await
        .unwrap();
    let body = json_body(&request_task.await.unwrap());
    assert_eq!(body["shares"], "12");
    let withdraw = VaultWithdraw {
        holder: holder.address.as_wallet_address().inner(),
        vault: 3,
        shares: "12".to_string(),
        nonce: NONCE,
    };
    assert_eq!(
        recover(body["signature"].as_str().unwrap(), &withdraw),
        withdraw.holder
    );

    let (base_url, request_task) = serve_once(accepted).await;
    HypercallClient::new(base_url)
        .cancel_vault_requests(&holder, 3)
        .await
        .unwrap();
    let request = request_task.await.unwrap();
    assert!(request.starts_with("POST /v1/vaults/3/cancel HTTP/1.1\r\n"));
    let body = json_body(&request);
    let cancel = VaultCancel {
        holder: holder.address.as_wallet_address().inner(),
        vault: 3,
        nonce: NONCE,
    };
    assert_eq!(
        recover(body["signature"].as_str().unwrap(), &cancel),
        cancel.holder
    );
}

#[tokio::test]
async fn non_positive_amounts_are_refused_before_sending() {
    let client = HypercallClient::new("http://127.0.0.1:9");
    let holder = wallet(PRIVATE_KEY);
    for result in [
        client.deposit_to_vault(&holder, 3, dec!(0)).await,
        client.withdraw_from_vault(&holder, 3, dec!(-1)).await,
    ] {
        assert!(matches!(result, Err(ClientError::InvalidInput(_))));
    }
}

#[tokio::test]
async fn history_reads_page_and_decode() {
    let response = serde_json::json!({
        "vault_id": 3,
        "wallet": "0xa1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
        "fills": [{
            "seq": 7,
            "at_ms": 1_800_000_000_000_u64,
            "request_id": "0192b1c4-7d2e-7a10-8000-000000000003",
            "kind": "mint",
            "usdc": "50",
            "shares": "40",
            "share_price": "1.25",
        }],
        "next_before_seq": 7,
    });
    let (base_url, request_task) = serve_once(response.to_string()).await;
    let page = HypercallClient::new(base_url)
        .get_vault_history(
            3,
            "0xa1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1"
                .parse()
                .unwrap(),
            VaultPage {
                before_seq: Some(9),
                limit: Some(1),
            },
        )
        .await
        .unwrap();
    assert_eq!(page.fills[0].share_price, dec!(1.25));
    assert_eq!(page.next_before_seq, Some(7));
    let request = request_task.await.unwrap();
    assert!(request.starts_with(
        "GET /v1/vaults/3/history?before_seq=9&limit=1&wallet=0xa1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1 HTTP/1.1\r\n"
    ));
}

#[tokio::test]
async fn close_vault_posts_a_signature_only_the_owner_recovers() {
    assert_eq!(
        VaultClose::eip712_encode_type(),
        "VaultClose(address owner,uint64 vault,uint64 nonce)"
    );
    let accepted = serde_json::json!({
        "success": true,
        "request_id": "0192b1c4-7d2e-7a10-8000-000000000004",
        "vault_id": 3,
    });
    let (base_url, request_task) = serve_once(accepted.to_string()).await;
    let owner = wallet(PRIVATE_KEY);
    HypercallClient::new(base_url)
        .close_vault(&owner, 3)
        .await
        .unwrap();
    let request = request_task.await.unwrap();
    assert!(request.starts_with("POST /v1/vaults/3/close HTTP/1.1\r\n"));
    let body = json_body(&request);
    assert_eq!(body["owner"], owner.address.to_string());
    assert_eq!(body["nonce"], NONCE);
    let close = VaultClose {
        owner: owner.address.as_wallet_address().inner(),
        vault: 3,
        nonce: NONCE,
    };
    let signature = body["signature"].as_str().unwrap();
    assert_eq!(recover(signature, &close), close.owner);
    assert_ne!(
        recover(signature, &close),
        wallet(OTHER_KEY).address.as_wallet_address().inner()
    );
    assert_ne!(
        recover(
            signature,
            &VaultClose {
                vault: 4,
                ..close.clone()
            }
        ),
        close.owner
    );
}

#[tokio::test]
async fn status_changes_read_pages() {
    let response = serde_json::json!({
        "vault_id": 3,
        "changes": [{
            "at_ms": 5,
            "from": "frozen",
            "to": "closed",
            "signer": "owner",
            "signer_wallet": "0xa1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
            "cursor": 77,
        }],
        "next_before": 77,
    });
    let (base_url, request_task) = serve_once(response.to_string()).await;
    let page = HypercallClient::new(base_url)
        .get_vault_status_changes(
            3,
            VaultPage {
                before_seq: Some(90),
                limit: Some(1),
            },
        )
        .await
        .unwrap();
    assert_eq!(page.changes[0].to, "closed");
    assert_eq!(page.next_before, Some(77));
    assert!(request_task
        .await
        .unwrap()
        .starts_with("GET /v1/vaults/3/status-changes?before=90&limit=1 HTTP/1.1\r\n"));
}
