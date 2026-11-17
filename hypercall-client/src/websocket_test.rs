use super::*;
use sonic_rs::JsonValueTrait;

#[test]
fn test_ws_client_new() {
    let client = WsClient::new();
    // Client should be created successfully
    assert!(client.shared.messages.try_lock().is_ok());
    assert!(client.shared.pongs.try_lock().is_ok());
    assert!(client.shared.tx.try_lock().is_ok());
}

#[test]
fn test_ws_client_default() {
    let client = WsClient::default();
    assert!(client.shared.messages.try_lock().is_ok());
}

#[tokio::test]
async fn test_ws_client_get_messages_empty() {
    let client = WsClient::new();
    let messages = client.get_messages().await;
    assert!(messages.is_empty());
}

#[tokio::test]
async fn test_ws_client_clear_messages() {
    let client = WsClient::new();
    // Add a message manually
    client
        .shared
        .messages
        .lock()
        .await
        .push(sonic_rs::json!({"test": "value"}));
    assert_eq!(client.get_messages().await.len(), 1);

    // Clear messages
    client.clear_messages().await;
    assert!(client.get_messages().await.is_empty());
}

#[tokio::test]
async fn test_ws_client_count_messages() {
    let client = WsClient::new();
    // Add some messages
    {
        let mut messages = client.shared.messages.lock().await;
        messages.push(sonic_rs::json!({"type": "order", "id": 1}));
        messages.push(sonic_rs::json!({"type": "trade", "id": 2}));
        messages.push(sonic_rs::json!({"type": "order", "id": 3}));
    }

    let order_count = client
        .count_messages(|m| m.get("type").and_then(|t| t.as_str()) == Some("order"))
        .await;
    assert_eq!(order_count, 2);

    let trade_count = client
        .count_messages(|m| m.get("type").and_then(|t| t.as_str()) == Some("trade"))
        .await;
    assert_eq!(trade_count, 1);
}

#[tokio::test]
async fn test_ws_client_subscribe_not_connected() {
    let client = WsClient::new();
    let result = client.subscribe(vec!["orders"]).await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        crate::error::ClientError::WebSocket(_)
    ));
}

#[tokio::test]
async fn test_ws_client_ping_not_connected() {
    let client = WsClient::new();
    let result = client.ping().await;
    assert!(matches!(
        result.unwrap_err(),
        crate::error::ClientError::WebSocket(_)
    ));
}

#[tokio::test]
async fn test_ws_client_sends_ping_with_payload() {
    let client = WsClient::new();
    let (tx, mut rx) = mpsc::channel(1);
    *client
        .shared
        .tx
        .lock()
        .expect("WebSocket sender mutex poisoned") = Some(tx);

    client.ping_with_payload(b"health".to_vec()).await.unwrap();

    match rx.recv().await.unwrap() {
        tungstenite::Message::Ping(payload) => assert_eq!(payload, b"health"),
        other => panic!("Expected ping frame, got {other:?}"),
    }
}

#[tokio::test]
async fn test_ws_client_rejects_oversized_ping_payload() {
    let client = WsClient::new();
    let (tx, _rx) = mpsc::channel(1);
    *client
        .shared
        .tx
        .lock()
        .expect("WebSocket sender mutex poisoned") = Some(tx);

    let result = client.ping_with_payload(vec![0; 126]).await;
    assert!(matches!(
        result.unwrap_err(),
        crate::error::ClientError::WebSocket(_)
    ));
}

#[tokio::test]
async fn test_ws_client_tracks_received_pongs() {
    let client = WsClient::new();
    client.shared.pongs.lock().await.push(b"health".to_vec());

    let pong = client
        .wait_for_pong(|payload| payload == b"health", 100)
        .await;
    assert_eq!(pong, Some(b"health".to_vec()));

    client.clear_pongs().await;
    assert!(client.get_pongs().await.is_empty());
}

