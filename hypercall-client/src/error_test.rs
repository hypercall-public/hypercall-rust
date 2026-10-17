use std::error::Error;

use alloy::primitives::{Address, B256};

use super::{ClientError, RfqInputError, SigningUnavailableReason};

#[test]
fn json_conversion_preserves_the_source_variant() {
    let source: sonic_rs::Error = sonic_rs::from_str::<i32>("not json").unwrap_err();
    let error: ClientError = source.into();

    assert!(matches!(error, ClientError::Json(_)));
    assert!(error.source().is_some());
}

#[test]
fn account_event_mismatch_preserves_addresses() {
    let observed = Address::repeat_byte(0x11);
    let expected = Address::repeat_byte(0x22);
    let error = ClientError::AccountCreatedManagerMismatch { observed, expected };

    assert!(matches!(
        error,
        ClientError::AccountCreatedManagerMismatch {
            observed: actual_observed,
            expected: actual_expected,
        } if actual_observed == observed && actual_expected == expected
    ));
}

#[test]
fn transaction_failure_preserves_its_source() {
    let error = ClientError::TransactionSubmissionUnknown {
        operation: "deposit".to_owned(),
        transaction_hash: B256::repeat_byte(0x33),
        source: Box::new(std::io::Error::other("transport stopped")),
    };

    assert!(matches!(
        &error,
        ClientError::TransactionSubmissionUnknown { operation, .. }
            if operation == "deposit"
    ));
    assert!(error.source().is_some());
}

#[test]
fn signing_context_preserves_its_source() {
    let error = ClientError::signing(
        "Failed to sign withdrawal",
        std::io::Error::other("signer unavailable"),
    );

    assert!(matches!(
        &error,
        ClientError::Signing { operation, .. } if operation == "Failed to sign withdrawal"
    ));
    assert!(error.source().is_some());
    assert_eq!(
        error.to_string(),
        "Signing error: Failed to sign withdrawal: signer unavailable"
    );
}

#[test]
fn unavailable_signing_display_preserves_the_public_contract() {
    let cases = [
        (
            SigningUnavailableReason::AwsKmsPrivateKeyMaterial,
            "Signing error: private key material is unavailable for AWS KMS wallets",
        ),
        (
            SigningUnavailableReason::HyperliquidMockApiUnauthenticated,
            "Signing error: Hyperliquid mock API is not authenticated",
        ),
        (
            SigningUnavailableReason::HyperliquidSimulationVenueUnauthenticated,
            "Signing error: Hyperliquid simulation venue is not authenticated",
        ),
    ];

    for (reason, expected) in cases {
        let error = ClientError::SigningUnavailable(reason);
        assert_eq!(error.to_string(), expected);
        assert!(error.source().is_some());
    }
}

#[test]
fn rfq_input_preserves_its_local_classification_source_and_legacy_display() {
    let source = "not-a-uuid".parse::<uuid::Uuid>().unwrap_err();
    let expected = format!("API error (0): Invalid rfq_id: {source}");
    let error = ClientError::RfqInput(RfqInputError::InvalidUuid {
        label: "Invalid rfq_id",
        source,
    });

    assert!(matches!(
        &error,
        ClientError::RfqInput(RfqInputError::InvalidUuid {
            label: "Invalid rfq_id",
            ..
        })
    ));
    let rfq_source = error.source().expect("RFQ input source");
    assert!(rfq_source.source().is_some());
    assert_eq!(error.to_string(), expected);
}

#[test]
fn rfq_input_without_source_preserves_legacy_api_zero_display() {
    let error = ClientError::RfqInput(RfqInputError::LegCountMismatch {
        request_count: 2,
        price_count: 1,
    });

    assert_eq!(
        error.to_string(),
        "API error (0): leg count mismatch: request has 2 legs but 1 prices provided"
    );
    assert!(matches!(
        error,
        ClientError::RfqInput(RfqInputError::LegCountMismatch {
            request_count: 2,
            price_count: 1,
        })
    ));
}

#[test]
fn reverted_transaction_display_preserves_legacy_hash_placement() {
    let transaction_hash = B256::repeat_byte(0x44);
    let approval = ClientError::TransactionReverted {
        operation: "HyperEVM USDC approval",
        transaction_hash: Some(transaction_hash),
    };
    let factory = ClientError::TransactionReverted {
        operation: "Factory account creation",
        transaction_hash: Some(transaction_hash),
    };

    assert_eq!(
        approval.to_string(),
        format!("HyperEVM USDC approval transaction {transaction_hash} reverted")
    );
    assert_eq!(
        factory.to_string(),
        "Factory account creation transaction reverted"
    );
}

#[test]
fn api_display_is_stable_at_the_public_boundary() {
    let error = ClientError::Api {
        status: 404,
        message: "Not found".to_owned(),
    };

    assert_eq!(error.to_string(), "API error (404): Not found");
}

#[test]
fn api_source_display_is_stable_and_preserves_its_source() {
    let error = ClientError::api_source(400, std::io::Error::other("venue rejected request"));

    assert_eq!(error.to_string(), "API error (400): venue rejected request");
    assert!(matches!(&error, ClientError::ApiSource { status: 400, .. }));
    assert!(error.source().is_some());
}

#[tokio::test]
async fn connection_errors_are_classified_structurally() {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_millis(50))
        .build()
        .unwrap();
    let source = client.get("http://127.0.0.1:1").send().await.unwrap_err();
    let error = ClientError::Http(source);

    assert!(matches!(&error, ClientError::Http(_)));
    assert!(error.is_connection_error());
}

#[test]
fn api_error_code_reads_the_body_after_the_action_prefix() {
    let unsupported = ClientError::Api {
        status: 400,
        message: r#"submit TEE perp TWAP: {"error":"unsupported_action","code":"unsupported_action","message":"TWAP is not enabled by every admitted TEE release"}"#.into(),
    };
    assert_eq!(
        unsupported.api_error_code().as_deref(),
        Some("unsupported_action")
    );
    assert!(unsupported.is_unsupported_action());

    let invalid = ClientError::Api {
        status: 400,
        message: r#"submit TEE perp batch modify: {"code":"invalid_field","message":"action.modifies may contain at most 50 orders"}"#.into(),
    };
    assert_eq!(invalid.api_error_code().as_deref(), Some("invalid_field"));
    assert!(!invalid.is_unsupported_action());

    let plain = ClientError::Api {
        status: 502,
        message: "submit TEE perp TWAP: bad gateway".into(),
    };
    assert_eq!(plain.api_error_code(), None);
    assert!(!ClientError::InvalidInput("x".into()).is_unsupported_action());
}
