use alloy::primitives::Signature;
use hypercall_sdk_types::{
    RfqV2AutoExecutionLimit, RfqV2BuilderAttribution, RfqV2PackageLeg, RfqV2QuoteRef,
    RfqV2Signature,
};
use proptest::prelude::*;

use super::*;

const KEY: &str = "0x59c6995e998f97a5a0044966f094538e5b2f9f8c9360f0c2b3b8a4e1f2f8f96d";
const CHAIN_ID: u64 = 998;

fn wallet() -> HypercallWallet {
    HypercallWallet::from_private_key(KEY, CHAIN_ID as u32).unwrap()
}

fn account() -> WalletAddress {
    wallet().address.into_wallet_address()
}

fn auth(n: u128) -> RfqV2ActionAuth {
    RfqV2ActionAuth {
        operation_id: Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0000 + n),
        expires_at_ms: 1_900_000_050_000,
        nonce: 1_900_000_000_000 + n as u64,
    }
}

fn terms() -> RfqV2Terms {
    RfqV2Terms {
        id: Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001),
        taker_account: account(),
        legs: vec![
            RfqV2PackageLeg {
                instrument: "BTC-20300125-100000-C".into(),
                taker_side: RfqV2Side::Buy,
                quantity_micros_per_unit: 1_000_000,
            },
            RfqV2PackageLeg {
                instrument: "BTC-20300125-90000-P".into(),
                taker_side: RfqV2Side::Sell,
                quantity_micros_per_unit: 1_000_000,
            },
        ],
        target: 5_000_000,
        fill_policy: RfqV2FillPolicy::RepeatedPartial {
            minimum: 1_000_000,
            step: 1_000_000,
        },
        execution_mode: RfqV2ExecutionMode::Manual,
        auto_execution_limit: None,
        quote_policy: RfqV2QuotePolicy::IndicativeAllowed,
        taker_identity: RfqV2TakerIdentity::Anonymous,
        routing: RfqV2Routing::selected_makers(
            vec![
                WalletAddress::from([0x22; 20]),
                WalletAddress::from([0x11; 20]),
            ],
            128,
        )
        .unwrap(),
        quote_deadline_ms: 1_900_000_030_000,
        expires_at_ms: 1_900_000_060_000,
        fee_limits: RfqV2FeeLimits {
            per_execution: 0,
            cumulative: 0,
        },
        builder: None,
        private_terms_blinding: RfqV2Digest([0x5b; 32].into()),
    }
}

fn firm_terms() -> RfqV2FirmQuoteTerms {
    RfqV2FirmQuoteTerms {
        id: Uuid::from_u128(0xf),
        rfq_id: terms().id,
        rfq_terms_commitment: rfq_v2_terms_commitment(&terms()).unwrap(),
        maker_account: account(),
        maker_generation: 1,
        rfq_generation: 0,
        priced_legs: vec![
            RfqV2PricedLeg {
                instrument: "BTC-20300125-100000-C".into(),
                price_micros: 2_000_000,
            },
            RfqV2PricedLeg {
                instrument: "BTC-20300125-90000-P".into(),
                price_micros: 500_000,
            },
        ],
        reference_quantity: 1_000_000,
        maker_receives_reference_premium: true,
        reference_premium_micros: 1_500_000,
        capacity: 2_000_000,
        fill_policy: terms().fill_policy,
        withdrawal_policy: RfqV2WithdrawalPolicy::Permitted,
        fee_limits: terms().fee_limits,
        expires_at_ms: 1_900_000_050_000,
        protection_scope: None,
        source_indicative_quote_id: None,
    }
}

fn indicative_terms() -> RfqV2IndicativeQuoteTerms {
    let firm = firm_terms();
    RfqV2IndicativeQuoteTerms {
        id: Uuid::from_u128(0xe),
        rfq_id: firm.rfq_id,
        rfq_terms_commitment: firm.rfq_terms_commitment,
        maker_account: firm.maker_account,
        priced_legs: firm.priced_legs,
        reference_quantity: firm.reference_quantity,
        maker_receives_reference_premium: firm.maker_receives_reference_premium,
        reference_premium_micros: firm.reference_premium_micros,
        indicated_capacity: firm.capacity,
        expires_at_ms: firm.expires_at_ms,
    }
}

