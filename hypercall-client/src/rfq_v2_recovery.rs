//! Client-side recovery for RFQ v2: everything a client needs after a
//! timeout, a lost WebSocket frame, a reconnect, or a process restart.
//!
//! The server's answer is always the outcome. These helpers never infer an
//! outcome from silence: an ambiguous submission is resolved by resending the
//! identical signed bytes under the original operation ID, a lost hint is
//! recovered by re-reading the recipient change feed from the last applied
//! cursor, an execution action is enabled only from fresh server reads, and a
//! maker hold is released only on conclusive, identity-bound
//! evidence. Every recovery state is serializable so a client can persist it
//! and resume after a restart.
//!
//! Compiled with the `rfq-v2` cargo feature (on by default). See
//! [`crate::rfq_v2`] for availability and the operation ID rules.

use std::collections::{BTreeMap, BTreeSet};

use reqwest::Method;
use serde::{Deserialize, Serialize};
use tokio::time::Duration;
use uuid::Uuid;

use hypercall_sdk_types::{
    ws_protocol::WsRfqV2Changed, ListRfqV2ChangesQuery, ListRfqV2ChangesResponse,
    ListRfqV2QuotesQuery, RfqV2AuthorizedTerms, RfqV2Digest, RfqV2ExecutionRequest,
    RfqV2ExecutionResponse, RfqV2FirmQuoteTerms, RfqV2QuoteView, RfqV2Response, RfqV2SweepRequest,
    WalletAddress,
};

use crate::{
    rfq_v2::{
        ambiguous_rfq_v2_submission, fill_policy_accepts, parent_auto_execution_limit,
        parent_fill_policy, parent_legs, quote_within_auto_limit, quote_within_projection_range,
        send_rfq_v2_bytes, RfqV2ParentUnitGrid, RfqV2ReadCredentials, RFQ_V2_ROUTE,
    },
    ClientError, HypercallClient, Result,
};

/// Largest page the change feed and quote list accept.
const MAX_PAGE_LIMIT: usize = 100;

mod json_u128 {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &u128, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u128, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

/// Bounds for reconciling an execution or sweep whose outcome is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RfqV2ReconcilePolicy {
    /// Delay before each identical resubmission.
    pub retry_interval: Duration,
    /// Identical resubmissions to send before reporting the outcome as still
    /// ambiguous. Keep the total inside the signed request's expiry.
    pub max_resubmissions: u32,
}

/// The definite result of a submission that was first ambiguous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RfqV2ReconciledExecution {
    /// The server's result for the original operation ID.
    pub response: RfqV2ExecutionResponse,
    /// Identical resubmissions sent. Each carried the original operation ID
    /// and the original signed bytes; none was a new operation.
    pub resubmissions: u32,
}

impl HypercallClient {
    /// Reconcile an execution after [`ClientError::RfqV2SubmissionAmbiguous`].
    ///
    /// A timeout is not an execution result: the request may still complete
    /// later. This resends the identical signed request, byte for byte, under
    /// its original operation ID. The server treats that as an exact retry,
    /// so a completed operation returns its original result and is never
    /// applied twice. It never signs, renumbers, or alters the
    /// request. A definite rejection is returned as is; if every
    /// resubmission is still ambiguous, the result stays ambiguous.
    pub async fn reconcile_rfq_v2_execution(
        &self,
        rfq_id: Uuid,
        request: &RfqV2ExecutionRequest,
        policy: RfqV2ReconcilePolicy,
    ) -> Result<RfqV2ReconciledExecution> {
        let route = format!("{RFQ_V2_ROUTE}/{rfq_id}/executions");
        let encoded = sonic_rs::to_vec(request)?;
        self.reconcile_rfq_v2_submission(rfq_id, request.operation_id, &route, encoded, policy)
            .await
    }

    /// Reconcile a sweep after [`ClientError::RfqV2SubmissionAmbiguous`],
    /// with the same exact-retry rules as
    /// [`HypercallClient::reconcile_rfq_v2_execution`].
    pub async fn reconcile_rfq_v2_sweep(
        &self,
        rfq_id: Uuid,
        request: &RfqV2SweepRequest,
        policy: RfqV2ReconcilePolicy,
    ) -> Result<RfqV2ReconciledExecution> {
        let route = format!("{RFQ_V2_ROUTE}/{rfq_id}/sweeps");
        let encoded = sonic_rs::to_vec(request)?;
        self.reconcile_rfq_v2_submission(rfq_id, request.operation_id, &route, encoded, policy)
            .await
    }

