use super::*;
use flate2::{Compress, Compression, FlushCompress};
use std::io::Cursor;
use tokio::io::AsyncReadExt;

const HEAD_PLAIN: &str = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\n";
const HEAD_DEFLATE: &str = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
     Sec-WebSocket-Extensions: permessage-deflate; server_no_context_takeover\r\n\r\n";
const TEXT: u8 = 0x1;
const PING: u8 = 0x9;

fn deflate_with(compressor: &mut Compress, message: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(message.len() + 1024);
    compressor
        .compress_vec(message, &mut out, FlushCompress::Sync)
        .unwrap();
    assert!(out.ends_with(&DEFLATE_TAIL));
    out.truncate(out.len() - DEFLATE_TAIL.len());
    out
}

fn deflate(message: &[u8]) -> Vec<u8> {
    deflate_with(&mut Compress::new(Compression::new(3), false), message)
}

fn server_frame(first: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = frame_header(first, payload.len());
    frame.extend_from_slice(payload);
    frame
}

async fn read_all(
    wire: Vec<u8>,
    offered: bool,
    max_frame: usize,
    max_message: usize,
) -> io::Result<Vec<u8>> {
    let mut stream = InflatingStream::new(Cursor::new(wire), offered, max_frame, max_message);
    let mut out = Vec::new();
    stream.read_to_end(&mut out).await.map(|_| out)
}

fn with_head(head: &str, frames: &[Vec<u8>]) -> Vec<u8> {
    let mut wire = head.as_bytes().to_vec();
    for frame in frames {
        wire.extend_from_slice(frame);
    }
    wire
}

#[test]
fn parses_the_hypercall_response() {
    let parsed = parse_response_extensions(HEAD_DEFLATE, true).unwrap();
    assert_eq!(
        parsed,
        Some(Negotiated {
            server_no_context_takeover: true
        })
    );
    let with_all = "HTTP/1.1 101 x\r\nsec-websocket-extensions: permessage-deflate; \
                    server_no_context_takeover; client_no_context_takeover; server_max_window_bits=15\r\n\r\n";
    assert!(parse_response_extensions(with_all, true).unwrap().is_some());
    let quoted = "HTTP/1.1 101 x\r\nSec-WebSocket-Extensions: permessage-deflate; \
                  server_max_window_bits=\"12\"\r\n\r\n";
    assert!(parse_response_extensions(quoted, true).unwrap().is_some());
    assert_eq!(parse_response_extensions(HEAD_PLAIN, true).unwrap(), None);
    assert_eq!(parse_response_extensions(HEAD_PLAIN, false).unwrap(), None);
}

#[test]
fn rejects_extensions_the_client_did_not_offer_or_cannot_honor() {
    // Accepted without being offered.
    assert!(parse_response_extensions(HEAD_DEFLATE, false).is_err());
    for extensions in [
        "x-webkit-deflate-frame",
        "permessage-deflate; unknown_param",
        "permessage-deflate; server_no_context_takeover; server_no_context_takeover",
        "permessage-deflate; server_max_window_bits=20",
        "permessage-deflate; server_max_window_bits",
        "permessage-deflate, permessage-deflate",
        // Only valid when the client offered it, and it never does.
        "permessage-deflate; client_max_window_bits=15",
        // Unbalanced or malformed quoting.
        "permessage-deflate; server_max_window_bits=\"15",
        "permessage-deflate; server_max_window_bits=15\"",
        "permessage-deflate; server_max_window_bits=\"\"",
    ] {
        let head = format!("HTTP/1.1 101 x\r\nSec-WebSocket-Extensions: {extensions}\r\n\r\n");
        assert!(
            parse_response_extensions(&head, true).is_err(),
            "{extensions} must fail the handshake"
        );
    }
}

