use super::*;
use crate::{HypercallWallet, NonceProvider};
use rust_decimal_macros::dec;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const PRIVATE_KEY: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const ACCOUNT: &str = "0x0000000000000000000000000000000000000002";

struct FixedNonce(u64);

impl NonceProvider for FixedNonce {
    fn next_nonce(&self) -> u64 {
        self.0
    }

    fn current_nonce(&self) -> u64 {
        self.0
    }
}

async fn serve_responses(
    response_bodies: Vec<String>,
) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::with_capacity(response_bodies.len());
        for response_body in response_bodies {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let header_end = loop {
                let mut buffer = [0_u8; 1024];
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0, "client disconnected before sending headers");
                request.extend_from_slice(&buffer[..read]);
                if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let headers = std::str::from_utf8(&request[..header_end]).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .map(str::parse::<usize>)
                })
                .transpose()
                .unwrap()
                .unwrap_or(0);
            while request.len() - header_end < content_length {
                let mut buffer = [0_u8; 1024];
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0, "client disconnected before sending its body");
                request.extend_from_slice(&buffer[..read]);
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            requests.push(String::from_utf8(request).unwrap());
        }
        requests
    });
    (format!("http://{address}"), task)
}

async fn serve_once(response_body: &str) -> (String, tokio::task::JoinHandle<String>) {
    let (base_url, requests) = serve_responses(vec![response_body.to_string()]).await;
    let request = tokio::spawn(async move { requests.await.unwrap().pop().unwrap() });
    (base_url, request)
}

fn canonical_order(order_id: i64, reduce_only: bool) -> serde_json::Value {
    serde_json::json!({
        "order_id": order_id,
        "wallet_address": ACCOUNT,
        "symbol": "BTC-PERP",
        "side": "Buy",
        "price": "100000",
        "size": "0.25",
        "tif": "gtc",
        "status": "open",
        "created_at": 1785715200000_i64,
        "updated_at": null,
        "filled_size": "0",
        "client_id": "0x1234",
        "reduce_only": reduce_only,
        "mmp_enabled": false,
        "instrument_type": "perp"
    })
}

fn orders_page(data: Vec<serde_json::Value>, limit: usize, offset: usize) -> String {
    serde_json::json!({
        "success": true,
        "pagination": {"limit": limit, "offset": offset, "count": data.len()},
        "data": data
    })
    .to_string()
}

#[test]
fn perp_units_require_exact_positive_e8_values() {
    assert_eq!(
        to_perp_units(dec!(1.23456789), "size").unwrap(),
        123_456_789
    );
    assert!(to_perp_units(dec!(0), "size").is_err());
    assert!(to_perp_units(dec!(0.000000001), "size").is_err());
    assert!(to_perp_units(Decimal::MAX, "size").is_err());

    let maximum = Decimal::from_i128_with_scale(i128::from(u64::MAX), 8);
    assert_eq!(to_perp_units(maximum, "size").unwrap(), u64::MAX);
    let overflow = Decimal::from_i128_with_scale(i128::from(u64::MAX) + 1, 8);
    assert!(to_perp_units(overflow, "size").is_err());
}

#[tokio::test]
async fn typed_all_orders_uses_returned_pagination_without_requesting_a_limit() {
    let responses = vec![
        orders_page(vec![canonical_order(1, false)], 1, 5),
        orders_page(Vec::new(), 1, 6),
    ];
    let (base_url, requests) = serve_responses(responses).await;
    let client = HypercallClient::new(&base_url);

    let orders = client
        .get_all_orders_typed(ACCOUNT.parse::<AccountAddress>().unwrap(), None)
        .await
        .unwrap();

    assert_eq!(orders.len(), 1);
    let requests = requests.await.unwrap();
    assert!(requests[0].starts_with("GET /orders?wallet="));
    assert!(requests[0].contains("&offset=0 HTTP/1.1\r\n"));
    assert!(requests[1].contains("&offset=6 HTTP/1.1\r\n"));
    assert!(requests.iter().all(|request| !request.contains("limit=")));
}

#[tokio::test]
async fn raw_all_orders_uses_returned_pagination_without_requesting_a_limit() {
    let responses = vec![
        orders_page(vec![canonical_order(1, false)], 1, 0),
        orders_page(Vec::new(), 1, 1),
    ];
    let (base_url, requests) = serve_responses(responses).await;
    let client = HypercallClient::new(&base_url);

    let orders = client
        .get_all_orders(ACCOUNT.parse::<AccountAddress>().unwrap(), None)
        .await
        .unwrap();

    assert_eq!(orders.len(), 1);
    let requests = requests.await.unwrap();
    assert!(requests[0].starts_with("GET /orders?wallet="));
    assert!(requests[0].contains("&offset=0 HTTP/1.1\r\n"));
    assert!(requests[1].contains("&offset=1 HTTP/1.1\r\n"));
    assert!(requests.iter().all(|request| !request.contains("limit=")));
}