    async fn reconcile_rfq_v2_submission(
        &self,
        rfq_id: Uuid,
        operation_id: Uuid,
        route: &str,
        encoded: Vec<u8>,
        policy: RfqV2ReconcilePolicy,
    ) -> Result<RfqV2ReconciledExecution> {
        let mut last = None;
        for resubmission in 1..=policy.max_resubmissions {
            tokio::time::sleep(policy.retry_interval).await;
            match send_rfq_v2_bytes(
                self.request(Method::POST, route),
                encoded.clone(),
                "RFQ v2 reconciliation failed",
            )
            .await
            {
                Ok(response) => {
                    return Ok(RfqV2ReconciledExecution {
                        response,
                        resubmissions: resubmission,
                    })
                }
                Err(error) if ambiguous_rfq_v2_submission(&error) => last = Some(error),
                Err(error) => return Err(error),
            }
        }
        let source = last.unwrap_or_else(|| {
            ClientError::InvalidInput("RFQ v2 reconcile policy allows no resubmission".to_string())
        });
        Err(ClientError::RfqV2SubmissionAmbiguous {
            rfq_id: Some(rfq_id),
            operation_id,
            source: Box::new(source),
        })
    }
}

/// What a received change hint means for the local view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RfqV2HintDisposition {
    /// At or below the applied cursor: a page already read covered it.
    AlreadyApplied,
    /// At or below a hint that is still waiting for a feed read. That read
    /// starts from the applied cursor, so it covers this sequence too.
    Duplicate,
    /// New: read the change feed before treating the local view as current.
    ReplayRequired,
}

/// Persistable recipient change-feed position with hint deduplication.
///
/// Global sequences have gaps for every change routed to another wallet, so
/// a gap never proves a lost frame. Only reading the recipient feed from the
/// last applied cursor does.
///
/// A hint carries no state and every feed read starts from the applied
/// cursor, so the cursor keeps only the highest hint above it. Its size stays
/// fixed however many hints arrive while the feed is unavailable or behind.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "ChangeCursorWire", into = "ChangeCursorWire")]
pub struct RfqV2ChangeCursor {
    last_applied_sequence: u64,
    /// The highest hint sequence above the cursor that no page read has
    /// covered.
    pending_high_water: Option<u64>,
}

/// The persisted cursor shape. Earlier releases stored every pending hint
/// sequence in `pending`; this one stores at most the highest. Either form
/// reads back as the same high-water mark, and a cursor written now still
/// reads in an earlier release.
#[derive(Serialize, Deserialize)]
struct ChangeCursorWire {
    last_applied_sequence: u64,
    #[serde(default)]
    pending: BTreeSet<u64>,
}

impl From<ChangeCursorWire> for RfqV2ChangeCursor {
    fn from(wire: ChangeCursorWire) -> Self {
        Self {
            last_applied_sequence: wire.last_applied_sequence,
            pending_high_water: wire
                .pending
                .last()
                .copied()
                .filter(|sequence| *sequence > wire.last_applied_sequence),
        }
    }
}

impl From<RfqV2ChangeCursor> for ChangeCursorWire {
    fn from(cursor: RfqV2ChangeCursor) -> Self {
        Self {
            last_applied_sequence: cursor.last_applied_sequence,
            pending: cursor.pending_high_water.into_iter().collect(),
        }
    }
}

impl RfqV2ChangeCursor {
    /// Resume from a persisted, fully applied sequence.
    pub fn new(last_applied_sequence: u64) -> Self {
        Self {
            last_applied_sequence,
            pending_high_water: None,
        }
    }

    /// The last change-feed sequence whose reads were applied.
    pub fn last_applied_sequence(&self) -> u64 {
        self.last_applied_sequence
    }

    /// Hint sequences retained above the cursor: at most one, the highest,
    /// because a feed read from the cursor covers every lower one.
    pub fn pending_hints(&self) -> usize {
        usize::from(self.pending_high_water.is_some())
    }

    /// The highest hint sequence above the cursor not yet covered by a feed
    /// read, if any.
    pub fn pending_high_water(&self) -> Option<u64> {
        self.pending_high_water
    }

    /// Classify one hint sequence.
    pub fn observe_hint(&mut self, sequence: u64) -> RfqV2HintDisposition {
        if sequence <= self.last_applied_sequence {
            RfqV2HintDisposition::AlreadyApplied
        } else if self
            .pending_high_water
            .is_some_and(|pending| sequence <= pending)
        {
            RfqV2HintDisposition::Duplicate
        } else {
            self.pending_high_water = Some(sequence);
            RfqV2HintDisposition::ReplayRequired
        }
    }

    /// Move to `next_after_sequence` once a page's reads are applied.
    fn commit(&mut self, next_after_sequence: u64) {
        self.last_applied_sequence = next_after_sequence;
        self.pending_high_water = self
            .pending_high_water
            .filter(|pending| *pending > next_after_sequence);
    }
}