#[tokio::test]
async fn test_ws_client_responds_to_server_ping() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        ws.send(tungstenite::Message::Ping(b"server".to_vec()))
            .await
            .unwrap();

        match tokio::time::timeout(Duration::from_secs(1), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            tungstenite::Message::Pong(payload) => assert_eq!(payload, b"server"),
            other => panic!("Expected pong frame, got {other:?}"),
        }
    });

    let client = WsClient::new();
    client
        .connect(&format!("http://{}", addr), None)
        .await
        .unwrap();

    server.await.unwrap();
}

#[tokio::test]
async fn test_ws_client_wait_for_message_timeout() {
    let client = WsClient::new();

    // Wait for a message that doesn't exist with short timeout
    let result = client
        .wait_for_message(|m| m.get("never").is_some(), 100)
        .await;
    assert!(result.is_none());
}

#[tokio::test]
async fn test_ws_client_wait_for_message_found() {
    let client = WsClient::new();

    // Add a message in a background task after a delay
    let messages = client.shared.messages.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        messages.lock().await.push(sonic_rs::json!({"found": true}));
    });

    let result = client
        .wait_for_message(|m| m.get("found").is_some(), 500)
        .await;
    assert!(result.is_some());
    assert_eq!(result.unwrap()["found"], true);
}

#[test]
fn test_ws_message_subscribe_serialize() {
    let msg = WsMessage::Subscribe {
        channel: "orders".to_string(),
        symbols: None,
        expiry: None,
        option_type: None,
        view: None,
    };
    let json = sonic_rs::to_string(&msg).unwrap();
    assert!(json.contains("Subscribe"));
    assert!(json.contains("orders"));
}

#[test]
fn test_ws_message_unsubscribe_serialize() {
    let msg = WsMessage::Unsubscribe {
        channel: "trades".to_string(),
        symbols: None,
        expiry: None,
        option_type: None,
    };
    let json = sonic_rs::to_string(&msg).unwrap();
    assert!(json.contains("Unsubscribe"));
    assert!(json.contains("trades"));
}

#[test]
fn test_ws_message_subscribed_deserialize() {
    let json = r#"{"type": "Subscribed", "channel": "orders"}"#;
    let msg: WsMessage = sonic_rs::from_str(json).unwrap();
    match msg {
        WsMessage::Subscribed { channel, .. } => assert_eq!(channel, "orders"),
        _ => panic!("Expected Subscribed variant"),
    }
}

#[test]
fn test_ws_message_unsubscribed_deserialize() {
    let json = r#"{"type": "Unsubscribed", "channel": "trades"}"#;
    let msg: WsMessage = sonic_rs::from_str(json).unwrap();
    match msg {
        WsMessage::Unsubscribed { channel } => assert_eq!(channel, "trades"),
        _ => panic!("Expected Unsubscribed variant"),
    }
}

#[test]
fn test_ws_message_error_deserialize() {
    let json = r#"{"type": "Error", "message": "invalid channel"}"#;
    let msg: WsMessage = sonic_rs::from_str(json).unwrap();
    match msg {
        WsMessage::Error { message, .. } => assert_eq!(message, "invalid channel"),
        _ => panic!("Expected Error variant"),
    }
}

#[test]
fn test_ws_message_unknown_type_deserialize() {
    let json = r#"{"type": "unknown_type", "data": "something"}"#;
    let msg: WsMessage = sonic_rs::from_str(json).unwrap();
    match msg {
        WsMessage::Other => {}
        _ => panic!("Expected Other variant"),
    }
}

#[test]
fn test_ws_message_debug() {
    let msg = WsMessage::Subscribed {
        channel: "orders".to_string(),
        batch: None,
    };
    let debug = format!("{:?}", msg);
    assert!(debug.contains("Subscribe"));
    assert!(debug.contains("orders"));
}

#[test]
fn test_ws_message_clone() {
    let msg = WsMessage::Error {
        code: None,
        channel: None,
        message: "test error".to_string(),
    };
    let cloned = msg.clone();
    match cloned {
        WsMessage::Error { message, .. } => assert_eq!(message, "test error"),
        _ => panic!("Clone failed"),
    }
}
