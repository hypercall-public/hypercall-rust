use super::{WsDeliveryClass, WsPressureCause, WsRecoveryAction, WsSlowConsumerCloseReason};

#[test]
fn slow_consumer_close_reason_preserves_wire_shape() {
    let reason =
        WsSlowConsumerCloseReason::new(WsDeliveryClass::OrderedPublic, WsPressureCause::MessageAge);

    let json = serde_json::to_string(&reason).unwrap();
    assert_eq!(
        json,
        r#"{"error":"slow_consumer","class":"ordered_public","cause":"message_age","recovery":"snapshot_resubscribe"}"#
    );
    assert_eq!(
        serde_json::from_str::<WsSlowConsumerCloseReason>(&json).unwrap(),
        reason
    );
}

#[test]
fn every_delivery_class_has_an_explicit_recovery_action() {
    let recoveries = WsDeliveryClass::ALL.map(WsDeliveryClass::recovery_action);

    assert_eq!(recoveries[0], WsRecoveryAction::Resubscribe);
    assert_eq!(recoveries[1], WsRecoveryAction::SnapshotResubscribe);
    assert_eq!(recoveries[2], WsRecoveryAction::RestReconcile);
    assert_eq!(recoveries[3], WsRecoveryAction::PortfolioRefetch);
    assert_eq!(recoveries[4], WsRecoveryAction::RestReconcile);
}
