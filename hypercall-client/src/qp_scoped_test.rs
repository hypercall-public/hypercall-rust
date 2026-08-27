use super::*;
use crate::wallet::HypercallWallet;
use hypercall_ws_protocol::QpServerMessage;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;

const TEST_PRIVATE_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

fn scope(label: &str) -> ScopeId {
    ScopeId::from_label(label).expect("test scope label fits")
}

fn quote(instrument: &str) -> IndicativeQuote {
    IndicativeQuote {
        instrument: instrument.to_string(),
        bid_price: "100".to_string(),
        ask_price: "101".to_string(),
        max_bid_size: "10".to_string(),
        max_ask_size: "10".to_string(),
    }
}

// ---- ScopedOutboundState (pure) ----

#[test]
fn set_conflates_per_scope_and_counts_superseded() {
    let mut state = ScopedOutboundState::default();
    state.set(scope("A"), vec![quote("A-1")]);
    state.set(scope("A"), vec![quote("A-2")]);
    state.set(scope("B"), vec![quote("B-1")]);

    assert_eq!(state.take_superseded(), 1, "A-1 superseded while dirty");

    let first = state.next_frame(true).unwrap();
    let second = state.next_frame(true).unwrap();
    assert!(state.next_frame(true).is_none());

    match (first, second) {
        (
            OutboundFrame::Scoped {
                scope: s1,
                quotes: q1,
            },
            OutboundFrame::Scoped {
                scope: s2,
                quotes: q2,
            },
        ) => {
            assert_eq!(s1, scope("A"));
            assert_eq!(q1[0].instrument, "A-2", "latest snapshot won");
            assert_eq!(s2, scope("B"));
            assert_eq!(q2[0].instrument, "B-1");
        }
        other => panic!("expected two scoped frames, got {other:?}"),
    }
}

/// The borrow-serializing shadow enum must stay byte-identical to the
/// owned wire enum for both indicative variants.
#[test]
fn outbound_frame_wire_shape_matches_client_outbound() {
    let scoped = OutboundFrame::Scoped {
        scope: scope("MSFT"),
        quotes: Arc::new(vec![quote("MSFT-C-450")]),
    };
    let owned = ClientOutbound::ScopedIndicativeQuoteUpdate {
        scope: scope("MSFT"),
        quotes: vec![quote("MSFT-C-450")],
    };
    assert_eq!(
        scoped.to_json().unwrap(),
        serde_json::to_string(&owned).unwrap()
    );

    let union = OutboundFrame::Union(vec![quote("A-1"), quote("B-1")]);
    let owned = ClientOutbound::IndicativeQuoteUpdate {
        quotes: vec![quote("A-1"), quote("B-1")],
    };
    assert_eq!(
        union.to_json().unwrap(),
        serde_json::to_string(&owned).unwrap()
    );
}

#[test]
fn empty_snapshot_replays_until_confirmed_then_slot_is_dropped() {
    let mut state = ScopedOutboundState::default();
    state.set(scope("A"), vec![quote("A-1")]);
    let _ = state.next_frame(true).unwrap();

    state.set(scope("A"), Vec::new());
    match state.next_frame(true).unwrap() {
        OutboundFrame::Scoped { quotes, .. } => {
            assert!(quotes.is_empty(), "explicit clear goes to the wire");
        }
        other => panic!("expected scoped frame, got {other:?}"),
    }

    // The write was NOT confirmed (socket died): reconnect replay must
    // re-send the clear, not silently lose it.
    state.mark_all_dirty();
    match state.next_frame(true).unwrap() {
        OutboundFrame::Scoped { quotes, .. } => {
            assert!(quotes.is_empty(), "unconfirmed clear is replayed");
        }
        other => panic!("expected scoped frame, got {other:?}"),
    }

    // Once the write is confirmed the slot retires and stops replaying.
    state.finish_scoped_clear(scope("A"));
    state.mark_all_dirty();
    assert!(state.next_frame(true).is_none());
}