/// Reject a change page whose cursor does not progress, as the watcher does.
#[expect(
    clippy::result_large_err,
    reason = "public client methods preserve the established unboxed ClientError contract"
)]
fn check_change_page(after_sequence: u64, page: &ListRfqV2ChangesResponse) -> Result<()> {
    if page.next_after_sequence < after_sequence
        || ((page.has_more || !page.changes.is_empty())
            && page.next_after_sequence == after_sequence)
    {
        return Err(ClientError::RfqV2ChangeFeedCursor {
            after_sequence,
            next_after_sequence: page.next_after_sequence,
            has_more: page.has_more,
        });
    }
    Ok(())
}

/// Decode an RFQ v2 change hint from a raw [`crate::WsClient`] message.
pub fn rfq_v2_change_hint(message: &sonic_rs::Value) -> Option<WsRfqV2Changed> {
    use sonic_rs::JsonValueTrait as _;
    if message.get("type").as_str() != Some("RfqV2Changed") {
        return None;
    }
    sonic_rs::from_str(&sonic_rs::to_string(message).ok()?).ok()
}

/// One object in a private mirror.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RfqV2MirrorObject {
    Parent { rfq_id: Uuid },
    Quote { rfq_id: Uuid, quote_id: Uuid },
}

/// A lifecycle state the mirror applied from a server read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2MirrorTransition {
    pub object: RfqV2MirrorObject,
    pub status: String,
    /// Remaining target for a parent, remaining capacity for a firm quote,
    /// and zero for an indicative quote, which has no executable capacity.
    #[serde(with = "json_u128")]
    pub remaining: u128,
}

/// Counters a client can export to prove it recovered without duplication.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2MirrorStats {
    pub hints_received: u64,
    pub duplicate_hints: u64,
    pub already_applied_hints: u64,
    pub pages_applied: u64,
    pub rfq_refreshes: u64,
    /// A lifecycle state applied again after the object had left it.
    /// Lifecycle states only move forward, so this stays zero.
    pub duplicate_transitions: u64,
}

/// A recipient's local view of its private RFQs, rebuilt only from
/// authorized reads named by the change feed. WebSocket hints only decide
/// when to read the feed; they never change the view themselves.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2PrivateMirror {
    cursor: RfqV2ChangeCursor,
    parents: BTreeMap<Uuid, RfqV2Response>,
    quotes: BTreeMap<Uuid, BTreeMap<Uuid, RfqV2QuoteView>>,
    transitions: Vec<RfqV2MirrorTransition>,
    stats: RfqV2MirrorStats,
}

impl RfqV2PrivateMirror {
    /// An empty mirror that reads the change feed after `cursor`.
    pub fn new(cursor: RfqV2ChangeCursor) -> Self {
        Self {
            cursor,
            ..Self::default()
        }
    }

    pub fn cursor(&self) -> &RfqV2ChangeCursor {
        &self.cursor
    }

    pub fn stats(&self) -> RfqV2MirrorStats {
        self.stats
    }

    /// Cached RFQs still live in the mirror (an open or partially filled
    /// parent) whose parent, or one of whose live quotes, has a signed
    /// deadline at or before `now_ms`. No change event announces an expiry,
    /// so these need a reread.
    fn live_rfqs_past_a_deadline(&self, now_ms: u64) -> Vec<Uuid> {
        let live = |status: &str| matches!(status, "open" | "partially_filled");
        self.parents
            .iter()
            .filter(|(_, parent)| live(&parent.status))
            .filter(|(rfq_id, parent)| {
                let parent_expiry = match &parent.parent {
                    RfqV2AuthorizedTerms::Taker { terms } => terms.expires_at_ms,
                    RfqV2AuthorizedTerms::Maker { terms } => terms.expires_at_ms,
                };
                parent_expiry <= now_ms
                    || self.quotes.get(*rfq_id).is_some_and(|quotes| {
                        quotes.values().any(|quote| match quote {
                            RfqV2QuoteView::Firm { terms, status, .. } => {
                                live(status) && terms.expires_at_ms <= now_ms
                            }
                            RfqV2QuoteView::Indicative { terms, status, .. } => {
                                live(status) && terms.expires_at_ms <= now_ms
                            }
                        })
                    })
            })
            .map(|(rfq_id, _)| *rfq_id)
            .collect()
    }

    /// Every lifecycle state applied, in order.
    pub fn transitions(&self) -> &[RfqV2MirrorTransition] {
        &self.transitions
    }

    pub fn parent(&self, rfq_id: Uuid) -> Option<&RfqV2Response> {
        self.parents.get(&rfq_id)
    }

    pub fn parents(&self) -> &BTreeMap<Uuid, RfqV2Response> {
        &self.parents
    }

    /// The authorized quotes of one RFQ, by quote ID, terminal ones included.
    pub fn quotes(&self, rfq_id: Uuid) -> Option<&BTreeMap<Uuid, RfqV2QuoteView>> {
        self.quotes.get(&rfq_id)
    }

    pub fn all_quotes(&self) -> &BTreeMap<Uuid, BTreeMap<Uuid, RfqV2QuoteView>> {
        &self.quotes
    }

