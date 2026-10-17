#![expect(
    clippy::result_large_err,
    reason = "PerpVenue fixes the established unboxed ClientError result contract"
)]

#[cfg(feature = "aws-kms")]
use alloy::signers::aws::{aws_config, aws_sdk_kms, AwsSigner};
use alloy::signers::Signer;
use hypercall_client::error::Result;
use hypercall_client::ClientError;
use hypersdk::hypercore::{
    signing::agent_signing_hash,
    types::{
        Action, ActionRequest, BatchCancel, BatchCancelCloid, BatchOrder, Builder, Cancel,
        CancelByCloid, ClearinghouseState, Fill, OkResponse, OrderGrouping, OrderRequest,
        OrderResponseStatus, OrderTypePlacement, Response, TimeInForce as HyperliquidTimeInForce,
    },
    Cloid, HttpClient, NonceHandler, PrivateKeySigner,
};
use rust_decimal::{Decimal, RoundingStrategy};

use crate::{
    hyperliquid_client_with_base_url, signing_wire::action_connection_id, HyperliquidAddress,
    HyperliquidChain, HyperliquidPerpAsset, HyperliquidPerpAssetRegistry, PerpVenue,
    PerpVenueCancelByClientIdRequest, PerpVenueCancelByOidRequest, PerpVenueFuture,
    PerpVenueOrderRequest, Tif,
};

pub type HyperliquidOrderResponseStatus = OrderResponseStatus;

pub trait HyperliquidSigner: Signer + Send + Sync {}

impl<T> HyperliquidSigner for T where T: Signer + Send + Sync {}

pub struct DirectHyperliquidPerpVenue<S = PrivateKeySigner> {
    client: HttpClient,
    signer: S,
    nonce: NonceHandler,
    registry: HyperliquidPerpAssetRegistry,
    vault_address: Option<HyperliquidAddress>,
    builder_address: Option<HyperliquidAddress>,
}

impl DirectHyperliquidPerpVenue<PrivateKeySigner> {
    pub fn mainnet(private_key: &str, registry: HyperliquidPerpAssetRegistry) -> Result<Self> {
        Self::new(HyperliquidChain::Mainnet, private_key, registry)
    }

    pub fn testnet(private_key: &str, registry: HyperliquidPerpAssetRegistry) -> Result<Self> {
        Self::new(HyperliquidChain::Testnet, private_key, registry)
    }

    pub fn new(
        chain: HyperliquidChain,
        private_key: &str,
        registry: HyperliquidPerpAssetRegistry,
    ) -> Result<Self> {
        let client = HttpClient::new(chain);
        Self::with_client(client, private_key, registry)
    }

    pub fn with_base_url(
        chain: HyperliquidChain,
        base_url: &str,
        private_key: &str,
        registry: HyperliquidPerpAssetRegistry,
    ) -> Result<Self> {
        let client = hyperliquid_client_with_base_url(chain, base_url)?;
        Self::with_client(client, private_key, registry)
    }

    fn with_client(
        client: HttpClient,
        private_key: &str,
        registry: HyperliquidPerpAssetRegistry,
    ) -> Result<Self> {
        let signer = private_key
            .parse::<PrivateKeySigner>()
            .map_err(|error| ClientError::signing("invalid Hyperliquid key", error))?;
        Ok(Self::from_signer_with_client(client, signer, registry))
    }
}

#[cfg(feature = "aws-kms")]
impl DirectHyperliquidPerpVenue<AwsSigner> {
    pub async fn mainnet_aws_kms(
        key_id: impl Into<String>,
        registry: HyperliquidPerpAssetRegistry,
    ) -> Result<Self> {
        Self::from_aws_kms(HyperliquidChain::Mainnet, key_id, registry).await
    }

    pub async fn testnet_aws_kms(
        key_id: impl Into<String>,
        registry: HyperliquidPerpAssetRegistry,
    ) -> Result<Self> {
        Self::from_aws_kms(HyperliquidChain::Testnet, key_id, registry).await
    }

    pub async fn from_aws_kms(
        chain: HyperliquidChain,
        key_id: impl Into<String>,
        registry: HyperliquidPerpAssetRegistry,
    ) -> Result<Self> {
        let client = HttpClient::new(chain);
        Self::from_aws_kms_with_client(client, key_id, registry).await
    }

