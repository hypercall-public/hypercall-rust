//! Direct Hyperliquid perp venue implementation.
//!
//! This crate implements a direct perp venue using Hyperliquid's native
//! exchange API through `hypersdk`.

#[cfg(feature = "venue")]
mod perp_venue;
mod prepared;
#[cfg(feature = "venue")]
mod registry;
mod tif;
#[cfg(feature = "venue")]
mod venue;

pub use hypersdk::hypercore::Chain as HyperliquidChain;
pub use hypersdk::Address as HyperliquidAddress;

#[cfg(feature = "venue")]
pub use perp_venue::{
    PerpVenue, PerpVenueCancelByClientIdRequest, PerpVenueCancelByOidRequest, PerpVenueFuture,
    PerpVenueOrderRequest,
};
pub use prepared::{
    HyperliquidSubmissionClassification, PreparedHyperliquidAction, PreparedHyperliquidActionError,
    PreparedPerpCancelByCloid, PreparedPerpLimitOrder, PREPARED_HYPERLIQUID_ACTION_VERSION,
};
#[cfg(feature = "venue")]
pub use registry::{HyperliquidPerpAsset, HyperliquidPerpAssetRegistry};
pub use tif::Tif;
#[cfg(feature = "venue")]
pub use venue::{DirectHyperliquidPerpVenue, HyperliquidOrderResponseStatus, HyperliquidSigner};

#[cfg(feature = "venue")]
fn hyperliquid_client_with_base_url(
    chain: HyperliquidChain,
    base_url: &str,
) -> hypercall_client::error::Result<hypersdk::hypercore::HttpClient> {
    let url = base_url.parse::<url::Url>().map_err(|error| {
        hypercall_client::ClientError::InvalidInput(format!(
            "invalid Hyperliquid base URL '{}': {}",
            base_url, error
        ))
    })?;
    Ok(hypersdk::hypercore::HttpClient::new(chain).with_url(url))
}
