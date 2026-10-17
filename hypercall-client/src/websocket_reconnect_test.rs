use super::*;
use futures::{SinkExt, StreamExt};
use hypercall_ws_protocol::{WsDeliveryClass, WsPressureCause, WsSlowConsumerCloseReason};
use sonic_rs::{JsonContainerTrait, JsonValueTrait};
use tokio::net::TcpListener;
use tokio::time::{timeout, Instant};
use tokio_tungstenite::tungstenite::protocol::{frame::coding::CloseCode, CloseFrame};

fn reconnect_test_config() -> WsClientConfig {
    WsClientConfig {
        reconnect_initial_delay: Duration::from_millis(5),
        reconnect_max_delay: Duration::from_millis(20),
        reconnect_reset_after: Duration::from_millis(50),
        authentication_timeout: Duration::from_secs(1),
        subscription_timeout: Duration::from_secs(1),
    }
}

async fn wait_for_state(
    client: &WsClient,
    check: impl Fn(&WsConnectionState) -> bool,
) -> WsConnectionState {
    wait_for_state_within(client, Duration::from_secs(2), check).await
}

/// [`wait_for_state`] with an explicit deadline, for a state that is only
/// reachable after a deliberately long client delay.
async fn wait_for_state_within(
    client: &WsClient,
    within: Duration,
    check: impl Fn(&WsConnectionState) -> bool,
) -> WsConnectionState {
    let deadline = Instant::now() + within;
    loop {
        let state = client.connection_state();
        if check(&state) {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for WebSocket state, current state: {state:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn next_text(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> sonic_rs::Value {
    loop {
        match timeout(Duration::from_secs(1), socket.next())
            .await
            .expect("timed out waiting for client frame")
            .expect("client socket ended")
            .expect("client frame failed")
        {
            tungstenite::Message::Text(text) => {
                return sonic_rs::from_str(&text).expect("client text frame must be JSON");
            }
            tungstenite::Message::Ping(payload) => {
                socket
                    .send(tungstenite::Message::Pong(payload))
                    .await
                    .unwrap();
            }
            other => panic!("expected client text frame, received {other:?}"),
        }
    }
}

async fn acknowledge_subscription(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    expected_channel: &str,
) -> sonic_rs::Value {
    let subscription = next_text(socket).await;
    assert_eq!(
        subscription.get("type").and_then(|value| value.as_str()),
        Some("Subscribe")
    );
    assert_eq!(
        subscription.get("channel").and_then(|value| value.as_str()),
        Some(expected_channel)
    );
    socket
        .send(tungstenite::Message::Text(format!(
            r#"{{"type":"Subscribed","channel":"{}"}}"#,
            expected_channel
        )))
        .await
        .unwrap();
    subscription
}

#[test]
fn recovery_plan_is_conservative_for_each_channel_class() {
    let mut plan = WsRecoveryPlan::default();
    plan.include_channel("indicative_market_data");
    assert!(plan.is_empty());

    plan.include_channel("orderbook");
    assert!(plan.snapshot_resubscribe);
    assert!(!plan.rest_reconcile);

    plan.include_channel("portfolio");
    assert!(plan.portfolio_refetch);
    assert!(plan.rest_reconcile);

    plan.include_channel("fills");
    assert!(plan.rest_reconcile);

    #[cfg(feature = "rfq-v2")]
    {
        let mut rfq_plan = WsRecoveryPlan::default();
        rfq_plan.include_channel("rfq_v2");
        assert!(rfq_plan.rest_reconcile);
        assert!(!rfq_plan.application_reconcile);
    }

    plan.include_channel("future_channel");
    assert!(plan.application_reconcile);
}

#[cfg(feature = "rfq-v2")]
#[tokio::test]
async fn rfq_v2_grant_is_authenticated_before_subscription_and_restored_on_reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let account: hypercall_sdk_types::WalletAddress = "0x00000000000000000000000000000000000000a1"
        .parse()
        .unwrap();
    let grant = RfqV2ReadGrant {
        account,
        issued_at_ms: 1_900_000_000_000,
        expires_at_ms: 1_900_000_030_000,
        nonce: 17,
        signature: format!("0x{}1b", "11".repeat(64)).parse().unwrap(),
    };
    let expected_token = encode_rfq_v2_read_grant(&grant).unwrap();
    let (replayed_tx, replayed_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut replayed_tx = Some(replayed_tx);
        for connection in 0..2 {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let auth = next_text(&mut socket).await;
            assert_eq!(auth["type"], "AuthenticateRfqV2");
            assert_eq!(auth["token"], expected_token);
            socket
                .send(tungstenite::Message::Text(format!(
                    r#"{{"type":"RfqV2Authenticated","account":"{account}"}}"#
                )))
                .await
                .unwrap();
            acknowledge_subscription(&mut socket, "rfq_v2").await;
            if connection == 0 {
                socket.close(None).await.unwrap();
            } else {
                replayed_tx.take().unwrap().send(()).unwrap();
                while let Some(Ok(frame)) = socket.next().await {
                    if matches!(frame, tungstenite::Message::Close(_)) {
                        break;
                    }
                }
            }
        }
    });

    let client = WsClient::with_config(reconnect_test_config());
    client
        .connect_rfq_v2(&format!("http://{address}"), &grant)
        .await
        .unwrap();
    client.subscribe_rfq_v2().await.unwrap();
    timeout(Duration::from_secs(2), replayed_rx)
        .await
        .expect("signed RFQ grant and subscription were not restored")
        .unwrap();
    let state = wait_for_state(&client, |state| {
        matches!(state, WsConnectionState::RecoveryRequired(_))
    })
    .await;
    let WsConnectionState::RecoveryRequired(plan) = state else {
        unreachable!();
    };
    assert!(plan.rest_reconcile);
    assert!(!plan.application_reconcile);
    client.disconnect().await;
    server.await.unwrap();
}

#[cfg(feature = "rfq-v2")]
#[tokio::test]
async fn rejected_rfq_v2_grant_does_not_start_an_unauthenticated_session() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let account = "0x00000000000000000000000000000000000000a1"
        .parse()
        .unwrap();
    let grant = RfqV2ReadGrant {
        account,
        issued_at_ms: 1_900_000_000_000,
        expires_at_ms: 1_900_000_030_000,
        nonce: 17,
        signature: format!("0x{}1b", "11".repeat(64)).parse().unwrap(),
    };
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        assert_eq!(next_text(&mut socket).await["type"], "AuthenticateRfqV2");
        socket
            .send(tungstenite::Message::Text(
                r#"{"type":"Error","message":"Invalid or expired RFQ V2 read grant"}"#.into(),
            ))
            .await
            .unwrap();
    });

    let client = WsClient::with_config(reconnect_test_config());
    let error = client
        .connect_rfq_v2(&format!("http://{address}"), &grant)
        .await
        .expect_err("rejected grant must fail connection");
    assert!(error
        .to_string()
        .contains("Invalid or expired RFQ V2 read grant"));
    assert!(!client.shared.started.load(Ordering::Acquire));
    server.await.unwrap();
}

