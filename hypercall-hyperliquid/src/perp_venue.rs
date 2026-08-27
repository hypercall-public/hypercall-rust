use std::future::Future;
use std::pin::Pin;

use crate::Tif;
use hypercall_client::error::Result;

pub type PerpVenueFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct PerpVenueOrderRequest {
    pub symbol: String,
    pub is_buy: bool,
    pub price: f64,
    pub size: f64,
    pub tif: Tif,
    pub reduce_only: bool,
    pub client_id: Option<u128>,
}

impl PerpVenueOrderRequest {
    pub fn new(symbol: impl Into<String>, is_buy: bool, price: f64, size: f64, tif: Tif) -> Self {
        Self {
            symbol: symbol.into(),
            is_buy,
            price,
            size,
            tif,
            reduce_only: false,
            client_id: None,
        }
    }

    pub fn reduce_only(mut self, reduce_only: bool) -> Self {
        self.reduce_only = reduce_only;
        self
    }

    pub fn client_id(mut self, client_id: u128) -> Self {
        self.client_id = Some(client_id);
        self
    }
}

#[derive(Debug, Clone)]
pub struct PerpVenueCancelByOidRequest {
    pub symbol: String,
    pub order_id: u64,
}

impl PerpVenueCancelByOidRequest {
    pub fn new(symbol: impl Into<String>, order_id: u64) -> Self {
        Self {
            symbol: symbol.into(),
            order_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PerpVenueCancelByClientIdRequest {
    pub symbol: String,
    pub client_id: u128,
}

impl PerpVenueCancelByClientIdRequest {
    pub fn new(symbol: impl Into<String>, client_id: u128) -> Self {
        Self {
            symbol: symbol.into(),
            client_id,
        }
    }
}

pub trait PerpVenue {
    type OrderResult;

    fn place_order<'a>(
        &'a self,
        request: PerpVenueOrderRequest,
    ) -> PerpVenueFuture<'a, Self::OrderResult>;

    fn cancel_by_oid<'a>(
        &'a self,
        request: PerpVenueCancelByOidRequest,
    ) -> PerpVenueFuture<'a, Self::OrderResult>;

    fn cancel_by_client_id<'a>(
        &'a self,
        request: PerpVenueCancelByClientIdRequest,
    ) -> PerpVenueFuture<'a, Self::OrderResult>;
}
