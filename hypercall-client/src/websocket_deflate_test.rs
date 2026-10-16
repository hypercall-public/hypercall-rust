//! permessage-deflate negotiation, decoding, and fallback.
//!
//! The deflate peers here speak the wire protocol directly, so the tests pin
//! the exact bytes a Hypercall `/ws` server sends: an RFC 7692 response header
//! and text frames with RSV1 set whose payload is a raw DEFLATE block with the
//! trailing `00 00 ff ff` removed. The fallback peers are plain Tungstenite
//! servers, which ignore the offer the same way the API does with
//! `ws_permessage_deflate_enabled` off.

use super::*;
use flate2::{Compress, Compression, FlushCompress};
use futures::{SinkExt, StreamExt};
use sonic_rs::JsonValueTrait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{timeout, Instant};
use tokio_tungstenite::tungstenite;

const OPCODE_TEXT: u8 = 0x1;
const OPCODE_CLOSE: u8 = 0x8;
const DEFLATE_TAIL: [u8; 4] = [0x00, 0x00, 0xff, 0xff];
const HYPERCALL_RESPONSE: &str =
    "permessage-deflate; server_no_context_takeover; client_no_context_takeover";

/// Reads the upgrade request and answers `101`, echoing `extensions` if given.
async fn accept_handshake(stream: &mut TcpStream, extensions: Option<&str>) -> String {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let byte = timeout(Duration::from_secs(2), stream.read_u8())
            .await
            .expect("timed out reading the upgrade request")
            .expect("upgrade request ended early");
        head.push(byte);
    }
    let head = String::from_utf8(head).expect("upgrade request is ASCII");
    let key = header(&head, "sec-websocket-key").expect("client sent Sec-WebSocket-Key");
    let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
    let mut response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n"
    );
    if let Some(extensions) = extensions {
        response.push_str(&format!("Sec-WebSocket-Extensions: {extensions}\r\n"));
    }
    response.push_str("\r\n");
    stream.write_all(response.as_bytes()).await.unwrap();
    head
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

