//! Direct Hyperliquid perp venue implementation.
//!
//! This crate implements a direct perp venue using Hyperliquid's native
//! exchange API through `hypersdk`.
#[cfg(feature = "venue")]
mod perp_venue;
mod prepared;
mod prepared_spot;
mod prepared_twap;
mod signing_wire;
pub use signing_wire::recover_action_request;
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
    HyperliquidBatchModifyClassification, HyperliquidSubmissionClassification,
    PreparedHyperliquidAction, PreparedHyperliquidActionError, PreparedPerpCancelByCloid,
    PreparedPerpCancelByOid, PreparedPerpLeverageUpdate, PreparedPerpLimitOrder, PreparedPerpOrder,
    PreparedPerpOrderBatch, PreparedPerpOrderGrouping, PreparedPerpOrderType,
    PreparedPerpTriggerKind, MAX_BATCH_MODIFY_CHILDREN, PREPARED_HYPERLIQUID_ACTION_VERSION,
};
pub use prepared_spot::{
    is_spot_asset, quantize_spot_price_down, quantize_spot_size_down, validate_spot_price,
    validate_spot_size, PreparedSpotLimitOrder, HYPERLIQUID_MAX_PRICE_SIGNIFICANT_FIGURES,
    HYPERLIQUID_SPOT_ASSET_END, HYPERLIQUID_SPOT_ASSET_OFFSET, HYPERLIQUID_SPOT_MAX_PRICE_DECIMALS,
};
pub use prepared_twap::{
    PreparedModifyTarget, PreparedPerpModify, PreparedPerpTwapCancel, PreparedPerpTwapOrder,
    TwapAction, TwapCancelWire, TwapOrderWire, TWAP_MAX_MINUTES, TWAP_MIN_MINUTES,
};
#[cfg(feature = "venue")]
pub use registry::{HyperliquidPerpAsset, HyperliquidPerpAssetRegistry};
pub use tif::Tif;
#[cfg(feature = "venue")]
pub use venue::{DirectHyperliquidPerpVenue, HyperliquidOrderResponseStatus, HyperliquidSigner};

/// Preserve a hypersdk `anyhow` report behind SDK errors without flattening
/// its source chain into a string.
#[cfg(feature = "venue")]
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct HypersdkReport(#[source] anyhow::Error);

#[cfg(all(test, feature = "venue"))]
mod tests {
    use std::error::Error;

    use super::HypersdkReport;

    #[test]
    fn hypersdk_report_preserves_legacy_top_level_display_and_source_chain() {
        let report = anyhow::Error::new(std::io::Error::other("transport unavailable"))
            .context("Hyperliquid request failed");
        let error = HypersdkReport(report);

        assert_eq!(error.to_string(), "Hyperliquid request failed");
        assert!(error.source().is_some());
    }
}

#[cfg(feature = "venue")]
#[expect(
    clippy::result_large_err,
    reason = "the public venue facade preserves its established unboxed ClientError contract"
)]
fn hyperliquid_client_with_base_url(
    chain: HyperliquidChain,
    base_url: &str,
) -> hypercall_client::error::Result<hypersdk::hypercore::HttpClient> {
    let url = base_url.parse::<url::Url>().map_err(|source| {
        hypercall_client::ClientError::invalid_input(
            format!("Hyperliquid base URL '{base_url}'"),
            source,
        )
    })?;
    Ok(hypersdk::hypercore::HttpClient::new(chain).with_url(url))
}
