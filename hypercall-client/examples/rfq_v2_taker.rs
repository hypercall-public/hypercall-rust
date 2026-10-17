//! RFQ V2 taker: request a quote for one option, wait for firm quotes, and
//! execute the best one. Cancels the RFQ if nothing executable arrives.
//!
//! ```bash
//! HYPERCALL_PRIVATE_KEY=0x... RFQ_INSTRUMENT=BTC-20261225-100000-C \
//!   cargo run -p hypercall-client --example rfq_v2_taker
//! ```
//!
//! Optional: `HYPERCALL_API_URL` (default `https://api.hypercall.xyz`),
//! `HYPERCALL_CHAIN_ID` (default `999`), `RFQ_SIDE` (`buy` or `sell`, default
//! `buy`), `RFQ_CONTRACTS` (whole contracts, default `1`).
//!
//! This places a real trade on the target environment.

use std::time::Duration;

use alloy::primitives::B256;
use hypercall_client::rfq_v2::{
    encode_rfq_v2_read_grant, select_best_rfq_v2_quote, RfqV2BestQuoteCriteria,
    RfqV2ReadCredentials,
};
use hypercall_client::rfq_v2_recovery::RfqV2ReconcilePolicy;
use hypercall_client::rfq_v2_signing::{RfqV2ActionAuth, RfqV2InvalidationTarget};
use hypercall_client::{ClientError, HypercallClient, HypercallWallet};
use hypercall_sdk_types::{
    ListRfqV2QuotesQuery, RfqV2Digest, RfqV2ExecutionMode, RfqV2FeeLimits, RfqV2FillPolicy,
    RfqV2Firmness, RfqV2PackageLeg, RfqV2QuotePolicy, RfqV2QuoteView, RfqV2Routing, RfqV2Side,
    RfqV2TakerIdentity, RfqV2Terms,
};
use uuid::Uuid;

/// One package unit is 1,000,000 on the wire.
const PACKAGE: u128 = 1_000_000;

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_url = env_or("HYPERCALL_API_URL", "https://api.hypercall.xyz");
    let chain_id: u32 = env_or("HYPERCALL_CHAIN_ID", "999").parse()?;
    let instrument = std::env::var("RFQ_INSTRUMENT")?;
    let taker_side = match env_or("RFQ_SIDE", "buy").as_str() {
        "buy" => RfqV2Side::Buy,
        "sell" => RfqV2Side::Sell,
        other => return Err(format!("RFQ_SIDE must be buy or sell, not {other}").into()),
    };
    let contracts: u128 = env_or("RFQ_CONTRACTS", "1").parse()?;

    let wallet =
        HypercallWallet::from_private_key(&std::env::var("HYPERCALL_PRIVATE_KEY")?, chain_id)?;
    let client = HypercallClient::new(&api_url);
    let taker = wallet.address.into_wallet_address();

    // Nonces are millisecond timestamps, unique per signer. Take them from
    // the server clock and step by one for each signed action.
    let now = client.server_now_ms().await?;
    let mut nonce = now;
    let mut next_nonce = || {
        nonce += 1;
        nonce
    };

    // One package of `contracts` contracts, filled whole or not at all, sent
    // to every eligible maker.
    let terms = RfqV2Terms {
        id: Uuid::now_v7(),
        taker_account: taker,
        legs: vec![RfqV2PackageLeg {
            instrument,
            taker_side,
            quantity_micros_per_unit: contracts * 1_000_000,
        }],
        target: PACKAGE,
        fill_policy: RfqV2FillPolicy::FullSizeOnce,
        execution_mode: RfqV2ExecutionMode::Manual,
        auto_execution_limit: None,
        quote_policy: RfqV2QuotePolicy::FirmOnly,
        taker_identity: RfqV2TakerIdentity::Disclosed,
        routing: RfqV2Routing::AllEligible,
        quote_deadline_ms: now + 20_000,
        expires_at_ms: now + 60_000,
        fee_limits: RfqV2FeeLimits {
            per_execution: 0,
            cumulative: 0,
        },
        builder: None,
        // Only anonymous RFQs need a blinding value.
        private_terms_blinding: RfqV2Digest(B256::ZERO),
    };
    let create = wallet.sign_create_rfq_v2(terms, next_nonce()).await?;
    let created = client.create_rfq_v2(&create).await?;
    let rfq_id = created.rfq_id;
    println!(
        "created RFQ {rfq_id}, quotes close at {}",
        created.quote_deadline_ms
    );

    // Private reads take a signed grant. One grant covers this whole session.
    let grant = wallet
        .sign_rfq_v2_read_grant(taker, now, now + 300_000, next_nonce())
        .await?;
    let token = encode_rfq_v2_read_grant(&grant)?;
    let credentials = || RfqV2ReadCredentials {
        bearer_token: &token,
    };
    let firm_quotes = ListRfqV2QuotesQuery {
        firmness: Some(RfqV2Firmness::Firm),
        ..ListRfqV2QuotesQuery::default()
    };

    let best = loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let now = client.server_now_ms().await?;
        let parent = client.get_rfq_v2(rfq_id, credentials()).await?;
        let quotes = client
            .list_rfq_v2_quotes(rfq_id, &firm_quotes, credentials())
            .await?;
        let criteria = RfqV2BestQuoteCriteria::for_parent(&parent, parent.remaining_target, now);
        if let Ok(RfqV2QuoteView::Firm { terms, .. }) =
            select_best_rfq_v2_quote(&quotes.quotes, criteria)
        {
            break Some(terms.clone());
        }
        if now >= created.quote_deadline_ms {
            break None;
        }
    };

    let Some(quote) = best else {
        let auth = RfqV2ActionAuth::new(client.server_now_ms().await? + 30_000, next_nonce());
        let cancel = wallet
            .sign_rfq_v2_invalidation(RfqV2InvalidationTarget::CancelRfq { rfq_id }, auth)
            .await?;
        client.cancel_rfq_v2(rfq_id, &cancel).await?;
        println!("no executable quote; cancelled RFQ {rfq_id}");
        return Ok(());
    };
    println!(
        "best quote {} from {}: maker receives premium {} = {}",
        quote.id,
        quote.maker_account,
        quote.maker_receives_reference_premium,
        quote.reference_premium_micros
    );

    // The execution signature names the quote, the size, and the fee cap.
    let auth = RfqV2ActionAuth::new(client.server_now_ms().await? + 30_000, next_nonce());
    let execution = wallet
        .sign_rfq_v2_execution(rfq_id, quote.id, PACKAGE, 0, auth)
        .await?;
    let result = match client.execute_rfq_v2(rfq_id, &execution).await {
        Ok(result) => result,
        // A timeout or 5xx is not a result. Resend the identical signed
        // request under its operation ID until the server answers.
        Err(ClientError::RfqV2SubmissionAmbiguous { .. }) => {
            let policy = RfqV2ReconcilePolicy {
                retry_interval: Duration::from_secs(2),
                max_resubmissions: 5,
            };
            client
                .reconcile_rfq_v2_execution(rfq_id, &execution, policy)
                .await?
                .response
        }
        Err(error) => return Err(error.into()),
    };
    println!(
        "execution {}: {} fill(s), {} package units left",
        result.status,
        result.executions.len(),
        result.rfq_remaining_target
    );
    Ok(())
}