#[test]
fn confirmed_clear_keeps_slot_when_scope_was_reset_meanwhile() {
    let mut state = ScopedOutboundState::default();
    state.set(scope("A"), Vec::new());
    let _ = state.next_frame(true).unwrap();

    // The caller revived the scope while the clear was on the wire: the
    // confirmation must not discard the new snapshot.
    state.set(scope("A"), vec![quote("A-2")]);
    state.finish_scoped_clear(scope("A"));
    match state.next_frame(true).unwrap() {
        OutboundFrame::Scoped { quotes, .. } => {
            assert_eq!(quotes[0].instrument, "A-2");
        }
        other => panic!("expected scoped frame, got {other:?}"),
    }
}

#[test]
fn mark_all_dirty_replays_every_live_slot() {
    let mut state = ScopedOutboundState::default();
    state.set(scope("A"), vec![quote("A-1")]);
    state.set(scope("B"), vec![quote("B-1")]);
    while state.next_frame(true).is_some() {}

    state.mark_all_dirty();
    let mut replayed = Vec::new();
    while let Some(OutboundFrame::Scoped { scope, .. }) = state.next_frame(true) {
        replayed.push(scope);
    }
    replayed.sort_by_key(|s| s.to_hex());
    assert_eq!(replayed, vec![scope("A"), scope("B")]);
}

#[test]
fn union_clear_replays_until_confirmed() {
    let mut state = ScopedOutboundState::default();
    state.set(scope("A"), vec![quote("A-1")]);
    let _ = state.next_frame(false).unwrap();
    state.finish_union_flush();

    // The wallet's ONLY slot is cleared and the union write fails: the
    // empty slot must survive so reconnect replay re-sends the clearing
    // union instead of silently losing it (nothing else would ever go
    // dirty again).
    state.set(scope("A"), Vec::new());
    match state.next_frame(false).unwrap() {
        OutboundFrame::Union(quotes) => assert!(quotes.is_empty()),
        other => panic!("expected union, got {other:?}"),
    }
    state.mark_all_dirty();
    match state.next_frame(false).unwrap() {
        OutboundFrame::Union(quotes) => assert!(quotes.is_empty(), "clear replayed"),
        other => panic!("expected union, got {other:?}"),
    }

    // Once confirmed, the cleared slot retires and nothing replays.
    state.finish_union_flush();
    state.mark_all_dirty();
    assert!(state.next_frame(false).is_none());
}

#[test]
fn fallback_flushes_union_as_one_full_snapshot() {
    let mut state = ScopedOutboundState::default();
    state.set(scope("A"), vec![quote("A-1")]);
    state.set(scope("B"), vec![quote("B-1")]);

    match state.next_frame(false).unwrap() {
        OutboundFrame::Union(quotes) => {
            let mut instruments: Vec<_> = quotes.iter().map(|q| q.instrument.as_str()).collect();
            instruments.sort_unstable();
            assert_eq!(instruments, vec!["A-1", "B-1"]);
        }
        other => panic!("expected full snapshot, got {other:?}"),
    }
    assert!(
        state.next_frame(false).is_none(),
        "one union frame discharges all dirtiness"
    );
}

// ---- integration against a mock server ----

struct NoopTestCallbacks;
impl QpClientCallbacks for NoopTestCallbacks {}

/// Re-enters the handle (and therefore the outbound mutex) from inside a
/// callback; deadlocks if the runtime invokes callbacks under the lock.
struct ReentrantCallbacks {
    handle: std::sync::Mutex<Option<QpScopedHandle>>,
}
impl QpClientCallbacks for ReentrantCallbacks {
    fn on_stale_messages_drained(&self, _count: u64) {
        if let Some(handle) = self.handle.lock().unwrap().as_ref() {
            handle.set_indicative_quotes(scope("REENTER"), Vec::new());
        }
    }
}

/// Accept one connection: consume the auth frame, reply Authenticated with
/// `capabilities`, then forward every received text frame until close.
async fn mock_qp_session(
    listener: &TcpListener,
    capabilities: Vec<String>,
    frames_tx: mpsc::Sender<ClientOutbound>,
    close_after: usize,
) {
    let (stream, _) = listener.accept().await.unwrap();
    let mut ws = accept_async(stream).await.unwrap();

    let auth = ws.next().await.unwrap().unwrap();
    assert!(matches!(auth, Message::Text(_)));
    let authenticated = QpServerMessage::Authenticated {
        wallet: "0x0000000000000000000000000000000000000000".to_string(),
        capabilities,
    };
    ws.send(Message::Text(
        serde_json::to_string(&authenticated).unwrap(),
    ))
    .await
    .unwrap();

    let mut received = 0usize;
    while received < close_after {
        match ws.next().await {
            Some(Ok(Message::Text(text))) => {
                let msg: ClientOutbound = serde_json::from_str(&text).unwrap();
                frames_tx.send(msg).await.unwrap();
                received += 1;
            }
            Some(Ok(Message::Ping(data))) => {
                let _ = ws.send(Message::Pong(data)).await;
            }
            Some(Ok(_)) => {}
            _ => return,
        }
    }
}