    /// Record a WebSocket hint. Call
    /// [`HypercallClient::sync_rfq_v2_mirror`] on `ReplayRequired`.
    pub fn observe_hint(&mut self, hint: &WsRfqV2Changed) -> RfqV2HintDisposition {
        self.stats.hints_received += 1;
        let disposition = self.cursor.observe_hint(hint.sequence);
        match disposition {
            RfqV2HintDisposition::Duplicate => self.stats.duplicate_hints += 1,
            RfqV2HintDisposition::AlreadyApplied => self.stats.already_applied_hints += 1,
            RfqV2HintDisposition::ReplayRequired => {}
        }
        disposition
    }

    fn record(&mut self, object: RfqV2MirrorObject, status: &str, remaining: u128) {
        let last = self
            .transitions
            .iter()
            .rev()
            .find(|transition| transition.object == object);
        if last.is_some_and(|last| last.status == status && last.remaining == remaining) {
            return;
        }
        let entered_before = self.transitions.iter().any(|transition| {
            transition.object == object
                && transition.status == status
                && transition.remaining == remaining
        });
        if entered_before {
            self.stats.duplicate_transitions += 1;
        }
        self.transitions.push(RfqV2MirrorTransition {
            object,
            status: status.to_string(),
            remaining,
        });
    }

    #[expect(
        clippy::result_large_err,
        reason = "public client methods preserve the established unboxed ClientError contract"
    )]
    fn apply_refresh(
        &mut self,
        parent: RfqV2Response,
        quotes: BTreeMap<Uuid, RfqV2QuoteView>,
    ) -> Result<()> {
        let rfq_id = parent.rfq_id;
        // Quote records are retained terminal history: a refresh that no
        // longer lists a known quote is a broken read, not a deletion.
        // Validate before recording anything, so a rejected read leaves the
        // persisted mirror exactly as it was.
        if let Some(known) = self.quotes.get(&rfq_id) {
            if let Some(missing) = known.keys().find(|id| !quotes.contains_key(id)) {
                return Err(ClientError::RfqV2ProtocolViolation(format!(
                    "RFQ v2 quote {missing} vanished from the authorized history of RFQ {rfq_id}"
                )));
            }
        }
        self.record(
            RfqV2MirrorObject::Parent { rfq_id },
            &parent.status,
            parent.remaining_target,
        );
        for (quote_id, quote) in &quotes {
            let (status, remaining) = match quote {
                RfqV2QuoteView::Indicative { status, .. } => (status, 0),
                RfqV2QuoteView::Firm {
                    status,
                    remaining_capacity,
                    ..
                } => (status, *remaining_capacity),
            };
            self.record(
                RfqV2MirrorObject::Quote {
                    rfq_id,
                    quote_id: *quote_id,
                },
                status,
                remaining,
            );
        }
        self.parents.insert(rfq_id, parent);
        self.quotes.insert(rfq_id, quotes);
        self.stats.rfq_refreshes += 1;
        Ok(())
    }
}

/// Persistable position in one RFQ's authorized quote list, with the quotes
/// received so far. A client that disconnects mid-scan resumes from the
/// stored cursor; after a complete scan it resumes after the scan's
/// `as_of_sequence`, so later quotes arrive exactly once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2QuoteHistory {
    rfq_id: Uuid,
    page_limit: usize,
    next_cursor: Option<String>,
    after_sequence: u64,
    quotes: BTreeMap<Uuid, RfqV2QuoteView>,
    pages: u64,
}

impl RfqV2QuoteHistory {
    /// Start a terminal-inclusive history scan of `rfq_id`.
    #[expect(
        clippy::result_large_err,
        reason = "public client methods preserve the established unboxed ClientError contract"
    )]
    pub fn new(rfq_id: Uuid, page_limit: usize) -> Result<Self> {
        if page_limit == 0 || page_limit > MAX_PAGE_LIMIT {
            return Err(ClientError::InvalidInput(format!(
                "RFQ v2 quote page limit must be 1..={MAX_PAGE_LIMIT}"
            )));
        }
        Ok(Self {
            rfq_id,
            page_limit,
            next_cursor: None,
            after_sequence: 0,
            quotes: BTreeMap::new(),
            pages: 0,
        })
    }

    pub fn rfq_id(&self) -> Uuid {
        self.rfq_id
    }

    /// Quotes received so far, by quote ID.
    pub fn quotes(&self) -> &BTreeMap<Uuid, RfqV2QuoteView> {
        &self.quotes
    }

    /// Whether the last page ended the scan.
    pub fn complete(&self) -> bool {
        self.pages > 0 && self.next_cursor.is_none()
    }

    /// The change-feed sequence the last complete scan covered.
    pub fn as_of_sequence(&self) -> u64 {
        self.after_sequence
    }

    pub fn pages(&self) -> u64 {
        self.pages
    }
}

