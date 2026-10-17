# hypercall-client


Rust API client for Hypercall.

## Responsibilities

- HTTP and websocket clients for Hypercall API surfaces.
- EIP-712 wallet helpers for public options, RFQ, and quote-provider signing.
- TEE perp trading: enrollment, builder approval, orders, triggers, batches,
  cancels, leverage, preview, and markets.
- RFQ V2: EIP-712 signing for every action, the `/private/rfqs*` transport,
  ambiguous-submission recovery, and the private `rfq_v2` WebSocket channel.
- Request and response convenience types re-exported from `hypercall-sdk-types`.

## Getting Started

Add the client from the public Git repository:

```toml
[dependencies]
hypercall-client = { git = "https://github.com/hypercall-public/hypercall-rust" }
rust_decimal = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Create a production API client:

```rust,no_run
use hypercall_client::HypercallClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = HypercallClient::new("https://api.hypercall.xyz");
    let instruments = client.get_instrument_specs("BTC").await?;

    println!("BTC instruments: {}", instruments.len());
    Ok(())
}
```

## WebSocket recovery

`WsClient` reconnects established `/ws` sessions with capped exponential
backoff and jitter. It identifies the wallet again, restores the exact desired
subscriptions and filters, and waits for subscription confirmations before
reporting that the transport is connected.

```rust,no_run
use hypercall_client::{WsClient, WsConnectionState};

#[tokio::main]
async fn main() -> hypercall_client::Result<()> {
    let ws = WsClient::new();
    ws.connect("https://api.hypercall.xyz", None).await?;
    ws.subscribe_with_options(
        "indicative_market_data",
        Some(vec!["BTC".to_string()]),
        None,
        None,
    )
    .await?;

    if let WsConnectionState::RecoveryRequired(plan) = ws.connection_state() {
        // Fetch the snapshots or REST state selected by `plan`, then acknowledge it.
        ws.mark_recovered()?;
    }
    Ok(())
}
```

Replaceable public streams become ready after resubscription. Ordered public
and private streams enter `RecoveryRequired` because reconnecting cannot prove
that no events were missed. Callers must fetch the required order book snapshot,
portfolio snapshot, or durable REST state before calling `mark_recovered()`.
Portfolio subscribers must do both because durable position-expiry events share
the portfolio channel.
Use `connection_state_receiver()` to watch transitions and
`last_disconnect_reason()` to inspect structured slow-consumer closes.

## Batched indicative market data

`subscribe_indicative_market_data(symbols)` subscribes to
`indicative_market_data_v2`, which delivers one `IndicativeMarketDataBatch`
frame per batch window (25 ms by default). Rows arrive up to one window later,
in exchange for far fewer frames that compress much better when the connection
uses `permessage-deflate`. If the server confirms the channel without
`"batch": true` or rejects it as `unknown_channel`, the client moves the
subscription to `indicative_market_data`, one `IndicativeMarketData` frame per
row. Opt out with `WsClient::new().with_batched_indicative(false)`. Read rows
through `hypercall_client::indicative`, which handles both:

```rust,no_run
use hypercall_client::{indicative::indicative_rows_from_value, WsClient};

#[tokio::main]
async fn main() -> hypercall_client::Result<()> {
    let ws = WsClient::new();
    ws.connect("https://api.hypercall.xyz", None).await?;
    ws.subscribe_indicative_market_data(Some(vec!["BTC-20261002-87000-C".to_string()]))
        .await?;

    for message in ws.get_messages().await {
        if let Some(rows) = indicative_rows_from_value(&message)? {
            for row in rows {
                println!("{} bid {:?} ask {:?}", row.instrument, row.best_bid, row.best_ask);
            }
        }
    }
    Ok(())
}
```

## Wallet Signing

`HypercallWallet` supports local private-key signing in every build:

```rust,no_run
use hypercall_client::{HypercallSigner, HypercallWallet};

