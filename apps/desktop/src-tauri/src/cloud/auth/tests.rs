use super::{
    AccountSession, Config, Credential, EligibleWorkspace, Failure, HttpTransport, TokenRequest,
    Tokens, Transport, Vault, Workspace, WorkspaceKind, WorkspaceResponse, short_hostname,
};
use crate::cloud::{engine::Engine, state::StateStore};
use std::{
    ffi::OsStr,
    sync::{Arc, Mutex},
};
use url::Url;

#[derive(Default)]
struct MemoryVault(Mutex<Option<Credential>>);

impl Vault for MemoryVault {
    fn read(&self) -> Result<Option<Credential>, Failure> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn write(&self, value: Option<&Credential>) -> Result<(), Failure> {
        *self.0.lock().unwrap() = value.cloned();
        Ok(())
    }
}

#[test]
fn cloud_device_label_uses_only_a_safe_short_hostname() {
    assert_eq!(
        short_hostname(OsStr::new("Studio-Mac.local.internal")),
        Some("Studio-Mac")
    );
    for unsuitable in [
        "",
        ".missing",
        "localhost.localdomain",
        "127.0.0.1",
        "fe80::a",
        "home office",
        "München",
        "<desktop>",
        "-desktop",
        "desktop-",
        "desk\ntop",
        "desk\u{202e}top",
        &"a".repeat(64),
    ] {
        assert_eq!(short_hostname(OsStr::new(unsuitable)), None);
    }
}

#[test]
fn cloud_auth_build_origins_default_only_when_both_are_absent() {
    let default = Config::from_origins(None, None);
    if cfg!(debug_assertions) {
        let config = default.unwrap();
        assert_eq!(config.issuer, "http://localhost:3000");
        assert_eq!(config.resource, "http://localhost:3000");
        assert_eq!(
            config.authorization.as_str(),
            "http://localhost:3000/devices/authorize/account"
        );
        assert_eq!(
            config.token.as_str(),
            "http://localhost:3000/v1/devices/token"
        );
        assert_eq!(
            config.session.as_str(),
            "http://localhost:3000/v1/devices/session"
        );
        assert_eq!(
            config.revocation.as_str(),
            "http://localhost:3000/v1/devices/logout"
        );
    } else {
        assert!(default.is_none());
    }

    for (issuer, resource) in [
        (Some("https://issuer.example"), None),
        (None, Some("https://resource.example")),
        (Some(""), Some("")),
        (Some("invalid"), Some("https://resource.example")),
        (
            Some("https://issuer.example"),
            Some("http://remote.example"),
        ),
    ] {
        assert!(Config::from_origins(issuer, resource).is_none());
    }
    let explicit = Config::from_origins(
        Some("https://issuer.example"),
        Some("https://resource.example"),
    )
    .unwrap();
    assert_eq!(explicit.issuer, "https://issuer.example");
    assert_eq!(explicit.resource, "https://resource.example");
    assert_eq!(
        explicit.token.as_str(),
        "https://resource.example/v1/devices/token"
    );
}

