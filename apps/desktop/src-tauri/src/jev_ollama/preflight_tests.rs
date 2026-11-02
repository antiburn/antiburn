use super::*;
use antiburn_local::analysis::jev::JevQuestion;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn capabilities() -> ModelCapabilities {
    ModelCapabilities {
        model: "selected:latest".into(),
        model_revision: Some("discovered-digest".into()),
        ..ModelCapabilities::jev_default()
    }
}

fn request(capabilities: &ModelCapabilities) -> JevRequest {
    JevRequest {
        model: capabilities.model.clone(),
        state: json!("Synthetic evidence must not reach a changed model."),
        questions: BTreeMap::from([(
            "q".into(),
            JevQuestion::Noul {
                instructions: json!("Does this apply?"),
                criteria: None,
            },
        )]),
    }
}

async fn accept_request(
    listener: &tokio::net::TcpListener,
) -> (tokio::net::TcpStream, String, Vec<u8>) {
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut request = Vec::new();
    loop {
        let mut buffer = [0; 4096];
        let count = socket.read(&mut buffer).await.unwrap();
        assert!(count > 0);
        request.extend_from_slice(&buffer[..count]);
        assert!(request.len() < 8192);
        if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
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
                return (
                    socket,
                    headers.into_owned(),
                    request[end + 4..end + 4 + length].to_vec(),
                );
            }
        }
    }
}

async fn accept_tags(listener: &tokio::net::TcpListener) -> tokio::net::TcpStream {
    let (socket, headers, body) = accept_request(listener).await;
    assert!(headers.starts_with("GET /api/tags HTTP/1.1\r\n"));
    assert!(body.is_empty());
    assert!(!headers.contains("Synthetic evidence"));
    socket
}

#[tokio::test]
async fn unpinned_connection_discovers_a_digest_and_checks_it_before_inference() {
    use crate::jev::config::{
        SystemOneConnection, SystemOneEndpoint, SystemOneProvider, resolve_execution_capabilities,
    };

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let connection = SystemOneConnection {
        provider: SystemOneProvider::Ollama,
        endpoint: SystemOneEndpoint::BaseUrl(base.clone()),
        model: "selected:latest".into(),
        model_revision: None,
        credential: None,
        ..SystemOneConnection::default()
    };
    let server = tokio::spawn(async move {
        let tags = json!({"models": [{"name": "selected:latest", "digest": "discovered-digest"}]});
        let routes = [
            ("GET /api/version", json!({"version": "0.40.1"})),
            ("GET /api/tags", tags.clone()),
            (
                "POST /api/show",
                json!({"capabilities": ["decision"], "template": "{{ .Prompt }}",
                "model_info": {"decision.type": "tev1"}}),
            ),
            (
                "GET /api/ps",
                json!({"models": [{"name": "selected:latest", "digest": "discovered-digest", "context_length": 4096}]}),
            ),
            ("GET /api/tags", tags),
            (
                "POST /v1/systemone",
                json!({"model": "selected:latest", "answers": {"q": {"type": "noul", "noul": 0.9}},
                "usage": {"input_tokens": 12, "output_tokens": 1}}),
            ),
        ];
        for (route, response) in routes {
            let (mut socket, headers, body) = accept_request(&listener).await;
            assert!(headers.starts_with(&format!("{route} HTTP/1.1")));
            if route == "POST /api/show" {
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                    json!({"model": "selected:latest"})
                );
            } else if route == "POST /v1/systemone" {
                assert_eq!(
                    serde_json::from_slice::<JevRequest>(&body).unwrap(),
                    request(&capabilities())
                );
            } else {
                assert!(body.is_empty());
            }
            let body = response.to_string();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });
    let capabilities = resolve_execution_capabilities(&connection, None)
        .await
        .unwrap();
    assert!(connection.model_revision.is_none());
    assert_eq!(
        capabilities.model_revision.as_deref(),
        Some("discovered-digest")
    );
    let sends = AtomicUsize::new(0);
    let usage = AtomicUsize::new(0);
    let request = request(&capabilities);
    let response = OllamaClient::new(&base, None)
        .unwrap()
        .evaluate_with_usage_cancellable(
            &request,
            &capabilities,
            || false,
            || {
                sends.fetch_add(1, Ordering::AcqRel);
            },
            |reported| {
                assert_eq!(
                    reported,
                    JevUsage {
                        input_tokens: 12,
                        output_tokens: 1
                    }
                );
                usage.fetch_add(1, Ordering::AcqRel);
                Ok(())
            },
        )
        .await
        .unwrap();
    assert_eq!(response.usage.input_tokens, 12);
    assert_eq!(
        (sends.load(Ordering::Acquire), usage.load(Ordering::Acquire)),
        (1, 1)
    );
    server.await.unwrap();
}