/// An unmasked server frame with FIN set.
fn server_frame(opcode: u8, compressed: bool, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x80 | if compressed { 0x40 } else { 0 } | opcode];
    match payload.len() {
        len @ 0..=125 => frame.push(len as u8),
        len @ 126..=0xffff => {
            frame.push(126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        }
        len => {
            frame.push(127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(payload);
    frame
}

struct ClientFrame {
    compressed: bool,
    opcode: u8,
    payload: Vec<u8>,
}

async fn read_client_frame(stream: &mut TcpStream) -> ClientFrame {
    let mut head = [0_u8; 2];
    timeout(Duration::from_secs(2), stream.read_exact(&mut head))
        .await
        .expect("timed out waiting for a client frame")
        .expect("client frame header");
    assert_ne!(head[0] & 0x80, 0, "client frames are not fragmented here");
    assert_ne!(head[1] & 0x80, 0, "client frames must be masked");
    let len = match head[1] & 0x7f {
        126 => u64::from(stream.read_u16().await.unwrap()),
        127 => stream.read_u64().await.unwrap(),
        len => u64::from(len),
    };
    let mut mask = [0_u8; 4];
    stream.read_exact(&mut mask).await.unwrap();
    let mut payload = vec![0_u8; usize::try_from(len).unwrap()];
    stream.read_exact(&mut payload).await.unwrap();
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte ^= mask[index % 4];
    }
    ClientFrame {
        compressed: head[0] & 0x40 != 0,
        opcode: head[0] & 0x0f,
        payload,
    }
}

/// Deflates one message the way RFC 7692 frames it: sync flush, tail removed.
fn deflate(compressor: &mut Compress, message: &[u8]) -> Vec<u8> {
    let consumed_before = compressor.total_in();
    let mut out = Vec::with_capacity(message.len() * 2 + 64);
    compressor
        .compress_vec(message, &mut out, FlushCompress::Sync)
        .unwrap();
    assert_eq!(
        compressor.total_in() - consumed_before,
        message.len() as u64,
        "the output buffer must hold the whole message"
    );
    assert!(out.ends_with(&DEFLATE_TAIL));
    out.truncate(out.len() - DEFLATE_TAIL.len());
    out
}

/// A fresh DEFLATE stream per message, as a no-context-takeover server sends.
fn deflate_independent(message: &[u8]) -> Vec<u8> {
    deflate(&mut Compress::new(Compression::new(3), false), message)
}

fn index_price_update(seq: u64) -> String {
    let prices = (0..40)
        .map(|index| {
            format!(
                r#"{{"underlying":"ASSET{index}","price":"{}.25","timestamp":1767225600000}}"#,
                60_000 + index
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"type":"IndexPriceUpdate","seq":{seq},"prices":[{prices}]}}"#)
}

async fn wait_for_seq(client: &WsClient, seq: u64) -> sonic_rs::Value {
    client
        .wait_for_message(
            |message| message.get("seq").and_then(|value| value.as_u64()) == Some(seq),
            2_000,
        )
        .await
        .unwrap_or_else(|| panic!("client never recorded message {seq}"))
}

async fn wait_for_close(stream: &mut TcpStream) {
    loop {
        let frame = read_client_frame(stream).await;
        if frame.opcode == OPCODE_CLOSE {
            return;
        }
    }
}

#[test]
fn deflate_is_offered_by_default_and_can_be_disabled() {
    assert!(WsClient::new().options.permessage_deflate);
    assert!(WsClient::default().options.permessage_deflate);
    assert!(
        !WsClient::new()
            .with_permessage_deflate(false)
            .options
            .permessage_deflate
    );
    assert!(
        WsClient::with_config(WsClientConfig::default())
            .with_permessage_deflate(true)
            .options
            .permessage_deflate
    );
}

#[tokio::test]
async fn negotiates_deflate_and_decodes_compressed_frames() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let first = index_price_update(1);
    let second = index_price_update(2);
    let small = r#"{"type":"Heartbeat","seq":3}"#;

    let server = {
        let (first, second) = (first.clone(), second.clone());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let head = accept_handshake(&mut stream, Some(HYPERCALL_RESPONSE)).await;
            let offer = header(&head, "sec-websocket-extensions")
                .expect("the client offers an extension by default");
            assert!(offer.starts_with("permessage-deflate"), "offer: {offer}");
            assert!(
                offer.contains("client_no_context_takeover"),
                "offer: {offer}"
            );

            // Client to server: RFC 7692 lets either side send uncompressed
            // messages, and client commands are small, so they go out raw.
            let subscribe = read_client_frame(&mut stream).await;
            assert!(!subscribe.compressed, "client frames are not compressed");
            assert_eq!(subscribe.opcode, OPCODE_TEXT);
            let subscribe: serde_json::Value = serde_json::from_slice(&subscribe.payload).unwrap();
            assert_eq!(subscribe["type"], "Subscribe");
            assert_eq!(subscribe["channel"], "index_prices");

            // Server to client: each message is its own DEFLATE stream, and a
            // message below the server's threshold goes out raw.
            for message in [&first, &second] {
                let compressed = deflate_independent(message.as_bytes());
                assert!(compressed.len() * 3 < message.len());
                stream
                    .write_all(&server_frame(OPCODE_TEXT, true, &compressed))
                    .await
                    .unwrap();
            }
            stream
                .write_all(&server_frame(OPCODE_TEXT, false, small.as_bytes()))
                .await
                .unwrap();
            wait_for_close(&mut stream).await;
        })
    };

    let client = WsClient::new();
    client
        .connect(&format!("http://{addr}"), None)
        .await
        .unwrap();
    client.subscribe(vec!["index_prices"]).await.unwrap();

    for (seq, raw) in [(1, &first), (2, &second)] {
        let received = wait_for_seq(&client, seq).await;
        let expected: sonic_rs::Value = sonic_rs::from_str(raw).unwrap();
        assert_eq!(received, expected);
    }
    wait_for_seq(&client, 3).await;
    assert_eq!(client.connection_state(), WsConnectionState::Ready);

    client.disconnect().await;
    timeout(Duration::from_secs(2), server)
        .await
        .expect("server finished")
        .unwrap();
}

