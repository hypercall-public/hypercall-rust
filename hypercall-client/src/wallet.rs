//! EIP-712 wallet for signing Hypercall operations.
//!
//! Every write operation on Hypercall requires an EIP-712 signature. This module
//! provides [`HypercallWallet`] which wraps a private key and exposes typed
//! signing methods for each action type.
//!
//! # Two Wallet Types
//!
//! - **Manager wallet**: owns the account. Signs public Hypercall write
//!   requests such as `set_margin_mode`, `place_order`, and `cancel_order`.
//!
//! # Example
//!
//! ```rust,no_run
//! use hypercall_client::{HypercallWallet, PlaceOrderSignature};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let wallet = HypercallWallet::from_private_key(
//!     "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
//!     998,
//! )?;
//! println!("Address: {}", wallet.address);
//!
//! // Sign an options order
//! let sig = wallet.sign_place_order_payload(PlaceOrderSignature {
//!     wallet: wallet.address,
//!     symbol: "BTC-20260501-76000-C",
//!     side: "Buy",
//!     size: "5",
//!     price: "2000",
//!     tif: "ioc",
//!     route: hypercall_sdk_types::OrderRoute::BestExecution,
//!     client_id: "order_1",
//!     reduce_only: false,
//!     nonce: wallet.next_nonce(),
//! }).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Nonce Management
//!
//! Nonces are seeded from epoch-ms at construction time and increment atomically.
//! Each `next_nonce()` call returns a unique value. Signed on-chain actions
//! require each (signer, nonce) pair to be used at most once.

use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[cfg(feature = "kms")]
use alloy::signers::aws::{aws_config, aws_sdk_kms, AwsSigner};
use alloy::{
    primitives::{Signature, B256},
    signers::{local::PrivateKeySigner, Signer},
    sol,
    sol_types::{Eip712Domain, SolStruct},
};
use hypercall_sdk_types::{
    HcUpdateApiWalletAction, HlCancelByCloidAction, HlCancelByOidAction, HlLimitOrderAction,
    HlSetAbstractionAction, OrderRoute, WalletAddress,
};

use crate::error::{ClientError, Result};

/// Hypercall account address used by the public client API.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct AccountAddress(WalletAddress);

impl AccountAddress {
    pub fn as_wallet_address(&self) -> &WalletAddress {
        &self.0
    }

    pub fn into_wallet_address(self) -> WalletAddress {
        self.0
    }

    pub fn as_hex(&self) -> String {
        self.0.as_hex()
    }
}

impl std::ops::Deref for AccountAddress {
    type Target = WalletAddress;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<WalletAddress> for AccountAddress {
    fn from(address: WalletAddress) -> Self {
        Self(address)
    }
}

impl From<&WalletAddress> for AccountAddress {
    fn from(address: &WalletAddress) -> Self {
        Self(*address)
    }
}

impl From<AccountAddress> for WalletAddress {
    fn from(address: AccountAddress) -> Self {
        address.0
    }
}

impl From<&AccountAddress> for AccountAddress {
    fn from(address: &AccountAddress) -> Self {
        *address
    }
}

impl From<alloy::primitives::Address> for AccountAddress {
    fn from(address: alloy::primitives::Address) -> Self {
        Self(WalletAddress::from(address))
    }
}

impl FromStr for AccountAddress {
    type Err = <WalletAddress as FromStr>::Err;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        WalletAddress::from_str(value).map(Self)
    }
}

impl std::fmt::Display for AccountAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Debug for AccountAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