#[tokio::test]
async fn scoped_frames_flow_and_replay_after_reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (frames_tx, mut frames_rx) = mpsc::channel(16);

    let server = tokio::spawn(async move {
        // Session 1: scoped-capable, close after both scopes arrive.
        mock_qp_session(
            &listener,
            vec![CAP_SCOPED_INDICATIVE.to_string()],
            frames_tx.clone(),
            2,
        )
        .await;
        // Session 2 (reconnect): both scopes must be REPLAYED without new
        // set_indicative_quotes calls.
        mock_qp_session(
            &listener,
            vec![CAP_SCOPED_INDICATIVE.to_string()],
            frames_tx.clone(),
            2,
        )
        .await;
    });

    let wallet = Arc::new(HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 999).unwrap());
    let (handle, runtime) = qp_scoped_channel();
    handle.set_indicative_quotes(scope("MSFT"), vec![quote("MSFT-C-450")]);
    handle.set_indicative_quotes(scope("SPX"), vec![quote("SP500-C-5000")]);

    let mut config = QpClientConfig::new(format!("http://{addr}"));
    config.reconnect_delay = Duration::from_millis(50);
    let (inbound_tx, _inbound_rx) = mpsc::channel(16);
    let client = tokio::spawn(run_qp_scoped_client(
        config,
        wallet,
        runtime,
        inbound_tx,
        Arc::new(NoopTestCallbacks),
    ));

    let mut session_frames = Vec::new();
    for _ in 0..4 {
        let frame = tokio::time::timeout(Duration::from_secs(5), frames_rx.recv())
            .await
            .expect("frame within timeout")
            .expect("channel open");
        session_frames.push(frame);
    }

    // Both sessions saw both scopes (order within a session is unspecified).
    for chunk in session_frames.chunks(2) {
        let mut scopes: Vec<ScopeId> = chunk
            .iter()
            .map(|frame| match frame {
                ClientOutbound::ScopedIndicativeQuoteUpdate { scope, .. } => *scope,
                other => panic!("expected scoped frame, got {other:?}"),
            })
            .collect();
        scopes.sort_by_key(|s| s.to_hex());
        assert_eq!(scopes, vec![scope("MSFT"), scope("SPX")]);
    }

    client.abort();
    server.abort();
}

#[tokio::test]
async fn legacy_server_gets_union_full_snapshot_fallback() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (frames_tx, mut frames_rx) = mpsc::channel(16);

    let server = tokio::spawn(async move {
        // No capabilities: the client must never send a scoped frame.
        mock_qp_session(&listener, Vec::new(), frames_tx, 1).await;
    });

    let wallet = Arc::new(HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 999).unwrap());
    let (handle, runtime) = qp_scoped_channel();
    handle.set_indicative_quotes(scope("MSFT"), vec![quote("MSFT-C-450")]);
    handle.set_indicative_quotes(scope("SPX"), vec![quote("SP500-C-5000")]);

    let config = QpClientConfig::new(format!("http://{addr}"));
    let (inbound_tx, _inbound_rx) = mpsc::channel(16);
    let client = tokio::spawn(run_qp_scoped_client(
        config,
        wallet,
        runtime,
        inbound_tx,
        Arc::new(NoopTestCallbacks),
    ));

    let frame = tokio::time::timeout(Duration::from_secs(5), frames_rx.recv())
        .await
        .expect("frame within timeout")
        .expect("channel open");
    match frame {
        ClientOutbound::IndicativeQuoteUpdate { quotes } => {
            let mut instruments: Vec<_> = quotes.iter().map(|q| q.instrument.as_str()).collect();
            instruments.sort_unstable();
            assert_eq!(instruments, vec!["MSFT-C-450", "SP500-C-5000"]);
        }
        other => panic!("legacy server must get a full snapshot, got {other:?}"),
    }

    client.abort();
    server.abort();
}