    pub async fn with_base_url_aws_kms(
        chain: HyperliquidChain,
        base_url: &str,
        key_id: impl Into<String>,
        registry: HyperliquidPerpAssetRegistry,
    ) -> Result<Self> {
        let client = hyperliquid_client_with_base_url(chain, base_url)?;
        Self::from_aws_kms_with_client(client, key_id, registry).await
    }

    async fn from_aws_kms_with_client(
        client: HttpClient,
        key_id: impl Into<String>,
        registry: HyperliquidPerpAssetRegistry,
    ) -> Result<Self> {
        let key_id = key_id.into();
        let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let kms_client = aws_sdk_kms::Client::new(&aws_config);
        let signer = AwsSigner::new(kms_client, key_id.clone(), None)
            .await
            .map_err(|error| {
                ClientError::signing(
                    format!("failed to initialize AWS KMS Hyperliquid signer {key_id}"),
                    error,
                )
            })?;
        Ok(Self::from_signer_with_client(client, signer, registry))
    }
}

impl<S> DirectHyperliquidPerpVenue<S> {
    pub fn from_signer(
        chain: HyperliquidChain,
        signer: S,
        registry: HyperliquidPerpAssetRegistry,
    ) -> Self {
        let client = HttpClient::new(chain);
        Self::from_signer_with_client(client, signer, registry)
    }

    fn from_signer_with_client(
        client: HttpClient,
        signer: S,
        registry: HyperliquidPerpAssetRegistry,
    ) -> Self {
        Self {
            client,
            signer,
            nonce: NonceHandler::default(),
            registry,
            vault_address: None,
            builder_address: None,
        }
    }

    pub fn with_vault_address(mut self, vault_address: HyperliquidAddress) -> Self {
        self.vault_address = Some(vault_address);
        self
    }

    /// Attribute orders to a builder without charging an additional fee.
    /// The trading account must have approved this builder on Hyperliquid.
    pub fn with_zero_fee_builder(mut self, builder_address: HyperliquidAddress) -> Self {
        self.builder_address = Some(builder_address);
        self
    }

    /// Wallet that signs direct venue orders. This may differ from the
    /// builder-approval owner and the vault or subaccount order target.
    pub fn signer_address(&self) -> HyperliquidAddress
    where
        S: Signer,
    {
        self.signer.address()
    }

    pub fn validate_order(&self, request: &PerpVenueOrderRequest) -> Result<()> {
        self.order_batch(request).map(|_| ())
    }

    pub async fn clearinghouse_state(
        &self,
        user: HyperliquidAddress,
    ) -> Result<ClearinghouseState> {
        self.client
            .clearinghouse_state(user, None)
            .await
            .map_err(|source| ClientError::api_source(400, crate::HypersdkReport(source)))
    }

    pub async fn user_fills(&self, user: HyperliquidAddress) -> Result<Vec<Fill>> {
        self.client
            .user_fills(user)
            .await
            .map_err(|source| ClientError::api_source(400, crate::HypersdkReport(source)))
    }

    fn resolve_asset(&self, symbol: &str) -> Result<&HyperliquidPerpAsset> {
        self.registry.resolve(symbol).ok_or_else(|| {
            ClientError::InvalidInput(format!(
                "unknown symbol '{}', available: {:?}",
                symbol,
                self.registry.available()
            ))
        })
    }

    fn order_batch(&self, request: &PerpVenueOrderRequest) -> Result<BatchOrder> {
        if self.builder_address == Some(HyperliquidAddress::ZERO) {
            return Err(ClientError::InvalidInput(
                "builder address must not be zero".to_string(),
            ));
        }
        let asset = self.resolve_asset(&request.symbol)?;
        let size = quantize_size(decimal_from_f64(request.size, "size")?, asset.sz_decimals)?;
        let limit_px = quantize_limit_price(
            decimal_from_f64(request.price, "price")?,
            request.is_buy,
            asset.sz_decimals,
        )?;
        Ok(BatchOrder {
            orders: vec![OrderRequest {
                asset: asset.asset_index as usize,
                is_buy: request.is_buy,
                limit_px,
                sz: size,
                reduce_only: request.reduce_only,
                order_type: OrderTypePlacement::Limit {
                    tif: hyperliquid_tif(request.tif),
                },
                cloid: request.client_id.map(cloid_from_u128).unwrap_or_default(),
            }],
            grouping: OrderGrouping::Na,
            builder: self.builder_address.map(|builder_address| Builder {
                builder_address,
                fee: 0,
            }),
        })
    }

