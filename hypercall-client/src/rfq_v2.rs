//! Typed low-level client for the RFQ V2 HTTP contract (`/private/rfqs*`).
//!
//! Callers supply complete signed payloads. [`crate::rfq_v2_signing`] builds
//! and signs them with a [`crate::HypercallWallet`]; remote signers and
//! non-Rust implementations sign the same EIP-712 messages and use the same
//! transport.
//!
//! # Availability
//!
//! Compiled with the `rfq-v2` cargo feature, on by default since 0.3.0. RFQ V2
//! is live in production from October 16, 2026. An environment that has not
//! enabled it refuses every RFQ V2 request, including private reads and
//! `AuthenticateRfqV2` socket authentication, with HTTP 403 and error code
//! `rfq_v2_disabled`. Over HTTP that surfaces as [`ClientError::RfqV2Api`]
//! with `status: 403` and that `code`; on the socket,
//! [`crate::WsClient::connect_rfq_v2`] fails. An environment that admits only
//! allowlisted accounts answers 403 `rfq_v2_not_allowlisted` for every other
//! account.
//!
//! # Operation IDs
//!
//! Every client-chosen operation ID must be a UUIDv7 whose embedded timestamp,
//! measured against the server clock, is at most one hour old and at most 30
//! seconds in the future. Any other ID is refused with HTTP 400
//! `rfq_v2_invalid_operation_id`. Mint a fresh `Uuid::now_v7()` for each new
//! operation, and reuse an ID only to retry the identical signed request while
//! it is still inside that window.

use std::cmp::Ordering;

use alloy::primitives::aliases::U512;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use displaystr::display;
use reqwest::{Method, RequestBuilder};
use serde::{de::DeserializeOwned, Serialize};
use tokio::{sync::mpsc, time::Duration};
use uuid::Uuid;

use hypercall_sdk_types::{
    CreateRfqV2Request, CreateRfqV2Response, DeclineRfqV2Request, ListRfqV2ChangesQuery,
    ListRfqV2ChangesResponse, ListRfqV2Query, ListRfqV2QuotesQuery, ListRfqV2QuotesResponse,
    ListRfqV2Response, PublishRfqV2FirmQuoteRequest, PublishRfqV2IndicativeQuoteRequest,
    PublishRfqV2QuoteResponse, RequestRfqV2FirmQuoteRequest, RfqV2AuthorizedTerms,
    RfqV2AutoExecutionLimit, RfqV2BulkWithdrawalRequest, RfqV2Digest, RfqV2ErrorResponse,
    RfqV2ExecutionRequest, RfqV2ExecutionResponse, RfqV2FillPolicy, RfqV2InvalidationRequest,
    RfqV2OperationResponse, RfqV2PackageLeg, RfqV2PreflightStatus, RfqV2QuoteReplacement,
    RfqV2QuoteView, RfqV2ReadGrant, RfqV2ReplacementRequest, RfqV2Response,
    RfqV2ScheduledQuoteCancelRequest, RfqV2ScheduledQuoteCancelResponse, RfqV2Side,
    RfqV2SweepRequest,
};

use crate::{ClientError, HypercallClient, Result};

pub(crate) const RFQ_V2_ROUTE: &str = "/private/rfqs";
const RFQ_V2_BULK_WITHDRAWAL_ROUTE: &str = "/private/rfq-quote-withdrawals";
const RFQ_V2_CANCEL_SCHEDULE_ROUTE: &str = "/private/rfq-quote-cancel-schedule";

async fn send_rfq_v2_json<T: DeserializeOwned>(
    request: RequestBuilder,
    action: &'static str,
) -> Result<T> {
    let response = request.send().await?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        // The status is already known, so a body that cannot be read keeps
        // it: a 4xx stays a definite refusal.
        let body = match response.text().await {
            Ok(body) => body,
            Err(error) => return Err(ClientError::api_source(status, error)),
        };
        if let Ok(error) = serde_json::from_str::<RfqV2ErrorResponse>(&body) {
            return Err(ClientError::RfqV2Api {
                status,
                code: error.code,
                message: error.message,
            });
        }
        return Err(ClientError::Api {
            status,
            message: format!("{action}: {body}"),
        });
    }
    Ok(response.json().await?)
}

async fn send_rfq_v2_body<T: DeserializeOwned, B: Serialize + ?Sized>(
    request: RequestBuilder,
    body: &B,
    action: &'static str,
) -> Result<T> {
    // Encode locally before dispatch. A serialization failure is a definite
    // client error, never an ambiguous submission.
    let encoded = sonic_rs::to_vec(body)?;
    send_rfq_v2_bytes(request, encoded, action).await
}

