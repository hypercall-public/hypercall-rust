use serde::{Deserialize, Serialize};

/// Delivery semantics for a WebSocket message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsDeliveryClass {
    ReplaceablePublic,
    OrderedPublic,
    PrivateDurable,
    ReplaceablePrivate,
    CommandResult,
}

impl WsDeliveryClass {
    pub const COUNT: usize = 5;
    pub const ALL: [Self; 5] = [
        Self::ReplaceablePublic,
        Self::OrderedPublic,
        Self::PrivateDurable,
        Self::ReplaceablePrivate,
        Self::CommandResult,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::ReplaceablePublic => "replaceable_public",
            Self::OrderedPublic => "ordered_public",
            Self::PrivateDurable => "private_durable",
            Self::ReplaceablePrivate => "replaceable_private",
            Self::CommandResult => "command_result",
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::ReplaceablePublic => 0,
            Self::OrderedPublic => 1,
            Self::PrivateDurable => 2,
            Self::ReplaceablePrivate => 3,
            Self::CommandResult => 4,
        }
    }

    pub const fn recovery_action(self) -> WsRecoveryAction {
        match self {
            Self::ReplaceablePublic => WsRecoveryAction::Resubscribe,
            Self::OrderedPublic => WsRecoveryAction::SnapshotResubscribe,
            Self::PrivateDurable | Self::CommandResult => WsRecoveryAction::RestReconcile,
            Self::ReplaceablePrivate => WsRecoveryAction::PortfolioRefetch,
        }
    }

    pub const fn recovery(self) -> &'static str {
        self.recovery_action().as_str()
    }
}

/// Safety boundary that caused the server to disconnect a slow consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsPressureCause {
    MessageLimit,
    ByteLimit,
    MessageAge,
    WriteTimeout,
}

impl WsPressureCause {
    pub const fn label(self) -> &'static str {
        match self {
            Self::MessageLimit => "message_limit",
            Self::ByteLimit => "byte_limit",
            Self::MessageAge => "message_age",
            Self::WriteTimeout => "write_timeout",
        }
    }
}

/// Recovery action required after a WebSocket delivery gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsRecoveryAction {
    Resubscribe,
    SnapshotResubscribe,
    RestReconcile,
    PortfolioRefetch,
}

impl WsRecoveryAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Resubscribe => "resubscribe",
            Self::SnapshotResubscribe => "snapshot_resubscribe",
            Self::RestReconcile => "rest_reconcile",
            Self::PortfolioRefetch => "portfolio_refetch",
        }
    }
}

/// WebSocket close error returned by the public `/ws` endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WsCloseError {
    SlowConsumer,
}

/// Structured JSON reason carried by a slow-consumer close frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WsSlowConsumerCloseReason {
    pub error: WsCloseError,
    #[serde(rename = "class")]
    pub delivery_class: WsDeliveryClass,
    pub cause: WsPressureCause,
    pub recovery: WsRecoveryAction,
}

impl WsSlowConsumerCloseReason {
    pub const fn new(delivery_class: WsDeliveryClass, cause: WsPressureCause) -> Self {
        Self {
            error: WsCloseError::SlowConsumer,
            delivery_class,
            cause,
            recovery: delivery_class.recovery_action(),
        }
    }
}

#[cfg(test)]
#[path = "recovery_test.rs"]
mod tests;