/// Lifecycle facts that keep an execution action disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RfqV2ExecutionBlock {
    /// Indicative quotes are never executable.
    NotFirm,
    /// The quote or parent belongs to a different RFQ or commitment.
    IdentityMismatch,
    ParentNotExecutable,
    QuoteNotExecutable,
    Expired,
    InsufficientCapacity,
    FillPolicy,
    /// Some leg would be a fractional or out-of-range contract quantity at
    /// this size, so the execution is refused.
    UnrepresentableSize,
    /// Some leg size, leg price, leg premium or the package premium at this
    /// size is outside the engine's fill record range, so the execution is
    /// refused.
    OutsideRecordRange,
    /// The quote's premium at this size is outside the parent's signed
    /// auto-execution limit.
    OutsideAutoLimit,
}

/// Whether a caller may offer one execution, decided from fresh reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2ExecutionAction {
    pub rfq_id: Uuid,
    pub quote_id: Uuid,
    pub enabled: bool,
    pub block: Option<RfqV2ExecutionBlock>,
    pub parent_status: String,
    pub quote_status: String,
    #[serde(with = "json_u128")]
    pub remaining_target: u128,
    #[serde(with = "json_u128")]
    pub remaining_capacity: u128,
    /// The caller's cached view disagreed with the server and was discarded.
    pub observation_stale: bool,
}

/// Decide an execution action from a fresh parent and quote read.
/// `observed` is the caller's cached quote, for example from an earlier
/// event; it never enables anything and is only compared with the read.
pub fn rfq_v2_execution_action(
    parent: &RfqV2Response,
    quote: &RfqV2QuoteView,
    requested_units: u128,
    now_ms: u64,
    observed: Option<&RfqV2QuoteView>,
) -> RfqV2ExecutionAction {
    let observation_stale = observed.is_some_and(|observed| observed != quote);
    let (quote_id, quote_status, remaining_capacity, block) = match quote {
        RfqV2QuoteView::Indicative { terms, status, .. } => (
            terms.id,
            status.clone(),
            0,
            Some(RfqV2ExecutionBlock::NotFirm),
        ),
        RfqV2QuoteView::Firm {
            terms,
            status,
            executable,
            remaining_capacity,
            ..
        } => {
            let block = if terms.rfq_id != parent.rfq_id
                || terms.rfq_terms_commitment != parent.terms_commitment
            {
                Some(RfqV2ExecutionBlock::IdentityMismatch)
            } else if !matches!(parent.status.as_str(), "open" | "partially_filled") {
                Some(RfqV2ExecutionBlock::ParentNotExecutable)
            } else if !executable || !matches!(status.as_str(), "open" | "partially_filled") {
                Some(RfqV2ExecutionBlock::QuoteNotExecutable)
            } else if terms.expires_at_ms <= now_ms {
                Some(RfqV2ExecutionBlock::Expired)
            } else if requested_units == 0
                || requested_units > *remaining_capacity
                || requested_units > parent.remaining_target
            {
                Some(RfqV2ExecutionBlock::InsufficientCapacity)
            } else if !fill_policy_accepts(
                terms.fill_policy,
                terms.capacity,
                *remaining_capacity,
                requested_units,
            ) || !fill_policy_accepts(
                parent_fill_policy(parent),
                parent.target,
                parent.remaining_target,
                requested_units,
            ) {
                // Every fill must satisfy the parent's signed policy as well
                // as the quote's.
                Some(RfqV2ExecutionBlock::FillPolicy)
            } else if !RfqV2ParentUnitGrid::of(parent_legs(parent)).admits(requested_units) {
                Some(RfqV2ExecutionBlock::UnrepresentableSize)
            } else if !quote_within_projection_range(parent_legs(parent), terms, requested_units) {
                Some(RfqV2ExecutionBlock::OutsideRecordRange)
            } else if !quote_within_auto_limit(
                parent_auto_execution_limit(parent),
                terms,
                requested_units,
            ) {
                Some(RfqV2ExecutionBlock::OutsideAutoLimit)
            } else {
                None
            };
            (terms.id, status.clone(), *remaining_capacity, block)
        }
    };
    RfqV2ExecutionAction {
        rfq_id: parent.rfq_id,
        quote_id,
        enabled: block.is_none(),
        block,
        parent_status: parent.status.clone(),
        quote_status,
        remaining_target: parent.remaining_target,
        remaining_capacity,
        observation_stale,
    }
}

/// Why a maker hold may be released.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "proof", rename_all = "snake_case")]
pub enum RfqV2MakerHoldProof {
    /// The maker's own quote filled; the hold becomes the fill.
    OwnFill {
        #[serde(with = "json_u128")]
        filled_units: u128,
    },
    /// The parent was filled by other quotes while this one stayed unfilled.
    CompetingWinner,
    /// The quote or parent reached a terminal state with nothing filled.
    TerminalUnfilled { status: String },
}

