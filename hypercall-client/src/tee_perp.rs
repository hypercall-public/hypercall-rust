//! TEE perp trading: Hyperliquid perp orders signed by the account's app key
//! (API wallet) and executed through Hypercall's TEE-held Hyperliquid API
//! wallet.
//!
//! # Flow
//!
//! 1. **Enroll the TEE API wallet once** (manager wallet):
//!    [`HypercallClient::enroll_tee_api_wallet`] requests the account's TEE
//!    wallet, signs the `HLAddApiWallet` approval in the `HypercallManagerSign`
//!    domain, submits it, and waits until Hyperliquid binds the wallet.
//! 2. **Approve the Hypercall builder once** (manager wallet, HyperEVM
//!    transaction): [`HypercallClient::approve_hypercall_builder`] calls
//!    `Account.approveZeroFeeBuilder(builder)` (Account 0.2.4 or newer). Orders
//!    are refused with [`TeePerpErrorCode::BuilderNotApproved`](crate::TeePerpErrorCode::BuilderNotApproved) until then.
//! 3. **Trade** with the app key (an active Account API wallet). Every action
//!    is EIP-712 signed in the `HypercallApiSign` domain.
//!
//! # Nonces
//!
//! The nonce is a Unix millisecond on the API server's clock. The server
//! derives the venue expiry as `nonce + 9000` and accepts it only when it falls
//! in `(server_now, server_now + 10000]`, so a nonce must lie in
//! `(server_now - 9000, server_now + 1000]`. Methods that take
//! `nonce: Option<u64>` stamp `None` from [`HypercallClient::next_tee_perp_nonce`],
//! which reads the server clock from the `/health` `Date` header (cached for a
//! minute) and keeps nonces strictly increasing per signer.
//!
//! # Outcomes
//!
//! A `200` response carries a [`TeePerpOrderStage`](crate::TeePerpOrderStage). `unknown` means the venue
//! may have received the action: reconcile before acting again, and never
//! resubmit the same intent through another route.
//!
//! A signed submission that fails in transit (transport error, timeout,
//! `5xx`, or an undecodable `2xx`) returns
//! [`ClientError::TeeSubmissionAmbiguous`] with the exact signed body. Resend
//! those bytes with [`HypercallClient::resend_tee_submission`] or reconcile;
//! calling the placing method again would sign a new nonce (and, for batches,
//! new nonce-derived cloids), which is a second order.
//!
//! # Example
//!
//! ```rust,no_run
//! use hypercall_client::{
//!     AccountAddress, ApproveHypercallBuilderParams, HypercallClient, HypercallWallet,
//!     HyperliquidInfoClient, PerpCancelByCloidParams, PerpLimitOrderParams, PerpPreviewAction,
//!     PerpPreviewRequest, PerpTimeInForce, Side, TeeApiWalletEnrollmentPolling, TeePerpBracketEntry,
//!     TeePerpBracketOrderParams, TeePerpExit, TeePerpLeverageParams, TeePerpMarketOrderParams,
//!     TeePerpTpsl, TeePerpTriggerExecution, TeePerpTriggerOrderParams, PERP_MARKET_SLIPPAGE_BPS,
//! };
//! use rust_decimal::Decimal;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = HypercallClient::new("https://api.hypercall.xyz");
//!     let manager = HypercallWallet::from_private_key("0xYOUR_MANAGER_KEY", 999)?;
//!     let app_key = HypercallWallet::from_private_key("0xYOUR_API_WALLET_KEY", 999)?;
//!     let account: AccountAddress = "0xYOUR_ACCOUNT_SOL_ADDRESS".parse()?;
//!     let hyperliquid = HyperliquidInfoClient::mainnet();
//!
//!     // One-time setup with the manager wallet.
//!     let enrollment = client
//!         .enroll_tee_api_wallet(
//!             &manager,
//!             account,
//!             &hyperliquid,
//!             TeeApiWalletEnrollmentPolling::default(),
//!         )
//!         .await?;
//!     println!("TEE API wallet: {}", enrollment.api_wallet);
//!     client
//!         .approve_hypercall_builder(
//!             &manager,
//!             ApproveHypercallBuilderParams {
//!                 rpc_url: "https://rpc.hyperliquid.xyz/evm".into(),
//!                 account,
//!                 builder: None, // read from /exchange-info hl_builder_address
//!             },
//!         )
//!         .await?;
//!
//!     // Market metadata and a read-only preview.
//!     let markets = client.get_tee_perp_markets().await?;
//!     let btc = markets
//!         .by_hyperliquid_coin("BTC")
//!         .ok_or("BTC is not configured")?;
//!     let preview = client
//!         .preview_tee_perp_action(&PerpPreviewRequest {
//!             account: account.into_wallet_address(),
//!             action: PerpPreviewAction::Leverage {
//!                 asset: btc.asset_id,
//!                 is_cross: true,
//!                 leverage: 5,
//!             },
//!         })
//!         .await?;
//!     println!(
//!         "{:?} {:?}, liquidation after {:?}",
//!         preview.decision, preview.reason_code, preview.liquidation_px_after
//!     );
//!
//!     // Leverage, then read the venue's setting back.
//!     client
//!         .update_tee_perp_leverage(
//!             &app_key,
//!             TeePerpLeverageParams {
//!                 account,
//!                 asset: btc.asset_id,
//!                 is_cross: true,
//!                 leverage: 5,
//!                 nonce: None,
//!             },
//!         )
//!         .await?;
//!     let venue = hyperliquid
//!         .active_asset_leverage(account, &btc.hyperliquid_coin)
//!         .await?;
//!     println!("venue leverage: {venue:?}");
//!
//!     // Limit (GTC, IOC, or ALO) and market (bounded IOC) orders.
//!     let limit = client
//!         .place_tee_perp_limit_order(
//!             &app_key,
//!             PerpLimitOrderParams {
//!                 account,
//!                 asset: btc.asset_id,
//!                 side: Side::Buy,
//!                 price: Decimal::new(95_000, 0),
//!                 size: Decimal::new(1, 3),
//!                 tif: PerpTimeInForce::Alo,
//!                 reduce_only: false,
//!                 client_order_id: Some(42),
//!                 nonce: None,
//!             },
//!         )
//!         .await?;
//!     if limit.requires_builder_approval() {
//!         return Err("approve the Hypercall builder first".into());
//!     }
//!     client
//!         .place_tee_perp_market_order(
//!             &app_key,
//!             TeePerpMarketOrderParams {
//!                 account,
//!                 asset: btc.asset_id,
//!                 side: Side::Buy,
//!                 size: Decimal::new(1, 3),
//!                 reference_price: Decimal::new(100_000, 0), // best ask
//!                 size_decimals: 5,
//!                 slippage_bps: Some(50),
//!                 reduce_only: false,
//!                 client_order_id: None,
//!                 nonce: None,
//!             },
//!         )
//!         .await?;
//!
//!     // A stop-market order, and an entry with attached TP/SL.
//!     client
//!         .place_tee_perp_trigger_order(
//!             &app_key,
//!             TeePerpTriggerOrderParams {
//!                 account,
//!                 asset: btc.asset_id,
//!                 side: Side::Sell,
//!                 size: Decimal::new(1, 3),
//!                 trigger_price: Decimal::new(90_000, 0),
//!                 tpsl: TeePerpTpsl::Sl,
//!                 execution: TeePerpTriggerExecution::Market {
//!                     size_decimals: 5,
//!                     slippage_bps: None,
//!                 },
//!                 reduce_only: true,
//!                 client_order_id: None,
//!                 nonce: None,
//!             },
//!         )
//!         .await?;
//!     let exit = |trigger: i64| {
//!         TeePerpExit::market(
//!             Decimal::new(trigger, 0),
//!             Side::Sell,
//!             5,
//!             PERP_MARKET_SLIPPAGE_BPS,
//!         )
//!     };
//!     let bracket = client
//!         .place_tee_perp_bracket_order(
//!             &app_key,
//!             TeePerpBracketOrderParams {
//!                 account,
//!                 entry: TeePerpBracketEntry {
//!                     asset: btc.asset_id,
//!                     side: Side::Buy,
//!                     price: Decimal::new(99_000, 0),
//!                     size: Decimal::new(1, 3),
//!                     tif: PerpTimeInForce::Gtc,
//!                     reduce_only: false,
//!                 },
//!                 take_profit: Some(exit(110_000)?),
//!                 stop_loss: Some(exit(92_000)?),
//!                 nonce: None,
//!             },
//!         )
//!         .await?;
//!     println!("bracket {:?}: {} children", bracket.stage, bracket.children.len());
//!
//!     client
//!         .cancel_tee_perp_order_by_cloid(
//!             &app_key,
//!             PerpCancelByCloidParams {
//!                 account,
//!                 asset: btc.asset_id,
//!                 client_order_id: 42,
//!                 nonce: None,
//!             },
//!         )
//!         .await?;
//!     Ok(())
//! }
//! ```
//!
//! # Modify, batch modify and TWAP
//!
//! [`HypercallClient::modify_tee_perp_order`] and
//! [`HypercallClient::batch_modify_tee_perp_orders`] replace resting limit
//! orders; each batch child reports its own [`TeePerpOrderStage`](crate::TeePerpOrderStage) in
//! [`TeePerpBatchModifyResponse::children`].
//! [`HypercallClient::place_tee_perp_twap_order`] and
//! [`HypercallClient::cancel_tee_perp_twap`] drive native TWAPs. TWAP and
//! batch modify are enabled per enclave release; a deployment without them
//! answers `400 unsupported_action` ([`ClientError::is_unsupported_action`]).
//!
//! Hyperliquid parity. TWAP submit and cancel carry every `twapOrder` and
//! `twapCancel` field (`a`, `b`, `s`, `r`, `m`, `t`; `a`, `t`). Modify and batch
//! modify have these scope cuts:
//!
//! - Modify replaces limit orders only (GTC, IOC, ALO). Hyperliquid's
//!   `modify` can also turn an order into a trigger order; the TEE route does not
//!   accept that, so a resting trigger order must be cancelled and re-placed.
//! - The order is addressed by its cloid plus its engine `directiveId`, never by
//!   `oid`, and the modify keeps the cloid. Hyperliquid also accepts an `oid`
//!   target and a new cloid.
//! - A batch modify carries at most the instance cap (50 by default, 100 at
//!   most), where Hyperliquid documents no per-call limit.
//! - No builder field: Hyperliquid documents none on `modify`, `batchModify`,
//!   `twapOrder` or `twapCancel`.
//!
//! ```rust,no_run
//! use hypercall_client::{
//!     AccountAddress, HypercallClient, HypercallWallet, PerpTimeInForce, Side,
//!     TeePerpBatchModifyParams, TeePerpModifyParams, TeePerpOrderReplacement, TeePerpOrderStage,
//!     TeePerpTwapCancelParams, TeePerpTwapParams,
//! };
//! use rust_decimal::Decimal;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = HypercallClient::new("https://api.hypercall.xyz");
//!     let app_key = HypercallWallet::from_private_key("0xYOUR_API_WALLET_KEY", 999)?;
//!     let account: AccountAddress = "0xYOUR_ACCOUNT_SOL_ADDRESS".parse()?;
//!
//!     // `directiveId` and cloid returned when each order was placed.
//!     let bid = TeePerpOrderReplacement {
//!         asset: 0,
//!         target_directive_id: "01999b5a-7c1e-7000-8000-000000000042".parse()?,
//!         cloid: 42,
//!         side: Side::Buy,
//!         price: Decimal::new(99_500, 0),
//!         size: Decimal::new(2, 3),
//!         tif: PerpTimeInForce::Alo,
//!         reduce_only: false,
//!     };
//!     let ask = TeePerpOrderReplacement {
//!         target_directive_id: "01999b5a-7c1e-7000-8000-000000000043".parse()?,
//!         cloid: 43,
//!         side: Side::Sell,
//!         price: Decimal::new(100_500, 0),
//!         ..bid
//!     };
//!
//!     // One order.
//!     let single = client
//!         .modify_tee_perp_order(
//!             &app_key,
//!             TeePerpModifyParams {
//!                 account,
//!                 replacement: bid,
//!                 nonce: None,
//!             },
//!         )
//!         .await?;
//!     println!("modify {:?}", single.stage);
//!
//!     // Both quotes in one signed batchModify; each child settles on its own.
//!     let batch = client
//!         .batch_modify_tee_perp_orders(
//!             &app_key,
//!             TeePerpBatchModifyParams {
//!                 account,
//!                 replacements: vec![bid, ask],
//!                 max_children: None, // the server default, 50
//!                 nonce: None,
//!             },
//!         )
//!         .await?;
//!     for child in &batch.children {
//!         match child.stage {
//!             TeePerpOrderStage::Accepted => println!("{} moved", child.cloid),
//!             TeePerpOrderStage::Rejected | TeePerpOrderStage::NotSubmitted => {
//!                 println!("{} unchanged: the old order still rests", child.cloid)
//!             }
//!             _ => println!("{} unknown: reconcile before touching it", child.cloid),
//!         }
//!     }
//!
//!     // A native TWAP. Deployments that have not enabled it answer
//!     // `400 unsupported_action`.
//!     match client
//!         .place_tee_perp_twap_order(
//!             &app_key,
//!             TeePerpTwapParams {
//!                 account,
//!                 asset: 0,
//!                 side: Side::Sell,
//!                 size: Decimal::new(5, 2),
//!                 reduce_only: true,
//!                 minutes: 30,
//!                 randomize: true,
//!                 nonce: None,
//!             },
//!         )
//!         .await
//!     {
//!         Ok(twap) => {
//!             if let Some(twap_id) = twap.twap_id {
//!                 client
//!                     .cancel_tee_perp_twap(
//!                         &app_key,
//!                         TeePerpTwapCancelParams {
//!                             account,
//!                             asset: 0,
//!                             twap_id,
//!                             nonce: None,
//!                         },
//!                     )
//!                     .await?;
//!             }
//!         }
//!         Err(error) if error.is_unsupported_action() => println!("TWAP is not enabled here"),
//!         Err(error) => return Err(error.into()),
//!     }
//!     Ok(())
//! }
//! ```

