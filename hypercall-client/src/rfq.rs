//! High-level RFQ quote builder for Quote Providers.
#![expect(
    clippy::result_large_err,
    reason = "the RFQ builder preserves the established unboxed public ClientError contract"
)]
//!
//! Constructs valid, signed `RfqResponse` messages from an inbound
//! `RfqRequest` and per-leg prices. The builder enforces the public RFQ
//! response constraints so invalid quotes are caught client-side instead of
//! being rejected later.
//!
//! # Example
//!
//! ```rust,no_run
//! use hypercall_client::{HypercallWallet, rfq};
//! use hypercall_client::qp_client::ServerInbound;
//! use rust_decimal_macros::dec;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let wallet = HypercallWallet::from_private_key("0x...", 999)?;
//!
//! // When you receive an RfqRequest from the server:
//! // let msg: ServerInbound = ...;
//! // if let ServerInbound::RfqRequest { rfq_id, legs, .. } = msg {
//! //     let quote = rfq::build_rfq_quote(&rfq_id, &legs, &[dec!(0.05)], 3000, &wallet).await?;
//! //     outbound_tx.send(quote).await?;
//! // }
//! # Ok(())
//! # }
//! ```

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;

use crate::error::{ClientError, Result, RfqInputError};
use crate::wallet::HypercallWallet;
use hypercall_ws_protocol::{QpClientMessage, QpResponseLeg, QpRfqLeg};

/// Build a signed RFQ quote response from an inbound RFQ request.
///
/// Only per-leg prices come from the quote provider. Instrument, side, and
/// size are echoed from the original request, preventing the class of bugs
/// where a QP constructs a semantically invalid response that is rejected
/// later.
///
/// # Net premium sign convention
///
/// - Taker **buy** legs contribute `-price * size` (debit to taker)
/// - Taker **sell** legs contribute `+price * size` (credit to taker)
///
/// The net premium is the sum across all legs.
pub async fn build_rfq_quote(
    rfq_id: &str,
    legs: &[QpRfqLeg],
    leg_prices: &[Decimal],
    valid_for_ms: u64,
    wallet: &HypercallWallet,
) -> Result<QpClientMessage> {
    if legs.is_empty() {
        return Err(ClientError::RfqInput(RfqInputError::NoLegs));
    }
    if legs.len() != leg_prices.len() {
        return Err(ClientError::RfqInput(RfqInputError::LegCountMismatch {
            request_count: legs.len(),
            price_count: leg_prices.len(),
        }));
    }

    let mut response_legs = Vec::with_capacity(legs.len());
    let mut net_premium = Decimal::ZERO;

    for (i, (leg, price)) in legs.iter().zip(leg_prices.iter()).enumerate() {
        if *price <= Decimal::ZERO {
            return Err(ClientError::RfqInput(RfqInputError::LegPriceNonPositive {
                index: i,
                price: *price,
            }));
        }

        let size: Decimal = leg.size.parse().map_err(|source| {
            ClientError::RfqInput(RfqInputError::InvalidLegSize {
                label: "invalid",
                value: leg.size.clone(),
                source,
            })
        })?;
        if size <= Decimal::ZERO {
            return Err(ClientError::RfqInput(RfqInputError::LegSizeNonPositive {
                index: i,
                size,
            }));
        }

        match leg.side.to_lowercase().as_str() {
            "buy" => net_premium -= *price * size,
            "sell" => net_premium += *price * size,
            other => {
                return Err(ClientError::RfqInput(RfqInputError::InvalidSide {
                    index: i,
                    side: other.to_owned(),
                }));
            }
        }

        response_legs.push(QpResponseLeg {
            instrument: leg.instrument.clone(),
            side: leg.side.clone(),
            price: price.to_string(),
            size: leg.size.clone(),
        });
    }

    let legs_hash = compute_qp_legs_hash(legs)?;

    let premium_scaled = net_premium * Decimal::new(1_000_000, 0);
    let premium_micro = premium_scaled.to_i64().ok_or(ClientError::RfqInput(
        RfqInputError::NetPremiumNotRepresentable { net_premium },
    ))?;
    let net_premium_i256 = alloy::primitives::I256::try_from(premium_micro).map_err(|source| {
        ClientError::RfqInput(RfqInputError::NetPremiumConversion {
            source: Box::new(source),
        })
    })?;
    let valid_for_u256 = alloy::primitives::U256::from(valid_for_ms);

    let rfq_uuid = uuid::Uuid::parse_str(rfq_id).map_err(|source| {
        ClientError::RfqInput(RfqInputError::InvalidUuid {
            label: "invalid rfq_id",
            source,
        })
    })?;
    let mut rfq_id_bytes = [0u8; 32];
    rfq_id_bytes[16..].copy_from_slice(rfq_uuid.as_bytes());

    let nonce = wallet.next_nonce();
    let signature = wallet
        .sign_submit_rfq_response(
            rfq_id_bytes,
            legs_hash,
            net_premium_i256,
            valid_for_u256,
            nonce,
        )
        .await?;

    Ok(QpClientMessage::RfqResponse {
        rfq_id: rfq_id.to_string(),
        action: "quote".to_string(),
        legs: Some(response_legs),
        net_premium: Some(net_premium.to_string()),
        valid_for_ms: Some(valid_for_ms),
        signature: Some(signature),
        nonce: Some(nonce),
    })
}