/// The persisted state of a maker hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RfqV2MakerHoldState {
    Unresolved,
    Released { proof: RfqV2MakerHoldProof },
}

/// What one round of server reads proved about a hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RfqV2MakerHoldEvidence {
    /// A status read was missing or 404. The outcome is unknown: missing
    /// status never proves a loss or the absence of a fill.
    Missing,
    /// The reads describe a different RFQ, commitment, quote, or maker.
    IdentityMismatch,
    /// The quote can still execute, or nothing terminal was proved yet.
    Inconclusive,
    Conclusive(RfqV2MakerHoldProof),
}

/// A maker's local reservation for a published firm quote, kept until the
/// RFQ outcome is conclusively known. Persist it with the change-feed cursor
/// before relying on the quote, and restore it after a restart: the hold
/// carries everything needed to decide the outcome again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RfqV2MakerHold {
    pub rfq_id: Uuid,
    /// The quote is the maker's operation: its ID is the idempotency key the
    /// publication, any replacement, and every fill refer to.
    pub quote_id: Uuid,
    pub terms_commitment: RfqV2Digest,
    pub maker_account: WalletAddress,
    #[serde(with = "json_u128")]
    pub capacity: u128,
    /// The quote's signed expiry, which closes its response window.
    pub response_window_ends_at_ms: u64,
    /// Last applied recipient change-feed sequence.
    pub last_cursor: u64,
    pub state: RfqV2MakerHoldState,
}

impl RfqV2MakerHold {
    /// Hold the full capacity of a firm quote the maker published.
    pub fn for_firm_quote(terms: &RfqV2FirmQuoteTerms, last_cursor: u64) -> Self {
        Self {
            rfq_id: terms.rfq_id,
            quote_id: terms.id,
            terms_commitment: terms.rfq_terms_commitment,
            maker_account: terms.maker_account,
            capacity: terms.capacity,
            response_window_ends_at_ms: terms.expires_at_ms,
            last_cursor,
            state: RfqV2MakerHoldState::Unresolved,
        }
    }

    pub fn released(&self) -> bool {
        matches!(self.state, RfqV2MakerHoldState::Released { .. })
    }

    /// Judge one round of reads. `None` is a missing or 404 read.
    pub fn evaluate(
        &self,
        parent: Option<&RfqV2Response>,
        quote: Option<&RfqV2QuoteView>,
        now_ms: u64,
    ) -> RfqV2MakerHoldEvidence {
        let (Some(parent), Some(quote)) = (parent, quote) else {
            return RfqV2MakerHoldEvidence::Missing;
        };
        let RfqV2QuoteView::Firm {
            terms,
            status,
            executable,
            remaining_capacity,
            ..
        } = quote
        else {
            return RfqV2MakerHoldEvidence::IdentityMismatch;
        };
        if parent.rfq_id != self.rfq_id
            || parent.terms_commitment != self.terms_commitment
            || terms.id != self.quote_id
            || terms.rfq_id != self.rfq_id
            || terms.rfq_terms_commitment != self.terms_commitment
            || terms.maker_account != self.maker_account
            || terms.capacity != self.capacity
            || *remaining_capacity > self.capacity
        {
            return RfqV2MakerHoldEvidence::IdentityMismatch;
        }
        if *executable {
            return RfqV2MakerHoldEvidence::Inconclusive;
        }
        let quote_terminal = matches!(
            status.as_str(),
            "filled" | "withdrawn" | "expired" | "replaced"
        );
        let parent_terminal = matches!(
            parent.status.as_str(),
            "filled" | "cancelled" | "expired" | "replaced"
        );
        let filled_units = self.capacity - remaining_capacity;
        if !(quote_terminal || parent_terminal) {
            // A used quote that can no longer execute (a spent one-shot
            // policy, or a remainder below its minimum) never fills again,
            // so its fill so far is final.
            if filled_units > 0 {
                return RfqV2MakerHoldEvidence::Conclusive(RfqV2MakerHoldProof::OwnFill {
                    filled_units,
                });
            }
            // Another quote filled part of the parent, and this unused quote
            // can no longer execute: the parent's policy admits no further
            // fill (a spent one-shot or a remainder below its minimum), and
            // its remaining target only shrinks, so it never will.
            if parent.status == "partially_filled" {
                return RfqV2MakerHoldEvidence::Conclusive(RfqV2MakerHoldProof::CompetingWinner);
            }
            return RfqV2MakerHoldEvidence::Inconclusive;
        }
        if filled_units > 0 {
            return RfqV2MakerHoldEvidence::Conclusive(RfqV2MakerHoldProof::OwnFill {
                filled_units,
            });
        }
        if parent.status == "filled" {
            return RfqV2MakerHoldEvidence::Conclusive(RfqV2MakerHoldProof::CompetingWinner);
        }
        let terminal = if quote_terminal {
            status
        } else {
            &parent.status
        };
        // Expiry is evidence only once this hold's own response window closed.
        if terminal == "expired" && now_ms < self.response_window_ends_at_ms {
            return RfqV2MakerHoldEvidence::Inconclusive;
        }
        RfqV2MakerHoldEvidence::Conclusive(RfqV2MakerHoldProof::TerminalUnfilled {
            status: terminal.clone(),
        })
    }

