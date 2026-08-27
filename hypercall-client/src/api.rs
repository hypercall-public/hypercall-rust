//! HTTP client for the Hypercall REST API.
//!
//! [`HypercallClient`] provides typed methods for every REST endpoint:
//! orders, markets, portfolio, margin mode, fills, and more.
//!
//! # Example
//!
//! ```rust,no_run
//! use hypercall_client::{HypercallClient, HypercallWallet};
//! use hypercall_sdk_types::{Side, TimeInForce};
//! use rust_decimal::Decimal;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let api = HypercallClient::new("https://api.hypercall.xyz");
//! let wallet = HypercallWallet::from_private_key("0x...", 999)?;
//!
//! // Get markets
//! let markets = api.get_markets().await?;
//!
//! // Get portfolio
//! let portfolio = api.get_portfolio(&wallet.address).await?;
//!
//! // Place an options order (typed args)
//! let resp = api.place_order(&wallet, "BTC-20260501-76000-C", Side::Buy, Decimal::new(2000, 0), Decimal::new(5, 0), TimeInForce::IOC).await?;
//!
//! // Cancel an order
//! api.cancel_order(&wallet, 12345).await?;
//!
//! // Set margin mode to the launch default.
//! api.set_margin_mode(&wallet, "standard").await?;
//! # Ok(())
//! # }
//! ```
//!
use std::time::Duration;

use reqwest::{Client, Method, RequestBuilder};
use rust_decimal::Decimal;
use serde::Serialize;
use sonic_rs::{json, JsonContainerTrait, JsonValueTrait, Value};
use tracing::{info, warn};

use hypercall_sdk_types::{
    api_models::MarketsResponse, ApiResponse, BulkCancelOrderResponse, BulkPlaceOrderRequest,
    BulkPlaceOrderResponse, ExchangeInfoResponse, HistoricalPnlInterval, HistoricalPnlResponse,
    HistoricalTheoInterval, HistoricalTheoResponse, InstrumentSpecResponse, JsonRpcResponse,
    LiquidationStatusData, LiquidationStatusResponse, MarginMode, Market, OptionSummary, Order,
    OrderMessage, OrderRoute, OrdersApiResponse, PlaceOrderRequest, Portfolio, PortfolioResponse,
    PublicLiquidationsResponse, ReplaceOrderRequest, Side, StandardMarginLiquidationOrderRequest,
    StandardMarginLiquidationOrderResponse, StandardMarginLiquidationPositionRequest, TimeInForce,
    WalletAddress,
};

use crate::error::{ClientError, Result};
use crate::wallet::{
    AccountAddress, CancelOrderSignature, HypercallSigner, HypercallWallet, PlaceOrderSignature,
    ReplaceOrderSignature, StandardMarginLiquidationSignature,
};

const API_ROUTE_ORDER: &str = "/order";
const API_ROUTE_BULK_ORDER: &str = "/bulk_order";
const API_ROUTE_ORDERS: &str = "/orders";
const API_ROUTE_USERNAME: &str = "/username";
const API_ROUTE_MARGIN_MODE: &str = "/margin-mode";
const API_ROUTE_LIQUIDATION_STATUS: &str = "/liquidation/status";
const API_ROUTE_STANDARD_MARGIN_LIQUIDATION: &str = "/liquidation/standard-margin";
const API_ROUTE_RFQ_REQUEST: &str = "/rfq/request";
const API_ROUTE_RFQ_ACCEPT: &str = "/rfq/accept";

#[derive(Debug, serde::Deserialize)]
pub struct UsernameLookupResponse {
    pub wallet_address: String,
    pub username: String,
}

#[derive(Debug, Clone, Default)]
pub struct PublicLiquidationsQuery {
    pub cursor: Option<String>,
    pub limit: Option<usize>,
    pub wallet: Option<String>,
    pub status: Option<String>,
    pub state: Option<String>,
    pub margin_mode: Option<String>,
    pub liquidation_mode: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum UsernameLookupEnvelope {
    Wrapped(ApiResponse<UsernameLookupResponse>),
    Direct(UsernameLookupResponse),
}

/// Deserialize a reqwest response body using sonic-rs instead of reqwest's built-in serde_json.
/// This is necessary because sonic_rs::Value cannot be deserialized by serde_json's deserializer.
async fn sonic_json<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let bytes = response.bytes().await?;
    sonic_rs::from_slice(&bytes).map_err(ClientError::from)
}

async fn reqwest_json<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    Ok(response.json().await?)
}

fn non_empty_username(username: String) -> Option<String> {
    let username = username.trim();
    if username.is_empty() {
        None
    } else {
        Some(username.to_string())
    }
}

fn signed_side(side: Side) -> &'static str {
    match side {
        Side::Buy => "Buy",
        Side::Sell => "Sell",
    }
}

fn signed_tif(tif: TimeInForce) -> &'static str {
    match tif {
        TimeInForce::GTC => "gtc",
        TimeInForce::IOC => "ioc",
        TimeInForce::FOK => "fok",
    }
}

fn generated_client_id(nonce: u64) -> String {
    format!("cli_{nonce}")
}

fn mmp_enabled_or_default(options: &OrderOptions) -> bool {
    options.mmp_enabled.unwrap_or(false)
}

fn optional_wallet_address(address: Option<AccountAddress>) -> Option<WalletAddress> {
    address.map(AccountAddress::into_wallet_address)
}

mod sealed {
    pub trait Sealed {}
}

pub trait OrderDecimalInput: sealed::Sealed {
    fn into_order_decimal(self) -> Decimal;
}

impl sealed::Sealed for Decimal {}

impl OrderDecimalInput for Decimal {
    fn into_order_decimal(self) -> Decimal {
        self
    }
}

#[cfg(feature = "test-utils")]
impl sealed::Sealed for f64 {}

#[cfg(feature = "test-utils")]
impl OrderDecimalInput for f64 {
    fn into_order_decimal(self) -> Decimal {
        Decimal::from_f64_retain(self).expect("test order price/size must convert to Decimal")
    }
}

#[derive(Debug, Clone, Default)]
pub struct OrderOptions {
    pub route: Option<OrderRoute>,
    pub client_id: Option<String>,
    pub reduce_only: Option<bool>,
    pub mmp_enabled: Option<bool>,
    pub builder_code_address: Option<AccountAddress>,
}

#[derive(Debug, Clone)]
pub struct PlaceOrderParams<'a> {
    pub account: AccountAddress,
    pub symbol: &'a str,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub tif: TimeInForce,
    pub nonce: Option<u64>,
    pub options: OrderOptions,
}

#[derive(Debug, Clone)]
pub struct ReplaceOrderParams<'a> {
    pub account: AccountAddress,
    pub order_id: u64,
    pub symbol: &'a str,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub tif: TimeInForce,
    pub nonce: Option<u64>,
    pub options: OrderOptions,
}

#[derive(Debug, Clone)]
pub struct BulkOrderParams<'a> {
    pub symbol: &'a str,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub tif: TimeInForce,
    pub options: OrderOptions,
}

#[derive(Debug, Clone)]
pub struct BulkReplaceOrderParams<'a> {
    pub order_id: u64,
    pub symbol: &'a str,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub tif: TimeInForce,
    pub options: OrderOptions,
}

