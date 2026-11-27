# Changelog

## 0.5.1 (2026-11-27)

Additive only: no existing public item, wire frame, request or response
changes (`cargo semver-checks` against 0.5.0 reports no semver update
required). `hypercall-client` 0.5.1 requires `hypercall-ws-protocol` 0.5.1,
because it sends the new delegated handshake frame.

Publish order: `hypercall-ws-protocol`, `hypercall-sdk-types`,
`hypercall-client`. Deploy the API server first: an older server refuses a
delegated handshake and answers `/approve-agent-with-expiry` with 404, so a
new client fails closed against it.

### Added: delegated quote-provider sessions (`hypercall-ws-protocol`, `hypercall-client`)

- `hypercall-ws-protocol` adds `ConnectDelegatedQuoteProvider`, a new
  `connect_delegated_quote_provider` frame. A short-lived key signs it for an
  already registered `provider` under a new EIP-712 type that binds the
  provider, `ConnectDelegatedQuoteProvider(address provider,address
  signer,uint256 timestamp,uint64 nonce)`. The key needs no registration of
  its own: it must hold RFQ authority for the provider's account (an active
  `Account.sol` API wallet, the signing manager, or an unexpired engine
  agent). Sessions, routing and indicatives stay keyed by the provider; firm
  quotes are signed by the key, exactly as a direct provider signs today.
- `hypercall_client::qp_client::run_qp_client_delegated` and
  `hypercall_client::qp_scoped::run_qp_scoped_client_delegated` open such a
  session. `run_qp_client` and `run_qp_scoped_client` are unchanged, and a
  direct handshake is byte-identical to 0.5.0.
- The server checks the key's authority at the handshake, on every firm
  quote and again at execution, and re-checks a live delegated session every
  2 s. A key that loses authority (API wallet removed, agent revoked or
  expired, manager changed) is disconnected with close code `4403` and its
  indicatives are evicted; a firm quote refused for signer authority
  disconnects at once. Delegated sessions require native RFQ V1 authority.

### Added: expiring agent approvals (`hypercall-sdk-types`, `hypercall-client`)

- New `POST /approve-agent-with-expiry` and
  `hypercall_sdk_types::requests::ApproveAgentWithExpiryRequest`, signed over
  a distinct `ApproveAgentWithExpiry(address agent,uint64 expiresAtMs,uint64
  nonce)` type so an ordinary `ApproveAgent` signature cannot be replayed
  with another lifetime. The expiry must be 1 minute to 90 days ahead.
  `POST /approve-agent` is unchanged and its approvals still never expire.
- `HypercallClient::approve_agent_with_expiry` and
  `HypercallClient::revoke_agent`. Expired agents still count toward the
  50-agent cap per wallet until revoked, so a client that rotates keys should
  revoke the previous one.

## 0.5.0 (unreleased)

`hypercall-ws-protocol`, `hypercall-sdk-types`, `hypercall-client`,
`hypercall-hyperliquid` and `hypercall-liquidator` all move to `0.5.0`
together. Each pins the others at `0.5.0`, so a build resolves one copy of
every Hypercall crate. `hypercall-covered-call-maker-sdk` stays at `0.1.0` and
depends on none of them.

Publish order: `hypercall-ws-protocol`, `hypercall-sdk-types`,
`hypercall-client`, `hypercall-hyperliquid`, `hypercall-liquidator`.

### Breaking: `hypercall-ws-protocol`