#[tokio::test]
async fn passes_bytes_through_when_not_negotiated() {
    let wire = with_head(HEAD_PLAIN, &[server_frame(0x80 | TEXT, b"{\"a\":1}")]);
    let out = read_all(wire.clone(), true, 1 << 20, 1 << 20)
        .await
        .unwrap();
    assert_eq!(out, wire);
}

#[tokio::test]
async fn rewrites_compressed_messages_as_plain_frames() {
    let message = br#"{"type":"IndexPriceUpdate","prices":[1,2,3,4,5,6,7,8,9,10,11,12]}"#;
    let plain = server_frame(0x80 | TEXT, b"{\"small\":true}");
    let wire = with_head(
        HEAD_DEFLATE,
        &[
            server_frame(0x80 | RSV1 | TEXT, &deflate(message)),
            plain.clone(),
            server_frame(0x80 | RSV1 | TEXT, &deflate(message)),
        ],
    );
    let out = read_all(wire, true, 1 << 20, 1 << 20).await.unwrap();
    let expected = with_head(
        HEAD_DEFLATE,
        &[
            server_frame(0x80 | TEXT, message),
            plain,
            server_frame(0x80 | TEXT, message),
        ],
    );
    assert_eq!(out, expected);
}

#[tokio::test]
async fn keeps_the_window_when_the_server_uses_context_takeover() {
    let head = "HTTP/1.1 101 x\r\nSec-WebSocket-Extensions: permessage-deflate\r\n\r\n";
    let message = br#"{"type":"IndicativeMarketData","instrument":"BTC-20261231-100000-C"}"#;
    let mut compressor = Compress::new(Compression::new(3), false);
    let first = deflate_with(&mut compressor, message);
    let repeat = deflate_with(&mut compressor, message);
    assert!(repeat.len() < first.len());
    let wire = with_head(
        head,
        &[
            server_frame(0x80 | RSV1 | TEXT, &first),
            server_frame(0x80 | RSV1 | TEXT, &repeat),
        ],
    );
    let out = read_all(wire, true, 1 << 20, 1 << 20).await.unwrap();
    let frame = server_frame(0x80 | TEXT, message);
    assert_eq!(out, with_head(head, &[frame.clone(), frame]));
}

#[tokio::test]
async fn assembles_fragmented_compressed_messages_around_control_frames() {
    let message = vec![b'a'; 4096];
    let compressed = deflate(&message);
    let (left, right) = compressed.split_at(compressed.len() / 2);
    let ping = server_frame(0x80 | PING, b"hb");
    let wire = with_head(
        HEAD_DEFLATE,
        &[
            server_frame(RSV1 | TEXT, left),
            ping.clone(),
            server_frame(0x80, right),
        ],
    );
    let out = read_all(wire, true, 1 << 20, 1 << 20).await.unwrap();
    assert_eq!(
        out,
        with_head(HEAD_DEFLATE, &[ping, server_frame(0x80 | TEXT, &message)])
    );
}

#[tokio::test]
async fn reframes_inflated_messages_within_the_frame_limit() {
    let message = vec![b'z'; 2_500];
    let wire = with_head(
        HEAD_DEFLATE,
        &[server_frame(0x80 | RSV1 | TEXT, &deflate(&message))],
    );
    let out = read_all(wire, true, 1_000, 1 << 20).await.unwrap();
    let expected = with_head(
        HEAD_DEFLATE,
        &[
            server_frame(TEXT, &message[..1_000]),
            server_frame(0x0, &message[1_000..2_000]),
            server_frame(0x80, &message[2_000..]),
        ],
    );
    assert_eq!(out, expected);
}

#[tokio::test]
async fn rejects_a_deflate_bomb_before_inflating_past_the_limit() {
    const LIMIT: usize = 1 << 20;
    // 64 MiB of zeros compresses to about 64 KiB.
    let bomb = deflate(&vec![0_u8; 64 << 20]);
    assert!(bomb.len() < 128 << 10);
    let wire = with_head(HEAD_DEFLATE, &[server_frame(0x80 | RSV1 | TEXT, &bomb)]);
    let error = read_all(wire, true, 16 << 20, LIMIT).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("exceeds"), "{error}");
}

