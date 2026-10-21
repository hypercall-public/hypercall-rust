# Changelog

## 0.4.0 (unreleased)

All five public crates move to `0.4.0` together. This is a security release.

### Security: distrust the compromised eMudhra root

eMudhra reported eight compromised intermediate CA keys on October 17, 2026.
The six TLS intermediates chain to `emSign ECC Root CA - G3`, which Mozilla
removed from NSS on October 20 and `webpki-roots` 1.0.10 drops
([advisory](https://thehackernews.com/2026/10/eight-emudhra-ca-keys-compromised.html)).

- `hypercall-client`'s default `alloy-rustls` feature ran the HTTP client on
  reqwest 0.11, which pins `webpki-roots` 0.25.4. That release still trusts
  `emSign ECC Root CA - G3` and will not be updated. The HTTP client now uses
  reqwest 0.12, whose `rustls-tls` trusts `webpki-roots` 1.x.
- `hypercall-client` (with `alloy-rustls`) and `hypercall-hyperliquid` depend
  on `webpki-roots` 1.0.10 directly. Cargo resolves one `webpki-roots` 1.x for
  the whole build, so no build that uses these crates can resolve a root set
  that still trusts the compromised root. This covers reqwest's `rustls-tls`
  and hypersdk's WebSocket.
- The other emSign roots (`emSign Root CA - C1`, `emSign Root CA - G1`,
  `emSign ECC Root CA - C3`) remain in Mozilla's store and in `webpki-roots`.
  They are not implicated in the compromise.
- `alloy-native-tls`, the `WsClient` WebSocket (native-tls), and Alloy 1.8's
  reqwest 0.13 rustls backend (rustls-platform-verifier) use the operating
  system's trust store. Keep the OS current; Apple and Microsoft blocked the
  eight intermediates on October 19 and 20.
- Removing the root does not reject every chain through the compromised
  keys. The advisory reports Sectigo cross-signed copies of the intermediates,
  which chain to Sectigo roots. rustls with a bundled root set does no
  revocation checking (browsers block these through CRLSet and OneCRL), so
  a chain through a cross-signed copy is not rejected by this release.

### Breaking

- reqwest types in the public API move from 0.11 to 0.12:
  `ClientError::Http(reqwest::Error)` and `HypercallClient::raw_post`, which
  returns `reqwest::Response`. Code that names these types must depend on
  reqwest 0.12.
- With the `gzip` feature, reqwest 0.12 reports a response body cut off
  mid-read as a decode error whose source is the body error. Callers that
  checked `reqwest::Error::is_body` on the outer error should walk the source
  chain.

## 0.3.0 (unreleased)

All five public crates move to `0.3.0` together: `hypercall-ws-protocol`,
`hypercall-sdk-types`, `hypercall-client`, `hypercall-hyperliquid`, and
`hypercall-liquidator`. This is the first public release since `0.1.0`, so it
also carries the unreleased `0.2.0` changes below.

### Added: RFQ V2

RFQ V2 is live in production for every account from October 16, 2026, and the
SDK covers the whole taker and maker surface.

- `hypercall-client::rfq_v2_signing` (new): EIP-712 signing for every RFQ V2
  action. `HypercallWallet` gains `sign_create_rfq_v2`,
  `sign_rfq_v2_firm_quote`, `sign_rfq_v2_indicative_quote`,
  `sign_rfq_v2_firm_up`, `sign_rfq_v2_decline`, `sign_rfq_v2_execution`,
  `sign_rfq_v2_sweep`, `sign_rfq_v2_invalidation` (cancel an RFQ or withdraw a
  quote), `sign_rfq_v2_bulk_withdrawal`, `sign_rfq_v2_quote_cancel_schedule`,
  `sign_rfq_v2_replacement`, and `sign_rfq_v2_quote_replacement`. Each returns
  the wire request the matching `HypercallClient` method sends.
  - The `rfq_v2_*_message` builders and `rfq_v2_signing_hash` expose the exact
    EIP-712 messages and digests for remote signers.
  - `rfq_v2_terms_commitment`, `rfq_v2_instrument_id`, and
    `rfq_v2_reference_premium` compute what quotes sign and what the server
    checks.
  - `RfqV2ActionAuth::new` mints the UUIDv7 operation ID each lifecycle action
    needs.
  - The builders refuse what the server would refuse before signing: an
    unsorted selected-maker set, an execution mode and auto limit that
    disagree, an empty, repeated or oversized (over 1,024) sweep, an empty,
    nil, repeated or oversized (over 64) bulk withdrawal, and a zero cancel
    deadline.
  - A server-side test signs every action with the SDK and checks it through
    the server's own admission path, so an SDK and server EIP-712 mismatch
    fails in CI.
- `hypercall-client::rfq_v2` (HTTP transport for `/private/rfqs*`),
  `rfq_v2_recovery` (exact-retry reconciliation, change-feed cursor, private
  mirror, maker holds), signed read grants
  (`HypercallWallet::sign_rfq_v2_read_grant`), and the private `rfq_v2`
  WebSocket channel (`WsClient::connect_rfq_v2`, `subscribe_rfq_v2`).
- `hypercall-sdk-types::rfq_v2`: the RFQ V2 wire DTOs, plus the
  `AuthenticateRfqV2`, `RfqV2Authenticated`, and `RfqV2Changed` WebSocket
  messages.
- `hypercall-ws-protocol`: `WsChannel::RfqV2` (`"rfq_v2"`).
- Examples: `rfq_v2_taker` (create, select the best quote, execute, reconcile
  an ambiguous execution, cancel when nothing arrives) and `rfq_v2_maker`
  (dead-man switch, discovery, firm quotes, declines).

### Changed

- The `rfq-v2` feature is now on by default in `hypercall-client`,
  `hypercall-sdk-types`, and `hypercall-ws-protocol`. Builds with
  `default-features = false` must list `rfq-v2` to keep RFQ V2.
- With the feature on, `WsChannel` has an `RfqV2` variant and
  `WsChannel::ALL` has 16 entries, and `WsMessage` has the three RFQ V2
  variants. Exhaustive matches on either enum need an arm for them.
- `ClientError` has the RFQ V2 variants (`RfqV2Api`,
  `RfqV2SubmissionAmbiguous`, `RfqV2ChangeFeedCursor`,
  `RfqV2ProtocolViolation`) whenever `rfq-v2` is on.
- `hypercall-ws-protocol` jumps from `0.1.0` to `0.3.0` to keep one version
  across the crates.

RFQ V1 (`/rfq/*`, the `rfq` channel, and the quote-provider client) does not
change.

## 0.2.0 (not released separately; ships in 0.3.0)

`hypercall-sdk-types`, `hypercall-client`, `hypercall-hyperliquid`, and
`hypercall-liquidator` move to `0.2.0`. `hypercall-ws-protocol` stays at
`0.1.0`. `hypercall-hyperliquid` and `hypercall-liquidator` have no API changes
of their own; they move because they expose `hypercall-client` types.

### Added: TEE perp trading

- `hypercall-sdk-types::tee_perp`: wire types for `/v1/tee-perp/*` and
  `/v1/tee-api-wallet/*`. Covers order, trigger, and batch wires (`na`,
  `normalTpsl`, `positionTpsl`), leverage, every submit response, preview
  request and response, markets, and enrollment. Also adds
  `PerpRejectionReasonCode` (the server's 16 codes) and `TeePerpErrorCode`
  (the server's 29 codes). Codes added by a later server deserialize as
  `Unrecognized`.
- `hypercall-client::tee_perp`:
  - Orders: limit (GTC, IOC, ALO), market (bounded IOC), trigger (stop or
    take-profit, market or limit), bracket (`normalTpsl`), position TP/SL,
    scale, and raw order batches.
  - Cancels by oid and cloid, and leverage and margin-mode changes.
  - `preview_tee_perp_action` and `get_tee_perp_markets`.
  - TEE API wallet enrollment: `request_tee_api_wallet_approval`,
    `submit_tee_api_wallet_approval`, and `enroll_tee_api_wallet`.
  - `approve_hypercall_builder`, which calls
    `Account.approveZeroFeeBuilder(builder)` from the manager wallet. The
    builder address comes from `/exchange-info` `hl_builder_address`.
  - `HyperliquidInfoClient` for enrollment status (`userRole`) and leverage
    readback (`activeAssetData`).
  - Signers: `TeePerpSigner` and `TeeApiWalletManagerSigner` traits,
    implemented for `HypercallWallet`, plus public EIP-712 digest helpers.
  - Server-clock nonces: `server_now_ms` and `next_tee_perp_nonce`.
  - Price and size helpers that match the Hypercall order panel:
    `perp_market_limit_price`, `round_perp_price_to_tick`,
    `build_perp_scale_order_legs`, and `tee_perp_batch_cloid`.
- `HypercallWallet::sign_authentication_message` for manager-authenticated
  routes.

- Modify, batch modify and TWAP: `modify_tee_perp_order`,
  `batch_modify_tee_perp_orders` (Hyperliquid `batchModify`, default cap 50,
  typed per-child `TeePerpBatchModifyChildResponse` stages),
  `place_tee_perp_twap_order` and `cancel_tee_perp_twap`, with the
  `HCPerpOrderModify`, `HCPerpOrderBatchModify`, `HCPerpTwapOrder` and
  `HCPerpTwapCancel` digests. The SDK signs the same vectors the API and the
  frontend pin. Modify is limit-only and cloid-addressed.
- The `twap` preview kind (`PerpPreviewAction::Twap`).
- `TeePerpOrderStage::Unrecognized` for stages a later server adds, and
  `TeePerpOrderStage::is_definitive`.
- `ClientError::TeeSubmissionAmbiguous` and `resend_tee_submission`: a
  signed TEE submission that fails in transit hands back its exact signed
  body for reconciliation or an identical resend.
- `ClientError::api_error_code` and `ClientError::is_unsupported_action`, for
  actions a deployment's enclave release has not enabled (TWAP, batch modify).

Schedule-cancel and prepared cancels are not exposed.
