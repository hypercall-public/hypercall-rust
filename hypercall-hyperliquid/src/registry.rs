use std::collections::HashMap;

use hypercall_client::{error::Result, ClientError};
use hypersdk::hypercore::{HttpClient, PerpMarket};

use crate::{hyperliquid_client_with_base_url, HyperliquidChain};

#[derive(Debug, Clone)]
pub struct HyperliquidPerpAsset {
    pub symbol: String,
    pub asset_index: u32,
    pub sz_decimals: u32,
    pub max_leverage: u32,
}

#[derive(Debug, Clone)]
pub struct HyperliquidPerpAssetRegistry {
    by_symbol: HashMap<String, HyperliquidPerpAsset>,
}

impl HyperliquidPerpAssetRegistry {
    pub fn new(assets: Vec<HyperliquidPerpAsset>) -> Self {
        let mut by_symbol = HashMap::new();
        for asset in assets {
            let upper = asset.symbol.to_uppercase();
            let bare = upper.trim_end_matches("-PERP").to_string();
            by_symbol.insert(upper.clone(), asset.clone());
            if bare != upper {
                by_symbol.insert(bare, asset.clone());
            }
            by_symbol.insert(asset.symbol.to_lowercase(), asset.clone());
            let lower_bare = asset.symbol.to_lowercase().replace("-perp", "");
            by_symbol.insert(lower_bare, asset);
        }
        Self { by_symbol }
    }

    pub fn from_markets(markets: Vec<PerpMarket>) -> Self {
        let assets = markets
            .into_iter()
            .map(|market| HyperliquidPerpAsset {
                symbol: market.name,
                asset_index: market.index as u32,
                sz_decimals: market.sz_decimals as u32,
                max_leverage: market.max_leverage as u32,
            })
            .collect();
        Self::new(assets)
    }

    pub async fn from_hyperliquid_metadata(chain: HyperliquidChain) -> Result<Self> {
        let client = HttpClient::new(chain);
        Self::from_hyperliquid_client_metadata(client).await
    }

    pub async fn from_hyperliquid_metadata_with_base_url(
        chain: HyperliquidChain,
        base_url: &str,
    ) -> Result<Self> {
        let client = hyperliquid_client_with_base_url(chain, base_url)?;
        Self::from_hyperliquid_client_metadata(client).await
    }

    async fn from_hyperliquid_client_metadata(client: HttpClient) -> Result<Self> {
        let markets = client
            .perps()
            .await
            .map_err(|source| ClientError::api_source(400, crate::HypersdkReport(source)))?;
        Ok(Self::from_markets(markets))
    }

    pub fn mainnet_defaults() -> Self {
        Self::new(vec![
            HyperliquidPerpAsset {
                symbol: "BTC-PERP".into(),
                asset_index: 0,
                sz_decimals: 5,
                max_leverage: 40,
            },
            HyperliquidPerpAsset {
                symbol: "ETH-PERP".into(),
                asset_index: 1,
                sz_decimals: 4,
                max_leverage: 25,
            },
        ])
    }

    pub fn testnet_defaults() -> Self {
        Self::new(vec![
            HyperliquidPerpAsset {
                symbol: "BTC-PERP".into(),
                asset_index: 3,
                sz_decimals: 5,
                max_leverage: 40,
            },
            HyperliquidPerpAsset {
                symbol: "ETH-PERP".into(),
                asset_index: 4,
                sz_decimals: 4,
                max_leverage: 25,
            },
        ])
    }

    pub fn resolve(&self, symbol: &str) -> Option<&HyperliquidPerpAsset> {
        self.by_symbol
            .get(symbol)
            .or_else(|| self.by_symbol.get(&symbol.to_uppercase()))
            .or_else(|| self.by_symbol.get(&symbol.to_lowercase()))
    }

    pub fn available(&self) -> Vec<&str> {
        let mut seen = HashMap::new();
        for asset in self.by_symbol.values() {
            seen.entry(asset.asset_index)
                .or_insert(asset.symbol.as_str());
        }
        let mut symbols: Vec<_> = seen.into_values().collect();
        symbols.sort();
        symbols
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_registries_use_hyperliquid_asset_indexes() {
        let mainnet = HyperliquidPerpAssetRegistry::mainnet_defaults();
        assert_eq!(mainnet.resolve("BTC").unwrap().asset_index, 0);
        assert_eq!(mainnet.resolve("ETH").unwrap().asset_index, 1);

        let testnet = HyperliquidPerpAssetRegistry::testnet_defaults();
        assert_eq!(testnet.resolve("BTC").unwrap().asset_index, 3);
        assert_eq!(testnet.resolve("ETH").unwrap().asset_index, 4);
    }
}