#[tokio::test]
async fn accepts_a_message_exactly_at_the_limit() {
    const LIMIT: usize = 256 << 10;
    let message = vec![0_u8; LIMIT];
    let wire = with_head(
        HEAD_DEFLATE,
        &[server_frame(0x80 | RSV1 | TEXT, &deflate(&message))],
    );
    let out = read_all(wire, true, LIMIT, LIMIT).await.unwrap();
    assert_eq!(
        out,
        with_head(HEAD_DEFLATE, &[server_frame(0x80 | TEXT, &message)])
    );

    let over = vec![0_u8; LIMIT + 1];
    let wire = with_head(
        HEAD_DEFLATE,
        &[server_frame(0x80 | RSV1 | TEXT, &deflate(&over))],
    );
    assert!(read_all(wire, true, LIMIT, LIMIT).await.is_err());
}

#[tokio::test]
async fn rejects_malformed_compressed_frames() {
    let cases = [
        // RSV1 on a control frame.
        vec![server_frame(0x80 | RSV1 | PING, b"x")],
        // RSV1 on a continuation frame.
        vec![server_frame(TEXT, b"{"), server_frame(0x80 | RSV1, b"}")],
        // Compressed bytes that are not DEFLATE.
        vec![server_frame(0x80 | RSV1 | TEXT, &[0xff; 16])],
        // Valid DEFLATE, but RSV2 or RSV3 set alongside RSV1. The rewritten
        // frame would drop the bit, so it must be rejected here.
        vec![server_frame(
            0x80 | RSV1 | 0x20 | TEXT,
            &deflate(b"{\"a\":1}"),
        )],
        vec![server_frame(
            0x80 | RSV1 | 0x10 | TEXT,
            &deflate(b"{\"a\":1}"),
        )],
        // RSV2 on the continuation of a compressed message.
        {
            let compressed = deflate(b"{\"a\":1}");
            let (left, right) = compressed.split_at(compressed.len() / 2);
            vec![
                server_frame(RSV1 | TEXT, left),
                server_frame(0x80 | 0x20, right),
            ]
        },
    ];
    for frames in cases {
        let wire = with_head(HEAD_DEFLATE, &frames);
        assert!(read_all(wire, true, 1 << 20, 1 << 20).await.is_err());
    }

    let mut masked = frame_header(0x80 | RSV1 | TEXT, 4);
    masked[1] |= MASKED;
    masked.extend_from_slice(&[0, 0, 0, 0, 1, 2, 3, 4]);
    let wire = with_head(HEAD_DEFLATE, &[masked]);
    assert!(read_all(wire, true, 1 << 20, 1 << 20).await.is_err());
}

#[tokio::test]
async fn handshake_head_split_across_reads_is_handled() {
    let message = br#"{"n":1}"#;
    let wire = with_head(
        HEAD_DEFLATE,
        &[server_frame(0x80 | RSV1 | TEXT, &deflate(message))],
    );
    let (mut client, mut server) = tokio::io::duplex(8);
    let writer = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        for byte in wire {
            server.write_all(&[byte]).await.unwrap();
        }
    });
    let mut stream = InflatingStream::new(&mut client, true, 1 << 20, 1 << 20);
    let mut out = Vec::new();
    let expected = with_head(HEAD_DEFLATE, &[server_frame(0x80 | TEXT, message)]);
    let mut chunk = [0_u8; 3];
    while out.len() < expected.len() {
        let read = stream.read(&mut chunk).await.unwrap();
        assert_ne!(read, 0);
        out.extend_from_slice(&chunk[..read]);
    }
    assert!(stream.deflate_negotiated());
    assert_eq!(out, expected);
    writer.await.unwrap();
}