- [`QpRfqLeg`](https://docs.rs/hypercall-ws-protocol/0.5.0/hypercall_ws_protocol/qp/struct.QpRfqLeg.html) gains [`physical: Option<QpPhysicalLeg>`](https://docs.rs/hypercall-ws-protocol/0.5.0/hypercall_ws_protocol/qp/struct.QpPhysicalLeg.html). A quote provider's
  `rfq_request` now marks each leg on a physically settled call series with
  its `settles_as` base, `deliver_token`, `deliver_token_symbol` and margin
  `basis`. Cash legs omit the field, so cash RFQs keep their exact wire bytes,
  and the legs hash a quote signs is unchanged (`instrument|side|size`). Code
  that builds `QpRfqLeg` with a literal must set `physical`.

### Breaking: `hypercall-sdk-types`

- [`Portfolio`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/api_models/struct.Portfolio.html) gains `spot_available_usdc: Option<Decimal>`, the free USDC
  under the spot-buy cash check after financing and reservations. It is
  `None` for standard margin accounts and when the cash facts are
  unavailable; it is not an order approval.
- [`WsMessage::MarginUpdate`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/ws_protocol/enum.WsMessage.html#variant.MarginUpdate) gains `spot_available_usdc` with the same
  meaning, and its `margin_summary` field is now `Box<MarginSummary>`.
  Matches that bind `margin_summary` by value see a `Box`.
- [`InstrumentSpecResponse`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/responses/struct.InstrumentSpecResponse.html) gains `settles_as`, `deliver_token` and
  `deliver_token_symbol`, set only for physically settled series. All three
  default to `None` when absent from the JSON. `settlement_time_utc` is now
  derived from each instrument's own expiry, so one underlying can list
  different UTC times across daylight saving changes.
- Code that builds these structs or variants with literals must set the new
  fields.

### Breaking: RFQ V2 (`hypercall-sdk-types`, `rfq-v2` feature)

- [`RfqV2MakerTerms`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/rfq_v2/struct.RfqV2MakerTerms.html) no longer has
  `auto_execution_limit`. The taker's limit is private: a maker learns only
  whether execution is automatic. Quotes bind the parent through
  `terms_commitment`, so maker signing is unchanged. Code that reads or builds
  the field must drop it. The taker view (`RfqV2Terms`) keeps it.
- [`WsFillUpdate`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/ws_protocol/struct.WsFillUpdate.html) and
  [`FillApiResponse`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/api_models/struct.FillApiResponse.html) gain
  `rfq_id`, `quote_id` and `execution_id` (all `Option<Uuid>`). Literals must
  set them. They are omitted from JSON when absent, so order-book and v1 RFQ
  fills keep their wire bytes.
- [`RfqV2OperationResponse`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/rfq_v2/struct.RfqV2OperationResponse.html)
  gains `maker_generation`, `rfq_generation` and `results`, all optional and
  omitted when absent. Literals must set them.

### Added: RFQ V2 maker API

- **Fill attribution.** Every RFQ V2 leg fill, maker and taker side, carries
  the RFQ, firm quote and execution that produced it, on the `fills`
  WebSocket message and on `GET /fills`. `execution_id` matches the
  executions cursor below.
- **Executions cursor.**
  [`HypercallClient::list_rfq_v2_executions`](https://docs.rs/hypercall-client/0.5.0/hypercall_client/struct.HypercallClient.html#method.list_rfq_v2_executions)
  reads `GET /private/rfqs/executions`: committed executions where the signed
  account is maker or taker, ascending by a durable `sequence`. Save
  `next_after_sequence` and resume from it. Every execution appears exactly
  once in cursor order. Types: `ListRfqV2ExecutionsQuery`,
  `ListRfqV2ExecutionsResponse`, `RfqV2AccountExecution`,
  `RfqV2AccountExecutionLeg` and `RfqV2ExecutionRole`.
- **Provider read.**
  [`HypercallClient::get_rfq_v2_provider`](https://docs.rs/hypercall-client/0.5.0/hypercall_client/struct.HypercallClient.html#method.get_rfq_v2_provider)
  reads `GET /private/rfq-provider`, the signed account's own quote-provider
  enrollment: `RfqV2ProviderResponse` with `enrolled`, `provider`, `status`,
  `tier`, `allowed_underlyings` and `receives_open_rfqs`. It carries no
  notional caps.
- **Bulk withdrawal results.** A quote-ID bulk withdrawal is best effort: it
  withdraws every live quote, skips ones that can no longer execute, and
  reports each ID in `results`
  ([`RfqV2BulkWithdrawalTargetResult`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/rfq_v2/struct.RfqV2BulkWithdrawalTargetResult.html),
  status `withdrawn`, `already_inactive`, `not_found`, `not_permitted` or
  `unauthorized`). `rfq` and `all_open` withdrawals return `maker_generation`
  and `rfq_generation` read after the commit.
- [`InstrumentSpecResponse`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/responses/struct.InstrumentSpecResponse.html)
  gains `rfq: bool` (defaults to `false` when absent), the same trading-mode
  flag as `GET /instruments`.
- The full `rfq_v2` view (`RfqV2Rfq`) and the `rfq_v2_grant_expired` socket
  error are under "Added: RFQ V2 full maker view on `rfq_v2`" below.
- **`WsClient` full view.** `WsClient::subscribe_rfq_v2_with_view(RfqV2View::Full)`
  subscribes with the full view and keeps it across reconnects.
  `rfq_v2_recovery::rfq_v2_full_read` decodes an `RfqV2Rfq` frame from
  `WsClient` messages, as `rfq_v2_change_hint` does for hints.
- **`WsClient` grant renewal.** `WsClient::reauthenticate_rfq_v2` sends a
  fresh read grant for the same account on the live connection. The
  subscriptions stay, and later reconnects use the new grant.

### Changed: `WsClient` keeps an RFQ V2 session past grant expiry

- A `connect_rfq_v2` session no longer closes itself when its read grant's
  `expires_at_ms` passes. The server keeps the connection and sends one
  `Error` with `code` `rfq_v2_grant_expired`, which now reaches the caller
  like any other message. Answer it, or renew ahead of expiry, with
  `WsClient::reauthenticate_rfq_v2`. A reconnect while the current grant is
  expired still ends the session.

### Changed: RFQ V2 server rules the SDK surfaces

- Firm quote publication refuses a quote nonce outside the engine time window
  with `400 rfq_v2_quote_nonce_rejected`. Sign firm quotes with a current
  millisecond nonce, which the client's nonce providers already do.
- RFQ creation consumes the parent's nonce in its own nonce set and refuses a
  nonce outside the window, or an `expires_at_ms` more than 2 days after it,
  with `400 rfq_v2_nonce_out_of_window`. Counter-style nonces no longer work
  for RFQ creation.
- Read grants are accepted with `issued_at_ms` up to 30 s ahead of the
  server clock. Lifetime is capped from both the issue time and the server
  clock.
- Routing skips makers that share a principal with the taker, and firm quote
  publication refuses a self-trade with `rfq_v2_self_trade`.
  `selected_makers` routes to its eligible subset; an empty subset gets
  `rfq_v2_no_eligible_makers`.

### Added: TEE spot orders

- `hypercall-sdk-types`: new [`tee_spot`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/tee_spot/index.html) module with [`TeeSpotOrderAction`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/tee_spot/struct.TeeSpotOrderAction.html),
  [`TeeSpotCancelAction`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/tee_spot/struct.TeeSpotCancelAction.html), [`TeeSpotResponse`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/tee_spot/struct.TeeSpotResponse.html), [`TEE_SPOT_ASSET_OFFSET`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/tee_spot/constant.TEE_SPOT_ASSET_OFFSET.html) and the
  [`TEE_SPOT_ORDER_SUBMIT_ROUTE`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/tee_spot/constant.TEE_SPOT_ORDER_SUBMIT_ROUTE.html) / [`TEE_SPOT_CANCEL_SUBMIT_ROUTE`](https://docs.rs/hypercall-sdk-types/0.5.0/hypercall_sdk_types/tee_spot/constant.TEE_SPOT_CANCEL_SUBMIT_ROUTE.html) constants.
- `hypercall-client`: new [`tee_spot`](https://docs.rs/hypercall-client/0.5.0/hypercall_client/tee_spot/index.html) module. [`TeeSpotSigner`](https://docs.rs/hypercall-client/0.5.0/hypercall_client/tee_spot/trait.TeeSpotSigner.html),
  [`TeeSpotOrderParams`](https://docs.rs/hypercall-client/0.5.0/hypercall_client/tee_spot/struct.TeeSpotOrderParams.html), [`TeeSpotCancelParams`](https://docs.rs/hypercall-client/0.5.0/hypercall_client/tee_spot/struct.TeeSpotCancelParams.html), the EIP-712 payloads
  [`TeeSpotOrderSignature`](https://docs.rs/hypercall-client/0.5.0/hypercall_client/tee_spot/struct.TeeSpotOrderSignature.html) and [`TeeSpotCancelSignature`](https://docs.rs/hypercall-client/0.5.0/hypercall_client/tee_spot/struct.TeeSpotCancelSignature.html), and
  [`tee_spot_order_signing_hash`](https://docs.rs/hypercall-client/0.5.0/hypercall_client/tee_spot/fn.tee_spot_order_signing_hash.html) / [`tee_spot_cancel_signing_hash`](https://docs.rs/hypercall-client/0.5.0/hypercall_client/tee_spot/fn.tee_spot_cancel_signing_hash.html).
- `hypercall-hyperliquid`: [`PreparedHyperliquidAction::prepare_user_spot_order`](https://docs.rs/hypercall-hyperliquid/0.5.0/hypercall_hyperliquid/struct.PreparedHyperliquidAction.html#method.prepare_user_spot_order)
  prepares one exact user spot order for the TEE rail, carrying the Hypercall
  builder at zero fee like every TEE perp order, and
  [`PreparedHyperliquidAction::user_spot_order`](https://docs.rs/hypercall-hyperliquid/0.5.0/hypercall_hyperliquid/struct.PreparedHyperliquidAction.html#method.user_spot_order) reads it back.

### Added: managed vaults

- `hypercall-client`: new [`vaults`](https://docs.rs/hypercall-client/0.5.0/hypercall_client/vaults/index.html) module. `HypercallClient` can list, read,
  create, deposit to, withdraw from, close and cancel requests on managed
  vaults, and read holders, settlements, history and status changes. Types:
  [`Vault`](https://docs.rs/hypercall-client/0.5.0/hypercall_client/vaults/struct.Vault.html), `VaultConfigParams`, `VaultMetadata`, `VaultHolder`,
  `VaultQueuedRequest`, `VaultSettlement`, `VaultSettlementsPage`,
  `VaultFill`, `VaultHistoryPage`, `VaultStatusChange`,
  `VaultStatusChangesPage`, `VaultPage`, `CreateVaultResponse` and
  `VaultRequestResponse`.
- The paged reads (`get_vault_settlements`, `get_vault_history`,
  `get_vault_status_changes`) all take a `VaultPage`. For status changes,
  `before_seq` is the previous page's `next_before`.

### Changed: `hypercall-hyperliquid`

- Requires `hypercall-client` 0.5 for its optional `hypercall-client`
  feature. It moves to 0.5.0 so it never pulls a second client version.

### Added: version 2 standard-margin auction support in `hypercall-liquidator`

Standard-margin full liquidation auctions can publish `auction_version` 2: the
minimum bid starts at the account's positive equity and falls linearly to a
floor (75% over 30 minutes by default), and any bid at or above the minimum
settles. A version 2 terms hash leaves the bid out. `hypercall-liquidator` now
bids on version 2 auctions; version 1 hashes and bids are unchanged.

- The terms hash follows the published `auction_version`: version 1 includes
  the bid, version 2 leaves it out. Any other version is refused with
  `AuctionTermsError::UnsupportedVersion` instead of being hashed as a known
  one.
- The locally computed terms hash is compared with the published
  `current_auction_terms_hash`. A mismatch stops the cycle with
  `AuctionTermsError::TermsHashMismatch` before a bid is signed, so a future
  encoding change fails loudly instead of as a rejected bid.
- Before signing, the bid is checked against the published bid for its
  valuation timestamp: exact for version 1, at or above the minimum for
  version 2. The liquidator bids the published minimum from a fresh status
  read, so it follows the floor down as the auction ages and never bids below
  it.
- New public `auction` module: `AuctionBidRule`, `auction_terms_hash`,
  `verify_published_terms_hash`, `AuctionTermsError` and
  `SUPPORTED_AUCTION_VERSIONS`.

### Breaking: `hypercall-liquidator`

- `CycleError` gains the `AuctionTerms` variant. Exhaustive matches on
  `CycleError` need a new arm.

### Added: Portfolio Margin loan repayment

- `HypercallClient::repay_pm_loan` repays an account's PM financing from its
  HyperCore USDC through `POST /v1/pm-financing/repay`. The Account.sol
  manager signs the new `RepayPmLoan` EIP-712 type
  (`RepayPmLoan(address wallet,address account,string underlying,string amount,uint64 nonce)`,
  Hypercall domain). `RepayAmount::Usdc` takes an exact amount with at most
  six decimals and signs its normalized string; `RepayAmount::All` signs `all`.
- `HypercallWallet::sign_repay_pm_loan` signs the action for callers that
  build the request themselves.
- `HypercallClient::get_pm_financing` reads `GET /v1/pm-financing`: loans,
  unpaid settlement obligations, totals, margin, and the new optional
  `repayment` status (unreserved withdrawable USDC, the reason a request
  would be refused now, and the latest requests). `repayment` is `None` from
  servers that predate voluntary repayment.
- `PmRepaymentStatus` types each request's status (`is_terminal` for
  `Repaid`, `Canceled` and `EscalatedManual`), and `PmRepaymentBlockedReason`
  types `blocked_reason`. Both read unknown future values as `Unknown`.
- `PmRepayRejection::from_error` maps a failed repayment to its stable server
  code (`pm_voluntary_repayment_*`, `forbidden`, `invalid_nonce`,
  `signature_verification_failed`), and
  `PmRepaymentBlockedReason::rejection` names the rejection a blocked request
  would get.

### Added: RFQ V2 full maker view on `rfq_v2`

- `WsMessage::Subscribe` has a new optional `view: Option<RfqV2View>`
  (`hint`, the default, or `full`). Code that builds `Subscribe` with a struct
  literal must add `view: None`.
- `WsMessage::RfqV2Rfq(WsRfqV2Rfq)` (`rfq-v2` feature) carries a change's
  `sequence` and the authenticated account's `RfqV2Response`, exactly the
  `GET /private/rfqs/{rfq_id}` body. A `full` subscription receives it instead
  of `RfqV2Changed`, and still receives `RfqV2Changed` for any change whose
  read the server could not build.
- An `rfq_v2` subscriber whose read grant lapsed receives one `Error` with
  `code` `rfq_v2_grant_expired` and `channel` `rfq_v2` when a change for it is
  withheld. Send `AuthenticateRfqV2` with a fresh grant to resume; the
  subscription stays.

### Breaking: `hypercall-client` drops `get_username`

- `HypercallClient::get_username` and `UsernameLookupResponse` are removed.
  The Analytics API owns usernames: read them from its `GET /username`
  directly. The Hypercall API's `/username` route is a deprecated proxy to it.

## 0.4.0 (2026-10-21)

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
  on `webpki-roots` 1.0.10 directly, and `hypercall-client` requires reqwest
  0.12.16 or later, the first 0.12 release on `webpki-roots` 1.x. Cargo resolves one
  `webpki-roots` 1.x for the whole build, so the SDK's rustls HTTP client
  (reqwest `rustls-tls`) and hypersdk's WebSocket cannot trust the compromised
  root. Other dependencies in your build may still pull `webpki-roots` 0.25 or
  earlier, which keeps the root; this release does not change those.
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
  [`ClientError::Http(reqwest::Error)`](https://docs.rs/hypercall-client/0.4.0/hypercall_client/error/enum.ClientError.html#variant.Http)
  and
  [`HypercallClient::raw_post`](https://docs.rs/hypercall-client/0.4.0/hypercall_client/api/struct.HypercallClient.html#method.raw_post),
  which returns `reqwest::Response`. Code that names these types must depend
  on reqwest 0.12.
- With the `gzip` feature, reqwest 0.12 reports a response body cut off
  mid-read as a decode error whose source is the body error. Callers that
  checked
  [`reqwest::Error::is_body`](https://docs.rs/reqwest/0.12.16/reqwest/struct.Error.html#method.is_body)
  on the outer error should walk the source chain.
- `RfqInputError` gains `NetPremiumSubMicroPrecision`. RFQ quote building now
  rejects a `net_premium` with more than six decimal places instead of
  truncating it in the EIP-712 message, so the signed value always matches the
  premium sent on the wire. Exhaustive matches on `RfqInputError` need a new
  arm.
- `TeePerpModifyResponse` gains `venue_order`, and
  `TeePerpOrderBatchChildResponse` gains `message` and `venue_order`
  (`TeePerpModifyVenueOrder`). Code that builds these structs with literals
  must set the new fields.

### Changed

- TEE perp modify and batch modify return only after Hyperliquid answers. An
  accepted modify's `venue_order` carries the replacement's new `oid`.
- `TeePerpOrderStage::Partial` reports a batch modify whose children
  disagree. The whole-action stage is `unknown` only when some child is
  genuinely unknown. `venue_cancelled_original()` reports a modify whose
  replacement failed after Hyperliquid cancelled the original order.
- TEE perp order actions carry no Hypercall rate limit; Hyperliquid's
  address-based limit governs them (see the `hypercall-client` README).
- The `hypercall-client` README documents registering a perps app key as an
  on-chain API wallet with `update_api_wallet` before it signs TEE perp
  requests. An options agent key alone is rejected.

## 0.3.0

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