#[tokio::test]
async fn decodes_compressed_frames_with_server_context_takeover() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let message = index_price_update(7);

    let server = {
        let message = message.clone();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            accept_handshake(&mut stream, Some("permessage-deflate")).await;
            // One compressor for the whole connection: the repeat is encoded
            // as a back-reference into the first message.
            let mut compressor = Compress::new(Compression::new(3), false);
            let first = deflate(&mut compressor, message.as_bytes());
            let repeat = deflate(&mut compressor, message.as_bytes());
            assert!(repeat.len() * 4 < first.len());
            for payload in [first, repeat] {
                stream
                    .write_all(&server_frame(OPCODE_TEXT, true, &payload))
                    .await
                    .unwrap();
            }
            wait_for_close(&mut stream).await;
        })
    };

    let client = WsClient::new();
    client
        .connect(&format!("http://{addr}"), None)
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while client
        .count_messages(|m| m.get("seq").and_then(|value| value.as_u64()) == Some(7))
        .await
        < 2
    {
        assert!(Instant::now() < deadline, "both messages must decode");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let expected: sonic_rs::Value = sonic_rs::from_str(&message).unwrap();
    assert!(client
        .get_messages()
        .await
        .iter()
        .all(|received| received == &expected));

    client.disconnect().await;
    timeout(Duration::from_secs(2), server)
        .await
        .expect("server finished")
        .unwrap();
}

/// Accepts one Tungstenite connection, capturing the upgrade request headers.
async fn accept_tungstenite(
    listener: TcpListener,
) -> (
    tokio_tungstenite::WebSocketStream<TcpStream>,
    tungstenite::http::HeaderMap,
) {
    let (stream, _) = listener.accept().await.unwrap();
    let mut captured = None;
    let socket = tokio_tungstenite::accept_hdr_async(
        stream,
        |request: &tungstenite::handshake::server::Request,
         response: tungstenite::handshake::server::Response| {
            captured = Some(request.headers().clone());
            Ok(response)
        },
    )
    .await
    .unwrap();
    (socket, captured.expect("request headers captured"))
}

/// Round-trips a subscribe and a message against a server that never
/// negotiates. Tungstenite fails the connection on any RSV1 frame, so reading
/// the subscribe as text proves the client did not compress.
async fn assert_uncompressed_round_trip(client: WsClient) -> tungstenite::http::HeaderMap {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let message = index_price_update(11);

    let server = {
        let message = message.clone();
        tokio::spawn(async move {
            let (mut socket, headers) = accept_tungstenite(listener).await;
            match timeout(Duration::from_secs(2), socket.next())
                .await
                .expect("timed out waiting for subscribe")
                .expect("client socket ended")
                .expect("client frame is valid without compression")
            {
                tungstenite::Message::Text(text) => {
                    let subscribe: serde_json::Value = serde_json::from_str(&text).unwrap();
                    assert_eq!(subscribe["channel"], "index_prices");
                }
                other => panic!("expected a text subscribe, received {other:?}"),
            }
            socket
                .send(tungstenite::Message::Text(message))
                .await
                .unwrap();
            while let Some(Ok(frame)) = socket.next().await {
                if matches!(frame, tungstenite::Message::Close(_)) {
                    break;
                }
            }
            headers
        })
    };

    client
        .connect(&format!("http://{addr}"), None)
        .await
        .unwrap();
    client.subscribe(vec!["index_prices"]).await.unwrap();
    let received = wait_for_seq(&client, 11).await;
    let expected: sonic_rs::Value = sonic_rs::from_str(&message).unwrap();
    assert_eq!(received, expected);
    client.disconnect().await;

    timeout(Duration::from_secs(2), server)
        .await
        .expect("server finished")
        .unwrap()
}

#[tokio::test]
async fn falls_back_to_uncompressed_when_server_declines() {
    let headers = assert_uncompressed_round_trip(WsClient::new()).await;
    let offer = headers
        .get("sec-websocket-extensions")
        .expect("the client still offers deflate")
        .to_str()
        .unwrap();
    assert!(offer.starts_with("permessage-deflate"), "offer: {offer}");
}

#[tokio::test]
async fn disabled_deflate_sends_no_offer() {
    let headers =
        assert_uncompressed_round_trip(WsClient::new().with_permessage_deflate(false)).await;
    assert!(headers.get("sec-websocket-extensions").is_none());
}

#[tokio::test]
async fn compressed_frame_without_negotiation_fails_the_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        accept_handshake(&mut stream, None).await;
        let payload = deflate_independent(index_price_update(1).as_bytes());
        stream
            .write_all(&server_frame(OPCODE_TEXT, true, &payload))
            .await
            .unwrap();
        // Hold the socket so only the client can end the session.
        let mut rest = Vec::new();
        let _ = timeout(Duration::from_secs(2), stream.read_to_end(&mut rest)).await;
    });

    let client = WsClient::new();
    client
        .connect(&format!("http://{addr}"), None)
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let reason = loop {
        if let Some(reason) = client.last_disconnect_reason() {
            break reason;
        }
        assert!(Instant::now() < deadline, "client must drop the connection");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(
        matches!(reason, WsDisconnectReason::Transport(_)),
        "unexpected reason: {reason:?}"
    );
    assert_eq!(client.count_messages(|_| true).await, 0);

    client.disconnect().await;
    server.await.unwrap();
}