#[cfg(feature = "rfq-v2")]
#[tokio::test]
async fn rfq_v2_socket_stops_when_its_signed_read_grant_expires() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let account = "0x00000000000000000000000000000000000000a1"
        .parse()
        .unwrap();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let grant = RfqV2ReadGrant {
        account,
        issued_at_ms: now_ms,
        expires_at_ms: now_ms + 750,
        nonce: 18,
        signature: format!("0x{}1b", "11".repeat(64)).parse().unwrap(),
    };
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        assert_eq!(next_text(&mut socket).await["type"], "AuthenticateRfqV2");
        socket
            .send(tungstenite::Message::Text(format!(
                r#"{{"type":"RfqV2Authenticated","account":"{account}"}}"#
            )))
            .await
            .unwrap();
        assert!(matches!(
            timeout(Duration::from_secs(2), socket.next())
                .await
                .unwrap(),
            Some(Ok(tungstenite::Message::Close(_)))
        ));
    });

    let client = WsClient::with_config(reconnect_test_config());
    client
        .connect_rfq_v2(&format!("http://{address}"), &grant)
        .await
        .unwrap();
    wait_for_state(&client, |state| {
        matches!(state, WsConnectionState::Disconnected)
    })
    .await;
    assert!(matches!(
        client.last_disconnect_reason(),
        Some(WsDisconnectReason::Protocol(reason)) if reason.contains("read grant expired")
    ));
    server.await.unwrap();
}

#[cfg(feature = "rfq-v2")]
#[tokio::test]
async fn rfq_v2_grant_expiring_during_backoff_prevents_reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let account = "0x00000000000000000000000000000000000000a1"
        .parse()
        .unwrap();
    let now_ms = current_unix_ms().unwrap();
    let grant = RfqV2ReadGrant {
        account,
        issued_at_ms: now_ms,
        expires_at_ms: now_ms + 1_000,
        nonce: 20,
        signature: format!("0x{}1b", "11".repeat(64)).parse().unwrap(),
    };
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        assert_eq!(next_text(&mut socket).await["type"], "AuthenticateRfqV2");
        socket
            .send(tungstenite::Message::Text(format!(
                r#"{{"type":"RfqV2Authenticated","account":"{account}"}}"#
            )))
            .await
            .unwrap();
        socket.close(None).await.unwrap();
        assert!(
            timeout(Duration::from_millis(2_700), listener.accept())
                .await
                .is_err(),
            "expired grant opened another socket after reconnect backoff"
        );
    });

    let client = WsClient::with_config(WsClientConfig {
        reconnect_initial_delay: Duration::from_millis(2_000),
        reconnect_max_delay: Duration::from_millis(2_000),
        ..reconnect_test_config()
    });
    client
        .connect_rfq_v2(&format!("http://{address}"), &grant)
        .await
        .unwrap();
    // The client reaches `Disconnected` only after the whole 2s backoff,
    // because it checks the grant when the backoff ends. The default 2s wait
    // raced that backoff and failed under CI load. Paused tokio time cannot
    // replace the wait: grant expiry is read from the wall clock, which a
    // paused runtime does not advance. The wide deadline does not weaken
    // the test: the reconnect it forbids would happen when the backoff ends,
    // and the server's unchanged 2.7s accept window detects it. This wait
    // only bounds how long the correct terminal state may take to appear.
    wait_for_state_within(&client, Duration::from_secs(20), |state| {
        matches!(state, WsConnectionState::Disconnected)
    })
    .await;
    assert!(matches!(
        client.last_disconnect_reason(),
        Some(WsDisconnectReason::Protocol(reason)) if reason.contains("read grant expired")
    ));
    server.await.unwrap();
}