async fn reply_tags(socket: &mut tokio::net::TcpStream, digest: &str) {
    let body = json!({"models": [{"name": "selected:latest", "digest": digest}]}).to_string();
    socket
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn missing_prepared_revision_blocks_both_inference_paths_and_model_loading() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client =
        OllamaClient::new(&format!("http://{}", listener.local_addr().unwrap()), None).unwrap();
    for revision in [
        None,
        Some(String::new()),
        Some("bad\ndigest".into()),
        Some("x".repeat(257)),
    ] {
        let mut capabilities = capabilities();
        capabilities.model_revision = revision;
        let request = request(&capabilities);
        assert_eq!(
            client
                .evaluate_with_usage_cancellable(
                    &request,
                    &capabilities,
                    || false,
                    || panic!("Missing prepared identity must not report a send."),
                    |_| panic!("Missing prepared identity must not record usage.")
                )
                .await,
            Err(JevError::UnsupportedModel)
        );
        assert_eq!(
            client.evaluate(&request, &capabilities).await,
            Err(OllamaError::UnsupportedRunner)
        );
        capabilities.total_input_tokens = CapabilityLimit::unknown();
        capabilities.state_and_longest_question_tokens = CapabilityLimit::unknown();
        assert!(matches!(
            client
                .discover_ready(DiscoveredModel {
                    name: capabilities.model.clone(),
                    digest: Some("discovered-digest".into()),
                    capabilities,
                })
                .await,
            Err(OllamaError::UnsupportedRunner)
        ));
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cancellation_after_identity_read_prevents_send_and_usage() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client =
        OllamaClient::new(&format!("http://{}", listener.local_addr().unwrap()), None).unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel = Arc::clone(&cancelled);
    let (finished, completed) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut socket = accept_tags(&listener).await;
        cancel.store(true, Ordering::Release);
        reply_tags(&mut socket, "discovered-digest").await;
        tokio::select! {
            extra = listener.accept() => panic!("Cancelled preflight sent another request: {extra:?}"),
            _ = completed => {},
        }
    });
    let capabilities = capabilities();
    let request = request(&capabilities);
    let result = client
        .evaluate_with_usage_cancellable(
            &request,
            &capabilities,
            || cancelled.load(Ordering::Acquire),
            || panic!("Cancellation after metadata must not report an inference send."),
            |_| panic!("Cancellation before inference must not record usage."),
        )
        .await;
    assert_eq!(result, Err(JevError::Cancelled));
    finished.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn ready_discovery_rechecks_identity_after_its_slot_wait_before_loading() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client =
        OllamaClient::new(&format!("http://{}", listener.local_addr().unwrap()), None).unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&reads);
    let digest = Arc::new(Mutex::new("discovered-digest".to_owned()));
    let current_digest = Arc::clone(&digest);
    let (finished, completed) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut socket = accept_tags(&listener).await;
        observed.fetch_add(1, Ordering::AcqRel);
        let digest = current_digest.lock().unwrap().clone();
        reply_tags(&mut socket, &digest).await;
        tokio::select! {
            extra = listener.accept() => panic!("Changed identity started a model load: {extra:?}"),
            _ = completed => {},
        }
    });
    let held = Arc::new(Mutex::new(Some(test_hooks::hold_inference_slots().await)));
    let released = Arc::clone(&held);
    let before_read = Arc::clone(&reads);
    let mut capabilities = capabilities();
    capabilities.total_input_tokens = CapabilityLimit::unknown();
    capabilities.state_and_longest_question_tokens = CapabilityLimit::unknown();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        test_hooks::with_inference_slot_pending(
            move || {
                assert_eq!(before_read.load(Ordering::Acquire), 0);
                *digest.lock().unwrap() = "replacement-digest".into();
                drop(released.lock().unwrap().take().unwrap());
            },
            client.discover_ready(DiscoveredModel {
                name: capabilities.model.clone(),
                digest: capabilities.model_revision.clone(),
                capabilities,
            }),
        ),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(OllamaError::UnsupportedRunner)));
    assert!(held.lock().unwrap().is_none());
    assert_eq!(reads.load(Ordering::Acquire), 1);
    finished.send(()).unwrap();
    server.await.unwrap();
}
