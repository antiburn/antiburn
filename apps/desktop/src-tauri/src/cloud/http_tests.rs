use super::auth::{Config, Failure, HttpTransport, TokenRequest, Transport};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread::JoinHandle,
    time::Duration,
};

pub(super) struct Request {
    pub(super) line: String,
    headers: Vec<(String, String)>,
    pub(super) body: Vec<u8>,
}

impl Request {
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn native_only(&self) {
        assert!(self.header("cookie").is_none());
        assert!(self.header("origin").is_none());
    }
}

pub(super) fn server(status: u16, body: &str) -> (Config, JoinHandle<Request>) {
    server_with_reply(Some((status, body)))
}

fn server_with_reply(reply: Option<(u16, &str)>) -> (Config, JoinHandle<Request>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let origin = url::Url::parse(&origin).unwrap();
    let mut config = Config::parse("https://api.example", "https://api.example").unwrap();
    config.authorization = origin.join("devices/authorize/account").unwrap();
    config.token = origin.join("v1/devices/token").unwrap();
    config.session = origin.join("v1/devices/grants/current").unwrap();
    config.workspaces = origin.join("v1/captures/destinations").unwrap();
    config.revocation = origin.join("v1/devices/grants/current/revoke").unwrap();
    let reply = reply.map(|(status, body)| (status, body.to_owned()));
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            assert!(bytes.len() < 16_384);
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
        }
        let text = String::from_utf8(bytes).unwrap();
        let mut lines = text.split("\r\n");
        let line = lines.next().unwrap().to_owned();
        let headers = lines
            .filter(|line| !line.is_empty())
            .map(|line| {
                let (key, value) = line.split_once(':').unwrap();
                (key.to_ascii_lowercase(), value.trim().to_owned())
            })
            .collect::<Vec<_>>();
        let length = headers
            .iter()
            .find(|(key, _)| key == "content-length")
            .map_or(0, |(_, value)| value.parse::<usize>().unwrap());
        assert!(length < 16_384);
        let mut request_body = vec![0; length];
        stream.read_exact(&mut request_body).unwrap();
        if let Some((status, body)) = reply {
            write!(
                stream,
                "HTTP/1.1 {status} Synthetic\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
        Request {
            line,
            headers,
            body: request_body,
        }
    });
    (config, worker)
}

#[test]
fn cloud_http_workspace_list_uses_only_the_current_bearer() {
    let (config, worker) = server(
        200,
        r#"{"workspaces":[{"workspace_id":"12345678-1234-1234-1234-123456789abc","name":"Example","type":"team","membership_generation":"opaque-generation"}]}"#,
    );
    let workspaces = HttpTransport
        .workspaces(&config, "synthetic-access")
        .unwrap();
    assert_eq!(
        workspaces[0].workspace.id,
        "12345678-1234-1234-1234-123456789abc"
    );
    assert_eq!(
        workspaces[0].workspace.kind,
        super::auth::WorkspaceKind::Team
    );
    assert_eq!(
        workspaces[0].membership_generation.as_deref(),
        Some("opaque-generation")
    );
    let request = worker.join().unwrap();
    assert_eq!(request.line, "GET /v1/captures/destinations HTTP/1.1");
    assert_eq!(
        request.header("authorization"),
        Some("Bearer synthetic-access")
    );
    assert!(request.body.is_empty());
    request.native_only();

    for (status, failure) in [
        (400, Failure::InvalidResponse),
        (401, Failure::ReauthRequired),
        (403, Failure::Rejected),
        (404, Failure::Unavailable),
        (503, Failure::Offline),
    ] {
        let (config, worker) = server(status, "{}");
        assert_eq!(
            HttpTransport.workspaces(&config, "synthetic-access"),
            Err(failure)
        );
        worker.join().unwrap();
    }

    let (config, worker) = server(
        200,
        r#"{"workspaces":[{"workspace_id":"12345678-1234-1234-1234-123456789abc","name":"A","type":"personal","membership_generation":null},{"workspace_id":"12345678-1234-1234-1234-123456789abc","name":"B","type":"team","membership_generation":"opaque-generation"}]}"#,
    );
    assert_eq!(
        HttpTransport.workspaces(&config, "synthetic-access"),
        Err(Failure::InvalidResponse)
    );
    worker.join().unwrap();
}

