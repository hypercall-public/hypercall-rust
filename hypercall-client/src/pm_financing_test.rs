use super::*;
use crate::test_support::{json_body, serve_once, serve_once_with_status, FixedNonce};
use crate::NonceProvider;
use alloy::sol_types::SolStruct;
use rust_decimal_macros::dec;
use std::str::FromStr;
use std::sync::Arc;

/// hypercall-auth's `signature.rs` test key and chain.
const SERVER_TEST_PRIVATE_KEY: &str =
    "0x59c6995e998f97a5a0044966f0945389dc9e86dae2f36eb873f6fb9d8e20f6f8";
const SERVER_TEST_CHAIN_ID: u32 = 998;
/// The signature `SignatureRecovery::recover_repay_pm_loan_signer` recovers
/// to the key above for account `0xa1..a1`, `MU`, `all`, nonce 7 on chain 998.
const SERVER_REPAY_ALL_SIGNATURE: &str = "0xdc779979af70ad0c595808dd618b0f88ac085f64871164e9fa73306c8e2b42b55db17666bea1a0216784224318d35c787a7311fb1a3231fc1953ae1e3aad2d0e1b";
const ACCOUNT: &str = "0xa1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";

fn account() -> AccountAddress {
    ACCOUNT.parse().unwrap()
}

fn recover(signature: &str, message: &RepayPmLoan, chain_id: u32) -> alloy::primitives::Address {
    let hash = message.eip712_signing_hash(&crate::wallet::hypercall_domain(chain_id));
    alloy::primitives::Signature::from_str(signature)
        .unwrap()
        .recover_address_from_prehash(&hash)
        .unwrap()
}

#[test]
fn repay_pm_loan_signs_the_server_type() {
    // hypercall-auth pins exactly this type; changing it breaks every signature.
    assert_eq!(
        RepayPmLoan::eip712_encode_type(),
        "RepayPmLoan(address wallet,address account,string underlying,string amount,uint64 nonce)"
    );
}

#[tokio::test]
async fn repay_pm_loan_signature_matches_the_server_vector() {
    let wallet =
        HypercallWallet::from_private_key(SERVER_TEST_PRIVATE_KEY, SERVER_TEST_CHAIN_ID).unwrap();
    let signature = wallet
        .sign_repay_pm_loan(account(), "MU", "all", 7)
        .await
        .unwrap();
    assert_eq!(signature, SERVER_REPAY_ALL_SIGNATURE);

    let signed = RepayPmLoan {
        wallet: wallet.address.as_wallet_address().inner(),
        account: account().as_wallet_address().inner(),
        underlying: "MU".to_string(),
        amount: "all".to_string(),
        nonce: 7,
    };
    let signer = wallet.address.as_wallet_address().inner();
    assert_eq!(recover(&signature, &signed, SERVER_TEST_CHAIN_ID), signer);
    for changed in [
        RepayPmLoan {
            underlying: "BTC".to_string(),
            ..signed.clone()
        },
        RepayPmLoan {
            amount: "60".to_string(),
            ..signed.clone()
        },
        RepayPmLoan {
            nonce: 8,
            ..signed.clone()
        },
    ] {
        assert_ne!(recover(&signature, &changed, SERVER_TEST_CHAIN_ID), signer);
    }
    assert_ne!(recover(&signature, &signed, 999), signer);
}

#[test]
fn repay_amount_signs_the_normalized_string() {
    assert_eq!(RepayAmount::All.signed_amount().unwrap(), "all");
    assert_eq!(
        RepayAmount::Usdc(dec!(60.500)).signed_amount().unwrap(),
        "60.5"
    );
    assert_eq!(RepayAmount::Usdc(dec!(100)).signed_amount().unwrap(), "100");
    assert_eq!(
        RepayAmount::Usdc(dec!(0.000001)).signed_amount().unwrap(),
        "0.000001"
    );
    for invalid in [dec!(0), dec!(-1), dec!(1.0000001)] {
        assert!(matches!(
            RepayAmount::Usdc(invalid).signed_amount(),
            Err(ClientError::InvalidInput(_))
        ));
    }
}