#[tokio::test]
async fn all_orders_rejects_a_zero_returned_page_limit() {
    let (base_url, _) = serve_once(&orders_page(Vec::new(), 0, 0)).await;
    let client = HypercallClient::new(&base_url);

    let error = client
        .get_all_orders_typed(ACCOUNT.parse::<AccountAddress>().unwrap(), None)
        .await
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("pagination.limit must be greater than zero"));
}

#[tokio::test]
async fn place_request_uses_explicit_nonce_client_id_and_signed_action_path() {
    let response = format!(
        r#"{{"stage":"enqueued","directiveId":"directive-1","actionKey":"hl_limit_order","account":"{ACCOUNT}","nonce":777,"recoveredSigner":null,"txHash":null,"rejection":null,"fills":null}}"#
    );
    let (base_url, request_task) = serve_once(&response).await;
    let client = HypercallClient::new(&base_url);
    let signer = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();

    let result = client
        .place_perp_limit_order(
            &signer,
            PerpLimitOrderParams {
                account: ACCOUNT.parse().unwrap(),
                asset: 3,
                side: Side::Buy,
                price: dec!(12.3456789),
                size: dec!(0.25),
                tif: PerpTimeInForce::Ioc,
                reduce_only: true,
                client_order_id: Some(9_007_199_254_740_992),
                nonce: Some(777),
            },
        )
        .await
        .unwrap();

    assert_eq!(result.stage, hypercall_sdk_types::DirectiveStage::Enqueued);
    let request = request_task.await.unwrap();
    assert!(request.starts_with("POST /v1/actions/hl_limit_order HTTP/1.1\r\n"));
    let body = request.split("\r\n\r\n").nth(1).unwrap();
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["nonce"], 777);
    assert_eq!(body["action"]["asset"], 3);
    assert_eq!(body["action"]["limitPx"], 1_234_567_890_u64);
    assert_eq!(body["action"]["sz"], 25_000_000_u64);
    assert_eq!(body["action"]["encodedTif"], 3);
    assert_eq!(body["action"]["cloid"], "9007199254740992");
    assert!(body["signature"].as_str().unwrap().starts_with("0x"));
}

#[tokio::test]
async fn set_account_abstraction_uses_manager_signed_unified_action_path() {
    let response = format!(
        r#"{{"stage":"submitted","directiveId":"abstraction-1","actionKey":"hl_set_abstraction","account":"{ACCOUNT}","nonce":778,"recoveredSigner":null,"txHash":"0x1","rejection":null,"fills":null}}"#
    );
    let (base_url, request_task) = serve_once(&response).await;
    let client = HypercallClient::new(&base_url);
    let manager = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();

    let result = client
        .set_account_abstraction(
            &manager,
            SetAccountAbstractionParams {
                account: ACCOUNT.parse().unwrap(),
                abstraction: HypercoreAccountAbstraction::UnifiedAccount,
                nonce: Some(778),
            },
        )
        .await
        .unwrap();

    assert_eq!(result.stage, hypercall_sdk_types::DirectiveStage::Submitted);
    let request = request_task.await.unwrap();
    assert!(request.starts_with("POST /v1/actions/hl_set_abstraction HTTP/1.1\r\n"));
    let body: serde_json::Value =
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["account"], ACCOUNT);
    assert_eq!(body["nonce"], 778);
    assert_eq!(body["action"]["user"], ACCOUNT);
    assert_eq!(body["action"]["abstraction"], 2);
    assert!(body["signature"].as_str().unwrap().starts_with("0x"));
}

#[tokio::test]
async fn update_api_wallet_uses_manager_signed_hypercall_action_path() {
    let api_wallet = "0x0000000000000000000000000000000000000003";
    let response = format!(
        r#"{{"stage":"submitted","directiveId":"api-wallet-1","actionKey":"hc_update_api_wallet","account":"{ACCOUNT}","nonce":779,"recoveredSigner":null,"txHash":"0x2","rejection":null,"fills":null}}"#
    );
    let (base_url, request_task) = serve_once(&response).await;
    let client = HypercallClient::new(&base_url);
    let manager = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();
    let name = alloy::primitives::keccak256("primary-api-wallet");

    let result = client
        .update_api_wallet(
            &manager,
            UpdateApiWalletParams {
                account: ACCOUNT.parse().unwrap(),
                name,
                api_wallet: api_wallet.parse().unwrap(),
                nonce: Some(779),
            },
        )
        .await
        .unwrap();

    assert_eq!(result.stage, hypercall_sdk_types::DirectiveStage::Submitted);
    let request = request_task.await.unwrap();
    assert!(request.starts_with("POST /v1/actions/hc_update_api_wallet HTTP/1.1\r\n"));
    let body: serde_json::Value =
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["account"], ACCOUNT);
    assert_eq!(body["nonce"], 779);
    assert_eq!(body["action"]["name"], format!("{name:#x}"));
    assert_eq!(body["action"]["addr"], api_wallet);
    assert!(body["signature"].as_str().unwrap().starts_with("0x"));
}

