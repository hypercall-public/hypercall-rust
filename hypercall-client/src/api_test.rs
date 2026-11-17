use super::*;

#[test]
fn test_client_new() {
    let client = HypercallClient::new("http://localhost:3000");
    assert_eq!(client.base_url, "http://localhost:3000");
}

#[test]
fn test_client_new_trims_trailing_slash() {
    let client = HypercallClient::new("http://localhost:3000/");
    assert_eq!(client.base_url, "http://localhost:3000");

    // trim_end_matches removes ALL trailing slashes
    let client2 = HypercallClient::new("http://localhost:3000///");
    assert_eq!(client2.base_url, "http://localhost:3000");
}

#[test]
fn test_client_new_from_string() {
    let client = HypercallClient::new(String::from("https://api.example.com"));
    assert_eq!(client.base_url, "https://api.example.com");
}

#[test]
fn test_client_debug() {
    let client = HypercallClient::new("http://localhost:3000");
    let debug = format!("{:?}", client);
    assert!(debug.contains("HypercallClient"));
    assert!(debug.contains("http://localhost:3000"));
}

#[test]
fn test_client_clone() {
    let client = HypercallClient::new("http://localhost:3000");
    let cloned = client.clone();
    assert_eq!(cloned.base_url, client.base_url);
}

#[test]
fn markets_response_deserializes_from_server_json() {
    let server_json = serde_json::json!({
        "success": true,
        "data": [{
            "underlying": "BTC",
            "expiry": 1735689600,
            "index_price": "95000",
            "atm_vol": "0.65",
            "instruments": [{
                "instrument_id": 1,
                "id": "BTC-20260101-100000-C",
                "underlying": "BTC",
                "strike": "100000",
                "expiry": 1735689600,
                "option_type": "call",
                "option_token_address": null,
                "mark_iv": "0.70",
                "volume_24h": "1500",
                "open_interest": "25000",
                "updated_at": "2026-01-01T00:00:00Z",
                "status": "ACTIVE",
                "trading_mode": "orderbook"
            }],
            "total_volume_24h": "1500",
            "total_open_interest": "25000"
        }]
    });

    let resp: MarketsResponse = serde_json::from_value(server_json).unwrap();
    assert!(resp.success);
    assert_eq!(resp.data.len(), 1);
    let inst = &resp.data[0].instruments[0];
    assert_eq!(inst.strike, Decimal::new(100000, 0));
    assert_eq!(inst.id, "BTC-20260101-100000-C");
}

#[test]
fn portfolio_response_deserializes_from_server_json() {
    let server_json = serde_json::json!({
        "success": true,
        "data": {
            "wallet_address": "0x0000000000000000000000000000000000000001",
            "positions": [{
                "wallet_address": "0x0000000000000000000000000000000000000001",
                "symbol": "BTC-20260101-100000-C",
                "amount": "2",
                "entry_price": "3000",
                "margin_posted": "100",
                "realized_pnl": "0",
                "unrealized_pnl": "50",
                "updated_at": "2026-01-01T00:00:00Z",
                "notional_value": "6000",
                "maintenance_margin": "0",
                "liquidation_price": "0",
                "margin_ratio": "0"
            }],
            "total_margin_used": "100",
            "available_balance": "900",
            "margin_mode": "standard"
        }
    });

    let resp: ApiResponse<PortfolioResponse> = serde_json::from_value(server_json).unwrap();
    assert!(resp.success);
    let portfolio = resp.data.unwrap();
    assert_eq!(portfolio.positions.len(), 1);
    assert_eq!(portfolio.total_margin_used, Decimal::new(100, 0));
}