/// Send already-encoded JSON. Reconciliation resends these exact bytes.
pub(crate) async fn send_rfq_v2_bytes<T: DeserializeOwned>(
    request: RequestBuilder,
    encoded: Vec<u8>,
    action: &'static str,
) -> Result<T> {
    send_rfq_v2_json(
        request
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(encoded),
        action,
    )
    .await
}

/// Report a transport failure, timeout, or 5xx on a mutation as
/// [`ClientError::RfqV2SubmissionAmbiguous`]: the server may have applied it.
#[expect(
    clippy::result_large_err,
    reason = "passes through the established unboxed public ClientError contract"
)]
fn classify_rfq_v2_mutation<T>(
    rfq_id: Option<Uuid>,
    operation_id: Uuid,
    result: Result<T>,
) -> Result<T> {
    match result {
        Err(source) if ambiguous_rfq_v2_submission(&source) => {
            Err(ClientError::RfqV2SubmissionAmbiguous {
                rfq_id,
                operation_id,
                source: Box::new(source),
            })
        }
        result => result,
    }
}

/// Transport failures, timeouts and 5xx responses leave the outcome unknown.
/// `501 Not Implemented` is the exception: the route was never served.
pub(crate) fn ambiguous_rfq_v2_submission(error: &ClientError) -> bool {
    match error {
        // A builder error happens before any byte is sent.
        ClientError::Http(error) => !error.is_builder(),
        error => matches!(
            error,
            ClientError::Api {
                status: 500 | 502..=599,
                ..
            } | ClientError::ApiSource {
                status: 500 | 502..=599,
                ..
            } | ClientError::RfqV2Api {
                status: 500 | 502..=599,
                ..
            }
        ),
    }
}

/// Caller-provided read session for role-filtered private RFQ data.
///
/// The token is borrowed for one request and is never stored on the client.
/// A caller may encode a wallet-signed `RfqV2ReadGrant` with
/// `encode_rfq_v2_read_grant` and reuse it until expiry.
pub struct RfqV2ReadCredentials<'a> {
    pub bearer_token: &'a str,
}

/// Encode a signed private-read grant as a bearer token. The grant must be
/// EIP-712 signed for `AuthorizeRfqReadV2` under the Hypercall domain.
#[expect(
    clippy::result_large_err,
    reason = "public client methods preserve the established unboxed ClientError contract"
)]
pub fn encode_rfq_v2_read_grant(grant: &RfqV2ReadGrant) -> Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(sonic_rs::to_vec(grant)?))
}

fn rfq_v2_grant_remaining(expires_at_ms: u64) -> std::time::Duration {
    let Ok(elapsed) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        return std::time::Duration::ZERO;
    };
    let now_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    std::time::Duration::from_millis(expires_at_ms.saturating_sub(now_ms))
}