    /// Release on conclusive evidence; anything else keeps the hold.
    pub fn apply(&mut self, evidence: &RfqV2MakerHoldEvidence) {
        if let (RfqV2MakerHoldState::Unresolved, RfqV2MakerHoldEvidence::Conclusive(proof)) =
            (&self.state, evidence)
        {
            self.state = RfqV2MakerHoldState::Released {
                proof: proof.clone(),
            };
        }
    }
}

fn not_found(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Api { status: 404, .. } | ClientError::RfqV2Api { status: 404, .. }
    )
}

#[expect(
    clippy::result_large_err,
    reason = "public client methods preserve the established unboxed ClientError contract"
)]
fn unix_now_ms() -> Result<u64> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| {
            ClientError::InvalidInput(format!("system clock before epoch: {error}"))
        })?;
    u64::try_from(elapsed.as_millis())
        .map_err(|_| ClientError::InvalidInput("system clock overflows u64 ms".to_string()))
}

impl HypercallClient {
    /// Fetch the next page of a quote history and merge it. A quote ID seen
    /// on an earlier page is a protocol violation and fails the page.
    pub async fn next_rfq_v2_quote_page(
        &self,
        history: &mut RfqV2QuoteHistory,
        credentials: RfqV2ReadCredentials<'_>,
    ) -> Result<()> {
        let query = ListRfqV2QuotesQuery {
            cursor: history.next_cursor.clone(),
            limit: Some(history.page_limit),
            after_sequence: (history.next_cursor.is_none() && history.after_sequence > 0)
                .then_some(history.after_sequence),
            include_terminal: Some(true),
            ..ListRfqV2QuotesQuery::default()
        };
        let page = self
            .list_rfq_v2_quotes(history.rfq_id, &query, credentials)
            .await?;
        let mut received = BTreeMap::new();
        for quote in page.quotes {
            let id = match &quote {
                RfqV2QuoteView::Indicative { terms, .. } => terms.id,
                RfqV2QuoteView::Firm { terms, .. } => terms.id,
            };
            if history.quotes.contains_key(&id) || received.insert(id, quote).is_some() {
                return Err(ClientError::RfqV2ProtocolViolation(format!(
                    "RFQ v2 quote {id} was delivered twice in the history of RFQ {}",
                    history.rfq_id
                )));
            }
        }
        let next_cursor = if page.page.has_more {
            Some(page.page.next_cursor.ok_or_else(|| {
                ClientError::RfqV2ProtocolViolation(
                    "RFQ v2 quote page has more results but no cursor".to_string(),
                )
            })?)
        } else {
            None
        };
        history.quotes.append(&mut received);
        if next_cursor.is_none() {
            history.after_sequence = page.as_of_sequence;
        }
        history.next_cursor = next_cursor;
        history.pages += 1;
        Ok(())
    }

    /// Page a quote history until the scan is complete.
    pub async fn sync_rfq_v2_quote_history(
        &self,
        history: &mut RfqV2QuoteHistory,
        credentials: RfqV2ReadCredentials<'_>,
    ) -> Result<()> {
        loop {
            self.next_rfq_v2_quote_page(
                history,
                RfqV2ReadCredentials {
                    bearer_token: credentials.bearer_token,
                },
            )
            .await?;
            if history.next_cursor.is_none() {
                return Ok(());
            }
        }
    }

    /// Read the recipient change feed from the mirror's last applied cursor
    /// and refresh each named RFQ from authorized reads. The cursor advances
    /// page by page, only after that page's reads are applied, so an error
    /// leaves a mirror that resumes safely. Call it after connecting, after
    /// every reconnect, whenever a hint requires it, and periodically.
    /// It only reads; it never resubmits an operation.
    pub async fn sync_rfq_v2_mirror(
        &self,
        mirror: &mut RfqV2PrivateMirror,
        credentials: RfqV2ReadCredentials<'_>,
    ) -> Result<BTreeSet<Uuid>> {
        let token = credentials.bearer_token;
        let mut refreshed = BTreeSet::new();
        loop {
            let after_sequence = mirror.cursor.last_applied_sequence;
            let query = ListRfqV2ChangesQuery {
                after_sequence: Some(after_sequence),
                limit: Some(MAX_PAGE_LIMIT as u32),
            };
            let page = self
                .list_rfq_v2_changes(
                    &query,
                    RfqV2ReadCredentials {
                        bearer_token: token,
                    },
                )
                .await?;
            check_change_page(after_sequence, &page)?;
            let named: BTreeSet<Uuid> = page.changes.iter().map(|change| change.rfq_id).collect();
            for rfq_id in named {
                self.refresh_mirror_rfq(mirror, rfq_id, token).await?;
                refreshed.insert(rfq_id);
            }
            mirror.cursor.commit(page.next_after_sequence);
            mirror.stats.pages_applied += 1;
            if !page.has_more {
                break;
            }
        }
        // Expiry is derived from the signed deadline, so no change event
        // announces it: reread every cached RFQ that is still live but has a
        // deadline the wall clock has passed.
        for rfq_id in mirror.live_rfqs_past_a_deadline(unix_now_ms()?) {
            if refreshed.insert(rfq_id) {
                self.refresh_mirror_rfq(mirror, rfq_id, token).await?;
            }
        }
        Ok(refreshed)
    }

