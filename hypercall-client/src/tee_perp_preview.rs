//! Preview forms of signed TEE perp order wires.

use hypercall_sdk_types::{
    PerpPreviewAction, PerpPreviewLimit, PerpPreviewOrder, PerpPreviewOrderType, PerpPreviewTrigger,
};

use super::*;

fn from_perp_units(units: u64) -> Decimal {
    Decimal::from_i128_with_scale(i128::from(units), 8).normalize()
}

/// The preview form of a signed order wire (1e8 integers back to decimals),
/// so a preview describes exactly what will be signed.
pub fn perp_preview_order(order: &TeePerpOrderWire) -> PerpPreviewOrder {
    PerpPreviewOrder {
        asset: order.asset,
        is_buy: order.is_buy,
        limit_px: from_perp_units(order.limit_px),
        sz: from_perp_units(order.sz),
        reduce_only: order.reduce_only,
        order_type: match order.order_type {
            TeePerpOrderType::Limit(limit) => PerpPreviewOrderType::Limit(PerpPreviewLimit {
                tif: limit.tif.into(),
            }),
            TeePerpOrderType::Trigger(trigger) => {
                PerpPreviewOrderType::Trigger(PerpPreviewTrigger {
                    trigger_px: from_perp_units(trigger.trigger_px),
                    is_market: trigger.is_market,
                    tpsl: trigger.tpsl,
                })
            }
        },
    }
}

/// The preview action of a signed order batch.
pub fn perp_preview_batch_action(batch: &TeePerpOrderBatchAction) -> PerpPreviewAction {
    PerpPreviewAction::OrderBatch {
        grouping: batch.grouping,
        orders: batch.orders.iter().map(perp_preview_order).collect(),
        leverage: None,
    }
}