#![allow(
    clippy::result_large_err,
    reason = "these helpers preserve the established unboxed public ClientError contract"
)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, B256, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol;
use alloy::sol_types::{Eip712Domain, SolStruct};
use hypercall_sdk_types::tee_perp::{
    TEE_API_WALLET_APPROVAL_REQUEST_ROUTE, TEE_API_WALLET_APPROVAL_ROUTE,
    TEE_PERP_CANCEL_BY_OID_SUBMIT_ROUTE, TEE_PERP_CANCEL_SUBMIT_ROUTE,
    TEE_PERP_LEVERAGE_SUBMIT_ROUTE, TEE_PERP_MARKETS_ROUTE, TEE_PERP_ORDER_BATCH_MODIFY_ROUTE,
    TEE_PERP_ORDER_BATCH_SUBMIT_ROUTE, TEE_PERP_ORDER_MODIFY_ROUTE, TEE_PERP_ORDER_SUBMIT_ROUTE,
    TEE_PERP_PREVIEW_ROUTE, TEE_PERP_TRIGGER_ORDER_SUBMIT_ROUTE, TEE_PERP_TWAP_CANCEL_ROUTE,
    TEE_PERP_TWAP_SUBMIT_ROUTE,
};
use hypercall_sdk_types::{
    ApiResponse, DirectiveDeliveryStatus, DirectiveDomainStatus, DirectiveStatusResponse,
    DirectiveSubmitRequest, DirectiveSubmitResponse, ExchangeInfoResponse, HlCancelByCloidAction,
    HlCancelByOidAction, HlLimitOrderAction, PerpPreviewAction, PerpPreviewLimit, PerpPreviewOrder,
    PerpPreviewOrderType, PerpPreviewRequest, PerpPreviewResponse, PerpPreviewTrigger,
    PerpTimeInForce, Side, TeeApiWalletApprovalPackage, TeeApiWalletApprovalRequest,
    TeeApiWalletApprovalSubmitRequest, TeePerpCancelResponse, TeePerpLeverageAction,
    TeePerpLeverageResponse, TeePerpLimitOrderType, TeePerpMarketsResponse,
    TeePerpOrderBatchAction, TeePerpOrderBatchResponse, TeePerpOrderGrouping, TeePerpOrderResponse,
    TeePerpOrderType, TeePerpOrderWire, TeePerpTpsl, TeePerpTriggerOrderType,
    TeePerpVenueActionResponse, WalletAddress, MAX_TEE_PERP_ORDER_BATCH_CHILDREN,
    TEE_PERP_VENUE_TTL_MS,
};
use hypercall_sdk_types::{
    TeePerpBatchModifyAction, TeePerpBatchModifyResponse, TeePerpModifyAction,
    TeePerpModifyResponse, TeePerpTwapAction, TeePerpTwapCancelAction, TeePerpTwapResponse,
    DEFAULT_TEE_PERP_BATCH_MODIFY_MAX_CHILDREN, MAX_TEE_PERP_BATCH_MODIFY_CHILDREN,
    TEE_PERP_TWAP_MAX_MINUTES, TEE_PERP_TWAP_MIN_MINUTES,
};
use reqwest::Method;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::api::HypercallClient;
use crate::error::{ClientError, Result};
use crate::perp::{
    to_perp_units, PerpCancelByCloidParams, PerpCancelByOidParams, PerpLimitOrderParams,
};
use crate::wallet::{
    evm_submission_lock, AccountAddress, HypercallSigner, HypercallWallet,
    PerpCancelByCloidSignature, PerpCancelByOidSignature, PerpDirectiveSigner,
    PerpLimitOrderSignature,
};

/// Default market-order slippage bound used by the Hypercall order panel:
/// an IOC limit 2000 bps (20%) through the reference price, rounded to a
/// valid tick inside that bound. Pass a tighter bound where it matters.
pub const PERP_MARKET_SLIPPAGE_BPS: u32 = 2_000;
/// Hyperliquid perp prices carry at most five significant figures.
pub const HYPERLIQUID_PERP_MAX_PRICE_SIGNIFICANT_FIGURES: u32 = 5;
/// Hyperliquid perp prices carry at most `6 - szDecimals` decimals.
pub const HYPERLIQUID_PERP_MAX_PRICE_DECIMALS: u32 = 6;
/// Scale orders split into this many legs at least.
pub const PERP_SCALE_MIN_ORDERS: usize = 2;
/// Scale orders split into this many legs at most.
pub const PERP_SCALE_MAX_ORDERS: usize = 100;
/// Hyperliquid info endpoint on mainnet.
pub const HYPERLIQUID_MAINNET_INFO_URL: &str = "https://api.hyperliquid.xyz/info";
/// Hyperliquid info endpoint on testnet.
pub const HYPERLIQUID_TESTNET_INFO_URL: &str = "https://api.hyperliquid-testnet.xyz/info";

const BPS_DENOMINATOR: u32 = 10_000;
const SERVER_CLOCK_TTL: Duration = Duration::from_secs(60);
const SCALE_WORKING_SCALE: u32 = 12;
const MAX_BATCH_CLOID_INDEX: usize = 255;
const HYPERCALL_API_DOMAIN: &str = "HypercallApiSign";
const HYPERCALL_MANAGER_DOMAIN: &str = "HypercallManagerSign";

