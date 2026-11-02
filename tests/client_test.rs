use snaptranslate_linux::client::{ClientError, DeepSeekClient, RecoveryHint};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

#[tokio::test]
async fn test_validation_missing_or_invalid_api_key() {
    let client = DeepSeekClient::default();
    let (tx, _rx) = mpsc::channel(10);

    // Empty key
    let err = client
        .stream_translation(
            "",
            "deepseek-flash",
            "data:image/png;base64,AAAA",
            "中文",
            tx.clone(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::MissingApiKey));

    // Whitespace key
    let err = client
        .stream_translation(
            "   ",
            "deepseek-flash",
            "data:image/png;base64,AAAA",
            "中文",
            tx.clone(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::MissingApiKey));

    // Non-ascii / control characters
    let err = client
        .stream_translation(
            "sk-test\x00key",
            "deepseek-flash",
            "data:image/png;base64,AAAA",
            "中文",
            tx.clone(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::InvalidApiKey));
}

#[tokio::test]
async fn test_validation_empty_or_oversized_image() {
    let client = DeepSeekClient::default();
    let (tx, _rx) = mpsc::channel(10);

    // Empty image
    let err = client
        .stream_translation(
            "sk-validkey12345678",
            "deepseek-flash",
            "",
            "中文",
            tx.clone(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::EmptyImage));

    // Oversized image (greater than 14 MiB)
    let huge_data = "a".repeat(15 * 1024 * 1024);
    let err = client
        .stream_translation(
            "sk-validkey12345678",
            "deepseek-flash",
            &huge_data,
            "中文",
            tx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::ImageTooLarge));
}

#[tokio::test]
async fn test_mock_sse_streaming_successful_roundtrip() {
    // Spin up a local mock HTTP server
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_url = format!("http://{}/v1/chat/completions", addr);

    tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;

            let response_body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\" \"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"World!\"}}]}\n\n",
                "data: [DONE]\n\n"
            );

            let http_response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
                response_body.len(),
                response_body
            );

            let _ = socket.write_all(http_response.as_bytes()).await;
        }
    });

    let client = DeepSeekClient::new(server_url);
    let (tx, mut rx) = mpsc::channel(10);

    let handle = tokio::spawn(async move {
        let mut tokens = Vec::new();
        while let Some(token) = rx.recv().await {
            tokens.push(token);
        }
        tokens
    });

    let result = client
        .stream_translation(
            "sk-test-valid-key",
            "deepseek-flash",
            "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
            "简体中文",
            tx,
        )
        .await
        .expect("Stream translation should succeed");

    assert_eq!(result, "Hello World!");

    let received_tokens = handle.await.unwrap();
    assert_eq!(received_tokens, vec!["Hello", " ", "World!"]);
}

#[tokio::test]
async fn test_mock_status_code_error_mappings() {
    async fn test_code(code: u16, expected: fn(&ClientError) -> bool) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_url = format!("http://{}/chat", addr);

        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                let http_response = format!(
                    "HTTP/1.1 {} Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    code
                );
                let _ = socket.write_all(http_response.as_bytes()).await;
            }
        });

        let client = DeepSeekClient::new(server_url);
        let (tx, _rx) = mpsc::channel(10);
        let err = client
            .stream_translation(
                "sk-test-key",
                "deepseek-flash",
                "data:image/png;base64,AAAA",
                "中文",
                tx,
            )
            .await
            .unwrap_err();

        assert!(expected(&err), "Expected error mapping for HTTP {}", code);
    }

    test_code(401, |e| matches!(e, ClientError::Unauthorized)).await;
    test_code(403, |e| matches!(e, ClientError::Forbidden)).await;
    test_code(402, |e| matches!(e, ClientError::InsufficientBalance)).await;
    test_code(400, |e| matches!(e, ClientError::ServerUnavailable(400))).await;
    test_code(404, |e| matches!(e, ClientError::ServerUnavailable(404))).await;
    test_code(413, |e| matches!(e, ClientError::ImageTooLarge)).await;
    test_code(429, |e| matches!(e, ClientError::RateLimited)).await;
    test_code(500, |e| matches!(e, ClientError::ServerUnavailable(500))).await;
}