#[cfg(feature = "rfq-v2")]
#[tokio::test]
async fn rfq_v2_connected_client_recovers_a_lost_hint_by_polling_private_changes() {
    use hypercall_sdk_types::RfqV2ReadGrant;

    use crate::{rfq_v2::RfqV2ReadCredentials, HypercallClient};

    let rfq_id = uuid::Uuid::from_u128(0x2606);
    let account: hypercall_sdk_types::WalletAddress = "0x00000000000000000000000000000000000000a1"
        .parse()
        .unwrap();
    let grant = RfqV2ReadGrant {
        account,
        issued_at_ms: 1_900_000_000_000,
        expires_at_ms: 1_900_000_030_000,
        nonce: 19,
        signature: format!("0x{}1b", "11".repeat(64)).parse().unwrap(),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let socket_address = listener.local_addr().unwrap();
    let socket_server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        assert_eq!(next_text(&mut socket).await["type"], "AuthenticateRfqV2");
        socket
            .send(tungstenite::Message::Text(format!(
                r#"{{"type":"RfqV2Authenticated","account":"{account}"}}"#
            )))
            .await
            .unwrap();
        acknowledge_subscription(&mut socket, "rfq_v2").await;
        // No RfqV2Changed frame is sent; the connection stays open.
        while let Some(Ok(frame)) = socket.next().await {
            if matches!(frame, tungstenite::Message::Close(_)) {
                break;
            }
        }
    });

    let parent = serde_json::json!({
        "rfq_id": rfq_id,
        "status": "partially_filled",
        "terms_commitment": format!("0x{}", "4a".repeat(32)),
        "parent": {"view": "maker", "terms": {
            "id": rfq_id,
            "taker_account": null,
            "legs": [{"instrument": "BTC-20261225-100000-C", "taker_side": "buy", "quantity_micros_per_unit": "1000000"}],
            "target": "5000000",
            "fill_policy": {"mode": "full_size_once"},
            "execution_mode": "manual",
            "quote_policy": "firm_only",
            "taker_identity": "anonymous",
            "quote_deadline_ms": 1_900_000_020_000_u64,
            "expires_at_ms": 1_900_000_060_000_u64,
            "fee_limits": {"per_execution": "0", "cumulative": "0"}
        }},
        "target": "5000000",
        "remaining_target": "3000000",
        "quote_policy": "firm_only"
    });
    let (http_base, http_server) = crate::test_support::serve_sequence(vec![
        serde_json::json!({"changes": [], "next_after_sequence": 7, "has_more": false}).to_string(),
        serde_json::json!({
            "changes": [{"rfq_id": rfq_id, "sequence": 9}],
            "next_after_sequence": 9,
            "has_more": false
        })
        .to_string(),
        parent.to_string(),
    ])
    .await;

    let socket = WsClient::with_config(reconnect_test_config());
    socket
        .connect_rfq_v2(&format!("http://{socket_address}"), &grant)
        .await
        .unwrap();
    socket.subscribe_rfq_v2().await.unwrap();
    let http = HypercallClient::new(http_base);
    let token = encode_rfq_v2_read_grant(&grant).unwrap();
    let mut pages = http
        .watch_rfq_v2_changes(&grant, 7, Duration::from_millis(20))
        .unwrap();
    // The first poll is empty and does not occupy the bounded consumer queue.
    let changed = timeout(Duration::from_secs(1), pages.recv())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(changed.changes.len(), 1);
    let recovered = http
        .get_rfq_v2(
            changed.changes[0].rfq_id,
            RfqV2ReadCredentials {
                bearer_token: &token,
            },
        )
        .await
        .unwrap();
    drop(pages);
    assert_eq!(recovered.remaining_target, 3_000_000);
    assert_eq!(changed.next_after_sequence, 9);
    assert_eq!(socket.connection_state(), WsConnectionState::Ready);
    assert!(!socket.get_messages().await.iter().any(|message| {
        message.get("type").and_then(|value| value.as_str()) == Some("RfqV2Changed")
    }));
    socket.disconnect().await;
    socket_server.await.unwrap();
    let requests = http_server.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].starts_with("GET /private/rfqs/changes?"));
    assert!(requests[1].starts_with("GET /private/rfqs/changes?"));
    assert!(requests[2].starts_with(&format!("GET /private/rfqs/{rfq_id}?view=authorized")));
    assert!(requests
        .iter()
        .all(|request| request.contains(&format!("authorization: Bearer {token}"))));
    assert!(requests.iter().all(|request| !request.starts_with("POST ")));
}