#[test]
fn cloud_account_wire_rejects_legacy_claims_and_requires_membership_generations() {
    let origin = "https://api.example";
    let config = Config::parse(origin, origin).unwrap();
    let mut token = serde_json::json!({
        "access_token": "synthetic-access", "refresh_token": "synthetic-refresh",
        "expires_in": 60, "token_type": "Bearer",
        "grant_id": "synthetic-grant", "user_id": "synthetic-user",
        "grant_expires_at": "1970-01-01T02:46:40Z",
        "scope": "capture:write", "resource": origin, "iss": origin,
    });
    let decoded = serde_json::from_value::<Tokens>(token.clone()).unwrap();
    decoded.validate(&config, 100).unwrap();
    for workspace in [serde_json::json!("legacy"), serde_json::Value::Null] {
        token["workspace_id"] = workspace;
        let decoded = serde_json::from_value::<Tokens>(token.clone()).unwrap();
        assert_eq!(
            decoded.validate(&config, 100),
            Err(Failure::InvalidResponse)
        );
        assert_eq!(decoded.refresh_token, "synthetic-refresh");
    }

    let mut session = serde_json::json!({
        "user_id": "synthetic-user", "display_name": null,
        "grant_id": "synthetic-grant", "device_name": "Example desktop",
        "grant_expires_at": "1970-01-01T02:46:40Z",
        "access_expires_at": "1970-01-01T00:02:40Z",
        "scope": "capture:write", "resource": origin, "iss": origin,
    });
    assert!(serde_json::from_value::<AccountSession>(session.clone()).is_ok());
    session["workspace_id"] = serde_json::json!("legacy");
    assert!(serde_json::from_value::<AccountSession>(session).is_err());

    fn list(value: serde_json::Value) -> Result<Vec<EligibleWorkspace>, Failure> {
        serde_json::from_value::<WorkspaceResponse>(value)
            .map_err(|_| Failure::InvalidResponse)?
            .try_into()
    }
    let mut value = serde_json::json!({"workspaces": [
        {
            "workspace_id": "00000000-0000-0000-0000-000000000001",
            "name": "Personal", "type": "personal", "membership_generation": null
        },
        {
            "workspace_id": "00000000-0000-0000-0000-000000000002",
            "name": "Team", "type": "team", "membership_generation": "opaque-generation"
        }
    ]});
    let eligible = list(value.clone()).unwrap();
    assert_eq!(eligible[0].membership_generation, None);
    assert_eq!(
        eligible[1].membership_generation.as_deref(),
        Some("opaque-generation")
    );
    let absent = value["workspaces"][0]
        .as_object_mut()
        .unwrap()
        .remove("membership_generation")
        .unwrap();
    assert_eq!(list(value.clone()), Err(Failure::InvalidResponse));
    value["workspaces"][0]["membership_generation"] = absent;
    value["workspaces"][1]["membership_generation"] = serde_json::Value::Null;
    assert_eq!(list(value), Err(Failure::InvalidResponse));
}

