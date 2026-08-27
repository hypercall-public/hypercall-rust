use hypercall_sdk_types::wallet_address::test_wallet;
use hypercall_sdk_types::{
    BulkPlaceOrderRequest, OrderRoute, PlaceOrderRequest, Side, TimeInForce,
};
use sonic_rs::JsonValueTrait;

#[test]
fn test_bulk_order_serialization_preserves_strings() {
    // Create a PlaceOrderRequest with string price/size
    let wallet = test_wallet(1);

    let request = PlaceOrderRequest {
        wallet,
        price: "251.93".to_string(),
        size: "6.1878".to_string(),
        symbol: "ETH-20251226-2700-P".to_string(),
        side: Side::Sell,
        tif: TimeInForce::GTC,
        route: Some(OrderRoute::BestExecution),
        client_id: Some("test123".to_string()),
        nonce: 1,
        signature: "0xsignature".to_string(),
        reduce_only: false,
        mmp_enabled: false,
        builder_code_address: None,
    };

    let bulk_request = BulkPlaceOrderRequest {
        orders: vec![request],
    };

    // Serialize and check that price/size remain strings
    let json_value = sonic_rs::to_value(&bulk_request).unwrap();

    println!(
        "Serialized JSON: {}",
        sonic_rs::to_string_pretty(&json_value).unwrap()
    );

    let price_value = &json_value["orders"][0]["price"];
    let size_value = &json_value["orders"][0]["size"];

    // These should be strings, not numbers
    assert!(
        price_value.is_str(),
        "price should be serialized as string, got: {:?}",
        price_value
    );
    assert!(
        size_value.is_str(),
        "size should be serialized as string, got: {:?}",
        size_value
    );

    assert_eq!(price_value.as_str().unwrap(), "251.93");
    assert_eq!(size_value.as_str().unwrap(), "6.1878");
}
