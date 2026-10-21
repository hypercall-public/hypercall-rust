use alloy::{
    primitives::{keccak256, Address, FixedBytes},
    signers::{Signature, Signer},
};
use hypersdk::hypercore::types::api::UpdateLeverage;
use hypersdk::hypercore::{
    signing::agent_signing_hash,
    types::{
        Action, ActionRequest, BatchCancel, BatchCancelCloid, BatchModify, BatchOrder, Builder,
        Cancel, CancelByCloid, Modify, OkResponse, OrderGrouping, OrderRequest,
        OrderResponseStatus, OrderTypePlacement, Response, ScheduleCancel,
        TimeInForce as HyperliquidTimeInForce, TpSl,
    },
    Chain, Cloid, OidOrCloid,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::error::Error;

use crate::{
    prepared_twap::{
        PreparedModifyTarget, PreparedPerpModify, PreparedPerpTwapCancel, PreparedPerpTwapOrder,
        TwapAction, TwapCancelWire, TwapOrderWire, TWAP_MAX_MINUTES, TWAP_MIN_MINUTES,
    },
    signing_wire::{action_connection_id, twap_connection_id},
    Tif,
};

#[cfg(test)]
#[path = "prepared_test.rs"]
mod tests;

#[derive(Debug, thiserror::Error)]
#[displaystr::display]
pub enum PreparedHyperliquidActionError {
    InvalidInput(String) = "invalid prepared Hyperliquid action: {_0}",
    Signing {
        operation: &'static str,
        #[source]
        source: Box<dyn Error + Send + Sync + 'static>,
    } = "prepared Hyperliquid action signing failed: failed to {operation}: {source}",
    Serialization {
        operation: &'static str,
        #[source]
        source: serde_json::Error,
    } = "invalid prepared Hyperliquid action: {operation}: {source}",
    SignerMismatch {
        recovered: Address,
        expected: Address,
    } = "prepared Hyperliquid action signing failed: prepared Hyperliquid signature recovered {recovered}, expected {expected}",
    Hash {
        #[source]
        source: Box<dyn Error + Send + Sync + 'static>,
    } = "invalid prepared Hyperliquid action: failed to hash prepared Hyperliquid action: {source}",
}

type Result<T> = std::result::Result<T, PreparedHyperliquidActionError>;
use PreparedHyperliquidActionError as ClientError;

pub const PREPARED_HYPERLIQUID_ACTION_VERSION: u16 = 2;
const LEGACY_PREPARED_HYPERLIQUID_ACTION_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HyperliquidSubmissionClassification {
    Accepted,
    Rejected,
    Unknown,
}

/// Most children one prepared `batchModify` may carry. Hyperliquid documents
/// no limit; this structural bound matches the `order` batch cap so a parser
/// never decodes an unbounded batch. Deployments set a lower per-instance cap.
pub const MAX_BATCH_MODIFY_CHILDREN: usize = 100;

/// Per-child classification of one `batchModify` response, in signed order,
/// plus the whole-action classification: accepted only when every child was
/// accepted, rejected only when every child was rejected, otherwise unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidBatchModifyClassification {
    pub batch: HyperliquidSubmissionClassification,
    pub children: Vec<HyperliquidSubmissionClassification>,
}

impl HyperliquidBatchModifyClassification {
    fn uniform(class: HyperliquidSubmissionClassification, children: usize) -> Self {
        Self {
            batch: class,
            children: vec![class; children],
        }
    }

    fn from_children(children: Vec<HyperliquidSubmissionClassification>) -> Self {
        let all = |class| children.iter().all(|child| *child == class);
        let batch = if all(HyperliquidSubmissionClassification::Accepted) {
            HyperliquidSubmissionClassification::Accepted
        } else if all(HyperliquidSubmissionClassification::Rejected) {
            HyperliquidSubmissionClassification::Rejected
        } else {
            HyperliquidSubmissionClassification::Unknown
        };
        Self { batch, children }
    }
}

/// Economic fields for the first direct API execution slice.
///
/// The caller resolves the asset index and quantizes price and size before
/// preparation. Once prepared, none of these fields can be rebuilt by the
/// gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPerpLimitOrder {
    pub asset: u32,
    pub is_buy: bool,
    pub limit_px: Decimal,
    pub size: Decimal,
    pub reduce_only: bool,
    pub tif: Tif,
    pub cloid: u128,
}

/// Exact venue identity for cancelling one perpetual order by client order ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPerpCancelByCloid {
    pub asset: u32,
    pub cloid: u128,
}

/// Exact venue identity for cancelling one perpetual order by exchange order ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPerpCancelByOid {
    pub asset: u32,
    pub oid: u64,
}

/// The exchange `order` action can carry limits and triggered stop or take
/// orders. Market execution is an IOC limit with an explicit price bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedPerpOrderType {
    Limit(Tif),
    Trigger {
        is_market: bool,
        trigger_px: Decimal,
        tpsl: PreparedPerpTriggerKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparedPerpTriggerKind {
    TakeProfit,
    StopLoss,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreparedPerpOrderGrouping {
    None,
    NormalTpsl,
    PositionTpsl,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPerpOrder {
    pub asset: u32,
    pub is_buy: bool,
    pub limit_px: Decimal,
    pub size: Decimal,
    pub reduce_only: bool,
    pub order_type: PreparedPerpOrderType,
    pub cloid: u128,
}

/// Every child and the grouping of one validated native `order` action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPerpOrderBatch {
    pub orders: Vec<PreparedPerpOrder>,
    pub grouping: PreparedPerpOrderGrouping,
}

/// Exact mode and leverage requested for one Hyperliquid perpetual asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedPerpLeverageUpdate {
    pub asset: u32,
    pub is_cross: bool,
    pub leverage: u32,
}

impl PreparedPerpLimitOrder {
    pub fn new(
        asset: u32,
        is_buy: bool,
        limit_px: Decimal,
        size: Decimal,
        tif: Tif,
        cloid: u128,
    ) -> Self {
        Self {
            asset,
            is_buy,
            limit_px,
            size,
            reduce_only: false,
            tif,
            cloid,
        }
    }

    pub fn reduce_only(mut self, reduce_only: bool) -> Self {
        self.reduce_only = reduce_only;
        self
    }
}

/// Immutable unsigned Hyperliquid request.
///
/// `canonical_bytes` is the journal and gateway transport representation.
/// The gateway must decode those exact bytes, verify canonical encoding and
/// `action_digest`, attach only the enclave signature, and submit the resulting
/// `ActionRequest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedHyperliquidAction {
    pub version: u16,
    pub chain: Chain,
    /// For a TWAP action this is `Action::Noop`: the SDK `Action` has no TWAP
    /// variant, so the real action lives in `twap`. Every other action leaves
    /// `twap` unset, which keeps its canonical bytes unchanged.
    pub action: Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twap: Option<TwapAction>,
    pub nonce: u64,
    pub vault_address: Option<Address>,
    pub expires_after_ms: u64,
    pub action_digest: FixedBytes<32>,
}

