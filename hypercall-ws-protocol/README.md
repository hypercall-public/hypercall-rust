# hypercall-ws-protocol


WebSocket and quote-provider protocol DTOs for Hypercall clients.

## Contents

- Client subscription and control messages.
- Quote-provider request and response payloads.

## Getting Started

Add the protocol crate from the public Git repository:

```toml
[dependencies]
hypercall-ws-protocol = { git = "https://github.com/hypercall-public/hypercall-rust" }
```

Most trading integrations should use `hypercall-client`. Depend on this crate
directly only when you need to serialize or inspect WebSocket messages without
the client runtime.

## Features

- `rfq-v2` (default): private `rfq_v2` channel (`WsChannel::RfqV2`). See the
  `hypercall-client` README.

## Compatibility

Serde field names and enum variants are part of the public wire contract. Keep
changes explicit and covered by serialization tests.
