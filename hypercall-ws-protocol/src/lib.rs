//! Stable WebSocket wire DTOs for Hypercall clients.
//!
//! This crate intentionally contains protocol-shaped data only.

pub mod client;
pub mod qp;
pub mod recovery;

pub use client::{
    ClientControlMessage, GatewayErrorCode, GatewayStatus, GatewayStatusMessage,
    UnsupportedWriteKind,
};
pub use qp::{
    BorrowedIndicativeQuote, BorrowedIndicativeQuoteUpdate, BorrowedScopedIndicativeQuoteUpdate,
    GatewayResumeQuoteProvider, IndicativeQuote, QpClientMessage, QpInboundMessage,
    QpOutboundMessage, QpResponseLeg, QpRfqLeg, QpServerMessage, ScopeId, ScopeIdParseError,
    CAP_SCOPED_INDICATIVE,
};
pub use recovery::{
    WsCloseError, WsDeliveryClass, WsPressureCause, WsRecoveryAction, WsSlowConsumerCloseReason,
};