impl PreparedHyperliquidAction {
    /// Venue acceptance is separate from confirmation of the account setting.
    pub fn classify_leverage_response(body: &[u8]) -> HyperliquidSubmissionClassification {
        match serde_json::from_slice::<Response>(body) {
            Ok(Response::Err(_)) => HyperliquidSubmissionClassification::Rejected,
            Ok(Response::Ok(OkResponse::Default)) => HyperliquidSubmissionClassification::Accepted,
            _ => HyperliquidSubmissionClassification::Unknown,
        }
    }

    /// Classifies only the documented Hyperliquid order response shape.
    ///
    /// HTTP status alone cannot prove whether venue admission occurred. An
    /// unrecognized or contradictory response therefore remains unknown.
    pub fn classify_submission_response(body: &[u8]) -> HyperliquidSubmissionClassification {
        match serde_json::from_slice::<Response>(body) {
            Ok(Response::Err(_)) => HyperliquidSubmissionClassification::Rejected,
            Ok(Response::Ok(OkResponse::Order { statuses })) if statuses.len() == 1 => {
                match &statuses[0] {
                    OrderResponseStatus::Success
                    | OrderResponseStatus::WaitingForFill
                    | OrderResponseStatus::WaitingForTrigger
                    | OrderResponseStatus::Resting { .. }
                    | OrderResponseStatus::Filled { .. } => {
                        HyperliquidSubmissionClassification::Accepted
                    }
                    OrderResponseStatus::Error(_) => HyperliquidSubmissionClassification::Rejected,
                }
            }
            Ok(Response::Ok(OkResponse::Cancel { statuses })) if statuses.len() == 1 => {
                match &statuses[0] {
                    OrderResponseStatus::Success => HyperliquidSubmissionClassification::Accepted,
                    OrderResponseStatus::Error(_) => HyperliquidSubmissionClassification::Rejected,
                    _ => HyperliquidSubmissionClassification::Unknown,
                }
            }
            _ => HyperliquidSubmissionClassification::Unknown,
        }
    }

    /// Classifies an `order` action with `expected_orders` children. The venue
    /// answers with one status per child, in order. Only an all-accepted
    /// response is acceptance and only an all-rejected (or top-level error)
    /// response is rejection. A mixed or miscounted response stays unknown, so
    /// the per-child reconciler settles each reservation from venue state.
    pub fn classify_order_batch_response(
        body: &[u8],
        expected_orders: usize,
    ) -> HyperliquidSubmissionClassification {
        match serde_json::from_slice::<Response>(body) {
            Ok(Response::Err(_)) => HyperliquidSubmissionClassification::Rejected,
            Ok(Response::Ok(OkResponse::Order { statuses }))
                if expected_orders > 0 && statuses.len() == expected_orders =>
            {
                let accepted = statuses.iter().all(|status| {
                    matches!(
                        status,
                        OrderResponseStatus::Success
                            | OrderResponseStatus::WaitingForFill
                            | OrderResponseStatus::WaitingForTrigger
                            | OrderResponseStatus::Resting { .. }
                            | OrderResponseStatus::Filled { .. }
                    )
                });
                let rejected = statuses
                    .iter()
                    .all(|status| matches!(status, OrderResponseStatus::Error(_)));
                if accepted {
                    HyperliquidSubmissionClassification::Accepted
                } else if rejected {
                    HyperliquidSubmissionClassification::Rejected
                } else {
                    HyperliquidSubmissionClassification::Unknown
                }
            }
            _ => HyperliquidSubmissionClassification::Unknown,
        }
    }

    /// A schedule-cancel action has no per-order status. Its documented
    /// successful exchange response is the default response variant.
    pub fn classify_schedule_cancel_response(body: &[u8]) -> HyperliquidSubmissionClassification {
        match serde_json::from_slice::<Response>(body) {
            Ok(Response::Ok(OkResponse::Default)) => HyperliquidSubmissionClassification::Accepted,
            Ok(Response::Err(_)) => HyperliquidSubmissionClassification::Rejected,
            _ => HyperliquidSubmissionClassification::Unknown,
        }
    }