/// Build a decline response for an RFQ.
pub fn build_rfq_decline(rfq_id: &str) -> QpClientMessage {
    QpClientMessage::RfqResponse {
        rfq_id: rfq_id.to_string(),
        action: "decline".to_string(),
        legs: None,
        net_premium: None,
        valid_for_ms: None,
        signature: None,
        nonce: None,
    }
}

fn compute_qp_legs_hash(legs: &[QpRfqLeg]) -> Result<[u8; 32]> {
    use sha3::Digest;
    let canonical = legs
        .iter()
        .map(|l| {
            let size_canonical = l
                .size
                .parse::<Decimal>()
                .map_err(|source| {
                    ClientError::RfqInput(RfqInputError::InvalidLegSize {
                        label: "invalid",
                        value: l.size.clone(),
                        source,
                    })
                })?
                .to_string();
            Ok(format!(
                "{}|{}|{}",
                l.instrument,
                l.side.to_lowercase(),
                size_canonical
            ))
        })
        .collect::<Result<Vec<_>>>()?
        .join(";");
    let mut hasher = sha3::Keccak256::new();
    hasher.update(canonical.as_bytes());
    Ok(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HypercallWallet;
    use rust_decimal_macros::dec;

    const TEST_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    fn make_legs(specs: &[(&str, &str, &str)]) -> Vec<QpRfqLeg> {
        specs
            .iter()
            .map(|(inst, side, size)| QpRfqLeg {
                instrument: inst.to_string(),
                side: side.to_string(),
                size: size.to_string(),
            })
            .collect()
    }

    #[tokio::test]
    async fn single_leg_buy_produces_negative_premium() {
        let wallet = HypercallWallet::from_private_key(TEST_KEY, 998).unwrap();
        let legs = make_legs(&[("BTC-20260501-90000-C", "buy", "1")]);
        let msg = build_rfq_quote(
            "019060da-0000-7000-8000-000000000001",
            &legs,
            &[dec!(500)],
            3000,
            &wallet,
        )
        .await
        .unwrap();

        if let QpClientMessage::RfqResponse {
            net_premium,
            legs: resp_legs,
            signature,
            nonce,
            action,
            ..
        } = msg
        {
            assert_eq!(action, "quote");
            let premium: Decimal = net_premium.unwrap().parse().unwrap();
            assert!(
                premium < Decimal::ZERO,
                "taker-buy should be negative premium"
            );
            assert_eq!(premium, dec!(-500));
            assert!(signature.unwrap().starts_with("0x"));
            assert!(nonce.is_some());
            let resp = resp_legs.unwrap();
            assert_eq!(resp.len(), 1);
            assert_eq!(resp[0].instrument, "BTC-20260501-90000-C");
            assert_eq!(resp[0].side, "buy");
            assert_eq!(resp[0].size, "1");
        } else {
            panic!("expected RfqResponse");
        }
    }

    #[tokio::test]
    async fn single_leg_sell_produces_positive_premium() {
        let wallet = HypercallWallet::from_private_key(TEST_KEY, 998).unwrap();
        let legs = make_legs(&[("BTC-20260501-90000-C", "sell", "2")]);
        let msg = build_rfq_quote(
            "019060da-0000-7000-8000-000000000002",
            &legs,
            &[dec!(300)],
            3000,
            &wallet,
        )
        .await
        .unwrap();

        if let QpClientMessage::RfqResponse { net_premium, .. } = msg {
            let premium: Decimal = net_premium.unwrap().parse().unwrap();
            assert!(
                premium > Decimal::ZERO,
                "taker-sell should be positive premium"
            );
            assert_eq!(premium, dec!(600));
        } else {
            panic!("expected RfqResponse");
        }
    }

    #[tokio::test]
    async fn multi_leg_net_premium() {
        let wallet = HypercallWallet::from_private_key(TEST_KEY, 998).unwrap();
        let legs = make_legs(&[
            ("BTC-20260501-90000-C", "buy", "1"),
            ("BTC-20260501-100000-C", "sell", "1"),
        ]);
        let msg = build_rfq_quote(
            "019060da-0000-7000-8000-000000000003",
            &legs,
            &[dec!(500), dec!(200)],
            3000,
            &wallet,
        )
        .await
        .unwrap();

        if let QpClientMessage::RfqResponse { net_premium, .. } = msg {
            let premium: Decimal = net_premium.unwrap().parse().unwrap();
            // buy leg: -500, sell leg: +200 = -300
            assert_eq!(premium, dec!(-300));
        } else {
            panic!("expected RfqResponse");
        }
    }

    #[tokio::test]
    async fn rejects_zero_price() {
        let wallet = HypercallWallet::from_private_key(TEST_KEY, 998).unwrap();
        let legs = make_legs(&[("BTC-20260501-90000-C", "buy", "1")]);
        let err = build_rfq_quote(
            "019060da-0000-7000-8000-000000000004",
            &legs,
            &[dec!(0)],
            3000,
            &wallet,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            &err,
            ClientError::RfqInput(RfqInputError::LegPriceNonPositive { index: 0, price })
                if *price == dec!(0)
        ));
        assert_eq!(
            err.to_string(),
            "API error (0): leg 0 price must be positive, got 0"
        );
    }

    #[tokio::test]
    async fn rejects_negative_price() {
        let wallet = HypercallWallet::from_private_key(TEST_KEY, 998).unwrap();
        let legs = make_legs(&[("BTC-20260501-90000-C", "buy", "1")]);
        let err = build_rfq_quote(
            "019060da-0000-7000-8000-000000000005",
            &legs,
            &[dec!(-10)],
            3000,
            &wallet,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            &err,
            ClientError::RfqInput(RfqInputError::LegPriceNonPositive { index: 0, price })
                if *price == dec!(-10)
        ));
        assert_eq!(
            err.to_string(),
            "API error (0): leg 0 price must be positive, got -10"
        );
    }

    #[tokio::test]
    async fn rejects_leg_count_mismatch() {
        let wallet = HypercallWallet::from_private_key(TEST_KEY, 998).unwrap();
        let legs = make_legs(&[
            ("BTC-20260501-90000-C", "buy", "1"),
            ("BTC-20260501-100000-C", "sell", "1"),
        ]);
        let err = build_rfq_quote(
            "019060da-0000-7000-8000-000000000006",
            &legs,
            &[dec!(500)],
            3000,
            &wallet,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            &err,
            ClientError::RfqInput(RfqInputError::LegCountMismatch {
                request_count: 2,
                price_count: 1,
            })
        ));
        assert_eq!(
            err.to_string(),
            "API error (0): leg count mismatch: request has 2 legs but 1 prices provided"
        );
    }

    #[tokio::test]
    async fn rejects_invalid_side() {
        let wallet = HypercallWallet::from_private_key(TEST_KEY, 998).unwrap();
        let legs = make_legs(&[("BTC-20260501-90000-C", "long", "1")]);
        let err = build_rfq_quote(
            "019060da-0000-7000-8000-000000000007",
            &legs,
            &[dec!(500)],
            3000,
            &wallet,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            &err,
            ClientError::RfqInput(RfqInputError::InvalidSide { index: 0, side })
                if side == "long"
        ));
        assert_eq!(
            err.to_string(),
            "API error (0): leg 0 has invalid side: \"long\""
        );
    }

    #[tokio::test]
    async fn decline_has_no_signature() {
        let msg = build_rfq_decline("019060da-0000-7000-8000-000000000008");
        if let QpClientMessage::RfqResponse {
            action,
            signature,
            legs,
            ..
        } = msg
        {
            assert_eq!(action, "decline");
            assert!(signature.is_none());
            assert!(legs.is_none());
        } else {
            panic!("expected RfqResponse");
        }
    }

    #[tokio::test]
    async fn legs_hash_matches_server_canonicalization() {
        let legs = make_legs(&[("BTC-20260501-90000-C", "buy", "1.50")]);
        let hash = compute_qp_legs_hash(&legs).unwrap();

        use sha3::Digest;
        let size_canonical = "1.50".parse::<Decimal>().unwrap().to_string();
        let canonical = format!("BTC-20260501-90000-C|buy|{}", size_canonical);
        let mut hasher = sha3::Keccak256::new();
        hasher.update(canonical.as_bytes());
        let expected: [u8; 32] = hasher.finalize().into();

        assert_eq!(hash, expected);

        // Different representation of same value produces same hash
        let legs2 = make_legs(&[("BTC-20260501-90000-C", "buy", "01.50")]);
        let hash2 = compute_qp_legs_hash(&legs2).unwrap();
        assert_eq!(hash, hash2, "01.50 and 1.50 should produce same hash");
    }

    #[tokio::test]
    async fn signature_includes_signed_quote_payload() {
        let wallet = HypercallWallet::from_private_key(TEST_KEY, 998).unwrap();
        let legs = make_legs(&[("BTC-20260501-90000-C", "buy", "1")]);
        let msg = build_rfq_quote(
            "019060da-0000-7000-8000-000000000009",
            &legs,
            &[dec!(500)],
            3000,
            &wallet,
        )
        .await
        .unwrap();

        if let QpClientMessage::RfqResponse {
            net_premium,
            valid_for_ms,
            signature,
            nonce,
            ..
        } = msg
        {
            let premium: Decimal = net_premium.unwrap().parse().unwrap();
            assert_eq!(premium, dec!(-500));
            let valid_for_ms = valid_for_ms.unwrap();
            assert_eq!(valid_for_ms, 3000);
            let nonce = nonce.unwrap();
            let signature = signature.unwrap();
            assert!(signature.starts_with("0x"));

            let rfq_uuid = uuid::Uuid::parse_str("019060da-0000-7000-8000-000000000009").unwrap();
            let mut rfq_id_bytes = [0u8; 32];
            rfq_id_bytes[16..].copy_from_slice(rfq_uuid.as_bytes());
            let legs_hash = compute_qp_legs_hash(&legs).unwrap();
            let expected_signature = wallet
                .sign_submit_rfq_response(
                    rfq_id_bytes,
                    legs_hash,
                    alloy::primitives::I256::try_from(-500_000_000i64).unwrap(),
                    alloy::primitives::U256::from(valid_for_ms),
                    nonce,
                )
                .await
                .unwrap();
            assert_eq!(signature, expected_signature);
        } else {
            panic!("expected RfqResponse");
        }
    }
}