#[tokio::test]
async fn deflate_bomb_fails_the_connection_at_the_message_limit() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // One byte past the 64 MiB message limit, compressed to well under 1 MiB.
    let bomb = deflate(
        &mut Compress::new(Compression::fast(), false),
        &vec![b' '; MAX_MESSAGE_LEN + 1],
    );
    assert!(bomb.len() < 1 << 20);

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        accept_handshake(&mut stream, Some(HYPERCALL_RESPONSE)).await;
        stream
            .write_all(&server_frame(OPCODE_TEXT, true, &bomb))
            .await
            .unwrap();
        let mut rest = Vec::new();
        let _ = timeout(Duration::from_secs(5), stream.read_to_end(&mut rest)).await;
    });

    let client = WsClient::new();
    client
        .connect(&format!("http://{addr}"), None)
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let reason = loop {
        if let Some(reason) = client.last_disconnect_reason() {
            break reason;
        }
        assert!(Instant::now() < deadline, "client must drop the connection");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    match reason {
        WsDisconnectReason::Transport(message) => {
            assert!(
                message.contains("inflated WebSocket message exceeds"),
                "{message}"
            )
        }
        other => panic!("unexpected reason: {other:?}"),
    }
    assert_eq!(client.count_messages(|_| true).await, 0);

    client.disconnect().await;
    server.await.unwrap();
}

#[tokio::test]
async fn answers_a_ping_while_a_backlog_keeps_arriving() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        accept_handshake(&mut stream, Some(HYPERCALL_RESPONSE)).await;
        let (mut reader, mut writer) = stream.into_split();
        let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
        // The ping goes out first, then an inbound stream that never pauses,
        // so a pong queued until the reader goes idle would never be sent.
        let flood = tokio::spawn(async move {
            writer
                .write_all(&server_frame(0x9, false, b"keepalive"))
                .await
                .unwrap();
            let message = index_price_update(1);
            let frame = server_frame(OPCODE_TEXT, true, &deflate_independent(message.as_bytes()));
            let burst = frame.repeat(64);
            while stop_rx.try_recv().is_err() {
                if writer.write_all(&burst).await.is_err() {
                    break;
                }
            }
        });
        let pong = timeout(Duration::from_secs(2), async {
            loop {
                let mut head = [0_u8; 2];
                reader.read_exact(&mut head).await.unwrap();
                let len = usize::from(head[1] & 0x7f);
                assert!(len < 126, "only small client frames are expected");
                let mut mask = [0_u8; 4];
                reader.read_exact(&mut mask).await.unwrap();
                let mut payload = vec![0_u8; len];
                reader.read_exact(&mut payload).await.unwrap();
                for (index, byte) in payload.iter_mut().enumerate() {
                    *byte ^= mask[index % 4];
                }
                if head[0] & 0x0f == 0xA {
                    return payload;
                }
            }
        })
        .await
        .expect("pong must not wait for the backlog to drain");
        assert_eq!(pong, b"keepalive");
        let _ = stop_tx.send(());
        let _ = flood.await;
    });

    let client = WsClient::new();
    client
        .connect(&format!("http://{addr}"), None)
        .await
        .unwrap();
    timeout(Duration::from_secs(5), server)
        .await
        .expect("server finished")
        .unwrap();
    client.disconnect().await;
}

#[tokio::test]
async fn connect_errors_keep_their_source() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let Err(error) = connect_socket(&format!("ws://{addr}/ws"), WsClient::new().options).await
    else {
        panic!("nothing listens on {addr}");
    };
    assert!(matches!(error, ConnectError::Tcp(_)), "{error:?}");
    let source = std::error::Error::source(&error).expect("TCP failure keeps its source");
    assert!(source.downcast_ref::<std::io::Error>().is_some());

    let Err(error) = connect_socket("ws://", WsClient::new().options).await else {
        panic!("a URL without a host cannot connect");
    };
    assert!(
        matches!(error, ConnectError::Url(_) | ConnectError::MissingHost),
        "{error:?}"
    );
}

// ---------------------------------------------------------------------------
// `indicative_market_data_v2` (batched indicative market data)
// ---------------------------------------------------------------------------