async fn acknowledge_options_subscriptions(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) -> Vec<String> {
    let mut expiries = Vec::new();
    for _ in 0..2 {
        let subscription = acknowledge_subscription(socket, "options_chain").await;
        expiries.push(
            subscription
                .get("expiry")
                .and_then(|value| value.as_str())
                .expect("options subscription must retain expiry")
                .to_string(),
        );
    }
    expiries.sort();
    expiries
}

#[tokio::test]
async fn reconnect_preserves_multiple_filtered_subscriptions_on_one_channel() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (replayed_tx, replayed_rx) = oneshot::channel();
    let expected_expiries = vec!["2026-07-24".to_string(), "2026-07-31".to_string()];
    let server_expected_expiries = expected_expiries.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut first = tokio_tungstenite::accept_async(stream).await.unwrap();
        assert_eq!(
            acknowledge_options_subscriptions(&mut first).await,
            server_expected_expiries
        );

        let close_reason = serde_json::to_string(&WsSlowConsumerCloseReason::new(
            WsDeliveryClass::OrderedPublic,
            WsPressureCause::MessageAge,
        ))
        .unwrap();
        first
            .send(tungstenite::Message::Close(Some(CloseFrame {
                code: CloseCode::Policy,
                reason: close_reason.into(),
            })))
            .await
            .unwrap();

        let (stream, _) = listener.accept().await.unwrap();
        let mut second = tokio_tungstenite::accept_async(stream).await.unwrap();
        assert_eq!(
            acknowledge_options_subscriptions(&mut second).await,
            server_expected_expiries
        );
        let _ = replayed_tx.send(());
        while let Some(Ok(frame)) = second.next().await {
            if matches!(frame, tungstenite::Message::Close(_)) {
                break;
            }
        }
    });

    let client = WsClient::with_config(reconnect_test_config());
    client
        .connect(&format!("http://{}", address), None)
        .await
        .unwrap();
    for expiry in &expected_expiries {
        client
            .subscribe_with_options(
                "options_chain",
                None,
                Some(expiry.clone()),
                Some("call".to_string()),
            )
            .await
            .unwrap();
    }

    timeout(Duration::from_secs(2), replayed_rx)
        .await
        .expect("client did not replay both filtered subscriptions")
        .unwrap();
    let state = wait_for_state(&client, |state| {
        matches!(state, WsConnectionState::RecoveryRequired(_))
    })
    .await;
    let WsConnectionState::RecoveryRequired(plan) = state else {
        unreachable!();
    };
    assert!(plan.snapshot_resubscribe);
    assert_eq!(client.shared.subscriptions.read().await.len(), 2);

    client.disconnect().await;
    server.await.unwrap();
}

#[tokio::test]
async fn reconnects_and_restores_filtered_replaceable_subscription() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (replayed_tx, replayed_rx) = oneshot::channel();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut first = tokio_tungstenite::accept_async(stream).await.unwrap();
        let initial = acknowledge_subscription(&mut first, "indicative_market_data").await;
        assert_eq!(
            initial
                .get("symbols")
                .and_then(|value| value.as_array())
                .and_then(|symbols| symbols.first())
                .and_then(|value| value.as_str()),
            Some("BTC")
        );

        let close_reason = serde_json::to_string(&WsSlowConsumerCloseReason::new(
            WsDeliveryClass::ReplaceablePublic,
            WsPressureCause::MessageAge,
        ))
        .unwrap();
        first
            .send(tungstenite::Message::Close(Some(CloseFrame {
                code: CloseCode::Policy,
                reason: close_reason.into(),
            })))
            .await
            .unwrap();

        let (stream, _) = listener.accept().await.unwrap();
        let mut second = tokio_tungstenite::accept_async(stream).await.unwrap();
        let replayed = acknowledge_subscription(&mut second, "indicative_market_data").await;
        assert_eq!(
            replayed
                .get("symbols")
                .and_then(|value| value.as_array())
                .and_then(|symbols| symbols.first())
                .and_then(|value| value.as_str()),
            Some("BTC")
        );
        let _ = replayed_tx.send(());
        while let Some(Ok(frame)) = second.next().await {
            if matches!(frame, tungstenite::Message::Close(_)) {
                break;
            }
        }
    });

    let client = WsClient::with_config(reconnect_test_config());
    client
        .connect(&format!("http://{}", address), None)
        .await
        .unwrap();
    client
        .subscribe_with_options(
            "indicative_market_data",
            Some(vec!["BTC".to_string()]),
            None,
            None,
        )
        .await
        .unwrap();

    timeout(Duration::from_secs(2), replayed_rx)
        .await
        .expect("client did not reconnect")
        .unwrap();
    wait_for_state(&client, |state| matches!(state, WsConnectionState::Ready)).await;
    assert!(matches!(
        client.last_disconnect_reason(),
        Some(WsDisconnectReason::SlowConsumer(reason))
            if reason.delivery_class == WsDeliveryClass::ReplaceablePublic
    ));

    client.disconnect().await;
    server.await.unwrap();
}

