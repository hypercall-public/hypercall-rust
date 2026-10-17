use rust_decimal_macros::dec;

use super::{HypercallClient, OrderOptions, ReplaceOrderParams, Side, TimeInForce};
use crate::{
    test_support::{json_body, serve_once},
    HypercallWallet,
};

const TEST_PRIVATE_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

#[tokio::test]
async fn replace_order_sends_the_signed_request_contract() {
    let (base_url, request_task) = serve_once("{}").await;
    let client = HypercallClient::new(base_url);
    let signer = HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 999).unwrap();
    let builder = "0x0000000000000000000000000000000000000002"
        .parse()
        .unwrap();

    client
        .replace_order_with_params(
            &signer,
            ReplaceOrderParams {
                account: signer.address,
                order_id: 42,
                symbol: "BTC-20270101-100000-C",
                side: Side::Sell,
                price: dec!(12.3400),
                size: dec!(2.5),
                tif: TimeInForce::IOC,
                nonce: Some(777),
                options: OrderOptions {
                    client_id: Some("replacement-42".into()),
                    reduce_only: Some(true),
                    mmp_enabled: Some(true),
                    builder_code_address: Some(builder),
                    ..OrderOptions::default()
                },
            },
        )
        .await
        .expect_err("empty response is not an order message");

    let request = request_task.await.unwrap();
    assert!(request.starts_with("PUT /order HTTP/1.1\r\n"));
    let mut body = json_body(&request);
    assert!(body["signature"].as_str().unwrap().starts_with("0x"));
    body.as_object_mut().unwrap().remove("signature");
    assert_eq!(
        body,
        serde_json::json!({
            "wallet": signer.address.to_string(),
            "order_id": 42,
            "symbol": "BTC-20270101-100000-C",
            "side": "Sell",
            "price": "12.3400",
            "size": "2.5",
            "tif": "ioc",
            "client_id": "replacement-42",
            "nonce": 777,
            "reduce_only": true,
            "mmp_enabled": true,
            "builder_code_address": builder.to_string(),
        })
    );
}
