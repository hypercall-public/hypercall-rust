//! [`HypercallWallet`] signing for quote-provider sessions (direct and
//! delegated) and agent approval. Split out of `wallet.rs`; the methods stay on
//! [`HypercallWallet`], so their public paths are unchanged.

use super::*;

sol! {
    #[derive(Debug, PartialEq, Eq)]
    struct ConnectDelegatedQuoteProvider {
        address provider;
        address signer;
        uint256 timestamp;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ApproveAgentWithExpiry {
        address agent;
        uint64 expiresAtMs;
        uint64 nonce;
    }

    #[derive(Debug, PartialEq, Eq)]
    struct RevokeAgent {
        address agent;
        uint64 nonce;
    }
}

impl HypercallWallet {
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
            .map_err(|error| ClientError::signing("Failed to sign ConnectQuoteProvider", error))?;

        Ok(format!("{}", signature))
    }

    /// Sign a ConnectDelegatedQuoteProvider action: this wallet opens an RFQ
    /// V1 quote-provider session that quotes as the registered `provider`.
    /// The server admits it while this wallet holds RFQ authority for the
    /// provider's trading account (an active API wallet, an unexpired engine
    /// agent, or the manager).
    pub async fn sign_connect_delegated_quote_provider(
        &self,
        provider: AccountAddress,
        timestamp: alloy::primitives::U256,
        nonce: u64,
    ) -> Result<String> {
        let message = ConnectDelegatedQuoteProvider {
            provider: provider.as_wallet_address().inner(),
            signer: self.address.as_wallet_address().inner(),
            timestamp,
            nonce,
        };
        let signature = self
            .signer
            .sign_typed_data(&message, &self.hypercall_domain())
            .await
            .map_err(|error| {
                ClientError::signing("Failed to sign ConnectDelegatedQuoteProvider", error)
            })?;
        Ok(format!("{}", signature))
    }

    /// Sign an ApproveAgentWithExpiry action. This wallet approves `agent`
    /// until `expires_at_ms` (Unix milliseconds).
    pub async fn sign_approve_agent_with_expiry(
        &self,
        agent: AccountAddress,
        expires_at_ms: u64,
        nonce: u64,
    ) -> Result<String> {
        let message = ApproveAgentWithExpiry {
            agent: agent.as_wallet_address().inner(),
            expiresAtMs: expires_at_ms,
            nonce,
        };
        let signature = self
            .signer
            .sign_typed_data(&message, &self.hypercall_domain())
            .await
            .map_err(|error| {
                ClientError::signing("Failed to sign ApproveAgentWithExpiry", error)
            })?;
        Ok(format!("{}", signature))
    }

    /// Sign a RevokeAgent action. This wallet revokes `agent`.
    pub async fn sign_revoke_agent(&self, agent: AccountAddress, nonce: u64) -> Result<String> {
        let message = RevokeAgent {
            agent: agent.as_wallet_address().inner(),
            nonce,
        };
        let signature = self
            .signer
            .sign_typed_data(&message, &self.hypercall_domain())
            .await
            .map_err(|error| ClientError::signing("Failed to sign RevokeAgent", error))?;
        Ok(format!("{}", signature))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    const TEST_PRIVATE_KEY: &str =
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    fn recover<T: alloy::sol_types::SolStruct>(
        message: &T,
        domain: &alloy::sol_types::Eip712Domain,
        signature: &str,
    ) -> alloy::primitives::Address {
        alloy::primitives::Signature::from_str(signature)
            .unwrap()
            .recover_address_from_prehash(&message.eip712_signing_hash(domain))
            .unwrap()
    }

    /// The server (`hypercall-auth`) hashes these exact type strings; a
    /// renamed or reordered field would make every signature recover a
    /// different address.
    #[test]
    fn delegation_and_agent_type_strings_match_the_server() {
        use alloy::sol_types::SolStruct;
        assert_eq!(
            ConnectDelegatedQuoteProvider::eip712_encode_type(),
            "ConnectDelegatedQuoteProvider(address provider,address signer,uint256 timestamp,uint64 nonce)"
        );
        assert_eq!(
            ApproveAgentWithExpiry::eip712_encode_type(),
            "ApproveAgentWithExpiry(address agent,uint64 expiresAtMs,uint64 nonce)"
        );
        assert_eq!(
            RevokeAgent::eip712_encode_type(),
            "RevokeAgent(address agent,uint64 nonce)"
        );
    }

    #[tokio::test]
    async fn test_sign_connect_delegated_quote_provider() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let provider = AccountAddress::from(
            WalletAddress::from_str("0x1111111111111111111111111111111111111111").unwrap(),
        );
        let timestamp = alloy::primitives::U256::from(1711036800000u64);

        let signature = wallet
            .sign_connect_delegated_quote_provider(provider, timestamp, 5)
            .await
            .unwrap();

        let message = ConnectDelegatedQuoteProvider {
            provider: provider.as_wallet_address().inner(),
            signer: wallet.address.as_wallet_address().inner(),
            timestamp,
            nonce: 5,
        };
        let recovered = recover(&message, &wallet.hypercall_domain(), &signature);
        assert_eq!(recovered, wallet.address.as_wallet_address().inner());
    }

    #[tokio::test]
    async fn test_sign_approve_agent_with_expiry_and_revoke_agent() {
        let wallet = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 998).unwrap();
        let agent = AccountAddress::from(
            WalletAddress::from_str("0x2222222222222222222222222222222222222222").unwrap(),
        );

        let approval = wallet
            .sign_approve_agent_with_expiry(agent, 1_800_000_000_000, 6)
            .await
            .unwrap();
        let message = ApproveAgentWithExpiry {
            agent: agent.as_wallet_address().inner(),
            expiresAtMs: 1_800_000_000_000,
            nonce: 6,
        };
        let recovered = recover(&message, &wallet.hypercall_domain(), &approval);
        assert_eq!(recovered, wallet.address.as_wallet_address().inner());

        let revocation = wallet.sign_revoke_agent(agent, 7).await.unwrap();
        let message = RevokeAgent {
            agent: agent.as_wallet_address().inner(),
            nonce: 7,
        };
        let recovered = recover(&message, &wallet.hypercall_domain(), &revocation);
        assert_eq!(recovered, wallet.address.as_wallet_address().inner());
    }
}
