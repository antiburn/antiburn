use super::*;
use std::sync::Arc;

#[derive(Default)]
struct MemoryVault(Mutex<Secrets>);
impl Vault for Arc<MemoryVault> {
    fn read(&self) -> Result<Secrets, Failure> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn write(&self, value: &Secrets) -> Result<(), Failure> {
        *self.0.lock().unwrap() = value.clone();
        Ok(())
    }
}
struct MockTransport;
impl Transport for MockTransport {
    fn exchange(&self, _: &Config, _: &[(&str, &str)]) -> Result<auth::Tokens, Failure> {
        Ok(auth::Tokens {
            access_token: "synthetic-access".into(),
            refresh_token: "synthetic-rotated".into(),
            expires_in: 600,
            token_type: "Bearer".into(),
        })
    }
    fn profile(&self, _: &Config, _: &str) -> Result<Identity, Failure> {
        Ok(Identity {
            id: "synthetic-id".into(),
            login: "example".into(),
        })
    }
    fn revoke(&self, _: &Config, _: &str) -> Result<(), Failure> {
        Ok(())
    }
}
fn config() -> Config {
    Config::parse(
        "https://example.com/authorize",
        "https://example.com/token",
        "https://example.com/profile",
        "https://example.com/revoke",
        "public-test-client",
        "antiburn-review://cloud/auth",
        "antiburn-review",
    )
    .unwrap()
}
fn engine(auth: bool) -> (Engine, Arc<MemoryVault>) {
    let vault = Arc::new(MemoryVault::default());
    (
        Engine {
            config: auth.then(config),
            scheme: "antiburn-review".into(),
            vault: Box::new(vault.clone()),
            transport: Box::new(MockTransport),
            loaded: false,
            attempt_generation: 0,
            account_generation: 0,
            secrets: Secrets::default(),
            access: None,
            identity: None,
            status: AuthStatus::Idle,
        },
        vault,
    )
}
#[test]
fn cloud_pkce_matches_rfc7636() {
    assert_eq!(
        auth::challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}
#[test]
fn cloud_configuration_rejects_incomplete_insecure_and_wrong_callback() {
    assert!(
        Config::parse(
            "http://example.com/auth",
            "https://example.com/token",
            "https://example.com/profile",
            "https://example.com/revoke",
            "public",
            "antiburn-review://cloud/auth",
            "antiburn-review"
        )
        .is_none()
    );
    assert!(
        Config::parse(
            "https://example.com/auth",
            "https://evil.example/token",
            "https://example.com/profile",
            "https://example.com/revoke",
            "public",
            "antiburn-review://cloud/auth",
            "antiburn-review"
        )
        .is_none()
    );
    assert!(
        Config::parse(
            "https://example.com/auth",
            "https://example.com/token",
            "https://example.com/profile",
            "https://example.com/revoke",
            "public",
            "antiburn://cloud/auth",
            "antiburn-review"
        )
        .is_none()
    );
}
struct NoMarketingVault;
impl Vault for NoMarketingVault {
    fn read(&self) -> Result<Secrets, Failure> {
        panic!("Marketing must not read credential storage")
    }
    fn write(&self, _: &Secrets) -> Result<(), Failure> {
        panic!("Marketing must not write credential storage")
    }
}
#[test]
fn cloud_marketing_opens_exact_link_without_credentials_attempt_or_waiting() {
    let (mut engine, _) = engine(false);
    engine.vault = Box::new(NoMarketingVault);
    engine.refresh(100, || true).unwrap();
    assert_eq!(engine.begin(100, 0).unwrap(), "https://antiburn.com/cloud");
    engine.browser_result(true).unwrap();
    assert!(engine.status == AuthStatus::Idle);
    assert!(engine.secrets.pending.is_none());
    assert!(!engine.loaded);
    assert!(
        !engine
            .callback(
                "antiburn-review://cloud/return?state=legacy",
                101,
                0,
                || true
            )
            .unwrap()
    );
    assert!(engine.identity.is_none());
    assert_eq!(engine.begin(102, 0).unwrap(), "https://antiburn.com/cloud");
    engine.browser_result(false).unwrap();
    assert!(engine.status == AuthStatus::BrowserFailed);
    assert_eq!(engine.begin(103, 0).unwrap(), "https://antiburn.com/cloud");
    engine.browser_result(true).unwrap();
    assert!(engine.status == AuthStatus::Idle);
}
#[test]
fn cloud_auth_browser_handoff_retains_waiting_and_cancel() {
    let (mut engine, vault) = engine(true);
    assert!(engine.begin(100, 0).unwrap().contains("code_challenge="));
    engine.browser_result(true).unwrap();
    assert!(engine.status == AuthStatus::Waiting);
    assert!(vault.read().unwrap().pending.is_some());
    engine.invalidate(AuthStatus::Cancelled).unwrap();
    assert!(engine.status == AuthStatus::Cancelled);
    assert!(vault.read().unwrap().pending.is_none());
}
#[test]
fn cloud_auth_load_discards_obsolete_marketing_attempt() {
    let (mut engine, vault) = engine(true);
    vault
        .write(&Secrets {
            pending: Some(Attempt {
                state: "legacy".into(),
                verifier: None,
                expires_at: 1800,
            }),
            refresh_token: None,
        })
        .unwrap();
    engine.load(100).unwrap();
    assert!(engine.status == AuthStatus::Idle);
    assert!(vault.read().unwrap().pending.is_none());
}
#[test]
fn cloud_rejects_wrong_route_state_duplicates_expiry_and_superseded_attempt() {
    let (mut engine, _) = engine(true);
    engine.begin(100, 0).unwrap();
    let state = engine.secrets.pending.as_ref().unwrap().state.clone();
    for callback in [
        format!("antiburn-review://cloud/return?state={state}"),
        format!("antiburn-review://cloud/auth?state={state}&state={state}&code=example"),
        "antiburn-review://cloud/auth?state=wrong&code=example".into(),
        format!("antiburn-review://cloud/auth?state={state}&code=example#bad"),
    ] {
        assert!(!engine.callback(&callback, 101, 0, || true).unwrap());
    }
    let callback = format!("antiburn-review://cloud/auth?state={state}&code=example");
    assert!(!engine.callback(&callback, 700, 0, || true).unwrap());
    engine.invalidate(AuthStatus::Cancelled).unwrap();
    engine.begin(102, 0).unwrap();
    assert!(!engine.callback(&callback, 103, 0, || true).unwrap());
}
#[test]
fn cloud_verifies_stores_rotates_and_signs_out() {
    let (mut engine, vault) = engine(true);
    engine.begin(100, 0).unwrap();
    let state = engine.secrets.pending.as_ref().unwrap().state.clone();
    assert!(
        engine
            .callback(
                &format!("antiburn-review://cloud/auth?state={state}&code=example"),
                101,
                0,
                || true
            )
            .unwrap()
    );
    assert!(matches!(engine.status, AuthStatus::SignedIn));
    assert_eq!(engine.identity.as_ref().unwrap().login, "example");
    assert_eq!(
        vault.read().unwrap().refresh_token.as_deref(),
        Some("synthetic-rotated")
    );
    engine.access = None;
    engine.refresh(102, || true).unwrap();
    assert!(engine.access.is_some());
    engine.sign_out().unwrap();
    assert!(vault.read().unwrap().refresh_token.is_none());
    assert!(engine.identity.is_none());
}
#[test]
fn cloud_restart_restores_unexpired_attempt_without_exposing_secrets() {
    let (mut original, vault) = engine(true);
    original.begin(100, 0).unwrap();
    let (mut reopened, _) = engine(true);
    reopened.vault = Box::new(vault);
    reopened.load(101).unwrap();
    assert!(matches!(reopened.status, AuthStatus::Waiting));
    assert!(reopened.identity.is_none());
    assert!(reopened.begin(102, 0).is_err());
}
#[test]
fn cloud_cancelled_exchange_cannot_restore_identity() {
    let (mut engine, vault) = engine(true);
    engine.begin(100, 0).unwrap();
    let state = engine.secrets.pending.as_ref().unwrap().state.clone();
    assert!(
        !engine
            .callback(
                &format!("antiburn-review://cloud/auth?state={state}&code=example"),
                101,
                0,
                || false
            )
            .unwrap()
    );
    assert!(engine.identity.is_none());
    assert!(vault.read().unwrap().refresh_token.is_none());
}

struct CancelDuringProfile(Arc<AtomicU64>);
impl Transport for CancelDuringProfile {
    fn exchange(&self, config: &Config, fields: &[(&str, &str)]) -> Result<auth::Tokens, Failure> {
        MockTransport.exchange(config, fields)
    }
    fn profile(&self, _: &Config, _: &str) -> Result<Identity, Failure> {
        self.0.store(1, Ordering::Release);
        Err(Failure::Offline)
    }
    fn revoke(&self, _: &Config, _: &str) -> Result<(), Failure> {
        Ok(())
    }
}
#[test]
fn cloud_cancellation_during_profile_clears_rotated_credentials_even_on_offline_error() {
    let (mut engine, vault) = engine(true);
    let generation = Arc::new(AtomicU64::new(0));
    engine.transport = Box::new(CancelDuringProfile(generation.clone()));
    engine.begin(100, 0).unwrap();
    let state = engine.secrets.pending.as_ref().unwrap().state.clone();
    engine
        .callback(
            &format!("antiburn-review://cloud/auth?state={state}&code=example"),
            101,
            0,
            || generation.load(Ordering::Acquire) == 0,
        )
        .unwrap();
    assert!(matches!(engine.status, AuthStatus::Cancelled));
    assert!(vault.read().unwrap().refresh_token.is_none());
    assert!(engine.identity.is_none());
}
#[derive(Default)]
struct FailingVault {
    value: Mutex<Secrets>,
    fail_read: AtomicBool,
    fail_write: AtomicBool,
}
impl Vault for Arc<FailingVault> {
    fn read(&self) -> Result<Secrets, Failure> {
        if self.fail_read.load(Ordering::Acquire) {
            return Err(Failure::CredentialFailed);
        }
        Ok(self.value.lock().unwrap().clone())
    }
    fn write(&self, value: &Secrets) -> Result<(), Failure> {
        if self.fail_write.load(Ordering::Acquire) {
            return Err(Failure::CredentialFailed);
        }
        *self.value.lock().unwrap() = value.clone();
        Ok(())
    }
}
#[test]
fn cloud_auth_retry_recovers_after_unlocking_the_vault() {
    let (mut engine, _) = engine(true);
    let vault = Arc::new(FailingVault {
        fail_read: AtomicBool::new(true),
        ..FailingVault::default()
    });
    engine.vault = Box::new(vault.clone());

    let error = engine.begin(100, 0).unwrap_err();
    assert_eq!(error, Failure::CredentialFailed);
    engine.status = error.into();
    assert_eq!(
        engine
            .load(101)
            .and_then(|()| engine.invalidate(AuthStatus::Cancelled)),
        Err(Failure::CredentialFailed)
    );

    vault.fail_read.store(false, Ordering::Release);
    engine
        .load(102)
        .and_then(|()| engine.invalidate(AuthStatus::Cancelled))
        .unwrap();
    assert!(engine.status == AuthStatus::Idle);
    assert!(engine.identity.is_none());
    assert!(engine.begin(103, 2).unwrap().contains("code_challenge="));
    assert!(engine.status == AuthStatus::Opening);
}
#[test]
fn cloud_failed_signout_retains_credential_for_retry_and_never_claims_success() {
    let (mut engine, _) = engine(true);
    let vault = Arc::new(FailingVault {
        value: Mutex::new(Secrets {
            pending: None,
            refresh_token: Some("synthetic".into()),
        }),
        fail_write: AtomicBool::new(true),
        ..FailingVault::default()
    });
    engine.vault = Box::new(vault.clone());
    assert_eq!(engine.sign_out(), Err(Failure::CredentialFailed));
    assert_eq!(engine.secrets.refresh_token.as_deref(), Some("synthetic"));
    assert!(!matches!(engine.status, AuthStatus::SignedOut));
    vault.fail_write.store(false, Ordering::Release);
    engine.sign_out().unwrap();
    assert!(vault.read().unwrap().refresh_token.is_none());
}
#[test]
fn cloud_http_transport_rejects_redirects_and_bounds_response() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let size = stream.read(&mut request).unwrap();
        let request = std::str::from_utf8(&request[..size]).unwrap();
        assert!(request.starts_with("POST /token"));
        stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: https://example.invalid/should-not-follow\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let mut config = config();
    config.token = Url::parse(&format!("http://{address}/token")).unwrap();
    assert!(matches!(
        HttpTransport.exchange(
            &config,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", "synthetic")
            ]
        ),
        Err(Failure::Offline)
    ));
    server.join().unwrap();
}
#[test]
fn cloud_mock_server_exchanges_pkce_and_verifies_profile_before_identity() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (mut engine, vault) = engine(true);
    let config = engine.config.as_mut().unwrap();
    config.token = Url::parse(&format!("http://{address}/token")).unwrap();
    config.profile = Url::parse(&format!("http://{address}/profile")).unwrap();
    engine.transport = Box::new(HttpTransport);
    engine.begin(100, 0).unwrap();
    let attempt = engine.secrets.pending.clone().unwrap();
    let verifier = attempt.verifier.unwrap();
    let server = std::thread::spawn(move || {
        for index in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0u8; 1024];
                let read = stream.read(&mut chunk).unwrap();
                if read == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..read]);
                let text = String::from_utf8_lossy(&bytes);
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if body.len() >= length {
                        break;
                    }
                }
            }
            let request = String::from_utf8(bytes).unwrap();
            let body = if index == 0 {
                assert!(request.starts_with("POST /token"));
                assert!(request.contains(&format!("code_verifier={verifier}")));
                assert!(request.contains("code=single-use-example"));
                r#"{"access_token":"synthetic-access","refresh_token":"synthetic-refresh","expires_in":600,"token_type":"Bearer"}"#
            } else {
                assert!(request.starts_with("GET /profile"));
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer synthetic-access")
                );
                r#"{"id":"synthetic-id","login":"example"}"#
            };
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    let callback = format!(
        "antiburn-review://cloud/auth?state={}&code=single-use-example",
        attempt.state
    );
    assert!(engine.callback(&callback, 101, 0, || true).unwrap());
    assert_eq!(engine.identity.as_ref().unwrap().login, "example");
    assert!(vault.read().unwrap().pending.is_none());
    assert!(!engine.callback(&callback, 102, 0, || true).unwrap());
    server.join().unwrap();
}
struct OfflineProfile;
impl Transport for OfflineProfile {
    fn exchange(&self, config: &Config, fields: &[(&str, &str)]) -> Result<auth::Tokens, Failure> {
        MockTransport.exchange(config, fields)
    }
    fn profile(&self, _: &Config, _: &str) -> Result<Identity, Failure> {
        Err(Failure::Offline)
    }
    fn revoke(&self, _: &Config, _: &str) -> Result<(), Failure> {
        Err(Failure::Offline)
    }
}
#[test]
fn cloud_offline_initial_profile_never_persists_an_unverified_login() {
    let (mut engine, vault) = engine(true);
    engine.transport = Box::new(OfflineProfile);
    engine.begin(100, 0).unwrap();
    let state = engine.secrets.pending.as_ref().unwrap().state.clone();
    engine
        .callback(
            &format!("antiburn-review://cloud/auth?state={state}&code=example"),
            101,
            0,
            || true,
        )
        .unwrap();
    assert!(matches!(engine.status, AuthStatus::Offline));
    assert!(vault.read().unwrap().refresh_token.is_none());
    assert!(engine.identity.is_none());
}
#[test]
fn cloud_offline_profile_preserves_rotation_but_signout_clears_despite_remote_failure() {
    let (mut engine, vault) = engine(true);
    engine.load(100).unwrap();
    engine.secrets.refresh_token = Some("synthetic-previous".into());
    engine.vault.write(&engine.secrets).unwrap();
    engine.transport = Box::new(OfflineProfile);
    assert_eq!(engine.refresh(101, || true), Err(Failure::Offline));
    assert_eq!(
        vault.read().unwrap().refresh_token.as_deref(),
        Some("synthetic-rotated")
    );
    assert!(engine.identity.is_none());
    engine.sign_out().unwrap();
    assert!(vault.read().unwrap().refresh_token.is_none());
    assert!(matches!(engine.status, AuthStatus::SignedOut));
}
#[test]
fn cloud_cancellation_during_refresh_profile_clears_the_rotated_session() {
    let (mut engine, vault) = engine(true);
    engine.load(100).unwrap();
    engine.secrets.refresh_token = Some("synthetic-previous".into());
    engine.vault.write(&engine.secrets).unwrap();
    let generation = Arc::new(AtomicU64::new(0));
    engine.transport = Box::new(CancelDuringProfile(generation.clone()));
    assert_eq!(
        engine.refresh(101, || generation.load(Ordering::Acquire) == 0),
        Err(Failure::Cancelled)
    );
    assert!(vault.read().unwrap().refresh_token.is_none());
    assert!(engine.identity.is_none());
    assert!(engine.access.is_none());
}
#[test]
fn cloud_account_snapshots_exclude_folder_drafts_outside_onboarding() {
    let snapshot = Snapshot {
        auth_enabled: true,
        sync_enabled: false,
        status: AuthStatus::SignedIn,
        identity: Some(Identity {
            id: "synthetic".into(),
            login: "example".into(),
        }),
        expires_at: None,
        draft: Draft {
            screen: Screen::Folders,
            folders_initialized: true,
            folders: vec![Folder {
                path: "/Example/Private".into(),
                selected: true,
                session_count: Some(12),
                session_count_is_partial: false,
            }],
        },
    };
    let invalidated = snapshot_for_account(snapshot.clone(), 0, 1);
    assert!(invalidated.identity.is_none());
    assert!(matches!(invalidated.status, AuthStatus::Unavailable));
    assert!(matches!(invalidated.draft.screen, Screen::Handoff));
    assert!(
        snapshot_for_account(snapshot.clone(), 1, 1)
            .identity
            .is_some()
    );
    let mut failed_storage = snapshot.clone();
    failed_storage.status = AuthStatus::CredentialFailed;
    let failed_storage = snapshot_for_account(failed_storage, 0, 1);
    assert!(failed_storage.identity.is_none());
    assert!(matches!(
        failed_storage.status,
        AuthStatus::CredentialFailed
    ));
    for window in ["main", "settings"] {
        let account = snapshot_for_window(snapshot.clone(), window);
        assert!(account.draft.folders.is_empty());
        let json = serde_json::to_string(&account).unwrap();
        for forbidden in [
            "Private",
            "refresh_token",
            "access_token",
            "verifier",
            "callback",
        ] {
            assert!(!json.contains(forbidden));
        }
    }
    assert_eq!(
        snapshot_for_window(snapshot, "onboarding")
            .draft
            .folders
            .len(),
        1
    );
}
#[test]
fn cloud_callback_after_cancel_epoch_cannot_consume_the_uncleared_attempt() {
    let (mut engine, vault) = engine(true);
    engine.begin(100, 0).unwrap();
    let state = engine.secrets.pending.as_ref().unwrap().state.clone();
    let callback = format!("antiburn-review://cloud/auth?state={state}&code=example");
    // The cancel epoch advances before its worker acquires the engine lock.
    assert!(!engine.callback(&callback, 101, 1, || true).unwrap());
    assert!(vault.read().unwrap().pending.is_some());
    assert!(engine.identity.is_none());
    let (mut restored, _) = self::engine(true);
    restored.vault = Box::new(vault);
    assert!(!restored.callback(&callback, 101, 1, || true).unwrap());
    assert!(restored.identity.is_none());
    engine.invalidate(AuthStatus::Cancelled).unwrap();
    engine.begin(102, 1).unwrap();
    let state = engine.secrets.pending.as_ref().unwrap().state.clone();
    assert!(
        engine
            .callback(
                &format!("antiburn-review://cloud/auth?state={state}&code=example"),
                103,
                1,
                || true
            )
            .unwrap()
    );
}
#[test]
fn cloud_cleanup_without_an_attempt_preserves_verified_and_returned_status() {
    let (mut engine, _) = engine(true);
    for status in [AuthStatus::SignedIn, AuthStatus::Returned] {
        engine.status = status;
        engine.invalidate(AuthStatus::Cancelled).unwrap();
        assert!(engine.status == status);
    }
}
struct BrowserCancelDuringProfile(Arc<AtomicU64>);
impl Transport for BrowserCancelDuringProfile {
    fn exchange(&self, config: &Config, fields: &[(&str, &str)]) -> Result<auth::Tokens, Failure> {
        MockTransport.exchange(config, fields)
    }
    fn profile(&self, config: &Config, access: &str) -> Result<Identity, Failure> {
        self.0.fetch_add(1, Ordering::AcqRel);
        MockTransport.profile(config, access)
    }
    fn revoke(&self, _: &Config, _: &str) -> Result<(), Failure> {
        panic!("Browser cancellation must not revoke an established session")
    }
}
#[test]
fn cloud_browser_cancellation_does_not_invalidate_established_account_refresh() {
    let (mut engine, vault) = engine(true);
    engine.load(100).unwrap();
    engine.secrets.refresh_token = Some("synthetic-previous".into());
    engine.vault.write(&engine.secrets).unwrap();
    let browser_generation = Arc::new(AtomicU64::new(0));
    let account_generation = AtomicU64::new(0);
    engine.transport = Box::new(BrowserCancelDuringProfile(browser_generation.clone()));
    engine
        .refresh(101, || account_generation.load(Ordering::Acquire) == 0)
        .unwrap();
    engine.invalidate(AuthStatus::Cancelled).unwrap();
    assert_eq!(browser_generation.load(Ordering::Acquire), 1);
    assert!(matches!(engine.status, AuthStatus::SignedIn));
    assert!(engine.identity.is_some());
    assert_eq!(
        vault.read().unwrap().refresh_token.as_deref(),
        Some("synthetic-rotated")
    );
}
