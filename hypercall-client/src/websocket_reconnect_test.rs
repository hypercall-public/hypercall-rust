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
    let deadline = Instant::now() + Duration::from_secs(2);
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

    plan.include_channel("future_channel");
    assert!(plan.application_reconcile);
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