#[tokio::test]
async fn rejected_subscription_replay_stops_reconnecting() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stopped_tx, stopped_rx) = oneshot::channel();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut first = tokio_tungstenite::accept_async(stream).await.unwrap();
        acknowledge_subscription(&mut first, "indicative_market_data").await;
        first.send(tungstenite::Message::Close(None)).await.unwrap();

        let (stream, _) = listener.accept().await.unwrap();
        let mut second = tokio_tungstenite::accept_async(stream).await.unwrap();
        let replayed = next_text(&mut second).await;
        assert_eq!(
            replayed.get("channel").and_then(|value| value.as_str()),
            Some("indicative_market_data")
        );
        second
            .send(tungstenite::Message::Text(
                r#"{"type":"Error","message":"invalid subscription"}"#.to_string(),
            ))
            .await
            .unwrap();

        assert!(
            timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "client opened another connection after a permanent replay rejection"
        );
        let _ = stopped_tx.send(());
    });

    let client = WsClient::with_config(reconnect_test_config());
    client
        .connect(&format!("http://{}", address), None)
        .await
        .unwrap();
    client
        .subscribe(vec!["indicative_market_data"])
        .await
        .unwrap();

    wait_for_state(&client, |state| {
        matches!(state, WsConnectionState::Disconnected)
    })
    .await;
    timeout(Duration::from_secs(1), stopped_rx)
        .await
        .expect("server did not verify reconnect stopped")
        .unwrap();
    assert!(matches!(
        client.last_disconnect_reason(),
        Some(WsDisconnectReason::Protocol(message)) if message == "invalid subscription"
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn reauthenticates_and_requires_private_state_reconciliation() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let wallet: AccountAddress = "0x0000000000000000000000000000000000000001"
        .parse()
        .unwrap();
    let wallet_string = wallet.to_string();
    let server_wallet = wallet_string.clone();
    let (replayed_tx, replayed_rx) = oneshot::channel();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut first = tokio_tungstenite::accept_async(stream).await.unwrap();
        let auth = next_text(&mut first).await;
        assert_eq!(
            auth.get("type").and_then(|value| value.as_str()),
            Some("Authenticate")
        );
        first
            .send(tungstenite::Message::Text(format!(
                r#"{{"type":"Authenticated","wallet":"{}"}}"#,
                server_wallet
            )))
            .await
            .unwrap();
        acknowledge_subscription(&mut first, "fills").await;

        let close_reason = serde_json::to_string(&WsSlowConsumerCloseReason::new(
            WsDeliveryClass::PrivateDurable,
            WsPressureCause::WriteTimeout,
        ))
        .unwrap();
        first
            .send(tungstenite::Message::Close(Some(CloseFrame {
                code: CloseCode::Policy,
                reason: close_reason.into(),
            })))
            .await
            .unwrap();

        let (stream, _) = listener.accept().await.unwrap();
        let mut second = tokio_tungstenite::accept_async(stream).await.unwrap();
        let auth = next_text(&mut second).await;
        assert_eq!(
            auth.get("type").and_then(|value| value.as_str()),
            Some("Authenticate")
        );
        second
            .send(tungstenite::Message::Text(format!(
                r#"{{"type":"Authenticated","wallet":"{}"}}"#,
                server_wallet
            )))
            .await
            .unwrap();
        acknowledge_subscription(&mut second, "fills").await;
        let _ = replayed_tx.send(());
        while let Some(Ok(frame)) = second.next().await {
            if matches!(frame, tungstenite::Message::Close(_)) {
                break;
            }
        }
    });

    let client = WsClient::with_config(reconnect_test_config());
    client
        .connect(&format!("http://{}", address), Some(&wallet))
        .await
        .unwrap();
    client.subscribe(vec!["fills"]).await.unwrap();

    timeout(Duration::from_secs(2), replayed_rx)
        .await
        .expect("client did not reconnect")
        .unwrap();
    let state = wait_for_state(&client, |state| {
        matches!(state, WsConnectionState::RecoveryRequired(_))
    })
    .await;
    let WsConnectionState::RecoveryRequired(plan) = state else {
        unreachable!();
    };
    assert!(plan.rest_reconcile);
    assert!(!plan.snapshot_resubscribe);
    assert!(matches!(
        client.last_disconnect_reason(),
        Some(WsDisconnectReason::SlowConsumer(reason))
            if reason.delivery_class == WsDeliveryClass::PrivateDurable
    ));

    client.mark_recovered().unwrap();
    assert_eq!(client.connection_state(), WsConnectionState::Ready);
    client.disconnect().await;
    server.await.unwrap();
}