impl HypercallClient {
    /// Poll the signed recipient change feed even while its WebSocket remains
    /// connected. A WebSocket hint is only a wake-up signal; it cannot prove
    /// that no earlier hint was lost. The caller must refresh each returned
    /// RFQ and persist `next_after_sequence` only after applying the page.
    /// If applying a page fails, drop the receiver and restart from the last
    /// persisted cursor. Dropping the receiver stops polling. Recreate it
    /// with a fresh grant before the current one expires.
    #[expect(
        clippy::result_large_err,
        reason = "public client methods preserve the established unboxed ClientError contract"
    )]
    pub fn watch_rfq_v2_changes(
        &self,
        grant: &RfqV2ReadGrant,
        after_sequence: u64,
        poll_interval: Duration,
    ) -> Result<mpsc::Receiver<Result<ListRfqV2ChangesResponse>>> {
        if poll_interval.is_zero() {
            return Err(ClientError::InvalidInput(
                "RFQ v2 change poll interval must be positive".to_string(),
            ));
        }
        if rfq_v2_grant_remaining(grant.expires_at_ms).is_zero() {
            return Err(ClientError::InvalidInput(
                "RFQ v2 read grant expired".to_string(),
            ));
        }
        let token = encode_rfq_v2_read_grant(grant)?;
        let expires_at_ms = grant.expires_at_ms;
        let client = self.clone();
        let (sender, receiver) = mpsc::channel(1);
        tokio::spawn(async move {
            let mut cursor = after_sequence;
            let mut interval = tokio::time::interval(poll_interval);
            // A slow read must not trigger a burst of catch-up polls.
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                let remaining = rfq_v2_grant_remaining(expires_at_ms);
                if remaining.is_zero() {
                    break;
                }
                tokio::select! {
                    _ = sender.closed() => break,
                    _ = tokio::time::sleep(remaining) => break,
                    _ = interval.tick() => {}
                }
                if rfq_v2_grant_remaining(expires_at_ms).is_zero() {
                    break;
                }
                loop {
                    let remaining = rfq_v2_grant_remaining(expires_at_ms);
                    if remaining.is_zero() {
                        return;
                    }
                    let query = ListRfqV2ChangesQuery {
                        after_sequence: Some(cursor),
                        limit: Some(100),
                    };
                    let read = client.list_rfq_v2_changes(
                        &query,
                        RfqV2ReadCredentials {
                            bearer_token: &token,
                        },
                    );
                    let result = tokio::select! {
                        _ = sender.closed() => return,
                        _ = tokio::time::sleep(remaining) => return,
                        result = read => result,
                    };
                    let page = match result {
                        Ok(page) => page,
                        Err(error) => {
                            let auth_failed = matches!(
                                error,
                                ClientError::Api {
                                    status: 401 | 403,
                                    ..
                                } | ClientError::ApiSource {
                                    status: 401 | 403,
                                    ..
                                } | ClientError::RfqV2Api {
                                    status: 401 | 403,
                                    ..
                                }
                            );
                            let sent = tokio::select! {
                                _ = sender.closed() => false,
                                _ = tokio::time::sleep(remaining) => false,
                                result = sender.send(Err(error)) => result.is_ok(),
                            };
                            if !sent || auth_failed {
                                return;
                            }
                            break;
                        }
                    };
                    if page.next_after_sequence < cursor
                        || ((page.has_more || !page.changes.is_empty())
                            && page.next_after_sequence == cursor)
                    {
                        let error = ClientError::RfqV2ChangeFeedCursor {
                            after_sequence: cursor,
                            next_after_sequence: page.next_after_sequence,
                            has_more: page.has_more,
                        };
                        tokio::select! {
                            _ = sender.closed() => {},
                            _ = tokio::time::sleep(remaining) => {},
                            _ = sender.send(Err(error)) => {},
                        }
                        return;
                    }
                    let has_more = page.has_more;
                    let next_cursor = page.next_after_sequence;
                    // A quiet recipient feed often returns the same global
                    // cursor. Do not fill the one-page channel with no-op
                    // polls and block discovery of a later change.
                    if page.changes.is_empty() && next_cursor == cursor {
                        break;
                    }
                    let sent = tokio::select! {
                        _ = sender.closed() => false,
                        _ = tokio::time::sleep(remaining) => false,
                        result = sender.send(Ok(page)) => result.is_ok(),
                    };
                    if !sent {
                        return;
                    }
                    cursor = next_cursor;
                    if !has_more || rfq_v2_grant_remaining(expires_at_ms).is_zero() {
                        break;
                    }
                }
            }
        });
        Ok(receiver)
    }
}

impl HypercallClient {
    /// Set, renew, or remove a maker-wide V2 firm-quote cancel deadline.
    pub async fn schedule_rfq_v2_quote_cancel(
        &self,
        request: &RfqV2ScheduledQuoteCancelRequest,
    ) -> Result<RfqV2ScheduledQuoteCancelResponse> {
        classify_rfq_v2_mutation(
            None,
            request.operation_id,
            send_rfq_v2_body(
                self.request(Method::POST, RFQ_V2_CANCEL_SCHEDULE_ROUTE),
                request,
                "RFQ v2 quote cancel schedule failed",
            )
            .await,
        )
    }

    /// Relay a signed RFQ parent into the V2 delivery plane.
    pub async fn create_rfq_v2(&self, request: &CreateRfqV2Request) -> Result<CreateRfqV2Response> {
        classify_rfq_v2_mutation(
            Some(request.terms.id),
            request.terms.id,
            send_rfq_v2_body(
                self.request(Method::POST, RFQ_V2_ROUTE),
                request,
                "RFQ v2 create failed",
            )
            .await,
        )
    }

    /// Publish a non-executable indicative quote.
    pub async fn publish_rfq_v2_indicative_quote(
        &self,
        rfq_id: Uuid,
        request: &PublishRfqV2IndicativeQuoteRequest,
    ) -> Result<PublishRfqV2QuoteResponse> {
        let route = format!("{RFQ_V2_ROUTE}/{rfq_id}/quotes/indicative");
        classify_rfq_v2_mutation(
            Some(rfq_id),
            request.terms.id,
            send_rfq_v2_body(
                self.request(Method::POST, &route),
                request,
                "RFQ v2 indicative quote publication failed",
            )
            .await,
        )
    }

