use super::*;

#[test]
fn the_order_action_matches_the_server_wire() {
    let action = TeeSpotOrderAction {
        asset: 10_107,
        is_buy: true,
        limit_px: 3_000_000_000,
        sz: 200_000_000,
        tif: PerpTimeInForce::Gtc,
        cloid: 42,
    };
    let json = serde_json::to_value(action).unwrap();
    assert_eq!(json["asset"], 10_107);
    assert_eq!(json["isBuy"], true);
    assert_eq!(json["encodedTif"], 2);
    assert_eq!(
        serde_json::from_value::<TeeSpotOrderAction>(json).unwrap(),
        action
    );
}

#[test]
fn the_response_reads_the_server_shape() {
    let response: TeeSpotResponse = serde_json::from_value(serde_json::json!({
        "stage": "accepted",
        "errorCode": null,
        "requestId": "019a2c41-4630-8000-8000-000000000002",
        "account": "0xa1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
        "apiWallet": "0xa9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9",
        "cloid": "0x0000000000000000000000000000002a",
        "engineSeq": "12",
        "venueResponseHash": null,
        "message": null
    }))
    .unwrap();
    assert!(response.is_accepted());
    assert_eq!(response.engine_seq, Some(12));
}