fn batch_frame(instruments: &[&str]) -> String {
    let rows = instruments
        .iter()
        .map(|instrument| {
            format!(
                r#"{{"instrument":"{instrument}","best_bid":"10.5","best_ask":"11.25","num_providers":1,"published_at":1767225600000,"timestamp":1767225600000}}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"type":"IndicativeMarketDataBatch","flushed_at":1767225600025,"updates":[{rows}]}}"#
    )
}

async fn read_client_json(stream: &mut TcpStream) -> serde_json::Value {
    let frame = read_client_frame(stream).await;
    assert_eq!(frame.opcode, OPCODE_TEXT);
    serde_json::from_slice(&frame.payload).unwrap()
}

#[test]
fn batched_indicative_is_the_default_and_can_be_disabled() {
    assert!(WsClient::new().indicative_batched());
    assert!(!WsClient::new()
        .with_batched_indicative(false)
        .indicative_batched());
}

#[tokio::test]
async fn subscribes_to_v2_and_decodes_compressed_batches() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let batch = batch_frame(&["BTC-1-C", "BTC-2-C", "ETH-1-C"]);

    let server = {
        let batch = batch.clone();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            accept_handshake(&mut stream, Some(HYPERCALL_RESPONSE)).await;
            let subscribe = read_client_json(&mut stream).await;
            assert_eq!(
                subscribe,
                serde_json::json!({
                    "type": "Subscribe",
                    "channel": "indicative_market_data_v2",
                    "symbols": ["BTC-1-C", "BTC-2-C", "ETH-1-C"]
                })
            );
            let confirmed =
                r#"{"type":"Subscribed","channel":"indicative_market_data_v2","batch":true}"#;
            stream
                .write_all(&server_frame(OPCODE_TEXT, false, confirmed.as_bytes()))
                .await
                .unwrap();
            // Both negotiated: a compressed batch frame on the v2 channel.
            let compressed = deflate_independent(batch.as_bytes());
            stream
                .write_all(&server_frame(OPCODE_TEXT, true, &compressed))
                .await
                .unwrap();
            wait_for_close(&mut stream).await;
        })
    };

    let client = WsClient::new();
    client
        .connect(&format!("http://{addr}"), None)
        .await
        .unwrap();
    client
        .subscribe_indicative_market_data(Some(vec![
            "BTC-1-C".to_string(),
            "BTC-2-C".to_string(),
            "ETH-1-C".to_string(),
        ]))
        .await
        .unwrap();

    let message = client
        .wait_for_message(
            |message| {
                message.get("type").and_then(|value| value.as_str())
                    == Some("IndicativeMarketDataBatch")
            },
            2_000,
        )
        .await
        .expect("the compressed batch was inflated and recorded");
    let rows = crate::indicative::indicative_rows_from_value(&message)
        .unwrap()
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row.instrument.as_str())
            .collect::<Vec<_>>(),
        vec!["BTC-1-C", "BTC-2-C", "ETH-1-C"]
    );
    assert!(client.indicative_batched());

    client.disconnect().await;
    timeout(Duration::from_secs(2), server)
        .await
        .expect("server finished")
        .unwrap();
}

