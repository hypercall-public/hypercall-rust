use std::time::Duration;

use tokio::{io::AsyncWriteExt, net::TcpListener, sync::oneshot, time::timeout};

use super::*;
use crate::test_support::read_http_request;

#[tokio::test]
async fn rfq_posts_do_not_follow_cross_origin_redirects() {
    let redirect_target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redirect_target_url = format!("http://{}", redirect_target.local_addr().unwrap());
    let (target_seen_tx, target_seen_rx) = oneshot::channel();
    let target_task = tokio::spawn(async move {
        let (mut stream, _) = redirect_target.accept().await.unwrap();
        let request = read_http_request(&mut stream).await;
        let _ = target_seen_tx.send(request);
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });

    let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin_url = format!("http://{}", origin.local_addr().unwrap());
    let origin_task = tokio::spawn(async move {
        let (mut stream, _) = origin.accept().await.unwrap();
        let request = read_http_request(&mut stream).await;
        assert!(request.starts_with("POST /rfq/request HTTP/1.1\r\n"));
        let response = format!(
            "HTTP/1.1 302 Found\r\nLocation: {redirect_target_url}/captured\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let response = HypercallClient::new(origin_url)
        .raw_post(
            "/rfq/request",
            &sonic_rs::json!({"signature": "must-not-reach-another-origin"}),
        )
        .await
        .unwrap();
    let target_request = timeout(Duration::from_millis(100), target_seen_rx).await;
    target_task.abort();
    origin_task.await.unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::FOUND);
    assert!(
        target_request.is_err(),
        "cross-origin redirect received the RFQ request"
    );
}