#[derive(Debug, Clone)]
pub struct StandardMarginLiquidationParams {
    pub liquidated_wallet: AccountAddress,
    pub request_id: uuid::Uuid,
    pub auction_id: String,
    pub bid_usdc: String,
    pub positions: Vec<StandardMarginLiquidationPositionRequest>,
    pub portfolio_hash: String,
    pub auction_terms_hash: String,
    pub auction_version: u64,
    pub valuation_timestamp_ms: u64,
    pub bid_intent_hash: String,
    pub nonce: Option<u64>,
}

/// HTTP API client for Hypercall.
#[derive(Clone)]
pub struct HypercallClient {
    client: Client,
    base_url: String,
}

impl HypercallClient {
    /// Create a new API client.
    pub fn new(base_url: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("hypercall-client/0.1")
            .build()
            .expect("failed to build reqwest client");

        Self {
            client,
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    /// Base URL used by this client.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn route_url(&self, route: &str) -> String {
        format!("{}{}", self.base_url, route)
    }

    pub(crate) fn request(&self, method: Method, route: &str) -> RequestBuilder {
        self.client.request(method, self.route_url(route))
    }

    async fn ensure_success(
        response: reqwest::Response,
        action: &str,
    ) -> Result<reqwest::Response> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        Err(ClientError::Api {
            status,
            message: format!("{action}: {text}"),
        })
    }

    async fn send(request: RequestBuilder, action: &str) -> Result<reqwest::Response> {
        Self::ensure_success(request.send().await?, action).await
    }

    pub(crate) async fn send_reqwest_json<T: serde::de::DeserializeOwned>(
        request: RequestBuilder,
        action: &str,
    ) -> Result<T> {
        reqwest_json(Self::send(request, action).await?).await
    }

    async fn send_sonic_json<T: serde::de::DeserializeOwned>(
        request: RequestBuilder,
        action: &str,
    ) -> Result<T> {
        sonic_json(Self::send(request, action).await?).await
    }

    pub(crate) async fn send_json_body<T, B>(
        request: RequestBuilder,
        body: &B,
        action: &str,
    ) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
        B: Serialize + ?Sized,
    {
        Self::send_reqwest_json(request.json(body), action).await
    }