    /// Publish an executable firm quote for best-effort preflight and relay.
    pub async fn publish_rfq_v2_firm_quote(
        &self,
        rfq_id: Uuid,
        request: &PublishRfqV2FirmQuoteRequest,
    ) -> Result<PublishRfqV2QuoteResponse> {
        let route = format!("{RFQ_V2_ROUTE}/{rfq_id}/quotes/firm");
        classify_rfq_v2_mutation(
            Some(rfq_id),
            request.terms.id,
            send_rfq_v2_body(
                self.request(Method::POST, &route),
                request,
                "RFQ v2 firm quote publication failed",
            )
            .await,
        )
    }

    /// Ask the maker of an indicative quote for a fresh firm authorization.
    pub async fn request_rfq_v2_firm_quote(
        &self,
        rfq_id: Uuid,
        indicative_quote_id: Uuid,
        request: &RequestRfqV2FirmQuoteRequest,
    ) -> Result<RfqV2OperationResponse> {
        let route =
            format!("{RFQ_V2_ROUTE}/{rfq_id}/quotes/{indicative_quote_id}/firm-up-requests");
        classify_rfq_v2_mutation(
            Some(rfq_id),
            request.operation_id,
            send_rfq_v2_body(
                self.request(Method::POST, &route),
                request,
                "RFQ v2 firm-up request failed",
            )
            .await,
        )
    }

    /// Decline an RFQ this maker was routed. Only the taker and this maker
    /// can see the decline. It does not affect any quote or execution.
    pub async fn decline_rfq_v2(
        &self,
        rfq_id: Uuid,
        request: &DeclineRfqV2Request,
    ) -> Result<RfqV2OperationResponse> {
        let route = format!("{RFQ_V2_ROUTE}/{rfq_id}/declines");
        classify_rfq_v2_mutation(
            Some(rfq_id),
            request.operation_id,
            send_rfq_v2_body(
                self.request(Method::POST, &route),
                request,
                "RFQ v2 decline failed",
            )
            .await,
        )
    }

    /// Submit one signed firm-quote allocation for execution.
    pub async fn execute_rfq_v2(
        &self,
        rfq_id: Uuid,
        request: &RfqV2ExecutionRequest,
    ) -> Result<RfqV2ExecutionResponse> {
        let route = format!("{RFQ_V2_ROUTE}/{rfq_id}/executions");
        classify_rfq_v2_mutation(
            Some(rfq_id),
            request.operation_id,
            send_rfq_v2_body(
                self.request(Method::POST, &route),
                request,
                "RFQ v2 execution failed",
            )
            .await,
        )
    }

    /// Submit an exact, ordered, all-or-nothing multi-quote allocation.
    pub async fn sweep_rfq_v2(
        &self,
        rfq_id: Uuid,
        request: &RfqV2SweepRequest,
    ) -> Result<RfqV2ExecutionResponse> {
        let route = format!("{RFQ_V2_ROUTE}/{rfq_id}/sweeps");
        classify_rfq_v2_mutation(
            Some(rfq_id),
            request.operation_id,
            send_rfq_v2_body(
                self.request(Method::POST, &route),
                request,
                "RFQ v2 sweep failed",
            )
            .await,
        )
    }

    /// Make a signed parent RFQ terminal.
    pub async fn cancel_rfq_v2(
        &self,
        rfq_id: Uuid,
        request: &RfqV2InvalidationRequest,
    ) -> Result<RfqV2OperationResponse> {
        let route = format!("{RFQ_V2_ROUTE}/{rfq_id}/cancellations");
        classify_rfq_v2_mutation(
            Some(rfq_id),
            request.operation_id,
            send_rfq_v2_body(
                self.request(Method::POST, &route),
                request,
                "RFQ v2 cancellation failed",
            )
            .await,
        )
    }

    /// Make a signed quote terminal when its withdrawal policy permits it.
    pub async fn withdraw_rfq_v2_quote(
        &self,
        rfq_id: Uuid,
        quote_id: Uuid,
        request: &RfqV2InvalidationRequest,
    ) -> Result<RfqV2OperationResponse> {
        let route = format!("{RFQ_V2_ROUTE}/{rfq_id}/quotes/{quote_id}/withdrawals");
        classify_rfq_v2_mutation(
            Some(rfq_id),
            request.operation_id,
            send_rfq_v2_body(
                self.request(Method::POST, &route),
                request,
                "RFQ v2 quote withdrawal failed",
            )
            .await,
        )
    }