fn signer_of<T: SolStruct>(message: &T, signature: &RfqV2Signature) -> WalletAddress {
    let signature = Signature::try_from(signature.as_bytes().as_slice()).unwrap();
    WalletAddress::from(
        signature
            .recover_address_from_prehash(&rfq_v2_signing_hash(message, CHAIN_ID))
            .unwrap(),
    )
}

fn assert_invalid(result: Result<impl std::fmt::Debug>, needle: &str) {
    match result {
        Err(ClientError::InvalidInput(message)) => assert!(
            message.contains(needle),
            "expected {needle:?} in {message:?}"
        ),
        other => panic!("expected InvalidInput containing {needle:?}, got {other:?}"),
    }
}

#[test]
fn instrument_id_is_keccak_of_the_prefixed_symbol() {
    assert_eq!(
        rfq_v2_instrument_id("BTC-20261225-100000-C"),
        keccak256(b"instrumentBTC-20261225-100000-C")
    );
}

#[test]
fn type_strings_match_the_published_eip712_definitions() {
    assert_eq!(
        ExecuteRfqV2::eip712_encode_type(),
        "ExecuteRfqV2(bytes16 rfqId,bytes16 operationId,RfqV2ExecutionAllocation[] allocations,\
         uint128 maxFeeMicros,uint64 expiresAtMs,uint64 nonce)\
         RfqV2ExecutionAllocation(bytes16 quoteId,uint128 packageUnits)"
    );
    assert_eq!(
        DeclineRfqV2::eip712_encode_type(),
        "DeclineRfqV2(bytes16 rfqId,address makerAccount,bytes16 operationId,\
         uint64 expiresAtMs,uint64 nonce)"
    );
    assert_eq!(
        AuthorizeRfqV2::eip712_encode_type(),
        "AuthorizeRfqV2(RfqV2SigningTerms terms,uint64 nonce)\
         RfqV2SigningFeeLimits(uint128 perExecutionMicros,uint128 cumulativeMicros)\
         RfqV2SigningLeg(bytes32 instrumentId,uint8 takerSide,uint128 quantityMicrosPerUnit)\
         RfqV2SigningTerms(bytes16 rfqId,address takerAccount,RfqV2SigningLeg[] legs,\
         uint128 targetUnits,uint8 fillPolicy,uint128 minimumUnits,uint128 stepUnits,\
         uint8 executionMode,bool autoMakerReceivesPremium,uint128 autoPremiumLimitMicros,\
         uint8 quotePolicy,uint8 takerIdentity,uint8 routingMode,address[] selectedMakers,\
         uint64 quoteDeadlineMs,uint64 expiresAtMs,RfqV2SigningFeeLimits feeLimits,\
         address builder,bytes32 privateTermsBlinding)"
    );
}