    async fn post_json_sonic<T, B>(&self, route: &str, body: &B, action: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
        B: Serialize + ?Sized,
    {
        Self::send_sonic_json(self.request(Method::POST, route).json(body), action).await
    }

    /// Send a raw POST request to a path with a JSON body.
    pub async fn raw_post(&self, path: &str, body: &sonic_rs::Value) -> Result<reqwest::Response> {
        let url = format!("{}{}", self.base_url, path);
        let body_bytes = sonic_rs::to_vec(body)?;
        Ok(self
            .client
            .post(&url)
            .header("content-type", "application/json")
            .body(body_bytes)
            .send()
            .await?)
    }

    /// Check API health.
    pub async fn health_check(&self) -> Result<()> {
        self.health_check_with_timeout(Duration::from_secs(30))
            .await
    }

    /// Check API health using a custom request timeout.
    pub async fn health_check_with_timeout(&self, timeout: Duration) -> Result<()> {
        let response = self
            .client
            .get(format!("{}/health", self.base_url))
            .timeout(timeout)
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: "Health check failed".to_string(),
            })
        }
    }

    /// Check API readiness.
    pub async fn ready_check(&self) -> Result<()> {
        self.ready_check_with_timeout(Duration::from_secs(30)).await
    }

    /// Check API readiness using a custom request timeout.
    pub async fn ready_check_with_timeout(&self, timeout: Duration) -> Result<()> {
        let response = self
            .client
            .get(format!("{}/ready", self.base_url))
            .timeout(timeout)
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(ClientError::Api {
                status: response.status().as_u16(),
                message: "Readiness check failed".to_string(),
            })
        }
    }

    /// Get the public exchange configuration used for funding and signing.
    pub async fn get_exchange_info(&self) -> Result<ExchangeInfoResponse> {
        Self::send_reqwest_json(
            self.client.get(format!("{}/exchange-info", self.base_url)),
            "get exchange info",
        )
        .await
    }

    /// Get canonical instrument specifications for a currency.
    pub async fn get_instrument_specs(
        &self,
        currency: &str,
    ) -> Result<Vec<InstrumentSpecResponse>> {
        let body: JsonRpcResponse<Vec<InstrumentSpecResponse>> = Self::send_reqwest_json(
            self.client.get(format!(
                "{}/instrument-specs?currency={}",
                self.base_url, currency
            )),
            "get instrument specs",
        )
        .await?;
        body.result.ok_or_else(|| ClientError::Api {
            status: 200,
            message: body
                .error
                .map(|err| err.message)
                .unwrap_or_else(|| "No instrument specs returned".to_string()),
        })
    }

    /// Get all markets.
    pub async fn get_markets(&self) -> Result<Vec<Market>> {
        let markets_response: MarketsResponse = Self::send_reqwest_json(
            self.client.get(format!("{}/markets", self.base_url)),
            "get markets",
        )
        .await?;
        if !markets_response.success {
            return Err(ClientError::Api {
                status: 200,
                message: "Markets endpoint reported failure".to_string(),
            });
        }

        let mut markets = Vec::new();
        for market_info in markets_response.data {
            for instrument in market_info.instruments {
                markets.push(Market {
                    symbol: instrument.id.clone(),
                    strike: instrument.strike,
                    underlying: instrument.underlying.clone(),
                    expiry: instrument.expiry,
                    option_type: if instrument.option_type.eq_ignore_ascii_case("call") {
                        hypercall_sdk_types::OptionType::Call
                    } else {
                        hypercall_sdk_types::OptionType::Put
                    },
                });
            }
        }
        Ok(markets)
    }

    /// Place an order.
    pub async fn place_order<P, Q>(
        &self,
        wallet: &HypercallWallet,
        symbol: &str,
        side: Side,
        price: P,
        size: Q,
        tif: TimeInForce,
    ) -> Result<OrderMessage>
    where
        P: OrderDecimalInput,
        Q: OrderDecimalInput,
    {
        let nonce = wallet.next_nonce();
        self.place_order_with_params(
            wallet,
            PlaceOrderParams {
                account: wallet.address,
                symbol,
                side,
                price: price.into_order_decimal(),
                size: size.into_order_decimal(),
                tif,
                nonce: Some(nonce),
                options: OrderOptions::default(),
            },
        )
        .await
    }

    /// Place an order with an explicit nonce. Use this when you need to control
    /// the nonce directly (e.g., retrying with a known nonce, or testing replay
    /// protection). For normal usage, prefer `place_order()` which auto-increments.
    pub async fn place_order_with_nonce(
        &self,
        wallet: &HypercallWallet,
        nonce: u64,
        order: BulkOrderParams<'_>,
    ) -> Result<OrderMessage> {
        self.place_order_with_params(
            wallet,
            PlaceOrderParams {
                account: wallet.address,
                symbol: order.symbol,
                side: order.side,
                price: order.price,
                size: order.size,
                tif: order.tif,
                nonce: Some(nonce),
                options: order.options,
            },
        )
        .await
    }

    /// Place an order for a managed wallet/account with an explicit signer.
    pub async fn place_order_for_wallet(
        &self,
        signer: &HypercallWallet,
        account: AccountAddress,
        order: BulkOrderParams<'_>,
    ) -> Result<OrderMessage> {
        self.place_order_with_params(
            signer,
            PlaceOrderParams {
                account,
                symbol: order.symbol,
                side: order.side,
                price: order.price,
                size: order.size,
                tif: order.tif,
                nonce: None,
                options: order.options,
            },
        )
        .await
    }

    /// Place an order for a managed wallet/account with an explicit nonce.
    pub async fn place_order_for_wallet_with_nonce(
        &self,
        signer: &HypercallWallet,
        account: AccountAddress,
        nonce: u64,
        order: BulkOrderParams<'_>,
    ) -> Result<OrderMessage> {
        self.place_order_with_params(
            signer,
            PlaceOrderParams {
                account,
                symbol: order.symbol,
                side: order.side,
                price: order.price,
                size: order.size,
                tif: order.tif,
                nonce: Some(nonce),
                options: order.options,
            },
        )
        .await
    }

    pub async fn place_order_with_params<S>(
        &self,
        signer: &S,
        params: PlaceOrderParams<'_>,
    ) -> Result<OrderMessage>
    where
        S: HypercallSigner + ?Sized,
    {
        let route = params.options.route.unwrap_or(OrderRoute::BestExecution);
        let nonce = params.nonce.unwrap_or_else(|| signer.next_nonce());
        let side_str = signed_side(params.side);
        let tif_str = signed_tif(params.tif);
        let price_str = params.price.to_string();
        let size_str = params.size.to_string();
        let mmp_enabled = mmp_enabled_or_default(&params.options);
        let reduce_only = params.options.reduce_only.unwrap_or(false);
        let client_id = params
            .options
            .client_id
            .unwrap_or_else(|| generated_client_id(nonce));

        let signature = signer
            .sign_place_order_payload(PlaceOrderSignature {
                wallet: params.account,
                symbol: params.symbol,
                side: side_str,
                size: &size_str,
                price: &price_str,
                tif: tif_str,
                route,
                client_id: &client_id,
                reduce_only,
                nonce,
            })
            .await?;

        let request = PlaceOrderRequest {
            wallet: params.account.into_wallet_address(),
            price: price_str,
            size: size_str,
            symbol: params.symbol.to_string(),
            side: params.side,
            tif: params.tif,
            route: Some(route),
            client_id: Some(client_id),
            nonce,
            signature,
            reduce_only,
            mmp_enabled,
            builder_code_address: optional_wallet_address(params.options.builder_code_address),
        };

        Self::send_json_body(
            self.request(Method::POST, API_ROUTE_ORDER),
            &request,
            "place order",
        )
        .await
    }

    /// Cancel an order.
    pub async fn cancel_order<S>(&self, wallet: &S, order_id: u64) -> Result<Value>
    where
        S: HypercallSigner + ?Sized,
    {
        self.cancel_order_for_wallet(wallet, wallet.address(), order_id)
            .await
    }

    /// Cancel an order for a managed wallet/account with an explicit signer.
    pub async fn cancel_order_for_wallet<S>(
        &self,
        signer: &S,
        account: AccountAddress,
        order_id: u64,
    ) -> Result<Value>
    where
        S: HypercallSigner + ?Sized,
    {
        let nonce = signer.next_nonce();
        self.cancel_order_for_wallet_with_nonce(signer, account, order_id, nonce)
            .await
    }

    /// Cancel an order for a managed wallet/account with an explicit nonce.
    pub async fn cancel_order_for_wallet_with_nonce<S>(
        &self,
        signer: &S,
        account: AccountAddress,
        order_id: u64,
        nonce: u64,
    ) -> Result<Value>
    where
        S: HypercallSigner + ?Sized,
    {
        let order_id_str = order_id.to_string();

        let signature = signer
            .sign_cancel_order_payload(CancelOrderSignature {
                wallet: account,
                order_id: &order_id_str,
                nonce,
            })
            .await?;

        Self::send_sonic_json(
            self.request(Method::DELETE, API_ROUTE_ORDER).json(&json!({
                "wallet": account,
                "order_id": order_id,
                "nonce": nonce,
                "signature": signature,
            })),
            "cancel order",
        )
        .await
    }

    /// Atomically cancel an order and place a new one.
    pub async fn replace_order_with_params<S>(
        &self,
        signer: &S,
        params: ReplaceOrderParams<'_>,
    ) -> Result<OrderMessage>
    where
        S: HypercallSigner + ?Sized,
    {
        let nonce = params.nonce.unwrap_or_else(|| signer.next_nonce());
        let side_str = signed_side(params.side);
        let tif_str = signed_tif(params.tif);
        let price_str = params.price.to_string();
        let size_str = params.size.to_string();
        let order_id_str = params.order_id.to_string();
        let mmp_enabled = mmp_enabled_or_default(&params.options);
        let reduce_only = params.options.reduce_only.unwrap_or(false);
        let client_id = params
            .options
            .client_id
            .unwrap_or_else(|| generated_client_id(nonce));

        let signature = signer
            .sign_replace_order_payload(ReplaceOrderSignature {
                wallet: params.account,
                order_id: &order_id_str,
                symbol: params.symbol,
                side: side_str,
                size: &size_str,
                price: &price_str,
                tif: tif_str,
                client_id: &client_id,
                reduce_only,
                nonce,
            })
            .await?;

        let request = ReplaceOrderRequest {
            wallet: params.account.into_wallet_address(),
            order_id: params.order_id,
            price: price_str,
            size: size_str,
            symbol: params.symbol.to_string(),
            side: params.side,
            tif: params.tif,
            client_id: Some(client_id),
            nonce,
            signature,
            reduce_only,
            mmp_enabled,
            builder_code_address: optional_wallet_address(params.options.builder_code_address),
        };

        Self::send_json_body(
            self.request(Method::PUT, API_ROUTE_ORDER),
            &request,
            "replace order",
        )
        .await
    }

    /// Maximum orders per bulk request.
    pub const MAX_BULK_CHUNK_SIZE: usize = 50;

    /// Place multiple orders in bulk, automatically chunking to stay within API limits.
    pub async fn bulk_place_orders<S, P, Q>(
        &self,
        wallet: &S,
        orders: Vec<(&str, Side, P, Q, TimeInForce)>,
    ) -> Result<BulkPlaceOrderResponse>
    where
        S: HypercallSigner + ?Sized,
        P: OrderDecimalInput,
        Q: OrderDecimalInput,
    {
        let orders = orders
            .into_iter()
            .map(|(symbol, side, price, size, tif)| BulkOrderParams {
                symbol,
                side,
                price: price.into_order_decimal(),
                size: size.into_order_decimal(),
                tif,
                options: OrderOptions {
                    route: Some(OrderRoute::BookOnly),
                    ..OrderOptions::default()
                },
            })
            .collect();
        self.bulk_place_orders_with_params(wallet, orders).await
    }

    pub async fn bulk_place_orders_with_params<S>(
        &self,
        wallet: &S,
        orders: Vec<BulkOrderParams<'_>>,
    ) -> Result<BulkPlaceOrderResponse>
    where
        S: HypercallSigner + ?Sized,
    {
        info!("Placing {} orders in bulk", orders.len());

        let mut order_requests = Vec::with_capacity(orders.len());

        for order in orders {
            let nonce = wallet.next_nonce();
            let side_str = signed_side(order.side);
            let tif_str = signed_tif(order.tif);
            let price_str = order.price.to_string();
            let size_str = order.size.to_string();
            let mmp_enabled = mmp_enabled_or_default(&order.options);
            let reduce_only = order.options.reduce_only.unwrap_or(false);
            let client_id = order
                .options
                .client_id
                .unwrap_or_else(|| generated_client_id(nonce));
            let route = order.options.route.unwrap_or(OrderRoute::BookOnly);

            let signature = wallet
                .sign_place_order_payload(PlaceOrderSignature {
                    wallet: wallet.address(),
                    symbol: order.symbol,
                    side: side_str,
                    size: &size_str,
                    price: &price_str,
                    tif: tif_str,
                    route,
                    client_id: &client_id,
                    reduce_only,
                    nonce,
                })
                .await?;

            order_requests.push(PlaceOrderRequest {
                wallet: wallet.address().into_wallet_address(),
                price: price_str,
                size: size_str,
                symbol: order.symbol.to_string(),
                side: order.side,
                tif: order.tif,
                route: Some(route),
                client_id: Some(client_id),
                nonce,
                signature,
                reduce_only,
                mmp_enabled,
                builder_code_address: optional_wallet_address(order.options.builder_code_address),
            });
        }

        let mut all_results = Vec::with_capacity(order_requests.len());

        for chunk in order_requests.chunks(Self::MAX_BULK_CHUNK_SIZE) {
            let payload = BulkPlaceOrderRequest {
                orders: chunk.to_vec(),
            };

            let bulk_response: BulkPlaceOrderResponse = Self::send_json_body(
                self.request(Method::POST, API_ROUTE_BULK_ORDER),
                &payload,
                "place bulk orders",
            )
            .await?;
            all_results.extend(bulk_response.results);
        }

        Ok(BulkPlaceOrderResponse {
            results: all_results,
        })
    }

    /// Cancel multiple orders in bulk, automatically chunking to stay within API limits.
    pub async fn bulk_cancel_orders<S>(
        &self,
        wallet: &S,
        order_ids: Vec<u64>,
    ) -> Result<BulkCancelOrderResponse>
    where
        S: HypercallSigner + ?Sized,
    {
        self.bulk_cancel_orders_for_wallet(wallet, wallet.address(), order_ids)
            .await
    }

    /// Cancel multiple orders for a managed wallet/account, automatically chunking.
    pub async fn bulk_cancel_orders_for_wallet<S>(
        &self,
        signer: &S,
        account: AccountAddress,
        order_ids: Vec<u64>,
    ) -> Result<BulkCancelOrderResponse>
    where
        S: HypercallSigner + ?Sized,
    {
        info!("Canceling {} orders in bulk", order_ids.len());

        let mut cancel_requests = Vec::with_capacity(order_ids.len());

        for order_id in &order_ids {
            let nonce = signer.next_nonce();
            let order_id_str = order_id.to_string();
            let signature = signer
                .sign_cancel_order_payload(CancelOrderSignature {
                    wallet: account,
                    order_id: &order_id_str,
                    nonce,
                })
                .await?;

            cancel_requests.push(json!({
                "wallet": account,
                "order_id": order_id,
                "nonce": nonce,
                "signature": signature,
            }));
        }

        let mut all_results = Vec::with_capacity(cancel_requests.len());

        for chunk in cancel_requests.chunks(Self::MAX_BULK_CHUNK_SIZE) {
            let bulk_response: BulkCancelOrderResponse = Self::send_json_body(
                self.request(Method::DELETE, API_ROUTE_BULK_ORDER),
                &json!({ "cancels": chunk }),
                "cancel bulk orders",
            )
            .await?;
            all_results.extend(bulk_response.results);
        }

        Ok(BulkCancelOrderResponse {
            results: all_results,
        })
    }

    /// Replace multiple orders in bulk, automatically chunking to stay within API limits.
    ///
    /// Each tuple is (cancel_order_id, symbol, side, price, size, tif).
    pub async fn bulk_replace_orders<S, P, Q>(
        &self,
        wallet: &S,
        replacements: Vec<(u64, &str, Side, P, Q, TimeInForce)>,
    ) -> Result<BulkPlaceOrderResponse>
    where
        S: HypercallSigner + ?Sized,
        P: OrderDecimalInput,
        Q: OrderDecimalInput,
    {
        let replacements = replacements
            .into_iter()
            .map(
                |(order_id, symbol, side, price, size, tif)| BulkReplaceOrderParams {
                    order_id,
                    symbol,
                    side,
                    price: price.into_order_decimal(),
                    size: size.into_order_decimal(),
                    tif,
                    options: OrderOptions::default(),
                },
            )
            .collect();
        self.bulk_replace_orders_with_params(wallet, replacements)
            .await
    }

    pub async fn bulk_replace_orders_with_params<S>(
        &self,
        wallet: &S,
        replacements: Vec<BulkReplaceOrderParams<'_>>,
    ) -> Result<BulkPlaceOrderResponse>
    where
        S: HypercallSigner + ?Sized,
    {
        info!("Replacing {} orders in bulk", replacements.len());

        let mut replace_requests = Vec::with_capacity(replacements.len());

        for replacement in replacements {
            let nonce = wallet.next_nonce();
            let side_str = signed_side(replacement.side);
            let tif_str = signed_tif(replacement.tif);
            let price_str = replacement.price.to_string();
            let size_str = replacement.size.to_string();
            let order_id_str = replacement.order_id.to_string();
            let mmp_enabled = mmp_enabled_or_default(&replacement.options);
            let reduce_only = replacement.options.reduce_only.unwrap_or(false);
            let client_id = replacement
                .options
                .client_id
                .unwrap_or_else(|| generated_client_id(nonce));

            let signature = wallet
                .sign_replace_order_payload(ReplaceOrderSignature {
                    wallet: wallet.address(),
                    order_id: &order_id_str,
                    symbol: replacement.symbol,
                    side: side_str,
                    size: &size_str,
                    price: &price_str,
                    tif: tif_str,
                    client_id: &client_id,
                    reduce_only,
                    nonce,
                })
                .await?;

            replace_requests.push(ReplaceOrderRequest {
                wallet: wallet.address().into_wallet_address(),
                order_id: replacement.order_id,
                price: price_str,
                size: size_str,
                symbol: replacement.symbol.to_string(),
                side: replacement.side,
                tif: replacement.tif,
                client_id: Some(client_id),
                nonce,
                signature,
                reduce_only,
                mmp_enabled,
                builder_code_address: optional_wallet_address(
                    replacement.options.builder_code_address,
                ),
            });
        }

        let mut all_results = Vec::with_capacity(replace_requests.len());

        for chunk in replace_requests.chunks(Self::MAX_BULK_CHUNK_SIZE) {
            let bulk_response: BulkPlaceOrderResponse = Self::send_sonic_json(
                self.request(Method::PUT, API_ROUTE_BULK_ORDER)
                    .json(&sonic_rs::json!({ "replacements": chunk })),
                "replace bulk orders",
            )
            .await?;
            all_results.extend(bulk_response.results);
        }

        Ok(BulkPlaceOrderResponse {
            results: all_results,
        })
    }

    /// Get orders for a wallet (single page).
    pub async fn get_orders(
        &self,
        wallet: impl Into<AccountAddress>,
        status: Option<&str>,
    ) -> Result<Value> {
        let wallet = wallet.into();
        let mut url = format!("{}?wallet={}", self.route_url(API_ROUTE_ORDERS), wallet);
        if let Some(status_filter) = status {
            url.push_str(&format!("&status={}", status_filter));
        }

        Self::send_sonic_json(self.client.get(&url), "get orders").await
    }

    /// Get one typed page of option and perp orders.
    pub async fn get_orders_typed(
        &self,
        wallet: impl Into<AccountAddress>,
        status: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<OrdersApiResponse> {
        let wallet = wallet.into();
        let mut query = vec![("wallet", wallet.to_string())];
        if let Some(status_filter) = status {
            query.push(("status", status_filter.to_string()));
        }
        if let Some(limit) = limit {
            query.push(("limit", limit.to_string()));
        }
        if let Some(offset) = offset {
            query.push(("offset", offset.to_string()));
        }
        Self::send_reqwest_json(
            self.request(Method::GET, API_ROUTE_ORDERS).query(&query),
            "get typed orders",
        )
        .await
    }

    /// Get the display username for a wallet.
    pub async fn get_username(&self, wallet: impl Into<AccountAddress>) -> Result<Option<String>> {
        let wallet = wallet.into();
        let response = self
            .client
            .get(format!(
                "{}?wallet={}",
                self.route_url(API_ROUTE_USERNAME),
                wallet
            ))
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status,
                message: format!("Failed to get username: {}", text),
            });
        }

        match response.json::<UsernameLookupEnvelope>().await? {
            UsernameLookupEnvelope::Direct(data) => Ok(non_empty_username(data.username)),
            UsernameLookupEnvelope::Wrapped(result) => {
                if !result.success {
                    return Err(ClientError::Api {
                        status: 200,
                        message: result
                            .error
                            .unwrap_or_else(|| "Username request failed".to_string()),
                    });
                }
                Ok(result
                    .data
                    .and_then(|data| non_empty_username(data.username)))
            }
        }
    }

    /// Get ALL orders for a wallet, paginating through all pages.
    pub async fn get_all_orders(
        &self,
        wallet: impl Into<AccountAddress>,
        status: Option<&str>,
    ) -> Result<Vec<Value>> {
        let wallet = wallet.into();
        let mut all_orders = Vec::new();
        let mut offset = 0;

        loop {
            let mut url = format!(
                "{}/orders?wallet={}&offset={}",
                self.base_url, wallet, offset
            );
            if let Some(status_filter) = status {
                url.push_str(&format!("&status={}", status_filter));
            }

            let result: Value = Self::send_sonic_json(self.client.get(&url), "get orders").await?;
            let pagination = result
                .get("pagination")
                .ok_or_else(|| invalid_orders_pagination("missing pagination object"))?;
            let returned_limit = pagination
                .get("limit")
                .and_then(|value| value.as_u64())
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| invalid_orders_pagination("invalid pagination.limit"))?;
            let returned_offset = pagination
                .get("offset")
                .and_then(|value| value.as_u64())
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| invalid_orders_pagination("invalid pagination.offset"))?;
            let data = result.get("data").unwrap_or(&result);
            let page: Vec<Value> = match data.as_array() {
                Some(arr) => arr.iter().cloned().collect(),
                None => break,
            };

            let count = page.len();
            all_orders.extend(page);
            match next_orders_page_offset(offset, returned_offset, returned_limit, count)? {
                Some(next_offset) => offset = next_offset,
                None => break,
            }
        }

        Ok(all_orders)
    }

    /// Get all option and perp orders as canonical typed rows.
    pub async fn get_all_orders_typed(
        &self,
        wallet: impl Into<AccountAddress>,
        status: Option<&str>,
    ) -> Result<Vec<Order>> {
        let wallet = wallet.into();
        let mut orders = Vec::new();
        let mut offset = 0;
        loop {
            let page = self
                .get_orders_typed(wallet, status, None, Some(offset))
                .await?;
            let count = page.data.len();
            let next_offset = next_orders_page_offset(
                offset,
                page.pagination.offset,
                page.pagination.limit,
                count,
            )?;
            orders.extend(page.data);
            match next_offset {
                Some(next_offset) => offset = next_offset,
                None => break,
            }
        }
        Ok(orders)
    }

    /// Get fills for a wallet.
    pub async fn get_fills(
        &self,
        wallet: impl Into<AccountAddress>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Value> {
        let wallet = wallet.into();
        let mut url = format!("{}/fills?wallet={}", self.base_url, wallet);
        if let Some(limit_val) = limit {
            url.push_str(&format!("&limit={}", limit_val));
        }
        if let Some(offset_val) = offset {
            url.push_str(&format!("&offset={}", offset_val));
        }

        Self::send_sonic_json(self.client.get(&url), "get fills").await
    }

    /// Get a typed page of option and perp fills.
    pub async fn get_fills_typed(
        &self,
        wallet: impl Into<AccountAddress>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<hypercall_sdk_types::FillsResponse> {
        let wallet = wallet.into();
        let mut query = vec![("wallet", wallet.to_string())];
        if let Some(limit) = limit {
            query.push(("limit", limit.to_string()));
        }
        if let Some(offset) = offset {
            query.push(("offset", offset.to_string()));
        }
        Self::send_reqwest_json(
            self.request(Method::GET, "/fills").query(&query),
            "get typed fills",
        )
        .await
    }

    /// Get portfolio for a wallet.
    pub async fn get_portfolio(
        &self,
        wallet: impl Into<AccountAddress>,
    ) -> Result<PortfolioResponse> {
        let wallet = wallet.into();
        let result: ApiResponse<PortfolioResponse> = Self::send_reqwest_json(
            self.client
                .get(format!("{}/portfolio?wallet={}", self.base_url, wallet)),
            "get portfolio",
        )
        .await?;
        if !result.success {
            return Err(ClientError::Api {
                status: 200,
                message: result
                    .error
                    .unwrap_or_else(|| "Portfolio request failed".to_string()),
            });
        }
        result.data.ok_or_else(|| ClientError::Api {
            status: 200,
            message: result
                .error
                .unwrap_or_else(|| "No portfolio data returned".to_string()),
        })
    }

    /// Get the canonical portfolio-margin-compatible snapshot.
    pub async fn get_portfolio_snapshot(
        &self,
        wallet: impl Into<AccountAddress>,
    ) -> Result<Portfolio> {
        let wallet = wallet.into();
        let result: hypercall_sdk_types::CanonicalApiResponse<Portfolio> = Self::send_reqwest_json(
            self.request(Method::GET, "/portfolio")
                .query(&[("wallet", wallet.to_string())]),
            "get canonical portfolio",
        )
        .await?;
        if !result.success {
            return Err(ClientError::Api {
                status: 200,
                message: result
                    .error
                    .unwrap_or_else(|| "Portfolio request failed".to_string()),
            });
        }
        result.data.ok_or_else(|| ClientError::Api {
            status: 200,
            message: result
                .error
                .unwrap_or_else(|| "No portfolio data returned".to_string()),
        })
    }

    /// Get global public liquidation history.
    pub async fn get_public_liquidations(
        &self,
        query: &PublicLiquidationsQuery,
    ) -> Result<PublicLiquidationsResponse> {
        let mut params = Vec::new();
        if let Some(cursor) = &query.cursor {
            params.push(("cursor", cursor.clone()));
        }
        if let Some(limit) = query.limit {
            params.push(("limit", limit.to_string()));
        }
        if let Some(wallet) = &query.wallet {
            params.push(("wallet", wallet.clone()));
        }
        if let Some(status) = &query.status {
            params.push(("status", status.clone()));
        }
        if let Some(state) = &query.state {
            params.push(("state", state.clone()));
        }
        if let Some(margin_mode) = &query.margin_mode {
            params.push(("margin_mode", margin_mode.clone()));
        }
        if let Some(liquidation_mode) = &query.liquidation_mode {
            params.push(("liquidation_mode", liquidation_mode.clone()));
        }

        let mut request = self.client.get(format!("{}/liquidations", self.base_url));
        if !params.is_empty() {
            request = request.query(&params);
        }
        let response = request.send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status,
                message: format!("Failed to get liquidations: {}", text),
            });
        }

        let result: PublicLiquidationsResponse = sonic_json(response).await?;
        if !result.success {
            return Err(ClientError::Api {
                status: 200,
                message: result
                    .error
                    .clone()
                    .unwrap_or_else(|| "Liquidations request failed".to_string()),
            });
        }
        Ok(result)
    }

    /// Get liquidation status for a wallet.
    pub async fn get_liquidation_status(
        &self,
        wallet: impl Into<AccountAddress>,
    ) -> Result<Option<LiquidationStatusData>> {
        let wallet = wallet.into();
        let result: LiquidationStatusResponse = Self::send_sonic_json(
            self.client
                .get(self.route_url(API_ROUTE_LIQUIDATION_STATUS))
                .query(&[("wallet", wallet.to_string())]),
            "get liquidation status",
        )
        .await?;
        if !result.success {
            return Err(ClientError::Api {
                status: 200,
                message: result
                    .error
                    .clone()
                    .unwrap_or_else(|| "Liquidation status request failed".to_string()),
            });
        }
        Ok(result.data)
    }

    /// Get historical equity snapshots for a wallet.
    ///
    /// `include_attribution`: pass `Some(true)` only when rendering the
    /// per-symbol breakdown. The default response omits attribution because it
    /// is substantially larger than the equity-only time series.
    pub async fn get_historical_pnl(
        &self,
        wallet: impl Into<AccountAddress>,
        interval: HistoricalPnlInterval,
        limit: Option<usize>,
        include_attribution: Option<bool>,
    ) -> Result<HistoricalPnlResponse> {
        let wallet = wallet.into();
        let mut request = self
            .client
            .get(format!("{}/historical-pnl", self.base_url))
            .query(&[
                ("wallet", wallet.to_string()),
                ("interval", interval.as_str().to_string()),
            ]);

        if let Some(limit_value) = limit {
            request = request.query(&[("limit", limit_value.to_string())]);
        }

        if let Some(true) = include_attribution {
            request = request.query(&[("include_attribution", "true")]);
        }

        let result: ApiResponse<HistoricalPnlResponse> =
            Self::send_reqwest_json(request, "get historical pnl").await?;
        if !result.success {
            return Err(ClientError::Api {
                status: 200,
                message: result
                    .error
                    .unwrap_or_else(|| "Historical pnl request failed".to_string()),
            });
        }

        result.data.ok_or_else(|| ClientError::Api {
            status: 200,
            message: result
                .error
                .unwrap_or_else(|| "No historical pnl data returned".to_string()),
        })
    }

    /// Get historical theoretical-price snapshots for an option instrument.
    pub async fn get_historical_theos(
        &self,
        instrument_name: &str,
        interval: HistoricalTheoInterval,
        limit: Option<usize>,
    ) -> Result<HistoricalTheoResponse> {
        let mut request = self
            .client
            .get(format!("{}/historical-theos", self.base_url))
            .query(&[
                ("instrument_name", instrument_name.to_string()),
                ("interval", interval.as_str().to_string()),
            ]);

        if let Some(limit_value) = limit {
            request = request.query(&[("limit", limit_value.to_string())]);
        }

        let result: ApiResponse<HistoricalTheoResponse> =
            Self::send_reqwest_json(request, "get historical theos").await?;
        if !result.success {
            return Err(ClientError::Api {
                status: 200,
                message: result
                    .error
                    .unwrap_or_else(|| "Historical theos request failed".to_string()),
            });
        }

        result.data.ok_or_else(|| ClientError::Api {
            status: 200,
            message: result
                .error
                .unwrap_or_else(|| "No historical theo data returned".to_string()),
        })
    }

    /// Set margin mode for a wallet.
    pub async fn set_margin_mode(
        &self,
        wallet: &HypercallWallet,
        margin_mode: &str,
    ) -> Result<Value> {
        info!(
            "Setting margin mode to {} for {}",
            margin_mode, wallet.address
        );

        let nonce = wallet.next_nonce();
        let signature = wallet.sign_set_margin_mode(margin_mode, nonce).await?;

        let response = self
            .client
            .post(self.route_url(API_ROUTE_MARGIN_MODE))
            .json(&json!({
                "wallet": wallet.address.to_string(),
                "margin_mode": margin_mode,
                "nonce": nonce,
                "signature": signature,
            }))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status,
                message: format!("Failed to set margin mode: {}", text),
            });
        }

        let result: Value = sonic_json(response).await?;
        Ok(result)
    }

    /// Set an account's margin mode using the typed SDK enum.
    pub async fn set_margin_mode_typed(
        &self,
        wallet: &HypercallWallet,
        margin_mode: MarginMode,
    ) -> Result<Value> {
        self.set_margin_mode(wallet, margin_mode.as_str()).await
    }

    pub async fn submit_standard_margin_liquidation_with_params<S>(
        &self,
        liquidator: &S,
        params: StandardMarginLiquidationParams,
    ) -> Result<StandardMarginLiquidationOrderResponse>
    where
        S: HypercallSigner + ?Sized,
    {
        let nonce = params.nonce.unwrap_or_else(|| liquidator.next_nonce());
        let request_id = params.request_id.to_string();
        let signature = liquidator
            .sign_standard_margin_liquidation_payload(StandardMarginLiquidationSignature {
                wallet: liquidator.address(),
                liquidated_wallet: params.liquidated_wallet,
                request_id: &request_id,
                auction_id: &params.auction_id,
                bid_usdc: &params.bid_usdc,
                portfolio_hash: &params.portfolio_hash,
                auction_terms_hash: &params.auction_terms_hash,
                bid_intent_hash: &params.bid_intent_hash,
                auction_version: params.auction_version,
                nonce,
            })
            .await?;
        let request = StandardMarginLiquidationOrderRequest {
            wallet: liquidator.address().into_wallet_address(),
            liquidated_wallet: params.liquidated_wallet.into_wallet_address(),
            request_id: params.request_id,
            auction_id: params.auction_id,
            bid_usdc: params.bid_usdc,
            positions: params.positions,
            portfolio_hash: params.portfolio_hash,
            auction_terms_hash: params.auction_terms_hash,
            auction_version: params.auction_version,
            valuation_timestamp_ms: params.valuation_timestamp_ms,
            bid_intent_hash: params.bid_intent_hash,
            nonce,
            signature,
        };

        let result: ApiResponse<StandardMarginLiquidationOrderResponse> = Self::send_json_body(
            self.request(Method::POST, API_ROUTE_STANDARD_MARGIN_LIQUIDATION),
            &request,
            "submit standard margin liquidation",
        )
        .await?;
        result.data.ok_or_else(|| ClientError::Api {
            status: 200,
            message: result
                .error
                .unwrap_or_else(|| "No liquidation response returned".to_string()),
        })
    }

    /// Fetch underlying price from options-summary endpoint.
    pub async fn fetch_underlying_price(&self, currency: &str) -> Result<Option<f64>> {
        let response = match self
            .client
            .get(format!("{}/options-summary", self.base_url))
            .query(&[("currency", currency)])
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(err) => {
                warn!("options-summary request failed for {}: {}", currency, err);
                return Ok(None);
            }
        };

        if !response.status().is_success() {
            warn!(
                "options-summary returned {} for {}",
                response.status(),
                currency
            );
            return Ok(None);
        }

        let body: JsonRpcResponse<Vec<OptionSummary>> = reqwest_json(response).await?;
        let price = body.result.and_then(|mut results| {
            results.iter_mut().find_map(|summary| {
                (summary.underlying_price > 0.0).then_some(summary.underlying_price)
            })
        });

        Ok(price)
    }

    // =========================================================================
    // RFQ endpoints
    // =========================================================================

    /// Submit an RFQ (taker-side).
    pub async fn submit_rfq(
        &self,
        wallet: &HypercallWallet,
        rfq_id: &str,
        legs: Vec<hypercall_sdk_types::RfqLegRequest>,
    ) -> Result<Value> {
        // Parse rfq_id to bytes for signing
        let rfq_uuid = uuid::Uuid::parse_str(rfq_id).map_err(|e| ClientError::Api {
            status: 0,
            message: format!("Invalid rfq_id: {}", e),
        })?;
        let mut rfq_id_bytes = [0u8; 32];
        rfq_id_bytes[16..].copy_from_slice(rfq_uuid.as_bytes());

        // Compute legs hash (propagates parse errors for non-numeric sizes)
        let legs_hash = compute_legs_hash(&legs)?;

        let nonce = wallet.next_nonce();
        let signature = wallet
            .sign_submit_rfq(rfq_id_bytes, legs_hash, nonce)
            .await?;

        let request = hypercall_sdk_types::SubmitRfqRequest {
            rfq_id: rfq_id.to_string(),
            legs,
            wallet_address: wallet.address.into_wallet_address(),
            nonce,
            signature,
            auto_accept_limit: None,
        };

        self.post_json_sonic(API_ROUTE_RFQ_REQUEST, &request, "RFQ submission failed")
            .await
    }

    /// Submit an auto-execute RFQ (taker-side).
    ///
    /// Similar to `submit_rfq` but the taker pre-authorizes execution with
    /// `limit_price`. Buy RFQs use it as a max debit. Sell RFQs use it as a
    /// min credit. The first QP quote satisfying that directional limit
    /// triggers immediate execution without a separate accept RTT.
    /// Uses the `SubmitAutoExecuteRfq` EIP-712 type for signing.
    pub async fn submit_auto_execute_rfq(
        &self,
        wallet: &HypercallWallet,
        rfq_id: &str,
        legs: Vec<hypercall_sdk_types::RfqLegRequest>,
        limit_price: rust_decimal::Decimal,
    ) -> Result<Value> {
        let rfq_uuid = uuid::Uuid::parse_str(rfq_id).map_err(|e| ClientError::Api {
            status: 0,
            message: format!("Invalid rfq_id: {}", e),
        })?;
        let mut rfq_id_bytes = [0u8; 32];
        rfq_id_bytes[16..].copy_from_slice(rfq_uuid.as_bytes());

        let legs_hash = compute_legs_hash(&legs)?;

        // Convert limit_price to micro-units (multiply by 1_000_000) for I256
        use rust_decimal::prelude::ToPrimitive;
        let limit_micro = (limit_price * Decimal::new(1_000_000, 0))
            .to_i64()
            .ok_or_else(|| ClientError::Api {
                status: 0,
                message: "limit_price overflow in micro-unit conversion".to_string(),
            })?;
        let limit_i256 =
            alloy::primitives::I256::try_from(limit_micro).map_err(|e| ClientError::Api {
                status: 0,
                message: format!("limit_price I256 conversion: {}", e),
            })?;

        let nonce = wallet.next_nonce();
        let signature = wallet
            .sign_submit_auto_execute_rfq(rfq_id_bytes, legs_hash, limit_i256, nonce)
            .await?;

        let request = hypercall_sdk_types::SubmitRfqRequest {
            rfq_id: rfq_id.to_string(),
            legs,
            wallet_address: wallet.address.into_wallet_address(),
            nonce,
            signature,
            auto_accept_limit: Some(limit_price.to_string()),
        };

        self.post_json_sonic(
            API_ROUTE_RFQ_REQUEST,
            &request,
            "Auto-execute RFQ submission failed",
        )
        .await
    }

    /// Accept an RFQ quote (taker-side).
    pub async fn accept_rfq_quote(
        &self,
        wallet: &HypercallWallet,
        rfq_id: &str,
        quote_id: &str,
        net_premium: alloy::primitives::I256,
    ) -> Result<Value> {
        let rfq_uuid = uuid::Uuid::parse_str(rfq_id).map_err(|e| ClientError::Api {
            status: 0,
            message: format!("Invalid rfq_id: {}", e),
        })?;
        let quote_uuid = uuid::Uuid::parse_str(quote_id).map_err(|e| ClientError::Api {
            status: 0,
            message: format!("Invalid quote_id: {}", e),
        })?;
        let mut rfq_id_bytes = [0u8; 32];
        rfq_id_bytes[16..].copy_from_slice(rfq_uuid.as_bytes());
        let mut quote_id_bytes = [0u8; 32];
        quote_id_bytes[16..].copy_from_slice(quote_uuid.as_bytes());

        let nonce = wallet.next_nonce();
        let signature = wallet
            .sign_accept_rfq_quote(rfq_id_bytes, quote_id_bytes, net_premium, nonce)
            .await?;

        let request = hypercall_sdk_types::AcceptRfqRequest {
            rfq_id: rfq_id.to_string(),
            quote_id: quote_id.to_string(),
            wallet_address: wallet.address.into_wallet_address(),
            nonce,
            signature,
        };

        self.post_json_sonic(API_ROUTE_RFQ_ACCEPT, &request, "RFQ accept failed")
            .await
    }

    /// Get RFQ status and quotes.
    pub async fn get_rfq(&self, rfq_id: &str) -> Result<Value> {
        let response = self
            .client
            .get(format!("{}/rfq/{}", self.base_url, rfq_id))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(ClientError::Api {
                status,
                message: format!("Get RFQ failed: {}", text),
            });
        }

        let result: Value = sonic_json(response).await?;
        Ok(result)
    }
}