    /// Atomically withdraw a signed maker-scoped set of open quotes.
    pub async fn withdraw_rfq_v2_quotes(
        &self,
        request: &RfqV2BulkWithdrawalRequest,
    ) -> Result<RfqV2OperationResponse> {
        classify_rfq_v2_mutation(
            None,
            request.operation_id,
            send_rfq_v2_body(
                self.request(Method::POST, RFQ_V2_BULK_WITHDRAWAL_ROUTE),
                request,
                "RFQ v2 bulk quote withdrawal failed",
            )
            .await,
        )
    }

    /// Retire a parent and publish a fresh signed replacement.
    pub async fn replace_rfq_v2(
        &self,
        rfq_id: Uuid,
        request: &RfqV2ReplacementRequest<CreateRfqV2Request>,
    ) -> Result<CreateRfqV2Response> {
        let route = format!("{RFQ_V2_ROUTE}/{rfq_id}/replacements");
        classify_rfq_v2_mutation(
            Some(rfq_id),
            request.operation_id,
            send_rfq_v2_body(
                self.request(Method::POST, &route),
                request,
                "RFQ v2 replacement failed",
            )
            .await,
        )
    }

    /// Retire a quote and publish a fresh signed replacement of the same kind.
    pub async fn replace_rfq_v2_quote(
        &self,
        rfq_id: Uuid,
        quote_id: Uuid,
        request: &RfqV2ReplacementRequest<RfqV2QuoteReplacement>,
    ) -> Result<PublishRfqV2QuoteResponse> {
        let route = format!("{RFQ_V2_ROUTE}/{rfq_id}/quotes/{quote_id}/replacements");
        classify_rfq_v2_mutation(
            Some(rfq_id),
            request.operation_id,
            send_rfq_v2_body(
                self.request(Method::POST, &route),
                request,
                "RFQ v2 quote replacement failed",
            )
            .await,
        )
    }

    /// Read the authorization-aware RFQ view for the current caller.
    pub async fn get_rfq_v2(
        &self,
        rfq_id: Uuid,
        credentials: RfqV2ReadCredentials<'_>,
    ) -> Result<RfqV2Response> {
        let route = format!("{RFQ_V2_ROUTE}/{rfq_id}?view=authorized");
        send_rfq_v2_json(
            self.request(Method::GET, &route)
                .bearer_auth(credentials.bearer_token),
            "RFQ v2 authorized read failed",
        )
        .await
    }

    /// List RFQs visible to the authorized caller, including terminal history.
    pub async fn list_rfqs_v2(
        &self,
        query: &ListRfqV2Query,
        credentials: RfqV2ReadCredentials<'_>,
    ) -> Result<ListRfqV2Response> {
        send_rfq_v2_json(
            self.request(Method::GET, RFQ_V2_ROUTE)
                .query(query)
                .bearer_auth(credentials.bearer_token),
            "RFQ v2 list failed",
        )
        .await
    }

    /// List the RFQ IDs that changed for this signed account after a global
    /// sequence. Gaps in the sequence are expected.
    pub async fn list_rfq_v2_changes(
        &self,
        query: &ListRfqV2ChangesQuery,
        credentials: RfqV2ReadCredentials<'_>,
    ) -> Result<ListRfqV2ChangesResponse> {
        send_rfq_v2_json(
            self.request(Method::GET, &format!("{RFQ_V2_ROUTE}/changes"))
                .query(query)
                .bearer_auth(credentials.bearer_token),
            "RFQ v2 change feed failed",
        )
        .await
    }

    /// List quotes visible to the authorized caller for one RFQ.
    pub async fn list_rfq_v2_quotes(
        &self,
        rfq_id: Uuid,
        query: &ListRfqV2QuotesQuery,
        credentials: RfqV2ReadCredentials<'_>,
    ) -> Result<ListRfqV2QuotesResponse> {
        let route = format!("{RFQ_V2_ROUTE}/{rfq_id}/quotes");
        send_rfq_v2_json(
            self.request(Method::GET, &route)
                .query(query)
                .bearer_auth(credentials.bearer_token),
            "RFQ v2 quote list failed",
        )
        .await
    }

    /// Read one full quote visible to the authorized caller.
    pub async fn get_rfq_v2_quote(
        &self,
        rfq_id: Uuid,
        quote_id: Uuid,
        credentials: RfqV2ReadCredentials<'_>,
    ) -> Result<RfqV2QuoteView> {
        let route = format!("{RFQ_V2_ROUTE}/{rfq_id}/quotes/{quote_id}");
        send_rfq_v2_json(
            self.request(Method::GET, &route)
                .bearer_auth(credentials.bearer_token),
            "RFQ v2 quote read failed",
        )
        .await
    }
}