sol! {
    /// One signed order. A limit child signs its TIF byte with
    /// `triggerPx = 0`, `isMarket = false`, `tpsl = 0`. A trigger child signs
    /// `encodedTif = 0`, its trigger price, and `tpsl` 1 (TP) or 2 (SL).
    #[derive(Debug, PartialEq, Eq)]
    struct PerpOrderTerms {
        uint32 asset;
        bool isBuy;
        uint64 limitPx;
        uint64 sz;
        bool reduceOnly;
        uint8 encodedTif;
        uint64 triggerPx;
        bool isMarket;
        uint8 tpsl;
        uint128 cloid;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct HCPerpTriggerOrder {
        address account;
        uint64 nonce;
        PerpOrderTerms order;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct HCPerpOrderBatch {
        address account;
        uint64 nonce;
        uint8 grouping;
        PerpOrderTerms[] orders;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct PerpLeverageChange {
        uint32 asset;
        bool isCross;
        uint32 leverage;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct HCPerpLeverageChange {
        address account;
        uint64 nonce;
        PerpLeverageChange action;
    }

    /// Replacement terms of one modify. `targetDirectiveId` is the UUID as a
    /// big-endian u128. Field names and order match the server's
    /// `PerpModifyTerms` exactly; the digest depends on both.
    #[derive(Debug, PartialEq, Eq)]
    struct PerpModifyTerms {
        uint32 asset;
        uint128 targetDirectiveId;
        bool isBuy;
        uint64 limitPx;
        uint64 sz;
        bool reduceOnly;
        uint8 encodedTif;
        uint128 cloid;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct HCPerpOrderModify {
        address account;
        uint64 nonce;
        PerpModifyTerms action;
    }

    /// Every child's replacement terms, in request order.
    #[derive(Debug, PartialEq, Eq)]
    struct HCPerpOrderBatchModify {
        address account;
        uint64 nonce;
        PerpModifyTerms[] modifies;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct PerpTwapTerms {
        uint32 asset;
        bool isBuy;
        uint64 sz;
        bool reduceOnly;
        uint32 minutes;
        bool randomize;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct HCPerpTwapOrder {
        address account;
        uint64 nonce;
        PerpTwapTerms action;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct PerpTwapCancelTerms {
        uint32 asset;
        uint64 twapId;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct HCPerpTwapCancel {
        address account;
        uint64 nonce;
        PerpTwapCancelTerms action;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct AddApiWallet {
        address apiWalletAddress;
        string apiWalletName;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct HLAddApiWallet {
        address account;
        uint64 nonce;
        AddApiWallet action;
    }

    #[sol(rpc)]
    interface HypercallAccountBuilderApproval {
        function approveZeroFeeBuilder(address builder) external;
    }
}

// ---------------------------------------------------------------------------
// Signing
// ---------------------------------------------------------------------------

/// App-key payload for `POST /v1/tee-perp/orders/submit-trigger`.
#[derive(Debug, Clone, Copy)]
pub struct TeePerpTriggerOrderSignature {
    pub account: AccountAddress,
    pub nonce: u64,
    pub order: TeePerpOrderWire,
}

/// App-key payload for `POST /v1/tee-perp/orders/submit-batch`.
#[derive(Debug, Clone, Copy)]
pub struct TeePerpOrderBatchSignature<'a> {
    pub account: AccountAddress,
    pub nonce: u64,
    pub batch: &'a TeePerpOrderBatchAction,
}

/// App-key payload for `POST /v1/tee-perp/leverage/submit`.
#[derive(Debug, Clone, Copy)]
pub struct TeePerpLeverageSignature {
    pub account: AccountAddress,
    pub nonce: u64,
    pub action: TeePerpLeverageAction,
}

/// App-key payload for `POST /v1/tee-perp/orders/modify` (`HCPerpOrderModify`).
#[derive(Debug, Clone, Copy)]
pub struct TeePerpModifySignature {
    pub account: AccountAddress,
    pub nonce: u64,
    pub action: TeePerpModifyAction,
}

/// App-key payload for `POST /v1/tee-perp/orders/batch-modify`
/// (`HCPerpOrderBatchModify`).
#[derive(Debug, Clone, Copy)]
pub struct TeePerpBatchModifySignature<'a> {
    pub account: AccountAddress,
    pub nonce: u64,
    pub batch: &'a TeePerpBatchModifyAction,
}

/// App-key payload for `POST /v1/tee-perp/twap/submit` (`HCPerpTwapOrder`).
#[derive(Debug, Clone, Copy)]
pub struct TeePerpTwapSignature {
    pub account: AccountAddress,
    pub nonce: u64,
    pub action: TeePerpTwapAction,
}

/// App-key payload for `POST /v1/tee-perp/twap/cancel` (`HCPerpTwapCancel`).
#[derive(Debug, Clone, Copy)]
pub struct TeePerpTwapCancelSignature {
    pub account: AccountAddress,
    pub nonce: u64,
    pub action: TeePerpTwapCancelAction,
}

/// Manager payload approving the account's TEE API wallet on Hyperliquid.
#[derive(Debug, Clone, Copy)]
pub struct TeeApiWalletApprovalSignature<'a> {
    pub account: AccountAddress,
    pub nonce: u64,
    pub api_wallet_address: AccountAddress,
    pub api_wallet_name: &'a str,
}

/// App-key signer for TEE perp routes. Plain limit orders and cancels reuse
/// the [`PerpDirectiveSigner`] schemas (`HLOrder`, `HLCancel`,
/// `HLCancelByCloid`); triggers, batches, leverage, modifies and TWAPs use
/// these.
#[expect(
    async_fn_in_trait,
    reason = "signers are used generically, never as Send-bound trait objects"
)]
pub trait TeePerpSigner: PerpDirectiveSigner {
    async fn sign_tee_perp_trigger_order_payload(
        &self,
        payload: TeePerpTriggerOrderSignature,
    ) -> Result<String>;
    async fn sign_tee_perp_order_batch_payload(
        &self,
        payload: TeePerpOrderBatchSignature<'_>,
    ) -> Result<String>;
    async fn sign_tee_perp_leverage_payload(
        &self,
        payload: TeePerpLeverageSignature,
    ) -> Result<String>;
    async fn sign_tee_perp_modify_payload(&self, payload: TeePerpModifySignature)
        -> Result<String>;
    async fn sign_tee_perp_batch_modify_payload(
        &self,
        payload: TeePerpBatchModifySignature<'_>,
    ) -> Result<String>;
    async fn sign_tee_perp_twap_payload(&self, payload: TeePerpTwapSignature) -> Result<String>;
    async fn sign_tee_perp_twap_cancel_payload(
        &self,
        payload: TeePerpTwapCancelSignature,
    ) -> Result<String>;
}

/// Manager signer for TEE API wallet enrollment.
#[expect(
    async_fn_in_trait,
    reason = "signers are used generically, never as Send-bound trait objects"
)]
pub trait TeeApiWalletManagerSigner: HypercallSigner {
    /// EIP-191 signature over Hypercall's timestamped authentication message.
    async fn sign_authentication_payload(&self, timestamp: i64) -> Result<String>;
    async fn sign_tee_api_wallet_approval_payload(
        &self,
        payload: TeeApiWalletApprovalSignature<'_>,
    ) -> Result<String>;
}

fn perp_order_terms(order: &TeePerpOrderWire) -> PerpOrderTerms {
    let (encoded_tif, trigger_px, is_market, tpsl) = match order.order_type {
        TeePerpOrderType::Limit(limit) => (limit.tif.encoded(), 0, false, 0),
        TeePerpOrderType::Trigger(trigger) => (
            0,
            trigger.trigger_px,
            trigger.is_market,
            trigger.tpsl.signed(),
        ),
    };
    PerpOrderTerms {
        asset: order.asset,
        isBuy: order.is_buy,
        limitPx: order.limit_px,
        sz: order.sz,
        reduceOnly: order.reduce_only,
        encodedTif: encoded_tif,
        triggerPx: trigger_px,
        isMarket: is_market,
        tpsl,
        cloid: order.cloid,
    }
}

fn trigger_order_message(payload: &TeePerpTriggerOrderSignature) -> Result<HCPerpTriggerOrder> {
    if !matches!(payload.order.order_type, TeePerpOrderType::Trigger(_)) {
        return Err(ClientError::InvalidInput(
            "a TEE trigger submission needs a trigger order".into(),
        ));
    }
    Ok(HCPerpTriggerOrder {
        account: payload.account.as_wallet_address().inner(),
        nonce: payload.nonce,
        order: perp_order_terms(&payload.order),
    })
}

fn order_batch_message(payload: &TeePerpOrderBatchSignature<'_>) -> Result<HCPerpOrderBatch> {
    validate_batch(payload.batch)?;
    Ok(HCPerpOrderBatch {
        account: payload.account.as_wallet_address().inner(),
        nonce: payload.nonce,
        grouping: payload.batch.grouping.signed(),
        orders: payload.batch.orders.iter().map(perp_order_terms).collect(),
    })
}

fn leverage_message(payload: &TeePerpLeverageSignature) -> HCPerpLeverageChange {
    HCPerpLeverageChange {
        account: payload.account.as_wallet_address().inner(),
        nonce: payload.nonce,
        action: PerpLeverageChange {
            asset: payload.action.asset,
            isCross: payload.action.is_cross,
            leverage: payload.action.leverage,
        },
    }
}

fn perp_modify_terms(action: &TeePerpModifyAction) -> PerpModifyTerms {
    PerpModifyTerms {
        asset: action.asset,
        targetDirectiveId: action.target_directive_id.as_u128(),
        isBuy: action.is_buy,
        limitPx: action.limit_px,
        sz: action.sz,
        reduceOnly: action.reduce_only,
        encodedTif: action.tif.encoded(),
        cloid: action.cloid,
    }
}

fn modify_message(payload: &TeePerpModifySignature) -> Result<HCPerpOrderModify> {
    validate_modify_action(&payload.action)?;
    Ok(HCPerpOrderModify {
        account: payload.account.as_wallet_address().inner(),
        nonce: payload.nonce,
        action: perp_modify_terms(&payload.action),
    })
}

fn batch_modify_message(
    payload: &TeePerpBatchModifySignature<'_>,
) -> Result<HCPerpOrderBatchModify> {
    // The signer enforces only the wire bound; the instance cap is the
    // request builder's and the server's to enforce.
    validate_batch_modify(payload.batch, MAX_TEE_PERP_BATCH_MODIFY_CHILDREN)?;
    Ok(HCPerpOrderBatchModify {
        account: payload.account.as_wallet_address().inner(),
        nonce: payload.nonce,
        modifies: payload
            .batch
            .modifies
            .iter()
            .map(perp_modify_terms)
            .collect(),
    })
}

fn twap_message(payload: &TeePerpTwapSignature) -> Result<HCPerpTwapOrder> {
    validate_twap_action(&payload.action)?;
    let action = payload.action;
    Ok(HCPerpTwapOrder {
        account: payload.account.as_wallet_address().inner(),
        nonce: payload.nonce,
        action: PerpTwapTerms {
            asset: action.asset,
            isBuy: action.is_buy,
            sz: action.sz,
            reduceOnly: action.reduce_only,
            minutes: action.minutes,
            randomize: action.randomize,
        },
    })
}

fn twap_cancel_message(payload: &TeePerpTwapCancelSignature) -> Result<HCPerpTwapCancel> {
    if payload.action.twap_id == 0 {
        return Err(ClientError::InvalidInput("twap_id must not be zero".into()));
    }
    Ok(HCPerpTwapCancel {
        account: payload.account.as_wallet_address().inner(),
        nonce: payload.nonce,
        action: PerpTwapCancelTerms {
            asset: payload.action.asset,
            twapId: payload.action.twap_id,
        },
    })
}

fn approval_message(payload: &TeeApiWalletApprovalSignature<'_>) -> HLAddApiWallet {
    HLAddApiWallet {
        account: payload.account.as_wallet_address().inner(),
        nonce: payload.nonce,
        action: AddApiWallet {
            apiWalletAddress: payload.api_wallet_address.as_wallet_address().inner(),
            apiWalletName: payload.api_wallet_name.to_string(),
        },
    }
}

fn signing_domain(name: &'static str, chain_id: u64) -> Result<Eip712Domain> {
    if !matches!(chain_id, 998 | 999) {
        return Err(ClientError::InvalidInput(format!(
            "unsupported directive chain id: {chain_id}"
        )));
    }
    Ok(Eip712Domain {
        name: Some(name.into()),
        version: Some("1".into()),
        chain_id: Some(U256::from(chain_id)),
        verifying_contract: Some(Address::ZERO),
        salt: None,
    })
}

/// EIP-712 digest the app key signs for a TEE trigger order.
pub fn tee_perp_trigger_order_signing_hash(
    chain_id: u64,
    payload: &TeePerpTriggerOrderSignature,
) -> Result<B256> {
    Ok(trigger_order_message(payload)?
        .eip712_signing_hash(&signing_domain(HYPERCALL_API_DOMAIN, chain_id)?))
}

/// EIP-712 digest the app key signs for a TEE order batch.
pub fn tee_perp_order_batch_signing_hash(
    chain_id: u64,
    payload: &TeePerpOrderBatchSignature<'_>,
) -> Result<B256> {
    Ok(order_batch_message(payload)?
        .eip712_signing_hash(&signing_domain(HYPERCALL_API_DOMAIN, chain_id)?))
}

/// EIP-712 digest the app key signs for a TEE leverage change.
pub fn tee_perp_leverage_signing_hash(
    chain_id: u64,
    payload: &TeePerpLeverageSignature,
) -> Result<B256> {
    Ok(leverage_message(payload)
        .eip712_signing_hash(&signing_domain(HYPERCALL_API_DOMAIN, chain_id)?))
}

/// EIP-712 digest the app key signs for a TEE single modify.
pub fn tee_perp_modify_signing_hash(
    chain_id: u64,
    payload: &TeePerpModifySignature,
) -> Result<B256> {
    Ok(modify_message(payload)?
        .eip712_signing_hash(&signing_domain(HYPERCALL_API_DOMAIN, chain_id)?))
}

/// EIP-712 digest the app key signs for a TEE batch modify. Child order is
/// signed.
pub fn tee_perp_batch_modify_signing_hash(
    chain_id: u64,
    payload: &TeePerpBatchModifySignature<'_>,
) -> Result<B256> {
    Ok(batch_modify_message(payload)?
        .eip712_signing_hash(&signing_domain(HYPERCALL_API_DOMAIN, chain_id)?))
}

/// EIP-712 digest the app key signs for a TEE TWAP.
pub fn tee_perp_twap_signing_hash(chain_id: u64, payload: &TeePerpTwapSignature) -> Result<B256> {
    Ok(
        twap_message(payload)?
            .eip712_signing_hash(&signing_domain(HYPERCALL_API_DOMAIN, chain_id)?),
    )
}

/// EIP-712 digest the app key signs for a TEE TWAP cancel.
pub fn tee_perp_twap_cancel_signing_hash(
    chain_id: u64,
    payload: &TeePerpTwapCancelSignature,
) -> Result<B256> {
    Ok(twap_cancel_message(payload)?
        .eip712_signing_hash(&signing_domain(HYPERCALL_API_DOMAIN, chain_id)?))
}

/// EIP-712 digest the manager signs to approve the TEE API wallet.
pub fn tee_api_wallet_approval_signing_hash(
    chain_id: u64,
    payload: &TeeApiWalletApprovalSignature<'_>,
) -> Result<B256> {
    Ok(approval_message(payload)
        .eip712_signing_hash(&signing_domain(HYPERCALL_MANAGER_DOMAIN, chain_id)?))
}

impl TeePerpSigner for HypercallWallet {
    async fn sign_tee_perp_trigger_order_payload(
        &self,
        payload: TeePerpTriggerOrderSignature,
    ) -> Result<String> {
        let message = trigger_order_message(&payload)?;
        self.sign_directive(&message, "HCPerpTriggerOrder", HYPERCALL_API_DOMAIN)
            .await
    }

    async fn sign_tee_perp_order_batch_payload(
        &self,
        payload: TeePerpOrderBatchSignature<'_>,
    ) -> Result<String> {
        let message = order_batch_message(&payload)?;
        self.sign_directive(&message, "HCPerpOrderBatch", HYPERCALL_API_DOMAIN)
            .await
    }

    async fn sign_tee_perp_leverage_payload(
        &self,
        payload: TeePerpLeverageSignature,
    ) -> Result<String> {
        let message = leverage_message(&payload);
        self.sign_directive(&message, "HCPerpLeverageChange", HYPERCALL_API_DOMAIN)
            .await
    }

    async fn sign_tee_perp_modify_payload(
        &self,
        payload: TeePerpModifySignature,
    ) -> Result<String> {
        let message = modify_message(&payload)?;
        self.sign_directive(&message, "HCPerpOrderModify", HYPERCALL_API_DOMAIN)
            .await
    }

    async fn sign_tee_perp_batch_modify_payload(
        &self,
        payload: TeePerpBatchModifySignature<'_>,
    ) -> Result<String> {
        let message = batch_modify_message(&payload)?;
        self.sign_directive(&message, "HCPerpOrderBatchModify", HYPERCALL_API_DOMAIN)
            .await
    }

    async fn sign_tee_perp_twap_payload(&self, payload: TeePerpTwapSignature) -> Result<String> {
        let message = twap_message(&payload)?;
        self.sign_directive(&message, "HCPerpTwapOrder", HYPERCALL_API_DOMAIN)
            .await
    }

    async fn sign_tee_perp_twap_cancel_payload(
        &self,
        payload: TeePerpTwapCancelSignature,
    ) -> Result<String> {
        let message = twap_cancel_message(&payload)?;
        self.sign_directive(&message, "HCPerpTwapCancel", HYPERCALL_API_DOMAIN)
            .await
    }
}

impl TeeApiWalletManagerSigner for HypercallWallet {
    async fn sign_authentication_payload(&self, timestamp: i64) -> Result<String> {
        self.sign_authentication_message(timestamp).await
    }

    async fn sign_tee_api_wallet_approval_payload(
        &self,
        payload: TeeApiWalletApprovalSignature<'_>,
    ) -> Result<String> {
        let message = approval_message(&payload);
        self.sign_directive(&message, "HLAddApiWallet", HYPERCALL_MANAGER_DOMAIN)
            .await
    }
}

// ---------------------------------------------------------------------------
// Server clock and nonces
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct ClockSample {
    offset_ms: i64,
    measured_at: Instant,
}

/// Shared cache of the API server's clock offset.
#[derive(Debug, Clone, Default)]
pub(crate) struct ServerClock(Arc<StdMutex<Option<ClockSample>>>);

impl ServerClock {
    fn cached_offset(&self) -> Option<i64> {
        let sample = *self.0.lock().expect("server clock cache poisoned");
        sample
            .filter(|sample| sample.measured_at.elapsed() < SERVER_CLOCK_TTL)
            .map(|sample| sample.offset_ms)
    }

    fn store(&self, offset_ms: i64) {
        *self.0.lock().expect("server clock cache poisoned") = Some(ClockSample {
            offset_ms,
            measured_at: Instant::now(),
        });
    }
}

fn local_now_ms() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch");
    i64::try_from(elapsed.as_millis()).expect("Unix milliseconds fit in i64")
}

/// Server clock offset from one `Date` header: the header has one-second
/// resolution, so assume mid-second, and assume the server stamped it halfway
/// through the round trip.
pub(crate) fn server_clock_offset_ms(
    date_header: &str,
    sent_ms: i64,
    received_ms: i64,
) -> Option<i64> {
    let server_ms = parse_http_date_ms(date_header)?;
    if received_ms < sent_ms {
        return None;
    }
    Some(server_ms + 500 - (sent_ms + received_ms) / 2)
}

/// Parse an RFC 9110 IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`).
pub(crate) fn parse_http_date_ms(value: &str) -> Option<i64> {
    let mut parts = value.trim().split_ascii_whitespace();
    let _weekday = parts.next()?;
    let day: i64 = parts.next()?.parse().ok()?;
    let month = match parts.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = parts.next()?.parse().ok()?;
    let mut time = parts.next()?.split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: i64 = time.next()?.parse().ok()?;
    if parts.next()? != "GMT" || parts.next().is_some() || time.next().is_some() {
        return None;
    }
    if !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(((days * 24 + hour) * 60 + minute) * 60_000 + second * 1_000)
}

fn nonce_registry() -> &'static StdMutex<HashMap<Address, u64>> {
    static NONCES: OnceLock<StdMutex<HashMap<Address, u64>>> = OnceLock::new();
    NONCES.get_or_init(|| StdMutex::new(HashMap::new()))
}

/// Next nonce for `signer`: one past the larger of the last nonce issued in
/// this process and `server_now_ms`.
pub(crate) fn next_nonce_at(signer: Address, server_now_ms: u64) -> u64 {
    let mut nonces = nonce_registry().lock().expect("nonce registry poisoned");
    let last = nonces.entry(signer).or_insert(0);
    *last = (*last).max(server_now_ms) + 1;
    *last
}

/// Venue expiry the server derives from a TEE perp nonce, or `None` when the
/// nonce is zero or overflows.
pub fn tee_perp_venue_expires_after_ms(nonce: u64) -> Option<u64> {
    if nonce == 0 {
        return None;
    }
    nonce.checked_add(TEE_PERP_VENUE_TTL_MS)
}

/// Whether the server would accept `nonce` at `server_now_ms`.
pub fn tee_perp_nonce_in_window(nonce: u64, server_now_ms: u64) -> bool {
    tee_perp_venue_expires_after_ms(nonce).is_some_and(|expires| {
        expires > server_now_ms
            && expires
                <= server_now_ms.saturating_add(
                    hypercall_sdk_types::tee_perp::TEE_PERP_MAX_VENUE_EXPIRY_WINDOW_MS,
                )
    })
}

// ---------------------------------------------------------------------------
// Price, size, and order construction
// ---------------------------------------------------------------------------

fn decimal_units(value: Decimal) -> Result<(u128, u32)> {
    if value <= Decimal::ZERO {
        return Err(ClientError::InvalidInput(format!(
            "{value} must be positive"
        )));
    }
    let normalized = value.normalize();
    let mantissa = u128::try_from(normalized.mantissa())
        .map_err(|_| ClientError::InvalidInput(format!("{value} must be positive")))?;
    Ok((mantissa, normalized.scale()))
}

fn units_to_decimal(units: u128, scale: u32) -> Result<Decimal> {
    let units = i128::try_from(units)
        .map_err(|_| ClientError::InvalidInput("price out of range".into()))?;
    Decimal::try_from_i128_with_scale(units, scale)
        .map(|value| value.normalize())
        .map_err(|error| ClientError::InvalidInput(format!("price out of range: {error}")))
}

fn pow10(exponent: u32) -> Result<u128> {
    10_u128
        .checked_pow(exponent)
        .ok_or(ClientError::ArithmeticOverflow {
            operation: "perp price scaling",
        })
}

fn validate_size_decimals(size_decimals: u32) -> Result<u32> {
    if size_decimals > HYPERLIQUID_PERP_MAX_PRICE_DECIMALS {
        return Err(ClientError::InvalidInput(format!(
            "size decimals {size_decimals} exceed {HYPERLIQUID_PERP_MAX_PRICE_DECIMALS}"
        )));
    }
    Ok(size_decimals)
}

/// The IOC limit price of a market order (or market trigger): `reference`
/// moved `slippage_bps` against the trader, rounded to a valid Hyperliquid
/// tick inside that bound (buys round down, sells round up). `reference` is
/// the best ask for a buy and the best bid for a sell (or the trigger price).
///
/// Fails when the rounded price would no longer cross `reference`, the same
/// refusal as the Hypercall order panel.
pub fn perp_market_limit_price(
    reference: Decimal,
    side: Side,
    size_decimals: u32,
    slippage_bps: u32,
) -> Result<Decimal> {
    let size_decimals = validate_size_decimals(size_decimals)?;
    if slippage_bps >= BPS_DENOMINATOR {
        return Err(ClientError::InvalidInput(format!(
            "slippage {slippage_bps} bps must be below {BPS_DENOMINATOR}"
        )));
    }
    let (units, scale) = decimal_units(reference)?;
    let factor = match side {
        Side::Buy => BPS_DENOMINATOR + slippage_bps,
        Side::Sell => BPS_DENOMINATOR - slippage_bps,
    };
    let overflow = ClientError::ArithmeticOverflow {
        operation: "perp market price",
    };
    let numerator = units.checked_mul(u128::from(factor)).ok_or(overflow)?;
    let denominator = pow10(scale)?
        .checked_mul(u128::from(BPS_DENOMINATOR))
        .ok_or(ClientError::ArithmeticOverflow {
            operation: "perp market price",
        })?;
    let exponent: i64 = if numerator >= denominator {
        i64::try_from((numerator / denominator).to_string().len()).unwrap_or(i64::MAX) - 1
    } else {
        let mut scaled = numerator;
        let mut places = 0_i64;
        while scaled < denominator {
            scaled = scaled
                .checked_mul(10)
                .ok_or(ClientError::ArithmeticOverflow {
                    operation: "perp market price",
                })?;
            places += 1;
        }
        -places
    };
    let max_decimals = i64::from(HYPERLIQUID_PERP_MAX_PRICE_DECIMALS - size_decimals);
    let sig_fig_exponent = exponent - i64::from(HYPERLIQUID_PERP_MAX_PRICE_SIGNIFICANT_FIGURES) + 1;
    let tick_exponent = sig_fig_exponent.min(0).max(-max_decimals);
    let tick_scale = u32::try_from(-tick_exponent).expect("tick scale is non-negative");
    let scaled_numerator =
        numerator
            .checked_mul(pow10(tick_scale)?)
            .ok_or(ClientError::ArithmeticOverflow {
                operation: "perp market price",
            })?;
    let quotient = scaled_numerator / denominator;
    let ticks = if matches!(side, Side::Buy) || scaled_numerator.is_multiple_of(denominator) {
        quotient
    } else {
        quotient + 1
    };
    let price = units_to_decimal(ticks, tick_scale)?;
    let crosses = match side {
        Side::Buy => price >= reference,
        Side::Sell => price <= reference,
    };
    if !crosses || price <= Decimal::ZERO {
        return Err(ClientError::InvalidInput(format!(
            "no marketable tick within {slippage_bps} bps of {reference}; use a limit order"
        )));
    }
    Ok(price)
}

fn round_working_price_to_tick(units: u128, side: Side, size_decimals: u32) -> Result<Decimal> {
    let magnitude = i64::try_from(units.to_string().len()).unwrap_or(i64::MAX)
        - 1
        - i64::from(SCALE_WORKING_SCALE);
    let sig_fig_decimals =
        (i64::from(HYPERLIQUID_PERP_MAX_PRICE_SIGNIFICANT_FIGURES) - 1 - magnitude).max(0);
    let decimals = sig_fig_decimals
        .min(i64::from(
            HYPERLIQUID_PERP_MAX_PRICE_DECIMALS - size_decimals,
        ))
        .max(0);
    let decimals = u32::try_from(decimals).expect("decimals are non-negative");
    let divisor = pow10(SCALE_WORKING_SCALE - decimals)?;
    let quotient = units / divisor;
    let ticks = if matches!(side, Side::Sell) && !units.is_multiple_of(divisor) {
        quotient + 1
    } else {
        quotient
    };
    units_to_decimal(ticks, decimals)
}

fn to_working_units(value: Decimal, scale: u32, field: &str) -> Result<u128> {
    let (units, value_scale) = decimal_units(value)
        .map_err(|_| ClientError::InvalidInput(format!("{field} must be positive")))?;
    if value_scale > scale {
        Ok(units / pow10(value_scale - scale)?)
    } else {
        units
            .checked_mul(pow10(scale - value_scale)?)
            .ok_or(ClientError::ArithmeticOverflow {
                operation: "perp scale order",
            })
    }
}

/// A price in working units, refusing digits past the working scale. Flooring
/// them would let a sell round to a tick below the caller's bound (for
/// example `1000.0000000000001` to `1000`).
fn to_working_price_units(value: Decimal, field: &str) -> Result<u128> {
    let (units, value_scale) = decimal_units(value)
        .map_err(|_| ClientError::InvalidInput(format!("{field} must be positive")))?;
    if value_scale > SCALE_WORKING_SCALE
        && !units.is_multiple_of(pow10(value_scale - SCALE_WORKING_SCALE)?)
    {
        return Err(ClientError::InvalidInput(format!(
            "{field} supports at most {SCALE_WORKING_SCALE} decimal places"
        )));
    }
    to_working_units(value, SCALE_WORKING_SCALE, field)
}

/// Round `price` to Hyperliquid's tick for `side`: buys round down and sells
/// round up, so the result is never worse for the trader than `price`.
pub fn round_perp_price_to_tick(price: Decimal, side: Side, size_decimals: u32) -> Result<Decimal> {
    let size_decimals = validate_size_decimals(size_decimals)?;
    let units = to_working_price_units(price, "price")?;
    round_working_price_to_tick(units, side, size_decimals)
}

/// One resting limit leg of a scale order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerpScaleLeg {
    pub price: Decimal,
    pub size: Decimal,
}

/// Inputs of [`build_perp_scale_order_legs`].
#[derive(Debug, Clone, Copy)]
pub struct PerpScaleOrderSpec {
    pub start_price: Decimal,
    pub end_price: Decimal,
    pub total_size: Decimal,
    /// 2 to 100 legs.
    pub order_count: usize,
    /// Last leg's size divided by the first leg's (1 = equal sizes), up to 100.
    pub size_skew: Decimal,
    pub side: Side,
    /// Hyperliquid `szDecimals` of the market.
    pub size_decimals: u32,
}

/// Split a scale order into evenly spaced limit legs, exactly as the
/// Hypercall order panel does: leg sizes follow `size_skew`, are floored to
/// the lot size with the remaining lots added to the last legs (so they sum
/// to `total_size`), and prices are rounded to a valid tick on the trader's
/// side.
pub fn build_perp_scale_order_legs(spec: PerpScaleOrderSpec) -> Result<Vec<PerpScaleLeg>> {
    let lot_decimals = validate_size_decimals(spec.size_decimals)?;
    if !(PERP_SCALE_MIN_ORDERS..=PERP_SCALE_MAX_ORDERS).contains(&spec.order_count) {
        return Err(ClientError::InvalidInput(format!(
            "scale orders need {PERP_SCALE_MIN_ORDERS} to {PERP_SCALE_MAX_ORDERS} legs"
        )));
    }
    let one = pow10(SCALE_WORKING_SCALE)?;
    let start = to_working_price_units(spec.start_price, "start price")?;
    let end = to_working_price_units(spec.end_price, "end price")?;
    let skew = to_working_units(spec.size_skew, SCALE_WORKING_SCALE, "size skew")?;
    if skew == 0 || skew > 100 * one {
        return Err(ClientError::InvalidInput(
            "size skew must be positive and at most 100".into(),
        ));
    }
    if start == 0 || end == 0 || start == end {
        return Err(ClientError::InvalidInput(
            "scale start and end prices must be positive and distinct".into(),
        ));
    }
    let (size_units, size_scale) = decimal_units(spec.total_size)?;
    if size_scale > lot_decimals {
        return Err(ClientError::InvalidInput(format!(
            "total size {} has more than {lot_decimals} decimals",
            spec.total_size
        )));
    }
    let size_lots = size_units * pow10(lot_decimals - size_scale)?;
    let count = spec.order_count as u128;
    if size_lots < count {
        return Err(ClientError::InvalidInput(
            "total size is too small for this many legs".into(),
        ));
    }

    let overflow = || ClientError::ArithmeticOverflow {
        operation: "perp scale order",
    };
    let steps = count - 1;
    let weights: Vec<i128> = (0..count)
        .map(|index| {
            let base = i128::try_from(steps * one).map_err(|_| overflow())?;
            let delta = (i128::try_from(skew).map_err(|_| overflow())?
                - i128::try_from(one).map_err(|_| overflow())?)
                * i128::try_from(index).map_err(|_| overflow())?;
            Ok(base + delta)
        })
        .collect::<Result<_>>()?;
    let total_weight: i128 = weights.iter().sum();
    let size_lots_signed = i128::try_from(size_lots).map_err(|_| overflow())?;
    let mut sizes: Vec<i128> = weights
        .iter()
        .map(|weight| {
            size_lots_signed
                .checked_mul(*weight)
                .map(|product| product / total_weight)
                .ok_or_else(overflow)
        })
        .collect::<Result<_>>()?;
    let mut remainder = size_lots_signed - sizes.iter().sum::<i128>();
    let mut index = sizes.len() - 1;
    while remainder > 0 {
        sizes[index] += 1;
        remainder -= 1;
        index = (index + sizes.len() - 1) % sizes.len();
    }
    if sizes.iter().any(|size| *size <= 0) {
        return Err(ClientError::InvalidInput(
            "total size is too small for this size skew".into(),
        ));
    }

    let start_signed = i128::try_from(start).map_err(|_| overflow())?;
    let end_signed = i128::try_from(end).map_err(|_| overflow())?;
    let steps_signed = i128::try_from(steps).map_err(|_| overflow())?;
    sizes
        .into_iter()
        .enumerate()
        .map(|(index, lots)| {
            let index = i128::try_from(index).map_err(|_| overflow())?;
            let price_units = start_signed + (end_signed - start_signed) * index / steps_signed;
            let price_units = u128::try_from(price_units).map_err(|_| overflow())?;
            let price = round_working_price_to_tick(price_units, spec.side, lot_decimals)?;
            let size =
                units_to_decimal(u128::try_from(lots).map_err(|_| overflow())?, lot_decimals)?;
            if price <= Decimal::ZERO {
                return Err(ClientError::InvalidInput(
                    "scale price range rounds to zero".into(),
                ));
            }
            Ok(PerpScaleLeg { price, size })
        })
        .collect()
}

/// Child cloid of one signed batch: the nonce in the high bits and the
/// 1-based child index in the low byte. Distinct per child, never zero, and
/// reproducible from the signed nonce.
pub fn tee_perp_batch_cloid(nonce: u64, index: usize) -> Result<u128> {
    if nonce == 0 {
        return Err(ClientError::InvalidInput("nonce must be nonzero".into()));
    }
    if index >= MAX_BATCH_CLOID_INDEX {
        return Err(ClientError::InvalidInput(format!(
            "batch child index {index} exceeds {}",
            MAX_BATCH_CLOID_INDEX - 1
        )));
    }
    Ok((u128::from(nonce) << 8) + index as u128 + 1)
}

fn nonzero_cloid(cloid: u128) -> Result<u128> {
    if cloid == 0 {
        return Err(ClientError::InvalidInput("cloid must not be zero".into()));
    }
    Ok(cloid)
}

/// A plain limit child for an order batch.
pub fn tee_perp_limit_order_wire(
    asset: u32,
    side: Side,
    price: Decimal,
    size: Decimal,
    tif: PerpTimeInForce,
    reduce_only: bool,
    cloid: u128,
) -> Result<TeePerpOrderWire> {
    Ok(TeePerpOrderWire {
        asset,
        is_buy: matches!(side, Side::Buy),
        limit_px: to_perp_units(price, "limit price")?,
        sz: to_perp_units(size, "size")?,
        reduce_only,
        order_type: TeePerpOrderType::Limit(TeePerpLimitOrderType { tif }),
        cloid: nonzero_cloid(cloid)?,
    })
}

/// One trigger exit or entry. `limit_price` is the signed execution bound:
/// the user's limit for a limit trigger, or the slippage-bounded worst price
/// from [`perp_market_limit_price`] for a market trigger.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is one independent order term, as in the HL order wire"
)]
pub fn tee_perp_trigger_order_wire(
    asset: u32,
    side: Side,
    trigger_price: Decimal,
    limit_price: Decimal,
    size: Decimal,
    is_market: bool,
    tpsl: TeePerpTpsl,
    reduce_only: bool,
    cloid: u128,
) -> Result<TeePerpOrderWire> {
    Ok(TeePerpOrderWire {
        asset,
        is_buy: matches!(side, Side::Buy),
        limit_px: to_perp_units(limit_price, "limit price")?,
        sz: to_perp_units(size, "size")?,
        reduce_only,
        order_type: TeePerpOrderType::Trigger(TeePerpTriggerOrderType {
            trigger_px: to_perp_units(trigger_price, "trigger price")?,
            is_market,
            tpsl,
        }),
        cloid: nonzero_cloid(cloid)?,
    })
}

