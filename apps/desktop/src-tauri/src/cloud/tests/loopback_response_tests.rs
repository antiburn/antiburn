use super::*;

/// Send one synthetic browser request and check the full response envelope.
async fn browser_get(address: &str, query: &str) -> String {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let request = format!("GET /cloud/callback?{query} HTTP/1.1\r\nHost: {address}\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut reply = String::new();
    timeout(Duration::from_secs(2), stream.read_to_string(&mut reply))
        .await
        .unwrap()
        .unwrap();
    let (headers, body) = reply.split_once("\r\n\r\n").unwrap();
    for header in [
        "Content-Type: text/html; charset=utf-8",
        "Cache-Control: no-store",
        "Referrer-Policy: no-referrer",
        "X-Content-Type-Options: nosniff",
        "Connection: close",
    ] {
        assert!(headers.lines().any(|line| line == header));
    }
    assert!(
        headers
            .lines()
            .any(|line| line == format!("Content-Length: {}", body.len()))
    );
    let style = body
        .split_once("<style>")
        .unwrap()
        .1
        .split_once("</style>")
        .unwrap()
        .0;
    let hash = STANDARD.encode(Sha256::digest(style.as_bytes()));
    let csp = format!(
        "Content-Security-Policy: default-src 'none'; style-src 'sha256-{hash}'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    );
    assert!(headers.lines().any(|line| line == csp));
    for forbidden in [
        "synthetic-",
        "https://",
        "http://",
        "href=",
        "src=",
        "<script",
        "<form",
        "url(",
        "signed in",
        "sync",
    ] {
        assert!(!body.contains(forbidden));
    }
    assert!(!headers.contains("Location:"));
    assert!(body.contains("<main>"));
    reply
}

/// Start the real listener with synthetic proof and no Cloud service.
fn start_browser_attempt() -> (String, tokio::task::JoinHandle<Option<Callback>>) {
    let listener = Listener::bind().unwrap();
    let attempt = Attempt {
        callback: listener.callback().into(),
        state: "synthetic-state".into(),
        verifier: "synthetic-verifier".into(),
        epoch: "synthetic-epoch".into(),
        expires_at: super::super::now() + 600,
        generation: 0,
    };
    let address = listener.listener.local_addr().unwrap().to_string();
    let task = tokio::spawn(async move {
        listener
            .wait(
                &Config::parse("https://api.example", "https://api.example").unwrap(),
                &attempt,
                || true,
            )
            .await
    });
    (address, task)
}

#[tokio::test]
async fn cloud_async_loopback_invalid_page_does_not_consume_the_valid_return() {
    let (address, task) = start_browser_attempt();
    let rejected = browser_get(
        &address,
        "state=synthetic-wrong&iss=https%3A%2F%2Fapi.example&code=synthetic-code",
    )
    .await;
    assert!(rejected.starts_with("HTTP/1.1 400 Bad Request"));
    assert!(rejected.contains("Sign-in link not accepted"));
    assert!(!rejected.contains("Browser step complete"));
    assert!(!task.is_finished());

    let accepted = browser_get(
        &address,
        "state=synthetic-state&iss=https%3A%2F%2Fapi.example&code=synthetic-code",
    )
    .await;
    assert!(accepted.starts_with("HTTP/1.1 200 OK"));
    assert!(accepted.contains("Browser step complete"));
    assert!(accepted.contains("Return to Antiburn"));
    assert!(accepted.contains("The desktop app will finish checking your sign-in."));
    assert!(matches!(task.await.unwrap(), Some(Callback::Code(code)) if code == "synthetic-code"));
}

#[tokio::test]
async fn cloud_async_loopback_denied_page_does_not_claim_browser_completion() {
    let (address, task) = start_browser_attempt();
    let denied = browser_get(
        &address,
        "state=synthetic-state&iss=https%3A%2F%2Fapi.example&error=access_denied",
    )
    .await;
    assert!(denied.starts_with("HTTP/1.1 200 OK"));
    assert!(denied.contains("Browser approval cancelled"));
    assert!(denied.contains("Try again in the desktop app, or continue locally."));
    assert!(!denied.contains("access_denied"));
    assert!(!denied.contains("Browser step complete"));
    assert!(matches!(task.await.unwrap(), Some(Callback::Cancelled)));
}
