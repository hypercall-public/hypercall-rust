# hypercall-sdk-types


Shared Rust DTOs for Hypercall SDK clients.

## Contents

- API request and response models.
- WebSocket event models.
- Helper enums and address types.
- Strict portfolio-margin and managed-perp directive models.

## Getting Started

Add the type crate from the public Git repository:

```toml
[dependencies]
hypercall-sdk-types = { git = "https://github.com/hypercall-public/hypercall-rust" }
```

Most integrations should use `hypercall-client` instead. Depend on this crate
directly only when you need public request, response, or WebSocket DTOs without
the client runtime.

## Features

- `schemars`: enables JSON Schema derives for public API and WebSocket DTOs.
- `utoipa`: enables OpenAPI schema derives for public API and WebSocket DTOs.
- `asyncapi`: enables AsyncAPI message derives and implies `schemars`.
- `test-utils`: enables test helpers for downstream crates.
- `rfq-v2` (default): RFQ V2 DTOs (`rfq_v2`) and the `AuthenticateRfqV2`,
  `RfqV2Authenticated`, and `RfqV2Changed` WebSocket messages. See the
  `hypercall-client` README.

## Compatibility

Serde field names and enum variants are public contracts. Keep changes explicit
and covered by serialization tests.

Canonical `Portfolio`, `Order`, `FillApiResponse`, and WebSocket order/fill
models require the server to identify margin mode and instrument family. Use the
typed `margin_mode_kind()` and `instrument_kind()` accessors when matching these
retained string wire fields. Managed-perp directive integers serialize as JSON
numbers only through JavaScript's safe integer range and as decimal strings
above it.

### Unreleased breaking changes

- Standard Margin portfolio rows now use `Option<Decimal>` for liquidation
  prices. `None` means no valid per-position liquidation estimate; it must not
  become a zero price. Margin summaries add the optional `standard_margin`
  public field, so Rust struct literals must supply it.
- The existing summary excess fields and NAV-based helper signatures retain
  their meaning. New cash-aware consumers use the explicit initial and
  maintenance headroom in the Standard breakdown and reject missing context.
  Per-position margin diagnostics are no longer additive account allocations.
  See the [API migration guide](https://docs.hypercall.xyz/docs/trading/over-api/operations/changelog#no-new-release---september-2-2026).

- `WsOrderMessage::filled_size` adds a public struct field, so downstream Rust
  struct literals must provide it. The value is the cumulative quantity filled
  for the order and is populated on `PARTIALLY_FILLED` updates. Older payloads
  without the optional field continue to deserialize it as `None`.
- [`RiskGridResponse::wallet`](src/api_models.rs) adds a required public struct field, so downstream
  Rust struct literals must provide it. The value is the canonical Hypercall
  account resolved from the requested manager or account. Older PM risk-grid
  payloads that omit `wallet` fail to deserialize, so clients built with this
  type require an API deployment that returns the field.
- [`WsMessage::Subscribe`](src/ws_protocol.rs) loses its `batch` field, and [`WsMessage::Subscribed`](src/ws_protocol.rs)
  and [`WsMessage::Error`](src/ws_protocol.rs) gain optional `batch`, `code`, and `channel` fields,
  so downstream Rust struct literals and exhaustive patterns need updating. A
  [`Subscribed`](src/ws_protocol.rs) for `indicative_market_data_v2` without `batch: true` means the
  server predates the channel, so fall back to `indicative_market_data`; a
  [`Subscribe`](src/ws_protocol.rs) to a name that is not a channel now returns [`Error`](src/ws_protocol.rs) with
  `code: "unknown_channel"` instead of a silent confirmation.

## Feature Flags

| feature | used by | why |
| --- | --- | --- |
| default | public SDK users | Empty by default. |
| schemars | API spec exporters | Enables JSON Schema derives for DTOs that appear in generated specs. |
| utoipa | API spec exporters | Enables OpenAPI schema derives for DTOs that appear in generated specs. |
| asyncapi | AsyncAPI exporter | Enables AsyncAPI message derives and JSON Schema support for WebSocket payloads. |
| test-utils | tests | Enables shared test constructors and fixtures. |