/// A take-profit or stop-loss exit executed as a market order once triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeePerpExit {
    pub trigger_price: Decimal,
    /// Worst execution price once triggered.
    pub limit_price: Decimal,
}

impl TeePerpExit {
    /// An exit whose execution bound is `trigger_price` moved `slippage_bps`
    /// against an exit on `exit_side`, as the Hypercall order panel signs it.
    pub fn market(
        trigger_price: Decimal,
        exit_side: Side,
        size_decimals: u32,
        slippage_bps: u32,
    ) -> Result<Self> {
        Ok(Self {
            trigger_price,
            limit_price: perp_market_limit_price(
                trigger_price,
                exit_side,
                size_decimals,
                slippage_bps,
            )?,
        })
    }
}

fn opposite(side: Side) -> Side {
    match side {
        Side::Buy => Side::Sell,
        Side::Sell => Side::Buy,
    }
}

fn exit_children(
    orders: &mut Vec<TeePerpOrderWire>,
    asset: u32,
    exit_side: Side,
    size: Decimal,
    take_profit: Option<TeePerpExit>,
    stop_loss: Option<TeePerpExit>,
    nonce: u64,
) -> Result<()> {
    if take_profit.is_none() && stop_loss.is_none() {
        return Err(ClientError::InvalidInput(
            "a TP/SL needs a take-profit or stop-loss price".into(),
        ));
    }
    for (tpsl, exit) in [(TeePerpTpsl::Tp, take_profit), (TeePerpTpsl::Sl, stop_loss)] {
        let Some(exit) = exit else { continue };
        let cloid = tee_perp_batch_cloid(nonce, orders.len())?;
        orders.push(tee_perp_trigger_order_wire(
            asset,
            exit_side,
            exit.trigger_price,
            exit.limit_price,
            size,
            true,
            tpsl,
            true,
            cloid,
        )?);
    }
    Ok(())
}