#[test]
fn test_http_failures_choose_actionable_recovery_without_blame_or_payloads() {
    for (error, hint, status) in [
        (
            ClientError::Unauthorized,
            RecoveryHint::Credentials,
            "HTTP 401",
        ),
        (ClientError::Forbidden, RecoveryHint::Service, "HTTP 403"),
        (
            ClientError::ServerUnavailable(400),
            RecoveryHint::Service,
            "HTTP 400",
        ),
        (
            ClientError::ServerUnavailable(404),
            RecoveryHint::Service,
            "HTTP 404",
        ),
        (
            ClientError::InsufficientBalance,
            RecoveryHint::Retry,
            "HTTP 402",
        ),
        (ClientError::RateLimited, RecoveryHint::Retry, "HTTP 429"),
        (
            ClientError::ServerUnavailable(503),
            RecoveryHint::Retry,
            "HTTP 503",
        ),
    ] {
        let failure = error.failure();
        assert_eq!(failure.recovery, hint);
        assert!(failure.message.contains(status));
        assert_eq!(failure.message, error.user_message());
    }
    assert!(ClientError::Forbidden
        .failure()
        .message
        .contains("不一定是密钥无效"));
    let failure = ClientError::ParseError("sk-private-response-from-server".into()).failure();
    assert_eq!(failure.recovery, RecoveryHint::Service);
    assert!(!failure.message.contains("sk-private"));
}

#[test]
fn test_retry_keeps_original_image_but_image_failures_require_recapture() {
    for error in [
        ClientError::Timeout,
        ClientError::IncompleteStream,
        ClientError::Cancelled,
    ] {
        assert_eq!(error.failure().recovery, RecoveryHint::Retry);
    }
    for error in [
        ClientError::EmptyImage,
        ClientError::ImageTooLarge,
        ClientError::EmptyTranslation,
        ClientError::OutputTruncated,
        ClientError::ContentFiltered,
    ] {
        assert_eq!(error.failure().recovery, RecoveryHint::Recapture);
    }
    for error in [ClientError::MissingApiKey, ClientError::InvalidApiKey] {
        assert_eq!(error.failure().recovery, RecoveryHint::Credentials);
    }
}

#[test]
fn test_invalid_request_url_goes_to_service_settings_without_echoing_secret() {
    let error = reqwest::Client::new()
        .get("invalid-url?key=sk-private-query")
        .build()
        .unwrap_err();
    let failure = ClientError::Http(error).failure();
    assert_eq!(failure.recovery, RecoveryHint::Service);
    assert!(failure.message.contains("API 基础地址"));
    assert!(!failure.message.contains("sk-private"));
    assert!(!failure.message.contains("invalid-url"));
}

struct MockStream {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockStream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) {
    let mut request = Vec::new();
    let mut bytes = [0; 4096];
    loop {
        let count = socket.read(&mut bytes).await.unwrap();
        assert_ne!(count, 0);
        request.extend_from_slice(&bytes[..count]);
        if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&request[..end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if request.len() >= end + 4 + length {
                return;
            }
        }
    }
}

async fn mock_stream(parts: Vec<Vec<u8>>, keep_open: bool) -> MockStream {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/chat", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
        for part in parts {
            socket
                .write_all(format!("{:x}\r\n", part.len()).as_bytes())
                .await
                .unwrap();
            socket.write_all(&part).await.unwrap();
            socket.write_all(b"\r\n").await.unwrap();
            socket.flush().await.unwrap();
            // Allow the client to observe each deliberately split HTTP chunk.
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if keep_open {
            std::future::pending::<()>().await;
        } else {
            socket.write_all(b"0\r\n\r\n").await.unwrap();
        }
    });
    MockStream { url, task }
}

async fn translate_from(url: &str, tx: mpsc::Sender<String>) -> Result<String, ClientError> {
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    DeepSeekClient::with_client(http, url)
        .stream_translation(
            "sk-test-key",
            "deepseek-flash",
            "data:image/png;base64,AAAA",
            "中文",
            tx,
        )
        .await
}

#[tokio::test]
async fn test_utf8_split_across_chunks_and_multiline_sse() {
    let body = concat!(
        ": heartbeat\r\n\r\n",
        "data: {\"choices\":\r\n",
        "data: [{\"delta\":{\"content\":\"你好🌍\"}}]}\r\n\r\n",
        "data: [DONE]\r\n\r\n"
    )
    .as_bytes();
    let chinese = body
        .windows(3)
        .position(|bytes| bytes == "你".as_bytes())
        .unwrap();
    let server = mock_stream(
        vec![
            body[..chinese + 1].to_vec(),
            body[chinese + 1..chinese + 2].to_vec(),
            body[chinese + 2..].to_vec(),
        ],
        false,
    )
    .await;
    let (tx, mut rx) = mpsc::channel(10);
    let result = translate_from(&server.url, tx).await.unwrap();
    assert_eq!(result, "你好🌍");
    assert_eq!(rx.recv().await.as_deref(), Some("你好🌍"));
}