#[test]
fn cloud_legacy_keyring_record_can_revoke_but_never_refresh() {
    use crate::cloud::state::{Cleanup, Phase};

    struct LogoutOnly(Arc<Mutex<Vec<String>>>);
    impl Transport for LogoutOnly {
        fn exchange(&self, _: &Config, _: &TokenRequest<'_>) -> Result<Tokens, Failure> {
            panic!("legacy proof must not refresh");
        }
        fn session(&self, _: &Config, _: &str) -> Result<AccountSession, Failure> {
            panic!("legacy proof must not restore access");
        }
        fn workspaces(&self, _: &Config, _: &str) -> Result<Vec<EligibleWorkspace>, Failure> {
            panic!("legacy proof must not read workspaces");
        }
        fn revoke(&self, _: &Config, refresh: &str) -> Result<(), Failure> {
            self.0.lock().unwrap().push(refresh.to_owned());
            Ok(())
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let state = StateStore::new(directory.path());
    let record = state
        .lock()
        .unwrap()
        .replace(Phase::Active, Cleanup::None)
        .unwrap();
    let origin = "https://api.example";
    let credential = serde_json::from_value::<Credential>(serde_json::json!({
        "epoch": record.epoch, "issuer": origin, "resource": origin,
        "refresh_token": "synthetic-legacy-refresh",
        "session": {
            "user": { "id": "synthetic-user", "display_name": "Example account" },
            "grant_id": "synthetic-grant", "device_name": "Example desktop",
            "workspace": { "id": "synthetic-workspace", "name": "Personal", "kind": "personal" },
            "resource": origin, "scope": "capture:write", "iss": origin,
            "expires_at": 10000, "access_expires_at": 160
        }
    }))
    .unwrap();
    let revoked = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new(
        Config::parse(origin, origin),
        Box::new(MemoryVault(Mutex::new(Some(credential)))),
        Box::new(LogoutOnly(revoked.clone())),
        state,
    )
    .with_clock(|| 100);
    assert_eq!(engine.refresh(100, || true), Err(Failure::ReauthRequired));
    assert!(engine.session().is_none());
    assert!(revoked.lock().unwrap().is_empty());
    engine.sign_out().unwrap();
    assert_eq!(*revoked.lock().unwrap(), ["synthetic-legacy-refresh"]);
    assert!(engine.vault.read().unwrap().is_none());
}

#[test]
fn cloud_account_approval_requires_one_personal_anchor_and_rotation_cannot_change_it() {
    use super::{CAPTURE_SCOPE, Callback};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Script {
        eligible: Mutex<Result<Vec<EligibleWorkspace>, Failure>>,
        exchanges: AtomicUsize,
        revoked: Mutex<Vec<String>>,
    }
    impl Transport for Arc<Script> {
        fn exchange(&self, config: &Config, _: &TokenRequest<'_>) -> Result<Tokens, Failure> {
            let exchange = self.exchanges.fetch_add(1, Ordering::Relaxed);
            Ok(Tokens {
                access_token: format!("synthetic-access-{exchange}"),
                refresh_token: format!("synthetic-refresh-{exchange}"),
                expires_in: 60,
                token_type: "Bearer".into(),
                grant_id: "synthetic-grant".into(),
                user_id: "synthetic-user".into(),
                legacy_workspace_claim: false,
                grant_expires_at: "1970-01-01T02:46:40Z".into(),
                scope: CAPTURE_SCOPE.into(),
                resource: config.resource.clone(),
                iss: config.issuer.clone(),
            })
        }
        fn session(&self, config: &Config, _: &str) -> Result<AccountSession, Failure> {
            Ok(AccountSession {
                user_id: "synthetic-user".into(),
                display_name: None,
                grant_id: "synthetic-grant".into(),
                device_name: "Example desktop".into(),
                scope: CAPTURE_SCOPE.into(),
                resource: config.resource.clone(),
                grant_expires_at: "1970-01-01T02:46:40Z".into(),
                access_expires_at: "1970-01-01T00:03:20Z".into(),
                iss: config.issuer.clone(),
            })
        }
        fn workspaces(&self, _: &Config, _: &str) -> Result<Vec<EligibleWorkspace>, Failure> {
            self.eligible.lock().unwrap().clone()
        }
        fn revoke(&self, _: &Config, refresh: &str) -> Result<(), Failure> {
            self.revoked.lock().unwrap().push(refresh.into());
            Ok(())
        }
    }

    fn personal(id: &str) -> EligibleWorkspace {
        EligibleWorkspace {
            workspace: Workspace {
                id: id.into(),
                name: "Personal".into(),
                kind: WorkspaceKind::Personal,
            },
            membership_generation: None,
        }
    }
    let config = Config::parse("https://api.example", "https://api.example").unwrap();
    let initial = personal("00000000-0000-0000-0000-000000000001");
    let changed = personal("00000000-0000-0000-0000-000000000002");
    for (eligible, accepted) in [
        (Err(Failure::Unavailable), false),
        (Ok(vec![]), false),
        (Ok(vec![initial.clone(), changed.clone()]), false),
        (Ok(vec![initial.clone()]), true),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let script = Arc::new(Script {
            eligible: Mutex::new(eligible),
            exchanges: AtomicUsize::new(0),
            revoked: Mutex::new(Vec::new()),
        });
        let mut engine = Engine::new(
            Some(config.clone()),
            Box::new(MemoryVault::default()),
            Box::new(script.clone()),
            StateStore::new(directory.path()),
        )
        .with_clock(|| 100);
        let (url, _listener, attempt) = engine.begin(100, 0).unwrap();
        let url = Url::parse(&url).unwrap();
        assert_eq!(url.path(), "/devices/authorize/account");
        assert_eq!(
            url.query_pairs()
                .map(|(key, _)| key.into_owned())
                .collect::<Vec<_>>(),
            [
                "redirect_uri",
                "state",
                "code_challenge",
                "code_challenge_method",
                "device_name",
            ]
        );
        let callback = engine.callback(
            &attempt,
            Callback::Code("synthetic-code".into()),
            100,
            || true,
        );
        if accepted {
            callback.unwrap();
            assert_eq!(engine.session().unwrap().workspace, initial.workspace);
            assert!(engine.vault.read().unwrap().unwrap().account_grant);
            *script.eligible.lock().unwrap() = Ok(vec![changed.clone()]);
            assert_eq!(engine.refresh(131, || true), Err(Failure::ReauthRequired));
            assert_eq!(
                *script.revoked.lock().unwrap(),
                ["synthetic-refresh-0", "synthetic-refresh-1"]
            );
        } else {
            assert_eq!(callback, Err(Failure::ReauthRequired));
            assert_eq!(*script.revoked.lock().unwrap(), ["synthetic-refresh-0"]);
        }
        assert!(engine.session().is_none());
        assert!(engine.vault.read().unwrap().is_none());
    }
}

#[test]
fn cloud_auth_http_requires_debug_and_exact_loopback_origins() {
    for origin in [
        "http://localhost:3000",
        "http://127.0.0.1:3000",
        "http://[::1]:3000",
    ] {
        assert_eq!(
            Config::parse(origin, origin).is_some(),
            cfg!(debug_assertions),
            "{origin}"
        );
        if cfg!(debug_assertions) {
            let config = Config::from_origins(Some(origin), Some(origin)).unwrap();
            assert_eq!(config.issuer, origin);
            assert_eq!(config.resource, origin);
        }
    }
}

#[test]
fn cloud_auth_rejects_origin_aliases_and_localhost_lookalikes() {
    for origin in [
        "http://remote.example:3000",
        "http://localhost.example:3000",
        "http://localhost.:3000",
        "http://localhоst:3000",
        "http://127.0.0.2:3000",
        "http://127.1:3000",
        "http://2130706433:3000",
        "http://0177.0.0.1:3000",
        "http://[0:0:0:0:0:0:0:1]:3000",
        "http://[::ffff:127.0.0.1]:3000",
        "http://LOCALHOST:3000",
        "http://localhost:80",
        "http://localhost:03000",
        "http://localhost:3000/",
        "http://localhost:3000/path",
        "http://localhost:3000?query",
        "http://localhost:3000#fragment",
        "http://user@localhost:3000",
        "http://user:password@localhost:3000",
        "http://localhost:3000@remote.example",
        "https://localhost:3000/",
        "https://LOCALHOST:3000",
        "https://localhost:443",
        "https://user@localhost:3000",
        "https://localhost:3000/path",
        "https://localhost:3000?query",
        "https://localhost:3000#fragment",
    ] {
        assert!(
            Config::parse(origin, "https://api.example").is_none(),
            "{origin}"
        );
        assert!(
            Config::parse("https://api.example", origin).is_none(),
            "{origin}"
        );
    }
}

#[test]
fn cloud_auth_transport_checks_every_url_before_sending_proof() {
    for target in [
        "http://remote.example/v1/devices/token",
        "http://localhost.example/v1/devices/token",
        "http://127.0.0.2/v1/devices/token",
        "http://user:password@localhost/v1/devices/token",
        "https://user:password@api.example/v1/devices/token",
        "ftp://localhost/v1/devices/token",
    ] {
        let mut config = Config::parse("https://api.example", "https://api.example").unwrap();
        config.token = Url::parse(target).unwrap();
        config.session = config.token.clone();
        config.revocation = config.token.clone();
        assert!(matches!(
            HttpTransport.exchange(
                &config,
                &TokenRequest::RefreshToken {
                    refresh_token: "synthetic-refresh"
                }
            ),
            Err(Failure::Unavailable)
        ));
        assert!(matches!(
            HttpTransport.session(&config, "synthetic-access"),
            Err(Failure::Unavailable)
        ));
        assert_eq!(
            HttpTransport.revoke(&config, "synthetic-refresh"),
            Err(Failure::Unavailable)
        );
    }
}

#[cfg(not(debug_assertions))]
#[test]
fn cloud_auth_release_disables_defaults_and_loopback_http_transport() {
    assert!(Config::from_origins(None, None).is_none());
    for origin in [
        "http://localhost:3000",
        "http://127.0.0.1:3000",
        "http://[::1]:3000",
    ] {
        assert!(Config::parse(origin, "https://api.example").is_none());
        assert!(Config::parse("https://api.example", origin).is_none());
        let mut config = Config::parse("https://api.example", "https://api.example").unwrap();
        let url = Url::parse(origin).unwrap();
        config.token = url.join("v1/devices/token").unwrap();
        config.session = url.join("v1/devices/session").unwrap();
        config.revocation = url.join("v1/devices/logout").unwrap();
        assert!(matches!(
            HttpTransport.exchange(
                &config,
                &TokenRequest::RefreshToken {
                    refresh_token: "synthetic-refresh"
                }
            ),
            Err(Failure::Unavailable)
        ));
        assert!(matches!(
            HttpTransport.session(&config, "synthetic-access"),
            Err(Failure::Unavailable)
        ));
        assert_eq!(
            HttpTransport.revoke(&config, "synthetic-refresh"),
            Err(Failure::Unavailable)
        );
    }
}

#[cfg(debug_assertions)]
mod debug_http {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread::JoinHandle,
        time::{Duration, Instant},
    };

    #[test]
    fn cloud_exchange_timeout_keeps_delivery_uncertain() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/token", listener.local_addr().unwrap());
        let (release, wait) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut received = Vec::new();
            let proof = b"synthetic-refresh";
            while !received.windows(proof.len()).any(|bytes| bytes == proof) {
                let mut bytes = [0; 1024];
                let count = stream.read(&mut bytes).unwrap();
                assert!(count > 0 && received.len() < 16_384);
                received.extend_from_slice(&bytes[..count]);
            }
            wait.recv_timeout(Duration::from_secs(3)).unwrap();
        });
        let _ = rustls::crypto::ring::default_provider().install_default();
        let error = reqwest::blocking::Client::builder()
            .no_proxy()
            .retry(reqwest::retry::never())
            .timeout(Duration::from_millis(300))
            .build()
            .unwrap()
            .post(url)
            .body("synthetic-refresh")
            .send()
            .unwrap_err();
        assert!(error.is_timeout());
        assert_eq!(super::super::exchange_failure(error), Failure::Offline);
        release.send(()).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn cloud_exchange_tls_failure_precedes_http_proof() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("https://{}/token", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = [0; 16_384];
            let count = stream.read(&mut bytes).unwrap();
            assert!(count > 0);
            assert_eq!(bytes[0], 0x16);
            stream.write_all(b"not a TLS response").unwrap();
        });
        let _ = rustls::crypto::ring::default_provider().install_default();
        let error = reqwest::blocking::Client::builder()
            .no_proxy()
            .https_only(true)
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap()
            .post(url)
            .body("synthetic-refresh")
            .send()
            .unwrap_err();
        assert!(error.is_connect());
        assert_eq!(
            super::super::exchange_failure(error),
            Failure::OfflineBeforeSend
        );
        worker.join().unwrap();
    }

    #[test]
    fn cloud_debug_default_never_sends_saved_https_refresh_or_logout_proof() {
        use super::super::{Session, User};
        use crate::cloud::{
            engine::Engine,
            state::{Cleanup, Phase, StateStore},
        };

        struct SavedVault(Credential);
        impl Vault for SavedVault {
            fn read(&self) -> Result<Option<Credential>, Failure> {
                Ok(Some(self.0.clone()))
            }
            fn write(&self, _: Option<&Credential>) -> Result<(), Failure> {
                panic!("an origin change must not rewrite the stored credential");
            }
        }

        struct NoRequests;
        impl Transport for NoRequests {
            fn exchange(&self, _: &Config, _: &TokenRequest<'_>) -> Result<Tokens, Failure> {
                panic!("an old refresh proof must not reach the new service");
            }
            fn session(&self, _: &Config, _: &str) -> Result<AccountSession, Failure> {
                panic!("an old connection must not restore access");
            }
            fn workspaces(&self, _: &Config, _: &str) -> Result<Vec<EligibleWorkspace>, Failure> {
                panic!("an old connection must not read workspaces");
            }
            fn revoke(&self, _: &Config, _: &str) -> Result<(), Failure> {
                panic!("old logout proofs must not reach the new service");
            }
        }

        for cleanup_refresh_tokens in [vec![], vec!["synthetic-cleanup".to_owned()]] {
            let directory = tempfile::tempdir().unwrap();
            let state = StateStore::new(directory.path());
            let record = state
                .lock()
                .unwrap()
                .replace(Phase::Active, Cleanup::None)
                .unwrap();
            let old = Config::parse("https://api.example", "https://api.example").unwrap();
            let session = Session {
                user: User {
                    id: "synthetic-user".into(),
                    display_name: "Example user".into(),
                },
                grant_id: "synthetic-grant".into(),
                device_name: "Example desktop".into(),
                workspace: Workspace {
                    id: "synthetic-workspace".into(),
                    name: "Example workspace".into(),
                    kind: WorkspaceKind::Personal,
                },
                resource: old.resource.clone(),
                scope: super::super::CAPTURE_SCOPE.into(),
                iss: old.issuer.clone(),
                expires_at: 10_000,
                access_expires_at: 160,
            };
            session.validate(&old, 100).unwrap();
            let credential = Credential {
                epoch: record.epoch,
                issuer: old.issuer.clone(),
                resource: old.resource.clone(),
                refresh_token: "synthetic-refresh".into(),
                cleanup_refresh_tokens: cleanup_refresh_tokens.clone(),
                account_grant: true,
                session: Some(session),
            };
            let mut engine = Engine::new(
                Config::from_origins(None, None),
                Box::new(SavedVault(credential)),
                Box::new(NoRequests),
                state,
            );
            if cleanup_refresh_tokens.is_empty() {
                assert_eq!(engine.refresh(100, || true), Err(Failure::ReauthRequired));
            }
            assert_eq!(engine.sign_out(), Err(Failure::CredentialFailed));
            assert!(engine.session().is_none());
            let retained = engine.vault.read().unwrap().unwrap();
            assert_eq!(retained.issuer, old.issuer);
            assert_eq!(retained.resource, old.resource);
            assert_eq!(retained.refresh_token, "synthetic-refresh");
            assert_eq!(retained.cleanup_refresh_tokens, cleanup_refresh_tokens);
        }
    }

    struct Request {
        line: String,
        headers: Vec<(String, String)>,
        body: serde_json::Value,
    }

    impl Request {
        fn header(&self, name: &str) -> Option<&str> {
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

    /// Serve one bounded synthetic request through a parsed local configuration.
    fn server(
        status: u16,
        body: impl FnOnce(&str) -> String,
        location: Option<&str>,
    ) -> (Config, JoinHandle<Request>) {
        serve(
            TcpListener::bind("127.0.0.1:0").unwrap(),
            status,
            body,
            location,
        )
    }

    fn serve(
        listener: TcpListener,
        status: u16,
        body: impl FnOnce(&str) -> String,
        location: Option<&str>,
    ) -> (Config, JoinHandle<Request>) {
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
        let config = Config::parse(&origin, &origin).unwrap();
        let body = body(&origin);
        let location = location
            .map(|value| format!("Location: {value}\r\n"))
            .unwrap_or_default();
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "no synthetic request arrived");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("synthetic accept failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
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
            let mut bytes = vec![0; length];
            stream.read_exact(&mut bytes).unwrap();
            let request_body = if bytes.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap()
            };
            write!(
                stream,
                "HTTP/1.1 {status} Synthetic\r\n{location}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            Request {
                line,
                headers,
                body: request_body,
            }
        });
        (config, worker)
    }

    #[test]
    fn cloud_debug_loopback_transport_binds_authority() {
        // Token, session, and logout belong to the same synthetic origin.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let (config, worker) = serve(
            listener.try_clone().unwrap(),
            200,
            |origin| {
                serde_json::json!({
                    "access_token": "synthetic-access",
                    "refresh_token": "synthetic-refresh",
                    "expires_in": 60, "token_type": "Bearer",
                    "grant_id": "synthetic-grant", "user_id": "synthetic-user",
                    "grant_expires_at": "1970-01-01T02:46:40Z",
                    "scope": "capture:write", "resource": origin, "iss": origin,
                })
                .to_string()
            },
            None,
        );
        let tokens = HttpTransport
            .exchange(
                &config,
                &TokenRequest::AuthorizationCode {
                    code: "synthetic-code",
                    code_verifier: "synthetic-verifier",
                },
            )
            .unwrap();
        tokens.validate(&config, 100).unwrap();
        let request = worker.join().unwrap();
        request.native_only();
        assert_eq!(request.line, "POST /v1/devices/token HTTP/1.1");
        assert!(request.header("authorization").is_none());
        assert_eq!(
            request.body,
            serde_json::json!({
                "grant_type": "authorization_code",
                "code": "synthetic-code", "code_verifier": "synthetic-verifier",
            })
        );
        let (config, worker) = serve(
            listener.try_clone().unwrap(),
            200,
            |origin| {
                serde_json::json!({
                    "user_id": "synthetic-user", "display_name": null,
                    "grant_id": "synthetic-grant", "device_name": "Example desktop",
                    "grant_expires_at": "1970-01-01T02:46:40Z",
                    "access_expires_at": "1970-01-01T00:02:40Z",
                    "scope": "capture:write", "resource": origin, "iss": origin,
                })
                .to_string()
            },
            None,
        );
        let session = HttpTransport
            .session(&config, &tokens.access_token)
            .unwrap();
        assert!(tokens.matches_session(&session));
        let request = worker.join().unwrap();
        request.native_only();
        assert_eq!(request.line, "GET /v1/devices/session HTTP/1.1");
        assert_eq!(
            request.header("authorization"),
            Some("Bearer synthetic-access")
        );
        assert_eq!(request.body, serde_json::Value::Null);

        let (config, worker) = serve(listener, 204, |_| String::new(), None);
        HttpTransport.revoke(&config, "synthetic-refresh").unwrap();
        let request = worker.join().unwrap();
        request.native_only();
        assert_eq!(request.line, "POST /v1/devices/logout HTTP/1.1");
        assert!(request.header("authorization").is_none());
        assert_eq!(
            request.body,
            serde_json::json!({ "refresh_token": "synthetic-refresh" })
        );
    }

    #[test]
    fn cloud_debug_loopback_does_not_follow_redirects() {
        let destination = TcpListener::bind("127.0.0.1:0").unwrap();
        destination.set_nonblocking(true).unwrap();
        let location = format!("http://{}/redirected", destination.local_addr().unwrap());
        for route in ["token", "session", "revocation"] {
            let (config, worker) = server(307, |_| "{}".into(), Some(&location));
            let result = match route {
                "token" => HttpTransport
                    .exchange(
                        &config,
                        &TokenRequest::RefreshToken {
                            refresh_token: "synthetic-refresh",
                        },
                    )
                    .map(|_| ()),
                "session" => HttpTransport
                    .session(&config, "synthetic-access")
                    .map(|_| ()),
                "revocation" => HttpTransport.revoke(&config, "synthetic-refresh"),
                _ => unreachable!(),
            };
            assert_eq!(result, Err(Failure::Offline));
            worker.join().unwrap().native_only();
            assert_eq!(
                destination.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }

    #[test]
    fn cloud_debug_loopback_bypasses_environment_proxies() {
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        proxy.set_nonblocking(true).unwrap();
        let address = format!("http://{}", proxy.local_addr().unwrap());
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args([
            "--exact",
            "cloud::auth::tests::debug_http::cloud_debug_loopback_transport_binds_authority",
            "--nocapture",
        ]);
        for name in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            command.env(name, &address);
        }
        command.env("NO_PROXY", "").env("no_proxy", "");
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        assert_eq!(
            proxy.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
