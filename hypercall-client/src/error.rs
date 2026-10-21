//! Error types for the Hypercall client SDK.
//!
//! All fallible operations return [`Result<T>`](crate::error::Result) which uses
//! [`ClientError`] as the error type.
//!
//! # Error Variants
//!
//! - [`ClientError::Api`]: server returned an error (4xx/5xx with message)
//! - [`ClientError::Http`]: network/transport failure
//! - [`ClientError::Signing`]: EIP-712 signature creation failed
//! - [`ClientError::InvalidInput`]: bad input (unknown symbol, invalid key, etc)
//! - [`ClientError::WebSocket`]: WS connection or message failure

use std::error::Error;

use alloy::primitives::{Address, B256};
#[cfg(feature = "rfq-v2")]
use uuid::Uuid;

use crate::wallet::AccountAddress;

/// Dynamically typed source retained by a public client error.
pub type BoxClientSource = Box<dyn Error + Send + Sync + 'static>;

/// Caller-side RFQ validation failures.
///
/// These remain a local-input classification even though their `Display`
/// contract is wrapped in the historical `API error (0)` text below. Existing
/// SDK users may persist or surface that text, so the representation migration
/// must not silently rewrite it.
#[derive(Debug, thiserror::Error)]
#[displaystr::display]
pub enum RfqInputError {
    InvalidUuid {
        label: &'static str,
        #[source]
        source: uuid::Error,
    } = "{label}: {source}",
    InvalidLegSize {
        label: &'static str,
        value: String,
        #[source]
        source: rust_decimal::Error,
    } = "{label} leg size {value:?}: {source}",
    LimitPriceOverflow = "limit_price overflow in micro-unit conversion",
    LimitPriceConversion {
        #[source]
        source: BoxClientSource,
    } = "limit_price I256 conversion: {source}",
    NoLegs = "RFQ has no legs",
    LegCountMismatch {
        request_count: usize,
        price_count: usize,
    } = "leg count mismatch: request has {request_count} legs but {price_count} prices provided",
    LegPriceNonPositive {
        index: usize,
        price: rust_decimal::Decimal,
    } = "leg {index} price must be positive, got {price}",
    LegSizeNonPositive {
        index: usize,
        size: rust_decimal::Decimal,
    } = "leg {index} size must be positive, got {size}",
    InvalidSide {
        index: usize,
        side: String,
    } = "leg {index} has invalid side: {side:?}",
    NetPremiumNotRepresentable {
        net_premium: rust_decimal::Decimal,
    } = "net_premium {net_premium} not representable as micro-units i64",
    NetPremiumSubMicroPrecision {
        net_premium: rust_decimal::Decimal,
    } = "net_premium {net_premium} has sub-micro precision; use at most 6 decimal places so the signed value matches the wire string",
    NetPremiumConversion {
        #[source]
        source: BoxClientSource,
    } = "net_premium I256 conversion failed: {source}",
}

/// A closed reason why the configured signer cannot perform an operation.
///
/// These variants preserve the public diagnostics that predate the typed
/// client-error migration without accepting arbitrary string-wrapped errors.
#[derive(Debug, thiserror::Error)]
#[displaystr::display]
pub enum SigningUnavailableReason {
    AwsKmsPrivateKeyMaterial = "private key material is unavailable for AWS KMS wallets",
    HyperliquidMockApiUnauthenticated = "Hyperliquid mock API is not authenticated",
    HyperliquidSimulationVenueUnauthenticated = "Hyperliquid simulation venue is not authenticated",
}