#[tokio::test]
async fn every_signed_request_recovers_to_the_wallet() {
    let wallet = wallet();
    let me = account();

    let create = wallet.sign_create_rfq_v2(terms(), 41).await.unwrap();
    let message = rfq_v2_create_message(&create.terms, create.nonce).unwrap();
    assert_eq!(signer_of(&message, &create.signature), me);

    let firm = wallet
        .sign_rfq_v2_firm_quote(firm_terms(), 7)
        .await
        .unwrap();
    let message = rfq_v2_firm_quote_message(&firm.terms, firm.nonce);
    assert_eq!(signer_of(&message, &firm.signature), me);

    let indicative = wallet
        .sign_rfq_v2_indicative_quote(indicative_terms())
        .await
        .unwrap();
    let message = rfq_v2_indicative_quote_message(&indicative.terms);
    assert_eq!(signer_of(&message, &indicative.signature), me);

    let rfq_id = terms().id;
    let firm_up = wallet
        .sign_rfq_v2_firm_up(rfq_id, Uuid::from_u128(0xe), 1_000_000, auth(1))
        .await
        .unwrap();
    assert_eq!(firm_up.operation_id, auth(1).operation_id);
    let message = rfq_v2_firm_up_message(rfq_id, Uuid::from_u128(0xe), 1_000_000, auth(1));
    assert_eq!(signer_of(&message, &firm_up.signature), me);

    let decline = wallet
        .sign_rfq_v2_decline(rfq_id, me, auth(2))
        .await
        .unwrap();
    let message = rfq_v2_decline_message(rfq_id, me, auth(2));
    assert_eq!(signer_of(&message, &decline.signature), me);

    let execution = wallet
        .sign_rfq_v2_execution(rfq_id, Uuid::from_u128(0xf), 1_000_000, 0, auth(3))
        .await
        .unwrap();
    let allocation = RfqV2SweepAllocation {
        quote_id: execution.quote_id,
        package_units: execution.package_units,
    };
    let message = rfq_v2_execution_message(rfq_id, &[allocation], 0, auth(3)).unwrap();
    assert_eq!(signer_of(&message, &execution.signature), me);

    let cancel = wallet
        .sign_rfq_v2_invalidation(RfqV2InvalidationTarget::CancelRfq { rfq_id }, auth(4))
        .await
        .unwrap();
    let message =
        rfq_v2_invalidation_message(RfqV2InvalidationTarget::CancelRfq { rfq_id }, auth(4));
    assert_eq!(signer_of(&message, &cancel.signature), me);

    let schedule = wallet
        .sign_rfq_v2_quote_cancel_schedule(me, Some(1_900_000_090_000), auth(5))
        .await
        .unwrap();
    let message =
        rfq_v2_quote_cancel_schedule_message(me, Some(1_900_000_090_000), auth(5)).unwrap();
    assert_eq!(signer_of(&message, &schedule.signature), me);

    let bulk = wallet
        .sign_rfq_v2_bulk_withdrawal(me, RfqV2BulkWithdrawalScope::AllOpen, auth(6))
        .await
        .unwrap();
    let message =
        rfq_v2_bulk_withdrawal_message(me, &RfqV2BulkWithdrawalScope::AllOpen, auth(6)).unwrap();
    assert_eq!(signer_of(&message, &bulk.signature), me);

    let successor = wallet
        .sign_create_rfq_v2(
            RfqV2Terms {
                id: Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0002),
                ..terms()
            },
            42,
        )
        .await
        .unwrap();
    let successor_id = successor.terms.id;
    let replacement = wallet
        .sign_rfq_v2_replacement(rfq_id, successor, auth(7))
        .await
        .unwrap();
    let message = rfq_v2_replacement_message(
        RfqV2ReplacementTarget::Rfq { rfq_id },
        successor_id,
        auth(7),
    );
    assert_eq!(signer_of(&message, &replacement.signature), me);
}

#[test]
fn create_refuses_terms_the_server_would_sign_differently() {
    let mut unsorted = terms();
    unsorted.routing = RfqV2Routing::SelectedMakers {
        makers: vec![
            WalletAddress::from([0x22; 20]),
            WalletAddress::from([0x11; 20]),
        ],
    };
    assert_invalid(rfq_v2_create_message(&unsorted, 1), "sorted ascending");

    let mut manual_with_limit = terms();
    manual_with_limit.auto_execution_limit = Some(RfqV2AutoExecutionLimit {
        maker_receives_premium: true,
        premium_micros: 1,
    });
    assert_invalid(
        rfq_v2_create_message(&manual_with_limit, 1),
        "must not carry",
    );

    let mut auto_without_limit = terms();
    auto_without_limit.execution_mode = RfqV2ExecutionMode::Auto;
    assert_invalid(rfq_v2_create_message(&auto_without_limit, 1), "requires");
}