/// Compute keccak256 hash of RFQ legs for signing.
///
/// Canonical format: `"instrument|side|size"` joined by `";"`. `size` is
/// parsed to `Decimal` and re-formatted via `Decimal::to_string()` so the
/// signed hash matches Hypercall's public RFQ signing contract. Without this,
/// a non-canonical size string (`"01.0"`, `"+5"`, `"1.50"` etc.) would produce
/// a different `legs_hash`, and the submit would be rejected as a signature
/// mismatch.
fn compute_legs_hash(legs: &[hypercall_sdk_types::RfqLegRequest]) -> Result<[u8; 32]> {
    use sha3::Digest;
    let canonical = legs
        .iter()
        .map(|l| {
            let side = match l.side {
                hypercall_sdk_types::Side::Buy => "buy",
                hypercall_sdk_types::Side::Sell => "sell",
            };
            let size_canonical = l
                .size
                .parse::<Decimal>()
                .map_err(|e| ClientError::Api {
                    status: 0,
                    message: format!("Invalid leg size {:?}: {}", l.size, e),
                })?
                .to_string();
            Ok(format!("{}|{}|{}", l.instrument, side, size_canonical))
        })
        .collect::<Result<Vec<_>>>()?
        .join(";");
    let mut hasher = sha3::Keccak256::new();
    hasher.update(canonical.as_bytes());
    Ok(hasher.finalize().into())
}

