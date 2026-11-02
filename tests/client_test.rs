use snaptranslate_linux::client::{ClientError, DeepSeekClient};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

#[tokio::test]
async fn test_validation_missing_or_invalid_api_key() {
    let client = DeepSeekClient::default();
    let (tx, _rx) = mpsc::channel(10);

    // Empty key
    let err = client
        .stream_translation("", "deepseek-flash", "data:image/png;base64,AAAA", "中文", tx.clone())
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::MissingApiKey));

    // Whitespace key
    let err = client
        .stream_translation("   ", "deepseek-flash", "data:image/png;base64,AAAA", "中文", tx.clone())
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::MissingApiKey));

    // Non-ascii / control characters
    let err = client
        .stream_translation("sk-test\x00key", "deepseek-flash", "data:image/png;base64,AAAA", "中文", tx.clone())
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
        .stream_translation("sk-validkey12345678", "deepseek-flash", "", "中文", tx.clone())
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::EmptyImage));

    // Oversized image (greater than 14 MiB)
    let huge_data = "a".repeat(15 * 1024 * 1024);
    let err = client
        .stream_translation("sk-validkey12345678", "deepseek-flash", &huge_data, "中文", tx)
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
            .stream_translation("sk-test-key", "deepseek-flash", "data:image/png;base64,AAAA", "中文", tx)
            .await
            .unwrap_err();

        assert!(expected(&err), "Expected error mapping for HTTP {}", code);
    }

    test_code(401, |e| matches!(e, ClientError::Unauthorized)).await;
    test_code(402, |e| matches!(e, ClientError::InsufficientBalance)).await;
    test_code(429, |e| matches!(e, ClientError::RateLimited)).await;
    test_code(500, |e| matches!(e, ClientError::ServerUnavailable(500))).await;
}