/// Exact parent and size constraints for deterministic local quote selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RfqV2BestQuoteCriteria<'a> {
    pub rfq_id: Uuid,
    pub terms_commitment: RfqV2Digest,
    pub requested_units: u128,
    /// The parent's remaining target from a fresh read
    /// ([`RfqV2Response::remaining_target`]). A quote's own capacity and
    /// `executable` flag do not bound the size against fills by other quotes.
    pub parent_remaining_target: u128,
    /// The parent's signed target ([`RfqV2Response::target`]).
    pub parent_target: u128,
    /// The parent's signed fill policy. Every fill must satisfy it as well
    /// as the quote's own policy.
    pub parent_fill_policy: RfqV2FillPolicy,
    /// The sizes at which every leg is a whole number of contract micro-units.
    pub parent_unit_grid: RfqV2ParentUnitGrid,
    /// The parent's signed legs, which with a quote's leg prices fix the
    /// sizes and premiums the server bounds to its fill record range.
    pub parent_legs: &'a [RfqV2PackageLeg],
    pub now_ms: u64,
    /// Signed parent limit, when the RFQ uses automatic execution.
    /// [`Self::for_parent`] takes it from the signed parent terms; never set
    /// it from any other source.
    pub auto_execution_limit: Option<RfqV2AutoExecutionLimit>,
}

impl<'a> RfqV2BestQuoteCriteria<'a> {
    /// Criteria bound to a fresh parent read: its identity,
    /// remaining and signed target, signed fill policy, leg grid, and signed
    /// auto-execution limit.
    pub fn for_parent(parent: &'a RfqV2Response, requested_units: u128, now_ms: u64) -> Self {
        Self {
            rfq_id: parent.rfq_id,
            terms_commitment: parent.terms_commitment,
            requested_units,
            parent_remaining_target: parent.remaining_target,
            parent_target: parent.target,
            parent_fill_policy: parent_fill_policy(parent),
            parent_unit_grid: RfqV2ParentUnitGrid::of(parent_legs(parent)),
            parent_legs: parent_legs(parent),
            now_ms,
            auto_execution_limit: parent_auto_execution_limit(parent),
        }
    }
}

/// Leg sizes are whole micro-units of a contract held in a `u64`. A fill of
/// `units` package units is `units * quantity_micros_per_unit / 1e6` contract
/// micro-units on each leg, and an execution is refused when any leg's
/// quantity is fractional or exceeds `u64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RfqV2ParentUnitGrid {
    /// Every admitted size is a multiple of this.
    pub step: u128,
    /// No admitted size exceeds this.
    pub ceiling: u128,
}

const CONTRACT_UNIT_MICROS: u128 = 1_000_000;

impl RfqV2ParentUnitGrid {
    /// A grid that admits every size, for a parent without legs.
    pub const UNBOUNDED: Self = Self {
        step: 1,
        ceiling: u128::MAX,
    };

    pub fn of(legs: &[RfqV2PackageLeg]) -> Self {
        legs.iter().fold(Self::UNBOUNDED, |grid, leg| {
            let quantity = leg.quantity_micros_per_unit;
            if quantity == 0 {
                // Admission refuses zero-quantity legs; never enable one.
                return Self {
                    step: 1,
                    ceiling: 0,
                };
            }
            // `quantity * units` is a multiple of 1e6 exactly when `units`
            // is a multiple of 1e6 / gcd(quantity, 1e6). That divides 1e6,
            // so the least common multiple stays a divisor of 1e6.
            let leg_step = CONTRACT_UNIT_MICROS / gcd(quantity, CONTRACT_UNIT_MICROS);
            let leg_ceiling = u128::from(u64::MAX) * CONTRACT_UNIT_MICROS / quantity;
            Self {
                step: grid.step / gcd(grid.step, leg_step) * leg_step,
                ceiling: grid.ceiling.min(leg_ceiling),
            }
        })
    }