#[tokio::test]
async fn repay_pm_loan_posts_the_signed_request() {
    let response = serde_json::json!({
        "account": ACCOUNT,
        "request_id": "0192b1c4-7d2e-7a10-8000-000000000001",
        "plan_id": "pm-voluntary-repay-0192b1c4",
        "nonce": 1_761_000_000_000_u64,
    });
    let (base_url, request_task) = serve_once(response.to_string()).await;
    let client = HypercallClient::new(base_url);
    let nonce_provider: Arc<dyn NonceProvider> = Arc::new(FixedNonce(1_761_000_000_000));
    let manager = HypercallWallet::from_private_key(SERVER_TEST_PRIVATE_KEY, 999)
        .unwrap()
        .with_nonce_provider(nonce_provider);

    let accepted = client
        .repay_pm_loan(
            &manager,
            RepayPmLoanParams {
                account: account(),
                underlying: "MU".to_string(),
                amount: RepayAmount::Usdc(dec!(60.50)),
                nonce: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(accepted.plan_id, "pm-voluntary-repay-0192b1c4");
    assert_eq!(accepted.nonce, 1_761_000_000_000);
    let request = request_task.await.unwrap();
    assert!(request.starts_with("POST /v1/pm-financing/repay HTTP/1.1\r\n"));
    let body = json_body(&request);
    let manager_address = manager.address.as_wallet_address().inner();
    assert_eq!(body["wallet"], manager.address.to_string());
    assert_eq!(body["account"], ACCOUNT);
    assert_eq!(body["underlying"], "MU");
    assert_eq!(body["amount"], "60.5");
    assert_eq!(body["nonce"], 1_761_000_000_000_u64);
    let signed = RepayPmLoan {
        wallet: manager_address,
        account: account().as_wallet_address().inner(),
        underlying: "MU".to_string(),
        amount: "60.5".to_string(),
        nonce: 1_761_000_000_000,
    };
    assert_eq!(
        recover(body["signature"].as_str().unwrap(), &signed, 999),
        manager_address
    );
}

#[tokio::test]
async fn repay_pm_loan_rejects_invalid_input_before_sending() {
    let client = HypercallClient::new("http://127.0.0.1:9");
    let manager = HypercallWallet::from_private_key(SERVER_TEST_PRIVATE_KEY, 999).unwrap();
    for (underlying, amount) in [("", RepayAmount::All), ("MU", RepayAmount::Usdc(dec!(0)))] {
        let error = client
            .repay_pm_loan(
                &manager,
                RepayPmLoanParams {
                    account: account(),
                    underlying: underlying.to_string(),
                    amount,
                    nonce: Some(1),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(error, ClientError::InvalidInput(_)));
    }
}

#[tokio::test]
async fn repay_pm_loan_surfaces_engine_rejections() {
    let (base_url, _request_task) = serve_once_with_status(
        "409 Conflict",
        serde_json::json!({
            "error": "pm_voluntary_repayment_insufficient_withdrawable",
            "code": "pm_voluntary_repayment_insufficient_withdrawable",
            "message": "amount exceeds withdrawable",
        })
        .to_string(),
    )
    .await;
    let client = HypercallClient::new(base_url);
    let manager = HypercallWallet::from_private_key(SERVER_TEST_PRIVATE_KEY, 999).unwrap();

    let error = client
        .repay_pm_loan(
            &manager,
            RepayPmLoanParams {
                account: account(),
                underlying: "MU".to_string(),
                amount: RepayAmount::All,
                nonce: Some(1),
            },
        )
        .await
        .unwrap_err();

    assert_eq!(
        PmRepayRejection::from_error(&error),
        Some(PmRepayRejection::InsufficientWithdrawable)
    );
    let ClientError::Api { status, message } = error else {
        panic!("expected an API error, got {error:?}");
    };
    assert_eq!(status, 409);
    assert!(message.contains("pm_voluntary_repayment_insufficient_withdrawable"));
}

fn financing_body() -> serde_json::Value {
    serde_json::json!({
        "wallet": "0x00000000000000000000000000000000000000b0",
        "account": ACCOUNT,
        "is_pm_account": true,
        "measured_at_ms": 1_761_000_000_000_u64,
        "borrow_apr": "0.08",
        "items": [
            {
                "kind": "loan",
                "underlying": "MU",
                "expiry_ts_ms": null,
                "principal_usdc": "1000",
                "interest_usdc": "1.25",
                "paid_usdc": null,
                "owed_usdc": "1001.25",
                "since_ms": 1_760_000_000_000_u64,
                "deadline_ms": null,
                "status": "open",
            },
            {
                "kind": "settlement",
                "underlying": "BTC",
                "expiry_ts_ms": 1_760_500_000_000_i64,
                "principal_usdc": "50",
                "interest_usdc": "0",
                "paid_usdc": "10",
                "owed_usdc": "40",
                "since_ms": 1_760_500_000_000_u64,
                "deadline_ms": 1_760_600_000_000_u64,
                "status": "grace_period",
            },
        ],
        "totals": {
            "loans_owed_usdc": "1001.25",
            "settlements_owed_usdc": "40",
            "total_owed_usdc": "1041.25",
            "missed_count": 0,
        },
        "margin": {
            "equity_usdc": "5000",
            "maintenance_requirement_usdc": "1200",
            "margin_utilization": "0.24",
        },
        "box_financing": null,
    })
}

#[tokio::test]
async fn get_pm_financing_reads_the_repayment_status() {
    let mut body = financing_body();
    body["repayment"] = serde_json::json!({
        "unreserved_withdrawable_usdc": "250.5",
        "blocked_reason": null,
        "requests": [{
            "plan_id": "pm-voluntary-repay-0192b1c4",
            "underlying": "MU",
            "amount_usdc": "60.5",
            "status": "Submitted",
            "updated_at_ms": 1_761_000_000_500_u64,
        }],
    });
    let (base_url, request_task) = serve_once(body.to_string()).await;
    let client = HypercallClient::new(base_url);

    let financing = client.get_pm_financing(account()).await.unwrap();

    let request = request_task.await.unwrap();
    assert!(request.starts_with(&format!(
        "GET /v1/pm-financing?wallet={ACCOUNT} HTTP/1.1\r\n"
    )));
    assert_eq!(financing.account, Some(account()));
    assert_eq!(financing.items[0].kind, PmFinancingItemKind::Loan);
    assert_eq!(
        financing.items[1].status,
        PmFinancingItemStatus::GracePeriod
    );
    assert_eq!(financing.totals.total_owed_usdc, dec!(1041.25));
    let repayment = financing.repayment.unwrap();
    assert_eq!(repayment.unreserved_withdrawable_usdc, Some(dec!(250.5)));
    assert_eq!(repayment.blocked_reason, None);
    assert_eq!(repayment.requests[0].amount_usdc, dec!(60.5));
    assert_eq!(repayment.requests[0].status, PmRepaymentStatus::Submitted);
    assert!(!repayment.requests[0].status.is_terminal());
}

#[test]
fn repayment_status_and_blocked_reason_mirror_the_server_strings() {
    let requests: Vec<_> = [
        "Planned",
        "Submitted",
        "Confirmed",
        "Failed",
        "TimedOut",
        "Canceled",
        "EscalatedManual",
        "PartiallyRepaid",
        "Repaid",
        "SomethingNew",
    ]
    .iter()
    .map(|status| {
        serde_json::json!({
            "plan_id": "p",
            "underlying": "MU",
            "amount_usdc": "1",
            "status": status,
            "updated_at_ms": 1,
        })
    })
    .collect();
    let repayment: PmFinancingRepayment = serde_json::from_value(serde_json::json!({
        "unreserved_withdrawable_usdc": null,
        "blocked_reason": "recovery_in_progress",
        "requests": requests,
    }))
    .unwrap();
    assert_eq!(repayment.unreserved_withdrawable_usdc, None);
    let blocked = repayment.blocked_reason.unwrap();
    assert_eq!(blocked, PmRepaymentBlockedReason::RecoveryInProgress);
    assert_eq!(
        blocked.rejection(),
        Some(PmRepayRejection::RecoveryInProgress)
    );
    let statuses: Vec<_> = repayment.requests.iter().map(|r| r.status).collect();
    use PmRepaymentStatus as S;
    assert_eq!(
        statuses,
        [
            S::Planned,
            S::Submitted,
            S::Confirmed,
            S::Failed,
            S::TimedOut,
            S::Canceled,
            S::EscalatedManual,
            S::PartiallyRepaid,
            S::Repaid,
            S::Unknown,
        ]
    );
    let terminal: Vec<_> = statuses.into_iter().filter(|s| s.is_terminal()).collect();
    assert_eq!(terminal, [S::Canceled, S::EscalatedManual, S::Repaid]);

    for (raw, reason, rejection) in [
        (
            "account_unhealthy",
            PmRepaymentBlockedReason::AccountUnhealthy,
            Some(PmRepayRejection::AccountUnhealthy),
        ),
        ("future_reason", PmRepaymentBlockedReason::Unknown, None),
    ] {
        let parsed: PmRepaymentBlockedReason = serde_json::from_value(raw.into()).unwrap();
        assert_eq!(parsed, reason);
        assert_eq!(parsed.rejection(), rejection);
    }
}

#[test]
fn repay_rejection_codes_match_the_server_catalog() {
    // hypercall-runtime-api's error catalog publishes these codes.
    let codes: Vec<_> = PmRepayRejection::ALL.iter().map(|r| r.code()).collect();
    assert_eq!(
        codes,
        [
            "pm_voluntary_repayment_unavailable",
            "pm_voluntary_repayment_nonce_out_of_bounds",
            "pm_voluntary_repayment_nonce_reused",
            "pm_voluntary_repayment_unknown_underlying",
            "pm_voluntary_repayment_unauthorized",
            "pm_voluntary_repayment_account_unhealthy",
            "pm_voluntary_repayment_recovery_in_progress",
            "pm_voluntary_repayment_nothing_owed",
            "pm_voluntary_repayment_invalid_amount",
            "pm_voluntary_repayment_below_minimum",
            "pm_voluntary_repayment_exceeds_owed",
            "pm_voluntary_repayment_insufficient_withdrawable",
            "pm_voluntary_repayment_rejected",
            "forbidden",
            "invalid_nonce",
            "signature_verification_failed",
        ]
    );
    for rejection in PmRepayRejection::ALL {
        assert_eq!(
            PmRepayRejection::from_code(rejection.code()),
            Some(rejection)
        );
    }
    assert_eq!(PmRepayRejection::from_code("rate_limited"), None);
    assert!(PmRepayRejection::NonceOutOfBounds.is_resignable());
    assert!(!PmRepayRejection::InsufficientWithdrawable.is_resignable());
}

#[test]
fn repay_rejection_reads_the_code_from_an_api_error() {
    let error = ClientError::Api {
        status: 409,
        message: r#"repay PM loan: {"error":"pm_voluntary_repayment_exceeds_owed","code":"pm_voluntary_repayment_exceeds_owed","message":"amount 70 exceeds owed 60.000001"}"#
            .to_string(),
    };
    assert_eq!(
        PmRepayRejection::from_error(&error),
        Some(PmRepayRejection::ExceedsOwed)
    );
    assert_eq!(
        PmRepayRejection::from_error(&ClientError::InvalidInput("x".to_string())),
        None
    );
}

#[tokio::test]
async fn repay_pm_loan_exact_amount_matches_the_viem_vector() {
    // Signed independently with viem (the frontend's signer) over the same
    // type and Hypercall domain on chain 999.
    const VIEM_REPAY_60_5_SIGNATURE: &str = "0x8581ae136bc6c7598f25da55ac91c4170c63fb5055b1b3a778b4676533ed04175bb8c3147e7cf80ffe081f43f66d1f35de544fa3fb501b977a0f8da4429ba4251b";
    let wallet = HypercallWallet::from_private_key(SERVER_TEST_PRIVATE_KEY, 999).unwrap();
    let amount = RepayAmount::Usdc(dec!(60.500)).signed_amount().unwrap();
    let signature = wallet
        .sign_repay_pm_loan(account(), "MU", &amount, 1_761_000_000_000)
        .await
        .unwrap();
    assert_eq!(signature, VIEM_REPAY_60_5_SIGNATURE);
}

#[tokio::test]
async fn get_pm_financing_accepts_a_server_without_repayment() {
    let (base_url, _request_task) = serve_once(financing_body().to_string()).await;
    let client = HypercallClient::new(base_url);

    let financing = client.get_pm_financing(account()).await.unwrap();

    assert!(financing.repayment.is_none());
    assert!(financing.box_financing.is_none());
}