#[tokio::test]
async fn connection_state_reports_negotiated_capabilities() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (frames_tx, mut frames_rx) = mpsc::channel(16);

    let server = tokio::spawn(async move {
        mock_qp_session(
            &listener,
            vec![CAP_SCOPED_INDICATIVE.to_string()],
            frames_tx,
            1,
        )
        .await;
    });

    let wallet = Arc::new(HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 999).unwrap());
    let (handle, runtime) = qp_scoped_channel();
    let mut state_rx = handle.connection_state();
    assert_eq!(*state_rx.borrow(), ConnectionState::Disconnected);

    let config = QpClientConfig::new(format!("http://{addr}"));
    let (inbound_tx, _inbound_rx) = mpsc::channel(16);
    let client = tokio::spawn(run_qp_scoped_client(
        config,
        wallet,
        runtime,
        inbound_tx,
        Arc::new(NoopTestCallbacks),
    ));

    tokio::time::timeout(Duration::from_secs(5), state_rx.changed())
        .await
        .expect("state change within timeout")
        .unwrap();
    assert!(state_rx.borrow().scoped_indicative());

    // Publishing after connect flows as a scoped frame.
    handle.set_indicative_quotes(scope("MSFT"), vec![quote("MSFT-C-450")]);
    let frame = tokio::time::timeout(Duration::from_secs(5), frames_rx.recv())
        .await
        .expect("frame within timeout")
        .expect("channel open");
    assert!(matches!(
        frame,
        ClientOutbound::ScopedIndicativeQuoteUpdate { .. }
    ));

    client.abort();
    server.abort();
}

#[tokio::test]
async fn connection_state_is_stored_even_with_no_subscribers() {
    // `watch::Sender::send` does not STORE the value when no receiver
    // exists at send time; the runtime must use `send_replace` so a
    // caller that subscribes only after connect still reads Connected.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (frames_tx, mut frames_rx) = mpsc::channel(16);

    let server = tokio::spawn(async move {
        // A high close_after keeps the session open while we assert.
        mock_qp_session(
            &listener,
            vec![CAP_SCOPED_INDICATIVE.to_string()],
            frames_tx,
            8,
        )
        .await;
    });

    let wallet = Arc::new(HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 999).unwrap());
    let (handle, runtime) = qp_scoped_channel();
    handle.set_indicative_quotes(scope("MSFT"), vec![quote("MSFT-C-450")]);

    let config = QpClientConfig::new(format!("http://{addr}"));
    let (inbound_tx, _inbound_rx) = mpsc::channel(16);
    let client = tokio::spawn(run_qp_scoped_client(
        config,
        wallet,
        runtime,
        inbound_tx,
        Arc::new(NoopTestCallbacks),
    ));

    // A delivered frame proves the Connected transition already happened
    // while zero receivers existed.
    let _frame = tokio::time::timeout(Duration::from_secs(5), frames_rx.recv())
        .await
        .expect("frame within timeout")
        .expect("channel open");

    let state_rx = handle.connection_state();
    assert!(
        state_rx.borrow().scoped_indicative(),
        "late subscriber must read the stored Connected state"
    );

    client.abort();
    server.abort();
}

#[tokio::test]
async fn watch_reads_disconnected_after_terminal_inbound_close() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();
        let _auth = ws.next().await.unwrap().unwrap();
        let authenticated = QpServerMessage::Authenticated {
            wallet: "0x0000000000000000000000000000000000000000".to_string(),
            capabilities: vec![CAP_SCOPED_INDICATIVE.to_string()],
        };
        ws.send(Message::Text(
            serde_json::to_string(&authenticated).unwrap(),
        ))
        .await
        .unwrap();
        // An inbound message the runtime must forward; the dropped
        // receiver turns this into the terminal INBOUND_CHANNEL_CLOSED.
        let rfq = r#"{"type":"rfq_request","rfq_id":"abc","legs":[],"taker_wallet":"0x123","request_timestamp":1,"response_deadline_ms":5000,"auto_execute":false}"#;
        ws.send(Message::Text(rfq.to_string())).await.unwrap();
        // Keep the socket open so the exit is driven by the closed
        // inbound channel, not by the transport.
        let _ = tokio::time::timeout(Duration::from_secs(5), ws.next()).await;
    });

    let wallet = Arc::new(HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 999).unwrap());
    let (handle, runtime) = qp_scoped_channel();
    let state_rx = handle.connection_state();

    let config = QpClientConfig::new(format!("http://{addr}"));
    let (inbound_tx, inbound_rx) = mpsc::channel(1);
    drop(inbound_rx);
    let client = tokio::spawn(run_qp_scoped_client(
        config,
        wallet,
        runtime,
        inbound_tx,
        Arc::new(NoopTestCallbacks),
    ));

    tokio::time::timeout(Duration::from_secs(5), client)
        .await
        .expect("runtime exits on closed inbound channel")
        .unwrap();
    assert_eq!(
        *state_rx.borrow(),
        ConnectionState::Disconnected,
        "watch must not report Connected after the runtime died"
    );
    server.abort();
}