    pub fn admits(&self, units: u128) -> bool {
        units <= self.ceiling && units.is_multiple_of(self.step)
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// The taker's signed fill policy, from either role's parent view.
pub(crate) fn parent_fill_policy(parent: &RfqV2Response) -> RfqV2FillPolicy {
    match &parent.parent {
        RfqV2AuthorizedTerms::Taker { terms } => terms.fill_policy,
        RfqV2AuthorizedTerms::Maker { terms } => terms.fill_policy,
    }
}

/// The parent's signed auto-execution limit, from either role's parent view.
pub(crate) fn parent_auto_execution_limit(
    parent: &RfqV2Response,
) -> Option<RfqV2AutoExecutionLimit> {
    match &parent.parent {
        RfqV2AuthorizedTerms::Taker { terms } => terms.auto_execution_limit,
        RfqV2AuthorizedTerms::Maker { terms } => terms.auto_execution_limit,
    }
}

/// The parent's signed legs, from either role's parent view.
pub(crate) fn parent_legs(parent: &RfqV2Response) -> &[RfqV2PackageLeg] {
    match &parent.parent {
        RfqV2AuthorizedTerms::Taker { terms } => &terms.legs,
        RfqV2AuthorizedTerms::Maker { terms } => &terms.legs,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[display]
pub enum RfqV2BestQuoteError {
    NoEligibleQuote = "no eligible firm quote covers the requested size",
}

/// The builder fee native RFQ execution charges per allocation. A quote's
/// signed `fee_limits` are ceilings, not charges, so selection prices the fee
/// actually charged; pricing the ceiling would reject executable quotes and
/// rank worse quotes first.
const CHARGED_EXECUTION_FEE_MICROS: u128 = 0;

/// Select the best eligible firm quote. Selection is local and executes
/// nothing.
pub fn select_best_rfq_v2_quote<'q>(
    quotes: &'q [RfqV2QuoteView],
    criteria: RfqV2BestQuoteCriteria<'_>,
) -> std::result::Result<&'q RfqV2QuoteView, RfqV2BestQuoteError> {
    let mut best: Option<(&RfqV2QuoteView, bool, u128, u128, u128, Uuid)> = None;

    for quote in quotes {
        let RfqV2QuoteView::Firm {
            terms,
            status,
            executable,
            remaining_capacity,
            preflight,
            ..
        } = quote
        else {
            continue;
        };
        if terms.rfq_id != criteria.rfq_id
            || terms.rfq_terms_commitment != criteria.terms_commitment
            || !executable
            || !matches!(status.as_str(), "open" | "partially_filled")
            || (status == "partially_filled"
                && matches!(terms.fill_policy, RfqV2FillPolicy::OneSmallerPackage { .. }))
            || terms.expires_at_ms <= criteria.now_ms
            || terms.reference_quantity == 0
            || criteria.requested_units == 0
            || criteria.requested_units > *remaining_capacity
            || criteria.requested_units > criteria.parent_remaining_target
            || !fill_policy_accepts(
                terms.fill_policy,
                terms.capacity,
                *remaining_capacity,
                criteria.requested_units,
            )
            || !criteria.parent_unit_grid.admits(criteria.requested_units)
            || !fill_policy_accepts(
                criteria.parent_fill_policy,
                criteria.parent_target,
                criteria.parent_remaining_target,
                criteria.requested_units,
            )
            || preflight
                .as_ref()
                .is_some_and(|result| result.status == RfqV2PreflightStatus::Rejected)
            || !quote_within_projection_range(criteria.parent_legs, terms, criteria.requested_units)
            || !quote_within_auto_limit(
                criteria.auto_execution_limit,
                terms,
                criteria.requested_units,
            )
        {
            continue;
        }

        let candidate = (
            quote,
            terms.maker_receives_reference_premium,
            terms.reference_premium_micros,
            terms.reference_quantity,
            CHARGED_EXECUTION_FEE_MICROS,
            terms.id,
        );
        let Some(current) = best else {
            best = Some(candidate);
            continue;
        };
        let units = U512::from(criteria.requested_units);
        let candidate_quantity = U512::from(candidate.3);
        let current_quantity = U512::from(current.3);
        let candidate_fee = U512::from(candidate.4);
        let current_fee = U512::from(current.4);
        let candidate_premium = U512::from(candidate.2) * units;
        let current_premium = U512::from(current.2) * units;
        let candidate_debit = candidate_fee * candidate_quantity
            + if candidate.1 {
                candidate_premium
            } else {
                U512::ZERO
            };
        let current_debit = current_fee * current_quantity
            + if current.1 {
                current_premium
            } else {
                U512::ZERO
            };
        let candidate_credit = if candidate.1 {
            U512::ZERO
        } else {
            candidate_premium
        };
        let current_credit = if current.1 {
            U512::ZERO
        } else {
            current_premium
        };
        let economics = (candidate_debit * current_quantity + current_credit * candidate_quantity)
            .cmp(&(current_debit * candidate_quantity + candidate_credit * current_quantity));
        if economics == Ordering::Less
            || (economics == Ordering::Equal && candidate.5.as_bytes() < current.5.as_bytes())
        {
            best = Some(candidate);
        }
    }

    best.map(|selected| selected.0)
        .ok_or(RfqV2BestQuoteError::NoEligibleQuote)
}

/// Exclusive magnitude bound the engine puts on every executed RFQ leg size,
/// leg price, leg premium and package premium: twelve integer digits, the
/// range of its `NUMERIC(20, 8)` fill and premium projections.
const MAX_PROJECTED_AMOUNT_EXCLUSIVE: u128 = 1_000_000_000_000;

/// Whether a fill of `requested_units` against this firm quote stays inside
/// the engine's fill record range, which refuses it otherwise. Mirrors the
/// engine's `within_projection_range` checks for one allocation, computed
/// exactly: a leg's size is `quantity_micros_per_unit * units` at 1e-12, its
/// price `price_micros` at 1e-6, and its premium their product at 1e-18,
/// debited from the taker on a bought leg and credited on a sold one. The
/// engine also bounds each option leg's underlying notional by the live
/// spot, which the client cannot see, so passing here does not prove that
/// check passes.
pub(crate) fn quote_within_projection_range(
    legs: &[RfqV2PackageLeg],
    terms: &hypercall_sdk_types::RfqV2FirmQuoteTerms,
    requested_units: u128,
) -> bool {
    let max = U512::from(MAX_PROJECTED_AMOUNT_EXCLUSIVE);
    let micros = U512::from(1_000_000_u128);
    let size_bound = max * micros * micros;
    let price_bound = max * micros;
    let premium_bound = size_bound * micros;
    let units = U512::from(requested_units);
    let (mut credit, mut debit) = (U512::ZERO, U512::ZERO);
    for (leg, priced) in legs.iter().zip(&terms.priced_legs) {
        let size = U512::from(leg.quantity_micros_per_unit) * units;
        let price = U512::from(priced.price_micros);
        let cash = price * size;
        if size >= size_bound || price >= price_bound || cash >= premium_bound {
            return false;
        }
        match leg.taker_side {
            RfqV2Side::Buy => debit += cash,
            RfqV2Side::Sell => credit += cash,
        }
    }
    credit.abs_diff(debit) < premium_bound
}

/// Whether a fill of `requested_units` against this firm quote stays inside
/// the parent's signed auto-execution premium bound. A manual parent has no
/// bound.
pub(crate) fn quote_within_auto_limit(
    limit: Option<RfqV2AutoExecutionLimit>,
    terms: &hypercall_sdk_types::RfqV2FirmQuoteTerms,
    requested_units: u128,
) -> bool {
    limit.is_none_or(|limit| {
        within_auto_limit(
            limit,
            terms.maker_receives_reference_premium,
            terms.reference_premium_micros,
            terms.reference_quantity,
            CHARGED_EXECUTION_FEE_MICROS,
            requested_units,
        )
    })
}

fn within_auto_limit(
    limit: RfqV2AutoExecutionLimit,
    maker_receives: bool,
    premium_micros: u128,
    reference_quantity: u128,
    fee_micros: u128,
    requested_units: u128,
) -> bool {
    let units = U512::from(requested_units);
    let reference = U512::from(reference_quantity);
    let scale = U512::from(1_000_000_u128);
    let premium = U512::from(premium_micros) * units * scale;
    let fee = U512::from(fee_micros) * reference * scale;
    let bound = U512::from(limit.premium_micros) * units * reference;
    match (limit.maker_receives_premium, maker_receives) {
        (true, true) => premium + fee <= bound,
        (true, false) => fee <= bound + premium,
        (false, true) => false,
        (false, false) => premium >= bound + fee,
    }
}

/// The per-fill policy rule against one signed authorization (a
/// parent's target or a quote's capacity) and its remaining amount: a
/// full-size fill is the whole signed amount, and a one-shot policy admits
/// nothing once the authorization has filled.
pub(crate) fn fill_policy_accepts(
    policy: RfqV2FillPolicy,
    signed_capacity: u128,
    remaining: u128,
    requested_units: u128,
) -> bool {
    let unfilled = remaining == signed_capacity;
    match policy {
        RfqV2FillPolicy::FullSizeOnce => unfilled && requested_units == signed_capacity,
        RfqV2FillPolicy::OneSmallerPackage { minimum, step } => {
            unfilled
                && requested_units >= minimum
                && step != 0
                && requested_units.is_multiple_of(step)
        }
        RfqV2FillPolicy::RepeatedPartial { minimum, step } => {
            requested_units >= minimum && step != 0 && requested_units.is_multiple_of(step)
        }
    }
}

#[cfg(test)]
#[path = "rfq_v2_test.rs"]
mod tests;