sol! {
    #[derive(Debug, PartialEq, Eq)]
    struct PlaceOrder {
        address wallet;
        string symbol;
        string side;
        string size;
        string price;
        string tif;
        string route;
        string clientId;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct PlaceOrderReduceOnly {
        address wallet;
        string symbol;
        string side;
        string size;
        string price;
        string tif;
        string route;
        string clientId;
        bool reduceOnly;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct CancelOrder {
        address wallet;
        string orderId;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct LimitOrder {
        uint32 asset;
        bool isBuy;
        uint64 limitPx;
        uint64 sz;
        bool reduceOnly;
        uint8 encodedTif;
        uint128 cloid;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct CancelOrderByOid {
        uint32 asset;
        uint64 oid;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct CancelOrderByCloid {
        uint32 asset;
        uint128 cloid;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct HLOrder {
        address account;
        uint64 nonce;
        LimitOrder action;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct HLCancel {
        address account;
        uint64 nonce;
        CancelOrderByOid action;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct HLCancelByCloid {
        address account;
        uint64 nonce;
        CancelOrderByCloid action;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct SetAbstraction {
        address user;
        uint8 abstraction;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct HLSetAbstraction {
        address account;
        uint64 nonce;
        SetAbstraction action;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct UpdateApiWallet {
        bytes32 name;
        address addr;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct HCUpdateApiWallet {
        address account;
        uint64 nonce;
        UpdateApiWallet action;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct SetMarginMode {
        address wallet;
        string marginMode;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct SubmitRFQ {
        bytes32 rfqId;
        bytes32 legsHash;
        address wallet;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct SubmitRFQResponse {
        bytes32 rfqId;
        bytes32 legsHash;
        int256 netPremium;
        uint256 validForMs;
        address qpWallet;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct AcceptRFQQuote {
        bytes32 rfqId;
        bytes32 quoteId;
        int256 netPremium;
        address wallet;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct SubmitAutoExecuteRfq {
        bytes32 rfqId;
        bytes32 legsHash;
        int256 limitPrice;
        address wallet;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ConnectQuoteProvider {
        address wallet;
        uint256 timestamp;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ReplaceOrder {
        address wallet;
        string orderId;
        string symbol;
        string side;
        string size;
        string price;
        string tif;
        string clientId;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ReplaceOrderReduceOnly {
        address wallet;
        string orderId;
        string symbol;
        string side;
        string size;
        string price;
        string tif;
        string clientId;
        bool reduceOnly;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct StandardMarginLiquidationOrder {
        address wallet;
        address liquidatedWallet;
        string requestId;
        string auctionId;
        string bidUsdc;
        string portfolioHash;
        string auctionTermsHash;
        string bidIntentHash;
        uint64 auctionVersion;
        uint64 nonce;
    }
}

fn hypercall_domain(chain_id: u32) -> Eip712Domain {
    Eip712Domain {
        name: Some("Hypercall".into()),
        version: Some("1".into()),
        chain_id: Some(alloy::primitives::U256::from(chain_id)),
        verifying_contract: Some(alloy::primitives::Address::ZERO),
        salt: None,
    }
}

#[derive(Clone)]
enum SignerBackend {
    Local(PrivateKeySigner),
    #[cfg(feature = "kms")]
    AwsKms(AwsSigner),
}

impl SignerBackend {
    fn address(&self) -> AccountAddress {
        match self {
            SignerBackend::Local(signer) => AccountAddress::from(signer.address()),
            #[cfg(feature = "kms")]
            SignerBackend::AwsKms(signer) => AccountAddress::from(signer.address()),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            SignerBackend::Local(_) => "local",
            #[cfg(feature = "kms")]
            SignerBackend::AwsKms(_) => "aws-kms",
        }
    }

    fn set_chain_id(&mut self, chain_id: u32) {
        match self {
            SignerBackend::Local(signer) => signer.set_chain_id(Some(u64::from(chain_id))),
            #[cfg(feature = "kms")]
            SignerBackend::AwsKms(signer) => signer.set_chain_id(Some(u64::from(chain_id))),
        }
    }

    fn ethereum_wallet(&self) -> alloy::network::EthereumWallet {
        match self {
            SignerBackend::Local(signer) => alloy::network::EthereumWallet::from(signer.clone()),
            #[cfg(feature = "kms")]
            SignerBackend::AwsKms(signer) => alloy::network::EthereumWallet::from(signer.clone()),
        }
    }

    async fn sign_typed_data<T: SolStruct + Send + Sync>(
        &self,
        message: &T,
        domain: &Eip712Domain,
    ) -> std::result::Result<Signature, alloy::signers::Error> {
        match self {
            SignerBackend::Local(signer) => signer.sign_typed_data(message, domain).await,
            #[cfg(feature = "kms")]
            SignerBackend::AwsKms(signer) => signer.sign_typed_data(message, domain).await,
        }
    }

    fn try_private_key_hex(&self) -> Result<String> {
        match self {
            SignerBackend::Local(signer) => Ok(format!("0x{}", hex::encode(signer.to_bytes()))),
            #[cfg(feature = "kms")]
            SignerBackend::AwsKms(_) => Err(ClientError::Signing(
                "private key material is unavailable for AWS KMS wallets".to_string(),
            )),
        }
    }
}

/// Wallet for signing Hypercall operations with EIP-712.
#[derive(Clone)]
pub struct HypercallWallet {
    /// The wallet address
    pub address: AccountAddress,
    /// The signer backend
    signer: SignerBackend,
    nonce_provider: Arc<dyn NonceProvider>,
    /// Chain ID for the EIP-712 signing domain. Production uses `999`.
    chain_id: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct PlaceOrderSignature<'a> {
    pub wallet: AccountAddress,
    pub symbol: &'a str,
    pub side: &'a str,
    pub size: &'a str,
    pub price: &'a str,
    pub tif: &'a str,
    pub route: OrderRoute,
    pub client_id: &'a str,
    pub reduce_only: bool,
    pub nonce: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct CancelOrderSignature<'a> {
    pub wallet: AccountAddress,
    pub order_id: &'a str,
    pub nonce: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct ReplaceOrderSignature<'a> {
    pub wallet: AccountAddress,
    pub order_id: &'a str,
    pub symbol: &'a str,
    pub side: &'a str,
    pub size: &'a str,
    pub price: &'a str,
    pub tif: &'a str,
    pub client_id: &'a str,
    pub reduce_only: bool,
    pub nonce: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct StandardMarginLiquidationSignature<'a> {
    pub wallet: AccountAddress,
    pub liquidated_wallet: AccountAddress,
    pub request_id: &'a str,
    pub auction_id: &'a str,
    pub bid_usdc: &'a str,
    pub portfolio_hash: &'a str,
    pub auction_terms_hash: &'a str,
    pub bid_intent_hash: &'a str,
    pub auction_version: u64,
    pub nonce: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct PerpLimitOrderSignature {
    pub account: AccountAddress,
    pub nonce: u64,
    pub action: HlLimitOrderAction,
}

#[derive(Debug, Clone, Copy)]
pub struct PerpCancelByOidSignature {
    pub account: AccountAddress,
    pub nonce: u64,
    pub action: HlCancelByOidAction,
}

#[derive(Debug, Clone, Copy)]
pub struct PerpCancelByCloidSignature {
    pub account: AccountAddress,
    pub nonce: u64,
    pub action: HlCancelByCloidAction,
}

#[derive(Debug, Clone, Copy)]
pub struct SetAccountAbstractionSignature {
    pub account: AccountAddress,
    pub nonce: u64,
    pub action: HlSetAbstractionAction,
}

#[derive(Debug, Clone, Copy)]
pub struct UpdateApiWalletSignature {
    pub account: AccountAddress,
    pub nonce: u64,
    pub action: HcUpdateApiWalletAction,
}

#[allow(async_fn_in_trait)]
pub trait HypercallSigner {
    fn address(&self) -> AccountAddress;
    fn next_nonce(&self) -> u64;
    async fn sign_place_order_payload(&self, payload: PlaceOrderSignature<'_>) -> Result<String>;
    async fn sign_cancel_order_payload(&self, payload: CancelOrderSignature<'_>) -> Result<String>;
    async fn sign_replace_order_payload(
        &self,
        payload: ReplaceOrderSignature<'_>,
    ) -> Result<String>;
    async fn sign_set_margin_mode_payload(&self, margin_mode: &str, nonce: u64) -> Result<String>;
    async fn sign_standard_margin_liquidation_payload(
        &self,
        payload: StandardMarginLiquidationSignature<'_>,
    ) -> Result<String>;
}

#[allow(async_fn_in_trait)]
pub trait PerpDirectiveSigner: HypercallSigner {
    async fn sign_perp_limit_order_payload(
        &self,
        payload: PerpLimitOrderSignature,
    ) -> Result<String>;
    async fn sign_perp_cancel_by_oid_payload(
        &self,
        payload: PerpCancelByOidSignature,
    ) -> Result<String>;
    async fn sign_perp_cancel_by_cloid_payload(
        &self,
        payload: PerpCancelByCloidSignature,
    ) -> Result<String>;
}

#[allow(async_fn_in_trait)]
pub trait ManagerDirectiveSigner: HypercallSigner {
    async fn sign_set_account_abstraction_payload(
        &self,
        payload: SetAccountAbstractionSignature,
    ) -> Result<String>;
}

#[allow(async_fn_in_trait)]
pub trait ApiWalletDirectiveSigner: HypercallSigner {
    async fn sign_update_api_wallet_payload(
        &self,
        payload: UpdateApiWalletSignature,
    ) -> Result<String>;
}

fn epoch_seeded_nonce() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

pub trait NonceProvider: Send + Sync {
    fn next_nonce(&self) -> u64;
    fn current_nonce(&self) -> u64;
}

#[derive(Debug)]
pub struct AtomicNonceProvider {
    counter: AtomicU64,
}

impl AtomicNonceProvider {
    pub fn new(seed: u64) -> Self {
        Self {
            counter: AtomicU64::new(seed),
        }
    }

    pub fn epoch_seeded() -> Self {
        Self::new(epoch_seeded_nonce())
    }
}

impl Default for AtomicNonceProvider {
    fn default() -> Self {
        Self::epoch_seeded()
    }
}

impl NonceProvider for AtomicNonceProvider {
    fn next_nonce(&self) -> u64 {
        let now = epoch_seeded_nonce();
        self.counter.fetch_max(now, Ordering::SeqCst);
        self.counter.fetch_add(1, Ordering::SeqCst)
    }

    fn current_nonce(&self) -> u64 {
        self.counter.load(Ordering::SeqCst)
    }
}

impl HypercallSigner for HypercallWallet {
    fn address(&self) -> AccountAddress {
        self.address
    }

    fn next_nonce(&self) -> u64 {
        HypercallWallet::next_nonce(self)
    }

    async fn sign_place_order_payload(&self, payload: PlaceOrderSignature<'_>) -> Result<String> {
        HypercallWallet::sign_place_order_payload(self, payload).await
    }

    async fn sign_cancel_order_payload(&self, payload: CancelOrderSignature<'_>) -> Result<String> {
        HypercallWallet::sign_cancel_order_payload(self, payload).await
    }

    async fn sign_replace_order_payload(
        &self,
        payload: ReplaceOrderSignature<'_>,
    ) -> Result<String> {
        HypercallWallet::sign_replace_order_payload(self, payload).await
    }

    async fn sign_set_margin_mode_payload(&self, margin_mode: &str, nonce: u64) -> Result<String> {
        self.sign_set_margin_mode(margin_mode, nonce).await
    }

    async fn sign_standard_margin_liquidation_payload(
        &self,
        payload: StandardMarginLiquidationSignature<'_>,
    ) -> Result<String> {
        HypercallWallet::sign_standard_margin_liquidation_payload(self, payload).await
    }
}

impl PerpDirectiveSigner for HypercallWallet {
    async fn sign_perp_limit_order_payload(
        &self,
        payload: PerpLimitOrderSignature,
    ) -> Result<String> {
        HypercallWallet::sign_perp_limit_order_payload(self, payload).await
    }

    async fn sign_perp_cancel_by_oid_payload(
        &self,
        payload: PerpCancelByOidSignature,
    ) -> Result<String> {
        HypercallWallet::sign_perp_cancel_by_oid_payload(self, payload).await
    }

    async fn sign_perp_cancel_by_cloid_payload(
        &self,
        payload: PerpCancelByCloidSignature,
    ) -> Result<String> {
        HypercallWallet::sign_perp_cancel_by_cloid_payload(self, payload).await
    }
}

impl ManagerDirectiveSigner for HypercallWallet {
    async fn sign_set_account_abstraction_payload(
        &self,
        payload: SetAccountAbstractionSignature,
    ) -> Result<String> {
        HypercallWallet::sign_set_account_abstraction_payload(self, payload).await
    }
}

impl ApiWalletDirectiveSigner for HypercallWallet {
    async fn sign_update_api_wallet_payload(
        &self,
        payload: UpdateApiWalletSignature,
    ) -> Result<String> {
        HypercallWallet::sign_update_api_wallet_payload(self, payload).await
    }
}

impl HypercallWallet {
    pub(crate) fn ethereum_wallet(&self) -> alloy::network::EthereumWallet {
        self.signer.ethereum_wallet()
    }

    pub(crate) fn chain_id(&self) -> u64 {
        u64::from(self.chain_id)
    }

    /// Create a new wallet from a private key.
    ///
    /// # Arguments
    /// * `private_key` - Hex-encoded private key (with or without 0x prefix)
    /// * `chain_id` - Chain ID for the EIP-712 signing domain. Production uses `999`.
    pub fn from_private_key(private_key: &str, chain_id: u32) -> Result<Self> {
        let signer = PrivateKeySigner::from_str(private_key)
            .map_err(|e| ClientError::Signing(format!("Invalid private key: {}", e)))?;

        let signer = SignerBackend::Local(signer);
        let address = signer.address();

        Ok(Self {
            address,
            signer,
            nonce_provider: Arc::new(AtomicNonceProvider::default()),
            chain_id,
        })
    }

    /// Create a new random wallet (useful for testing).
    pub fn random(chain_id: u32) -> Self {
        let signer = PrivateKeySigner::random();
        let signer = SignerBackend::Local(signer);
        let address = signer.address();

        Self {
            address,
            signer,
            nonce_provider: Arc::new(AtomicNonceProvider::default()),
            chain_id,
        }
    }

    /// Create a wallet from raw private key bytes.
    pub fn from_bytes(bytes: &[u8; 32], chain_id: u32) -> Result<Self> {
        let b256_key = B256::from_slice(bytes);
        let signer = PrivateKeySigner::from_bytes(&b256_key)
            .map_err(|e| ClientError::Signing(format!("Invalid private key bytes: {}", e)))?;

        let signer = SignerBackend::Local(signer);
        let address = signer.address();

        Ok(Self {
            address,
            signer,
            nonce_provider: Arc::new(AtomicNonceProvider::default()),
            chain_id,
        })
    }

    /// Set the chain ID for EIP-712 signing domain.
    pub fn with_chain_id(mut self, chain_id: u32) -> Self {
        self.chain_id = chain_id;
        self.signer.set_chain_id(chain_id);
        self
    }

    /// Return a copy of this wallet that obtains nonces from `nonce_provider`.
    pub fn with_nonce_provider(mut self, nonce_provider: Arc<dyn NonceProvider>) -> Self {
        self.nonce_provider = nonce_provider;
        self
    }

    /// Create a wallet backed by an AWS KMS key using the AWS SDK default credential chain.
    ///
    /// The key ID may be a key ID, key ARN, alias name, or alias ARN accepted by KMS.
    /// Credentials and region are resolved by the AWS SDK default provider chain.
    #[cfg(feature = "kms")]
    pub async fn from_aws_kms_key_id(key_id: impl Into<String>, chain_id: u32) -> Result<Self> {
        let key_id = key_id.into();
        let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let kms_client = aws_sdk_kms::Client::new(&aws_config);
        let signer = AwsSigner::new(kms_client, key_id, Some(u64::from(chain_id)))
            .await
            .map_err(|error| {
                ClientError::Signing(format!("Failed to initialize AWS KMS wallet: {error}"))
            })?;
        let signer = SignerBackend::AwsKms(signer);
        let address = signer.address();

        Ok(Self {
            address,
            signer,
            nonce_provider: Arc::new(AtomicNonceProvider::default()),
            chain_id,
        })
    }

    /// Get the next nonce for signing.
    /// Clamps to at least the current epoch millisecond so long-lived
    /// processes don't drift outside the engine's time bounds.
    pub fn next_nonce(&self) -> u64 {
        self.nonce_provider.next_nonce()
    }

    /// Get the current nonce without incrementing.
    pub fn current_nonce(&self) -> u64 {
        self.nonce_provider.current_nonce()
    }

    /// Try to get the private key as a hex string.
    ///
    /// This only succeeds for local private-key wallets. AWS KMS wallets never
    /// expose private key material.
    pub fn try_private_key_hex(&self) -> Result<String> {
        self.signer.try_private_key_hex()
    }

    /// Get the private key as a hex string for local private-key wallets.
    ///
    /// Panics when called on non-local wallets because private key material is
    /// unavailable. Prefer [`Self::try_private_key_hex`] when wallet backend is
    /// not statically known.
    pub fn private_key_hex(&self) -> String {
        self.try_private_key_hex()
            .expect("private_key_hex is only available for local private-key wallets")
    }

    /// Get the Hypercall EIP-712 domain.
    fn hypercall_domain(&self) -> Eip712Domain {
        hypercall_domain(self.chain_id)
    }

    fn directive_domain(&self, name: &'static str) -> Result<Eip712Domain> {
        if !matches!(self.chain_id, 998 | 999) {
            return Err(ClientError::InvalidInput(format!(
                "unsupported directive chain id: {}",
                self.chain_id
            )));
        }
        Ok(Eip712Domain {
            name: Some(name.into()),
            version: Some("1".into()),
            chain_id: Some(alloy::primitives::U256::from(self.chain_id)),
            verifying_contract: Some(alloy::primitives::Address::ZERO),
            salt: None,
        })
    }

    pub async fn sign_perp_limit_order_payload(
        &self,
        payload: PerpLimitOrderSignature,
    ) -> Result<String> {
        let action = payload.action;
        let message = HLOrder {
            account: payload.account.as_wallet_address().inner(),
            nonce: payload.nonce,
            action: LimitOrder {
                asset: action.asset,
                isBuy: action.is_buy,
                limitPx: action.limit_px,
                sz: action.sz,
                reduceOnly: action.reduce_only,
                encodedTif: action.tif.encoded(),
                cloid: action.cloid,
            },
        };
        self.sign_directive(&message, "HLOrder", "HypercallApiSign")
            .await
    }

    pub async fn sign_perp_cancel_by_oid_payload(
        &self,
        payload: PerpCancelByOidSignature,
    ) -> Result<String> {
        let message = HLCancel {
            account: payload.account.as_wallet_address().inner(),
            nonce: payload.nonce,
            action: CancelOrderByOid {
                asset: payload.action.asset,
                oid: payload.action.oid,
            },
        };
        self.sign_directive(&message, "HLCancel", "HypercallApiSign")
            .await
    }

    pub async fn sign_perp_cancel_by_cloid_payload(
        &self,
        payload: PerpCancelByCloidSignature,
    ) -> Result<String> {
        let message = HLCancelByCloid {
            account: payload.account.as_wallet_address().inner(),
            nonce: payload.nonce,
            action: CancelOrderByCloid {
                asset: payload.action.asset,
                cloid: payload.action.cloid,
            },
        };
        self.sign_directive(&message, "HLCancelByCloid", "HypercallApiSign")
            .await
    }

    pub async fn sign_set_account_abstraction_payload(
        &self,
        payload: SetAccountAbstractionSignature,
    ) -> Result<String> {
        let message = HLSetAbstraction {
            account: payload.account.as_wallet_address().inner(),
            nonce: payload.nonce,
            action: SetAbstraction {
                user: payload.action.user.inner(),
                abstraction: payload.action.abstraction.as_u8(),
            },
        };
        self.sign_directive(&message, "HLSetAbstraction", "HypercallManagerSign")
            .await
    }

    pub async fn sign_update_api_wallet_payload(
        &self,
        payload: UpdateApiWalletSignature,
    ) -> Result<String> {
        let message = HCUpdateApiWallet {
            account: payload.account.as_wallet_address().inner(),
            nonce: payload.nonce,
            action: UpdateApiWallet {
                name: payload.action.name,
                addr: payload.action.addr.inner(),
            },
        };
        self.sign_directive(&message, "HCUpdateApiWallet", "HypercallManagerSign")
            .await
    }

    async fn sign_directive<T>(
        &self,
        message: &T,
        type_name: &str,
        domain_name: &'static str,
    ) -> Result<String>
    where
        T: SolStruct + Send + Sync,
    {
        let domain = self.directive_domain(domain_name)?;
        let signature = self
            .signer
            .sign_typed_data(message, &domain)
            .await
            .map_err(|error| {
                ClientError::Signing(format!("Failed to sign {type_name}: {error}"))
            })?;
        Ok(format!("{signature}"))
    }

    /// Sign a PlaceOrder action.
    pub async fn sign_place_order_payload(
        &self,
        payload: PlaceOrderSignature<'_>,
    ) -> Result<String> {
        let domain = self.hypercall_domain();
        let signature = if payload.reduce_only {
            let message = PlaceOrderReduceOnly {
                wallet: payload.wallet.as_wallet_address().inner(),
                symbol: payload.symbol.to_string(),
                side: payload.side.to_string(),
                size: payload.size.to_string(),
                price: payload.price.to_string(),
                tif: payload.tif.to_string(),
                route: payload.route.as_signed_str().to_string(),
                clientId: payload.client_id.to_string(),
                reduceOnly: true,
                nonce: payload.nonce,
            };
            self.signer.sign_typed_data(&message, &domain).await
        } else {
            let message = PlaceOrder {
                wallet: payload.wallet.as_wallet_address().inner(),
                symbol: payload.symbol.to_string(),
                side: payload.side.to_string(),
                size: payload.size.to_string(),
                price: payload.price.to_string(),
                tif: payload.tif.to_string(),
                route: payload.route.as_signed_str().to_string(),
                clientId: payload.client_id.to_string(),
                nonce: payload.nonce,
            };
            self.signer.sign_typed_data(&message, &domain).await
        }
        .map_err(|e| ClientError::Signing(format!("Failed to sign PlaceOrder: {}", e)))?;

        Ok(format!("{}", signature))
    }

    /// Sign a CancelOrder action.
    pub async fn sign_cancel_order(&self, order_id: &str, nonce: u64) -> Result<String> {
        self.sign_cancel_order_payload(CancelOrderSignature {
            wallet: self.address,
            order_id,
            nonce,
        })
        .await
    }

    pub async fn sign_cancel_order_payload(
        &self,
        payload: CancelOrderSignature<'_>,
    ) -> Result<String> {
        let message = CancelOrder {
            wallet: payload.wallet.as_wallet_address().inner(),
            orderId: payload.order_id.to_string(),
            nonce: payload.nonce,
        };

        let domain = self.hypercall_domain();
        let signature = self
            .signer
            .sign_typed_data(&message, &domain)
            .await
            .map_err(|e| ClientError::Signing(format!("Failed to sign CancelOrder: {}", e)))?;

        Ok(format!("{}", signature))
    }

    /// Sign a ReplaceOrder action.
    pub async fn sign_replace_order_payload(
        &self,
        payload: ReplaceOrderSignature<'_>,
    ) -> Result<String> {
        let domain = self.hypercall_domain();
        let signature = if payload.reduce_only {
            let message = ReplaceOrderReduceOnly {
                wallet: payload.wallet.as_wallet_address().inner(),
                orderId: payload.order_id.to_string(),
                symbol: payload.symbol.to_string(),
                side: payload.side.to_string(),
                size: payload.size.to_string(),
                price: payload.price.to_string(),
                tif: payload.tif.to_string(),
                clientId: payload.client_id.to_string(),
                reduceOnly: true,
                nonce: payload.nonce,
            };
            self.signer.sign_typed_data(&message, &domain).await
        } else {
            let message = ReplaceOrder {
                wallet: payload.wallet.as_wallet_address().inner(),
                orderId: payload.order_id.to_string(),
                symbol: payload.symbol.to_string(),
                side: payload.side.to_string(),
                size: payload.size.to_string(),
                price: payload.price.to_string(),
                tif: payload.tif.to_string(),
                clientId: payload.client_id.to_string(),
                nonce: payload.nonce,
            };
            self.signer.sign_typed_data(&message, &domain).await
        }
        .map_err(|e| ClientError::Signing(format!("Failed to sign ReplaceOrder: {}", e)))?;

        Ok(format!("{}", signature))
    }

    pub async fn sign_standard_margin_liquidation_payload(
        &self,
        payload: StandardMarginLiquidationSignature<'_>,
    ) -> Result<String> {
        let message = StandardMarginLiquidationOrder {
            wallet: payload.wallet.as_wallet_address().inner(),
            liquidatedWallet: payload.liquidated_wallet.as_wallet_address().inner(),
            requestId: payload.request_id.to_string(),
            auctionId: payload.auction_id.to_string(),
            bidUsdc: payload.bid_usdc.to_string(),
            portfolioHash: payload.portfolio_hash.to_string(),
            auctionTermsHash: payload.auction_terms_hash.to_string(),
            bidIntentHash: payload.bid_intent_hash.to_string(),
            auctionVersion: payload.auction_version,
            nonce: payload.nonce,
        };

        let domain = self.hypercall_domain();
        let signature = self
            .signer
            .sign_typed_data(&message, &domain)
            .await
            .map_err(|e| {
                ClientError::Signing(format!(
                    "Failed to sign StandardMarginLiquidationOrder: {}",
                    e
                ))
            })?;

        Ok(format!("{}", signature))
    }

    /// Sign a SetMarginMode action.
    pub async fn sign_set_margin_mode(&self, margin_mode: &str, nonce: u64) -> Result<String> {
        let message = SetMarginMode {
            wallet: self.address.as_wallet_address().inner(),
            marginMode: margin_mode.to_string(),
            nonce,
        };

        let domain = self.hypercall_domain();
        let signature = self
            .signer
            .sign_typed_data(&message, &domain)
            .await
            .map_err(|e| ClientError::Signing(format!("Failed to sign SetMarginMode: {}", e)))?;

        Ok(format!("{}", signature))
    }

    /// Sign a SubmitRFQ action.
    pub async fn sign_submit_rfq(
        &self,
        rfq_id: [u8; 32],
        legs_hash: [u8; 32],
        nonce: u64,
    ) -> Result<String> {
        let message = SubmitRFQ {
            rfqId: alloy::primitives::FixedBytes(rfq_id),
            legsHash: alloy::primitives::FixedBytes(legs_hash),
            wallet: self.address.as_wallet_address().inner(),
            nonce,
        };

        let domain = self.hypercall_domain();
        let signature = self
            .signer
            .sign_typed_data(&message, &domain)
            .await
            .map_err(|e| ClientError::Signing(format!("Failed to sign SubmitRFQ: {}", e)))?;

        Ok(format!("{}", signature))
    }

    /// Sign a SubmitRFQResponse action (for quote providers).
    pub async fn sign_submit_rfq_response(
        &self,
        rfq_id: [u8; 32],
        legs_hash: [u8; 32],
        net_premium: alloy::primitives::I256,
        valid_for_ms: alloy::primitives::U256,
        nonce: u64,
    ) -> Result<String> {
        let message = SubmitRFQResponse {
            rfqId: alloy::primitives::FixedBytes(rfq_id),
            legsHash: alloy::primitives::FixedBytes(legs_hash),
            netPremium: net_premium,
            validForMs: valid_for_ms,
            qpWallet: self.address.as_wallet_address().inner(),
            nonce,
        };

        let domain = self.hypercall_domain();
        let signature = self
            .signer
            .sign_typed_data(&message, &domain)
            .await
            .map_err(|e| {
                ClientError::Signing(format!("Failed to sign SubmitRFQResponse: {}", e))
            })?;

        Ok(format!("{}", signature))
    }

    /// Sign an AcceptRFQQuote action.
    pub async fn sign_accept_rfq_quote(
        &self,
        rfq_id: [u8; 32],
        quote_id: [u8; 32],
        net_premium: alloy::primitives::I256,
        nonce: u64,
    ) -> Result<String> {
        let message = AcceptRFQQuote {
            rfqId: alloy::primitives::FixedBytes(rfq_id),
            quoteId: alloy::primitives::FixedBytes(quote_id),
            netPremium: net_premium,
            wallet: self.address.as_wallet_address().inner(),
            nonce,
        };

        let domain = self.hypercall_domain();
        let signature = self
            .signer
            .sign_typed_data(&message, &domain)
            .await
            .map_err(|e| ClientError::Signing(format!("Failed to sign AcceptRFQQuote: {}", e)))?;

        Ok(format!("{}", signature))
    }

    /// Sign a SubmitAutoExecuteRfq action.
    pub async fn sign_submit_auto_execute_rfq(
        &self,
        rfq_id: [u8; 32],
        legs_hash: [u8; 32],
        limit_price: alloy::primitives::I256,
        nonce: u64,
    ) -> Result<String> {
        let message = SubmitAutoExecuteRfq {
            rfqId: alloy::primitives::FixedBytes(rfq_id),
            legsHash: alloy::primitives::FixedBytes(legs_hash),
            limitPrice: limit_price,
            wallet: self.address.as_wallet_address().inner(),
            nonce,
        };

        let domain = self.hypercall_domain();
        let signature = self
            .signer
            .sign_typed_data(&message, &domain)
            .await
            .map_err(|e| {
                ClientError::Signing(format!("Failed to sign SubmitAutoExecuteRfq: {}", e))
            })?;

        Ok(format!("{}", signature))
    }

    /// Sign a ConnectQuoteProvider action (for QP WebSocket auth).
    pub async fn sign_connect_quote_provider(
        &self,
        timestamp: alloy::primitives::U256,
        nonce: u64,
    ) -> Result<String> {
        let message = ConnectQuoteProvider {
            wallet: self.address.as_wallet_address().inner(),
            timestamp,
            nonce,
        };

        let domain = self.hypercall_domain();
        let signature = self
            .signer
            .sign_typed_data(&message, &domain)
            .await
            .map_err(|e| {
                ClientError::Signing(format!("Failed to sign ConnectQuoteProvider: {}", e))
            })?;

        Ok(format!("{}", signature))
    }
}

impl std::fmt::Debug for HypercallWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HypercallWallet")
            .field("address", &self.address)
            .field("signer_backend", &self.signer.kind())
            .field("nonce", &self.current_nonce())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::Address;

    // Well-known test private key (DO NOT use in production)
    const TEST_PRIVATE_KEY: &str =
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    fn recover_address_from_signature(signature: &str, message_hash: &[u8; 32]) -> Address {
        let signature_bytes = hex::decode(signature.trim_start_matches("0x")).unwrap();
        assert_eq!(signature_bytes.len(), 65);

        let parity = match signature_bytes[64] {
            0 | 27 => false,
            1 | 28 => true,
            v => panic!("invalid signature v value: {v}"),
        };
        let sig =
            alloy::primitives::Signature::from_bytes_and_parity(&signature_bytes[..64], parity);

        sig.recover_address_from_prehash(&alloy::primitives::B256::from(*message_hash))
            .unwrap()
    }

    fn recover_typed_signer<T: SolStruct>(
        message: &T,
        domain: &Eip712Domain,
        signature: &str,
    ) -> Address {
        let message_hash = message.eip712_signing_hash(domain);
        recover_address_from_signature(signature, &message_hash)
    }

    fn place_signature(wallet: AccountAddress, nonce: u64) -> PlaceOrderSignature<'static> {
        PlaceOrderSignature {
            wallet,
            symbol: "BTC-20250131-100000-C",
            side: "Buy",
            size: "1",
            price: "100",
            tif: "gtc",
            route: OrderRoute::BestExecution,
            client_id: "cli_1",
            reduce_only: false,
            nonce,
        }
    }

    fn replace_signature(wallet: AccountAddress, nonce: u64) -> ReplaceOrderSignature<'static> {
        ReplaceOrderSignature {
            wallet,
            order_id: "42",
            symbol: "BTC-20250131-100000-C",
            side: "Buy",
            size: "1",
            price: "100",
            tif: "gtc",
            client_id: "cli_replace",
            reduce_only: false,
            nonce,
        }
    }

    #[test]
    fn test_random_wallet() {
        let wallet = HypercallWallet::random(998);
        assert_ne!(
            wallet.address.as_hex(),
            "0x0000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn test_from_private_key() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        // This is the known address for this private key (hardhat account 0)
        assert_eq!(
            wallet.address.as_hex().to_lowercase(),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
    }

    #[test]
    fn test_from_private_key_without_prefix() {
        let key_without_prefix = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
        let wallet = HypercallWallet::from_private_key(key_without_prefix, 998).unwrap();
        assert_eq!(
            wallet.address.as_hex().to_lowercase(),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
    }

    #[test]
    fn test_from_private_key_invalid() {
        let result = HypercallWallet::from_private_key("invalid_key", 998);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_bytes() {
        let bytes: [u8; 32] = [
            0xac, 0x09, 0x74, 0xbe, 0xc3, 0x9a, 0x17, 0xe3, 0x6b, 0xa4, 0xa6, 0xb4, 0xd2, 0x38,
            0xff, 0x94, 0x4b, 0xac, 0xb4, 0x78, 0xcb, 0xed, 0x5e, 0xfc, 0xae, 0x78, 0x4d, 0x7b,
            0xf4, 0xf2, 0xff, 0x80,
        ];
        let wallet = HypercallWallet::from_bytes(&bytes, 998).unwrap();
        assert_eq!(
            wallet.address.as_hex().to_lowercase(),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
    }

    #[test]
    fn test_nonce_management() {
        let wallet = HypercallWallet::random(998);

        // Nonce is seeded with the current epoch-ms at construction time
        // (see `epoch_seeded_nonce`) to avoid collisions when a smoke-test
        // run re-creates a wallet that already signed on a previous run.
        let initial = wallet.current_nonce();
        assert!(initial > 1, "expected epoch-seeded nonce, got {initial}");

        // next_nonce returns a unique increasing value. It may clamp to a
        // newer epoch-ms first if the clock advances after construction.
        let first = wallet.next_nonce();
        assert!(first >= initial);
        assert_eq!(wallet.current_nonce(), first + 1);

        let second = wallet.next_nonce();
        assert!(second > first);
        assert_eq!(wallet.current_nonce(), second + 1);
    }

    #[test]
    fn test_private_key_hex() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let hex = wallet.private_key_hex();
        assert!(hex.starts_with("0x"));
        assert_eq!(hex.len(), 66); // 0x + 64 hex chars
        assert_eq!(wallet.try_private_key_hex().unwrap(), hex);
    }

    #[test]
    fn test_wallet_debug() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let debug = format!("{:?}", wallet);
        assert!(debug.contains("HypercallWallet"));
        assert!(debug.contains("address"));
        assert!(debug.contains("signer_backend"));
        assert!(debug.contains("nonce"));
        assert!(!debug.contains(TEST_PRIVATE_KEY));
        assert!(!debug.contains(TEST_PRIVATE_KEY.trim_start_matches("0x")));
    }

    #[cfg(feature = "kms")]
    #[test]
    fn test_aws_kms_constructor_is_exposed_without_polling() {
        fn assert_send<T: Send>(_: T) {}

        let future = HypercallWallet::from_aws_kms_key_id("alias/hypercall-test", 998);
        assert_send(future);
    }

    #[test]
    fn test_wallet_clone() {
        let wallet1 = HypercallWallet::random(998);
        let wallet2 = wallet1.clone();

        assert_eq!(wallet1.address, wallet2.address);
        // Note: nonce counters share the same Arc
    }

    #[tokio::test]
    async fn test_sign_place_order_includes_route() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let nonce = 7;

        let signature = wallet
            .sign_place_order_payload(PlaceOrderSignature {
                wallet: wallet.address,
                symbol: "BTC-20250131-100000-C",
                side: "Buy",
                size: "1",
                price: "100",
                tif: "gtc",
                route: OrderRoute::BookOnly,
                client_id: "cli_1",
                reduce_only: false,
                nonce,
            })
            .await
            .unwrap();

        let message = PlaceOrder {
            wallet: wallet.address.as_wallet_address().inner(),
            symbol: "BTC-20250131-100000-C".to_string(),
            side: "Buy".to_string(),
            size: "1".to_string(),
            price: "100".to_string(),
            tif: "gtc".to_string(),
            route: "book_only".to_string(),
            clientId: "cli_1".to_string(),
            nonce,
        };
        let recovered = recover_typed_signer(&message, &wallet.hypercall_domain(), &signature);
        assert_eq!(recovered, wallet.address.as_wallet_address().inner());
    }

    #[tokio::test]
    async fn test_sign_place_order_reduce_only_uses_reduce_only_payload() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let nonce = 8;

        let signature = wallet
            .sign_place_order_payload(PlaceOrderSignature {
                wallet: wallet.address,
                symbol: "BTC-20250131-100000-C",
                side: "Sell",
                size: "1",
                price: "100",
                tif: "gtc",
                route: OrderRoute::BookOnly,
                client_id: "cli_ro",
                reduce_only: true,
                nonce,
            })
            .await
            .unwrap();

        let reduce_only_message = PlaceOrderReduceOnly {
            wallet: wallet.address.as_wallet_address().inner(),
            symbol: "BTC-20250131-100000-C".to_string(),
            side: "Sell".to_string(),
            size: "1".to_string(),
            price: "100".to_string(),
            tif: "gtc".to_string(),
            route: "book_only".to_string(),
            clientId: "cli_ro".to_string(),
            reduceOnly: true,
            nonce,
        };
        let recovered =
            recover_typed_signer(&reduce_only_message, &wallet.hypercall_domain(), &signature);
        assert_eq!(recovered, wallet.address.as_wallet_address().inner());

        let old_message = PlaceOrder {
            wallet: wallet.address.as_wallet_address().inner(),
            symbol: "BTC-20250131-100000-C".to_string(),
            side: "Sell".to_string(),
            size: "1".to_string(),
            price: "100".to_string(),
            tif: "gtc".to_string(),
            route: "book_only".to_string(),
            clientId: "cli_ro".to_string(),
            nonce,
        };
        let old_recovered =
            recover_typed_signer(&old_message, &wallet.hypercall_domain(), &signature);
        assert_ne!(old_recovered, wallet.address.as_wallet_address().inner());
    }

    #[tokio::test]
    async fn test_sign_cancel_order() {
        let wallet = HypercallWallet::random(998);
        let nonce = wallet.next_nonce();

        let signature = wallet.sign_cancel_order("12345", nonce).await.unwrap();
        assert!(!signature.is_empty());
    }

    #[tokio::test]
    async fn test_sign_set_margin_mode() {
        let wallet = HypercallWallet::random(998);
        let nonce = wallet.next_nonce();

        let signature = wallet
            .sign_set_margin_mode("portfolio", nonce)
            .await
            .unwrap();

        assert!(!signature.is_empty());
    }

    #[tokio::test]
    async fn test_signatures_are_deterministic() {
        // Same wallet + same params should produce same signature
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();

        let sig1 = wallet
            .sign_place_order_payload(place_signature(wallet.address, 1))
            .await
            .unwrap();

        let sig2 = wallet
            .sign_place_order_payload(place_signature(wallet.address, 1))
            .await
            .unwrap();

        assert_eq!(sig1, sig2);
    }

    #[tokio::test]
    async fn test_sign_submit_rfq() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let rfq_id = [1u8; 32];
        let legs_hash = [2u8; 32];
        let nonce = 1u64;

        let signature = wallet
            .sign_submit_rfq(rfq_id, legs_hash, nonce)
            .await
            .unwrap();
        assert!(!signature.is_empty());

        let message = SubmitRFQ {
            rfqId: alloy::primitives::FixedBytes(rfq_id),
            legsHash: alloy::primitives::FixedBytes(legs_hash),
            wallet: wallet.address.as_wallet_address().inner(),
            nonce,
        };
        let recovered = recover_typed_signer(&message, &wallet.hypercall_domain(), &signature);
        assert_eq!(recovered, wallet.address.as_wallet_address().inner());
    }

    #[tokio::test]
    async fn test_sign_submit_rfq_response() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let rfq_id = [1u8; 32];
        let legs_hash = [2u8; 32];
        let net_premium = alloy::primitives::I256::try_from(385_000_000i64).unwrap();
        let valid_for_ms = alloy::primitives::U256::from(3000u64);
        let nonce = 1u64;

        let signature = wallet
            .sign_submit_rfq_response(rfq_id, legs_hash, net_premium, valid_for_ms, nonce)
            .await
            .unwrap();
        assert!(!signature.is_empty());

        let message = SubmitRFQResponse {
            rfqId: alloy::primitives::FixedBytes(rfq_id),
            legsHash: alloy::primitives::FixedBytes(legs_hash),
            netPremium: net_premium,
            validForMs: valid_for_ms,
            qpWallet: wallet.address.as_wallet_address().inner(),
            nonce,
        };
        let recovered = recover_typed_signer(&message, &wallet.hypercall_domain(), &signature);
        assert_eq!(recovered, wallet.address.as_wallet_address().inner());
    }

    #[tokio::test]
    async fn test_sign_accept_rfq_quote() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let rfq_id = [1u8; 32];
        let quote_id = [3u8; 32];
        let net_premium = alloy::primitives::I256::try_from(385_000_000i64).unwrap();
        let nonce = 1u64;

        let signature = wallet
            .sign_accept_rfq_quote(rfq_id, quote_id, net_premium, nonce)
            .await
            .unwrap();
        assert!(!signature.is_empty());

        let message = AcceptRFQQuote {
            rfqId: alloy::primitives::FixedBytes(rfq_id),
            quoteId: alloy::primitives::FixedBytes(quote_id),
            netPremium: net_premium,
            wallet: wallet.address.as_wallet_address().inner(),
            nonce,
        };
        let recovered = recover_typed_signer(&message, &wallet.hypercall_domain(), &signature);
        assert_eq!(recovered, wallet.address.as_wallet_address().inner());
    }

    #[tokio::test]
    async fn test_sign_connect_quote_provider() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let timestamp = alloy::primitives::U256::from(1711036800000u64);
        let nonce = 1u64;

        let signature = wallet
            .sign_connect_quote_provider(timestamp, nonce)
            .await
            .unwrap();
        assert!(!signature.is_empty());

        let message = ConnectQuoteProvider {
            wallet: wallet.address.as_wallet_address().inner(),
            timestamp,
            nonce,
        };
        let recovered = recover_typed_signer(&message, &wallet.hypercall_domain(), &signature);
        assert_eq!(recovered, wallet.address.as_wallet_address().inner());
    }

    #[tokio::test]
    async fn test_sign_standard_margin_liquidation_order() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let liquidated_wallet = AccountAddress::from(
            WalletAddress::from_str("0x1111111111111111111111111111111111111111").unwrap(),
        );
        let nonce = 1u64;

        let signature = wallet
            .sign_standard_margin_liquidation_payload(StandardMarginLiquidationSignature {
                wallet: wallet.address,
                liquidated_wallet,
                request_id: "01972be2-3d8a-7000-8000-000000000001",
                auction_id: "auction-1",
                bid_usdc: "100",
                portfolio_hash: "portfolio-hash",
                auction_terms_hash: "terms-hash",
                bid_intent_hash: "bid-intent-hash",
                auction_version: 1,
                nonce,
            })
            .await
            .unwrap();
        assert!(!signature.is_empty());

        let message = StandardMarginLiquidationOrder {
            wallet: wallet.address.as_wallet_address().inner(),
            liquidatedWallet: liquidated_wallet.as_wallet_address().inner(),
            requestId: "01972be2-3d8a-7000-8000-000000000001".to_string(),
            auctionId: "auction-1".to_string(),
            bidUsdc: "100".to_string(),
            portfolioHash: "portfolio-hash".to_string(),
            auctionTermsHash: "terms-hash".to_string(),
            bidIntentHash: "bid-intent-hash".to_string(),
            auctionVersion: 1,
            nonce,
        };
        let recovered = recover_typed_signer(&message, &wallet.hypercall_domain(), &signature);
        assert_eq!(recovered, wallet.address.as_wallet_address().inner());
    }

    #[tokio::test]
    async fn test_rfq_signatures_are_deterministic() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let rfq_id = [1u8; 32];
        let legs_hash = [2u8; 32];

        let sig1 = wallet.sign_submit_rfq(rfq_id, legs_hash, 1).await.unwrap();
        let sig2 = wallet.sign_submit_rfq(rfq_id, legs_hash, 1).await.unwrap();
        assert_eq!(sig1, sig2);
    }

    #[tokio::test]
    async fn test_rfq_signatures_differ_with_different_nonce() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let rfq_id = [1u8; 32];
        let legs_hash = [2u8; 32];

        let sig1 = wallet.sign_submit_rfq(rfq_id, legs_hash, 1).await.unwrap();
        let sig2 = wallet.sign_submit_rfq(rfq_id, legs_hash, 2).await.unwrap();
        assert_ne!(sig1, sig2);
    }

    #[tokio::test]
    async fn test_signatures_differ_with_different_nonce() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();

        let sig1 = wallet
            .sign_place_order_payload(place_signature(wallet.address, 1))
            .await
            .unwrap();

        let sig2 = wallet
            .sign_place_order_payload(place_signature(wallet.address, 2))
            .await
            .unwrap();

        assert_ne!(sig1, sig2);
    }

    #[tokio::test]
    async fn test_sign_replace_order() {
        let wallet = HypercallWallet::random(998);
        let nonce = wallet.next_nonce();

        let signature = wallet
            .sign_replace_order_payload(replace_signature(wallet.address, nonce))
            .await
            .unwrap();

        assert!(!signature.is_empty());
    }

    #[tokio::test]
    async fn test_replace_order_signature_roundtrip() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let nonce = wallet.next_nonce();
        let order_id = "42";
        let symbol = "BTC-20250131-100000-C";
        let side = "Buy";
        let size = "5";
        let price = "0.05";
        let tif = "gtc";
        let client_id = "cli_rt";

        let signature = wallet
            .sign_replace_order_payload(ReplaceOrderSignature {
                wallet: wallet.address,
                order_id,
                symbol,
                side,
                size,
                price,
                tif,
                client_id,
                reduce_only: false,
                nonce,
            })
            .await
            .unwrap();

        let message = ReplaceOrder {
            wallet: wallet.address.as_wallet_address().inner(),
            orderId: order_id.to_string(),
            symbol: symbol.to_string(),
            side: side.to_string(),
            size: size.to_string(),
            price: price.to_string(),
            tif: tif.to_string(),
            clientId: client_id.to_string(),
            nonce,
        };
        let recovered = recover_typed_signer(&message, &wallet.hypercall_domain(), &signature);
        assert_eq!(recovered, wallet.address.as_wallet_address().inner());
    }

    #[tokio::test]
    async fn test_sign_replace_order_reduce_only_uses_reduce_only_payload() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let nonce = wallet.next_nonce();

        let signature = wallet
            .sign_replace_order_payload(ReplaceOrderSignature {
                wallet: wallet.address,
                order_id: "42",
                symbol: "BTC-20250131-100000-C",
                side: "Buy",
                size: "1",
                price: "100",
                tif: "gtc",
                client_id: "cli_replace_ro",
                reduce_only: true,
                nonce,
            })
            .await
            .unwrap();

        let reduce_only_message = ReplaceOrderReduceOnly {
            wallet: wallet.address.as_wallet_address().inner(),
            orderId: "42".to_string(),
            symbol: "BTC-20250131-100000-C".to_string(),
            side: "Buy".to_string(),
            size: "1".to_string(),
            price: "100".to_string(),
            tif: "gtc".to_string(),
            clientId: "cli_replace_ro".to_string(),
            reduceOnly: true,
            nonce,
        };
        let recovered =
            recover_typed_signer(&reduce_only_message, &wallet.hypercall_domain(), &signature);
        assert_eq!(recovered, wallet.address.as_wallet_address().inner());

        let old_message = ReplaceOrder {
            wallet: wallet.address.as_wallet_address().inner(),
            orderId: "42".to_string(),
            symbol: "BTC-20250131-100000-C".to_string(),
            side: "Buy".to_string(),
            size: "1".to_string(),
            price: "100".to_string(),
            tif: "gtc".to_string(),
            clientId: "cli_replace_ro".to_string(),
            nonce,
        };
        let old_recovered =
            recover_typed_signer(&old_message, &wallet.hypercall_domain(), &signature);
        assert_ne!(old_recovered, wallet.address.as_wallet_address().inner());
    }
}

#[cfg(test)]
#[path = "wallet_test.rs"]
mod directive_tests;
