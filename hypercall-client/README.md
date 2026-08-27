# hypercall-client


Rust API client for Hypercall.

## Responsibilities

- HTTP and websocket clients for Hypercall API surfaces.
- EIP-712 wallet helpers for public options, RFQ, and quote-provider signing.
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

## Funding

Funding is not a Hypercall API write. Use `get_exchange_info()` to fetch the
production Exchange contract address, chain ID, and signing domain. For the
HyperEVM USDC route, approve USDC to the Exchange contract and call
`depositUsdcFor(account, amount)` with a normal EVM wallet transaction. Do not
send USDC directly to `exchange_address`.

```rust,no_run
use hypercall_client::HypercallClient;

#[tokio::main]
async fn main() -> hypercall_client::Result<()> {
    let client = HypercallClient::new("https://api.hypercall.xyz");
    let exchange = client.get_exchange_info().await?;

    println!("exchange contract: {}", exchange.exchange_address);
    println!("chain id: {}", exchange.chain_id);
    Ok(())
}
```

The on-chain call shape is:

```solidity
function depositUsdcFor(address account, uint256 amount) external;
```

`amount` is USDC token units. For example, `100_000_000` is 100 USDC for a
6-decimal USDC token. Integrators that do not want to manage EVM approval and
transaction submission should fund through the Hypercall app.

Example with Foundry `cast`:

```bash
export HYPEREVM_RPC_URL="https://..."
export PRIVATE_KEY="0x..."
export USDC_ADDRESS="0x..."
export EXCHANGE_ADDRESS="$(curl -s https://api.hypercall.xyz/exchange-info | jq -r .exchange_address)"
export HYPERCALL_ACCOUNT="0x..."
export AMOUNT_UNITS="100000000" # 100 USDC with 6 decimals

cast send \
  --rpc-url "$HYPEREVM_RPC_URL" \
  --private-key "$PRIVATE_KEY" \
  "$USDC_ADDRESS" \
  "approve(address,uint256)" \
  "$EXCHANGE_ADDRESS" \
  "$AMOUNT_UNITS"

cast send \
  --rpc-url "$HYPEREVM_RPC_URL" \
  --private-key "$PRIVATE_KEY" \
  "$EXCHANGE_ADDRESS" \
  "depositUsdcFor(address,uint256)" \
  "$HYPERCALL_ACCOUNT" \
  "$AMOUNT_UNITS"
```

## Development

Run the focused client checks before changing this crate:

```bash
cargo test -p hypercall-client
cargo test -p hypercall-client --no-default-features
cargo check -p hypercall-client --features kms
```

## Feature Flags

| feature | used by | why |
| --- | --- | --- |
| default | client users | Enables the Alloy rustls backend. |
| alloy-rustls | live integrations | Enables Alloy's reqwest rustls backend. |
| alloy-native-tls | native TLS users | Enables Alloy's reqwest native TLS backend. |
| kms | AWS/client KMS users | Enables Alloy AWS signer support. |