/// The entry of a [`build_tee_perp_bracket_batch`].
#[derive(Debug, Clone, Copy)]
pub struct TeePerpBracketEntry {
    pub asset: u32,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub tif: PerpTimeInForce,
    pub reduce_only: bool,
}

/// An entry with attached TP/SL, grouped `normalTpsl` so Hyperliquid only
/// activates the exits once the entry fills. Exits are reduce-only market
/// triggers on the opposite side for the full entry size.
pub fn build_tee_perp_bracket_batch(
    entry: TeePerpBracketEntry,
    take_profit: Option<TeePerpExit>,
    stop_loss: Option<TeePerpExit>,
    nonce: u64,
) -> Result<TeePerpOrderBatchAction> {
    let mut orders = vec![tee_perp_limit_order_wire(
        entry.asset,
        entry.side,
        entry.price,
        entry.size,
        entry.tif,
        entry.reduce_only,
        tee_perp_batch_cloid(nonce, 0)?,
    )?];
    exit_children(
        &mut orders,
        entry.asset,
        opposite(entry.side),
        entry.size,
        take_profit,
        stop_loss,
        nonce,
    )?;
    Ok(TeePerpOrderBatchAction {
        orders,
        grouping: TeePerpOrderGrouping::NormalTpsl,
    })
}

/// Reduce-only TP/SL exits on an existing position, grouped `positionTpsl`.
/// `position_side` is the position's direction (`Buy` = long); exits trade
/// the opposite side.
pub fn build_tee_perp_position_tpsl_batch(
    asset: u32,
    position_side: Side,
    size: Decimal,
    take_profit: Option<TeePerpExit>,
    stop_loss: Option<TeePerpExit>,
    nonce: u64,
) -> Result<TeePerpOrderBatchAction> {
    let mut orders = Vec::with_capacity(2);
    exit_children(
        &mut orders,
        asset,
        opposite(position_side),
        size,
        take_profit,
        stop_loss,
        nonce,
    )?;
    Ok(TeePerpOrderBatchAction {
        orders,
        grouping: TeePerpOrderGrouping::PositionTpsl,
    })
}

/// Scale legs submitted together as one ungrouped batch.
pub fn build_tee_perp_scale_batch(
    asset: u32,
    side: Side,
    legs: &[PerpScaleLeg],
    tif: PerpTimeInForce,
    reduce_only: bool,
    nonce: u64,
) -> Result<TeePerpOrderBatchAction> {
    let orders = legs
        .iter()
        .enumerate()
        .map(|(index, leg)| {
            tee_perp_limit_order_wire(
                asset,
                side,
                leg.price,
                leg.size,
                tif,
                reduce_only,
                tee_perp_batch_cloid(nonce, index)?,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let batch = TeePerpOrderBatchAction {
        orders,
        grouping: TeePerpOrderGrouping::Na,
    };
    validate_batch(&batch)?;
    Ok(batch)
}

fn validate_batch(batch: &TeePerpOrderBatchAction) -> Result<()> {
    if batch.orders.is_empty() || batch.orders.len() > MAX_TEE_PERP_ORDER_BATCH_CHILDREN {
        return Err(ClientError::InvalidInput(format!(
            "an order batch needs 1 to {MAX_TEE_PERP_ORDER_BATCH_CHILDREN} orders"
        )));
    }
    let mut cloids = std::collections::BTreeSet::new();
    for order in &batch.orders {
        nonzero_cloid(order.cloid)?;
        if !cloids.insert(order.cloid) {
            return Err(ClientError::InvalidInput(
                "every order in a batch must use a distinct cloid".into(),
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Modify and TWAP
// ---------------------------------------------------------------------------

/// The new terms for one resting limit order, in human units.
///
/// The order is identified twice, and both must agree: by `target_directive_id`
/// (the `directiveId` its submit returned; for a batch child, that child's
/// `directiveId`) and by `cloid` (the order's client order ID, which the
/// modify preserves). Only limit orders (GTC, IOC, ALO) can be modified on
/// the TEE route; see the crate README for the Hyperliquid parity notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeePerpOrderReplacement {
    pub asset: u32,
    pub target_directive_id: uuid::Uuid,
    pub cloid: u128,
    pub side: Side,
    /// New limit price. At most 8 decimal places.
    pub price: Decimal,
    /// New size in coins. At most 8 decimal places.
    pub size: Decimal,
    pub tif: PerpTimeInForce,
    pub reduce_only: bool,
}

impl TeePerpOrderReplacement {
    /// The signed wire action, scaled to 1e8 units and validated as the
    /// server validates it.
    pub fn to_action(&self) -> Result<TeePerpModifyAction> {
        let action = TeePerpModifyAction {
            asset: self.asset,
            target_directive_id: self.target_directive_id,
            is_buy: matches!(self.side, Side::Buy),
            limit_px: to_perp_units(self.price, "limit price")?,
            sz: to_perp_units(self.size, "size")?,
            reduce_only: self.reduce_only,
            tif: self.tif,
            cloid: self.cloid,
        };
        validate_modify_action(&action)?;
        Ok(action)
    }
}

/// One modify's terms, held to the server's rules: a non-nil target, a
/// non-zero cloid, and a positive price and size.
fn validate_modify_action(action: &TeePerpModifyAction) -> Result<()> {
    if action.target_directive_id.is_nil() {
        return Err(ClientError::InvalidInput(
            "target_directive_id must not be nil".into(),
        ));
    }
    nonzero_cloid(action.cloid)?;
    if action.limit_px == 0 || action.sz == 0 {
        return Err(ClientError::InvalidInput(
            "a modify needs a positive limit price and size".into(),
        ));
    }
    Ok(())
}

/// A batch modify's shape: 1 to `max_children` children (never more than
/// [`MAX_TEE_PERP_BATCH_MODIFY_CHILDREN`]), each a valid single modify, with
/// distinct targets and distinct cloids.
fn validate_batch_modify(batch: &TeePerpBatchModifyAction, max_children: usize) -> Result<()> {
    let cap = max_children.min(MAX_TEE_PERP_BATCH_MODIFY_CHILDREN);
    if batch.modifies.is_empty() || batch.modifies.len() > cap {
        return Err(ClientError::InvalidInput(format!(
            "a batch modify needs 1 to {cap} orders, got {}",
            batch.modifies.len()
        )));
    }
    let mut targets = std::collections::BTreeSet::new();
    let mut cloids = std::collections::BTreeSet::new();
    for action in &batch.modifies {
        validate_modify_action(action)?;
        if !targets.insert(action.target_directive_id) {
            return Err(ClientError::InvalidInput(format!(
                "batch modify targets order {} twice",
                action.target_directive_id
            )));
        }
        if !cloids.insert(action.cloid) {
            return Err(ClientError::InvalidInput(
                "every order in a batch modify must use a distinct cloid".into(),
            ));
        }
    }
    Ok(())
}

/// Build the signed action of a batch modify, refusing one the server would
/// refuse. `max_children` is the instance cap (`None` for
/// [`DEFAULT_TEE_PERP_BATCH_MODIFY_MAX_CHILDREN`]); it can lower the cap but
/// never raise it past [`MAX_TEE_PERP_BATCH_MODIFY_CHILDREN`].
pub fn build_tee_perp_batch_modify(
    replacements: &[TeePerpOrderReplacement],
    max_children: Option<usize>,
) -> Result<TeePerpBatchModifyAction> {
    let max_children = max_children.unwrap_or(DEFAULT_TEE_PERP_BATCH_MODIFY_MAX_CHILDREN);
    if max_children == 0 || max_children > MAX_TEE_PERP_BATCH_MODIFY_CHILDREN {
        return Err(ClientError::InvalidInput(format!(
            "max_children must be 1 to {MAX_TEE_PERP_BATCH_MODIFY_CHILDREN}"
        )));
    }
    let batch = TeePerpBatchModifyAction {
        modifies: replacements
            .iter()
            .map(TeePerpOrderReplacement::to_action)
            .collect::<Result<_>>()?,
    };
    validate_batch_modify(&batch, max_children)?;
    Ok(batch)
}

fn validate_twap_action(action: &TeePerpTwapAction) -> Result<()> {
    if !(TEE_PERP_TWAP_MIN_MINUTES..=TEE_PERP_TWAP_MAX_MINUTES).contains(&action.minutes) {
        return Err(ClientError::InvalidInput(format!(
            "TWAP minutes must be {TEE_PERP_TWAP_MIN_MINUTES} to {TEE_PERP_TWAP_MAX_MINUTES}, got {}",
            action.minutes
        )));
    }
    if action.sz == 0 {
        return Err(ClientError::InvalidInput(
            "TWAP size must be positive".into(),
        ));
    }
    Ok(())
}

/// A signed body whose nonce identifies it to the server.
trait SignedTeeSubmission: serde::Serialize {
    fn nonce(&self) -> u64;
}

impl<A: serde::Serialize> SignedTeeSubmission for DirectiveSubmitRequest<A> {
    fn nonce(&self) -> u64 {
        self.nonce
    }
}

impl SignedTeeSubmission for TeeApiWalletApprovalSubmitRequest {
    fn nonce(&self) -> u64 {
        self.nonce
    }
}

/// Whether a failed submission may still have reached the server: any
/// transport or decode failure, a `408`, or a `5xx`. Other `4xx` answers
/// are definitive.
fn tee_submission_is_ambiguous(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Http(_)
            | ClientError::Api {
                status: 408 | 500..=599,
                ..
            }
    )
}

fn from_perp_units(units: u64) -> Decimal {
    Decimal::from_i128_with_scale(i128::from(units), 8).normalize()
}

/// The preview form of a signed order wire (1e8 integers back to decimals),
/// so a preview describes exactly what will be signed.
pub fn perp_preview_order(order: &TeePerpOrderWire) -> PerpPreviewOrder {
    PerpPreviewOrder {
        asset: order.asset,
        is_buy: order.is_buy,
        limit_px: from_perp_units(order.limit_px),
        sz: from_perp_units(order.sz),
        reduce_only: order.reduce_only,
        order_type: match order.order_type {
            TeePerpOrderType::Limit(limit) => PerpPreviewOrderType::Limit(PerpPreviewLimit {
                tif: limit.tif.into(),
            }),
            TeePerpOrderType::Trigger(trigger) => {
                PerpPreviewOrderType::Trigger(PerpPreviewTrigger {
                    trigger_px: from_perp_units(trigger.trigger_px),
                    is_market: trigger.is_market,
                    tpsl: trigger.tpsl,
                })
            }
        },
    }
}

/// The preview action of a signed order batch.
pub fn perp_preview_batch_action(batch: &TeePerpOrderBatchAction) -> PerpPreviewAction {
    PerpPreviewAction::OrderBatch {
        grouping: batch.grouping,
        orders: batch.orders.iter().map(perp_preview_order).collect(),
        leverage: None,
    }
}

// ---------------------------------------------------------------------------
// Request parameters
// ---------------------------------------------------------------------------

/// A market order: an IOC limit at `reference_price` moved `slippage_bps`
/// against the trader and rounded to a valid tick.
#[derive(Debug, Clone)]
pub struct TeePerpMarketOrderParams {
    pub account: AccountAddress,
    pub asset: u32,
    pub side: Side,
    pub size: Decimal,
    /// Best ask for a buy, best bid for a sell.
    pub reference_price: Decimal,
    /// Hyperliquid `szDecimals` of the market.
    pub size_decimals: u32,
    /// Defaults to [`PERP_MARKET_SLIPPAGE_BPS`].
    pub slippage_bps: Option<u32>,
    pub reduce_only: bool,
    pub client_order_id: Option<u128>,
    pub nonce: Option<u64>,
}

/// How a trigger order executes once triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeePerpTriggerExecution {
    /// Market (IOC) bounded `slippage_bps` beyond the trigger price.
    Market {
        size_decimals: u32,
        /// Defaults to [`PERP_MARKET_SLIPPAGE_BPS`].
        slippage_bps: Option<u32>,
    },
    /// A limit order at `price`.
    Limit { price: Decimal },
}

/// A native stop-loss or take-profit order (market or limit).
#[derive(Debug, Clone)]
pub struct TeePerpTriggerOrderParams {
    pub account: AccountAddress,
    pub asset: u32,
    pub side: Side,
    pub size: Decimal,
    pub trigger_price: Decimal,
    /// `Sl` for a stop, `Tp` for a take-profit.
    pub tpsl: TeePerpTpsl,
    pub execution: TeePerpTriggerExecution,
    pub reduce_only: bool,
    /// Defaults to `tee_perp_batch_cloid(nonce, 0)`.
    pub client_order_id: Option<u128>,
    pub nonce: Option<u64>,
}

/// An entry with attached TP/SL exits (`normalTpsl`).
#[derive(Debug, Clone)]
pub struct TeePerpBracketOrderParams {
    pub account: AccountAddress,
    pub entry: TeePerpBracketEntry,
    pub take_profit: Option<TeePerpExit>,
    pub stop_loss: Option<TeePerpExit>,
    pub nonce: Option<u64>,
}

/// TP/SL exits on an existing position (`positionTpsl`).
#[derive(Debug, Clone)]
pub struct TeePerpPositionTpslParams {
    pub account: AccountAddress,
    pub asset: u32,
    /// The position's direction (`Buy` = long).
    pub position_side: Side,
    pub size: Decimal,
    pub take_profit: Option<TeePerpExit>,
    pub stop_loss: Option<TeePerpExit>,
    pub nonce: Option<u64>,
}

/// A scale order: limit legs submitted as one ungrouped batch.
#[derive(Debug, Clone)]
pub struct TeePerpScaleOrderParams {
    pub account: AccountAddress,
    pub asset: u32,
    pub side: Side,
    /// From [`build_perp_scale_order_legs`] or caller-chosen.
    pub legs: Vec<PerpScaleLeg>,
    pub tif: PerpTimeInForce,
    pub reduce_only: bool,
    pub nonce: Option<u64>,
}

/// A caller-built order batch. Cloids must already be set; derive them from
/// the nonce with [`tee_perp_batch_cloid`] after reserving it with
/// [`HypercallClient::next_tee_perp_nonce`].
#[derive(Debug, Clone)]
pub struct TeePerpOrderBatchParams {
    pub account: AccountAddress,
    pub nonce: u64,
    pub batch: TeePerpOrderBatchAction,
}

/// A leverage and margin-mode change.
#[derive(Debug, Clone, Copy)]
pub struct TeePerpLeverageParams {
    pub account: AccountAddress,
    pub asset: u32,
    /// `true` for cross margin, `false` for isolated.
    pub is_cross: bool,
    /// At least 1.
    pub leverage: u32,
    pub nonce: Option<u64>,
}

/// One resting limit order's replacement
/// ([`HypercallClient::modify_tee_perp_order`]).
#[derive(Debug, Clone, Copy)]
pub struct TeePerpModifyParams {
    pub account: AccountAddress,
    pub replacement: TeePerpOrderReplacement,
    pub nonce: Option<u64>,
}

/// Several resting limit orders' replacements under one signature
/// ([`HypercallClient::batch_modify_tee_perp_orders`]).
#[derive(Debug, Clone)]
pub struct TeePerpBatchModifyParams {
    pub account: AccountAddress,
    /// In the order the venue applies and answers them.
    pub replacements: Vec<TeePerpOrderReplacement>,
    /// The server instance's `api.tee_perp_batch_modify_max_children`, when
    /// it is not the default ([`DEFAULT_TEE_PERP_BATCH_MODIFY_MAX_CHILDREN`]).
    /// At most [`MAX_TEE_PERP_BATCH_MODIFY_CHILDREN`].
    pub max_children: Option<usize>,
    pub nonce: Option<u64>,
}

/// A native Hyperliquid TWAP ([`HypercallClient::place_tee_perp_twap_order`]).
#[derive(Debug, Clone, Copy)]
pub struct TeePerpTwapParams {
    pub account: AccountAddress,
    pub asset: u32,
    pub side: Side,
    /// Total size in coins. At most 8 decimal places.
    pub size: Decimal,
    pub reduce_only: bool,
    /// [`TEE_PERP_TWAP_MIN_MINUTES`] to [`TEE_PERP_TWAP_MAX_MINUTES`].
    pub minutes: u32,
    /// Vary slice timing.
    pub randomize: bool,
    pub nonce: Option<u64>,
}

/// Cancel a running TWAP ([`HypercallClient::cancel_tee_perp_twap`]).
#[derive(Debug, Clone, Copy)]
pub struct TeePerpTwapCancelParams {
    pub account: AccountAddress,
    pub asset: u32,
    /// [`TeePerpTwapResponse::twap_id`] of the TWAP to cancel.
    pub twap_id: u64,
    pub nonce: Option<u64>,
}

/// Parameters for [`HypercallClient::approve_hypercall_builder`].
#[derive(Debug, Clone)]
pub struct ApproveHypercallBuilderParams {
    /// HyperEVM JSON-RPC endpoint.
    pub rpc_url: String,
    /// The Account.sol (0.2.4 or newer) the manager controls.
    pub account: AccountAddress,
    /// Builder to approve. `None` reads `hl_builder_address` from `/exchange-info`.
    pub builder: Option<AccountAddress>,
}

/// The manager-authenticated TEE API wallet approval challenge.
#[derive(Debug, Clone)]
pub struct TeeApiWalletApprovalChallenge {
    pub account: AccountAddress,
    pub manager: AccountAddress,
    /// Unix seconds signed in the authentication message.
    pub timestamp: i64,
    pub authentication_signature: String,
    pub package: TeeApiWalletApprovalPackage,
}

/// Progress of a TEE API wallet approval directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeeApiWalletApprovalProgress {
    Pending,
    Finalized,
    Failed,
}

/// Classify a `/v1/directives/{id}` status of the approval directive.
pub fn tee_api_wallet_approval_progress(
    status: &DirectiveStatusResponse,
) -> TeeApiWalletApprovalProgress {
    if status.domain_status == DirectiveDomainStatus::Completed
        && status.delivery_status == DirectiveDeliveryStatus::Finalized
    {
        return TeeApiWalletApprovalProgress::Finalized;
    }
    if matches!(
        status.domain_status,
        DirectiveDomainStatus::Failed | DirectiveDomainStatus::Rejected
    ) || matches!(
        status.delivery_status,
        DirectiveDeliveryStatus::CoreRejected
            | DirectiveDeliveryStatus::Reverted
            | DirectiveDeliveryStatus::Expired
            | DirectiveDeliveryStatus::DeadLettered
    ) {
        return TeeApiWalletApprovalProgress::Failed;
    }
    TeeApiWalletApprovalProgress::Pending
}

/// Result of [`HypercallClient::enroll_tee_api_wallet`].
#[derive(Debug, Clone)]
pub struct TeeApiWalletEnrollment {
    /// The TEE-controlled Hyperliquid API wallet now bound to the account.
    pub api_wallet: AccountAddress,
    pub api_wallet_name: String,
    /// `None` when Hyperliquid already had the wallet bound.
    pub directive_id: Option<String>,
}

/// Polling of [`HypercallClient::enroll_tee_api_wallet`].
#[derive(Debug, Clone, Copy)]
pub struct TeeApiWalletEnrollmentPolling {
    pub interval: Duration,
    pub attempts: u32,
}

impl Default for TeeApiWalletEnrollmentPolling {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(2),
            attempts: 60,
        }
    }
}

/// Hypercall builder published by `/exchange-info`. Never guessed.
pub fn resolve_hypercall_builder_address(
    exchange_info: &ExchangeInfoResponse,
) -> Result<AccountAddress> {
    let configured = exchange_info.hl_builder_address.as_deref().ok_or_else(|| {
        ClientError::InvalidInput("exchange-info does not publish hl_builder_address".into())
    })?;
    let builder: AccountAddress = configured
        .parse()
        .map_err(|source| ClientError::invalid_input("hl_builder_address", source))?;
    if builder.as_wallet_address().inner() == Address::ZERO {
        return Err(ClientError::InvalidInput(
            "exchange-info publishes a zero hl_builder_address".into(),
        ));
    }
    Ok(builder)
}

// ---------------------------------------------------------------------------
// Hyperliquid info reads
// ---------------------------------------------------------------------------

/// Hyperliquid margin setting read back from the venue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HyperliquidLeverage {
    pub is_cross: bool,
    pub value: u32,
}

#[derive(Debug, Deserialize)]
struct ActiveAssetData {
    user: String,
    coin: String,
    leverage: ActiveAssetLeverage,
}

#[derive(Debug, Deserialize)]
struct ActiveAssetLeverage {
    #[serde(rename = "type")]
    kind: String,
    value: u32,
}

/// Hyperliquid `userRole` response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HyperliquidUserRole {
    pub role: String,
    #[serde(default)]
    pub data: Option<HyperliquidUserRoleData>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HyperliquidUserRoleData {
    #[serde(default)]
    pub user: Option<String>,
}

impl HyperliquidUserRole {
    /// Whether this role is an API wallet (agent) bound to `account`.
    pub fn is_agent_for(&self, account: AccountAddress) -> bool {
        self.role == "agent"
            && self
                .data
                .as_ref()
                .and_then(|data| data.user.as_deref())
                .and_then(|user| user.parse::<AccountAddress>().ok())
                == Some(account)
    }
}

/// Minimal Hyperliquid `/info` reader for TEE perp readbacks.
#[derive(Debug, Clone)]
pub struct HyperliquidInfoClient {
    client: reqwest::Client,
    url: String,
}

impl HyperliquidInfoClient {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .expect("failed to build reqwest client"),
            url: url.into(),
        }
    }

    pub fn mainnet() -> Self {
        Self::new(HYPERLIQUID_MAINNET_INFO_URL)
    }

    pub fn testnet() -> Self {
        Self::new(HYPERLIQUID_TESTNET_INFO_URL)
    }

    /// The info client for a Hypercall directive chain id (999 or 998).
    pub fn for_chain_id(chain_id: u64) -> Result<Self> {
        match chain_id {
            999 => Ok(Self::mainnet()),
            998 => Ok(Self::testnet()),
            _ => Err(ClientError::InvalidInput(format!(
                "unsupported Hyperliquid chain id: {chain_id}"
            ))),
        }
    }

    async fn post<T: serde::de::DeserializeOwned>(
        &self,
        body: serde_json::Value,
        action: &str,
    ) -> Result<T> {
        HypercallClient::send_reqwest_json(self.client.post(&self.url).json(&body), action).await
    }

    /// Hyperliquid's role for `address`.
    pub async fn user_role(&self, address: AccountAddress) -> Result<HyperliquidUserRole> {
        self.post(
            serde_json::json!({"type": "userRole", "user": address.to_string()}),
            "read Hyperliquid user role",
        )
        .await
    }

    /// Whether Hyperliquid has `api_wallet` bound as an agent of `account`:
    /// the TEE API wallet enrollment status.
    pub async fn is_tee_api_wallet_active(
        &self,
        account: AccountAddress,
        api_wallet: AccountAddress,
    ) -> Result<bool> {
        Ok(self.user_role(api_wallet).await?.is_agent_for(account))
    }

    /// The account's leverage and margin mode for `coin` (e.g. `BTC` or
    /// `xyz:SNDK`), read back from Hyperliquid after a leverage change.
    pub async fn active_asset_leverage(
        &self,
        account: AccountAddress,
        coin: &str,
    ) -> Result<HyperliquidLeverage> {
        let data: ActiveAssetData = self
            .post(
                serde_json::json!({"type": "activeAssetData", "user": account.to_string(), "coin": coin}),
                "read Hyperliquid active asset data",
            )
            .await?;
        let echoed_user = data.user.parse::<AccountAddress>().ok();
        if echoed_user != Some(account) || data.coin != coin {
            return Err(ClientError::Api {
                status: 200,
                message: "Hyperliquid returned leverage for a different account or market".into(),
            });
        }
        let is_cross = match data.leverage.kind.as_str() {
            "cross" => true,
            "isolated" => false,
            _ => {
                return Err(ClientError::Api {
                    status: 200,
                    message: "Hyperliquid returned an invalid leverage setting".into(),
                })
            }
        };
        if data.leverage.value < 1 {
            return Err(ClientError::Api {
                status: 200,
                message: "Hyperliquid returned an invalid leverage setting".into(),
            });
        }
        Ok(HyperliquidLeverage {
            is_cross,
            value: data.leverage.value,
        })
    }
}