#[test]
fn cloud_http_code_exchange_sends_only_first_party_json_proof() {
    let (config, worker) = server(400, "{}");
    assert!(matches!(
        HttpTransport.exchange(
            &config,
            &TokenRequest::AuthorizationCode {
                code: "synthetic-code",
                code_verifier: "synthetic-verifier",
            },
        ),
        Err(Failure::Rejected)
    ));
    let request = worker.join().unwrap();
    assert_eq!(request.line, "POST /v1/devices/token HTTP/1.1");
    request.native_only();
    assert!(request.header("authorization").is_none());
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
        serde_json::json!({
            "grant_type": "authorization_code",
            "code": "synthetic-code",
            "code_verifier": "synthetic-verifier",
        })
    );
}

#[test]
fn cloud_http_refresh_sends_only_first_party_json_proof() {
    let (config, worker) = server(401, "{}");
    assert!(matches!(
        HttpTransport.exchange(
            &config,
            &TokenRequest::RefreshToken {
                refresh_token: "synthetic-refresh"
            },
        ),
        Err(Failure::Rejected)
    ));
    let request = worker.join().unwrap();
    assert_eq!(request.line, "POST /v1/devices/token HTTP/1.1");
    request.native_only();
    assert!(request.header("authorization").is_none());
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
        serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": "synthetic-refresh",
        })
    );
}

#[test]
fn cloud_http_refresh_distinguishes_connection_failure_from_a_lost_reply() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut config = Config::parse("https://api.example", "https://api.example").unwrap();
    config.token = format!("http://{}/v1/devices/token", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    drop(listener);
    assert!(matches!(
        HttpTransport.exchange(
            &config,
            &TokenRequest::RefreshToken {
                refresh_token: "synthetic-refresh"
            },
        ),
        Err(Failure::OfflineBeforeSend)
    ));

    let (config, worker) = server_with_reply(None);
    assert!(matches!(
        HttpTransport.exchange(
            &config,
            &TokenRequest::RefreshToken {
                refresh_token: "synthetic-refresh"
            },
        ),
        Err(Failure::Offline)
    ));
    let request = worker.join().unwrap();
    assert_eq!(request.line, "POST /v1/devices/token HTTP/1.1");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
        serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": "synthetic-refresh"
        })
    );
}

#[test]
fn cloud_http_session_sends_only_the_access_bearer() {
    let (config, worker) = server(403, "{}");
    assert!(matches!(
        HttpTransport.session(&config, "synthetic-access"),
        Err(Failure::Rejected)
    ));
    let request = worker.join().unwrap();
    assert_eq!(request.line, "GET /v1/devices/grants/current HTTP/1.1");
    request.native_only();
    assert_eq!(
        request.header("authorization"),
        Some("Bearer synthetic-access")
    );
    assert!(request.body.is_empty());
}

#[test]
fn cloud_http_logout_sends_only_refresh_proof_and_requires_synchronous_204() {
    for status in [204, 200, 202, 302, 401, 503] {
        let (config, worker) = server(status, "");
        assert_eq!(
            HttpTransport.revoke(&config, "synthetic-refresh").is_ok(),
            status == 204
        );
        let request = worker.join().unwrap();
        assert_eq!(
            request.line,
            "POST /v1/devices/grants/current/revoke HTTP/1.1"
        );
        request.native_only();
        assert!(request.header("authorization").is_none());
        assert_eq!(request.header("content-type"), Some("application/json"));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&request.body).unwrap(),
            serde_json::json!({ "refresh_token": "synthetic-refresh" })
        );
    }
}

fn token_json() -> serde_json::Value {
    serde_json::json!({
        "access_token": "synthetic-desktop-access",
        "refresh_token": "synthetic-desktop-refresh",
        "token_type": "Bearer", "expires_in": 60,
        "grant_id": "grant-synthetic", "user_id": "account-synthetic",
        "grant_expires_at": "1970-01-01T02:46:40Z",
        "scope": "capture:write", "resource": "https://api.example",
        "iss": "https://api.example"
    })
}

