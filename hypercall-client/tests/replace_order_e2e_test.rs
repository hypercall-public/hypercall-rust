//! E2E test for the replace order endpoint.
//!
//! Requires a running local server on localhost:3000 with markets created.
//! Run with: cargo test -p hypercall-client --test replace_order_e2e_test -- --ignored --nocapture

use hypercall_client::{HypercallClient, HypercallWallet, OrderOptions, ReplaceOrderParams};
use hypercall_sdk_types::{OrderMessage, OrderStatus, Side, TimeInForce};
use rust_decimal_macros::dec;

fn base_url() -> String {
    std::env::var("TEST_BASE_URL").unwrap_or_else(|_| "http://localhost:3000".to_string())
}

async fn pick_symbol(client: &HypercallClient) -> String {
    let markets = client.get_markets().await.expect("get_markets");
    for m in &markets {
        if m.symbol.starts_with("BTC-") && m.symbol.ends_with("-C") && m.expiry > 20260501 {
            return m.symbol.clone();
        }
    }
    panic!("No suitable BTC call found in markets");
}

#[tokio::test]
#[ignore] // requires running server
async fn test_replace_order_e2e() {
    let client = HypercallClient::new(base_url());
    let Ok(private_key) = std::env::var("HYPERCALL_E2E_PRIVATE_KEY") else {
        eprintln!("skipping replace-order e2e: HYPERCALL_E2E_PRIVATE_KEY is not set");
        return;
    };
    let wallet = HypercallWallet::from_private_key(&private_key, 998).expect("valid test key");

    let symbol = pick_symbol(&client).await;
    println!("Using symbol: {}", symbol);

    // 1. Place an initial order
    let place_result: OrderMessage = client
        .place_order(
            &wallet,
            &symbol,
            Side::Buy,
            dec!(0.001),
            dec!(1),
            TimeInForce::GTC,
        )
        .await
        .expect("place_order");

    let order_id = place_result
        .order_id
        .expect("place_order should return order_id");
    println!(
        "Placed order_id={}, status={:?}",
        order_id, place_result.status
    );

    // 2. Replace it with new price/size
    let replace_result: OrderMessage = client
        .replace_order_with_params(
            &wallet,
            ReplaceOrderParams {
                account: wallet.address,
                order_id,
                symbol: &symbol,
                side: Side::Buy,
                price: dec!(0.002),
                size: dec!(2),
                tif: TimeInForce::GTC,
                nonce: None,
                options: OrderOptions::default(),
            },
        )
        .await
        .expect("replace_order");

    let new_order_id = replace_result
        .order_id
        .expect("replace should return new order_id");
    println!(
        "Replaced: old_id={} -> new_id={}, status={:?}",
        order_id, new_order_id, replace_result.status
    );

    assert_ne!(order_id, new_order_id, "new order should have different ID");
    assert!(
        replace_result.status == OrderStatus::OpenOrder
            || replace_result.status == OrderStatus::Acked,
        "replace should produce OpenOrder or Acked, got {:?}",
        replace_result.status
    );

    // 3. Replace non-existent order should fail
    let bad_replace = client
        .replace_order_with_params(
            &wallet,
            ReplaceOrderParams {
                account: wallet.address,
                order_id: 99999999,
                symbol: &symbol,
                side: Side::Buy,
                price: dec!(0.003),
                size: dec!(1),
                tif: TimeInForce::GTC,
                nonce: None,
                options: OrderOptions::default(),
            },
        )
        .await;

    match bad_replace {
        Ok(msg) => {
            println!("Bad replace response: {:?}", msg.status);
            assert_eq!(
                msg.status,
                OrderStatus::RejectOrder,
                "replacing non-existent order should be rejected"
            );
        }
        Err(e) => {
            println!("Bad replace error (expected): {}", e);
        }
    }

    // 4. Cancel the new order to clean up
    let _ = client.cancel_order(&wallet, new_order_id).await;

    println!("\n=== E2E replace order test PASSED ===");
}
