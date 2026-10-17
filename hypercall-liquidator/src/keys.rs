use hypercall_client::HypercallWallet;
use thiserror::Error;

use crate::config::{KeyConfig, KeyKind};

#[derive(Debug, Error)]
#[displaystr::display]
pub enum KeyError {
    MissingConfigField {
        field: &'static str,
    } = "key config is missing {field}",
    MissingEnvValue {
        env_var: String,
    } = "environment variable {env_var} is not set",
    UnsupportedKmsProvider {
        provider: String,
    } = "unsupported KMS provider {provider}",
    KmsFeatureDisabled =
        "KMS key config requires building hypercall-liquidator with the kms feature",
    KmsWallet(#[source] Box<hypercall_client::ClientError>) =
        "AWS KMS wallet initialization failed: {_0}",
    PrivateKeyUnavailableForPlaintextExport =
        "this operation requires plaintext private_key_env; KMS private keys are not exportable",
    Wallet(#[source] Box<hypercall_client::ClientError>) = "wallet initialization failed: {_0}",
    ChainIdOutOfRange {
        chain_id: u64,
    } = "chain id {chain_id} does not fit in u32",
}

pub fn plaintext_private_key_from_env(config: &KeyConfig) -> Result<String, KeyError> {
    if config.kind != KeyKind::Plaintext {
        return Err(KeyError::PrivateKeyUnavailableForPlaintextExport);
    }
    let env_var = config
        .private_key_env
        .as_ref()
        .ok_or(KeyError::MissingConfigField {
            field: "private_key_env",
        })?;
    std::env::var(env_var).map_err(|_| KeyError::MissingEnvValue {
        env_var: env_var.clone(),
    })
}

pub async fn hypercall_wallet_from_key_config(
    config: &KeyConfig,
    chain_id: u64,
) -> Result<HypercallWallet, KeyError> {
    let chain_id = u32::try_from(chain_id).map_err(|_| KeyError::ChainIdOutOfRange { chain_id })?;
    match config.kind {
        KeyKind::Plaintext => {
            let private_key = plaintext_private_key_from_env(config)?;
            HypercallWallet::from_private_key(&private_key, chain_id)
                .map_err(|source| KeyError::Wallet(Box::new(source)))
        }
        KeyKind::Kms => {
            #[cfg(not(feature = "kms"))]
            {
                Err(KeyError::KmsFeatureDisabled)
            }
            #[cfg(feature = "kms")]
            {
                let provider = config.provider.as_deref().unwrap_or("aws");
                if provider != "aws" {
                    return Err(KeyError::UnsupportedKmsProvider {
                        provider: provider.to_string(),
                    });
                }
                let key_id_env =
                    config
                        .key_id_env
                        .as_ref()
                        .ok_or(KeyError::MissingConfigField {
                            field: "key_id_env",
                        })?;
                let key_id = std::env::var(key_id_env).map_err(|_| KeyError::MissingEnvValue {
                    env_var: key_id_env.clone(),
                })?;
                HypercallWallet::from_aws_kms_key_id(key_id, chain_id)
                    .await
                    .map_err(|source| KeyError::KmsWallet(Box::new(source)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_key_reads_configured_env_var() {
        std::env::set_var("HYPERCALL_LIQUIDATOR_TEST_KEY", "0xabc");
        let config = KeyConfig {
            kind: KeyKind::Plaintext,
            private_key_env: Some("HYPERCALL_LIQUIDATOR_TEST_KEY".to_string()),
            provider: None,
            key_id_env: None,
        };

        assert_eq!(plaintext_private_key_from_env(&config).unwrap(), "0xabc");
        std::env::remove_var("HYPERCALL_LIQUIDATOR_TEST_KEY");
    }

    #[test]
    fn kms_key_does_not_export_private_key() {
        let config = KeyConfig {
            kind: KeyKind::Kms,
            private_key_env: None,
            provider: Some("aws".to_string()),
            key_id_env: Some("HYPERCALL_LIQUIDATOR_TEST_KMS".to_string()),
        };

        let error = plaintext_private_key_from_env(&config).unwrap_err();
        assert!(matches!(
            error,
            KeyError::PrivateKeyUnavailableForPlaintextExport
        ));
    }
}