#[tokio::test]
async fn equivalent_symbol_filters_are_one_logical_subscription() {
    let client = WsClient::with_config(reconnect_test_config());
    // Record desired subscriptions without a live socket.
    client.shared.started.store(true, Ordering::Release);

    client
        .subscribe_indicative_market_data(Some(vec!["btc-1-c".to_string(), "ETH-1-C".to_string()]))
        .await
        .unwrap();
    client
        .subscribe_indicative_market_data(Some(vec![
            " ETH-1-C ".to_string(),
            "BTC-1-C".to_string(),
        ]))
        .await
        .unwrap();

    let replay: Vec<String> = client
        .shared
        .subscriptions
        .read()
        .await
        .values()
        .map(|desired| desired.subscribe.clone())
        .collect();
    assert_eq!(
        replay.len(),
        1,
        "case, spacing and order do not matter: {replay:?}"
    );
    assert!(
        !replay[0].contains("batch"),
        "batching is a channel, not a subscribe flag: {}",
        replay[0]
    );

    // Plain subscriptions are unchanged on the wire.
    client
        .subscribe_with_options("orderbook", None, None, None)
        .await
        .unwrap();
    assert!(client
        .shared
        .subscriptions
        .read()
        .await
        .contains_key(r#"{"type":"Subscribe","channel":"orderbook"}"#));
}

#[tokio::test]
async fn the_indicative_helper_unsubscribes_only_its_own_subscription() {
    let client = WsClient::with_config(reconnect_test_config());
    client.shared.started.store(true, Ordering::Release);
    client
        .subscribe_indicative_market_data(Some(vec!["BTC-1-C".to_string()]))
        .await
        .unwrap();
    for channel in ["indicative_market_data", "orderbook"] {
        client
            .subscribe_with_options(channel, None, None, None)
            .await
            .unwrap();
    }

    let channels = || async {
        let mut channels: Vec<String> = client
            .shared
            .subscriptions
            .read()
            .await
            .values()
            .map(|desired| desired.channel.clone())
            .collect();
        channels.sort();
        channels
    };

    // Generic unsubscribe is exact: the helper's v2 subscription survives.
    client
        .unsubscribe(vec!["indicative_market_data"])
        .await
        .unwrap();
    assert_eq!(
        channels().await,
        vec![
            "indicative_market_data_v2".to_string(),
            "orderbook".to_string()
        ]
    );

    // The helper removes only what the helper created.
    client
        .subscribe_with_options("indicative_market_data", None, None, None)
        .await
        .unwrap();
    client.unsubscribe_indicative_market_data().await.unwrap();
    assert_eq!(
        channels().await,
        vec![
            "indicative_market_data".to_string(),
            "orderbook".to_string()
        ]
    );
}

#[tokio::test]
async fn direct_subscriptions_to_both_indicative_channels_stay_independent() {
    let client = WsClient::with_config(reconnect_test_config());
    client.shared.started.store(true, Ordering::Release);
    for channel in ["indicative_market_data", "indicative_market_data_v2"] {
        client
            .subscribe_with_options(channel, Some(vec!["BTC-1-C".to_string()]), None, None)
            .await
            .unwrap();
    }

    client
        .unsubscribe(vec!["indicative_market_data"])
        .await
        .unwrap();
    let channels: Vec<String> = client
        .shared
        .subscriptions
        .read()
        .await
        .values()
        .map(|desired| desired.channel.clone())
        .collect();
    assert_eq!(channels, vec!["indicative_market_data_v2".to_string()]);
}

#[tokio::test]
async fn reconnect_fallback_waits_for_the_v1_confirmation_before_ready() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (fallback_sent_tx, fallback_sent_rx) = oneshot::channel();
    let (release_ack_tx, release_ack_rx) = oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        // First server: supports v2.
        let (stream, _) = listener.accept().await.unwrap();
        let mut first = tokio_tungstenite::accept_async(stream).await.unwrap();
        let subscribe = next_text(&mut first).await;
        assert_eq!(
            subscribe.get("channel").and_then(|value| value.as_str()),
            Some("indicative_market_data_v2")
        );
        first
            .send(tungstenite::Message::Text(
                r#"{"type":"Subscribed","channel":"indicative_market_data_v2","batch":true}"#
                    .to_string(),
            ))
            .await
            .unwrap();
        first.send(tungstenite::Message::Close(None)).await.unwrap();

        // Second server: predates v2 and confirms it without the marker.
        let (stream, _) = listener.accept().await.unwrap();
        let mut second = tokio_tungstenite::accept_async(stream).await.unwrap();
        let replayed = next_text(&mut second).await;
        assert_eq!(
            replayed.get("channel").and_then(|value| value.as_str()),
            Some("indicative_market_data_v2")
        );
        second
            .send(tungstenite::Message::Text(
                r#"{"type":"Subscribed","channel":"indicative_market_data_v2"}"#.to_string(),
            ))
            .await
            .unwrap();
        let unsubscribe = next_text(&mut second).await;
        assert_eq!(
            unsubscribe.get("type").and_then(|value| value.as_str()),
            Some("Unsubscribe")
        );
        let v1 = next_text(&mut second).await;
        assert_eq!(
            v1.get("channel").and_then(|value| value.as_str()),
            Some("indicative_market_data")
        );
        let _ = fallback_sent_tx.send(());
        release_ack_rx.await.unwrap();
        second
            .send(tungstenite::Message::Text(
                r#"{"type":"Subscribed","channel":"indicative_market_data"}"#.to_string(),
            ))
            .await
            .unwrap();
        while let Some(Ok(frame)) = second.next().await {
            if matches!(frame, tungstenite::Message::Close(_)) {
                break;
            }
        }
    });

    let client = WsClient::with_config(reconnect_test_config());
    client
        .connect(&format!("http://{address}"), None)
        .await
        .unwrap();
    client
        .subscribe_indicative_market_data(Some(vec!["BTC-1-C".to_string()]))
        .await
        .unwrap();

    timeout(Duration::from_secs(2), fallback_sent_rx)
        .await
        .expect("the client did not fall back on reconnect")
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        matches!(
            client.connection_state(),
            WsConnectionState::Resubscribing { .. }
        ),
        "not ready until the v1 confirmation arrives: {:?}",
        client.connection_state()
    );
    release_ack_tx.send(()).unwrap();
    wait_for_state(&client, |state| state.transport_connected()).await;
    assert!(!client.indicative_batched());

    client.disconnect().await;
    server.await.unwrap();
}