#[tokio::test]
async fn place_request_uses_signer_nonce_for_default_client_id() {
    let response = format!(
        r#"{{"stage":"rejected","directiveId":"directive-2","actionKey":"hl_limit_order","account":"{ACCOUNT}","nonce":888,"recoveredSigner":null,"txHash":null,"rejection":{{"code":"risk_rejected","message":"insufficient margin"}},"fills":null}}"#
    );
    let (base_url, request_task) = serve_once(&response).await;
    let client = HypercallClient::new(&base_url);
    let nonce_provider: Arc<dyn NonceProvider> = Arc::new(FixedNonce(888));
    let signer = HypercallWallet::from_private_key(PRIVATE_KEY, 999)
        .unwrap()
        .with_nonce_provider(nonce_provider);

    let result = client
        .place_perp_limit_order(
            &signer,
            PerpLimitOrderParams {
                account: ACCOUNT.parse().unwrap(),
                asset: 0,
                side: Side::Sell,
                price: dec!(100),
                size: dec!(1),
                tif: PerpTimeInForce::Gtc,
                reduce_only: false,
                client_order_id: None,
                nonce: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(result.stage, hypercall_sdk_types::DirectiveStage::Rejected);
    assert_eq!(result.rejection.unwrap().code, "risk_rejected");
    let request = request_task.await.unwrap();
    let body: serde_json::Value =
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["action"]["cloid"], body["nonce"]);
}

#[tokio::test]
async fn directive_status_uses_typed_status_path() {
    let directive_id = "00000000-0000-0000-0000-000000000003";
    let response = format!(
        r#"{{"directive_id":"{directive_id}","action_key":"hl_cancel_by_oid","domain_status":"accepted","delivery_status":"included","tx_hash":"0xabc","created_at":"2026-08-03T00:00:00Z"}}"#
    );
    let (base_url, request_task) = serve_once(&response).await;
    let client = HypercallClient::new(&base_url);

    let result = client.get_directive_status(directive_id).await.unwrap();

    assert_eq!(
        result.delivery_status,
        hypercall_sdk_types::DirectiveDeliveryStatus::Included
    );
    let request = request_task.await.unwrap();
    assert!(request.starts_with(&format!("GET /v1/directives/{directive_id} HTTP/1.1\r\n")));
}

#[tokio::test]
async fn cancellation_requests_use_native_oid_and_cloid_paths() {
    let signer = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();
    let account = ACCOUNT.parse().unwrap();

    let oid_response = format!(
        r#"{{"stage":"submitted","directiveId":"cancel-oid","actionKey":"hl_cancel_by_oid","account":"{ACCOUNT}","nonce":901,"recoveredSigner":null,"txHash":"0x1","rejection":null,"fills":null}}"#
    );
    let (base_url, request_task) = serve_once(&oid_response).await;
    HypercallClient::new(&base_url)
        .cancel_perp_order_by_oid(
            &signer,
            PerpCancelByOidParams {
                account,
                asset: 4,
                order_id: 9_007_199_254_740_992,
                nonce: Some(901),
            },
        )
        .await
        .unwrap();
    let request = request_task.await.unwrap();
    assert!(request.starts_with("POST /v1/actions/hl_cancel_by_oid HTTP/1.1\r\n"));
    let body: serde_json::Value =
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["action"]["oid"], "9007199254740992");

    let cloid_response = format!(
        r#"{{"stage":"enqueued","directiveId":"cancel-cloid","actionKey":"hl_cancel_by_cloid","account":"{ACCOUNT}","nonce":902,"recoveredSigner":null,"txHash":null,"rejection":null,"fills":null}}"#
    );
    let (base_url, request_task) = serve_once(&cloid_response).await;
    HypercallClient::new(&base_url)
        .cancel_perp_order_by_cloid(
            &signer,
            PerpCancelByCloidParams {
                account,
                asset: 4,
                client_order_id: u128::MAX,
                nonce: Some(902),
            },
        )
        .await
        .unwrap();
    let request = request_task.await.unwrap();
    assert!(request.starts_with("POST /v1/actions/hl_cancel_by_cloid HTTP/1.1\r\n"));
    let body: serde_json::Value =
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["action"]["cloid"], u128::MAX.to_string());
}