    fn cancel_by_oid_batch(&self, request: &PerpVenueCancelByOidRequest) -> Result<BatchCancel> {
        let asset = self.resolve_asset(&request.symbol)?;
        Ok(BatchCancel {
            cancels: vec![Cancel {
                asset: asset.asset_index as usize,
                oid: request.order_id,
            }],
        })
    }

    fn cancel_by_client_id_batch(
        &self,
        request: &PerpVenueCancelByClientIdRequest,
    ) -> Result<BatchCancelCloid> {
        let asset = self.resolve_asset(&request.symbol)?;
        Ok(BatchCancelCloid {
            cancels: vec![CancelByCloid {
                asset: asset.asset_index,
                cloid: cloid_from_u128(request.client_id),
            }],
        })
    }
}

impl<S> DirectHyperliquidPerpVenue<S>
where
    S: HyperliquidSigner,
{
    async fn sign_and_send(&self, action: Action) -> Result<Response> {
        let nonce = self.nonce.next();
        let signed = if matches!(&action, Action::Order(batch) if batch.builder.is_some()) {
            let connection_id = action_connection_id(&action, nonce, self.vault_address, None)
                .map_err(|error| ClientError::Signing {
                    operation: "Hyperliquid builder order hashing failed".to_string(),
                    source: Box::new(error),
                })?;
            let digest = agent_signing_hash(self.client.chain(), connection_id);
            let signature =
                self.signer
                    .sign_hash(&digest)
                    .await
                    .map_err(|error| ClientError::Signing {
                        operation: "Hyperliquid builder order signing failed".to_string(),
                        source: Box::new(error),
                    })?;
            ActionRequest {
                signature: signature.into(),
                action,
                nonce,
                vault_address: self.vault_address,
                expires_after: None,
            }
        } else {
            action
                .sign(
                    &self.signer,
                    nonce,
                    self.vault_address,
                    None,
                    self.client.chain(),
                )
                .await
                .map_err(|error| ClientError::Signing {
                    operation: "Hyperliquid action signing failed".to_string(),
                    source: error.into_boxed_dyn_error(),
                })?
        };
        self.client
            .send(signed)
            .await
            .map_err(|error| ClientError::api_source(400, crate::HypersdkReport(error)))
    }
}

impl<S> PerpVenue for DirectHyperliquidPerpVenue<S>
where
    S: HyperliquidSigner,
{
    type OrderResult = Vec<HyperliquidOrderResponseStatus>;

    fn place_order<'a>(
        &'a self,
        request: PerpVenueOrderRequest,
    ) -> PerpVenueFuture<'a, Self::OrderResult> {
        Box::pin(async move {
            let batch = self.order_batch(&request)?;
            let expected_size = batch.orders[0].sz;
            let statuses = match self.sign_and_send(Action::Order(batch)).await? {
                Response::Ok(OkResponse::Order { statuses }) => statuses,
                Response::Err(message) => {
                    return Err(ClientError::Api {
                        status: 400,
                        message,
                    });
                }
                response => {
                    return Err(ClientError::Api {
                        status: 400,
                        message: format!("unexpected Hyperliquid order response: {response:?}"),
                    });
                }
            };
            ensure_order_response_success(statuses, request.tif, expected_size).map_err(|message| {
                ClientError::Api {
                    status: 400,
                    message,
                }
            })
        })
    }

    fn cancel_by_oid<'a>(
        &'a self,
        request: PerpVenueCancelByOidRequest,
    ) -> PerpVenueFuture<'a, Self::OrderResult> {
        Box::pin(async move {
            let batch = self.cancel_by_oid_batch(&request)?;
            match self.sign_and_send(Action::Cancel(batch)).await? {
                Response::Ok(OkResponse::Cancel { statuses }) => Ok(statuses),
                Response::Err(message) => Err(ClientError::Api {
                    status: 400,
                    message,
                }),
                response => Err(ClientError::Api {
                    status: 400,
                    message: format!("unexpected Hyperliquid cancel response: {response:?}"),
                }),
            }
        })
    }

    fn cancel_by_client_id<'a>(
        &'a self,
        request: PerpVenueCancelByClientIdRequest,
    ) -> PerpVenueFuture<'a, Self::OrderResult> {
        Box::pin(async move {
            let batch = self.cancel_by_client_id_batch(&request)?;
            match self.sign_and_send(Action::CancelByCloid(batch)).await? {
                Response::Ok(OkResponse::Cancel { statuses }) => Ok(statuses),
                Response::Err(message) => Err(ClientError::Api {
                    status: 400,
                    message,
                }),
                response => Err(ClientError::Api {
                    status: 400,
                    message: format!("unexpected Hyperliquid cancel response: {response:?}"),
                }),
            }
        })
    }
}