// `displaystr` builds its `Display` match from the raw variant list and does
// not honor `#[cfg]` on variants, so the enum is emitted through this macro
// with or without the `rfq-v2` variants. The variants stay in the macro body
// (not passed in by the caller) so their `{_0}` format captures share the
// hygiene context of the bindings `displaystr` generates.
macro_rules! client_error_enum {
    ($($rfq_v2_feature:literal)?) => {
        /// Client error type.
        #[derive(Debug, thiserror::Error)]
        #[displaystr::display]
        pub enum ClientError {
            /// HTTP request failed
            Http(#[from] reqwest::Error) = "HTTP error: {_0}",
            /// JSON serialization/deserialization failed
            Json(#[from] sonic_rs::Error) = "JSON error: {_0}",
            /// Decoding an inbound server frame failed. Inbound frames are parsed
            /// with Serde, whose default recursion limit bounds hostile nesting.
            InboundJson(#[from] serde_json::Error) = "inbound JSON decode error: {_0}",
            /// A concrete signing operation failed.
            Signing {
                operation: String,
                #[source]
                source: BoxClientSource,
            } = "Signing error: {operation}: {source}",
            /// The selected signer cannot perform the requested operation.
            SigningUnavailable(#[source] SigningUnavailableReason) = "Signing error: {_0}",
            /// API returned an error
            Api { status: u16, message: String } = "API error ({status}): {message}",
            $(
                /// RFQ v2 response with its machine-readable domain code preserved.
                #[doc = ""]
                #[doc = concat!("Requires the `", $rfq_v2_feature, "` feature.")]
                RfqV2Api {
                    status: u16,
                    code: String,
                    message: String,
                } = "RFQ v2 API error ({status}, {code}): {message}",
                /// The signed change feed returned a non-progressing global cursor.
                #[doc = ""]
                #[doc = concat!("Requires the `", $rfq_v2_feature, "` feature.")]
                RfqV2ChangeFeedCursor {
                    after_sequence: u64,
                    next_after_sequence: u64,
                    has_more: bool,
                } = "RFQ v2 change feed cursor did not advance after {after_sequence}: next {next_after_sequence}, has_more {has_more}",
                /// An RFQ v2 read broke a durable contract: a quote delivered twice
                /// across pages, retained history that vanished, or a page with more
                /// results and no cursor.
                #[doc = ""]
                #[doc = concat!("Requires the `", $rfq_v2_feature, "` feature.")]
                RfqV2ProtocolViolation(String) = "RFQ v2 protocol violation: {_0}",
                /// A mutation may have been applied: the request timed out,
                /// failed in transport, or got a 5xx. `rfq_id` is `None` for
                /// maker-wide actions (bulk withdrawal, cancel schedule).
                /// `operation_id` is the stable identity of the request: the
                /// object ID for create and publish, the operation ID
                /// otherwise. Reconcile by these IDs or resend only the
                /// identical signed request.
                #[doc = ""]
                #[doc = concat!("Requires the `", $rfq_v2_feature, "` feature.")]
                RfqV2SubmissionAmbiguous {
                    rfq_id: Option<Uuid>,
                    operation_id: Uuid,
                    #[source]
                    source: Box<ClientError>,
                } = "RFQ v2 submission outcome is unknown for operation {operation_id} (RFQ {rfq_id:?}): {source}; reconcile or retry only the identical signed request",
            )?
            /// An API-shaped external response failed while retaining its concrete source.
            ApiSource {
                status: u16,
                #[source]
                source: BoxClientSource,
            } = "API error ({status}): {source}",
            /// WebSocket error
            WebSocket(String) = "WebSocket error: {_0}",
            /// Invalid input
            InvalidInput(String) = "invalid input: {_0}",
            /// A concrete parser or validator rejected caller-provided input.
            InvalidInputSource {
                context: String,
                #[source]
                source: BoxClientSource,
            } = "invalid input: invalid {context}: {source}",
            /// Caller-side RFQ validation with the legacy public diagnostic text.
            RfqInput(#[source] RfqInputError) = "API error (0): {_0}",
            /// A typed failure returned by the HyperEVM provider or contract binding.
            Onchain {
                operation: String,
                #[source]
                source: BoxClientSource,
            } = "failed to {operation}: {source}",
            /// A submitted transaction was confirmed as reverted.
            TransactionReverted {
                operation: &'static str,
                transaction_hash: Option<B256>,
            } = (
                "{operation} transaction{} reverted",
                transaction_hash_display_fragment(operation, transaction_hash.as_ref()),
            ),
            /// A confirmed Factory transaction omitted its required creation event.
            MissingAccountCreatedEvent = "confirmed Factory transaction did not emit AccountCreated",
            /// The Factory event named a different manager from the submitting wallet.
            AccountCreatedManagerMismatch {
                observed: Address,
                expected: Address,
            } = "AccountCreated manager {observed} does not match submitting manager {expected}",
            /// The Factory event named an account different from the simulated address.
            AccountCreatedAddressMismatch {
                observed: Address,
                expected: Address,
            } = "AccountCreated account {observed} does not match simulated account {expected}",
            /// A checked client-side calculation overflowed.
            ArithmeticOverflow { operation: &'static str } = "{operation} overflow",
            /// An HTTP withdrawal request may have reached the server before failing.
            WithdrawalSubmissionAmbiguous {
                action: String,
                route: String,
                account: AccountAddress,
                destination: AccountAddress,
                destination_dex: Option<u32>,
                amount: String,
                nonce: u64,
                #[source]
                source: Box<ClientError>,
            } = "{action} submission outcome is unknown: route={route}, account={account}, destination={destination}, destination_dex={destination_dex:?}, amount={amount}, nonce={nonce}, submission_error={source}; reconcile or retry only with these identical fields",
            /// A signed TEE perp (or TEE API wallet) submission may have reached the
            /// server before failing: a transport error, a timeout, a `5xx`, or a
            /// `2xx` whose body did not decode. `request_json` is the exact signed
            /// body that was sent. Reconcile from venue or directive state, or resend
            /// exactly these bytes with
            /// [`HypercallClient::resend_tee_submission`](crate::HypercallClient::resend_tee_submission),
            /// which the server deduplicates. Never re-run the high-level method: a
            /// fresh nonce (and nonce-derived cloids) would sign a second action.
            TeeSubmissionAmbiguous {
                action: String,
                route: String,
                nonce: u64,
                request_json: String,
                #[source]
                source: Box<ClientError>,
            } = "{action} submission outcome is unknown: route={route}, nonce={nonce}, submission_error={source}; reconcile, or resend only the identical signed request={request_json}",
            /// A broadcast transaction has an unknown submission outcome.
            TransactionSubmissionUnknown {
                operation: String,
                transaction_hash: B256,
                #[source]
                source: BoxClientSource,
            } = "{operation} submission outcome is unknown: hash={transaction_hash}, rpc_error={source}; reconcile this transaction before retrying",
            /// A broadcast transaction has an unknown confirmation outcome.
            TransactionConfirmationUnknown {
                operation: String,
                transaction_hash: B256,
                #[source]
                source: BoxClientSource,
            } = "{operation} {transaction_hash} was broadcast, but confirmation is unknown: {source}; reconcile this transaction before retrying",
            /// A confirmed receipt omitted its required block number.
            MissingTransactionBlock { transaction_hash: B256 } =
                "confirmed Hypercall USDC deposit transaction {transaction_hash} has no block number",
        }
    };
}

#[cfg(feature = "rfq-v2")]
client_error_enum!("rfq-v2");

#[cfg(not(feature = "rfq-v2"))]
client_error_enum! {}

impl ClientError {
    /// Add operation context without discarding the concrete signer error.
    pub fn signing(
        operation: impl Into<String>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self::Signing {
            operation: operation.into(),
            source: Box::new(source),
        }
    }

    /// Preserve the legacy API error boundary without flattening the source.
    pub fn api_source(status: u16, source: impl Error + Send + Sync + 'static) -> Self {
        Self::ApiSource {
            status,
            source: Box::new(source),
        }
    }

    /// Add input context without discarding the concrete parser error.
    pub fn invalid_input(
        context: impl Into<String>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self::InvalidInputSource {
            context: context.into(),
            source: Box::new(source),
        }
    }

    /// The stable `code` of an API error body (`{"code": ..., "message": ...}`),
    /// when this is an [`ClientError::Api`] whose body carries one.
    ///
    /// For example `unsupported_action`, which the TEE perp routes return
    /// (HTTP 400) when the admitted enclave release does not enable an action
    /// kind such as TWAP or batch modify.
    pub fn api_error_code(&self) -> Option<String> {
        let Self::Api { message, .. } = self else {
            return None;
        };
        // `ensure_success` prefixes the body with the action and ": ".
        let body = &message[message.find('{')?..];
        let value: serde_json::Value = serde_json::from_str(body).ok()?;
        value.get("code")?.as_str().map(str::to_owned)
    }

    /// Whether the server refused the request because this deployment does
    /// not enable the action (`400 unsupported_action`). Nothing was signed
    /// at the venue or reserved; retrying the same action will not help until
    /// the operator enables it.
    pub fn is_unsupported_action(&self) -> bool {
        self.api_error_code().as_deref() == Some("unsupported_action")
    }

    /// Returns true if this error indicates a connection-level failure
    /// (TCP connect error or request timeout), as opposed to an HTTP response error.
    pub fn is_connection_error(&self) -> bool {
        match self {
            Self::Http(e) => e.is_connect() || e.is_timeout(),
            _ => false,
        }
    }
}

/// Result type alias for client operations.
pub type Result<T> = std::result::Result<T, ClientError>;

/// Keep the pre-typed-error public text while retaining a structured hash for
/// reconciliation. Factory creation historically omitted its confirmed hash;
/// the funding operations placed the hash between `transaction` and
/// `reverted`.
fn transaction_hash_display_fragment(operation: &str, transaction_hash: Option<&B256>) -> String {
    if operation == "Factory account creation" {
        return String::new();
    }

    transaction_hash
        .map(|transaction_hash| format!(" {transaction_hash}"))
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "error_test.rs"]
mod tests;