    /// A modify answers with the order statuses of the replacement or with the
    /// bare `modify` response. A top-level error or an errored status is a
    /// rejection; anything else unrecognized stays unknown.
    pub fn classify_modify_response(body: &[u8]) -> HyperliquidSubmissionClassification {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
            return HyperliquidSubmissionClassification::Unknown;
        };
        match value.get("status").and_then(serde_json::Value::as_str) {
            Some("err") => return HyperliquidSubmissionClassification::Rejected,
            Some("ok") => {}
            _ => return HyperliquidSubmissionClassification::Unknown,
        }
        match value
            .pointer("/response/type")
            .and_then(serde_json::Value::as_str)
        {
            Some("modify" | "batchModify") => HyperliquidSubmissionClassification::Accepted,
            Some("default")
                if value
                    .get("response")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|response| response.len() == 1) =>
            {
                HyperliquidSubmissionClassification::Accepted
            }
            Some("order") => Self::classify_submission_response(body),
            _ => HyperliquidSubmissionClassification::Unknown,
        }
    }

    /// Classify a `batchModify` with `expected` children, per child.
    ///
    /// The venue answers a multi-order modify with an `order` response
    /// carrying one status per child, in signed order: a resting, filled or
    /// success status means that child's replacement was applied, an error
    /// status means it was not. A top-level error rejects every child. A
    /// single-child batch also accepts the bare `modify`/`default` response
    /// [`Self::classify_modify_response`] recognizes. Anything else, including
    /// a miscounted status list, leaves every child unknown, so the per-child
    /// reconciler settles it from venue state.
    pub fn classify_batch_modify_response(
        body: &[u8],
        expected: usize,
    ) -> HyperliquidBatchModifyClassification {
        let uniform = |class| HyperliquidBatchModifyClassification::uniform(class, expected);
        if expected == 0 {
            return uniform(HyperliquidSubmissionClassification::Unknown);
        }
        match serde_json::from_slice::<Response>(body) {
            Ok(Response::Err(_)) => {
                return uniform(HyperliquidSubmissionClassification::Rejected);
            }
            Ok(Response::Ok(OkResponse::Order { statuses })) => {
                if statuses.len() != expected {
                    return uniform(HyperliquidSubmissionClassification::Unknown);
                }
                let children = statuses
                    .iter()
                    .map(|status| match status {
                        OrderResponseStatus::Success
                        | OrderResponseStatus::Resting { .. }
                        | OrderResponseStatus::Filled { .. } => {
                            HyperliquidSubmissionClassification::Accepted
                        }
                        OrderResponseStatus::Error(_) => {
                            HyperliquidSubmissionClassification::Rejected
                        }
                        // A modify replacement is a plain limit order; a
                        // trigger or waiting status is not a modify outcome.
                        OrderResponseStatus::WaitingForFill
                        | OrderResponseStatus::WaitingForTrigger => {
                            HyperliquidSubmissionClassification::Unknown
                        }
                    })
                    .collect();
                return HyperliquidBatchModifyClassification::from_children(children);
            }
            _ => {}
        }
        if expected == 1 {
            return uniform(Self::classify_modify_response(body));
        }
        uniform(HyperliquidSubmissionClassification::Unknown)
    }

    /// `twapOrder` answers `{"status":{"running":{"twapId":N}}}` or
    /// `{"status":{"error":"..."}}` inside an ok response.
    pub fn classify_twap_order_response(body: &[u8]) -> HyperliquidSubmissionClassification {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
            return HyperliquidSubmissionClassification::Unknown;
        };
        match value.get("status").and_then(serde_json::Value::as_str) {
            Some("err") => return HyperliquidSubmissionClassification::Rejected,
            Some("ok") => {}
            _ => return HyperliquidSubmissionClassification::Unknown,
        }
        if value
            .pointer("/response/type")
            .and_then(serde_json::Value::as_str)
            != Some("twapOrder")
        {
            return HyperliquidSubmissionClassification::Unknown;
        }
        if Self::twap_id_from_order_response(body).is_some() {
            HyperliquidSubmissionClassification::Accepted
        } else if value.pointer("/response/data/status/error").is_some() {
            HyperliquidSubmissionClassification::Rejected
        } else {
            HyperliquidSubmissionClassification::Unknown
        }
    }

    /// The venue-assigned TWAP id of an accepted `twapOrder` response.
    pub fn twap_id_from_order_response(body: &[u8]) -> Option<u64> {
        let value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
        if value.get("status")?.as_str()? != "ok" {
            return None;
        }
        value
            .pointer("/response/data/status/running/twapId")?
            .as_u64()
    }

    /// `twapCancel` answers `{"status":"success"}` or `{"status":{"error":".."}}`.
    pub fn classify_twap_cancel_response(body: &[u8]) -> HyperliquidSubmissionClassification {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
            return HyperliquidSubmissionClassification::Unknown;
        };
        match value.get("status").and_then(serde_json::Value::as_str) {
            Some("err") => return HyperliquidSubmissionClassification::Rejected,
            Some("ok") => {}
            _ => return HyperliquidSubmissionClassification::Unknown,
        }
        if value
            .pointer("/response/type")
            .and_then(serde_json::Value::as_str)
            != Some("twapCancel")
        {
            return HyperliquidSubmissionClassification::Unknown;
        }
        match value.pointer("/response/data/status") {
            Some(serde_json::Value::String(status)) if status == "success" => {
                HyperliquidSubmissionClassification::Accepted
            }
            Some(status) if status.get("error").is_some() => {
                HyperliquidSubmissionClassification::Rejected
            }
            _ => HyperliquidSubmissionClassification::Unknown,
        }
    }

    pub fn prepare_limit_order(
        chain: Chain,
        order: PreparedPerpLimitOrder,
        nonce: u64,
        vault_address: Option<Address>,
        expires_after_ms: u64,
    ) -> Result<Self> {
        Self::prepare_limit_order_with_builder(
            chain,
            order,
            nonce,
            vault_address,
            expires_after_ms,
            None,
        )
    }

    /// Prepare an exact order whose optional builder receives no additional fee.
    pub fn prepare_limit_order_with_builder(
        chain: Chain,
        order: PreparedPerpLimitOrder,
        nonce: u64,
        vault_address: Option<Address>,
        expires_after_ms: u64,
        builder_address: Option<Address>,
    ) -> Result<Self> {
        if builder_address == Some(Address::ZERO) {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid builder address must not be zero".to_string(),
            ));
        }
        if crate::prepared_spot::is_spot_asset(order.asset) {
            return Err(ClientError::InvalidInput(format!(
                "Hyperliquid asset {} is a spot asset; prepare it as a spot order",
                order.asset
            )));
        }
        let asset = usize::try_from(order.asset).map_err(|_| {
            ClientError::InvalidInput("Hyperliquid asset index does not fit usize".to_string())
        })?;
        let action = Action::Order(BatchOrder {
            orders: vec![OrderRequest {
                asset,
                is_buy: order.is_buy,
                limit_px: order.limit_px,
                sz: order.size,
                reduce_only: order.reduce_only,
                order_type: OrderTypePlacement::Limit {
                    tif: match order.tif {
                        Tif::Gtc => HyperliquidTimeInForce::Gtc,
                        Tif::Ioc => HyperliquidTimeInForce::Ioc,
                        Tif::Alo => HyperliquidTimeInForce::Alo,
                    },
                },
                cloid: cloid_from_u128(order.cloid),
            }],
            grouping: OrderGrouping::Na,
            builder: builder_address.map(|builder_address| Builder {
                builder_address,
                fee: 0,
            }),
        });
        let action_digest = action_digest(&action, chain, nonce, vault_address, expires_after_ms)?;
        let prepared = Self {
            version: PREPARED_HYPERLIQUID_ACTION_VERSION,
            chain,
            action,
            twap: None,
            nonce,
            vault_address,
            expires_after_ms,
            action_digest,
        };
        prepared.validate()?;
        Ok(prepared)
    }

    /// Freeze an exchange-native batch. One builder field attributes every
    /// child order, including attached stop-loss and take-profit orders.
    pub fn prepare_order_batch_with_builder(
        chain: Chain,
        orders: Vec<PreparedPerpOrder>,
        grouping: PreparedPerpOrderGrouping,
        nonce: u64,
        vault_address: Option<Address>,
        expires_after_ms: u64,
        builder_address: Address,
    ) -> Result<Self> {
        if builder_address == Address::ZERO {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid builder address must not be zero".to_string(),
            ));
        }
        let orders = orders
            .into_iter()
            .map(|order| {
                let asset = usize::try_from(order.asset).map_err(|_| {
                    ClientError::InvalidInput(
                        "Hyperliquid asset index does not fit usize".to_string(),
                    )
                })?;
                let order_type = match order.order_type {
                    PreparedPerpOrderType::Limit(tif) => OrderTypePlacement::Limit {
                        tif: match tif {
                            Tif::Gtc => HyperliquidTimeInForce::Gtc,
                            Tif::Ioc => HyperliquidTimeInForce::Ioc,
                            Tif::Alo => HyperliquidTimeInForce::Alo,
                        },
                    },
                    PreparedPerpOrderType::Trigger {
                        is_market,
                        trigger_px,
                        tpsl,
                    } => OrderTypePlacement::Trigger {
                        is_market,
                        trigger_px,
                        tpsl: match tpsl {
                            PreparedPerpTriggerKind::TakeProfit => TpSl::Tp,
                            PreparedPerpTriggerKind::StopLoss => TpSl::Sl,
                        },
                    },
                };
                Ok(OrderRequest {
                    asset,
                    is_buy: order.is_buy,
                    limit_px: order.limit_px,
                    sz: order.size,
                    reduce_only: order.reduce_only,
                    order_type,
                    cloid: cloid_from_u128(order.cloid),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Self::prepare_action(
            chain,
            Action::Order(BatchOrder {
                orders,
                grouping: match grouping {
                    PreparedPerpOrderGrouping::None => OrderGrouping::Na,
                    PreparedPerpOrderGrouping::NormalTpsl => OrderGrouping::NormalTpsl,
                    PreparedPerpOrderGrouping::PositionTpsl => OrderGrouping::PositionTpsl,
                },
                builder: Some(Builder {
                    builder_address,
                    fee: 0,
                }),
            }),
            nonce,
            vault_address,
            expires_after_ms,
        )
    }

    pub fn prepare_cancel_by_cloid(
        chain: Chain,
        cancel: PreparedPerpCancelByCloid,
        nonce: u64,
        vault_address: Option<Address>,
        expires_after_ms: u64,
    ) -> Result<Self> {
        let action = Action::CancelByCloid(BatchCancelCloid {
            cancels: vec![CancelByCloid {
                asset: cancel.asset,
                cloid: cloid_from_u128(cancel.cloid),
            }],
        });
        let action_digest = action_digest(&action, chain, nonce, vault_address, expires_after_ms)?;
        let prepared = Self {
            version: PREPARED_HYPERLIQUID_ACTION_VERSION,
            chain,
            action,
            twap: None,
            nonce,
            vault_address,
            expires_after_ms,
            action_digest,
        };
        prepared.validate()?;
        Ok(prepared)
    }

    pub fn prepare_cancel_by_oid(
        chain: Chain,
        cancel: PreparedPerpCancelByOid,
        nonce: u64,
        vault_address: Option<Address>,
        expires_after_ms: u64,
    ) -> Result<Self> {
        if cancel.oid == 0 {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid oid must not be zero".to_string(),
            ));
        }
        Self::prepare_action(
            chain,
            Action::Cancel(BatchCancel {
                cancels: vec![Cancel {
                    asset: cancel.asset as usize,
                    oid: cancel.oid,
                }],
            }),
            nonce,
            vault_address,
            expires_after_ms,
        )
    }

    /// `None` clears an existing dead-man's-switch schedule.
    pub fn prepare_schedule_cancel(
        chain: Chain,
        trigger_at_ms: Option<u64>,
        nonce: u64,
        vault_address: Option<Address>,
        expires_after_ms: u64,
    ) -> Result<Self> {
        if trigger_at_ms == Some(0) {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid schedule time must not be zero".to_string(),
            ));
        }
        Self::prepare_action(
            chain,
            Action::ScheduleCancel(ScheduleCancel {
                time: trigger_at_ms,
            }),
            nonce,
            vault_address,
            expires_after_ms,
        )
    }

    /// Replace one resting limit order in place. Hyperliquid documents no
    /// builder field on `batchModify`, so none is attached. The replacement
    /// keeps its client order id.
    pub fn prepare_modify(
        chain: Chain,
        modify: PreparedPerpModify,
        nonce: u64,
        vault_address: Option<Address>,
        expires_after_ms: u64,
    ) -> Result<Self> {
        Self::prepare_batch_modify(chain, vec![modify], nonce, vault_address, expires_after_ms)
    }

    /// Replace up to [`MAX_BATCH_MODIFY_CHILDREN`] resting limit orders in
    /// one signed `batchModify`, in the given order. Each child keeps its
    /// client order id; no child may repeat another's target or cloid.
    /// `always_place` is never set, so the venue never places a replacement
    /// whose cancel failed.
    pub fn prepare_batch_modify(
        chain: Chain,
        modifies: Vec<PreparedPerpModify>,
        nonce: u64,
        vault_address: Option<Address>,
        expires_after_ms: u64,
    ) -> Result<Self> {
        let modifies = modifies
            .into_iter()
            .map(modify_wire)
            .collect::<Result<Vec<_>>>()?;
        Self::prepare_action(
            chain,
            Action::BatchModify(BatchModify { modifies }),
            nonce,
            vault_address,
            expires_after_ms,
        )
    }

    /// Start a TWAP. Hyperliquid documents no builder field on `twapOrder`.
    pub fn prepare_twap_order(
        chain: Chain,
        twap: PreparedPerpTwapOrder,
        nonce: u64,
        vault_address: Option<Address>,
        expires_after_ms: u64,
    ) -> Result<Self> {
        Self::prepare_twap_action(
            chain,
            TwapAction::Order {
                twap: TwapOrderWire {
                    asset: twap.asset,
                    is_buy: twap.is_buy,
                    size: twap.size,
                    reduce_only: twap.reduce_only,
                    minutes: twap.minutes,
                    randomize: twap.randomize,
                },
            },
            nonce,
            vault_address,
            expires_after_ms,
        )
    }

    pub fn prepare_twap_cancel(
        chain: Chain,
        cancel: PreparedPerpTwapCancel,
        nonce: u64,
        vault_address: Option<Address>,
        expires_after_ms: u64,
    ) -> Result<Self> {
        Self::prepare_twap_action(
            chain,
            TwapAction::Cancel(TwapCancelWire {
                asset: cancel.asset,
                twap_id: cancel.twap_id,
            }),
            nonce,
            vault_address,
            expires_after_ms,
        )
    }

    fn prepare_twap_action(
        chain: Chain,
        twap: TwapAction,
        nonce: u64,
        vault_address: Option<Address>,
        expires_after_ms: u64,
    ) -> Result<Self> {
        let action_digest = twap_digest(&twap, chain, nonce, vault_address, expires_after_ms)?;
        let prepared = Self {
            version: PREPARED_HYPERLIQUID_ACTION_VERSION,
            chain,
            action: Action::Noop,
            twap: Some(twap),
            nonce,
            vault_address,
            expires_after_ms,
            action_digest,
        };
        prepared.validate()?;
        Ok(prepared)
    }

    /// Leverage updates always target the agent's own managed account.
    pub fn prepare_leverage_update(
        chain: Chain,
        update: PreparedPerpLeverageUpdate,
        nonce: u64,
        expires_after_ms: u64,
    ) -> Result<Self> {
        if update.leverage == 0 {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid leverage must be positive".to_string(),
            ));
        }
        Self::prepare_action(
            chain,
            Action::UpdateLeverage(UpdateLeverage {
                asset: update.asset as usize,
                is_cross: update.is_cross,
                leverage: update.leverage,
            }),
            nonce,
            None,
            expires_after_ms,
        )
    }

    fn prepare_action(
        chain: Chain,
        action: Action,
        nonce: u64,
        vault_address: Option<Address>,
        expires_after_ms: u64,
    ) -> Result<Self> {
        let action_digest = action_digest(&action, chain, nonce, vault_address, expires_after_ms)?;
        let prepared = Self {
            version: PREPARED_HYPERLIQUID_ACTION_VERSION,
            chain,
            action,
            twap: None,
            nonce,
            vault_address,
            expires_after_ms,
            action_digest,
        };
        prepared.validate()?;
        Ok(prepared)
    }

    /// Decodes only the current canonical representation.
    ///
    /// Semantically equivalent JSON with different bytes is rejected because
    /// the engine authorization binds one exact prepared-request hash.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self> {
        let prepared: Self =
            serde_json::from_slice(bytes).map_err(|source| ClientError::Serialization {
                operation: "invalid prepared Hyperliquid action encoding",
                source,
            })?;
        prepared.validate()?;
        let canonical = prepared.canonical_bytes()?;
        if canonical != bytes {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action is not canonically encoded".to_string(),
            ));
        }
        Ok(prepared)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != PREPARED_HYPERLIQUID_ACTION_VERSION {
            return Err(ClientError::InvalidInput(format!(
                "unsupported prepared Hyperliquid action version {}, expected {}",
                self.version, PREPARED_HYPERLIQUID_ACTION_VERSION
            )));
        }
        self.validate_journaled()
    }

    /// Validate a previously accepted action on re-read. Version 1 is never
    /// admitted by `validate` or `from_canonical_bytes` for fresh submission.
    #[expect(clippy::too_many_lines, reason = "pre-existing; split pending")]
    pub fn validate_journaled(&self) -> Result<()> {
        if !matches!(
            self.version,
            LEGACY_PREPARED_HYPERLIQUID_ACTION_VERSION | PREPARED_HYPERLIQUID_ACTION_VERSION
        ) {
            return Err(ClientError::InvalidInput(format!(
                "unsupported prepared Hyperliquid action version {}, expected {}",
                self.version, PREPARED_HYPERLIQUID_ACTION_VERSION
            )));
        }
        if self.nonce == 0 {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid nonce must not be zero".to_string(),
            ));
        }
        if self.expires_after_ms == 0 {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid expires_after_ms must not be zero".to_string(),
            ));
        }
        if self.expires_after_ms <= self.nonce {
            return Err(ClientError::InvalidInput(format!(
                "prepared Hyperliquid expires_after_ms {} must be after nonce {}",
                self.expires_after_ms, self.nonce
            )));
        }
        if let Some(twap) = &self.twap {
            if !matches!(self.action, Action::Noop) {
                return Err(ClientError::InvalidInput(
                    "prepared Hyperliquid TWAP must carry the placeholder action".to_string(),
                ));
            }
            match twap {
                TwapAction::Order { twap } => {
                    if twap.size <= Decimal::ZERO
                        || !(TWAP_MIN_MINUTES..=TWAP_MAX_MINUTES).contains(&twap.minutes)
                        || crate::prepared_spot::is_spot_asset(twap.asset)
                    {
                        return Err(ClientError::InvalidInput(format!(
                            "prepared Hyperliquid TWAP needs a positive size, a perp asset, and {TWAP_MIN_MINUTES} to {TWAP_MAX_MINUTES} minutes"
                        )));
                    }
                }
                TwapAction::Cancel(cancel) => {
                    if cancel.twap_id == 0 {
                        return Err(ClientError::InvalidInput(
                            "prepared Hyperliquid TWAP id must not be zero".to_string(),
                        ));
                    }
                }
            }
        } else {
            match &self.action {
                Action::BatchModify(batch) => {
                    if batch.modifies.is_empty() || batch.modifies.len() > MAX_BATCH_MODIFY_CHILDREN
                    {
                        return Err(ClientError::InvalidInput(format!(
                            "prepared Hyperliquid modify must contain 1 to {MAX_BATCH_MODIFY_CHILDREN} orders"
                        )));
                    }
                    let mut cloids = std::collections::BTreeSet::new();
                    let mut oids = std::collections::BTreeSet::new();
                    for modify in &batch.modifies {
                        validate_modify_child(modify)?;
                        // One venue order per child: a repeated cloid or oid
                        // would make the batch's per-child outcome ambiguous.
                        if !cloids.insert(modify.order.cloid) {
                            return Err(ClientError::InvalidInput(
                                "prepared Hyperliquid modify repeats a cloid".to_string(),
                            ));
                        }
                        if let OidOrCloid::Left(oid) = modify.oid {
                            if !oids.insert(oid) {
                                return Err(ClientError::InvalidInput(
                                    "prepared Hyperliquid modify repeats an oid".to_string(),
                                ));
                            }
                        }
                    }
                }
                Action::Order(batch) => {
                    if batch.orders.is_empty() || batch.orders.len() > 100 {
                        return Err(ClientError::InvalidInput(
                            "prepared Hyperliquid action must contain 1 to 100 orders".to_string(),
                        ));
                    }
                    if !matches!(
                        batch.grouping,
                        OrderGrouping::Na | OrderGrouping::NormalTpsl | OrderGrouping::PositionTpsl
                    ) || batch.builder.as_ref().is_some_and(|builder| {
                        builder.fee != 0 || builder.builder_address == Address::ZERO
                    }) {
                        return Err(ClientError::InvalidInput(
                            "prepared Hyperliquid order has unsupported grouping or builder fee"
                                .to_string(),
                        ));
                    }
                    for order in &batch.orders {
                        if order.limit_px <= Decimal::ZERO || order.sz <= Decimal::ZERO {
                            return Err(ClientError::InvalidInput(
                                "prepared Hyperliquid price and size must be positive".to_string(),
                            ));
                        }
                        match order.order_type {
                            OrderTypePlacement::Limit {
                                tif:
                                    HyperliquidTimeInForce::Gtc
                                    | HyperliquidTimeInForce::Ioc
                                    | HyperliquidTimeInForce::Alo,
                            } => {}
                            OrderTypePlacement::Trigger { trigger_px, .. }
                                if trigger_px > Decimal::ZERO => {}
                            _ => {
                                return Err(ClientError::InvalidInput(
                                    "unsupported or invalid Hyperliquid order type".to_string(),
                                ));
                            }
                        }
                        if order.cloid == Cloid::ZERO {
                            return Err(ClientError::InvalidInput(
                                "prepared Hyperliquid cloid must not be zero".to_string(),
                            ));
                        }
                    }
                    // Reservations and venue outcomes are matched per cloid, so a
                    // repeated cloid makes per-order reconciliation ambiguous. The
                    // intent validator enforces the same rule.
                    let mut cloids = std::collections::BTreeSet::new();
                    if !batch.orders.iter().all(|order| cloids.insert(order.cloid)) {
                        return Err(ClientError::InvalidInput(
                            "prepared Hyperliquid order batch must not repeat a cloid".to_string(),
                        ));
                    }
                }
                Action::CancelByCloid(batch) => {
                    if batch.cancels.len() != 1 || batch.cancels[0].cloid == Cloid::ZERO {
                        return Err(ClientError::InvalidInput(
                            "prepared Hyperliquid cancel must contain exactly one non-zero cloid"
                                .to_string(),
                        ));
                    }
                }
                Action::Cancel(batch) => {
                    if batch.cancels.len() != 1 || batch.cancels[0].oid == 0 {
                        return Err(ClientError::InvalidInput(
                            "prepared Hyperliquid cancel must contain exactly one non-zero oid"
                                .to_string(),
                        ));
                    }
                }
                Action::UpdateLeverage(update) => {
                    if update.leverage == 0 || self.vault_address.is_some() {
                        return Err(ClientError::InvalidInput(
                        "prepared Hyperliquid leverage must be positive and target the agent's managed account"
                            .to_string(),
                    ));
                    }
                }
                Action::ScheduleCancel(schedule) => {
                    if schedule.time == Some(0) {
                        return Err(ClientError::InvalidInput(
                            "prepared Hyperliquid schedule time must not be zero".to_string(),
                        ));
                    }
                }
                _ => {
                    return Err(ClientError::InvalidInput(
                        "unsupported Hyperliquid account action".to_string(),
                    ));
                }
            }
        }
        let expected = if let Some(twap) = &self.twap {
            twap_digest(
                twap,
                self.chain,
                self.nonce,
                self.vault_address,
                self.expires_after_ms,
            )?
        } else if self.version == LEGACY_PREPARED_HYPERLIQUID_ACTION_VERSION {
            let connection_id = self
                .action
                .hash(self.nonce, self.vault_address, Some(self.expires_after_ms))
                .map_err(|source| ClientError::Hash {
                    source: Box::new(source),
                })?;
            agent_signing_hash(self.chain, connection_id)
        } else {
            action_digest(
                &self.action,
                self.chain,
                self.nonce,
                self.vault_address,
                self.expires_after_ms,
            )?
        };
        if self.action_digest != expected {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action digest does not match frozen signing inputs"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|source| ClientError::Serialization {
            operation: "failed to encode prepared Hyperliquid action",
            source,
        })
    }

    pub fn request_hash(&self) -> Result<FixedBytes<32>> {
        Ok(keccak256(self.canonical_bytes()?))
    }

    pub fn cloid(&self) -> Result<FixedBytes<16>> {
        self.validate()?;
        match &self.action {
            Action::Order(batch) => Ok(batch.orders[0].cloid),
            Action::CancelByCloid(batch) => Ok(batch.cancels[0].cloid),
            Action::BatchModify(batch) => Ok(batch.modifies[0].order.cloid),
            _ => Err(ClientError::InvalidInput(
                "prepared Hyperliquid action has no cloid".to_string(),
            )),
        }
    }

    /// Return the exact economic order encoded by the validated native action.
    /// The builder address an `order` action attributes, if any.
    pub fn order_builder(&self) -> Option<Address> {
        match &self.action {
            Action::Order(batch) => batch
                .builder
                .as_ref()
                .map(|builder| builder.builder_address),
            _ => None,
        }
    }

    pub fn limit_order(&self) -> Result<PreparedPerpLimitOrder> {
        self.validate()?;
        let Action::Order(batch) = &self.action else {
            return Err(ClientError::InvalidInput(
                "only Hyperliquid perp limit orders are supported".to_string(),
            ));
        };
        if batch.orders.len() != 1 || !matches!(batch.grouping, OrderGrouping::Na) {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action is not one standalone limit order".to_string(),
            ));
        }
        let order = &batch.orders[0];
        let asset = u32::try_from(order.asset).map_err(|_| {
            ClientError::InvalidInput(
                "prepared Hyperliquid asset index does not fit u32".to_string(),
            )
        })?;
        if crate::prepared_spot::is_spot_asset(asset) {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid order is for a spot asset, not a perp".to_string(),
            ));
        }
        let tif = match order.order_type {
            OrderTypePlacement::Limit {
                tif: HyperliquidTimeInForce::Gtc,
            } => Tif::Gtc,
            OrderTypePlacement::Limit {
                tif: HyperliquidTimeInForce::Ioc,
            } => Tif::Ioc,
            OrderTypePlacement::Limit {
                tif: HyperliquidTimeInForce::Alo,
            } => Tif::Alo,
            _ => {
                return Err(ClientError::InvalidInput(
                    "only limit orders are supported".to_string(),
                ));
            }
        };
        let mut cloid = [0_u8; 16];
        cloid.copy_from_slice(order.cloid.as_slice());
        Ok(PreparedPerpLimitOrder {
            asset,
            is_buy: order.is_buy,
            limit_px: order.limit_px,
            size: order.sz,
            reduce_only: order.reduce_only,
            tif,
            cloid: u128::from_be_bytes(cloid),
        })
    }

    /// Return every child and the grouping of a validated native `order`
    /// action, including standalone limits, triggers, and grouped brackets.
    pub fn order_batch(&self) -> Result<PreparedPerpOrderBatch> {
        self.validate()?;
        let Action::Order(batch) = &self.action else {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action is not an order batch".to_string(),
            ));
        };
        let orders = batch
            .orders
            .iter()
            .map(|order| {
                let asset = u32::try_from(order.asset).map_err(|_| {
                    ClientError::InvalidInput(
                        "prepared Hyperliquid asset index does not fit u32".to_string(),
                    )
                })?;
                let order_type = match order.order_type {
                    OrderTypePlacement::Limit { tif } => PreparedPerpOrderType::Limit(match tif {
                        HyperliquidTimeInForce::Gtc => Tif::Gtc,
                        HyperliquidTimeInForce::Ioc => Tif::Ioc,
                        HyperliquidTimeInForce::Alo => Tif::Alo,
                        _ => {
                            return Err(ClientError::InvalidInput(
                                "unsupported or invalid Hyperliquid order type".to_string(),
                            ))
                        }
                    }),
                    OrderTypePlacement::Trigger {
                        is_market,
                        trigger_px,
                        tpsl,
                    } => PreparedPerpOrderType::Trigger {
                        is_market,
                        trigger_px,
                        tpsl: match tpsl {
                            TpSl::Tp => PreparedPerpTriggerKind::TakeProfit,
                            TpSl::Sl => PreparedPerpTriggerKind::StopLoss,
                        },
                    },
                };
                let mut cloid = [0_u8; 16];
                cloid.copy_from_slice(order.cloid.as_slice());
                Ok(PreparedPerpOrder {
                    asset,
                    is_buy: order.is_buy,
                    limit_px: order.limit_px,
                    size: order.sz,
                    reduce_only: order.reduce_only,
                    order_type,
                    cloid: u128::from_be_bytes(cloid),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let grouping = match batch.grouping {
            OrderGrouping::Na => PreparedPerpOrderGrouping::None,
            OrderGrouping::NormalTpsl => PreparedPerpOrderGrouping::NormalTpsl,
            OrderGrouping::PositionTpsl => PreparedPerpOrderGrouping::PositionTpsl,
            _ => {
                return Err(ClientError::InvalidInput(
                    "prepared Hyperliquid order has unsupported grouping".to_string(),
                ))
            }
        };
        Ok(PreparedPerpOrderBatch { orders, grouping })
    }

    /// Return the exact cancel identity encoded by the validated native action.
    pub fn cancel_by_cloid(&self) -> Result<PreparedPerpCancelByCloid> {
        self.validate()?;
        let Action::CancelByCloid(batch) = &self.action else {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action is not cancel-by-cloid".to_string(),
            ));
        };
        let mut cloid = [0_u8; 16];
        cloid.copy_from_slice(batch.cancels[0].cloid.as_slice());
        Ok(PreparedPerpCancelByCloid {
            asset: batch.cancels[0].asset,
            cloid: u128::from_be_bytes(cloid),
        })
    }

    pub fn cancel_by_oid(&self) -> Result<PreparedPerpCancelByOid> {
        self.validate()?;
        let Action::Cancel(batch) = &self.action else {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action is not cancel-by-oid".to_string(),
            ));
        };
        let asset = u32::try_from(batch.cancels[0].asset).map_err(|_| {
            ClientError::InvalidInput(
                "prepared Hyperliquid asset index does not fit u32".to_string(),
            )
        })?;
        Ok(PreparedPerpCancelByOid {
            asset,
            oid: batch.cancels[0].oid,
        })
    }

    pub fn leverage_update(&self) -> Result<PreparedPerpLeverageUpdate> {
        self.validate()?;
        let Action::UpdateLeverage(update) = &self.action else {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action is not a leverage update".to_string(),
            ));
        };
        let asset = u32::try_from(update.asset).map_err(|_| {
            ClientError::InvalidInput(
                "prepared Hyperliquid asset index does not fit u32".to_string(),
            )
        })?;
        Ok(PreparedPerpLeverageUpdate {
            asset,
            is_cross: update.is_cross,
            leverage: update.leverage,
        })
    }

    /// Return the exact replacement terms of a validated single-child modify.
    /// A multi-child `batchModify` is refused: callers that bind one child
    /// must never see only the first child of a larger batch.
    pub fn modify(&self) -> Result<PreparedPerpModify> {
        let mut modifies = self.batch_modifies()?;
        if modifies.len() != 1 {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid modify must contain exactly one order".to_string(),
            ));
        }
        Ok(modifies.remove(0))
    }

    /// Return the exact replacement terms of every child of a validated
    /// `batchModify`, in signed order.
    pub fn batch_modifies(&self) -> Result<Vec<PreparedPerpModify>> {
        self.validate()?;
        let Action::BatchModify(batch) = &self.action else {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action is not a modify".to_string(),
            ));
        };
        batch.modifies.iter().map(prepared_modify).collect()
    }

    pub fn twap_order(&self) -> Result<PreparedPerpTwapOrder> {
        self.validate()?;
        let Some(TwapAction::Order { twap }) = &self.twap else {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action is not a TWAP order".to_string(),
            ));
        };
        Ok(PreparedPerpTwapOrder {
            asset: twap.asset,
            is_buy: twap.is_buy,
            size: twap.size,
            reduce_only: twap.reduce_only,
            minutes: twap.minutes,
            randomize: twap.randomize,
        })
    }

    pub fn twap_cancel(&self) -> Result<PreparedPerpTwapCancel> {
        self.validate()?;
        let Some(TwapAction::Cancel(cancel)) = &self.twap else {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action is not a TWAP cancel".to_string(),
            ));
        };
        Ok(PreparedPerpTwapCancel {
            asset: cancel.asset,
            twap_id: cancel.twap_id,
        })
    }

    pub fn schedule_cancel(&self) -> Result<Option<u64>> {
        self.validate()?;
        let Action::ScheduleCancel(schedule) = &self.action else {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid action is not schedule-cancel".to_string(),
            ));
        };
        Ok(schedule.time)
    }

    /// Convenience helper for non-production tests and direct callers.
    ///
    /// Production gateway signing is performed by the enclave over the same
    /// `action_digest`, then passed to `signed_request_bytes`.
    pub async fn sign<S>(&self, signer: &S) -> Result<Signature>
    where
        S: Signer + Send + Sync,
    {
        self.validate()?;
        signer
            .sign_hash(&self.action_digest)
            .await
            .map_err(|source| ClientError::Signing {
                operation: "sign prepared Hyperliquid action digest",
                source: Box::new(source),
            })
    }

    /// Attaches a signature without rebuilding any frozen signing input.
    pub fn signed_request_bytes(
        &self,
        signature: Signature,
        expected_api_wallet: Address,
    ) -> Result<Vec<u8>> {
        self.validate()?;
        let recovered = signature
            .recover_address_from_prehash(&self.action_digest)
            .map_err(|source| ClientError::Signing {
                operation: "recover prepared Hyperliquid action signer",
                source: Box::new(source),
            })?;
        if recovered != expected_api_wallet {
            return Err(ClientError::SignerMismatch {
                recovered,
                expected: expected_api_wallet,
            });
        }
        let request = ActionRequest {
            action: self.action.clone(),
            nonce: self.nonce,
            signature: signature.into(),
            vault_address: self.vault_address,
            expires_after: Some(self.expires_after_ms),
        };
        let mut body =
            serde_json::to_value(&request).map_err(|source| ClientError::Serialization {
                operation: "failed to encode signed Hyperliquid request",
                source,
            })?;
        // The SDK has no TWAP action; submit the exact twap wire that was hashed.
        if let Some(twap) = &self.twap {
            body["action"] =
                serde_json::to_value(twap).map_err(|source| ClientError::Serialization {
                    operation: "failed to encode signed Hyperliquid TWAP action",
                    source,
                })?;
        }
        // Send a clearing schedule-cancel as `{"type":"scheduleCancel"}`, the
        // same shape it was hashed and signed in (see `signing_wire`).
        if matches!(&self.action, Action::ScheduleCancel(schedule) if schedule.time.is_none()) {
            if let Some(action) = body.get_mut("action").and_then(|a| a.as_object_mut()) {
                action.remove("time");
            }
        }
        serde_json::to_vec(&body).map_err(|source| ClientError::Serialization {
            operation: "failed to encode signed Hyperliquid request",
            source,
        })
    }
}

