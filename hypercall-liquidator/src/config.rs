use std::fs;
use std::path::Path;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
#[displaystr::display]
pub enum ConfigError {
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    } = "failed to read config {path}: {source}",
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    } = "failed to parse TOML config {path}: {source}",
    UnsupportedMode(String) = "liquidation.mode must be standard_margin_only, got {_0}",
    DryRunRequired = "dry_run must be true unless liquidation.enabled is false or the caller explicitly overrides this guard",
    NonPositive {
        field: &'static str,
    } = "{field} must be positive",
    InvalidHedgeNotionalRange = "hedge.max_order_notional_usdc must be greater than or equal to hedge.min_order_notional_usdc",
    InvalidMaxSlippageBps = "hedge.max_slippage_bps must be less than 10000",
    MissingPlaintextEnv = "plaintext key config requires private_key_env",
    MissingKmsKeyIdEnv = "kms key config requires key_id_env",
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HypercallLiquidatorConfig {
    pub hypercall: HypercallConfig,
    pub liquidation: LiquidationConfig,
    pub hedge: HedgeConfig,
    pub keys: KeyConfigs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HypercallConfig {
    pub api_url: String,
    pub chain_id: u64,
    pub account: String,
    pub poll_interval_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiquidationConfig {
    pub enabled: bool,
    pub mode: String,
    pub assume_penalty_profitability: bool,
    pub min_maintenance_buffer_bps: u32,
    #[serde(with = "rust_decimal::serde::str")]
    pub max_bid_usdc: Decimal,
    pub max_accounts_per_cycle: usize,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HedgeConfig {
    pub enabled: bool,
    pub venue: String,
    pub taker_only: bool,
    pub max_slippage_bps: u32,
    #[serde(with = "rust_decimal::serde::str")]
    pub delta_band_usd: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub min_order_notional_usdc: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub max_order_notional_usdc: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyConfigs {
    pub hypercall: KeyConfig,
    pub hyperliquid: KeyConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyKind {
    Plaintext,
    Kms,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyConfig {
    pub kind: KeyKind,
    pub private_key_env: Option<String>,
    pub provider: Option<String>,
    pub key_id_env: Option<String>,
}

impl HypercallLiquidatorConfig {
    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(input).map_err(|source| ConfigError::Parse {
            path: "<inline>".to_string(),
            source,
        })?;
        config.validate_allowing_live_execution_for_tests()?;
        Ok(config)
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let display = path.display().to_string();
        let contents = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: display.clone(),
            source,
        })?;
        let config: Self = toml::from_str(&contents).map_err(|source| ConfigError::Parse {
            path: display,
            source,
        })?;
        config.validate_allowing_live_execution_for_tests()?;
        Ok(config)
    }

    pub fn validate_for_live_start(&self, allow_live_execution: bool) -> Result<(), ConfigError> {
        self.validate_common()?;
        if self.liquidation.enabled && !self.liquidation.dry_run && !allow_live_execution {
            return Err(ConfigError::DryRunRequired);
        }
        Ok(())
    }

    fn validate_allowing_live_execution_for_tests(&self) -> Result<(), ConfigError> {
        self.validate_common()
    }

    fn validate_common(&self) -> Result<(), ConfigError> {
        if self.liquidation.mode != "standard_margin_only" {
            return Err(ConfigError::UnsupportedMode(self.liquidation.mode.clone()));
        }
        if self.hypercall.poll_interval_ms == 0 {
            return Err(ConfigError::NonPositive {
                field: "hypercall.poll_interval_ms",
            });
        }
        if self.liquidation.max_bid_usdc <= Decimal::ZERO {
            return Err(ConfigError::NonPositive {
                field: "liquidation.max_bid_usdc",
            });
        }
        if self.liquidation.max_accounts_per_cycle == 0 {
            return Err(ConfigError::NonPositive {
                field: "liquidation.max_accounts_per_cycle",
            });
        }
        if self.hedge.delta_band_usd <= Decimal::ZERO {
            return Err(ConfigError::NonPositive {
                field: "hedge.delta_band_usd",
            });
        }
        if self.hedge.min_order_notional_usdc <= Decimal::ZERO {
            return Err(ConfigError::NonPositive {
                field: "hedge.min_order_notional_usdc",
            });
        }
        if self.hedge.max_order_notional_usdc <= Decimal::ZERO {
            return Err(ConfigError::NonPositive {
                field: "hedge.max_order_notional_usdc",
            });
        }
        if self.hedge.max_order_notional_usdc < self.hedge.min_order_notional_usdc {
            return Err(ConfigError::InvalidHedgeNotionalRange);
        }
        if self.hedge.max_slippage_bps >= 10_000 {
            return Err(ConfigError::InvalidMaxSlippageBps);
        }
        self.keys.hypercall.validate()?;
        self.keys.hyperliquid.validate()?;
        Ok(())
    }
}

impl KeyConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        match self.kind {
            KeyKind::Plaintext => {
                if self
                    .private_key_env
                    .as_deref()
                    .unwrap_or_default()
                    .is_empty()
                {
                    return Err(ConfigError::MissingPlaintextEnv);
                }
            }
            KeyKind::Kms => {
                if self.key_id_env.as_deref().unwrap_or_default().is_empty() {
                    return Err(ConfigError::MissingKmsKeyIdEnv);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_CONFIG: &str = include_str!("../examples/liquidator.example.toml");

    #[test]
    fn parses_example_config() {
        let config = HypercallLiquidatorConfig::from_toml_str(VALID_CONFIG).unwrap();
        assert_eq!(config.liquidation.mode, "standard_margin_only");
        assert!(config.liquidation.dry_run);
        assert_eq!(config.keys.hypercall.kind, KeyKind::Plaintext);
    }

    #[test]
    fn rejects_non_standard_margin_mode() {
        let input = VALID_CONFIG.replace("standard_margin_only", "portfolio_margin");
        let error = HypercallLiquidatorConfig::from_toml_str(&input).unwrap_err();
        assert!(matches!(error, ConfigError::UnsupportedMode(_)));
    }

    #[test]
    fn live_start_requires_explicit_override() {
        let input = VALID_CONFIG.replace("dry_run = true", "dry_run = false");
        let config = HypercallLiquidatorConfig::from_toml_str(&input).unwrap();
        let error = config.validate_for_live_start(false).unwrap_err();
        assert!(matches!(error, ConfigError::DryRunRequired));
        config.validate_for_live_start(true).unwrap();
    }

    #[test]
    fn rejects_invalid_hedge_notional_range() {
        let input = VALID_CONFIG.replace(
            "min_order_notional_usdc = \"25\"",
            "min_order_notional_usdc = \"10001\"",
        );
        let error = HypercallLiquidatorConfig::from_toml_str(&input).unwrap_err();
        assert!(matches!(error, ConfigError::InvalidHedgeNotionalRange));
    }

    #[test]
    fn rejects_invalid_max_slippage_bps() {
        let input = VALID_CONFIG.replace("max_slippage_bps = 75", "max_slippage_bps = 10000");
        let error = HypercallLiquidatorConfig::from_toml_str(&input).unwrap_err();
        assert!(matches!(error, ConfigError::InvalidMaxSlippageBps));
    }
}
