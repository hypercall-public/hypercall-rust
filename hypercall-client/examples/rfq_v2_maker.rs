//! RFQ V2 maker: arm the quote dead-man switch, find RFQs routed to this
//! account, and answer each with one firm quote or a decline.
//!
//! ```bash
//! HYPERCALL_PRIVATE_KEY=0x... QUOTE_PRICE_MICROS=1250000 \
//!   cargo run -p hypercall-client --example rfq_v2_maker
//! ```
//!
//! Optional: `HYPERCALL_API_URL` (default `https://api.hypercall.xyz`),
//! `HYPERCALL_CHAIN_ID` (default `999`), `MAKER_ROUNDS` (polling rounds,
//! default `30`).
//!
//! `price_leg` quotes every leg at `QUOTE_PRICE_MICROS`. Replace it with your
//! own pricing before pointing this at a live account: the quotes it signs
//! are executable.

use std::collections::HashSet;
use std::time::Duration;

use hypercall_client::rfq_v2::{encode_rfq_v2_read_grant, RfqV2ReadCredentials};
use hypercall_client::rfq_v2_signing::{rfq_v2_reference_premium, RfqV2ActionAuth};
use hypercall_client::{HypercallClient, HypercallWallet};
use hypercall_sdk_types::{
    ListRfqV2Query, RfqV2AuthorizedTerms, RfqV2FeeLimits, RfqV2FillPolicy, RfqV2FirmQuoteTerms,
    RfqV2MakerTerms, RfqV2PricedLeg, RfqV2Response, RfqV2Role, RfqV2WithdrawalPolicy,
    WalletAddress,
};
use uuid::Uuid;

/// How long each signed quote stays executable.
const QUOTE_TTL_MS: u64 = 15_000;
/// Every RFQ quote is withdrawn if the switch is not renewed within this.
const DEAD_MAN_MS: u64 = 30_000;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

/// Placeholder pricing: one flat price per leg, in USD micros.
fn price_leg(_instrument: &str) -> Option<u128> {
    std::env::var("QUOTE_PRICE_MICROS").ok()?.parse().ok()
}

/// Build firm quote terms for the full remaining size of one RFQ, or `None`
/// when this maker does not price every leg.
fn quote_for(
    rfq: &RfqV2Response,
    terms: &RfqV2MakerTerms,
    maker: WalletAddress,
    now_ms: u64,
) -> Option<RfqV2FirmQuoteTerms> {
    let generation = rfq.maker_quote_generation?;
    let priced_legs = terms
        .legs
        .iter()
        .map(|leg| {
            Some(RfqV2PricedLeg {
                instrument: leg.instrument.clone(),
                price_micros: price_leg(&leg.instrument)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    // A full-size quote must cover the whole target at its reference size.
    // Partial-fill quotes reference one step.
    let (capacity, reference_quantity, step) = match terms.fill_policy {
        RfqV2FillPolicy::FullSizeOnce => (terms.target, terms.target, None),
        RfqV2FillPolicy::OneSmallerPackage { minimum, step }
        | RfqV2FillPolicy::RepeatedPartial { minimum, step } => {
            if rfq.remaining_target < minimum {
                return None;
            }
            (rfq.remaining_target, step, Some(step))
        }
    };
    let (maker_receives, premium) =
        rfq_v2_reference_premium(&terms.legs, &priced_legs, reference_quantity, step).ok()?;
    Some(RfqV2FirmQuoteTerms {
        id: Uuid::now_v7(),
        rfq_id: rfq.rfq_id,
        rfq_terms_commitment: rfq.terms_commitment,
        maker_account: maker,
        maker_generation: generation.maker_generation,
        rfq_generation: generation.rfq_generation,
        priced_legs,
        reference_quantity,
        maker_receives_reference_premium: maker_receives,
        reference_premium_micros: premium,
        capacity,
        fill_policy: terms.fill_policy,
        withdrawal_policy: RfqV2WithdrawalPolicy::Permitted,
        fee_limits: RfqV2FeeLimits {
            per_execution: 0,
            cumulative: 0,
        },
        expires_at_ms: terms.expires_at_ms.min(now_ms + QUOTE_TTL_MS),
        protection_scope: None,
        source_indicative_quote_id: None,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_url = env_or("HYPERCALL_API_URL", "https://api.hypercall.xyz");
    let chain_id: u32 = env_or("HYPERCALL_CHAIN_ID", "999").parse()?;
    let rounds: u32 = env_or("MAKER_ROUNDS", "30").parse()?;

    let wallet =
        HypercallWallet::from_private_key(&std::env::var("HYPERCALL_PRIVATE_KEY")?, chain_id)?;
    let client = HypercallClient::new(&api_url);
    let maker = wallet.address.into_wallet_address();

    let now = client.server_now_ms().await?;
    let mut nonce = now;
    let mut next_nonce = || {
        nonce += 1;
        nonce
    };
    let grant = wallet
        .sign_rfq_v2_read_grant(maker, now, now + 600_000, next_nonce())
        .await?;
    let token = encode_rfq_v2_read_grant(&grant)?;
    let open_for_me = ListRfqV2Query {
        role: Some(RfqV2Role::Maker),
        status: Some("open".into()),
        ..ListRfqV2Query::default()
    };

    let mut answered = HashSet::new();
    for _ in 0..rounds {
        let now = client.server_now_ms().await?;

        // Renew the dead-man switch every round. If this process stops, the
        // server withdraws every open quote once the deadline passes.
        let schedule = wallet
            .sign_rfq_v2_quote_cancel_schedule(
                maker,
                Some(now + DEAD_MAN_MS),
                RfqV2ActionAuth::new(now + 10_000, next_nonce()),
            )
            .await?;
        client.schedule_rfq_v2_quote_cancel(&schedule).await?;

        let page = client
            .list_rfqs_v2(
                &open_for_me,
                RfqV2ReadCredentials {
                    bearer_token: &token,
                },
            )
            .await?;
        for rfq in page.rfqs {
            let RfqV2AuthorizedTerms::Maker { terms } = &rfq.parent else {
                continue;
            };
            if terms.quote_deadline_ms <= now || !answered.insert(rfq.rfq_id) {
                continue;
            }
            match quote_for(&rfq, terms, maker, now) {
                Some(quote) => {
                    let request = wallet.sign_rfq_v2_firm_quote(quote, next_nonce()).await?;
                    let published = client
                        .publish_rfq_v2_firm_quote(rfq.rfq_id, &request)
                        .await?;
                    println!("quoted RFQ {}: {published:?}", rfq.rfq_id);
                }
                None => {
                    let decline = wallet
                        .sign_rfq_v2_decline(
                            rfq.rfq_id,
                            maker,
                            RfqV2ActionAuth::new(terms.quote_deadline_ms, next_nonce()),
                        )
                        .await?;
                    client.decline_rfq_v2(rfq.rfq_id, &decline).await?;
                    println!("declined RFQ {}", rfq.rfq_id);
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // Disarm the switch on a clean exit.
    let now = client.server_now_ms().await?;
    let disarm = wallet
        .sign_rfq_v2_quote_cancel_schedule(
            maker,
            None,
            RfqV2ActionAuth::new(now + 10_000, next_nonce()),
        )
        .await?;
    client.schedule_rfq_v2_quote_cancel(&disarm).await?;
    Ok(())
}