#[tokio::test]
async fn fallback_leaves_direct_v2_subscriptions_alone() {
    let client = WsClient::with_config(reconnect_test_config());
    client.shared.started.store(true, Ordering::Release);
    client
        .subscribe_with_options(
            "indicative_market_data_v2",
            Some(vec!["ETH-1-C".to_string()]),
            None,
            None,
        )
        .await
        .unwrap();
    client
        .subscribe_indicative_market_data(Some(vec!["BTC-1-C".to_string()]))
        .await
        .unwrap();

    client
        .shared
        .v2_fallback_pending
        .store(true, Ordering::Release);
    let (unsubscribes, replacements) = super::take_v2_fallback(&client.shared).await.unwrap();
    assert_eq!(unsubscribes.len(), 1, "only the logical subscription moves");
    assert_eq!(replacements.len(), 1);

    let mut stored: Vec<(String, bool)> = client
        .shared
        .subscriptions
        .read()
        .await
        .values()
        .map(|desired| (desired.channel.clone(), desired.logical_indicative))
        .collect();
    stored.sort();
    assert_eq!(
        stored,
        vec![
            ("indicative_market_data".to_string(), true),
            ("indicative_market_data_v2".to_string(), false),
        ]
    );

    // The direct v2 subscription still answers to its own name, and the
    // fallen-back logical one to the helper.
    client
        .unsubscribe(vec!["indicative_market_data_v2"])
        .await
        .unwrap();
    client.unsubscribe_indicative_market_data().await.unwrap();
    assert!(client.shared.subscriptions.read().await.is_empty());
}