#[test]
fn full_size_once_signs_zero_minimum_and_step() {
    let mut full = terms();
    full.fill_policy = RfqV2FillPolicy::FullSizeOnce;
    let message = rfq_v2_create_message(&full, 1).unwrap();
    assert_eq!(
        (
            message.terms.fillPolicy,
            message.terms.minimumUnits,
            message.terms.stepUnits
        ),
        (0, 0, 0)
    );
}

#[test]
fn optional_terms_use_their_zero_encodings() {
    let message = rfq_v2_create_message(&terms(), 1).unwrap();
    assert_eq!(message.terms.builder, Address::ZERO);
    assert!(!message.terms.autoMakerReceivesPremium);
    assert_eq!(message.terms.autoPremiumLimitMicros, 0);

    let mut with_builder = terms();
    with_builder.builder = Some(RfqV2BuilderAttribution {
        recipient: WalletAddress::from([0x33; 20]),
    });
    with_builder.fee_limits = RfqV2FeeLimits {
        per_execution: 10,
        cumulative: 20,
    };
    let message = rfq_v2_create_message(&with_builder, 1).unwrap();
    assert_eq!(message.terms.builder, Address::from([0x33; 20]));

    let firm = rfq_v2_firm_quote_message(&firm_terms(), 1);
    assert_eq!(firm.terms.protectionScope, B256::ZERO);
    assert!(!firm.terms.hasSourceIndicativeQuote);
    assert_eq!(firm.terms.sourceIndicativeQuoteId, FixedBytes::<16>::ZERO);

    let sourced = rfq_v2_firm_quote_message(
        &RfqV2FirmQuoteTerms {
            source_indicative_quote_id: Some(Uuid::from_u128(0xe)),
            ..firm_terms()
        },
        1,
    );
    assert!(sourced.terms.hasSourceIndicativeQuote);
    assert_eq!(
        sourced.terms.sourceIndicativeQuoteId,
        uuid_bytes(Uuid::from_u128(0xe))
    );
}

#[test]
fn invalidation_signs_scope_and_reason() {
    let rfq_id = Uuid::from_u128(1);
    let quote_id = Uuid::from_u128(2);
    let cancel =
        rfq_v2_invalidation_message(RfqV2InvalidationTarget::CancelRfq { rfq_id }, auth(1));
    assert_eq!((cancel.scope, cancel.reason), (0, 0));
    assert_eq!(cancel.quoteId, FixedBytes::<16>::ZERO);

    let withdraw = rfq_v2_invalidation_message(
        RfqV2InvalidationTarget::WithdrawQuote { rfq_id, quote_id },
        auth(1),
    );
    assert_eq!((withdraw.scope, withdraw.reason), (1, 1));
    assert_eq!(withdraw.quoteId, uuid_bytes(quote_id));
    assert_ne!(
        rfq_v2_signing_hash(&cancel, CHAIN_ID),
        rfq_v2_signing_hash(&withdraw, CHAIN_ID)
    );
}

#[test]
fn replacement_names_the_old_and_new_object() {
    let rfq_id = Uuid::from_u128(1);
    let successor = Uuid::from_u128(9);
    let rfq =
        rfq_v2_replacement_message(RfqV2ReplacementTarget::Rfq { rfq_id }, successor, auth(1));
    assert_eq!(rfq.scope, 0);
    assert_eq!(rfq.oldObjectId, uuid_bytes(rfq_id));
    assert_eq!(rfq.newObjectId, uuid_bytes(successor));

    let quote_id = Uuid::from_u128(2);
    let quote = rfq_v2_replacement_message(
        RfqV2ReplacementTarget::Quote { rfq_id, quote_id },
        successor,
        auth(1),
    );
    assert_eq!(quote.scope, 1);
    assert_eq!(quote.rfqId, uuid_bytes(rfq_id));
    assert_eq!(quote.oldObjectId, uuid_bytes(quote_id));
}