#[tokio::test]
async fn test_done_finishes_without_waiting_for_connection_close() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"完成\"}}]}\n\n",
        "data: [DONE]\n\n",
        "data: invalid trailing response\n\n"
    );
    let server = mock_stream(vec![body.as_bytes().to_vec()], true).await;
    let (tx, _rx) = mpsc::channel(10);
    let result = tokio::time::timeout(Duration::from_secs(1), translate_from(&server.url, tx))
        .await
        .expect("DONE must finish even while HTTP remains open")
        .unwrap();
    assert_eq!(result, "完成");
}

#[tokio::test]
async fn test_eof_without_done_keeps_partial_translation_but_reports_failure() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"部分译文\"},\"finish_reason\":\"stop\"}]}\n\n";
    let server = mock_stream(vec![body.as_bytes().to_vec()], false).await;
    let (tx, mut rx) = mpsc::channel(10);
    let error = translate_from(&server.url, tx).await.unwrap_err();
    assert!(matches!(error, ClientError::IncompleteStream));
    assert_eq!(rx.recv().await.as_deref(), Some("部分译文"));
}

#[tokio::test]
async fn test_done_without_nonblank_translation_is_not_success() {
    for body in [
        "data: [DONE]\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"  \"}}]}\n\ndata: [DONE]\n\n",
    ] {
        let server = mock_stream(vec![body.as_bytes().to_vec()], false).await;
        let (tx, _rx) = mpsc::channel(10);
        assert!(matches!(
            translate_from(&server.url, tx).await,
            Err(ClientError::EmptyTranslation)
        ));
    }
}

#[tokio::test]
async fn test_truncated_and_filtered_finishes_preserve_received_text() {
    for reason in ["length", "content_filter"] {
        let body = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"部分译文\"}},\"finish_reason\":\"{reason}\"}}]}}\n\ndata: [DONE]\n\n");
        let server = mock_stream(vec![body.into_bytes()], false).await;
        let (tx, mut rx) = mpsc::channel(10);
        let error = translate_from(&server.url, tx).await.unwrap_err();
        match reason {
            "length" => assert!(matches!(error, ClientError::OutputTruncated)),
            _ => assert!(matches!(error, ClientError::ContentFiltered)),
        }
        assert_eq!(rx.recv().await.as_deref(), Some("部分译文"));
    }
}

#[tokio::test]
async fn test_malformed_response_is_reported_without_echoing_payload() {
    let server = mock_stream(
        vec![b"data: secret-sk-sensitive-server-payload\n\n".to_vec()],
        false,
    )
    .await;
    let (tx, _rx) = mpsc::channel(10);
    let error = translate_from(&server.url, tx).await.unwrap_err();
    assert!(matches!(error, ClientError::ParseError(_)));
    assert!(!error.to_string().contains("secret-sk"));
    assert!(!error.user_message().contains("secret-sk"));
    assert!(!ClientError::ParseError("private response".into())
        .user_message()
        .contains("private response"));
}

#[tokio::test]
async fn test_receiver_close_cancels_while_waiting_for_more_stream_data() {
    let body = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n";
    let server = mock_stream(vec![body.to_vec()], true).await;
    let (tx, mut rx) = mpsc::channel(10);
    let url = server.url.clone();
    let request = tokio::spawn(async move { translate_from(&url, tx).await });
    assert_eq!(rx.recv().await.as_deref(), Some("Hello"));
    drop(rx);
    let error = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .expect("closing the receiver must cancel a stalled stream")
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, ClientError::Cancelled));
}

#[tokio::test]
async fn test_receiver_close_cancels_while_waiting_for_response_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/chat", listener.local_addr().unwrap());
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let server = MockStream {
        url: url.clone(),
        task: tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            ready_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        }),
    };
    let (tx, rx) = mpsc::channel(10);
    let request = tokio::spawn(async move { translate_from(&url, tx).await });
    tokio::time::timeout(Duration::from_secs(1), ready_rx)
        .await
        .unwrap()
        .unwrap();
    drop(rx);
    let error = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .expect("closing the receiver must cancel while awaiting headers")
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, ClientError::Cancelled));
    drop(server);
}
