//! Standard-margin full-liquidation auction terms, keyed by the published
//! `auction_version`.
//!
//! Version 1 auctions take an exact bid, and the bid is part of the terms
//! hash. Version 2 auctions publish a descending minimum bid: the minimum
//! starts at the account's positive equity and falls over the auction, any
//! bid at or above the minimum settles, and the terms hash leaves the bid out.
//!
//! The server prices `current_required_bid_usdc` at
//! `current_valuation_timestamp_ms`, and settlement prices the minimum at the
//! bid's signed valuation timestamp. Bidding from a fresh status read is how
//! a bidder follows the floor down. A bid signed with an old valuation
//! timestamp is priced against the higher, older minimum.

use rust_decimal::Decimal;
use thiserror::Error;

use hypercall_sdk_types::WalletAddress;

/// Auction versions this crate can price and hash.
pub const SUPPORTED_AUCTION_VERSIONS: [u64; 2] = [1, 2];

const AUCTION_TERMS_DOMAIN: &str = "standard-margin-liquidation-auction-terms-v5";

/// How an auction version admits a bid against the published bid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuctionBidRule {
    /// Version 1: the bid must equal the published required bid.
    Exact,
    /// Version 2: the bid must be at or above the published minimum.
    AtLeastMinimum,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[displaystr::display]
pub enum AuctionTermsError {
    UnsupportedVersion {
        version: u64,
    } = "unsupported standard-margin auction_version {version}; this crate handles versions 1 and 2, upgrade hypercall-liquidator",
    BidNotExact {
        bid_usdc: Decimal,
        required_bid_usdc: Decimal,
    } = "version 1 auction requires an exact bid of {required_bid_usdc}, got {bid_usdc}",
    BidBelowMinimum {
        bid_usdc: Decimal,
        minimum_bid_usdc: Decimal,
    } = "version 2 auction bid {bid_usdc} is below the published minimum {minimum_bid_usdc}",
    NegativeBid {
        bid_usdc: Decimal,
    } = "auction bid {bid_usdc} is negative",
    TermsHashMismatch {
        auction_version: u64,
        computed: String,
        published: String,
    } = "locally computed version {auction_version} auction terms hash {computed} does not match the published {published}; upgrade hypercall-liquidator",
}

impl AuctionBidRule {
    /// The bid rule for a published `auction_version`. Unknown versions are
    /// refused rather than hashed as a known one.
    pub fn for_version(auction_version: u64) -> Result<Self, AuctionTermsError> {
        match auction_version {
            1 => Ok(Self::Exact),
            2 => Ok(Self::AtLeastMinimum),
            version => Err(AuctionTermsError::UnsupportedVersion { version }),
        }
    }

    /// Check a bid against the published bid for the same valuation
    /// timestamp. Never admits a version 2 bid below the published minimum.
    pub fn check_bid(
        self,
        published_bid_usdc: Decimal,
        bid_usdc: Decimal,
    ) -> Result<(), AuctionTermsError> {
        if bid_usdc < Decimal::ZERO {
            return Err(AuctionTermsError::NegativeBid { bid_usdc });
        }
        match self {
            Self::Exact if bid_usdc != published_bid_usdc => Err(AuctionTermsError::BidNotExact {
                bid_usdc,
                required_bid_usdc: published_bid_usdc,
            }),
            Self::AtLeastMinimum if bid_usdc < published_bid_usdc => {
                Err(AuctionTermsError::BidBelowMinimum {
                    bid_usdc,
                    minimum_bid_usdc: published_bid_usdc,
                })
            }
            Self::Exact | Self::AtLeastMinimum => Ok(()),
        }
    }
}

/// The auction terms hash settlement expects for these terms. Version 1
/// includes the bid; version 2 leaves it out so a bid may exceed the
/// published minimum.
pub fn auction_terms_hash(
    auction_id: &str,
    liquidated_wallet: &WalletAddress,
    auction_version: u64,
    portfolio_hash: &str,
    bid_usdc: Decimal,
    valuation_timestamp_ms: u64,
) -> Result<String, AuctionTermsError> {
    use sha2::{Digest, Sha256};

    let rule = AuctionBidRule::for_version(auction_version)?;
    let mut hasher = Sha256::new();
    hash_str(&mut hasher, AUCTION_TERMS_DOMAIN);
    hash_str(&mut hasher, auction_id);
    hash_str(&mut hasher, &liquidated_wallet.to_string());
    hasher.update(auction_version.to_le_bytes());
    hash_str(&mut hasher, portfolio_hash);
    if rule == AuctionBidRule::Exact {
        hash_str(&mut hasher, &bid_usdc.normalize().to_string());
    }
    hasher.update(valuation_timestamp_ms.to_le_bytes());
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Compare a locally computed terms hash with the one the server published,
/// when it published one. A mismatch means this crate and the server
/// disagree on the terms encoding, so the bid would fail settlement.
pub fn verify_published_terms_hash(
    auction_version: u64,
    computed: &str,
    published: Option<&str>,
) -> Result<(), AuctionTermsError> {
    match published {
        Some(published) if published != computed => Err(AuctionTermsError::TermsHashMismatch {
            auction_version,
            computed: computed.to_string(),
            published: published.to_string(),
        }),
        _ => Ok(()),
    }
}

fn hash_str(hasher: &mut sha2::Sha256, value: &str) {
    use sha2::Digest;

    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

#[cfg(test)]
#[path = "auction_test.rs"]
mod tests;