pub(crate) fn action_digest(
    action: &Action,
    chain: Chain,
    nonce: u64,
    vault_address: Option<Address>,
    expires_after_ms: u64,
) -> Result<FixedBytes<32>> {
    let connection_id = action_connection_id(action, nonce, vault_address, Some(expires_after_ms))
        .map_err(|source| ClientError::Hash {
            source: Box::new(source),
        })?;
    Ok(agent_signing_hash(chain, connection_id))
}

pub(crate) fn twap_digest(
    twap: &TwapAction,
    chain: Chain,
    nonce: u64,
    vault_address: Option<Address>,
    expires_after_ms: u64,
) -> Result<FixedBytes<32>> {
    let connection_id = twap_connection_id(twap, nonce, vault_address, Some(expires_after_ms))
        .map_err(|source| ClientError::Hash {
            source: Box::new(source),
        })?;
    Ok(agent_signing_hash(chain, connection_id))
}

pub(crate) fn cloid_from_u128(cloid: u128) -> Cloid {
    Cloid::from(cloid.to_be_bytes())
}

/// Native wire form of one modify child.
fn modify_wire(modify: PreparedPerpModify) -> Result<Modify> {
    if crate::prepared_spot::is_spot_asset(modify.asset) {
        return Err(ClientError::InvalidInput(format!(
            "Hyperliquid asset {} is a spot asset; modify supports perps only",
            modify.asset
        )));
    }
    let asset = usize::try_from(modify.asset).map_err(|_| {
        ClientError::InvalidInput("Hyperliquid asset index does not fit usize".to_string())
    })?;
    let oid = match modify.target {
        PreparedModifyTarget::Oid(oid) => OidOrCloid::Left(oid),
        PreparedModifyTarget::Cloid(cloid) => OidOrCloid::Right(cloid_from_u128(cloid)),
    };
    Ok(Modify {
        oid,
        order: OrderRequest {
            asset,
            is_buy: modify.is_buy,
            limit_px: modify.limit_px,
            sz: modify.size,
            reduce_only: modify.reduce_only,
            order_type: OrderTypePlacement::Limit {
                tif: match modify.tif {
                    Tif::Gtc => HyperliquidTimeInForce::Gtc,
                    Tif::Ioc => HyperliquidTimeInForce::Ioc,
                    Tif::Alo => HyperliquidTimeInForce::Alo,
                },
            },
            cloid: cloid_from_u128(modify.cloid),
        },
    })
}

