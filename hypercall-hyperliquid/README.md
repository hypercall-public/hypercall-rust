# hypercall-hyperliquid


Direct Hyperliquid implementation of the public `hypercall-client` perp venue
trait.

## Responsibilities

- Map `hypercall-client` perp venue requests into native Hyperliquid order and
  cancel requests.
- Sign and submit native Hyperliquid actions with `hypersdk`.
- Keep direct Hyperliquid dependencies out of `hypercall-client`.

## Getting Started

Add the adapter from the public Git repository:

```toml
[dependencies]
hypercall-hyperliquid = { git = "https://github.com/hypercall-public/hypercall-rust" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Construct a venue with the asset registry for the target Hyperliquid chain:

```rust,no_run
use hypercall_hyperliquid::{
    DirectHyperliquidPerpVenue, HyperliquidPerpAssetRegistry,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let venue = DirectHyperliquidPerpVenue::mainnet(
        "0xYOUR_PRIVATE_KEY",
        HyperliquidPerpAssetRegistry::mainnet_defaults(),
    )?;

    let _ = venue;
    Ok(())
}
```

## AWS KMS

Enable the `aws-kms` feature to build a venue from an AWS KMS secp256k1 key:

```rust,no_run
use hypercall_hyperliquid::{
    DirectHyperliquidPerpVenue, HyperliquidPerpAssetRegistry,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let venue = DirectHyperliquidPerpVenue::testnet_aws_kms(
        "alias/hypercall-hyperliquid-trader",
        HyperliquidPerpAssetRegistry::testnet_defaults(),
    )
    .await?;

    let _ = venue;
    Ok(())
}
```

The constructor loads AWS credentials from the process environment through the
standard AWS SDK provider chain.

## Development

Run the focused checks before changing this crate:

```bash
cargo test -p hypercall-hyperliquid
cargo check -p hypercall-hyperliquid --features aws-kms
cargo test -p hypercall-client
```

## Feature Flags

| feature | used by | why |
| --- | --- | --- |
| default | Hyperliquid adapter users | Empty, avoids AWS signer dependencies by default. |
| aws-kms | AWS/KMS adapter users | Enables Alloy AWS signer support. |