fn hyperliquid_tif(tif: Tif) -> HyperliquidTimeInForce {
    match tif {
        Tif::Alo => HyperliquidTimeInForce::Alo,
        Tif::Gtc => HyperliquidTimeInForce::Gtc,
        Tif::Ioc => HyperliquidTimeInForce::Ioc,
    }
}

fn decimal_from_f64(value: f64, name: &str) -> Result<Decimal> {
    if value <= 0.0 {
        return Err(ClientError::InvalidInput(format!(
            "{} must be positive, got {}",
            name, value
        )));
    }
    Decimal::try_from(value)
        .map_err(|_| ClientError::InvalidInput(format!("invalid {} {}", name, value)))
}

fn quantize_size(size: Decimal, sz_decimals: u32) -> Result<Decimal> {
    let quantized = size.round_dp_with_strategy(sz_decimals, RoundingStrategy::ToZero);
    if quantized <= Decimal::ZERO {
        return Err(ClientError::InvalidInput(format!(
            "order size {} is below Hyperliquid lot precision {}",
            size, sz_decimals
        )));
    }
    Ok(quantized)
}

fn quantize_limit_price(price: Decimal, is_buy: bool, sz_decimals: u32) -> Result<Decimal> {
    let max_decimal_places = 6u32.saturating_sub(sz_decimals);
    let significant_decimal_places = significant_decimal_places(price, 5);
    let decimal_places = max_decimal_places.min(significant_decimal_places);
    let strategy = if is_buy {
        RoundingStrategy::ToZero
    } else {
        RoundingStrategy::AwayFromZero
    };
    let rounded = round_positive_decimal(price, decimal_places, strategy)?;
    if rounded <= Decimal::ZERO {
        return Err(ClientError::InvalidInput(format!(
            "limit price {} is below Hyperliquid tick precision",
            price
        )));
    }
    Ok(rounded)
}

fn significant_decimal_places(price: Decimal, significant_figures: u32) -> u32 {
    if price >= Decimal::ONE {
        let integer_digits = price.trunc().to_string().len() as u32;
        return significant_figures.saturating_sub(integer_digits);
    }

    let normalized = price.normalize().to_string();
    let Some((_, fractional)) = normalized.split_once('.') else {
        return significant_figures;
    };
    let leading_zeroes = fractional
        .chars()
        .take_while(|character| *character == '0')
        .count() as u32;
    leading_zeroes + significant_figures
}

fn round_positive_decimal(
    value: Decimal,
    decimal_places: u32,
    strategy: RoundingStrategy,
) -> Result<Decimal> {
    if value < Decimal::from(100_000u32) {
        return Ok(value.round_dp_with_strategy(decimal_places, strategy));
    }

    let integer_digits = value.trunc().to_string().len() as u32;
    let shift = integer_digits.saturating_sub(5);
    let Some(factor) = 10u64.checked_pow(shift) else {
        return Err(ClientError::InvalidInput(format!(
            "limit price {} is too large to quantize",
            value
        )));
    };
    let factor = Decimal::from(factor);
    Ok((value / factor).round_dp_with_strategy(0, strategy) * factor)
}