    /// Reread one RFQ and its whole quote history into the mirror.
    async fn refresh_mirror_rfq(
        &self,
        mirror: &mut RfqV2PrivateMirror,
        rfq_id: Uuid,
        token: &str,
    ) -> Result<()> {
        let parent = self
            .get_rfq_v2(
                rfq_id,
                RfqV2ReadCredentials {
                    bearer_token: token,
                },
            )
            .await?;
        let mut history = RfqV2QuoteHistory::new(rfq_id, MAX_PAGE_LIMIT)?;
        self.sync_rfq_v2_quote_history(
            &mut history,
            RfqV2ReadCredentials {
                bearer_token: token,
            },
        )
        .await?;
        mirror.apply_refresh(parent, history.quotes)
    }

    /// Decide whether to offer an execution from fresh server reads of the
    /// parent and quote. A cached or event-derived `observed` view is never
    /// trusted: a stale "capacity remains" event cannot enable a fill whose
    /// capacity is already used.
    pub async fn refresh_rfq_v2_execution_action(
        &self,
        rfq_id: Uuid,
        quote_id: Uuid,
        requested_units: u128,
        observed: Option<&RfqV2QuoteView>,
        credentials: RfqV2ReadCredentials<'_>,
    ) -> Result<RfqV2ExecutionAction> {
        let token = credentials.bearer_token;
        let parent = self
            .get_rfq_v2(
                rfq_id,
                RfqV2ReadCredentials {
                    bearer_token: token,
                },
            )
            .await?;
        let quote = self
            .get_rfq_v2_quote(
                rfq_id,
                quote_id,
                RfqV2ReadCredentials {
                    bearer_token: token,
                },
            )
            .await?;
        Ok(rfq_v2_execution_action(
            &parent,
            &quote,
            requested_units,
            unix_now_ms()?,
            observed,
        ))
    }

    /// Re-decide a maker hold from server reads, then advance its
    /// change-feed cursor. A 404 or missing status leaves the hold
    /// unresolved; only identity-bound own-fill, competing-winner, or
    /// terminal evidence releases it. A released hold is final.
    pub async fn reconcile_rfq_v2_maker_hold(
        &self,
        hold: &mut RfqV2MakerHold,
        credentials: RfqV2ReadCredentials<'_>,
    ) -> Result<RfqV2MakerHoldEvidence> {
        if let RfqV2MakerHoldState::Released { proof } = &hold.state {
            return Ok(RfqV2MakerHoldEvidence::Conclusive(proof.clone()));
        }
        let token = credentials.bearer_token;
        // Read the feed position first: every change up to it happened
        // before the status reads below, so saving it afterwards can
        // never skip a change those reads did not observe.
        let mut cursor = hold.last_cursor;
        loop {
            let query = ListRfqV2ChangesQuery {
                after_sequence: Some(cursor),
                limit: Some(MAX_PAGE_LIMIT as u32),
            };
            let page = self
                .list_rfq_v2_changes(
                    &query,
                    RfqV2ReadCredentials {
                        bearer_token: token,
                    },
                )
                .await?;
            check_change_page(cursor, &page)?;
            cursor = page.next_after_sequence;
            if !page.has_more {
                break;
            }
        }
        let parent = match self
            .get_rfq_v2(
                hold.rfq_id,
                RfqV2ReadCredentials {
                    bearer_token: token,
                },
            )
            .await
        {
            Ok(parent) => Some(parent),
            Err(error) if not_found(&error) => None,
            Err(error) => return Err(error),
        };
        let quote = match self
            .get_rfq_v2_quote(
                hold.rfq_id,
                hold.quote_id,
                RfqV2ReadCredentials {
                    bearer_token: token,
                },
            )
            .await
        {
            Ok(quote) => Some(quote),
            Err(error) if not_found(&error) => None,
            Err(error) => return Err(error),
        };
        let evidence = hold.evaluate(parent.as_ref(), quote.as_ref(), unix_now_ms()?);
        hold.apply(&evidence);
        hold.last_cursor = cursor;
        Ok(evidence)
    }
}

#[cfg(test)]
#[path = "rfq_v2_recovery_test.rs"]
mod tests;