// ---------------------------------------------------------------------------
// Client methods
// ---------------------------------------------------------------------------

fn approval_typed_data_mismatch(field: &str) -> ClientError {
    ClientError::InvalidInput(format!(
        "TEE API wallet approval typed data does not match the package: {field}"
    ))
}

fn validate_approval_typed_data(challenge: &TeeApiWalletApprovalChallenge) -> Result<()> {
    let typed = &challenge.package.typed_data;
    if typed["primaryType"] != "HLAddApiWallet" {
        return Err(approval_typed_data_mismatch("primaryType"));
    }
    if typed["domain"]["name"] != HYPERCALL_MANAGER_DOMAIN {
        return Err(approval_typed_data_mismatch("domain.name"));
    }
    let message = &typed["message"];
    let same_address = |value: &serde_json::Value, expected: WalletAddress| {
        value
            .as_str()
            .and_then(|text| text.parse::<WalletAddress>().ok())
            == Some(expected)
    };
    if !same_address(&message["account"], challenge.account.into_wallet_address()) {
        return Err(approval_typed_data_mismatch("message.account"));
    }
    let nonce = match &message["nonce"] {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    };
    if nonce != Some(challenge.package.nonce) {
        return Err(approval_typed_data_mismatch("message.nonce"));
    }
    let action = &message["action"];
    if !same_address(
        &action["apiWalletAddress"],
        challenge.package.api_wallet_address,
    ) {
        return Err(approval_typed_data_mismatch(
            "message.action.apiWalletAddress",
        ));
    }
    if action["apiWalletName"] != challenge.package.api_wallet_name.as_str() {
        return Err(approval_typed_data_mismatch("message.action.apiWalletName"));
    }
    Ok(())
}

fn onchain_error(
    operation: impl Into<String>,
    source: impl std::error::Error + Send + Sync + 'static,
) -> ClientError {
    ClientError::Onchain {
        operation: operation.into(),
        source: Box::new(source),
    }
}

/// An Account implementation older than 0.2.4 has no `approveZeroFeeBuilder`
/// and reverts with no data.
fn is_missing_function_revert(error: &alloy::contract::Error) -> bool {
    match error.as_revert_data() {
        Some(data) => data.is_empty(),
        None => error
            .to_string()
            .to_ascii_lowercase()
            .contains("execution reverted"),
    }
}

