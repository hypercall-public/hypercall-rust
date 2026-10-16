#![expect(
    clippy::result_large_err,
    reason = "cycle execution preserves the public unboxed liquidation error contract"
)]

use std::{error::Error as StdError, str::FromStr};

use futures::stream::{self, StreamExt, TryStreamExt};
use hypercall_client::{
    AccountAddress, HypercallClient, PublicLiquidationsQuery, StandardMarginLiquidationParams,
};
use hypercall_hyperliquid::PerpVenue;
use hypercall_sdk_types::{StandardMarginLiquidationOrderResponse, WalletAddress};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    config::HypercallLiquidatorConfig,
    hedge::{
        plan_delta_hedge, submit_hedge_plan, DeltaHedgeInput, HedgePlan, HedgePlanDecision,
        HedgeSkipReason,
    },
    keys::{hypercall_wallet_from_key_config, KeyError},
    planner::{evaluate_candidate, CandidateDecision, CandidateEvaluationError},
    reader::ReaderError,
    types::{
        AuctionStatus, LiquidationCandidate, MarginSnapshot, PositionDelta,
        StandardMarginLiquidationBidTerms,
    },
};

/// Dynamically typed source retained by a liquidation-cycle error.
pub type BoxCycleSource = Box<dyn StdError + Send + Sync + 'static>;

