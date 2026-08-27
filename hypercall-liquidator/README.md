# hypercall-liquidator


Reference standard margin liquidator for Hypercall.

This crate is intentionally conservative. It is a reference implementation for
operators building their own liquidator, not a turnkey production service.

## Getting Started

Clone the public repository and validate the example config:

```bash
git clone https://github.com/hypercall-public/hypercall-rust
cd hypercall-rust
cargo run -p hypercall-liquidator -- check-config \
  --config hypercall-liquidator/examples/liquidator.example.toml
```

For library use, depend on the crate directly:

```toml
[dependencies]
hypercall-liquidator = { git = "https://github.com/hypercall-public/hypercall-rust" }
```

## What It Does

- Reads standard margin facts from the public Hypercall portfolio API.
- Discovers candidate accounts from the public Hypercall liquidation API.
- Reads current full-liquidation bid terms from the Hypercall liquidation status API.
- Applies a configured maintenance-margin buffer before considering a bid.
- Assumes liquidation penalty profitability for the first version.
- Submits a signed standard-margin liquidation order through the Hypercall REST API when `dry_run = false`.
- Plans and submits a separate taker-only Hyperliquid delta hedge when the caller supplies acquired deltas.
- Exposes a feature-gated Leptos dashboard component through `--features ui`.
- Keeps Hypercall liquidation eligibility separate from external hedge state.

## What It Does Not Do

- It does not run portfolio margin liquidations.
- It does not credit Hyperliquid collateral, PnL, or margin into Hypercall.
- It does not guarantee profitability.
- It does not move collateral automatically.
- It does not make fake hedge fills for tests or demos.
- It does not infer option delta from portfolio rows that do not contain delta.

## Boundary

Hypercall state decides whether an account is liquidatable. The Hyperliquid
hedge only manages the liquidator operator's risk. If Hyperliquid state is
missing, stale, or under-margined, the liquidator must pause or skip hedging,
not change Hypercall margin facts.

## Config

Start from:

```sh
cp hypercall-liquidator/examples/liquidator.example.toml ./liquidator.toml
```

Validate without executing anything:

```sh
cargo run -p hypercall-liquidator -- check-config --config ./liquidator.toml
```

Inspect public liquidation candidates without submitting transactions:

```sh
cargo run -p hypercall-liquidator -- inspect --config ./liquidator.toml
```

Pass `--account <wallet>` to inspect a specific account instead of discovering
candidate accounts from `/liquidations`.

Run one cycle. With the example config this returns a dry-run execution record:

```sh
cargo run -p hypercall-liquidator -- run-once --config ./liquidator.toml
```

`dry_run = true` is the default example. A config with `dry_run = false` is
rejected by the CLI unless `--allow-live-execution` is passed.

To submit the external Hyperliquid hedge after a live liquidation, pass the
hedge facts explicitly and opt into the live hedge:

```sh
cargo run -p hypercall-liquidator -- run-once \
  --config ./liquidator.toml \
  --allow-live-execution \
  --allow-live-hedge \
  --hyperliquid-chain testnet \
  --hedge-underlying BTC \
  --hedge-mark-price-usdc 70000 \
  --hedge-delta -0.5 \
  --hedge-position-symbol BTC-20261231-90000-C
```

The CLI only submits the hedge after a liquidation transaction has been
submitted. Dry-run liquidations and skipped liquidation cycles never submit a
hedge order.

## Key Modes

Plaintext env key:

```toml
[keys.hypercall]
kind = "plaintext"
private_key_env = "HYPERCALL_LIQUIDATOR_PRIVATE_KEY"
```

Hypercall API KMS key reference:

```toml
[keys.hypercall]
kind = "kms"
provider = "aws"
key_id_env = "HYPERCALL_KMS_KEY_ID"
```

AWS KMS is supported for Hypercall API request signing when built with
`--features kms`. Direct Hyperliquid CLI hedging uses `keys.hyperliquid` and
currently requires `kind = "plaintext"` unless the caller supplies a compatible
venue signer implementation.

## UI

The `ui` feature exposes a Leptos `LiquidatorDashboard` component plus a
serializable `DashboardSnapshot` model. The snapshot has separate panels for:

- Hypercall equity, IM, and MM.
- Hyperliquid equity, IM, and MM.
- Liquidation bid and maintenance excess.
- Hedge venue, order side, symbol, and notional.
- Collateral prompts.
- Kill switches.

Compile it explicitly:

```sh
cargo test -p hypercall-liquidator --features ui
```

Open the static preview for screenshot QA:

```sh
open hypercall-liquidator/examples/dashboard.preview.html
```

## Feature Flags

| feature | used by | why |
| --- | --- | --- |
| default | liquidator users | Enables the optional Leptos UI and rustls for the reference app. |
| rustls | live integrations | Enables the Hypercall client's rustls backend. |
| native-tls | native TLS users | Enables the Hypercall client's native-tls backend. |
| kms | operators using AWS KMS | Enables AWS KMS wallet signing through `hypercall-client`. |
| ui | public liquidator UI | Enables Leptos UI components. |