fn main() -> hypercall_client::Result<()> {
    let wallet = HypercallWallet::from_private_key("0xYOUR_PRIVATE_KEY", 999)?;

    println!("wallet: {}", wallet.address());
    Ok(())
}
```

Enable the `kms` feature to use AWS KMS-backed signing through the AWS SDK
default credential chain:

```bash
cargo check -p hypercall-client --features kms
```

Private key export only works for local private-key wallets. KMS wallets never
expose private key material.

## Unified-account setup

Create and register the managed `Account.sol` before selecting portfolio
margin. The caller supplies the HyperEVM RPC URL and the Factory address for
the target deployment. The SDK reads the Factory's live minimum HYPE deposit
and applies the same 1% deposit and 20% gas-limit buffers as the Hypercall app.

```rust,no_run
use hypercall_client::{
    AccountAddress, CreateAccountParams, HypercallClient, HypercallWallet, MarginMode,
    UpdateApiWalletParams,
};
use alloy::primitives::keccak256;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = HypercallClient::new("https://api.hypercall.xyz");
    let manager = HypercallWallet::from_private_key("0xYOUR_MANAGER_WALLET_KEY", 999)?;
    let api_wallet = HypercallWallet::from_private_key("0xYOUR_API_WALLET_KEY", 999)?;
    let factory: AccountAddress = "0xYOUR_FACTORY_ADDRESS".parse()?;

    let created = client
        .create_account(
            &manager,
            CreateAccountParams {
                rpc_url: "https://YOUR_HYPEREVM_RPC".to_string(),
                factory_address: factory,
                salt: None,
            },
        )
        .await?;
    client
        .update_api_wallet(
            &manager,
            UpdateApiWalletParams {
                account: created.account,
                name: keccak256("primary-api-wallet"),
                api_wallet: api_wallet.address,
                nonce: None,
            },
        )
        .await?;
    client
        .set_margin_mode_typed(&manager, MarginMode::Portfolio)
        .await?;

    println!("account: {}", created.account);
    println!("transaction: {}", created.transaction_hash);
    Ok(())
}
```

The manager and salt select a deterministic CREATE2 address. Account creation
is not idempotent: submitting again with an already-used manager/salt pair
reverts. `salt: None` uses salt zero to match the Hypercall app. Supply another
salt only when deliberately creating a distinct managed account.

API-wallet updates are manager-authorized directives. Reusing a name replaces
its current API wallet. Passing the zero address removes that named wallet.

Account abstraction changes are manager-authorized. The SDK binds the signed
action target to the supplied managed Account address.

```rust,no_run
use hypercall_client::{
    AccountAddress, DirectiveDeliveryStatus, DirectiveStage, HypercallClient, HypercallWallet,
    HypercoreAccountAbstraction, SetAccountAbstractionParams,
};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = HypercallClient::new("https://api.hypercall.xyz");
    let manager = HypercallWallet::from_private_key("0xYOUR_MANAGER_WALLET_KEY", 999)?;
    let account: AccountAddress = "0xYOUR_ACCOUNT_SOL_ADDRESS".parse()?;

    let submitted = client
        .set_account_abstraction(
            &manager,
            SetAccountAbstractionParams {
                account,
                abstraction: HypercoreAccountAbstraction::UnifiedAccount,
                nonce: None,
            },
        )
        .await?;
    if submitted.stage == DirectiveStage::Rejected {
        return Err(format!("account abstraction rejected: {:?}", submitted.rejection).into());
    }

    loop {
        let status = client.get_directive_status(&submitted.directive_id).await?;
        match status.delivery_status {
            DirectiveDeliveryStatus::Finalized => break,
            DirectiveDeliveryStatus::CoreRejected
            | DirectiveDeliveryStatus::Reverted
            | DirectiveDeliveryStatus::Expired
            | DirectiveDeliveryStatus::DeadLettered => {
                return Err(format!(
                    "account abstraction delivery failed: {:?}",
                    status.delivery_status
                )
                .into());
            }
            _ => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }

    Ok(())
}
```

`Finalized` confirms transaction delivery, not that HyperCore has exposed the
new account mode. After finalization, poll Hyperliquid's authoritative Info API
until `userAbstraction` is exactly `unifiedAccount`. Do not submit a
risk-increasing perp order before that read confirms the mode. The typed SDK
deliberately exposes only unified-account mode because the server and Account
contract reject all other `hl_set_abstraction` values.

## Portfolio margin perp orders

Managed perp methods take an existing Account.sol address and the numeric
HyperCore asset ID. Creating or discovering the account and authorizing the API
wallet are separate setup steps. This example assumes the authoritative
Hyperliquid Info API already reports `userAbstraction` as `unifiedAccount`.
Prices and sizes must be positive and exactly representable with 8 decimal
places. The client rejects unsupported precision or overflow instead of
rounding.

```rust,no_run
use hypercall_client::{
    AccountAddress, DirectiveDeliveryStatus, DirectiveStage, HypercallClient, HypercallWallet,
    PerpCancelByCloidParams, PerpLimitOrderParams, PerpTimeInForce, Side,
};
use rust_decimal::Decimal;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = HypercallClient::new("https://api.hypercall.xyz");
    let signer = HypercallWallet::from_private_key("0xYOUR_API_WALLET_KEY", 999)?;
    let account: AccountAddress = "0xYOUR_ACCOUNT_SOL_ADDRESS".parse()?;
    let asset = 0; // Numeric HyperCore asset ID supplied by your integration.
    let client_order_id = 42_u128;

    let submitted = client
        .place_perp_limit_order(
            &signer,
            PerpLimitOrderParams {
                account,
                asset,
                side: Side::Buy,
                price: Decimal::new(100_000_125, 3),
                size: Decimal::new(1, 2),
                tif: PerpTimeInForce::Gtc,
                reduce_only: false,
                client_order_id: Some(client_order_id),
                nonce: None,
            },
        )
        .await?;

    match submitted.stage {
        DirectiveStage::Rejected => {
            println!("rejected: {:?}", submitted.rejection);
            return Ok(());
        }
        DirectiveStage::Enqueued | DirectiveStage::Submitted => {}
    }

    let status = loop {
        let status = client
            .get_directive_status(&submitted.directive_id)
            .await?;
        if matches!(
            status.delivery_status,
            DirectiveDeliveryStatus::Included
                | DirectiveDeliveryStatus::CoreRejected
                | DirectiveDeliveryStatus::Finalized
                | DirectiveDeliveryStatus::Reverted
                | DirectiveDeliveryStatus::Expired
                | DirectiveDeliveryStatus::DeadLettered
        ) {
            break status;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    println!("delivery status: {:?}", status.delivery_status);

    let portfolio = client.get_portfolio_snapshot(account).await?;
    println!("margin mode: {:?}", portfolio.margin_mode_kind()?);

    let orders = client
        .get_orders_typed(account, Some("open"), Some(50), None)
        .await?;
    for order in &orders.data {
        println!("{}: {:?}", order.symbol, order.instrument_kind()?);
    }

    client
        .cancel_perp_order_by_cloid(
            &signer,
            PerpCancelByCloidParams {
                account,
                asset,
                client_order_id,
                nonce: None,
            },
        )
        .await?;

    Ok(())
}
```

When `client_order_id` is omitted for placement, the selected nonce is used as
the `u128` client order ID. An explicit nonce is available on every managed perp
request for deterministic orchestration. Allocate nonces durably and atomically
per signer. Never reuse a nonce, including when retrying a request. The response
always reports the server's actual stage, directive ID, recovered signer,
rejection, transaction hash, and any fills it knows about.

## TEE perp trading

TEE perp methods submit Hyperliquid perp orders that Hypercall executes through
a TEE-held Hyperliquid API wallet, after Portfolio Margin admission. They are
separate from the managed `/v1/actions/*` directive methods above and use the
same app key (an active Account.sol API wallet) to sign.

Setup, once per account, with the manager wallet:

1. `enroll_tee_api_wallet` requests the account's TEE API wallet, signs the
   `HLAddApiWallet` approval (domain `HypercallManagerSign`), submits it, and
   waits until Hyperliquid binds the wallet. `HyperliquidInfoClient::is_tee_api_wallet_active`
   reads the enrollment status.
2. `approve_hypercall_builder` sends `Account.approveZeroFeeBuilder(builder)`
   (Account 0.2.4 or newer) with the builder from `/exchange-info`
   `hl_builder_address`. Until then orders come back with
   `errorCode = builder_not_approved` (`requires_builder_approval()`).

Trading, with the app key (domain `HypercallApiSign`):

| Method | Route | Signed type |
| --- | --- | --- |
| `place_tee_perp_limit_order` (GTC, IOC, ALO), `place_tee_perp_market_order` (bounded IOC) | `POST /v1/tee-perp/orders/submit` | `HLOrder` |
| `place_tee_perp_trigger_order` (stop or take-profit, market or limit) | `POST /v1/tee-perp/orders/submit-trigger` | `HCPerpTriggerOrder` |
| `place_tee_perp_bracket_order` (`normalTpsl`), `place_tee_perp_position_tpsl` (`positionTpsl`), `place_tee_perp_scale_order` and `place_tee_perp_order_batch` (`na`) | `POST /v1/tee-perp/orders/submit-batch` | `HCPerpOrderBatch` |
| `cancel_tee_perp_order_by_oid` | `POST /v1/tee-perp/cancels/submit-by-oid` | `HLCancel` |
| `cancel_tee_perp_order_by_cloid` | `POST /v1/tee-perp/cancels/submit` | `HLCancelByCloid` |
| `update_tee_perp_leverage` | `POST /v1/tee-perp/leverage/submit` | `HCPerpLeverageChange` |
| `modify_tee_perp_order` | `POST /v1/tee-perp/orders/modify` | `HCPerpOrderModify` |
| `batch_modify_tee_perp_orders` | `POST /v1/tee-perp/orders/batch-modify` | `HCPerpOrderBatchModify` |
| `place_tee_perp_twap_order` | `POST /v1/tee-perp/twap/submit` | `HCPerpTwapOrder` |
| `cancel_tee_perp_twap` | `POST /v1/tee-perp/twap/cancel` | `HCPerpTwapCancel` |
| `preview_tee_perp_action` | `POST /v1/tee-perp/preview` | none (read-only) |
| `get_tee_perp_markets` | `GET /v1/tee-perp/markets` | none |

Nonces are Unix milliseconds on the API server's clock. The server derives the
venue expiry as `nonce + 9000` and accepts it only within the next 10 seconds,
so a nonce must lie in `(server_now - 9000, server_now + 1000]`. Leave `nonce`
as `None` and the client stamps it from the `/health` `Date` header (cached for
a minute), strictly increasing per signer. Batch child cloids are
`(nonce << 8) + index + 1` (`tee_perp_batch_cloid`).

Every submit returns a stage. `unknown` means the venue may have received the
action: reconcile before acting again, and never resubmit the same intent on
another route. A signed submission that fails in transit (transport error,
timeout, `5xx`, or an undecodable `2xx`) returns
`ClientError::TeeSubmissionAmbiguous` carrying the exact signed body; resend
those bytes with `resend_tee_submission` (the server deduplicates them) or
reconcile. Calling the placing method again signs a new nonce, and new
nonce-derived cloids, so it is a second order. Rejections carry a typed `PerpRejectionReasonCode`, the same
code the preview returns.

```rust,no_run
use hypercall_client::{
    AccountAddress, ApproveHypercallBuilderParams, HypercallClient, HypercallWallet,
    HyperliquidInfoClient, PerpCancelByCloidParams, PerpLimitOrderParams, PerpPreviewAction,
    PerpPreviewRequest, PerpTimeInForce, Side, TeeApiWalletEnrollmentPolling, TeePerpBracketEntry,
    TeePerpBracketOrderParams, TeePerpExit, TeePerpLeverageParams, TeePerpMarketOrderParams,
    TeePerpTpsl, TeePerpTriggerExecution, TeePerpTriggerOrderParams, PERP_MARKET_SLIPPAGE_BPS,
};
use rust_decimal::Decimal;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = HypercallClient::new("https://api.hypercall.xyz");
    let manager = HypercallWallet::from_private_key("0xYOUR_MANAGER_KEY", 999)?;
    let app_key = HypercallWallet::from_private_key("0xYOUR_API_WALLET_KEY", 999)?;
    let account: AccountAddress = "0xYOUR_ACCOUNT_SOL_ADDRESS".parse()?;
    let hyperliquid = HyperliquidInfoClient::mainnet();

    // One-time setup with the manager wallet.
    let enrollment = client
        .enroll_tee_api_wallet(
            &manager,
            account,
            &hyperliquid,
            TeeApiWalletEnrollmentPolling::default(),
        )
        .await?;
    println!("TEE API wallet: {}", enrollment.api_wallet);
    client
        .approve_hypercall_builder(
            &manager,
            ApproveHypercallBuilderParams {
                rpc_url: "https://rpc.hyperliquid.xyz/evm".into(),
                account,
                builder: None, // read from /exchange-info hl_builder_address
            },
        )
        .await?;

    // Market metadata and a read-only preview.
    let markets = client.get_tee_perp_markets().await?;
    let btc = markets
        .by_hyperliquid_coin("BTC")
        .ok_or("BTC is not configured")?;
    let preview = client
        .preview_tee_perp_action(&PerpPreviewRequest {
            account: account.into_wallet_address(),
            action: PerpPreviewAction::Leverage {
                asset: btc.asset_id,
                is_cross: true,
                leverage: 5,
            },
        })
        .await?;
    println!(
        "{:?} {:?}, liquidation after {:?}",
        preview.decision, preview.reason_code, preview.liquidation_px_after
    );

    // Leverage, then read the venue's setting back.
    client
        .update_tee_perp_leverage(
            &app_key,
            TeePerpLeverageParams {
                account,
                asset: btc.asset_id,
                is_cross: true,
                leverage: 5,
                nonce: None,
            },
        )
        .await?;
    let venue = hyperliquid
        .active_asset_leverage(account, &btc.hyperliquid_coin)
        .await?;
    println!("venue leverage: {venue:?}");

    // Limit (GTC, IOC, or ALO) and market (bounded IOC) orders.
    let limit = client
        .place_tee_perp_limit_order(
            &app_key,
            PerpLimitOrderParams {
                account,
                asset: btc.asset_id,
                side: Side::Buy,
                price: Decimal::new(95_000, 0),
                size: Decimal::new(1, 3),
                tif: PerpTimeInForce::Alo,
                reduce_only: false,
                client_order_id: Some(42),
                nonce: None,
            },
        )
        .await?;
    if limit.requires_builder_approval() {
        return Err("approve the Hypercall builder first".into());
    }
    client
        .place_tee_perp_market_order(
            &app_key,
            TeePerpMarketOrderParams {
                account,
                asset: btc.asset_id,
                side: Side::Buy,
                size: Decimal::new(1, 3),
                reference_price: Decimal::new(100_000, 0), // best ask
                size_decimals: 5,
                slippage_bps: Some(50),
                reduce_only: false,
                client_order_id: None,
                nonce: None,
            },
        )
        .await?;

    // A stop-market order, and an entry with attached TP/SL.
    client
        .place_tee_perp_trigger_order(
            &app_key,
            TeePerpTriggerOrderParams {
                account,
                asset: btc.asset_id,
                side: Side::Sell,
                size: Decimal::new(1, 3),
                trigger_price: Decimal::new(90_000, 0),
                tpsl: TeePerpTpsl::Sl,
                execution: TeePerpTriggerExecution::Market {
                    size_decimals: 5,
                    slippage_bps: None,
                },
                reduce_only: true,
                client_order_id: None,
                nonce: None,
            },
        )
        .await?;
    let exit = |trigger: i64| {
        TeePerpExit::market(
            Decimal::new(trigger, 0),
            Side::Sell,
            5,
            PERP_MARKET_SLIPPAGE_BPS,
        )
    };
    let bracket = client
        .place_tee_perp_bracket_order(
            &app_key,
            TeePerpBracketOrderParams {
                account,
                entry: TeePerpBracketEntry {
                    asset: btc.asset_id,
                    side: Side::Buy,
                    price: Decimal::new(99_000, 0),
                    size: Decimal::new(1, 3),
                    tif: PerpTimeInForce::Gtc,
                    reduce_only: false,
                },
                take_profit: Some(exit(110_000)?),
                stop_loss: Some(exit(92_000)?),
                nonce: None,
            },
        )
        .await?;
    println!("bracket {:?}: {} children", bracket.stage, bracket.children.len());

    client
        .cancel_tee_perp_order_by_cloid(
            &app_key,
            PerpCancelByCloidParams {
                account,
                asset: btc.asset_id,
                client_order_id: 42,
                nonce: None,
            },
        )
        .await?;
    Ok(())
}
```

Market and market-trigger prices follow the Hypercall order panel: the
reference price moved `slippage_bps` against the trader (default
`PERP_MARKET_SLIPPAGE_BPS`, 2000 bps), rounded to a valid Hyperliquid tick
inside that bound, refused when no such tick still crosses. Scale legs come
from `build_perp_scale_order_legs`. Schedule-cancel and prepared cancels are not
part of the TEE perp SDK surface.

### Modify, batch modify and TWAP

`modify_tee_perp_order` replaces one resting limit order's price, size, side,
TIF and reduce-only flag. `batch_modify_tee_perp_orders` replaces several under
one signature and one venue nonce (Hyperliquid `batchModify`). The engine
admits every child or none, then each child settles on its own, so read
`children` (each with its `index`, `requestId`, `targetDirectiveId`, `cloid` and
`stage`) rather than only the whole-action stage, which is `accepted` or
`rejected` only when every child agrees. `children_to_reconcile()` lists the
children whose outcome is not definitive; a child stage this SDK does not know
parses as `Unrecognized` and is treated as unknown. A `rejected` or
`not_submitted` child leaves the original order resting.

The client refuses a batch before signing when it is empty, repeats a target or
cloid, or exceeds `max_children` (default 50, the server's default
`api.tee_perp_batch_modify_max_children`; never above 100). A batch is charged
one order placement per child against the rate limit.

TWAP (`place_tee_perp_twap_order`, `cancel_tee_perp_twap`) runs 5 to 1440
minutes, and the engine reserves the full size until fills arrive. TWAP and
batch modify are gated by the enclave release: a deployment that has not
enabled one answers `400 unsupported_action`, which
`ClientError::is_unsupported_action()` detects. Nothing was reserved or sent to
the venue in that case.

Hyperliquid parity. TWAP submit and cancel carry every `twapOrder` and
`twapCancel` field (`a`, `b`, `s`, `r`, `m`, `t`; `a`, `t`). Modify and batch
modify have these scope cuts:

- Modify replaces limit orders only (GTC, IOC, ALO). Hyperliquid's
  `modify` can also turn an order into a trigger order; the TEE route does not
  accept that, so a resting trigger order must be cancelled and re-placed.
- The order is addressed by its cloid plus its engine `directiveId`, never by
  `oid`, and the modify keeps the cloid. Hyperliquid also accepts an `oid`
  target and a new cloid.
- A batch modify carries at most the instance cap (50 by default, 100 at
  most), where Hyperliquid documents no per-call limit.
- No builder field: Hyperliquid documents none on `modify`, `batchModify`,
  `twapOrder` or `twapCancel`.

```rust,no_run
use hypercall_client::{
    AccountAddress, HypercallClient, HypercallWallet, PerpTimeInForce, Side,
    TeePerpBatchModifyParams, TeePerpModifyParams, TeePerpOrderReplacement, TeePerpOrderStage,
    TeePerpTwapCancelParams, TeePerpTwapParams,
};
use rust_decimal::Decimal;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = HypercallClient::new("https://api.hypercall.xyz");
    let app_key = HypercallWallet::from_private_key("0xYOUR_API_WALLET_KEY", 999)?;
    let account: AccountAddress = "0xYOUR_ACCOUNT_SOL_ADDRESS".parse()?;

    // `directiveId` and cloid returned when each order was placed.
    let bid = TeePerpOrderReplacement {
        asset: 0,
        target_directive_id: "01999b5a-7c1e-7000-8000-000000000042".parse()?,
        cloid: 42,
        side: Side::Buy,
        price: Decimal::new(99_500, 0),
        size: Decimal::new(2, 3),
        tif: PerpTimeInForce::Alo,
        reduce_only: false,
    };
    let ask = TeePerpOrderReplacement {
        target_directive_id: "01999b5a-7c1e-7000-8000-000000000043".parse()?,
        cloid: 43,
        side: Side::Sell,
        price: Decimal::new(100_500, 0),
        ..bid
    };

    // One order.
    let single = client
        .modify_tee_perp_order(
            &app_key,
            TeePerpModifyParams {
                account,
                replacement: bid,
                nonce: None,
            },
        )
        .await?;
    println!("modify {:?}", single.stage);

    // Both quotes in one signed batchModify; each child settles on its own.
    let batch = client
        .batch_modify_tee_perp_orders(
            &app_key,
            TeePerpBatchModifyParams {
                account,
                replacements: vec![bid, ask],
                max_children: None, // the server default, 50
                nonce: None,
            },
        )
        .await?;
    for child in &batch.children {
        match child.stage {
            TeePerpOrderStage::Accepted => println!("{} moved", child.cloid),
            TeePerpOrderStage::Rejected | TeePerpOrderStage::NotSubmitted => {
                println!("{} unchanged: the old order still rests", child.cloid)
            }
            _ => println!("{} unknown: reconcile before touching it", child.cloid),
        }
    }

    // A native TWAP. Deployments that have not enabled it answer
    // `400 unsupported_action`.
    match client
        .place_tee_perp_twap_order(
            &app_key,
            TeePerpTwapParams {
                account,
                asset: 0,
                side: Side::Sell,
                size: Decimal::new(5, 2),
                reduce_only: true,
                minutes: 30,
                randomize: true,
                nonce: None,
            },
        )
        .await
    {
        Ok(twap) => {
            if let Some(twap_id) = twap.twap_id {
                client
                    .cancel_tee_perp_twap(
                        &app_key,
                        TeePerpTwapCancelParams {
                            account,
                            asset: 0,
                            twap_id,
                            nonce: None,
                        },
                    )
                    .await?;
            }
        }
        Err(error) if error.is_unsupported_action() => println!("TWAP is not enabled here"),
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
```

## Funding

Direct deposits are HyperEVM transactions. Withdrawals are signed Hypercall API
requests. The methods are explicit about margin mode so a caller can never fall
back to the wrong custody route.

The caller supplies the HyperEVM RPC URL and environment-specific USDC address.
The SDK fetches Exchange and Router from `/exchange-info`, verifies the API,
wallet, and RPC chain IDs match, and verifies the current account identity and
margin mode before approving funds.

### Standard Margin

```rust,no_run
use hypercall_client::{
    AccountAddress, DepositUsdcParams, HypercallClient, HypercallWallet,
    WithdrawUsdcParams,
};
use rust_decimal_macros::dec;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = HypercallClient::new("https://api.hypercall.xyz");
    let wallet = HypercallWallet::from_private_key("0xYOUR_PRIVATE_KEY", 999)?;
    let usdc: AccountAddress = "0xYOUR_HYPEREVM_USDC_ADDRESS".parse()?;
    let destination: AccountAddress = "0xYOUR_WITHDRAWAL_DESTINATION".parse()?;

    let deposit = client
        .deposit_standard_usdc(
            &wallet,
            DepositUsdcParams {
                rpc_url: "https://YOUR_HYPEREVM_RPC".to_string(),
                usdc_address: usdc,
                account: wallet.address,
                amount: dec!(100),
            },
        )
        .await?;
    println!("deposit transaction: {}", deposit.deposit_transaction_hash);

    let accepted = client
        .withdraw_standard_usdc(
            &wallet,
            WithdrawUsdcParams {
                account: wallet.address,
                destination,
                amount: dec!(10),
                nonce: None,
            },
        )
        .await?;
    println!("withdrawal directive: {}", accepted.directive_id);
    Ok(())
}
```

Standard deposits approve the exact amount only when the existing allowance is
insufficient, then call `Exchange.depositUsdcFor(account, amount)` and wait for
both transactions to confirm successfully.

### Portfolio Margin

Portfolio deposits must be funded by the manager of the canonical Account.sol.
The SDK resolves that identity through the portfolio API, requires Portfolio
Margin and an enabled Router, approves the exact amount only when needed, then
calls `Router.deposit(account, usdc, amount, true)`.

```rust,no_run
use hypercall_client::{
    AccountAddress, DepositUsdcParams, HypercallClient, HypercallWallet,
    WithdrawUsdcParams,
};
use rust_decimal_macros::dec;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = HypercallClient::new("https://api.hypercall.xyz");
    let manager = HypercallWallet::from_private_key("0xYOUR_MANAGER_KEY", 999)?;
    let account: AccountAddress = "0xYOUR_ACCOUNT_SOL".parse()?;
    let usdc: AccountAddress = "0xYOUR_HYPEREVM_USDC_ADDRESS".parse()?;
    let destination: AccountAddress = "0xYOUR_WITHDRAWAL_DESTINATION".parse()?;

    client
        .deposit_portfolio_usdc(
            &manager,
            DepositUsdcParams {
                rpc_url: "https://YOUR_HYPEREVM_RPC".to_string(),
                usdc_address: usdc,
                account,
                amount: dec!(100),
            },
        )
        .await?;

    let accepted = client
        .withdraw_portfolio_usdc(
            &manager,
            WithdrawUsdcParams {
                account,
                destination,
                amount: dec!(10.123456),
                nonce: None,
            },
        )
        .await?;
    let status = client.get_directive_status(&accepted.directive_id).await?;
    println!("withdrawal delivery: {:?}", status.delivery_status);
    Ok(())
}
```

Deposit methods return only after the deposit receipt succeeds. A withdrawal
response means Hypercall accepted the request, not that delivery is final.
Poll `get_directive_status` with the returned directive ID. For durable
orchestration, allocate nonces atomically per signer and never use a nonce for a
different request. If a withdrawal uses an SDK-generated nonce and the HTTP
outcome is ambiguous, the error includes the selected nonce and canonical
request fields. Reconcile first. Any retry of that ambiguous request must reuse
the original nonce and every other canonical field exactly.

## Development

Run the focused client checks before changing this crate:

```bash
cargo test -p hypercall-client
cargo test -p hypercall-client --no-default-features
cargo check -p hypercall-client --features kms
cargo test -p hypercall-client --no-default-features --features alloy-rustls
cargo build -p hypercall-client --examples
```

## Feature Flags

| feature | used by | why |
| --- | --- | --- |
| default | client users | Enables the Alloy rustls backend and `rfq-v2`. |
| alloy-rustls | live integrations | Enables Alloy's reqwest rustls backend. |
| alloy-native-tls | native TLS users | Enables Alloy's reqwest native TLS backend. |
| kms | AWS/client KMS users | Enables Alloy AWS signer support. |
| rfq-v2 (default) | RFQ V2 takers and makers | RFQ V2 client: `rfq_v2`, `rfq_v2_recovery`, `rfq_v2_signing`, signed read grants, and the private `rfq_v2` WebSocket channel. |

### RFQ V2 (`rfq-v2`)

RFQ V2 is a signed RFQ contract under `/private/rfqs*`: the taker signs the
RFQ, the maker signs the quote, and the taker signs the execution. It adds
anonymous takers, selected-maker routing, indicative quotes with firm-up,
repeated partial fills, atomic sweeps across up to 1,024 quotes, bulk quote
withdrawal, and a maker dead-man switch. V1 RFQ (`/rfq/*` and the `rfq`
channel) is unchanged.

The `rfq-v2` feature is on by default since 0.3.0. It also enables `rfq-v2` on
`hypercall-sdk-types` (the RFQ V2 DTOs and the `AuthenticateRfqV2`,
`RfqV2Authenticated`, and `RfqV2Changed` WebSocket messages) and on
`hypercall-ws-protocol` (the `rfq_v2` channel). With
`default-features = false`, list it explicitly:

```toml
[dependencies]
hypercall-client = { git = "https://github.com/hypercall-public/hypercall-rust", tag = "v0.3.0", default-features = false, features = ["alloy-rustls", "rfq-v2"] }
```

| Module | What it does |
| --- | --- |
| `rfq_v2_signing` | Builds and signs every RFQ V2 action with a `HypercallWallet`, and exposes the exact EIP-712 messages and digests for remote signers. |
| `rfq_v2` | HTTP transport for create, quote, firm-up, decline, execute, sweep, cancel, withdraw, replace, the cancel schedule, and authorized reads. `select_best_rfq_v2_quote` picks the best executable firm quote locally. |
| `rfq_v2_recovery` | Exact-retry reconciliation of ambiguous executions and sweeps, the change-feed cursor, a private mirror, and maker hold tracking. |
| `WsClient::connect_rfq_v2` | The private `rfq_v2` change-hint channel, authenticated with a signed read grant and re-authenticated on reconnect. |

Take a quote as a taker:

```rust,no_run
use alloy::primitives::B256;
use hypercall_client::rfq_v2::{
    encode_rfq_v2_read_grant, select_best_rfq_v2_quote, RfqV2BestQuoteCriteria,
    RfqV2ReadCredentials,
};
use hypercall_client::rfq_v2_signing::RfqV2ActionAuth;
use hypercall_client::{HypercallClient, HypercallWallet};
use hypercall_sdk_types::{
    ListRfqV2QuotesQuery, RfqV2Digest, RfqV2ExecutionMode, RfqV2FeeLimits, RfqV2FillPolicy,
    RfqV2PackageLeg, RfqV2QuotePolicy, RfqV2QuoteView, RfqV2Routing, RfqV2Side,
    RfqV2TakerIdentity, RfqV2Terms,
};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = HypercallClient::new("https://api.hypercall.xyz");
    let wallet = HypercallWallet::from_private_key("0xYOUR_PRIVATE_KEY", 999)?;
    let taker = wallet.address.into_wallet_address();
    let now = client.server_now_ms().await?;

    let terms = RfqV2Terms {
        id: Uuid::now_v7(),
        taker_account: taker,
        legs: vec![RfqV2PackageLeg {
            instrument: "BTC-20261225-100000-C".into(),
            taker_side: RfqV2Side::Buy,
            quantity_micros_per_unit: 1_000_000, // 1 contract per package
        }],
        target: 1_000_000, // 1 package
        fill_policy: RfqV2FillPolicy::FullSizeOnce,
        execution_mode: RfqV2ExecutionMode::Manual,
        auto_execution_limit: None,
        quote_policy: RfqV2QuotePolicy::FirmOnly,
        taker_identity: RfqV2TakerIdentity::Disclosed,
        routing: RfqV2Routing::AllEligible,
        quote_deadline_ms: now + 20_000,
        expires_at_ms: now + 60_000,
        fee_limits: RfqV2FeeLimits { per_execution: 0, cumulative: 0 },
        builder: None,
        private_terms_blinding: RfqV2Digest(B256::ZERO),
    };
    let created = client
        .create_rfq_v2(&wallet.sign_create_rfq_v2(terms, now).await?)
        .await?;

    // Private reads take a signed read grant as a bearer token.
    let grant = wallet
        .sign_rfq_v2_read_grant(taker, now, now + 300_000, now + 1)
        .await?;
    let token = encode_rfq_v2_read_grant(&grant)?;
    let read = || RfqV2ReadCredentials { bearer_token: &token };

    // Poll until a quote arrives (the full example also handles the deadline).
    let parent = client.get_rfq_v2(created.rfq_id, read()).await?;
    let quotes = client
        .list_rfq_v2_quotes(created.rfq_id, &ListRfqV2QuotesQuery::default(), read())
        .await?;
    let criteria = RfqV2BestQuoteCriteria::for_parent(&parent, parent.remaining_target, now);
    if let Ok(RfqV2QuoteView::Firm { terms, .. }) =
        select_best_rfq_v2_quote(&quotes.quotes, criteria)
    {
        let auth = RfqV2ActionAuth::new(now + 30_000, now + 2);
        let execution = wallet
            .sign_rfq_v2_execution(created.rfq_id, terms.id, 1_000_000, 0, auth)
            .await?;
        let result = client.execute_rfq_v2(created.rfq_id, &execution).await?;
        println!("{} fill(s)", result.executions.len());
    }
    Ok(())
}
```

Runnable versions, with deadline handling, ambiguous-submission
reconciliation, the maker dead-man switch, and quote pricing, are in
[`examples/rfq_v2_taker.rs`](examples/rfq_v2_taker.rs) and
[`examples/rfq_v2_maker.rs`](examples/rfq_v2_maker.rs):

```bash
HYPERCALL_PRIVATE_KEY=0x... RFQ_INSTRUMENT=BTC-20261225-100000-C \
  cargo run -p hypercall-client --example rfq_v2_taker
```

Rules the server enforces:

- **Nonces** are millisecond timestamps within two days before and one day
  after server time, unique per signer. `HypercallClient::server_now_ms` reads
  the server clock.
- **Operation IDs** on execution, sweep, cancel, withdrawal, bulk withdrawal,
  the cancel schedule, and replacement must be UUIDv7s at most one hour old
  and at most 30 seconds in the future by the server clock, or the server
  answers 400 `rfq_v2_invalid_operation_id`. `RfqV2ActionAuth::new` mints a
  fresh one. Reuse an ID only to retry the identical signed request inside
  that window.
- **Selected makers** are signed as a sorted, duplicate-free set. Build them
  with `RfqV2Routing::selected_makers`; the signer refuses an unsorted list.
- **Quote premiums** must equal what the server derives from the leg prices.
  `rfq_v2_signing::rfq_v2_reference_premium` computes it.
- **Ambiguous submissions** (transport error, timeout, 5xx) on a mutation
  return `ClientError::RfqV2SubmissionAmbiguous`. Do not sign a new request.
  Resend the original with `reconcile_rfq_v2_execution` or
  `reconcile_rfq_v2_sweep`, which resend the identical bytes under the
  original operation ID.
- **Availability**: RFQ V2 is open to every account on
  `https://api.hypercall.xyz`. A non-production environment can run it
  disabled, which answers every V2 route with 403 `rfq_v2_disabled`
  (`ClientError::RfqV2Api`), or limited to an allowlist, which answers 403
  `rfq_v2_not_allowlisted` to accounts not on it.

## WebSocket compression

`WsClient` offers RFC 7692 `permessage-deflate` on every connection and
reconnect. When the server accepts, compressed frames are inflated before they
reach `get_messages()` or `wait_for_message()`, so the JSON is identical either
way. When the server does not accept, the session runs uncompressed. An
inflated message larger than 64 MiB, the same limit as an uncompressed one,
fails the connection before more output is allocated. The client sends its own
messages uncompressed, which RFC 7692 allows.

```rust,no_run
use hypercall_client::WsClient;

#[tokio::main]
async fn main() -> hypercall_client::Result<()> {
    // Opt out, for example to rule compression out while debugging.
    let ws = WsClient::new().with_permessage_deflate(false);
    ws.connect("https://api.hypercall.xyz", None).await?;
    Ok(())
}
```

TLS uses the platform trust store through `native-tls`, for both compressed
and uncompressed sessions.