#[test]
fn execution_refuses_what_the_server_refuses() {
    let rfq_id = Uuid::from_u128(1);
    assert_invalid(
        rfq_v2_execution_message(rfq_id, &[], 0, auth(1)),
        "at least one",
    );
    let repeated = [
        RfqV2SweepAllocation {
            quote_id: Uuid::from_u128(2),
            package_units: 1,
        },
        RfqV2SweepAllocation {
            quote_id: Uuid::from_u128(2),
            package_units: 2,
        },
    ];
    assert_invalid(
        rfq_v2_execution_message(rfq_id, &repeated, 0, auth(1)),
        "repeats",
    );
    let too_many: Vec<_> = (0..=MAX_RFQ_V2_EXECUTION_ALLOCATIONS as u128)
        .map(|n| RfqV2SweepAllocation {
            quote_id: Uuid::from_u128(n + 1),
            package_units: 1,
        })
        .collect();
    assert_invalid(
        rfq_v2_execution_message(rfq_id, &too_many, 0, auth(1)),
        "maximum is 1024",
    );
    assert!(rfq_v2_execution_message(
        rfq_id,
        &too_many[..MAX_RFQ_V2_EXECUTION_ALLOCATIONS],
        0,
        auth(1)
    )
    .is_ok());
}

#[test]
fn bulk_withdrawal_refuses_what_the_server_refuses() {
    let maker = account();
    let quote = |rfq: u128, quote: u128| RfqV2QuoteRef {
        rfq_id: Uuid::from_u128(rfq),
        quote_id: Uuid::from_u128(quote),
    };
    let scope = |quote_ids| RfqV2BulkWithdrawalScope::QuoteIds { quote_ids };
    assert_invalid(
        rfq_v2_bulk_withdrawal_message(maker, &scope(vec![]), auth(1)),
        "at least one",
    );
    assert_invalid(
        rfq_v2_bulk_withdrawal_message(maker, &scope(vec![quote(1, 2), quote(1, 2)]), auth(1)),
        "nil or repeated",
    );
    assert_invalid(
        rfq_v2_bulk_withdrawal_message(maker, &scope(vec![quote(0, 2)]), auth(1)),
        "nil or repeated",
    );
    let too_many = (0..=MAX_RFQ_V2_BULK_WITHDRAWAL_QUOTES as u128)
        .map(|n| quote(1, n + 1))
        .collect();
    assert_invalid(
        rfq_v2_bulk_withdrawal_message(maker, &scope(too_many), auth(1)),
        "maximum is 64",
    );
    assert_invalid(
        rfq_v2_bulk_withdrawal_message(
            maker,
            &RfqV2BulkWithdrawalScope::Rfq {
                rfq_id: Uuid::nil(),
            },
            auth(1),
        ),
        "nil RFQ",
    );

    let exact = rfq_v2_bulk_withdrawal_message(maker, &scope(vec![quote(1, 2)]), auth(1)).unwrap();
    assert_eq!((exact.scope, exact.rfqId), (0, FixedBytes::ZERO));
    let rfq = rfq_v2_bulk_withdrawal_message(
        maker,
        &RfqV2BulkWithdrawalScope::Rfq {
            rfq_id: Uuid::from_u128(1),
        },
        auth(1),
    )
    .unwrap();
    assert_eq!(rfq.scope, 1);
    assert!(rfq.targets.is_empty());
    let all =
        rfq_v2_bulk_withdrawal_message(maker, &RfqV2BulkWithdrawalScope::AllOpen, auth(1)).unwrap();
    assert_eq!((all.scope, all.rfqId), (2, FixedBytes::ZERO));
}

#[test]
fn cancel_schedule_reserves_zero_for_removal() {
    assert_invalid(
        rfq_v2_quote_cancel_schedule_message(account(), Some(0), auth(1)),
        "nonzero",
    );
    let removal = rfq_v2_quote_cancel_schedule_message(account(), None, auth(1)).unwrap();
    assert_eq!(removal.cancelAtMs, 0);
}