/// Structural rules every modify child obeys: positive terms, a resting
/// limit type, a non-zero cloid preserved by a cloid-addressed target, a
/// non-zero oid, and a perp asset.
fn validate_modify_child(modify: &Modify) -> Result<()> {
    let order = &modify.order;
    if order.limit_px <= Decimal::ZERO || order.sz <= Decimal::ZERO {
        return Err(ClientError::InvalidInput(
            "prepared Hyperliquid price and size must be positive".to_string(),
        ));
    }
    if !matches!(
        order.order_type,
        OrderTypePlacement::Limit {
            tif: HyperliquidTimeInForce::Gtc
                | HyperliquidTimeInForce::Ioc
                | HyperliquidTimeInForce::Alo
        }
    ) {
        return Err(ClientError::InvalidInput(
            "prepared Hyperliquid modify supports limit orders only".to_string(),
        ));
    }
    if order.cloid == Cloid::ZERO {
        return Err(ClientError::InvalidInput(
            "prepared Hyperliquid cloid must not be zero".to_string(),
        ));
    }
    match modify.oid {
        OidOrCloid::Left(0) => {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid oid must not be zero".to_string(),
            ));
        }
        OidOrCloid::Right(cloid) if cloid != order.cloid => {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid modify must preserve its cloid".to_string(),
            ));
        }
        _ => {}
    }
    if u32::try_from(order.asset)
        .map(crate::prepared_spot::is_spot_asset)
        .unwrap_or(true)
    {
        return Err(ClientError::InvalidInput(
            "prepared Hyperliquid modify supports perp assets only".to_string(),
        ));
    }
    Ok(())
}