impl HypercallClient {
    /// The API server's current Unix millisecond, from the `/health` `Date`
    /// header (cached for a minute per client and its clones).
    pub async fn server_now_ms(&self) -> Result<u64> {
        let offset = match self.server_clock.cached_offset() {
            Some(offset) => offset,
            None => {
                let sent_ms = local_now_ms();
                let response = self.request(Method::GET, "/health").send().await?;
                let received_ms = local_now_ms();
                let date = response
                    .headers()
                    .get(reqwest::header::DATE)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| ClientError::Api {
                        status: response.status().as_u16(),
                        message: "/health response carries no Date header to stamp TEE nonces"
                            .into(),
                    })?;
                let offset =
                    server_clock_offset_ms(date, sent_ms, received_ms).ok_or_else(|| {
                        ClientError::Api {
                            status: response.status().as_u16(),
                            message: format!("/health Date header is not an HTTP date: {date}"),
                        }
                    })?;
                self.server_clock.store(offset);
                offset
            }
        };
        u64::try_from(local_now_ms() + offset).map_err(|_| ClientError::ArithmeticOverflow {
            operation: "server clock",
        })
    }

    /// Next TEE perp nonce for `signer`, stamped on the server clock and
    /// strictly increasing per signer within this process.
    pub async fn next_tee_perp_nonce(&self, signer: AccountAddress) -> Result<u64> {
        let now = self.server_now_ms().await?;
        Ok(next_nonce_at(signer.as_wallet_address().inner(), now))
    }

    async fn resolve_tee_nonce(&self, signer: AccountAddress, nonce: Option<u64>) -> Result<u64> {
        match nonce {
            Some(nonce) => Ok(nonce),
            None => self.next_tee_perp_nonce(signer).await,
        }
    }

    async fn post_tee<T, B>(&self, route: &str, body: &B, action: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
        B: serde::Serialize + ?Sized,
    {
        Self::send_json_body(self.request(Method::POST, route), body, action).await
    }

    /// POST a signed, mutating submission. The body is serialized once and
    /// those exact bytes are sent, so an ambiguous failure can hand them back
    /// in [`ClientError::TeeSubmissionAmbiguous`] for an identical resend.
    async fn post_tee_signed<T, B>(&self, route: &str, body: &B, action: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
        B: SignedTeeSubmission,
    {
        let request_json = sonic_rs::to_string(body)
            .map_err(|source| ClientError::invalid_input("TEE request body", source))?;
        self.send_tee_submission(route, request_json, body.nonce(), action, false)
            .await
    }

    async fn send_tee_submission<T>(
        &self,
        route: &str,
        request_json: String,
        nonce: u64,
        action: &str,
        resend: bool,
    ) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let request = self
            .request(Method::POST, route)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request_json.clone());
        match Self::send_reqwest_json(request, action).await {
            // A resend cannot settle the original's outcome: the signed action
            // can expire (and be refused before journal dedup) while the
            // original still executed, so every resend failure stays ambiguous.
            Err(error) if resend || tee_submission_is_ambiguous(&error) => {
                Err(ClientError::TeeSubmissionAmbiguous {
                    action: action.to_owned(),
                    route: route.to_owned(),
                    nonce,
                    request_json,
                    source: Box::new(error),
                })
            }
            result => result,
        }
    }

    /// Resend the exact signed body of a [`ClientError::TeeSubmissionAmbiguous`].
    /// The server recognises the same signed action (same nonce, same terms)
    /// and does not submit it to the venue twice; a repeat comes back with
    /// `duplicate_directive` and stage `unknown`, meaning reconcile. Pass the
    /// error's `route` and `request_json` unchanged.
    ///
    /// Any failure of the resend, including a `4xx` such as an expired
    /// signature, is returned as [`ClientError::TeeSubmissionAmbiguous`]: the
    /// original request may still have executed, so reconcile before signing
    /// a fresh action.
    pub async fn resend_tee_submission<T>(&self, route: &str, request_json: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let nonce = serde_json::from_str::<serde_json::Value>(request_json)
            .ok()
            .and_then(|body| match &body["nonce"] {
                serde_json::Value::Number(number) => number.as_u64(),
                serde_json::Value::String(text) => text.parse().ok(),
                _ => None,
            })
            .ok_or_else(|| {
                ClientError::InvalidInput("request_json carries no signed nonce".into())
            })?;
        self.send_tee_submission(
            route,
            request_json.to_owned(),
            nonce,
            "resend TEE submission",
            true,
        )
        .await
    }

    /// Place one plain limit order (GTC, IOC, or ALO) on the TEE route.
    ///
    /// `client_order_id` defaults to the nonce.
    pub async fn place_tee_perp_limit_order<S>(
        &self,
        signer: &S,
        params: PerpLimitOrderParams,
    ) -> Result<TeePerpOrderResponse>
    where
        S: PerpDirectiveSigner + ?Sized,
    {
        let nonce = self
            .resolve_tee_nonce(signer.address(), params.nonce)
            .await?;
        let action = HlLimitOrderAction {
            asset: params.asset,
            is_buy: matches!(params.side, Side::Buy),
            limit_px: to_perp_units(params.price, "perp limit price")?,
            sz: to_perp_units(params.size, "perp size")?,
            reduce_only: params.reduce_only,
            tif: params.tif,
            cloid: nonzero_cloid(params.client_order_id.unwrap_or(u128::from(nonce)))?,
        };
        let signature = signer
            .sign_perp_limit_order_payload(PerpLimitOrderSignature {
                account: params.account,
                nonce,
                action,
            })
            .await?;
        let request = DirectiveSubmitRequest {
            account: params.account.into_wallet_address(),
            nonce,
            action,
            signature,
        };
        self.post_tee_signed(
            TEE_PERP_ORDER_SUBMIT_ROUTE,
            &request,
            "submit TEE perp order",
        )
        .await
    }

    /// Place a market order: an IOC limit bounded by `slippage_bps`.
    pub async fn place_tee_perp_market_order<S>(
        &self,
        signer: &S,
        params: TeePerpMarketOrderParams,
    ) -> Result<TeePerpOrderResponse>
    where
        S: PerpDirectiveSigner + ?Sized,
    {
        let size_decimals = validate_size_decimals(params.size_decimals)?;
        if params.size.normalize().scale() > size_decimals {
            return Err(ClientError::InvalidInput(format!(
                "size {} has more than {size_decimals} decimals",
                params.size
            )));
        }
        let price = perp_market_limit_price(
            params.reference_price,
            params.side,
            size_decimals,
            params.slippage_bps.unwrap_or(PERP_MARKET_SLIPPAGE_BPS),
        )?;
        self.place_tee_perp_limit_order(
            signer,
            PerpLimitOrderParams {
                account: params.account,
                asset: params.asset,
                side: params.side,
                price,
                size: params.size,
                tif: PerpTimeInForce::Ioc,
                reduce_only: params.reduce_only,
                client_order_id: params.client_order_id,
                nonce: params.nonce,
            },
        )
        .await
    }

    /// Place one native stop-loss or take-profit order (market or limit).
    pub async fn place_tee_perp_trigger_order<S>(
        &self,
        signer: &S,
        params: TeePerpTriggerOrderParams,
    ) -> Result<TeePerpOrderBatchResponse>
    where
        S: TeePerpSigner + ?Sized,
    {
        let (limit_price, is_market) = match params.execution {
            TeePerpTriggerExecution::Market {
                size_decimals,
                slippage_bps,
            } => (
                perp_market_limit_price(
                    params.trigger_price,
                    params.side,
                    size_decimals,
                    slippage_bps.unwrap_or(PERP_MARKET_SLIPPAGE_BPS),
                )?,
                true,
            ),
            TeePerpTriggerExecution::Limit { price } => (price, false),
        };
        let nonce = self
            .resolve_tee_nonce(signer.address(), params.nonce)
            .await?;
        let cloid = match params.client_order_id {
            Some(cloid) => cloid,
            None => tee_perp_batch_cloid(nonce, 0)?,
        };
        let order = tee_perp_trigger_order_wire(
            params.asset,
            params.side,
            params.trigger_price,
            limit_price,
            params.size,
            is_market,
            params.tpsl,
            params.reduce_only,
            cloid,
        )?;
        let signature = signer
            .sign_tee_perp_trigger_order_payload(TeePerpTriggerOrderSignature {
                account: params.account,
                nonce,
                order,
            })
            .await?;
        let request = DirectiveSubmitRequest {
            account: params.account.into_wallet_address(),
            nonce,
            action: order,
            signature,
        };
        self.post_tee_signed(
            TEE_PERP_TRIGGER_ORDER_SUBMIT_ROUTE,
            &request,
            "submit TEE perp trigger order",
        )
        .await
    }

    /// Submit a caller-built order batch (1 to 100 children).
    pub async fn place_tee_perp_order_batch<S>(
        &self,
        signer: &S,
        params: TeePerpOrderBatchParams,
    ) -> Result<TeePerpOrderBatchResponse>
    where
        S: TeePerpSigner + ?Sized,
    {
        validate_batch(&params.batch)?;
        let signature = signer
            .sign_tee_perp_order_batch_payload(TeePerpOrderBatchSignature {
                account: params.account,
                nonce: params.nonce,
                batch: &params.batch,
            })
            .await?;
        let request = DirectiveSubmitRequest {
            account: params.account.into_wallet_address(),
            nonce: params.nonce,
            action: params.batch,
            signature,
        };
        self.post_tee_signed(
            TEE_PERP_ORDER_BATCH_SUBMIT_ROUTE,
            &request,
            "submit TEE perp order batch",
        )
        .await
    }

    /// Place an entry with attached TP/SL exits (`normalTpsl`).
    pub async fn place_tee_perp_bracket_order<S>(
        &self,
        signer: &S,
        params: TeePerpBracketOrderParams,
    ) -> Result<TeePerpOrderBatchResponse>
    where
        S: TeePerpSigner + ?Sized,
    {
        let nonce = self
            .resolve_tee_nonce(signer.address(), params.nonce)
            .await?;
        let batch = build_tee_perp_bracket_batch(
            params.entry,
            params.take_profit,
            params.stop_loss,
            nonce,
        )?;
        self.place_tee_perp_order_batch(
            signer,
            TeePerpOrderBatchParams {
                account: params.account,
                nonce,
                batch,
            },
        )
        .await
    }

    /// Place TP/SL exits on an existing position (`positionTpsl`).
    pub async fn place_tee_perp_position_tpsl<S>(
        &self,
        signer: &S,
        params: TeePerpPositionTpslParams,
    ) -> Result<TeePerpOrderBatchResponse>
    where
        S: TeePerpSigner + ?Sized,
    {
        let nonce = self
            .resolve_tee_nonce(signer.address(), params.nonce)
            .await?;
        let batch = build_tee_perp_position_tpsl_batch(
            params.asset,
            params.position_side,
            params.size,
            params.take_profit,
            params.stop_loss,
            nonce,
        )?;
        self.place_tee_perp_order_batch(
            signer,
            TeePerpOrderBatchParams {
                account: params.account,
                nonce,
                batch,
            },
        )
        .await
    }

    /// Place a scale order: its legs as one ungrouped batch.
    pub async fn place_tee_perp_scale_order<S>(
        &self,
        signer: &S,
        params: TeePerpScaleOrderParams,
    ) -> Result<TeePerpOrderBatchResponse>
    where
        S: TeePerpSigner + ?Sized,
    {
        let nonce = self
            .resolve_tee_nonce(signer.address(), params.nonce)
            .await?;
        let batch = build_tee_perp_scale_batch(
            params.asset,
            params.side,
            &params.legs,
            params.tif,
            params.reduce_only,
            nonce,
        )?;
        self.place_tee_perp_order_batch(
            signer,
            TeePerpOrderBatchParams {
                account: params.account,
                nonce,
                batch,
            },
        )
        .await
    }

    /// Cancel a resting order by Hyperliquid order ID.
    pub async fn cancel_tee_perp_order_by_oid<S>(
        &self,
        signer: &S,
        params: PerpCancelByOidParams,
    ) -> Result<TeePerpVenueActionResponse>
    where
        S: PerpDirectiveSigner + ?Sized,
    {
        if params.order_id == 0 {
            return Err(ClientError::InvalidInput("oid must not be zero".into()));
        }
        let nonce = self
            .resolve_tee_nonce(signer.address(), params.nonce)
            .await?;
        let action = HlCancelByOidAction {
            asset: params.asset,
            oid: params.order_id,
        };
        let signature = signer
            .sign_perp_cancel_by_oid_payload(PerpCancelByOidSignature {
                account: params.account,
                nonce,
                action,
            })
            .await?;
        let request = DirectiveSubmitRequest {
            account: params.account.into_wallet_address(),
            nonce,
            action,
            signature,
        };
        self.post_tee_signed(
            TEE_PERP_CANCEL_BY_OID_SUBMIT_ROUTE,
            &request,
            "submit TEE perp cancel by oid",
        )
        .await
    }

    /// Cancel a resting order by client order ID.
    pub async fn cancel_tee_perp_order_by_cloid<S>(
        &self,
        signer: &S,
        params: PerpCancelByCloidParams,
    ) -> Result<TeePerpCancelResponse>
    where
        S: PerpDirectiveSigner + ?Sized,
    {
        let nonce = self
            .resolve_tee_nonce(signer.address(), params.nonce)
            .await?;
        let action = HlCancelByCloidAction {
            asset: params.asset,
            cloid: nonzero_cloid(params.client_order_id)?,
        };
        let signature = signer
            .sign_perp_cancel_by_cloid_payload(PerpCancelByCloidSignature {
                account: params.account,
                nonce,
                action,
            })
            .await?;
        let request = DirectiveSubmitRequest {
            account: params.account.into_wallet_address(),
            nonce,
            action,
            signature,
        };
        self.post_tee_signed(
            TEE_PERP_CANCEL_SUBMIT_ROUTE,
            &request,
            "submit TEE perp cancel by cloid",
        )
        .await
    }

    /// Change leverage and margin mode for one market. Read the venue's
    /// resulting setting back with [`HyperliquidInfoClient::active_asset_leverage`].
    pub async fn update_tee_perp_leverage<S>(
        &self,
        signer: &S,
        params: TeePerpLeverageParams,
    ) -> Result<TeePerpLeverageResponse>
    where
        S: TeePerpSigner + ?Sized,
    {
        if params.leverage == 0 {
            return Err(ClientError::InvalidInput(
                "leverage must be greater than zero".into(),
            ));
        }
        let nonce = self
            .resolve_tee_nonce(signer.address(), params.nonce)
            .await?;
        let action = TeePerpLeverageAction {
            asset: params.asset,
            is_cross: params.is_cross,
            leverage: params.leverage,
        };
        let signature = signer
            .sign_tee_perp_leverage_payload(TeePerpLeverageSignature {
                account: params.account,
                nonce,
                action,
            })
            .await?;
        let request = DirectiveSubmitRequest {
            account: params.account.into_wallet_address(),
            nonce,
            action,
            signature,
        };
        self.post_tee_signed(
            TEE_PERP_LEVERAGE_SUBMIT_ROUTE,
            &request,
            "submit TEE perp leverage",
        )
        .await
    }

    /// Replace one resting limit order's price, size, side, TIF and
    /// reduce-only flag (Hyperliquid `modify`). The cloid is preserved.
    ///
    /// The engine reserves the larger of the old and new terms until the
    /// outcome is known: `rejected` or `not_submitted` restores the old terms
    /// (the original order still rests); `accepted` moves to the new terms.
    /// An `unknown` stage must be reconciled before modifying again.
    pub async fn modify_tee_perp_order<S>(
        &self,
        signer: &S,
        params: TeePerpModifyParams,
    ) -> Result<TeePerpModifyResponse>
    where
        S: TeePerpSigner + ?Sized,
    {
        let action = params.replacement.to_action()?;
        let nonce = self
            .resolve_tee_nonce(signer.address(), params.nonce)
            .await?;
        let signature = signer
            .sign_tee_perp_modify_payload(TeePerpModifySignature {
                account: params.account,
                nonce,
                action,
            })
            .await?;
        let request = DirectiveSubmitRequest {
            account: params.account.into_wallet_address(),
            nonce,
            action,
            signature,
        };
        self.post_tee_signed(
            TEE_PERP_ORDER_MODIFY_ROUTE,
            &request,
            "submit TEE perp modify",
        )
        .await
    }

    /// Replace several resting limit orders in one signed Hyperliquid
    /// `batchModify` (one nonce, one venue call).
    ///
    /// The engine admits every child or none. After the venue answers, each
    /// child settles on its own: read
    /// [`TeePerpBatchModifyResponse::children`] (or
    /// [`TeePerpBatchModifyResponse::children_to_reconcile`]) rather than only
    /// the whole-action stage, which is `accepted` or `rejected` only when
    /// every child agrees. Charged as one order placement per child against
    /// the rate limit.
    ///
    /// Refused locally, before signing, when the batch is empty, exceeds
    /// `max_children` (50 by default), repeats a target or cloid, or carries an
    /// invalid child. A deployment whose enclave release does not enable batch
    /// modify answers `400 unsupported_action`
    /// ([`ClientError::is_unsupported_action`]).
    pub async fn batch_modify_tee_perp_orders<S>(
        &self,
        signer: &S,
        params: TeePerpBatchModifyParams,
    ) -> Result<TeePerpBatchModifyResponse>
    where
        S: TeePerpSigner + ?Sized,
    {
        let batch = build_tee_perp_batch_modify(&params.replacements, params.max_children)?;
        let nonce = self
            .resolve_tee_nonce(signer.address(), params.nonce)
            .await?;
        let signature = signer
            .sign_tee_perp_batch_modify_payload(TeePerpBatchModifySignature {
                account: params.account,
                nonce,
                batch: &batch,
            })
            .await?;
        let request = DirectiveSubmitRequest {
            account: params.account.into_wallet_address(),
            nonce,
            action: batch,
            signature,
        };
        self.post_tee_signed(
            TEE_PERP_ORDER_BATCH_MODIFY_ROUTE,
            &request,
            "submit TEE perp batch modify",
        )
        .await
    }

    /// Start a native Hyperliquid TWAP (`twapOrder`). The engine reserves
    /// the full size at admission and releases it as fills arrive. Keep
    /// [`TeePerpTwapResponse::twap_id`] to cancel it.
    ///
    /// TWAP is disabled at runtime on deployments whose enclave release does
    /// not enable it; those answer `400 unsupported_action`
    /// ([`ClientError::is_unsupported_action`]).
    pub async fn place_tee_perp_twap_order<S>(
        &self,
        signer: &S,
        params: TeePerpTwapParams,
    ) -> Result<TeePerpTwapResponse>
    where
        S: TeePerpSigner + ?Sized,
    {
        let action = TeePerpTwapAction {
            asset: params.asset,
            is_buy: matches!(params.side, Side::Buy),
            sz: to_perp_units(params.size, "size")?,
            reduce_only: params.reduce_only,
            minutes: params.minutes,
            randomize: params.randomize,
        };
        validate_twap_action(&action)?;
        let nonce = self
            .resolve_tee_nonce(signer.address(), params.nonce)
            .await?;
        let signature = signer
            .sign_tee_perp_twap_payload(TeePerpTwapSignature {
                account: params.account,
                nonce,
                action,
            })
            .await?;
        let request = DirectiveSubmitRequest {
            account: params.account.into_wallet_address(),
            nonce,
            action,
            signature,
        };
        self.post_tee_signed(TEE_PERP_TWAP_SUBMIT_ROUTE, &request, "submit TEE perp TWAP")
            .await
    }

    /// Cancel a running native TWAP (`twapCancel`). Like TWAP submit, this
    /// can answer `400 unsupported_action` where TWAP is disabled.
    pub async fn cancel_tee_perp_twap<S>(
        &self,
        signer: &S,
        params: TeePerpTwapCancelParams,
    ) -> Result<TeePerpVenueActionResponse>
    where
        S: TeePerpSigner + ?Sized,
    {
        if params.twap_id == 0 {
            return Err(ClientError::InvalidInput("twap_id must not be zero".into()));
        }
        let nonce = self
            .resolve_tee_nonce(signer.address(), params.nonce)
            .await?;
        let action = TeePerpTwapCancelAction {
            asset: params.asset,
            twap_id: params.twap_id,
        };
        let signature = signer
            .sign_tee_perp_twap_cancel_payload(TeePerpTwapCancelSignature {
                account: params.account,
                nonce,
                action,
            })
            .await?;
        let request = DirectiveSubmitRequest {
            account: params.account.into_wallet_address(),
            nonce,
            action,
            signature,
        };
        self.post_tee_signed(
            TEE_PERP_TWAP_CANCEL_ROUTE,
            &request,
            "submit TEE perp TWAP cancel",
        )
        .await
    }

    /// Preview an action's admission, margin, liquidation prices and max
    /// size without signing or reserving anything.
    pub async fn preview_tee_perp_action(
        &self,
        request: &PerpPreviewRequest,
    ) -> Result<PerpPreviewResponse> {
        let envelope: ApiResponse<PerpPreviewResponse> = self
            .post_tee(TEE_PERP_PREVIEW_ROUTE, request, "preview TEE perp action")
            .await?;
        if !envelope.success {
            return Err(ClientError::Api {
                status: 200,
                message: envelope
                    .error
                    .unwrap_or_else(|| "TEE perp preview failed".into()),
            });
        }
        envelope.data.ok_or_else(|| ClientError::Api {
            status: 200,
            message: "TEE perp preview returned no data".into(),
        })
    }

    /// Configured TEE perp markets and whether each is supported.
    pub async fn get_tee_perp_markets(&self) -> Result<TeePerpMarketsResponse> {
        Self::send_reqwest_json(
            self.request(Method::GET, TEE_PERP_MARKETS_ROUTE),
            "get TEE perp markets",
        )
        .await
    }

    /// Step 1 of enrollment: authenticate as the manager and fetch the
    /// account's TEE API wallet with its `HLAddApiWallet` approval payload.
    pub async fn request_tee_api_wallet_approval<S>(
        &self,
        manager: &S,
        account: AccountAddress,
    ) -> Result<TeeApiWalletApprovalChallenge>
    where
        S: TeeApiWalletManagerSigner + ?Sized,
    {
        let timestamp = i64::try_from(self.server_now_ms().await? / 1_000).map_err(|_| {
            ClientError::ArithmeticOverflow {
                operation: "authentication timestamp",
            }
        })?;
        let manager_address = manager.address();
        let authentication_signature = manager.sign_authentication_payload(timestamp).await?;
        let request = TeeApiWalletApprovalRequest {
            account: account.into_wallet_address(),
            manager: manager_address.into_wallet_address(),
            timestamp,
            signature: authentication_signature.clone(),
        };
        let package: TeeApiWalletApprovalPackage = self
            .post_tee(
                TEE_API_WALLET_APPROVAL_REQUEST_ROUTE,
                &request,
                "request TEE API wallet approval",
            )
            .await?;
        Ok(TeeApiWalletApprovalChallenge {
            account,
            manager: manager_address,
            timestamp,
            authentication_signature,
            package,
        })
    }

    /// Step 2 of enrollment: sign the approval (after checking the returned
    /// typed data names exactly the packaged wallet) and submit it.
    pub async fn submit_tee_api_wallet_approval<S>(
        &self,
        manager: &S,
        challenge: &TeeApiWalletApprovalChallenge,
    ) -> Result<DirectiveSubmitResponse>
    where
        S: TeeApiWalletManagerSigner + ?Sized,
    {
        validate_approval_typed_data(challenge)?;
        let signature = manager
            .sign_tee_api_wallet_approval_payload(TeeApiWalletApprovalSignature {
                account: challenge.account,
                nonce: challenge.package.nonce,
                api_wallet_address: AccountAddress::from(
                    challenge.package.api_wallet_address.inner(),
                ),
                api_wallet_name: &challenge.package.api_wallet_name,
            })
            .await?;
        let request = TeeApiWalletApprovalSubmitRequest {
            account: challenge.account.into_wallet_address(),
            manager: challenge.manager.into_wallet_address(),
            timestamp: challenge.timestamp,
            authentication_signature: challenge.authentication_signature.clone(),
            tee_wallet_id: challenge.package.tee_wallet_id,
            nonce: challenge.package.nonce,
            signature,
        };
        self.post_tee_signed(
            TEE_API_WALLET_APPROVAL_ROUTE,
            &request,
            "submit TEE API wallet approval",
        )
        .await
    }

    /// Enroll the account's TEE API wallet end to end, as the Hypercall
    /// order panel does: request it, skip the approval when Hyperliquid
    /// already binds it, otherwise submit the approval, poll the directive
    /// until it finalizes, and confirm the binding on Hyperliquid.
    pub async fn enroll_tee_api_wallet<S>(
        &self,
        manager: &S,
        account: AccountAddress,
        hyperliquid: &HyperliquidInfoClient,
        polling: TeeApiWalletEnrollmentPolling,
    ) -> Result<TeeApiWalletEnrollment>
    where
        S: TeeApiWalletManagerSigner + ?Sized,
    {
        let challenge = self
            .request_tee_api_wallet_approval(manager, account)
            .await?;
        let api_wallet = AccountAddress::from(challenge.package.api_wallet_address.inner());
        let api_wallet_name = challenge.package.api_wallet_name.clone();
        if hyperliquid
            .is_tee_api_wallet_active(account, api_wallet)
            .await?
        {
            return Ok(TeeApiWalletEnrollment {
                api_wallet,
                api_wallet_name,
                directive_id: None,
            });
        }
        let response = self
            .submit_tee_api_wallet_approval(manager, &challenge)
            .await?;
        if let Some(rejection) = &response.rejection {
            return Err(ClientError::Api {
                status: 200,
                message: format!(
                    "TEE API wallet approval rejected ({}): {}",
                    rejection.code, rejection.message
                ),
            });
        }
        let mut finalized = false;
        for _ in 0..polling.attempts {
            // The directive can briefly be absent before its row appears.
            if let Ok(status) = self.get_directive_status(&response.directive_id).await {
                match tee_api_wallet_approval_progress(&status) {
                    TeeApiWalletApprovalProgress::Finalized => {
                        finalized = true;
                        break;
                    }
                    TeeApiWalletApprovalProgress::Failed => {
                        return Err(ClientError::Api {
                            status: 200,
                            message: format!(
                                "TEE API wallet approval {} failed: domain {:?}, delivery {:?}",
                                response.directive_id, status.domain_status, status.delivery_status
                            ),
                        });
                    }
                    TeeApiWalletApprovalProgress::Pending => {}
                }
            }
            tokio::time::sleep(polling.interval).await;
        }
        if !finalized {
            return Err(ClientError::Api {
                status: 200,
                message: format!(
                    "TEE API wallet approval {} is still pending",
                    response.directive_id
                ),
            });
        }
        if !hyperliquid
            .is_tee_api_wallet_active(account, api_wallet)
            .await?
        {
            return Err(ClientError::Api {
                status: 200,
                message: "Hyperliquid did not bind the finalized TEE API wallet to this account"
                    .into(),
            });
        }
        Ok(TeeApiWalletEnrollment {
            api_wallet,
            api_wallet_name,
            directive_id: Some(response.directive_id),
        })
    }

    /// Approve the zero-fee Hypercall builder for the account on Hyperliquid
    /// with one manager HyperEVM transaction calling
    /// `Account.approveZeroFeeBuilder(builder)`. Idempotent on the venue.
    ///
    /// Needed once per account before TEE perp orders are accepted (they are
    /// refused with `builder_not_approved` until then). Requires Account
    /// 0.2.4 or newer and a little HYPE on the manager for gas.
    pub async fn approve_hypercall_builder(
        &self,
        manager: &HypercallWallet,
        params: ApproveHypercallBuilderParams,
    ) -> Result<B256> {
        let builder = match params.builder {
            Some(builder) => builder,
            None => resolve_hypercall_builder_address(&self.get_exchange_info().await?)?,
        };
        let builder = builder.as_wallet_address().inner();
        if builder == Address::ZERO {
            return Err(ClientError::InvalidInput("builder must not be zero".into()));
        }
        let rpc_url = params
            .rpc_url
            .parse()
            .map_err(|source| ClientError::invalid_input("HyperEVM RPC URL", source))?;
        let provider = ProviderBuilder::new()
            .wallet(manager.ethereum_wallet())
            .connect_http(rpc_url);
        let rpc_chain_id = provider
            .get_chain_id()
            .await
            .map_err(|error| onchain_error("read HyperEVM chain ID", error))?;
        if rpc_chain_id != manager.chain_id() {
            return Err(ClientError::InvalidInput(format!(
                "HyperEVM RPC chain ID {rpc_chain_id} does not match wallet chain ID {}",
                manager.chain_id()
            )));
        }
        let manager_address = manager.address.as_wallet_address().inner();
        let balance = provider
            .get_balance(manager_address)
            .await
            .map_err(|error| onchain_error("read manager HYPE balance", error))?;
        if balance.is_zero() {
            return Err(ClientError::InvalidInput(
                "the manager wallet needs a small amount of HYPE on HyperEVM to pay the network fee"
                    .into(),
            ));
        }
        let account = HypercallAccountBuilderApproval::new(
            params.account.as_wallet_address().inner(),
            &provider,
        );
        let call = account.approveZeroFeeBuilder(builder).from(manager_address);
        if let Err(error) = call.call().await {
            if is_missing_function_revert(&error) {
                return Err(ClientError::InvalidInput(
                    "this Account implementation predates approveZeroFeeBuilder (needs 0.2.4); trading cannot be enabled yet".into(),
                ));
            }
            return Err(onchain_error("simulate approveZeroFeeBuilder", error));
        }
        let submission_lock = evm_submission_lock(manager_address);
        let _guard = submission_lock.lock().await;
        let receipt = call
            .send()
            .await
            .map_err(|error| onchain_error("submit approveZeroFeeBuilder", error))?
            .with_timeout(Some(Duration::from_secs(60)))
            .get_receipt()
            .await
            .map_err(|error| onchain_error("confirm approveZeroFeeBuilder", error))?;
        if !receipt.status() {
            return Err(ClientError::TransactionReverted {
                operation: "approveZeroFeeBuilder",
                transaction_hash: Some(receipt.transaction_hash),
            });
        }
        Ok(receipt.transaction_hash)
    }
}

#[cfg(test)]
#[path = "tee_perp_test.rs"]
mod tests;

#[cfg(test)]
#[path = "tee_perp_modify_test.rs"]
mod modify_tests;