proptest! {
    #[test]
    fn sweep_order_and_every_allocation_field_are_signed(
        units in proptest::collection::vec(1u128..=u128::from(u64::MAX), 2..16),
    ) {
        let rfq_id = Uuid::from_u128(1);
        let allocations: Vec<_> = units
            .iter()
            .enumerate()
            .map(|(n, units)| RfqV2SweepAllocation {
                quote_id: Uuid::from_u128(n as u128 + 1),
                package_units: *units,
            })
            .collect();
        let hash = |allocations: &[RfqV2SweepAllocation]| {
            rfq_v2_signing_hash(
                &rfq_v2_execution_message(rfq_id, allocations, 0, auth(1)).unwrap(),
                CHAIN_ID,
            )
        };
        let original = hash(&allocations);

        let mut rotated = allocations.clone();
        rotated.rotate_left(1);
        prop_assert_ne!(original, hash(&rotated));

        let mut resized = allocations.clone();
        resized[0].package_units += 1;
        prop_assert_ne!(original, hash(&resized));

        prop_assert_ne!(original, hash(&allocations[1..]));
    }

    #[test]
    fn terms_commitment_binds_target_deadlines_and_blinding(
        target in 1u128..=u128::from(u64::MAX),
        deadline in 1u64..1_900_000_059_999,
        blinding in any::<[u8; 32]>(),
    ) {
        let base = terms();
        let commitment = rfq_v2_terms_commitment(&base).unwrap();
        let changed = [
            RfqV2Terms { target: base.target ^ target, ..terms() },
            RfqV2Terms { quote_deadline_ms: base.quote_deadline_ms ^ deadline, ..terms() },
            RfqV2Terms {
                private_terms_blinding: RfqV2Digest(B256::from(blinding)),
                ..terms()
            },
        ];
        for (index, changed) in changed.iter().enumerate() {
            let same = match index {
                0 => changed.target == base.target,
                1 => changed.quote_deadline_ms == base.quote_deadline_ms,
                _ => changed.private_terms_blinding == base.private_terms_blinding,
            };
            prop_assert_eq!(rfq_v2_terms_commitment(changed).unwrap() == commitment, same);
        }
    }
}

#[test]
fn reference_premium_nets_buy_legs_against_sell_legs() {
    let parent = terms();
    let firm = firm_terms();
    // Taker buys the call at 2.00 and sells the put at 0.50: maker receives 1.50.
    assert_eq!(
        rfq_v2_reference_premium(&parent.legs, &firm.priced_legs, 1_000_000, Some(1_000_000))
            .unwrap(),
        (true, 1_500_000)
    );
    let mut credit = firm.priced_legs.clone();
    credit[0].price_micros = 100_000;
    assert_eq!(
        rfq_v2_reference_premium(&parent.legs, &credit, 2_000_000, None).unwrap(),
        (false, 800_000)
    );
    let mut flat = firm.priced_legs.clone();
    flat[0].price_micros = 500_000;
    assert_eq!(
        rfq_v2_reference_premium(&parent.legs, &flat, 1_000_000, None).unwrap(),
        (false, 0)
    );
}

#[test]
fn reference_premium_refuses_what_the_server_refuses() {
    let parent = terms();
    let firm = firm_terms();
    assert_invalid(
        rfq_v2_reference_premium(&parent.legs, &firm.priced_legs[..1], 1_000_000, None),
        "prices 1 legs",
    );
    let mut swapped = firm.priced_legs.clone();
    swapped.swap(0, 1);
    assert_invalid(
        rfq_v2_reference_premium(&parent.legs, &swapped, 1_000_000, None),
        "where the RFQ has",
    );
    let mut fractional = firm.priced_legs.clone();
    fractional[0].price_micros = 2_000_001;
    assert_invalid(
        rfq_v2_reference_premium(&parent.legs, &fractional, 1, None),
        "whole number",
    );
    assert_invalid(
        rfq_v2_reference_premium(&parent.legs, &firm.priced_legs, 0, None),
        "nonzero",
    );
}