#[tokio::test]
async fn a_rejected_direct_v2_subscription_fails_restoration() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (first_done_tx, first_done_rx) = oneshot::channel();
    let (close_first_tx, close_first_rx) = oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut first = tokio_tungstenite::accept_async(stream).await.unwrap();
        // This server supports v2: both subscriptions are confirmed with the
        // marker, so nothing falls back yet.
        for _ in 0..2 {
            let subscribe = next_text(&mut first).await;
            assert_eq!(
                subscribe.get("channel").and_then(|value| value.as_str()),
                Some("indicative_market_data_v2")
            );
            first
                .send(tungstenite::Message::Text(
                    r#"{"type":"Subscribed","channel":"indicative_market_data_v2","batch":true}"#
                        .to_string(),
                ))
                .await
                .unwrap();
        }
        let _ = first_done_tx.send(());
        let _ = close_first_rx.await;
        first.send(tungstenite::Message::Close(None)).await.unwrap();

        let (stream, _) = listener.accept().await.unwrap();
        let mut second = tokio_tungstenite::accept_async(stream).await.unwrap();
        let replayed = next_text(&mut second).await;
        assert_eq!(
            replayed.get("channel").and_then(|value| value.as_str()),
            Some("indicative_market_data_v2")
        );
        second
            .send(tungstenite::Message::Text(
                r#"{"type":"Error","message":"Unknown channel 'indicative_market_data_v2'","code":"unknown_channel","channel":"indicative_market_data_v2"}"#
                    .to_string(),
            ))
            .await
            .unwrap();
        while let Some(Ok(frame)) = second.next().await {
            if matches!(frame, tungstenite::Message::Close(_)) {
                break;
            }
        }
    });

    let client = WsClient::with_config(reconnect_test_config());
    client
        .connect(&format!("http://{address}"), None)
        .await
        .unwrap();
    client
        .subscribe_with_options("indicative_market_data_v2", None, None, None)
        .await
        .unwrap();
    // A logical v2 subscription alongside does not make the direct one's
    // rejection recoverable.
    client
        .subscribe_indicative_market_data(Some(vec!["BTC-1-C".to_string()]))
        .await
        .unwrap();
    timeout(Duration::from_secs(2), first_done_rx)
        .await
        .expect("both subscriptions confirmed")
        .unwrap();
    assert!(client.indicative_batched());
    close_first_tx.send(()).unwrap();

    wait_for_state(&client, |state| {
        matches!(state, WsConnectionState::Disconnected)
    })
    .await;
    assert!(matches!(
        client.last_disconnect_reason(),
        Some(WsDisconnectReason::Protocol(message)) if message.contains("does not support")
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn an_unmarked_confirmation_for_a_direct_v2_subscription_fails_restoration() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (first_done_tx, first_done_rx) = oneshot::channel();
    let (close_first_tx, close_first_rx) = oneshot::channel::<()>();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut first = tokio_tungstenite::accept_async(stream).await.unwrap();
        next_text(&mut first).await;
        first
            .send(tungstenite::Message::Text(
                r#"{"type":"Subscribed","channel":"indicative_market_data_v2","batch":true}"#
                    .to_string(),
            ))
            .await
            .unwrap();
        let _ = first_done_tx.send(());
        let _ = close_first_rx.await;
        first.send(tungstenite::Message::Close(None)).await.unwrap();

        // A legacy server confirms the unknown channel without the marker.
        let (stream, _) = listener.accept().await.unwrap();
        let mut second = tokio_tungstenite::accept_async(stream).await.unwrap();
        next_text(&mut second).await;
        second
            .send(tungstenite::Message::Text(
                r#"{"type":"Subscribed","channel":"indicative_market_data_v2"}"#.to_string(),
            ))
            .await
            .unwrap();
        while let Some(Ok(frame)) = second.next().await {
            if matches!(frame, tungstenite::Message::Close(_)) {
                break;
            }
        }
    });

    let client = WsClient::with_config(reconnect_test_config());
    client
        .connect(&format!("http://{address}"), None)
        .await
        .unwrap();
    client
        .subscribe_with_options("indicative_market_data_v2", None, None, None)
        .await
        .unwrap();
    timeout(Duration::from_secs(2), first_done_rx)
        .await
        .expect("confirmed")
        .unwrap();
    close_first_tx.send(()).unwrap();

    wait_for_state(&client, |state| {
        matches!(state, WsConnectionState::Disconnected)
    })
    .await;
    assert!(matches!(
        client.last_disconnect_reason(),
        Some(WsDisconnectReason::Protocol(message)) if message.contains("does not support")
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn fallback_onto_a_direct_v1_subscription_keeps_both_owners() {
    let client = WsClient::with_config(reconnect_test_config());
    client.shared.started.store(true, Ordering::Release);
    let symbols = Some(vec!["BTC-1-C".to_string()]);
    client
        .subscribe_with_options("indicative_market_data", symbols.clone(), None, None)
        .await
        .unwrap();
    client
        .subscribe_indicative_market_data(symbols)
        .await
        .unwrap();

    client
        .shared
        .v2_fallback_pending
        .store(true, Ordering::Release);
    super::take_v2_fallback(&client.shared).await.unwrap();

    let owners = || async {
        client
            .shared
            .subscriptions
            .read()
            .await
            .values()
            .map(|desired| {
                (
                    desired.channel.clone(),
                    desired.direct,
                    desired.logical_indicative,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        owners().await,
        vec![("indicative_market_data".to_string(), true, true)],
        "one v1 entry, owned by both"
    );

    client.unsubscribe_indicative_market_data().await.unwrap();
    assert_eq!(
        owners().await,
        vec![("indicative_market_data".to_string(), true, false)],
        "the direct subscription survives the helper's release"
    );
    client
        .unsubscribe(vec!["indicative_market_data"])
        .await
        .unwrap();
    assert!(owners().await.is_empty());
}

#[tokio::test]
async fn fallback_keeps_a_direct_claim_on_a_shared_v2_entry() {
    let client = WsClient::with_config(reconnect_test_config());
    client.shared.started.store(true, Ordering::Release);
    let symbols = Some(vec!["BTC-1-C".to_string()]);
    client
        .subscribe_with_options("indicative_market_data_v2", symbols.clone(), None, None)
        .await
        .unwrap();
    client
        .subscribe_indicative_market_data(symbols)
        .await
        .unwrap();

    client
        .shared
        .v2_fallback_pending
        .store(true, Ordering::Release);
    let (unsubscribes, replacements) = super::take_v2_fallback(&client.shared).await.unwrap();
    assert!(
        unsubscribes.is_empty(),
        "the direct owner still wants the v2 subscription"
    );
    assert_eq!(replacements.len(), 1);

    let mut owners: Vec<(String, bool, bool)> = client
        .shared
        .subscriptions
        .read()
        .await
        .values()
        .map(|desired| {
            (
                desired.channel.clone(),
                desired.direct,
                desired.logical_indicative,
            )
        })
        .collect();
    owners.sort();
    assert_eq!(
        owners,
        vec![
            ("indicative_market_data".to_string(), false, true),
            ("indicative_market_data_v2".to_string(), true, false),
        ]
    );
}