fn session_json() -> serde_json::Value {
    serde_json::json!({
        "user_id": "account-synthetic", "display_name": null,
        "grant_id": "grant-synthetic", "device_name": "Example desktop",
        "scope": "capture:write", "resource": "https://api.example",
        "grant_expires_at": "1970-01-01T02:46:40Z",
        "access_expires_at": "1970-01-01T00:03:00Z",
        "iss": "https://api.example"
    })
}

#[test]
fn cloud_http_frozen_token_and_flat_session_responses_bind_exact_authority() {
    let (config, worker) = server(200, &token_json().to_string());
    let tokens = HttpTransport
        .exchange(
            &config,
            &TokenRequest::AuthorizationCode {
                code: "synthetic",
                code_verifier: "synthetic",
            },
        )
        .unwrap();
    worker.join().unwrap().native_only();
    tokens.validate(&config, 100).unwrap();
    let (config, worker) = server(200, &session_json().to_string());
    let session = HttpTransport
        .session(&config, &tokens.access_token)
        .unwrap();
    worker.join().unwrap().native_only();
    assert!(tokens.matches_session(&session));
    assert_eq!(session.display_name, None);
    assert_eq!(session.access_expires_at, "1970-01-01T00:03:00Z");
    for field in [
        "grant_id",
        "user_id",
        "scope",
        "resource",
        "iss",
        "grant_expires_at",
    ] {
        let mut changed = token_json();
        changed[field] = "different".into();
        let changed: super::auth::Tokens = serde_json::from_value(changed).unwrap();
        assert!(!changed.matches_session(&session), "{field}");
    }
}

#[test]
fn cloud_http_returns_decoded_logout_proof_before_authority_validation() {
    use super::auth::{Callback, Vault};
    use super::{engine::Engine, state::StateStore, tests::MemoryVault};
    use std::sync::Arc;

    for expires in [0, 901] {
        let mut body = token_json();
        body["expires_in"] = expires.into();
        let (config, worker) = server(200, &body.to_string());
        let directory = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let mut engine = Engine::new(
            Some(config),
            Box::new(vault.clone()),
            Box::new(HttpTransport),
            StateStore::new(directory.path()),
        );
        let (_, _listener, attempt) = engine.begin(100, 0).unwrap();
        assert_eq!(
            engine.callback(&attempt, Callback::Code("synthetic".into()), 101, || true),
            Err(Failure::ReauthRequired)
        );
        worker.join().unwrap();
        assert!(engine.session().is_none());
        let cleanup = vault.read().unwrap().unwrap();
        assert!(cleanup.session.is_none());
        assert_eq!(cleanup.refresh_token, "synthetic-desktop-refresh");
    }
}

#[test]
fn cloud_http_rejects_unusable_proof_and_old_or_malformed_session_shapes() {
    for body in [
        {
            let mut body = token_json();
            body["expires_in"] = (-1).into();
            body
        },
        {
            let mut body = token_json();
            body["refresh_token"] = "".into();
            body
        },
    ] {
        let (config, worker) = server(200, &body.to_string());
        assert!(matches!(
            HttpTransport.exchange(
                &config,
                &TokenRequest::RefreshToken {
                    refresh_token: "synthetic"
                }
            ),
            Err(Failure::InvalidResponse)
        ));
        worker.join().unwrap();
    }
    for body in [
        serde_json::json!({"user":{"id":"old"},"device_id":"old"}),
        {
            let mut value = session_json();
            value["grant_expires_at"] = "not-a-deadline".into();
            value
        },
        {
            let mut value = session_json();
            value["access_expires_at"] = 100.into();
            value
        },
    ] {
        let (config, worker) = server(200, &body.to_string());
        assert!(matches!(
            HttpTransport.session(&config, "synthetic"),
            Err(Failure::InvalidResponse)
        ));
        worker.join().unwrap();
    }
}
