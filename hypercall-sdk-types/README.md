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

- `WsOrderMessage::filled_size` adds a public struct field, so downstream Rust
  struct literals must provide it. The value is the cumulative quantity filled
  for the order and is populated on `PARTIALLY_FILLED` updates. Older payloads
  without the optional field continue to deserialize it as `None`.

## Feature Flags

| feature | used by | why |
| --- | --- | --- |
| default | public SDK users | Empty by default. |
| schemars | API spec exporters | Enables JSON Schema derives for DTOs that appear in generated specs. |
| utoipa | API spec exporters | Enables OpenAPI schema derives for DTOs that appear in generated specs. |
| asyncapi | AsyncAPI exporter | Enables AsyncAPI message derives and JSON Schema support for WebSocket payloads. |
| test-utils | tests | Enables shared test constructors and fixtures. |