#[tokio::test]
async fn stale_drain_callback_may_reenter_handle() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (frames_tx, mut frames_rx) = mpsc::channel(16);

    let server = tokio::spawn(async move {
        mock_qp_session(
            &listener,
            vec![CAP_SCOPED_INDICATIVE.to_string()],
            frames_tx,
            2,
        )
        .await;
    });

    let wallet = Arc::new(HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 999).unwrap());
    let (handle, runtime) = qp_scoped_channel();
    let callbacks = Arc::new(ReentrantCallbacks {
        handle: std::sync::Mutex::new(None),
    });
    *callbacks.handle.lock().unwrap() = Some(handle.clone());

    // Two sets before connect: the first snapshot is superseded while
    // dirty, so the runtime reports a stale drain on connect and the
    // callback re-enters the handle. If callbacks ran under the outbound
    // mutex this would deadlock and the recv below would time out.
    handle.set_indicative_quotes(scope("MSFT"), vec![quote("OLD")]);
    handle.set_indicative_quotes(scope("MSFT"), vec![quote("MSFT-C-450")]);

    let config = QpClientConfig::new(format!("http://{addr}"));
    let (inbound_tx, _inbound_rx) = mpsc::channel(16);
    let client = tokio::spawn(run_qp_scoped_client(
        config, wallet, runtime, inbound_tx, callbacks,
    ));

    let frame = tokio::time::timeout(Duration::from_secs(5), frames_rx.recv())
        .await
        .expect("no deadlock: frame within timeout")
        .expect("channel open");
    assert!(matches!(
        frame,
        ClientOutbound::ScopedIndicativeQuoteUpdate { .. }
    ));

    client.abort();
    server.abort();
}

#[tokio::test]
async fn keepalive_republishes_quiet_scopes() {
    // The server TTL-evicts rows it stops receiving; a quiet scope must
    // be republished on the keepalive interval with no new set() calls.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (frames_tx, mut frames_rx) = mpsc::channel(16);

    let server = tokio::spawn(async move {
        mock_qp_session(
            &listener,
            vec![CAP_SCOPED_INDICATIVE.to_string()],
            frames_tx,
            3,
        )
        .await;
    });

    let wallet = Arc::new(HypercallWallet::from_private_key(TEST_PRIVATE_KEY, 999).unwrap());
    let (handle, runtime) = qp_scoped_channel();
    handle.set_indicative_quotes(scope("MSFT"), vec![quote("MSFT-C-450")]);

    let mut config = QpClientConfig::new(format!("http://{addr}"));
    config.indicative_republish_interval = Duration::from_millis(100);
    let (inbound_tx, _inbound_rx) = mpsc::channel(16);
    let client = tokio::spawn(run_qp_scoped_client(
        config,
        wallet,
        runtime,
        inbound_tx,
        Arc::new(NoopTestCallbacks),
    ));

    // One set() call, three frames: the initial publish plus two
    // keepalive republishes of the unchanged snapshot.
    for _ in 0..3 {
        let frame = tokio::time::timeout(Duration::from_secs(5), frames_rx.recv())
            .await
            .expect("frame within timeout")
            .expect("channel open");
        match frame {
            ClientOutbound::ScopedIndicativeQuoteUpdate { scope: s, quotes } => {
                assert_eq!(s, scope("MSFT"));
                assert_eq!(quotes[0].instrument, "MSFT-C-450");
            }
            other => panic!("expected scoped frame, got {other:?}"),
        }
    }

    client.abort();
    server.abort();
}
