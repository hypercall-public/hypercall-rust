//! Signed TEE submissions: the nonce that identifies a body to the server,
//! and which failures leave its outcome unknown.

use super::*;

/// A signed body whose nonce identifies it to the server.
pub(crate) trait SignedTeeSubmission: serde::Serialize {
    fn nonce(&self) -> u64;
}

impl<A: serde::Serialize> SignedTeeSubmission for DirectiveSubmitRequest<A> {
    fn nonce(&self) -> u64 {
        self.nonce
    }
}

impl SignedTeeSubmission for TeeApiWalletApprovalSubmitRequest {
    fn nonce(&self) -> u64 {
        self.nonce
    }
}

/// Whether a failed submission may still have reached the server: any
/// transport or decode failure, a `408`, or a `5xx`. Other `4xx` answers
/// are definitive.
pub(super) fn tee_submission_is_ambiguous(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Http(_)
            | ClientError::Api {
                status: 408 | 500..=599,
                ..
            }
    )
}
