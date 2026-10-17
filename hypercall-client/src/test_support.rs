use std::{
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use alloy::{
    rpc::{client::RpcClient, json_rpc::RequestPacket},
    transports::{
        mock::{Asserter, MockTransport},
        TransportError, TransportFut,
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tower::Service;

use crate::NonceProvider;

pub(crate) struct FixedNonce(pub(crate) u64);

#[derive(Clone, Default)]
pub(crate) struct RecordedRequests(Arc<Mutex<Vec<RequestPacket>>>);

impl RecordedRequests {
    pub(crate) fn take_json(&self) -> Vec<serde_json::Value> {
        std::mem::take(&mut *self.0.lock().unwrap())
            .into_iter()
            .flat_map(|packet| match packet {
                RequestPacket::Single(request) => vec![request],
                RequestPacket::Batch(requests) => requests,
            })
            .map(|request| serde_json::to_value(request).unwrap())
            .collect()
    }
}

#[derive(Clone)]
struct RecordingTransport {
    inner: MockTransport,
    requests: RecordedRequests,
}

impl Service<RequestPacket> for RecordingTransport {
    type Response = alloy::rpc::json_rpc::ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: RequestPacket) -> Self::Future {
        self.requests.0.lock().unwrap().push(request.clone());
        self.inner.call(request)
    }
}

impl NonceProvider for FixedNonce {
    fn next_nonce(&self) -> u64 {
        self.0
    }

    fn current_nonce(&self) -> u64 {
        self.0
    }
}

pub(crate) fn recording_rpc_client(asserter: Asserter) -> (RpcClient, RecordedRequests) {
    let requests = RecordedRequests::default();
    let transport = RecordingTransport {
        inner: MockTransport::new(asserter),
        requests: requests.clone(),
    };
    (RpcClient::new(transport, true), requests)
}

pub(crate) async fn serve_once(response_body: impl Into<String>) -> (String, JoinHandle<String>) {
    serve_once_with_status("200 OK", response_body).await
}

pub(crate) async fn serve_once_with_status(
    status: &'static str,
    response_body: impl Into<String>,
) -> (String, JoinHandle<String>) {
    let listener = bind_listener().await;
    let address = listener.local_addr().unwrap();
    let response_body = response_body.into();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        exchange_http_response(stream, status, &response_body).await
    });
    (format!("http://{address}"), task)
}

pub(crate) async fn serve_once_then_without_response(
    response_body: impl Into<String>,
) -> (String, JoinHandle<Vec<String>>) {
    let listener = bind_listener().await;
    let address = listener.local_addr().unwrap();
    let response_body = response_body.into();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let first = exchange_http_response(stream, "200 OK", &response_body).await;
        let (mut stream, _) = listener.accept().await.unwrap();
        let second = read_http_request(&mut stream).await;
        vec![first, second]
    });
    (format!("http://{address}"), task)
}

pub(crate) async fn serve_sequence(
    response_bodies: Vec<String>,
) -> (String, JoinHandle<Vec<String>>) {
    serve_status_sequence(
        response_bodies
            .into_iter()
            .map(|body| ("200 OK", body))
            .collect(),
    )
    .await
}

pub(crate) async fn serve_status_sequence(
    responses: Vec<(&'static str, String)>,
) -> (String, JoinHandle<Vec<String>>) {
    let listener = bind_listener().await;
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::with_capacity(responses.len());
        for (status, response_body) in responses {
            let (stream, _) = listener.accept().await.unwrap();
            requests.push(exchange_http_response(stream, status, &response_body).await);
        }
        requests
    });
    (format!("http://{address}"), task)
}

pub(crate) async fn read_http_request(stream: &mut TcpStream) -> String {
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
    String::from_utf8(request).unwrap()
}

pub(crate) fn json_body(request: &str) -> serde_json::Value {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("captured HTTP request contains a body separator");
    serde_json::from_str(body).expect("captured HTTP request body is JSON")
}

async fn bind_listener() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").await.unwrap()
}

async fn exchange_http_response(
    mut stream: TcpStream,
    status: &str,
    response_body: &str,
) -> String {
    let request = read_http_request(&mut stream).await;
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response_body}",
        response_body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
    request
}
