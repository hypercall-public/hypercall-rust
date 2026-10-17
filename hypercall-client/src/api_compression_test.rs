use std::io::Write;

use flate2::{write::GzEncoder, Compression};
use tokio::io::AsyncWriteExt;

use super::*;
use crate::test_support::read_http_request;

// The public API gzips responses at its edge when the client asks for it.
// The client must ask, and must hand callers the decoded body.
#[tokio::test]
async fn requests_gzip_and_decodes_compressed_responses() {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(br#"{"success":true,"data":[]}"#).unwrap();
    let compressed = encoder.finish().unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_http_request(&mut stream).await;
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            compressed.len()
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        stream.write_all(&compressed).await.unwrap();
        request
    });

    let markets = HypercallClient::new(base_url).get_markets().await.unwrap();
    let request = server.await.unwrap().to_ascii_lowercase();

    assert!(markets.is_empty());
    assert!(
        request
            .lines()
            .any(|line| line.starts_with("accept-encoding:") && line.contains("gzip")),
        "client did not advertise gzip: {request}"
    );
}