impl std::fmt::Debug for HypercallClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HypercallClient")
            .field("base_url", &self.base_url)
            .finish()
    }
}

fn invalid_orders_pagination(message: &str) -> ClientError {
    ClientError::Api {
        status: 200,
        message: format!("invalid orders pagination: {message}"),
    }
}

fn next_orders_page_offset(
    requested_offset: usize,
    returned_offset: usize,
    returned_limit: usize,
    returned_count: usize,
) -> Result<Option<usize>> {
    if returned_limit == 0 {
        return Err(invalid_orders_pagination(
            "pagination.limit must be greater than zero",
        ));
    }
    if returned_count < returned_limit {
        return Ok(None);
    }

    let next_offset = returned_offset
        .checked_add(returned_count)
        .ok_or_else(|| invalid_orders_pagination("next offset overflowed usize"))?;
    if next_offset <= requested_offset {
        return Err(invalid_orders_pagination(
            "returned page does not advance the offset",
        ));
    }
    Ok(Some(next_offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_client_new() {
        let client = HypercallClient::new("http://localhost:3000");
        assert_eq!(client.base_url, "http://localhost:3000");
    }

    #[test]
    fn test_client_new_trims_trailing_slash() {
        let client = HypercallClient::new("http://localhost:3000/");
        assert_eq!(client.base_url, "http://localhost:3000");

        // trim_end_matches removes ALL trailing slashes
        let client2 = HypercallClient::new("http://localhost:3000///");
        assert_eq!(client2.base_url, "http://localhost:3000");
    }

    #[test]
    fn test_client_new_from_string() {
        let client = HypercallClient::new(String::from("https://api.example.com"));
        assert_eq!(client.base_url, "https://api.example.com");
    }

    #[test]
    fn test_client_debug() {
        let client = HypercallClient::new("http://localhost:3000");
        let debug = format!("{:?}", client);
        assert!(debug.contains("HypercallClient"));
        assert!(debug.contains("http://localhost:3000"));
    }

    #[test]
    fn test_client_clone() {
        let client = HypercallClient::new("http://localhost:3000");
        let cloned = client.clone();
        assert_eq!(cloned.base_url, client.base_url);
    }

    #[test]
    fn username_lookup_accepts_direct_response() {
        let response: UsernameLookupEnvelope = serde_json::from_value(serde_json::json!({
            "wallet_address": "0x0000000000000000000000000000000000000001",
            "username": "trader_1"
        }))
        .unwrap();

        match response {
            UsernameLookupEnvelope::Direct(data) => assert_eq!(data.username, "trader_1"),
            UsernameLookupEnvelope::Wrapped(_) => panic!("expected direct username response"),
        }
    }

    #[test]
    fn username_lookup_accepts_wrapped_response() {
        let response: UsernameLookupEnvelope = serde_json::from_value(serde_json::json!({
            "success": true,
            "data": {
                "wallet_address": "0x0000000000000000000000000000000000000001",
                "username": "trader_1"
            }
        }))
        .unwrap();

        match response {
            UsernameLookupEnvelope::Wrapped(data) => {
                assert_eq!(data.data.unwrap().username, "trader_1")
            }
            UsernameLookupEnvelope::Direct(_) => panic!("expected wrapped username response"),
        }
    }

    #[test]
    fn markets_response_deserializes_from_server_json() {
        let server_json = serde_json::json!({
            "success": true,
            "data": [{
                "underlying": "BTC",
                "expiry": 1735689600,
                "index_price": "95000",
                "atm_vol": "0.65",
                "instruments": [{
                    "instrument_id": 1,
                    "id": "BTC-20260101-100000-C",
                    "underlying": "BTC",
                    "strike": "100000",
                    "expiry": 1735689600,
                    "option_type": "call",
                    "option_token_address": null,
                    "mark_iv": "0.70",
                    "volume_24h": "1500",
                    "open_interest": "25000",
                    "updated_at": "2026-01-01T00:00:00Z",
                    "status": "ACTIVE",
                    "trading_mode": "orderbook"
                }],
                "total_volume_24h": "1500",
                "total_open_interest": "25000"
            }]
        });

        let resp: MarketsResponse = serde_json::from_value(server_json).unwrap();
        assert!(resp.success);
        assert_eq!(resp.data.len(), 1);
        let inst = &resp.data[0].instruments[0];
        assert_eq!(inst.strike, Decimal::new(100000, 0));
        assert_eq!(inst.id, "BTC-20260101-100000-C");
    }

    #[test]
    fn portfolio_response_deserializes_from_server_json() {
        let server_json = serde_json::json!({
            "success": true,
            "data": {
                "wallet_address": "0x0000000000000000000000000000000000000001",
                "positions": [{
                    "wallet_address": "0x0000000000000000000000000000000000000001",
                    "symbol": "BTC-20260101-100000-C",
                    "amount": "2",
                    "entry_price": "3000",
                    "margin_posted": "100",
                    "realized_pnl": "0",
                    "unrealized_pnl": "50",
                    "updated_at": "2026-01-01T00:00:00Z",
                    "notional_value": "6000",
                    "maintenance_margin": "0",
                    "liquidation_price": "0",
                    "margin_ratio": "0"
                }],
                "total_margin_used": "100",
                "available_balance": "900",
                "margin_mode": "standard"
            }
        });

        let resp: ApiResponse<PortfolioResponse> = serde_json::from_value(server_json).unwrap();
        assert!(resp.success);
        let portfolio = resp.data.unwrap();
        assert_eq!(portfolio.positions.len(), 1);
        assert_eq!(portfolio.total_margin_used, Decimal::new(100, 0));
    }
}