#[tokio::test]
async fn falls_back_to_v1_when_the_server_confirms_v2_without_the_marker() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        accept_handshake(&mut stream, None).await;
        let subscribe = read_client_json(&mut stream).await;
        assert_eq!(subscribe["channel"], "indicative_market_data_v2");
        // What every server without the v2 channel answers.
        let confirmed = r#"{"type":"Subscribed","channel":"indicative_market_data_v2"}"#;
        stream
            .write_all(&server_frame(OPCODE_TEXT, false, confirmed.as_bytes()))
            .await
            .unwrap();

        // The client drops the dead subscription and moves the same filter
        // to the v1 channel.
        assert_eq!(
            read_client_json(&mut stream).await,
            serde_json::json!({
                "type": "Unsubscribe",
                "channel": "indicative_market_data_v2",
                "symbols": ["BTC-1-C"]
            })
        );
        assert_eq!(
            read_client_json(&mut stream).await,
            serde_json::json!({
                "type": "Subscribe",
                "channel": "indicative_market_data",
                "symbols": ["BTC-1-C"]
            })
        );
        // Later indicative subscriptions go straight to v1.
        assert_eq!(
            read_client_json(&mut stream).await["channel"],
            "indicative_market_data"
        );
        let _ = seen_tx.send(());
        wait_for_close(&mut stream).await;
    });

    let client = WsClient::new();
    client
        .connect(&format!("http://{addr}"), None)
        .await
        .unwrap();
    client
        .subscribe_indicative_market_data(Some(vec!["BTC-1-C".to_string()]))
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    while client.indicative_batched() {
        assert!(Instant::now() < deadline, "the client never fell back");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Wait until the fallback rewrote the stored subscription for replay.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let channels: Vec<String> = client
            .shared
            .subscriptions
            .read()
            .await
            .values()
            .map(|desired| desired.channel.clone())
            .collect();
        if channels == vec!["indicative_market_data".to_string()] {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "replay entry not moved: {channels:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    client
        .subscribe_indicative_market_data(Some(vec!["ETH-1-C".to_string()]))
        .await
        .unwrap();
    timeout(Duration::from_secs(2), seen_rx)
        .await
        .unwrap()
        .unwrap();

    client.disconnect().await;
    timeout(Duration::from_secs(2), server)
        .await
        .expect("server finished")
        .unwrap();
}

#[tokio::test]
async fn falls_back_to_v1_when_the_server_rejects_v2_as_unknown() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        accept_handshake(&mut stream, None).await;
        let subscribe = read_client_json(&mut stream).await;
        assert_eq!(subscribe["channel"], "indicative_market_data_v2");
        // A server that rejects unknown channels but lacks v2.
        let confirmed = r#"{"type":"Error","message":"Unknown channel 'indicative_market_data_v2'","code":"unknown_channel","channel":"indicative_market_data_v2"}"#;
        stream
            .write_all(&server_frame(OPCODE_TEXT, false, confirmed.as_bytes()))
            .await
            .unwrap();

        // The client drops the dead subscription and moves the same filter
        // to the v1 channel.
        assert_eq!(
            read_client_json(&mut stream).await,
            serde_json::json!({
                "type": "Unsubscribe",
                "channel": "indicative_market_data_v2",
                "symbols": ["BTC-1-C"]
            })
        );
        assert_eq!(
            read_client_json(&mut stream).await,
            serde_json::json!({
                "type": "Subscribe",
                "channel": "indicative_market_data",
                "symbols": ["BTC-1-C"]
            })
        );
        // Later indicative subscriptions go straight to v1.
        assert_eq!(
            read_client_json(&mut stream).await["channel"],
            "indicative_market_data"
        );
        let _ = seen_tx.send(());
        wait_for_close(&mut stream).await;
    });

    let client = WsClient::new();
    client
        .connect(&format!("http://{addr}"), None)
        .await
        .unwrap();
    client
        .subscribe_indicative_market_data(Some(vec!["BTC-1-C".to_string()]))
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    while client.indicative_batched() {
        assert!(Instant::now() < deadline, "the client never fell back");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Wait until the fallback rewrote the stored subscription for replay.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let channels: Vec<String> = client
            .shared
            .subscriptions
            .read()
            .await
            .values()
            .map(|desired| desired.channel.clone())
            .collect();
        if channels == vec!["indicative_market_data".to_string()] {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "replay entry not moved: {channels:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    client
        .subscribe_indicative_market_data(Some(vec!["ETH-1-C".to_string()]))
        .await
        .unwrap();
    timeout(Duration::from_secs(2), seen_rx)
        .await
        .unwrap()
        .unwrap();

    client.disconnect().await;
    timeout(Duration::from_secs(2), server)
        .await
        .expect("server finished")
        .unwrap();
}

#[tokio::test]
async fn opting_out_subscribes_to_v1() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        accept_handshake(&mut stream, None).await;
        assert_eq!(
            read_client_json(&mut stream).await,
            serde_json::json!({"type": "Subscribe", "channel": "indicative_market_data"})
        );
        let _ = seen_tx.send(());
        wait_for_close(&mut stream).await;
    });

    let client = WsClient::new().with_batched_indicative(false);
    client
        .connect(&format!("http://{addr}"), None)
        .await
        .unwrap();
    client.subscribe_indicative_market_data(None).await.unwrap();
    // Disconnecting races queued sends, so wait until the server saw it.
    timeout(Duration::from_secs(2), seen_rx)
        .await
        .unwrap()
        .unwrap();
    client.disconnect().await;
    timeout(Duration::from_secs(2), server)
        .await
        .expect("server finished")
        .unwrap();
}