#[derive(Debug, Error)]
#[displaystr::display]
pub enum CycleError {
    InvalidAccount {
        account: String,
        #[source]
        detail: BoxCycleSource,
    } = "invalid account address {account}: {detail}",
    PortfolioRead(#[from] hypercall_client::ClientError) = "portfolio read failed: {_0}",
    Reader(#[from] ReaderError) = "portfolio margin read failed: {_0}",
    Candidate(#[from] CandidateEvaluationError) = "candidate evaluation failed: {_0}",
    Key(#[from] KeyError) = "key resolution failed: {_0}",
    AuctionInactiveBeforeSubmit {
        account: String,
    } = "auction is no longer active before liquidation submit for {account}",
    FreshAuctionNotEligible(CandidateDecision) =
        "fresh auction state is no longer eligible before liquidation submit: {_0:?}",
    LiquidationStatusNotFound {
        account: String,
    } = "liquidation status not found for {account}",
    LiquidationStatusNotFullStandardMargin {
        account: String,
    } = "liquidation status for {account} is not active full standard-margin liquidation",
    MissingCurrentBidTerms {
        account: String,
        field: &'static str,
    } = "liquidation status for {account} is missing current engine-priced bid terms: {field}",
    SkippedCycle = "cannot execute liquidation for skipped cycle",
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "boxing a serialized public liquidation decision would change its wire contract"
)]
pub enum CycleDecision {
    Eligible {
        candidate: LiquidationCandidate,
        auction: AuctionStatus,
        planner: CandidateDecision,
    },
    Skip(CycleSkipReason),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CycleSkipReason {
    NoActiveAuction { account: String },
    MissingStandardMarginBidTerms { account: String, field: String },
    Planner(CandidateDecision),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LiquidationExecution {
    DryRun {
        candidate: LiquidationCandidate,
        auction: AuctionStatus,
    },
    SubmittedStandardMargin {
        receipt: StandardMarginLiquidationReceipt,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StandardMarginLiquidationReceipt {
    pub account: String,
    pub liquidator_wallet: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub bid_usdc: Decimal,
    pub request_id: String,
    pub auction_id: String,
    pub response: StandardMarginLiquidationOrderResponse,
}

#[derive(Debug, Clone)]
pub enum HedgeExecution<R> {
    Submitted { plan: HedgePlan, result: R },
    Skipped(PostLiquidationHedgeSkip),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostLiquidationHedgeSkip {
    LiquidationNotSubmitted,
    NoHedgePlan(HedgeSkipReason),
}

pub async fn read_standard_margin_liquidation_cycle(
    config: &HypercallLiquidatorConfig,
    api: &HypercallClient,
    account: &str,
    positions: Vec<PositionDelta>,
) -> Result<CycleDecision, CycleError> {
    let wallet = WalletAddress::from_str(account).map_err(|source| CycleError::InvalidAccount {
        account: account.to_string(),
        detail: Box::new(source),
    })?;
    let account_address = AccountAddress::from(wallet);
    let status = api
        .get_liquidation_status(&account_address)
        .await?
        .ok_or_else(|| CycleError::LiquidationStatusNotFound {
            account: account.to_string(),
        })?;

    if !status.margin_mode.eq_ignore_ascii_case("standard")
        || status.state != "in_liquidation"
        || status.liquidation_mode.as_deref() != Some("full")
    {
        return Ok(CycleDecision::Skip(CycleSkipReason::NoActiveAuction {
            account: account.to_string(),
        }));
    }

    let Some(full) = status.full_liquidation else {
        return Ok(CycleDecision::Skip(
            CycleSkipReason::MissingStandardMarginBidTerms {
                account: account.to_string(),
                field: "full_liquidation".to_string(),
            },
        ));
    };
    let terms = match standard_margin_bid_terms_from_status(account, &wallet, &full) {
        Ok(terms) => terms,
        Err(CycleError::MissingCurrentBidTerms { account, field }) => {
            return Ok(CycleDecision::Skip(
                CycleSkipReason::MissingStandardMarginBidTerms {
                    account,
                    field: field.to_string(),
                },
            ));
        }
        Err(error) => return Err(error),
    };
    let current_bid_usdc = terms.bid_usdc;
    let margin = MarginSnapshot {
        mode: status.margin_mode,
        equity: terms.equity,
        initial_margin_required: terms.mm_required,
        maintenance_margin_required: terms.mm_required,
        maintenance_margin: terms.maintenance_margin,
    };

    let decision = plan_liquidation_cycle(
        config,
        account,
        margin,
        Some(AuctionStatus {
            account: account.to_string(),
            start_time: full.started_at.unwrap_or_default() as u64,
            current_bid_usdc,
            current_cost_usdc: Decimal::ZERO,
        }),
        positions,
    )?;
    let CycleDecision::Eligible {
        mut candidate,
        auction,
        planner,
    } = decision
    else {
        return Ok(decision);
    };
    candidate.standard_margin_terms = Some(terms);
    Ok(CycleDecision::Eligible {
        candidate,
        auction,
        planner,
    })
}

pub async fn discover_standard_margin_liquidation_cycles(
    config: &HypercallLiquidatorConfig,
    api: &HypercallClient,
) -> Result<Vec<CycleDecision>, CycleError> {
    let max_accounts = config.liquidation.max_accounts_per_cycle.max(1);
    let mut cursor = None;
    let mut seen_accounts: Vec<String> = Vec::new();
    let mut active_decisions = Vec::new();
    let mut inactive_decisions = Vec::new();

    loop {
        let page = api
            .get_public_liquidations(&PublicLiquidationsQuery {
                cursor: cursor.clone(),
                limit: Some(max_accounts),
                status: Some("in_liquidation".to_string()),
                margin_mode: Some("standard".to_string()),
                liquidation_mode: Some("full".to_string()),
                ..Default::default()
            })
            .await?;

        let mut accounts = Vec::new();
        for entry in page.data {
            if !seen_accounts
                .iter()
                .any(|account| account.eq_ignore_ascii_case(&entry.wallet))
            {
                seen_accounts.push(entry.wallet.clone());
                accounts.push(entry.wallet);
            }
        }

        if !accounts.is_empty() {
            let concurrency = accounts.len().clamp(1, 8);
            let decisions = stream::iter(accounts.into_iter().map(|account| async move {
                read_standard_margin_liquidation_cycle(config, api, &account, Vec::new()).await
            }))
            .buffered(concurrency)
            .try_collect::<Vec<_>>()
            .await?;

            for decision in decisions {
                if matches!(
                    decision,
                    CycleDecision::Skip(CycleSkipReason::NoActiveAuction { .. })
                        | CycleDecision::Skip(
                            CycleSkipReason::MissingStandardMarginBidTerms { .. }
                        )
                ) {
                    inactive_decisions.push(decision);
                } else {
                    active_decisions.push(decision);
                    if active_decisions.len() >= max_accounts {
                        active_decisions.truncate(max_accounts);
                        return Ok(active_decisions);
                    }
                }
            }
        }

        cursor = page.page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    if active_decisions.is_empty() {
        inactive_decisions.truncate(max_accounts);
        Ok(inactive_decisions)
    } else {
        active_decisions.truncate(max_accounts);
        Ok(active_decisions)
    }
}

pub fn plan_liquidation_cycle(
    config: &HypercallLiquidatorConfig,
    account: &str,
    margin: MarginSnapshot,
    auction: Option<AuctionStatus>,
    positions: Vec<PositionDelta>,
) -> Result<CycleDecision, CycleError> {
    let Some(auction) = auction else {
        return Ok(CycleDecision::Skip(CycleSkipReason::NoActiveAuction {
            account: account.to_string(),
        }));
    };

    let candidate = LiquidationCandidate {
        account: account.to_string(),
        margin,
        current_bid_usdc: auction.current_bid_usdc,
        positions,
        standard_margin_terms: None,
    };
    let planner = evaluate_candidate(&config.liquidation, &candidate)?;
    match planner {
        CandidateDecision::Eligible { .. } => Ok(CycleDecision::Eligible {
            candidate,
            auction,
            planner,
        }),
        skip => Ok(CycleDecision::Skip(CycleSkipReason::Planner(skip))),
    }
}

pub async fn execute_standard_margin_liquidation_cycle(
    config: &HypercallLiquidatorConfig,
    api: &HypercallClient,
    decision: CycleDecision,
) -> Result<LiquidationExecution, CycleError> {
    let CycleDecision::Eligible {
        candidate, auction, ..
    } = decision
    else {
        return Err(CycleError::SkippedCycle);
    };

    if config.liquidation.dry_run {
        return Ok(LiquidationExecution::DryRun { candidate, auction });
    }

    let liquidated_wallet = WalletAddress::from_str(&candidate.account).map_err(|source| {
        CycleError::InvalidAccount {
            account: candidate.account.clone(),
            detail: Box::new(source),
        }
    })?;
    let liquidated_account = AccountAddress::from(liquidated_wallet);
    let status = api
        .get_liquidation_status(&liquidated_account)
        .await?
        .ok_or_else(|| CycleError::LiquidationStatusNotFound {
            account: candidate.account.clone(),
        })?;
    if !status.margin_mode.eq_ignore_ascii_case("standard")
        || status.state != "in_liquidation"
        || status.liquidation_mode.as_deref() != Some("full")
    {
        return Err(CycleError::LiquidationStatusNotFullStandardMargin {
            account: candidate.account.clone(),
        });
    }
    let full = status
        .full_liquidation
        .ok_or_else(|| CycleError::MissingCurrentBidTerms {
            account: candidate.account.clone(),
            field: "full_liquidation",
        })?;
    let terms =
        standard_margin_bid_terms_from_status(&candidate.account, &liquidated_wallet, &full)?;
    let margin = MarginSnapshot {
        mode: status.margin_mode,
        equity: terms.equity,
        initial_margin_required: terms.mm_required,
        maintenance_margin_required: terms.mm_required,
        maintenance_margin: terms.maintenance_margin,
    };
    let candidate = LiquidationCandidate {
        margin,
        standard_margin_terms: Some(terms.clone()),
        ..candidate
    };
    let fresh_candidate = LiquidationCandidate {
        current_bid_usdc: terms.bid_usdc,
        ..candidate
    };
    if fresh_candidate.margin.maintenance_excess() >= Decimal::ZERO {
        return Err(CycleError::LiquidationStatusNotFullStandardMargin {
            account: fresh_candidate.account.clone(),
        });
    }
    match evaluate_candidate(&config.liquidation, &fresh_candidate)? {
        CandidateDecision::Eligible { .. } => {}
        skip => return Err(CycleError::FreshAuctionNotEligible(skip)),
    }

    let liquidator =
        hypercall_wallet_from_key_config(&config.keys.hypercall, config.hypercall.chain_id).await?;
    let request_id = uuid::Uuid::now_v7();
    let bid_intent_hash = format!("hypercall-liquidator:{request_id}");
    let response = api
        .submit_standard_margin_liquidation_with_params(
            &liquidator,
            StandardMarginLiquidationParams {
                liquidated_wallet: liquidated_account,
                request_id,
                auction_id: terms.auction_id.clone(),
                bid_usdc: terms.bid_usdc.to_string(),
                positions: terms.positions,
                portfolio_hash: terms.portfolio_hash.clone(),
                auction_terms_hash: terms.auction_terms_hash.clone(),
                auction_version: terms.auction_version,
                valuation_timestamp_ms: terms.valuation_timestamp_ms,
                bid_intent_hash,
                nonce: None,
            },
        )
        .await?;

    Ok(LiquidationExecution::SubmittedStandardMargin {
        receipt: StandardMarginLiquidationReceipt {
            account: fresh_candidate.account,
            liquidator_wallet: liquidator.address.to_string(),
            bid_usdc: terms.bid_usdc,
            request_id: response.request_id.clone(),
            auction_id: response.auction_id.clone(),
            response,
        },
    })
}

fn standard_margin_bid_terms_from_status(
    account: &str,
    wallet: &WalletAddress,
    full: &hypercall_sdk_types::FullLiquidationStatusData,
) -> Result<StandardMarginLiquidationBidTerms, CycleError> {
    let missing = |field| CycleError::MissingCurrentBidTerms {
        account: account.to_string(),
        field,
    };
    let auction_id = full
        .auction_id
        .clone()
        .ok_or_else(|| missing("auction_id"))?;
    let bid_usdc = full
        .current_required_bid_usdc
        .ok_or_else(|| missing("current_required_bid_usdc"))?;
    let equity = full
        .current_equity
        .ok_or_else(|| missing("current_equity"))?;
    let mm_required = full
        .current_mm_required
        .ok_or_else(|| missing("current_mm_required"))?;
    let maintenance_margin = full
        .current_maintenance_margin
        .ok_or_else(|| missing("current_maintenance_margin"))?;
    let positions = full
        .current_positions
        .clone()
        .ok_or_else(|| missing("current_positions"))?;
    let portfolio_hash = full
        .current_portfolio_hash
        .clone()
        .ok_or_else(|| missing("current_portfolio_hash"))?;
    let auction_version = full
        .current_auction_version
        .ok_or_else(|| missing("current_auction_version"))?;
    let valuation_timestamp_ms = full
        .current_valuation_timestamp_ms
        .ok_or_else(|| missing("current_valuation_timestamp_ms"))?;
    let auction_terms_hash = public_standard_margin_auction_terms_hash(
        &auction_id,
        wallet,
        auction_version,
        &portfolio_hash,
        bid_usdc,
        valuation_timestamp_ms,
    );

    Ok(StandardMarginLiquidationBidTerms {
        auction_id,
        bid_usdc,
        equity,
        mm_required,
        maintenance_margin,
        positions,
        portfolio_hash,
        auction_terms_hash,
        auction_version,
        valuation_timestamp_ms,
    })
}

fn public_standard_margin_auction_terms_hash(
    auction_id: &str,
    liquidated_wallet: &WalletAddress,
    auction_version: u64,
    portfolio_hash: &str,
    bid_usdc: Decimal,
    valuation_timestamp_ms: u64,
) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hash_str(&mut hasher, "standard-margin-liquidation-auction-terms-v5");
    hash_str(&mut hasher, auction_id);
    hash_str(&mut hasher, &liquidated_wallet.to_string());
    hasher.update(auction_version.to_le_bytes());
    hash_str(&mut hasher, portfolio_hash);
    hash_str(&mut hasher, &bid_usdc.normalize().to_string());
    hasher.update(valuation_timestamp_ms.to_le_bytes());
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn hash_str(hasher: &mut sha2::Sha256, value: &str) {
    use sha2::Digest;

    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

pub async fn execute_hedge_after_liquidation<V>(
    config: &HypercallLiquidatorConfig,
    liquidation: &LiquidationExecution,
    venue: &V,
    input: &DeltaHedgeInput,
) -> hypercall_client::error::Result<HedgeExecution<V::OrderResult>>
where
    V: PerpVenue,
{
    if !matches!(
        liquidation,
        LiquidationExecution::SubmittedStandardMargin { .. }
    ) {
        return Ok(HedgeExecution::Skipped(
            PostLiquidationHedgeSkip::LiquidationNotSubmitted,
        ));
    }

    let plan = match plan_delta_hedge(&config.hedge, input) {
        HedgePlanDecision::Place(plan) => plan,
        HedgePlanDecision::Skip(reason) => {
            return Ok(HedgeExecution::Skipped(
                PostLiquidationHedgeSkip::NoHedgePlan(reason),
            ));
        }
    };
    let result = submit_hedge_plan(venue, &plan).await?;
    Ok(HedgeExecution::Submitted { plan, result })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HypercallLiquidatorConfig;
    use hypercall_client::ClientError;
    use hypercall_hyperliquid::{
        PerpVenueCancelByClientIdRequest, PerpVenueCancelByOidRequest, PerpVenueFuture,
        PerpVenueOrderRequest, Tif,
    };
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;
    use std::sync::{Arc, Mutex};

    fn config() -> HypercallLiquidatorConfig {
        HypercallLiquidatorConfig::from_toml_str(include_str!(
            "../examples/liquidator.example.toml"
        ))
        .unwrap()
    }

    fn margin(equity: Decimal) -> MarginSnapshot {
        MarginSnapshot {
            mode: "standard".to_string(),
            equity,
            initial_margin_required: dec!(1500),
            maintenance_margin_required: dec!(1000),
            maintenance_margin: equity - dec!(1000),
        }
    }

    fn auction() -> AuctionStatus {
        AuctionStatus {
            account: "0x0000000000000000000000000000000000000001".to_string(),
            start_time: 1,
            current_bid_usdc: dec!(1000),
            current_cost_usdc: dec!(-50),
        }
    }

    #[test]
    fn skips_without_active_auction() {
        let decision = plan_liquidation_cycle(
            &config(),
            "0x0000000000000000000000000000000000000001",
            margin(dec!(900)),
            None,
            Vec::new(),
        )
        .unwrap();

        assert!(matches!(
            decision,
            CycleDecision::Skip(CycleSkipReason::NoActiveAuction { .. })
        ));
    }

    #[test]
    fn eligible_uses_standard_margin_bid_terms() {
        let decision = plan_liquidation_cycle(
            &config(),
            "0x0000000000000000000000000000000000000001",
            margin(dec!(900)),
            Some(auction()),
            Vec::new(),
        )
        .unwrap();

        let CycleDecision::Eligible {
            candidate,
            auction,
            planner,
        } = decision
        else {
            panic!("expected eligible liquidation");
        };
        assert_eq!(candidate.current_bid_usdc, dec!(1000));
        assert_eq!(auction.current_cost_usdc, dec!(-50));
        assert!(matches!(planner, CandidateDecision::Eligible { .. }));
    }

    #[test]
    fn standard_margin_bid_terms_report_missing_current_field() {
        let account = "0x0000000000000000000000000000000000000001";
        let wallet = WalletAddress::from_str(account).unwrap();
        let full = hypercall_sdk_types::FullLiquidationStatusData {
            auction_id: Some("auction-1".to_string()),
            request_id: None,
            tx_hash: None,
            started_at: Some(1),
            chain_start_time: None,
            margin_needed: None,
            stop_request_id: None,
            stop_tx_hash: None,
            liquidated_at: None,
            winner: None,
            bonus: None,
            resolution_tx_hash: None,
            current_required_bid_usdc: Some(dec!(1000)),
            current_equity: Some(dec!(900)),
            current_mm_required: Some(dec!(1000)),
            current_maintenance_margin: Some(dec!(-100)),
            current_positions: None,
            current_portfolio_hash: Some("portfolio-hash".to_string()),
            current_auction_terms_hash: None,
            current_auction_version: Some(1),
            current_valuation_timestamp_ms: Some(1_700_000_000_000),
        };

        let error = standard_margin_bid_terms_from_status(account, &wallet, &full)
            .expect_err("missing positions should fail term extraction");

        assert!(matches!(
            error,
            CycleError::MissingCurrentBidTerms {
                account: returned_account,
                field: "current_positions",
            } if returned_account == account
        ));
    }

    #[test]
    fn standard_margin_bid_terms_use_current_margin_fields() {
        let account = "0x0000000000000000000000000000000000000001";
        let wallet = WalletAddress::from_str(account).unwrap();
        let full = hypercall_sdk_types::FullLiquidationStatusData {
            auction_id: Some("auction-1".to_string()),
            request_id: None,
            tx_hash: None,
            started_at: Some(1),
            chain_start_time: None,
            margin_needed: None,
            stop_request_id: None,
            stop_tx_hash: None,
            liquidated_at: None,
            winner: None,
            bonus: None,
            resolution_tx_hash: None,
            current_required_bid_usdc: Some(dec!(1100)),
            current_equity: Some(dec!(900)),
            current_mm_required: Some(dec!(1000)),
            current_maintenance_margin: Some(dec!(-100)),
            current_positions: Some(Vec::new()),
            current_portfolio_hash: Some("portfolio-hash".to_string()),
            current_auction_terms_hash: None,
            current_auction_version: Some(1),
            current_valuation_timestamp_ms: Some(1_700_000_000_000),
        };

        let terms = standard_margin_bid_terms_from_status(account, &wallet, &full)
            .expect("complete current terms should extract");

        assert_eq!(terms.bid_usdc, dec!(1100));
        assert_eq!(terms.equity, dec!(900));
        assert_eq!(terms.mm_required, dec!(1000));
        assert_eq!(terms.maintenance_margin, dec!(-100));
    }

    #[test]
    fn standard_margin_auction_terms_hash_matches_engine_vector() {
        let wallet = WalletAddress::from_str("0x0000000000000000000000000000000000000001").unwrap();

        let hash = public_standard_margin_auction_terms_hash(
            "auction-1",
            &wallet,
            1,
            "portfolio-hash",
            dec!(1100.00),
            1_700_000_000_000,
        );

        assert_eq!(
            hash,
            "sha256:500166c79edbea78bb54b448587648b3bc8246d4b3ce13951423e828d23991c8"
        );
    }

    #[tokio::test]
    async fn hedge_is_skipped_until_liquidation_submission_is_observed() {
        let cfg = config();
        let venue = RecordingVenue::default();
        let liquidation = LiquidationExecution::DryRun {
            candidate: LiquidationCandidate {
                account: "0x0000000000000000000000000000000000000001".to_string(),
                margin: margin(dec!(900)),
                current_bid_usdc: dec!(1000),
                positions: Vec::new(),
                standard_margin_terms: None,
            },
            auction: auction(),
        };
        let hedge = execute_hedge_after_liquidation(
            &cfg,
            &liquidation,
            &venue,
            &DeltaHedgeInput {
                underlying: "BTC".to_string(),
                mark_price_usdc: dec!(70000),
                positions: vec![PositionDelta {
                    symbol: "BTC-20261231-90000-C".to_string(),
                    underlying: "BTC".to_string(),
                    delta: -0.5,
                }],
            },
        )
        .await
        .unwrap();

        assert!(matches!(
            hedge,
            HedgeExecution::Skipped(PostLiquidationHedgeSkip::LiquidationNotSubmitted)
        ));
        assert!(venue.orders().is_empty());
    }

    #[tokio::test]
    async fn hedge_submits_after_liquidation_submission() {
        let cfg = config();
        let venue = RecordingVenue::default();
        let liquidation = LiquidationExecution::SubmittedStandardMargin {
            receipt: StandardMarginLiquidationReceipt {
                account: "0x0000000000000000000000000000000000000001".to_string(),
                liquidator_wallet: "0x0000000000000000000000000000000000000002".to_string(),
                bid_usdc: dec!(1000),
                request_id: "0197d846-f400-7000-8000-000000000001".to_string(),
                auction_id: "auction-1".to_string(),
                response: StandardMarginLiquidationOrderResponse {
                    request_id: "0197d846-f400-7000-8000-000000000001".to_string(),
                    auction_id: "auction-1".to_string(),
                    liquidated_wallet: "0x0000000000000000000000000000000000000001".to_string(),
                    liquidator_wallet: "0x0000000000000000000000000000000000000002".to_string(),
                },
            },
        };
        let hedge = execute_hedge_after_liquidation(
            &cfg,
            &liquidation,
            &venue,
            &DeltaHedgeInput {
                underlying: "BTC".to_string(),
                mark_price_usdc: dec!(70000),
                positions: vec![PositionDelta {
                    symbol: "BTC-20261231-90000-C".to_string(),
                    underlying: "BTC".to_string(),
                    delta: -0.5,
                }],
            },
        )
        .await
        .unwrap();

        assert!(matches!(hedge, HedgeExecution::Submitted { .. }));
        assert_eq!(venue.orders().len(), 1);
        assert_eq!(venue.orders()[0].tif, Tif::Ioc);
    }

    #[derive(Clone, Default)]
    struct RecordingVenue {
        orders: Arc<Mutex<Vec<PerpVenueOrderRequest>>>,
    }

    impl RecordingVenue {
        fn orders(&self) -> Vec<PerpVenueOrderRequest> {
            self.orders.lock().expect("orders poisoned").clone()
        }
    }

    impl PerpVenue for RecordingVenue {
        type OrderResult = PerpVenueOrderRequest;

        fn place_order<'a>(
            &'a self,
            request: PerpVenueOrderRequest,
        ) -> PerpVenueFuture<'a, Self::OrderResult> {
            Box::pin(async move {
                self.orders
                    .lock()
                    .expect("orders poisoned")
                    .push(request.clone());
                Ok(request)
            })
        }

        fn cancel_by_oid(
            &self,
            _request: PerpVenueCancelByOidRequest,
        ) -> PerpVenueFuture<'_, Self::OrderResult> {
            Box::pin(async {
                Err(ClientError::InvalidInput(
                    "recording venue does not support cancel_by_oid".to_string(),
                ))
            })
        }

        fn cancel_by_client_id(
            &self,
            _request: PerpVenueCancelByClientIdRequest,
        ) -> PerpVenueFuture<'_, Self::OrderResult> {
            Box::pin(async {
                Err(ClientError::InvalidInput(
                    "recording venue does not support cancel_by_client_id".to_string(),
                ))
            })
        }
    }
}
