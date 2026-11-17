use super::*;
use rust_decimal_macros::dec;
use std::str::FromStr;

/// The Hypercall engine's own test vector for version 2 terms (`auction-1`, wallet `0x..01`, `portfolio-hash`,
/// valuation 1_700_000_000_000). The bid is not an input.
const ENGINE_V2_VECTOR: &str =
    "sha256:18433ebaed4fe3171f4273e713fe607d0406f227fe497d2ce00538da83472363";
const VALUATION_MS: u64 = 1_700_000_000_000;

fn wallet() -> WalletAddress {
    WalletAddress::from_str("0x0000000000000000000000000000000000000001").unwrap()
}

fn hash(version: u64, bid: Decimal, valuation_ms: u64) -> Result<String, AuctionTermsError> {
    auction_terms_hash(
        "auction-1",
        &wallet(),
        version,
        "portfolio-hash",
        bid,
        valuation_ms,
    )
}

/// The terms hash exactly as published hypercall-liquidator 0.4.0 computed
/// it: the bid is hashed for every version.
fn hash_0_4_0(version: u64, bid: Decimal, valuation_ms: u64) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hash_str(&mut hasher, AUCTION_TERMS_DOMAIN);
    hash_str(&mut hasher, "auction-1");
    hash_str(&mut hasher, &wallet().to_string());
    hasher.update(version.to_le_bytes());
    hash_str(&mut hasher, "portfolio-hash");
    hash_str(&mut hasher, &bid.normalize().to_string());
    hasher.update(valuation_ms.to_le_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Mirror of the engine's `StandardAuctionSchedule::min_bid_usdc`: positive
/// equity at the start, falling linearly to `floor_bps` of it over
/// `decay_ms`, then flat.
fn engine_minimum(equity: Decimal, floor_bps: u32, decay_ms: u64, elapsed_ms: u64) -> Decimal {
    let positive_equity = equity.max(Decimal::ZERO);
    let floor = Decimal::from(floor_bps.min(10_000)) / Decimal::from(10_000);
    let progress = if elapsed_ms >= decay_ms {
        Decimal::ONE
    } else {
        Decimal::from(elapsed_ms) / Decimal::from(decay_ms)
    };
    (positive_equity * (Decimal::ONE - (Decimal::ONE - floor) * progress))
        .clamp(Decimal::ZERO, positive_equity)
}

#[test]
fn published_0_4_0_hash_fails_version_2_terms() {
    // Root cause: 0.4.0 hashes the bid into version 2 terms, so it never
    // reproduces the engine's version 2 hash and settlement rejects the bid.
    for bid in [dec!(75), dec!(750), dec!(1000)] {
        assert_ne!(hash_0_4_0(2, bid, VALUATION_MS), ENGINE_V2_VECTOR);
    }
    // The verifier names the failure before a bid is ever signed.
    let stale = hash_0_4_0(2, dec!(75), VALUATION_MS);
    assert!(matches!(
        verify_published_terms_hash(2, &stale, Some(ENGINE_V2_VECTOR)),
        Err(AuctionTermsError::TermsHashMismatch {
            auction_version: 2,
            ..
        })
    ));
}

#[test]
fn current_hash_matches_engine_version_2_vector_for_any_bid() {
    for bid in [dec!(0), dec!(75), dec!(750), dec!(1100.00)] {
        let computed = hash(2, bid, VALUATION_MS).unwrap();
        assert_eq!(computed, ENGINE_V2_VECTOR);
        verify_published_terms_hash(2, &computed, Some(ENGINE_V2_VECTOR)).unwrap();
    }
}

#[test]
fn version_1_hash_is_unchanged_from_0_4_0_and_binds_the_bid() {
    for bid in [dec!(1000), dec!(1100.00), dec!(0)] {
        assert_eq!(
            hash(1, bid, VALUATION_MS).unwrap(),
            hash_0_4_0(1, bid, VALUATION_MS)
        );
    }
    assert_ne!(
        hash(1, dec!(1000), VALUATION_MS).unwrap(),
        hash(1, dec!(1001), VALUATION_MS).unwrap()
    );
}

#[test]
fn unknown_versions_are_refused() {
    for version in [0, 3, u64::MAX] {
        assert_eq!(
            AuctionBidRule::for_version(version),
            Err(AuctionTermsError::UnsupportedVersion { version })
        );
        assert!(matches!(
            hash(version, dec!(1), VALUATION_MS),
            Err(AuctionTermsError::UnsupportedVersion { .. })
        ));
    }
}

#[test]
fn version_1_requires_the_exact_bid() {
    let rule = AuctionBidRule::for_version(1).unwrap();
    rule.check_bid(dec!(1000), dec!(1000)).unwrap();
    rule.check_bid(dec!(1000), dec!(1000.000)).unwrap();
    assert!(matches!(
        rule.check_bid(dec!(1000), dec!(1000.01)),
        Err(AuctionTermsError::BidNotExact { .. })
    ));
    assert!(matches!(
        rule.check_bid(dec!(1000), dec!(999.99)),
        Err(AuctionTermsError::BidNotExact { .. })
    ));
}

#[test]
fn version_2_admits_the_minimum_and_above_but_never_below() {
    let rule = AuctionBidRule::for_version(2).unwrap();
    rule.check_bid(dec!(750), dec!(750)).unwrap();
    rule.check_bid(dec!(750), dec!(900)).unwrap();
    assert_eq!(
        rule.check_bid(dec!(750), dec!(749.999999)),
        Err(AuctionTermsError::BidBelowMinimum {
            bid_usdc: dec!(749.999999),
            minimum_bid_usdc: dec!(750),
        })
    );
    assert!(matches!(
        rule.check_bid(dec!(0), dec!(-1)),
        Err(AuctionTermsError::NegativeBid { .. })
    ));
}

#[test]
fn version_2_floor_decays_and_each_bid_respects_its_own_valuation() {
    // Default schedule: 7500 bps floor over 30 minutes, equity 1000.
    let (equity, floor_bps, decay_ms) = (dec!(1000), 7_500, 1_800_000);
    let started_at = VALUATION_MS;
    let rule = AuctionBidRule::for_version(2).unwrap();

    let snapshots: Vec<(u64, Decimal)> = [0, 450_000, 900_000, 1_800_000, 3_600_000]
        .into_iter()
        .map(|elapsed| {
            (
                started_at + elapsed,
                engine_minimum(equity, floor_bps, decay_ms, elapsed),
            )
        })
        .collect();
    assert_eq!(
        snapshots.iter().map(|(_, min)| *min).collect::<Vec<_>>(),
        vec![dec!(1000), dec!(937.5), dec!(875), dec!(750), dec!(750)]
    );

    for window in snapshots.windows(2) {
        let ((earlier_ms, earlier_min), (later_ms, later_min)) = (window[0], window[1]);
        assert!(later_min <= earlier_min, "the floor never rises");
        // The bidder bids the published minimum for each fresh read.
        rule.check_bid(later_min, later_min).unwrap();
        // A bid priced from a later read is too low for an earlier
        // valuation while the floor is still falling.
        if later_min < earlier_min {
            assert!(rule.check_bid(earlier_min, later_min).is_err());
        }
        // The terms hash binds the valuation timestamp, so a bid cannot pair
        // a later, lower minimum with an earlier timestamp's terms.
        assert_ne!(
            hash(2, later_min, earlier_ms).unwrap(),
            hash(2, later_min, later_ms).unwrap()
        );
    }

    // Underwater accounts publish a zero minimum.
    assert_eq!(engine_minimum(dec!(-5), floor_bps, decay_ms, 0), dec!(0));
    rule.check_bid(dec!(0), dec!(0)).unwrap();
}