fn ensure_order_response_success(
    statuses: Vec<HyperliquidOrderResponseStatus>,
    tif: Tif,
    expected_size: Decimal,
) -> std::result::Result<Vec<HyperliquidOrderResponseStatus>, String> {
    if let Some(error) = statuses.iter().find_map(|status| match status {
        HyperliquidOrderResponseStatus::Error(error) => Some(error),
        _ => None,
    }) {
        return Err(format!("Hyperliquid order rejected: {error}"));
    }
    if matches!(tif, Tif::Ioc) {
        let filled_size = statuses.iter().find_map(|status| match status {
            HyperliquidOrderResponseStatus::Filled { total_sz, .. } => Some(*total_sz),
            _ => None,
        });
        match filled_size {
            Some(total_sz) if total_sz >= expected_size => {}
            Some(total_sz) => {
                return Err(format!(
                    "Hyperliquid IOC order partially filled: requested {}, filled {}",
                    expected_size, total_sz
                ));
            }
            None => {
                return Err(format!(
                    "Hyperliquid IOC order did not report a full fill for requested size {}",
                    expected_size
                ));
            }
        }
    }
    Ok(statuses)
}

fn cloid_from_u128(client_id: u128) -> Cloid {
    Cloid::from(client_id.to_be_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn direct_hyperliquid_venue() -> DirectHyperliquidPerpVenue {
        DirectHyperliquidPerpVenue::testnet(
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            HyperliquidPerpAssetRegistry::testnet_defaults(),
        )
        .unwrap()
    }

    #[test]
    fn order_batch_maps_public_request() {
        let venue = direct_hyperliquid_venue();
        let batch = venue
            .order_batch(
                &PerpVenueOrderRequest::new("BTC-PERP", false, 77_000.0, 0.01, Tif::Ioc)
                    .reduce_only(true)
                    .client_id(42),
            )
            .unwrap();

        assert_eq!(batch.orders.len(), 1);
        let order = &batch.orders[0];
        assert_eq!(order.asset, 3);
        assert!(!order.is_buy);
        assert_eq!(order.limit_px, Decimal::try_from(77_000.0).unwrap());
        assert_eq!(order.sz, Decimal::try_from(0.01).unwrap());
        assert!(order.reduce_only);
        assert_eq!(order.cloid, cloid_from_u128(42));
        match order.order_type {
            OrderTypePlacement::Limit { tif } => {
                assert!(matches!(tif, HyperliquidTimeInForce::Ioc));
            }
            _ => panic!("expected limit order"),
        }
    }

    #[test]
    fn order_batch_attaches_only_zero_fee_builder() {
        let builder_address = "0x1111111111111111111111111111111111111111"
            .parse()
            .unwrap();
        let venue = direct_hyperliquid_venue().with_zero_fee_builder(builder_address);
        let batch = venue
            .order_batch(&PerpVenueOrderRequest::new(
                "BTC-PERP",
                true,
                70_000.0,
                0.01,
                Tif::Ioc,
            ))
            .unwrap();
        let builder = batch.builder.unwrap();
        assert_eq!(builder.builder_address, builder_address);
        assert_eq!(builder.fee, 0);
    }

    #[test]
    fn order_validation_rejects_zero_builder_address() {
        let venue = direct_hyperliquid_venue().with_zero_fee_builder(HyperliquidAddress::ZERO);
        let order = PerpVenueOrderRequest::new("BTC-PERP", true, 70_000.0, 0.01, Tif::Ioc);
        assert!(matches!(
            venue.validate_order(&order),
            Err(ClientError::InvalidInput(message)) if message == "builder address must not be zero"
        ));
    }

    #[test]
    fn signer_address_comes_from_signer() {
        assert_eq!(
            direct_hyperliquid_venue().signer_address(),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
                .parse::<HyperliquidAddress>()
                .unwrap()
        );
    }

    #[test]
    fn order_batch_quantizes_size_to_asset_precision() {
        let venue = direct_hyperliquid_venue();
        let batch = venue
            .order_batch(&PerpVenueOrderRequest::new(
                "BTC-PERP",
                true,
                70_000.0,
                0.142857142857,
                Tif::Ioc,
            ))
            .unwrap();

        assert_eq!(batch.orders[0].sz, Decimal::from_i128_with_scale(14285, 5));
    }

    #[test]
    fn order_batch_quantizes_limit_price_to_hyperliquid_precision() {
        let venue = direct_hyperliquid_venue();
        let buy = venue
            .order_batch(&PerpVenueOrderRequest::new(
                "BTC-PERP",
                true,
                67_626.422_5,
                0.01,
                Tif::Ioc,
            ))
            .unwrap();
        let sell = venue
            .order_batch(&PerpVenueOrderRequest::new(
                "BTC-PERP",
                false,
                67_626.422_5,
                0.01,
                Tif::Ioc,
            ))
            .unwrap();

        assert_eq!(buy.orders[0].limit_px, Decimal::from(67_626u32));
        assert_eq!(sell.orders[0].limit_px, Decimal::from(67_627u32));
    }

    #[test]
    fn order_batch_quantizes_large_limit_price_to_five_significant_figures() {
        let venue = direct_hyperliquid_venue();
        let batch = venue
            .order_batch(&PerpVenueOrderRequest::new(
                "BTC-PERP",
                true,
                123_456.78,
                0.01,
                Tif::Ioc,
            ))
            .unwrap();

        assert_eq!(batch.orders[0].limit_px, Decimal::from(123_450u32));
    }

    #[test]
    fn order_batch_rejects_size_below_asset_precision() {
        let venue = direct_hyperliquid_venue();
        let error = venue
            .order_batch(&PerpVenueOrderRequest::new(
                "BTC-PERP",
                true,
                70_000.0,
                0.000001,
                Tif::Ioc,
            ))
            .unwrap_err();

        assert!(matches!(error, ClientError::InvalidInput(_)));
    }

    #[test]
    fn order_response_errors_are_failures() {
        let error = ensure_order_response_success(
            vec![HyperliquidOrderResponseStatus::Error(
                "bad lot size".to_string(),
            )],
            Tif::Ioc,
            Decimal::new(1, 2),
        )
        .unwrap_err();

        assert!(error.contains("bad lot size"));
    }

    #[test]
    fn ioc_order_response_requires_full_fill() {
        let error = ensure_order_response_success(
            vec![HyperliquidOrderResponseStatus::Filled {
                total_sz: Decimal::new(9, 3),
                avg_px: Decimal::from(70_000u32),
                oid: 1,
            }],
            Tif::Ioc,
            Decimal::new(1, 2),
        )
        .unwrap_err();

        assert!(error.contains("partially filled"));
    }

    #[test]
    fn ioc_order_response_accepts_full_fill() {
        let statuses = ensure_order_response_success(
            vec![HyperliquidOrderResponseStatus::Filled {
                total_sz: Decimal::new(1, 2),
                avg_px: Decimal::from(70_000u32),
                oid: 1,
            }],
            Tif::Ioc,
            Decimal::new(1, 2),
        )
        .unwrap();

        assert_eq!(statuses.len(), 1);
    }

    #[test]
    fn cancel_batches_map_public_requests() {
        let venue = direct_hyperliquid_venue();

        let cancel = venue
            .cancel_by_oid_batch(&PerpVenueCancelByOidRequest::new("ETH", 12345))
            .unwrap();
        assert_eq!(cancel.cancels.len(), 1);
        assert_eq!(cancel.cancels[0].asset, 4);
        assert_eq!(cancel.cancels[0].oid, 12345);

        let cancel_by_cloid = venue
            .cancel_by_client_id_batch(&PerpVenueCancelByClientIdRequest::new("BTC", 99))
            .unwrap();
        assert_eq!(cancel_by_cloid.cancels.len(), 1);
        assert_eq!(cancel_by_cloid.cancels[0].asset, 3);
        assert_eq!(cancel_by_cloid.cancels[0].cloid, cloid_from_u128(99));
    }

    #[test]
    fn order_batch_rejects_invalid_inputs() {
        let venue = direct_hyperliquid_venue();

        assert!(venue
            .order_batch(&PerpVenueOrderRequest::new(
                "DOGE-PERP",
                true,
                1.0,
                1.0,
                Tif::Gtc
            ))
            .is_err());
        assert!(venue
            .order_batch(&PerpVenueOrderRequest::new(
                "BTC-PERP",
                true,
                0.0,
                1.0,
                Tif::Gtc
            ))
            .is_err());
        assert!(venue
            .order_batch(&PerpVenueOrderRequest::new(
                "BTC-PERP",
                true,
                1.0,
                0.0,
                Tif::Gtc
            ))
            .is_err());
    }

    fn assert_perp_venue<T: PerpVenue>(_venue: &T) {}

    #[test]
    fn direct_venue_implements_public_trait() {
        let venue = direct_hyperliquid_venue();
        assert_perp_venue(&venue);
    }
}
