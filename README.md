# Hypercall Public SDK Packages

Public SDK packages for Hypercall API, WebSocket, signing, Hyperliquid hedging,
standard margin liquidation, and physically settled covered-call integrations.

## Getting Started

Add the client from the public Git repository:

```toml
[dependencies]
hypercall-client = { git = "https://github.com/hypercall-public/hypercall-rust" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Pin a tag or commit SHA for production integrations once a release tag is
available:

```toml
[dependencies]
hypercall-client = { git = "https://github.com/hypercall-public/hypercall-rust", tag = "v0.3.0" }
```

Then create a production API client:

```rust,no_run
use hypercall_client::HypercallClient;

#[tokio::main]
async fn main() -> hypercall_client::Result<()> {
    let client = HypercallClient::new("https://api.hypercall.xyz");
    let markets = client.get_markets().await?;

    println!("markets: {}", markets.len());
    Ok(())
}
```

## Packages

- `hypercall-covered-call-maker-sdk`: Rust client for policy-validating, signing, and
  submitting standalone covered-call RFQs, plus constructing explicit manual
  exercise actions.
- `hypercall-client`: API, WebSocket, wallet, RFQ (V1 and V2), and perp
  helper client.
- `hypercall-hyperliquid`: Direct Hyperliquid implementation of the public perp
  venue trait.
- `hypercall-liquidator`: Reference standard margin liquidator.
- `hypercall-sdk-types`: API request/response, WebSocket, enums, and address
  types shared by Hypercall SDK clients.
- `hypercall-ws-protocol`: WebSocket and quote-provider protocol DTOs.

## Compatibility

- Serde field names and enum variants are part of the public wire contract.
- Feature flags are part of the public package surface.
- New releases should preserve documented behavior or call out breaking changes
  clearly in release notes.

## Development

Run focused checks before publishing package changes:

```bash
cargo test -p hypercall-client
cargo test -p hypercall-covered-call-maker-sdk
cargo test -p hypercall-hyperliquid
cargo test -p hypercall-liquidator
cargo test -p hypercall-sdk-types
cargo test -p hypercall-ws-protocol
```