#[tokio::test]
async fn canonical_reads_decode_pm_positions_and_typed_order_fill_pages() {
    let portfolio = serde_json::json!({
        "success": true,
        "data": {
            "wallet_address": ACCOUNT,
            "positions": [
                {
                    "wallet_address": ACCOUNT,
                    "symbol": "BTC-20261231-100000-C",
                    "amount": "-1",
                    "entry_price": "2000",
                    "margin_posted": "0",
                    "realized_pnl": "0",
                    "unrealized_pnl": "10",
                    "updated_at": "2026-08-03T00:00:00Z",
                    "instrument_type": "option",
                    "notional_value": "1990",
                    "maintenance_margin": "100",
                    "liquidation_price": "0",
                    "margin_ratio": "0.1"
                },
                {
                    "wallet_address": ACCOUNT,
                    "symbol": "BTC-PERP",
                    "amount": "0.25",
                    "entry_price": "100000",
                    "margin_posted": "0",
                    "realized_pnl": "0",
                    "unrealized_pnl": "25",
                    "updated_at": "2026-08-03T00:00:00Z",
                    "instrument_type": "perp",
                    "notional_value": "25025",
                    "maintenance_margin": "500",
                    "liquidation_price": "80000",
                    "margin_ratio": "0.2"
                }
            ],
            "total_margin_used": "750",
            "available_balance": "9250",
            "withdrawable_usdc": "500",
            "portfolio_snapshot_timestamp_ms": 1785715200000_u64,
            "span_margin": {
                "equity": "10000",
                "initial_margin_required": "750",
                "maintenance_margin_required": "600",
                "open_orders_initial_margin": "50",
                "option_margin_required": "250",
                "scanning_risk": "200",
                "option_floor": "100",
                "gamma_overlay": "25",
                "hypercore_margin_required": "500"
            },
            "margin_mode": "portfolio",
            "margin_summary": {
                "mode": "portfolio",
                "equity": "10000",
                "position_im": "700",
                "open_orders_im": "50",
                "initial_margin": "9250",
                "maintenance_margin": "9400"
            }
        },
        "error": null
    })
    .to_string();
    let orders = orders_page(vec![canonical_order(42, true)], 10, 5);
    let fills = serde_json::json!({
        "success": true,
        "data": [{
            "fill_id": 7,
            "trade_id": 8,
            "wallet_address": ACCOUNT,
            "symbol": "BTC-PERP",
            "price": "100000",
            "size": "0.25",
            "fee": "1",
            "side": "Buy",
            "is_taker": true,
            "timestamp": 1785715200000_i64,
            "created_at": "2026-08-03T00:00:00Z",
            "builder_code_address": null,
            "builder_code_fee": null,
            "realized_pnl": "2",
            "explorer_url": null,
            "instrument_type": "perp"
        }],
        "pagination": {"limit": 20, "offset": 0, "count": 1}
    })
    .to_string();
    let (base_url, requests) = serve_responses(vec![portfolio, orders, fills]).await;
    let client = HypercallClient::new(&base_url);
    let account: AccountAddress = ACCOUNT.parse().unwrap();

    let portfolio = client.get_portfolio_snapshot(account).await.unwrap();
    assert_eq!(
        portfolio.margin_mode_kind().unwrap(),
        hypercall_sdk_types::MarginMode::Portfolio
    );
    assert_eq!(portfolio.positions.len(), 2);
    assert_eq!(
        portfolio.positions[1].instrument_kind().unwrap(),
        hypercall_sdk_types::InstrumentKind::Perp
    );
    assert_eq!(portfolio.withdrawable_usdc, Some(dec!(500)));
    assert!(portfolio.span_margin.is_some());
    assert!(portfolio.margin_summary.is_some());

    let orders = client
        .get_orders_typed(account, Some("open"), Some(10), Some(5))
        .await
        .unwrap();
    assert_eq!(orders.pagination.offset, 5);
    assert_eq!(orders.data[0].client_id.as_deref(), Some("0x1234"));
    assert_eq!(orders.data[0].reduce_only, Some(true));
    assert_eq!(orders.data[0].tif.as_deref(), Some("gtc"));

    let fills = client
        .get_fills_typed(account, Some(20), Some(0))
        .await
        .unwrap();
    assert_eq!(fills.pagination.count, 1);
    assert_eq!(
        fills.data[0].instrument_kind().unwrap(),
        hypercall_sdk_types::InstrumentKind::Perp
    );

    let requests = requests.await.unwrap();
    assert!(requests[0].starts_with("GET /portfolio?wallet="));
    assert!(requests[1].starts_with("GET /orders?wallet="));
    assert!(requests[1].contains("&status=open&limit=10&offset=5 HTTP/1.1"));
    assert!(requests[2].starts_with("GET /fills?wallet="));
}