/// Typed replacement terms of one validated modify child.
fn prepared_modify(modify: &Modify) -> Result<PreparedPerpModify> {
    let order = &modify.order;
    let asset = u32::try_from(order.asset).map_err(|_| {
        ClientError::InvalidInput("prepared Hyperliquid asset index does not fit u32".to_string())
    })?;
    let tif = match order.order_type {
        OrderTypePlacement::Limit {
            tif: HyperliquidTimeInForce::Gtc,
        } => Tif::Gtc,
        OrderTypePlacement::Limit {
            tif: HyperliquidTimeInForce::Ioc,
        } => Tif::Ioc,
        OrderTypePlacement::Limit {
            tif: HyperliquidTimeInForce::Alo,
        } => Tif::Alo,
        _ => {
            return Err(ClientError::InvalidInput(
                "prepared Hyperliquid modify supports limit orders only".to_string(),
            ))
        }
    };
    let mut cloid = [0_u8; 16];
    cloid.copy_from_slice(order.cloid.as_slice());
    let cloid = u128::from_be_bytes(cloid);
    Ok(PreparedPerpModify {
        target: match modify.oid {
            OidOrCloid::Left(oid) => PreparedModifyTarget::Oid(oid),
            OidOrCloid::Right(_) => PreparedModifyTarget::Cloid(cloid),
        },
        asset,
        is_buy: order.is_buy,
        limit_px: order.limit_px,
        size: order.sz,
        reduce_only: order.reduce_only,
        tif,
        cloid,
    })
}

#[cfg(test)]
#[path = "prepared_batch_modify_test.rs"]
mod batch_modify_tests;
