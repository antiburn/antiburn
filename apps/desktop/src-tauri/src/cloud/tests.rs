use super::*;
use auth::{Credential, Session, TokenRequest, Tokens, Transport, Vault, WorkspaceKind};
use std::sync::{Arc, atomic::AtomicBool};

#[test]
fn cloud_source_preferences_keep_legacy_shapes_and_reject_unknown_providers() {
    let raw = r#"{"path":"/Example/claude/projects/project","selected":true}"#;
    let legacy: Folder = serde_json::from_str(raw).unwrap();
    assert!(legacy.source_kind.is_claude());
    assert_eq!(serde_json::to_string(&legacy).unwrap(), raw);
    let database: Folder = serde_json::from_str(
        r#"{"path":"/Example/opencode/opencode.db","selected":true,"sourceKind":"opencode_sqlite_v2"}"#
    ).unwrap();
    assert_eq!(database.source_kind, SourceKind::OpenCodeSqliteV2);
    assert!(
        serde_json::from_str::<Folder>(
            r#"{"path":"/Example/source","selected":false,"sourceKind":"unknown"}"#
        )
        .is_err()
    );
}

#[test]
fn cloud_startup_and_onboarding_prereads_use_the_worker_boundary() {
    let source = include_str!("mod.rs");
    let setup = source
        .split_once("pub fn initialize(")
        .unwrap()
        .1
        .split_once("\n}")
        .unwrap()
        .0;
    assert!(!setup.contains("initialize_cloud_setup"));
    let begin = source
        .split_once("pub async fn cloud_begin(")
        .unwrap()
        .1
        .split_once("\n}")
        .unwrap()
        .0;
    assert!(
        begin.find("work.run(Priority::Normal").unwrap()
            < begin.find(".compare_exchange(").unwrap()
    );
    assert!(
        begin.find("let expected_generation =").unwrap()
            < begin.find("work.run(Priority::Normal").unwrap()
    );
    assert!(!begin.contains("fetch_add("));
    let preread = source
        .split_once("pub(crate) async fn onboarding_requires_cancel(")
        .unwrap()
        .1;
    let admission = preread.find("work.run(Priority::Cleanup").unwrap();
    let read = preread.find(".onboarding_state()").unwrap();
    assert!(admission < read);
    let commands = include_str!("../commands/mod.rs");
    let onboarding = commands
        .split_once("pub async fn set_onboarding_state(")
        .unwrap()
        .1
        .split_once("\n}")
        .unwrap()
        .0;
    assert!(onboarding.contains(
        "if let Ok(true) = crate::cloud::onboarding_requires_cancel(app.clone(), state).await"
    ));
    assert!(!onboarding.contains("onboarding_requires_cancel(app.clone(), state).await?"));
    assert!(!onboarding.contains(".onboarding_state()"));
}

#[test]
fn cloud_snapshot_fences_account_changes_after_blocking_store_reads() {
    let source = include_str!("mod.rs");
    let snapshot = source
        .split_once("fn snapshot(")
        .unwrap()
        .1
        .split_once("\n}")
        .unwrap()
        .0;
    let read = snapshot.find(".current_cloud_capture_request(").unwrap();
    let fence = snapshot.find("let current = current();").unwrap();
    let identity = snapshot.find("identity: session.map(").unwrap();
    assert!(read < fence && fence < identity);
    assert!(snapshot.contains("draft = Draft::default()"));
    let expiry = snapshot
        .find("session.filter(|_| current && engine.session().is_some())")
        .unwrap();
    assert!(read < expiry && expiry < identity);
    assert!(snapshot.contains("pending_sync.filter(|_| current && session.is_some())"));
    assert!(snapshot.contains("if !current || session.is_none()"));
    assert!(snapshot.contains("engine.status == AuthStatus::SignedIn && session.is_none()"));
}

/// Synthetic secure-store substitute shared by desktop engine and HTTP boundary tests.
#[derive(Default)]
pub(super) struct MemoryVault {
    value: Mutex<Option<Credential>>,
    fail_read: AtomicBool,
    fail_write: AtomicBool,
    on_credential_write: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Vault for Arc<MemoryVault> {
    fn read(&self) -> Result<Option<Credential>, Failure> {
        if self.fail_read.load(Ordering::Acquire) {
            return Err(Failure::CredentialFailed);
        }
        Ok(self.value.lock().unwrap().clone())
    }
    fn write(&self, value: Option<&Credential>) -> Result<(), Failure> {
        if self.fail_write.load(Ordering::Acquire) {
            return Err(Failure::CredentialFailed);
        }
        *self.value.lock().unwrap() = value.cloned();
        if value.is_some_and(|credential| credential.session.is_some())
            && let Some(action) = &self.on_credential_write
        {
            action();
        }
        Ok(())
    }
}

fn config() -> Config {
    Config::parse("https://api.example", "https://api.example").unwrap()
}

/// Supply synthetic desktop-grant authority without reading credentials or user content.
pub(super) fn session() -> Session {
    Session {
        user: auth::User {
            id: "synthetic-user".into(),
            display_name: "Example".into(),
        },
        grant_id: "synthetic-grant".into(),
        device_name: "Example desktop".into(),
        workspace: Workspace {
            id: "synthetic-personal".into(),
            name: "Personal".into(),
            kind: WorkspaceKind::Personal,
        },
        resource: "https://api.example".into(),
        scope: "capture:write".into(),
        iss: "https://api.example".into(),
        expires_at: 10_000,
        access_expires_at: 901,
    }
}

#[derive(Default)]
struct FakeTransport {
    calls: AtomicU64,
    fail_exchange: AtomicBool,
    fail_before_send: AtomicBool,
    fail_revoke: AtomicBool,
    fail_session: AtomicBool,
    generation: Option<Arc<AtomicU64>>,
    on_exchange: Option<Arc<dyn Fn() + Send + Sync>>,
    on_session: Option<Arc<dyn Fn() + Send + Sync>>,
    returned_session: Mutex<Option<Session>>,
    revocations: AtomicU64,
    revoked: Mutex<Vec<String>>,
    token_grant: Mutex<Option<String>>,
    token_expires_in: Mutex<Option<u64>>,
    fail_revoke_for: Mutex<Option<String>>,
    seen: Mutex<Vec<String>>,
}

impl Transport for Arc<FakeTransport> {
    fn exchange(&self, _: &Config, request: &TokenRequest<'_>) -> Result<Tokens, Failure> {
        if let Some(action) = &self.on_exchange {
            action();
        }
        if self.fail_before_send.load(Ordering::Acquire) {
            return Err(Failure::OfflineBeforeSend);
        }
        let count = self.calls.fetch_add(1, Ordering::AcqRel);
        self.seen.lock().unwrap().push(
            match request {
                TokenRequest::AuthorizationCode { .. } => "code",
                TokenRequest::RefreshToken { refresh_token } => refresh_token,
            }
            .to_string(),
        );
        if self.fail_exchange.load(Ordering::Acquire) {
            return Err(Failure::Offline);
        }
        Ok(Tokens {
            access_token: format!("synthetic-access-{count}"),
            refresh_token: format!("synthetic-refresh-{count}"),
            expires_in: self.token_expires_in.lock().unwrap().unwrap_or(60),
            token_type: "Bearer".into(),
            grant_id: self
                .token_grant
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| session().grant_id),
            user_id: session().user.id,
            workspace_id: session().workspace.id,
            grant_expires_at: "1970-01-01T02:46:40Z".into(),
            scope: auth::SCOPE.into(),
            resource: config().resource,
            iss: config().issuer,
        })
    }
    fn session(&self, _: &Config, _: &str) -> Result<Session, Failure> {
        if let Some(action) = &self.on_session {
            action();
        }
        if let Some(generation) = &self.generation {
            generation.fetch_add(1, Ordering::AcqRel);
        }
        if self.fail_session.load(Ordering::Acquire) {
            return Err(Failure::Offline);
        }
        Ok(self
            .returned_session
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(session))
    }
    fn revoke(&self, _: &Config, refresh: &str) -> Result<(), Failure> {
        self.revocations.fetch_add(1, Ordering::AcqRel);
        self.revoked.lock().unwrap().push(refresh.to_owned());
        if self.fail_revoke.load(Ordering::Acquire)
            || self.fail_revoke_for.lock().unwrap().as_deref() == Some(refresh)
        {
            return Err(Failure::Offline);
        }
        Ok(())
    }
}

fn engine(directory: &Path, vault: Arc<MemoryVault>, transport: Arc<FakeTransport>) -> Engine {
    Engine::new(
        Some(config()),
        Box::new(vault),
        Box::new(transport),
        state::StateStore::new(directory),
    )
    .with_clock(|| 100)
}

struct Fixture {
    directory: tempfile::TempDir,
    vault: Arc<MemoryVault>,
    transport: Arc<FakeTransport>,
    engine: Engine,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let vault = Arc::new(MemoryVault::default());
        let transport = Arc::new(FakeTransport::default());
        let engine = engine(directory.path(), vault.clone(), transport.clone());
        Self {
            directory,
            vault,
            transport,
            engine,
        }
    }
    fn sign_in(&mut self) {
        let (_, _listener, attempt) = self.engine.begin(100, 0).unwrap();
        self.engine
            .callback(
                &attempt,
                auth::Callback::Code("synthetic-code".into()),
                101,
                || true,
            )
            .unwrap();
    }
    fn reopen(&self) -> Engine {
        engine(
            self.directory.path(),
            self.vault.clone(),
            self.transport.clone(),
        )
    }
    fn restart(&mut self) {
        self.engine = self.reopen();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cloud_busy_begin_preserves_the_pending_attempt_and_its_callback() {
    let work = BlockingWork::default();
    let running = work.clone();
    let generation = Arc::new(AtomicU64::new(7));
    let active_generation = generation.clone();
    let (started, start) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let first = tokio::spawn(async move {
        running
            .run(Priority::Normal, move || {
                let mut fixture = Fixture::new();
                let (_, _listener, attempt) = fixture.engine.begin(100, 7).unwrap();
                started.send(()).unwrap();
                wait.recv_timeout(std::time::Duration::from_secs(5))
                    .map_err(|_| "test release".to_owned())?;
                assert_eq!(active_generation.load(Ordering::Acquire), 7);
                assert_eq!(
                    fixture.engine.attempt.as_ref().unwrap().epoch,
                    attempt.epoch
                );
                fixture
                    .engine
                    .callback(
                        &attempt,
                        auth::Callback::Code("synthetic-code".into()),
                        101,
                        || active_generation.load(Ordering::Acquire) == 7,
                    )
                    .unwrap();
                assert!(fixture.engine.session().is_some());
                Ok(())
            })
            .await
    });
    start.await.unwrap();
    assert_eq!(
        work.run(Priority::Normal, move || {
            generation.fetch_add(1, Ordering::AcqRel);
            Ok(())
        })
        .await,
        Err(work::BUSY.into())
    );
    release.send(()).unwrap();
    first.await.unwrap().unwrap();
}

#[test]
fn cloud_pkce_matches_rfc7636() {
    assert_eq!(
        auth::challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn cloud_configuration_pins_public_routes_and_rejects_noncanonical_inputs() {
    assert_eq!(config().issuer, "https://api.example");
    assert_eq!(config().resource, "https://api.example");
    assert_eq!(
        config().authorization.as_str(),
        "https://api.example/desktop/authorize"
    );
    assert_eq!(
        config().session.as_str(),
        "https://api.example/v1/desktop/session"
    );
    for origin in [
        "http://api.example",
        "https://api.example/",
        "https://API.example",
        "https://api.example:443",
        "https://u@api.example",
        "https://api.example?q=x",
        "https://api.example#x",
        "https://api.example/path",
    ] {
        assert!(Config::parse(origin, "https://api.example").is_none());
        assert!(Config::parse("https://api.example", origin).is_none());
    }
}

#[test]
fn cloud_authorize_uses_only_first_party_fields_and_literal_ephemeral_loopback() {
    let mut fixture = Fixture::new();
    let (url, listener, attempt) = fixture.engine.begin(100, 0).unwrap();
    let url = url::Url::parse(&url).unwrap();
    let fields = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(url.origin().ascii_serialization(), "https://api.example");
    assert_eq!(fields.len(), 5);
    assert_eq!(fields["device_name"], auth::DEVICE_NAME);
    assert_eq!(fields["code_challenge_method"], "S256");
    let callback = url::Url::parse(&fields["redirect_uri"]).unwrap();
    assert_eq!(callback.host_str(), Some("127.0.0.1"));
    assert!(callback.port().unwrap() > 0);
    assert_eq!(callback.path(), "/cloud/callback");
    assert_eq!(listener.callback(), attempt.callback);
    let stored =
        std::fs::read_to_string(fixture.directory.path().join("cloud-auth-state-v2")).unwrap();
    for secret in [&attempt.state, &attempt.verifier] {
        assert!(!stored.contains(secret));
    }
}

#[test]
fn cloud_callback_guards_exact_route_host_state_issuer_and_single_values() {
    let mut fixture = Fixture::new();
    let (_, _listener, attempt) = fixture.engine.begin(100, 0).unwrap();
    let host = attempt
        .callback
        .trim_start_matches("http://")
        .trim_end_matches("/cloud/callback");
    let good = format!(
        "/cloud/callback?state={}&code=synthetic&iss=https%3A%2F%2Fapi.example",
        attempt.state
    );
    assert!(auth::validate_callback(&good, host, &config(), &attempt, 101).is_some());
    for bad in [
        good.replace("/cloud/callback?", "/cloud/callback/?"),
        good.replace("/cloud/callback?", "/cloud/%63allback?"),
        good.replace("api.example", "other.example"),
        format!("{good}%2F"),
        format!("{good}&state={}", attempt.state),
        format!("{good}&code=again"),
        format!("{good}&unknown=value"),
        format!("{good}#fragment"),
        format!("http://{host}{good}"),
        good.replace(&attempt.state, "wrong"),
    ] {
        assert!(
            auth::validate_callback(&bad, host, &config(), &attempt, 101).is_none(),
            "{bad}"
        );
    }
    assert!(auth::validate_callback(&good, "localhost:1234", &config(), &attempt, 101).is_none());
    assert!(auth::validate_callback(&good, host, &config(), &attempt, 700).is_none());
}

#[test]
fn cloud_cancel_and_late_callback_cannot_restore_identity() {
    let mut fixture = Fixture::new();
    let (_, _listener, attempt) = fixture.engine.begin(100, 0).unwrap();
    fixture.engine.cancel(1).unwrap();
    assert_eq!(
        fixture
            .engine
            .callback(&attempt, auth::Callback::Code("late".into()), 101, || true),
        Err(Failure::Cancelled)
    );
    assert!(fixture.engine.session().is_none());
    assert!(fixture.vault.read().unwrap().is_none());
}

#[test]
fn cloud_browser_prelaunch_abort_requires_a_native_missing_launcher_error() {
    use std::io::{Error, ErrorKind};
    use tauri_plugin_opener::Error as OpenerError;

    assert!(browser_launch_did_not_start(&OpenerError::Io(
        Error::from_raw_os_error(2)
    )));
    for error in [
        Error::from(ErrorKind::NotFound),
        Error::from(ErrorKind::PermissionDenied),
        Error::other("launcher exited after handoff"),
    ] {
        assert!(!browser_launch_did_not_start(&OpenerError::Io(error)));
    }
}

#[test]
fn cloud_browser_prelaunch_abort_allows_retry_and_preserves_prior_warnings() {
    for prior_warning in [false, true] {
        let mut fixture = Fixture::new();
        if prior_warning {
            fixture
                .engine
                .state
                .lock()
                .unwrap()
                .replace(state::Phase::SignedOut, state::Cleanup::RemoteUnconfirmed)
                .unwrap();
        }
        let (_, listener, attempt) = fixture.engine.begin(100, 1).unwrap();
        drop(listener);
        fixture.engine.abort_browser_launch(&attempt).unwrap();
        assert!(fixture.engine.status == AuthStatus::BrowserFailed);
        assert!(fixture.engine.attempt.is_none());
        assert!(!fixture.engine.cleanup.blocks_connection());
        assert_eq!(fixture.engine.cleanup.prior_unconfirmed(), prior_warning);
        assert_eq!(fixture.transport.calls.load(Ordering::Acquire), 0);
        assert!(fixture.vault.read().unwrap().is_none());
        fixture.restart();
        fixture.engine.refresh(101, || true).unwrap();
        assert!(fixture.engine.status == AuthStatus::SignedOut);
        let (_, _listener, newer) = fixture.engine.begin(102, 2).unwrap();
        assert_ne!(attempt.epoch, newer.epoch);
        assert_eq!(
            fixture
                .engine
                .callback(&attempt, auth::Callback::Code("late".into()), 103, || true),
            Err(Failure::Cancelled)
        );
        fixture.engine.abort_browser_launch(&attempt).unwrap();
        assert_eq!(fixture.engine.attempt.as_ref().unwrap().epoch, newer.epoch);
        fixture
            .engine
            .callback(&newer, auth::Callback::Code("current".into()), 103, || true)
            .unwrap();
        fixture.engine.abort_browser_launch(&attempt).unwrap();
        assert!(fixture.engine.session().is_some());
        assert_eq!(fixture.engine.cleanup.prior_unconfirmed(), prior_warning);
    }
}

#[test]
fn cloud_cancel_during_session_request_never_saves_credentials() {
    let mut fixture = Fixture::new();
    let generation = Arc::new(AtomicU64::new(0));
    fixture.engine.transport = Box::new(Arc::new(FakeTransport {
        generation: Some(generation.clone()),
        ..FakeTransport::default()
    }));
    let (_, _listener, attempt) = fixture.engine.begin(100, 0).unwrap();
    assert_eq!(
        fixture.engine.callback(
            &attempt,
            auth::Callback::Code("synthetic".into()),
            101,
            || generation.load(Ordering::Acquire) == 0
        ),
        Err(Failure::Cancelled)
    );
    assert!(fixture.engine.session().is_none());
    assert!(fixture.vault.read().unwrap().is_none());
    fixture.restart();
    assert!(fixture.engine.refresh(102, || true).is_ok());
}

#[test]
fn cloud_restart_does_not_restore_an_ephemeral_listener() {
    let mut fixture = Fixture::new();
    let (_, listener, _) = fixture.engine.begin(100, 0).unwrap();
    drop(listener);
    fixture.restart();
    fixture.engine.refresh(101, || true).unwrap();
    assert!(fixture.engine.status == AuthStatus::RevocationUnconfirmed);
    assert!(fixture.engine.local_reset_available);
    assert!(fixture.engine.session().is_none());
}

#[test]
fn cloud_abandoned_browser_approval_needs_explicit_reset_without_losing_its_warning() {
    let mut fixture = Fixture::new();
    let (_, _listener, _) = fixture.engine.begin(100, 0).unwrap();
    fixture.engine.cancel(1).unwrap();
    assert!(fixture.engine.status == AuthStatus::RevocationUnconfirmed);
    assert!(fixture.engine.local_reset_available);
    assert!(matches!(
        fixture.engine.begin(101, 1),
        Err(Failure::Unavailable)
    ));
    fixture.engine.reset_local().unwrap();
    fixture.engine.begin(102, 2).unwrap();
    assert!(fixture.engine.cleanup == state::Cleanup::RemoteUnconfirmed);
}

#[test]
fn cloud_expired_or_replaced_browser_attempt_does_not_claim_remote_revocation() {
    for replaced in [false, true] {
        let mut fixture = Fixture::new();
        let (_, _listener, _) = fixture.engine.begin(100, 0).unwrap();
        if replaced {
            assert!(matches!(
                fixture.engine.begin(101, 1),
                Err(Failure::ReauthRequired)
            ));
        } else {
            fixture.engine.refresh(700, || true).unwrap();
        }
        assert!(
            fixture.engine.cleanup
                == state::Cleanup::Revoke {
                    prior_unconfirmed: false
                }
        );
        assert!(fixture.engine.local_reset_available);
        assert!(fixture.engine.session().is_none());
    }
}

#[test]
fn cloud_browser_denial_does_not_need_reset_but_native_pending_logout_does() {
    let mut fixture = Fixture::new();
    let (_, _listener, attempt) = fixture.engine.begin(100, 0).unwrap();
    fixture
        .engine
        .callback(&attempt, auth::Callback::Cancelled, 101, || true)
        .unwrap();
    assert!(fixture.engine.cleanup == state::Cleanup::None);
    fixture.engine.begin(102, 1).unwrap();
    fixture.engine.sign_out().unwrap();
    assert!(fixture.engine.status == AuthStatus::RevocationUnconfirmed);
    assert!(fixture.engine.local_reset_available);
}

#[test]
fn cloud_rotates_under_shared_state_and_keeps_access_only_in_memory() {
    let mut fixture = Fixture::new();
    fixture.sign_in();
    assert!(fixture.engine.session().is_some());
    fixture.restart();
    fixture.engine.refresh(102, || true).unwrap();
    assert!(fixture.engine.session().is_some());
    assert_eq!(
        fixture.vault.read().unwrap().unwrap().refresh_token,
        "synthetic-refresh-1"
    );
    fixture.engine.refresh(200, || true).unwrap();
    assert_eq!(
        *fixture.transport.seen.lock().unwrap(),
        ["code", "synthetic-refresh-0", "synthetic-refresh-1"]
    );
    let stored =
        std::fs::read_to_string(fixture.directory.path().join("cloud-auth-state-v2")).unwrap();
    assert!(!stored.contains("access"));
    assert!(!stored.contains("refresh"));
}

#[test]
fn cloud_expired_exchange_session_or_vault_work_never_installs_access() {
    use std::sync::atomic::AtomicI64;

    for refresh in [false, true] {
        for stage in ["exchange", "session", "vault"] {
            let mut fixture = Fixture::new();
            if refresh {
                fixture.sign_in();
            }
            let started_at = if refresh { 200 } else { 101 };
            let clock = Arc::new(AtomicI64::new(started_at));
            let read_clock = clock.clone();
            fixture.engine = fixture
                .engine
                .with_clock(move || read_clock.load(Ordering::Acquire));
            let expire: Arc<dyn Fn() + Send + Sync> =
                Arc::new(move || clock.store(started_at + 1, Ordering::Release));
            fixture.transport = Arc::new(FakeTransport {
                calls: AtomicU64::new(u64::from(refresh)),
                token_expires_in: Mutex::new(Some(1)),
                on_exchange: (stage == "exchange").then(|| expire.clone()),
                on_session: (stage == "session").then(|| expire.clone()),
                fail_revoke: AtomicBool::new(true),
                ..FakeTransport::default()
            });
            fixture.engine.transport = Box::new(fixture.transport.clone());
            fixture.vault = Arc::new(MemoryVault {
                value: Mutex::new(fixture.vault.read().unwrap()),
                on_credential_write: (stage == "vault").then(|| expire.clone()),
                ..MemoryVault::default()
            });
            fixture.engine.vault = Box::new(fixture.vault.clone());
            let result = if refresh {
                fixture.engine.refresh(started_at, || true)
            } else {
                let (_, _listener, attempt) = fixture.engine.begin(100, 0).unwrap();
                fixture.engine.callback(
                    &attempt,
                    auth::Callback::Code("synthetic".into()),
                    started_at,
                    || true,
                )
            };
            assert_eq!(result, Err(Failure::ReauthRequired), "{refresh}/{stage}");
            assert!(fixture.engine.session().is_none(), "{refresh}/{stage}");
            let cleanup = fixture.vault.read().unwrap().unwrap();
            assert!(cleanup.session.is_none(), "{refresh}/{stage}");
            assert_eq!(cleanup.refresh_token, "synthetic-refresh-0");
            assert_eq!(
                cleanup.cleanup_refresh_tokens,
                if refresh {
                    vec!["synthetic-refresh-1"]
                } else {
                    Vec::new()
                }
            );
            assert!(fixture.engine.cleanup.blocks_connection());
            fixture.restart();
            fixture.engine.refresh(started_at + 2, || true).unwrap();
            assert!(fixture.engine.session().is_none());
            assert_eq!(
                fixture.transport.calls.load(Ordering::Acquire),
                1 + u64::from(refresh)
            );
            fixture
                .transport
                .fail_revoke
                .store(false, Ordering::Release);
            fixture.engine.sign_out().unwrap();
            assert!(fixture.vault.read().unwrap().is_none());
        }
    }
}

#[test]
fn cloud_vault_work_cannot_outlast_session_or_fixed_grant_deadlines() {
    use std::sync::atomic::AtomicI64;

    for fixed_grant in [false, true] {
        let mut fixture = Fixture::new();
        let started_at = if fixed_grant { 9_900 } else { 101 };
        let expires_at = if fixed_grant { 10_000 } else { 102 };
        let clock = Arc::new(AtomicI64::new(started_at));
        let read_clock = clock.clone();
        fixture.engine = fixture
            .engine
            .with_clock(move || read_clock.load(Ordering::Acquire));
        let mut authority = session();
        authority.access_expires_at = expires_at;
        fixture.transport = Arc::new(FakeTransport {
            token_expires_in: Mutex::new(Some(900)),
            returned_session: Mutex::new(Some(authority)),
            fail_revoke: AtomicBool::new(true),
            ..FakeTransport::default()
        });
        fixture.engine.transport = Box::new(fixture.transport.clone());
        fixture.vault = Arc::new(MemoryVault {
            on_credential_write: Some(Arc::new(move || {
                clock.store(expires_at, Ordering::Release);
            })),
            ..MemoryVault::default()
        });
        fixture.engine.vault = Box::new(fixture.vault.clone());
        let (_, _listener, attempt) = fixture.engine.begin(started_at - 1, 0).unwrap();
        assert_eq!(
            fixture.engine.callback(
                &attempt,
                auth::Callback::Code("synthetic".into()),
                started_at,
                || true,
            ),
            Err(Failure::ReauthRequired)
        );
        assert!(fixture.engine.session().is_none());
        assert!(fixture.vault.read().unwrap().unwrap().session.is_none());
        assert!(fixture.engine.cleanup.blocks_connection());
    }
}

#[test]
fn cloud_cached_session_hides_expired_token_session_and_grant_authority() {
    use std::sync::atomic::AtomicI64;

    for (started_at, expires_in, access_expires_at, expires_at) in [
        (101, 1, 901, 102),
        (101, 900, 102, 102),
        (9_900, 900, 10_000, 10_000),
    ] {
        let mut fixture = Fixture::new();
        let clock = Arc::new(AtomicI64::new(started_at));
        let read_clock = clock.clone();
        fixture.engine = fixture
            .engine
            .with_clock(move || read_clock.load(Ordering::Acquire));
        let mut authority = session();
        authority.access_expires_at = access_expires_at;
        fixture.transport = Arc::new(FakeTransport {
            token_expires_in: Mutex::new(Some(expires_in)),
            returned_session: Mutex::new(Some(authority)),
            ..FakeTransport::default()
        });
        fixture.engine.transport = Box::new(fixture.transport.clone());
        let (_, _listener, attempt) = fixture.engine.begin(started_at - 1, 0).unwrap();
        fixture
            .engine
            .callback(
                &attempt,
                auth::Callback::Code("synthetic".into()),
                started_at,
                || true,
            )
            .unwrap();
        clock.store(expires_at - 1, Ordering::Release);
        assert!(fixture.engine.session().is_some());
        clock.store(expires_at, Ordering::Release);
        assert!(fixture.engine.session().is_none());
        assert!(fixture.vault.read().unwrap().unwrap().session.is_some());
        assert_eq!(fixture.transport.calls.load(Ordering::Acquire), 1);
    }
}

#[test]
fn cloud_uncertain_refresh_persists_reauth_and_never_retries_predecessor() {
    let mut fixture = Fixture::new();
    fixture.sign_in();
    fixture
        .transport
        .fail_exchange
        .store(true, Ordering::Release);
    assert_eq!(
        fixture.engine.refresh(200, || true),
        Err(Failure::ReauthRequired)
    );
    fixture
        .transport
        .fail_exchange
        .store(false, Ordering::Release);
    fixture.engine.refresh(201, || true).unwrap();
    assert!(fixture.engine.status == AuthStatus::ReauthRequired);
    assert_eq!(fixture.transport.calls.load(Ordering::Acquire), 2);
    fixture.restart();
    fixture.engine.refresh(201, || true).unwrap();
    fixture.engine.refresh(202, || true).unwrap();
    assert!(fixture.engine.status == AuthStatus::ReauthRequired);
    assert!(fixture.engine.session().is_none());
    assert_eq!(fixture.transport.calls.load(Ordering::Acquire), 2);
    assert_eq!(
        *fixture.transport.seen.lock().unwrap(),
        ["code", "synthetic-refresh-0"]
    );
}

#[test]
fn cloud_refresh_retries_an_unsent_proof_after_reconnect_or_restart() {
    for restart in [false, true] {
        let mut fixture = Fixture::new();
        fixture.sign_in();
        fixture
            .transport
            .fail_before_send
            .store(true, Ordering::Release);
        assert_eq!(fixture.engine.refresh(200, || true), Err(Failure::Offline));
        assert!(fixture.engine.session().is_none());
        assert!(!fixture.engine.cleanup.blocks_connection());
        assert_eq!(fixture.transport.calls.load(Ordering::Acquire), 1);
        fixture
            .transport
            .fail_before_send
            .store(false, Ordering::Release);
        if restart {
            fixture.restart();
        }
        fixture.engine.refresh(201, || true).unwrap();
        assert!(fixture.engine.status == AuthStatus::SignedIn);
        assert_eq!(
            *fixture.transport.seen.lock().unwrap(),
            ["code", "synthetic-refresh-0"]
        );
        assert_eq!(
            fixture.vault.read().unwrap().unwrap().refresh_token,
            "synthetic-refresh-1"
        );
    }
}

#[test]
fn cloud_unsent_refresh_does_not_reopen_a_cancelled_generation() {
    for cancel_during_restore in [false, true] {
        let mut fixture = Fixture::new();
        fixture.sign_in();
        let generation = Arc::new(AtomicU64::new(0));
        let cancelled = generation.clone();
        let transport = Arc::new(FakeTransport {
            fail_before_send: AtomicBool::new(true),
            on_exchange: Some(Arc::new(move || {
                if !cancel_during_restore {
                    cancelled.fetch_add(1, Ordering::AcqRel);
                }
            })),
            ..FakeTransport::default()
        });
        fixture.engine.transport = Box::new(transport.clone());
        let checks = AtomicU64::new(0);
        assert_eq!(
            fixture.engine.refresh(200, || {
                if cancel_during_restore && checks.fetch_add(1, Ordering::AcqRel) == 2 {
                    generation.fetch_add(1, Ordering::AcqRel);
                }
                generation.load(Ordering::Acquire) == 0
            }),
            Err(Failure::Cancelled)
        );
        assert!(fixture.engine.session().is_none());
        fixture.restart();
        fixture.engine.refresh(201, || true).unwrap();
        assert!(fixture.engine.status == AuthStatus::ReauthRequired);
        assert!(fixture.engine.cleanup.blocks_connection());
        assert_eq!(transport.calls.load(Ordering::Acquire), 0);
        assert_eq!(fixture.transport.calls.load(Ordering::Acquire), 1);
    }
}

#[test]
fn cloud_fixed_grant_deadline_does_not_slide_on_refresh() {
    let mut fixture = Fixture::new();
    fixture.sign_in();
    assert_eq!(
        fixture.engine.refresh(10_001, || true),
        Err(Failure::ReauthRequired)
    );
    assert!(fixture.engine.session().is_none());
    assert_eq!(fixture.transport.calls.load(Ordering::Acquire), 1);
    let mut value = session();
    value.expires_at += 1;
    assert!(!session().same_grant(&value));
    value = session();
    value.resource = "https://other.example/".into();
    assert_eq!(
        value.validate(&config(), 100),
        Err(Failure::InvalidResponse)
    );
}

#[test]
fn cloud_refresh_uses_the_grant_deadline_not_the_previous_access_deadline() {
    let mut fixture = Fixture::new();
    fixture.sign_in();
    fixture
        .vault
        .value
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .session
        .as_mut()
        .unwrap()
        .access_expires_at = 120;
    fixture.restart();
    fixture.engine.refresh(200, || true).unwrap();
    assert!(fixture.engine.session().is_some());
    assert_eq!(fixture.transport.calls.load(Ordering::Acquire), 2);
}

#[test]
fn cloud_rejected_refresh_keeps_both_grant_proofs_and_requires_original_revocation() {
    let mut fixture = Fixture::new();
    fixture.sign_in();
    *fixture.transport.token_grant.lock().unwrap() = Some("another-grant".into());
    *fixture.transport.fail_revoke_for.lock().unwrap() = Some("synthetic-refresh-0".into());
    assert_eq!(
        fixture.engine.refresh(200, || true),
        Err(Failure::ReauthRequired)
    );
    let cleanup = fixture.vault.read().unwrap().unwrap();
    assert_eq!(cleanup.refresh_token, "synthetic-refresh-0");
    assert_eq!(cleanup.cleanup_refresh_tokens, ["synthetic-refresh-1"]);
    assert!(cleanup.session.is_none());
    fixture.restart();
    fixture.engine.sign_out().unwrap();
    assert!(fixture.engine.cleanup.blocks_connection());
    assert!(fixture.vault.read().unwrap().is_some());
    *fixture.transport.fail_revoke_for.lock().unwrap() = None;
    fixture.engine.sign_out().unwrap();
    assert!(fixture.engine.cleanup == state::Cleanup::None);
    assert!(fixture.vault.read().unwrap().is_none());
    assert_eq!(
        *fixture.transport.revoked.lock().unwrap(),
        [
            "synthetic-refresh-0",
            "synthetic-refresh-1",
            "synthetic-refresh-0",
            "synthetic-refresh-1",
            "synthetic-refresh-0",
            "synthetic-refresh-1",
        ]
    );
    assert_eq!(fixture.transport.calls.load(Ordering::Acquire), 2);
}

#[test]
fn cloud_invalid_token_bounds_keep_logout_proof_without_restoring_access() {
    for expires_in in [0, 901] {
        let mut fixture = Fixture::new();
        fixture.sign_in();
        *fixture.transport.token_expires_in.lock().unwrap() = Some(expires_in);
        fixture.transport.fail_revoke.store(true, Ordering::Release);
        assert_eq!(
            fixture.engine.refresh(200, || true),
            Err(Failure::ReauthRequired)
        );
        assert!(fixture.engine.session().is_none());
        let cleanup = fixture.vault.read().unwrap().unwrap();
        assert!(cleanup.session.is_none());
        assert_eq!(cleanup.refresh_token, "synthetic-refresh-0");
        assert_eq!(cleanup.cleanup_refresh_tokens, ["synthetic-refresh-1"]);
        fixture.restart();
        fixture.engine.refresh(201, || true).unwrap();
        assert!(fixture.engine.session().is_none());
        fixture
            .transport
            .fail_revoke
            .store(false, Ordering::Release);
        fixture.engine.sign_out().unwrap();
        assert!(fixture.vault.read().unwrap().is_none());
    }
}

#[cfg(unix)]
#[test]
fn cloud_state_write_failure_still_retains_both_cleanup_proofs() {
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = Fixture::new();
    fixture.sign_in();
    let directory = fixture.directory.path().to_owned();
    let blocked = directory.clone();
    let transport = Arc::new(FakeTransport {
        calls: AtomicU64::new(1),
        token_grant: Mutex::new(Some("another-grant".into())),
        on_session: Some(Arc::new(move || {
            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o500)).unwrap();
        })),
        ..FakeTransport::default()
    });
    fixture.engine.transport = Box::new(transport);
    let result = fixture.engine.refresh(200, || true);
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result, Err(Failure::CredentialFailed));
    let cleanup = fixture.vault.read().unwrap().unwrap();
    assert_eq!(cleanup.refresh_token, "synthetic-refresh-0");
    assert_eq!(cleanup.cleanup_refresh_tokens, ["synthetic-refresh-1"]);
    assert!(cleanup.session.is_none());
    fixture.restart();
    fixture.engine.refresh(201, || true).unwrap();
    assert!(fixture.engine.session().is_none());
    fixture.engine.sign_out().unwrap();
    assert_eq!(
        *fixture.transport.revoked.lock().unwrap(),
        ["synthetic-refresh-0", "synthetic-refresh-1",]
    );
    assert!(fixture.vault.read().unwrap().is_none());
}

#[test]
fn cloud_stale_browser_cancel_does_not_abandon_a_newer_attempt() {
    let mut fixture = Fixture::new();
    let (_, _listener, attempt) = fixture.engine.begin(100, 2).unwrap();
    fixture.engine.cancel(1).unwrap();
    assert_eq!(
        fixture.engine.attempt.as_ref().unwrap().state,
        attempt.state
    );
    fixture.engine.refresh(101, || true).unwrap();
    assert!(fixture.engine.status == AuthStatus::Waiting);
    fixture.engine.cancel(3).unwrap();
    assert!(fixture.engine.status == AuthStatus::RevocationUnconfirmed);
}

#[test]
fn cloud_reordered_account_cleanup_cannot_restore_an_older_generation() {
    let mut fixture = Fixture::new();
    fixture.sign_in();
    assert!(fixture.engine.allows_account_cleanup(2, 2));
    fixture.engine.sign_out().unwrap();
    fixture.engine.generation = 2;
    assert!(!fixture.engine.allows_account_cleanup(1, 2));
    assert!(!fixture.engine.allows_account_cleanup(1, 1));
    assert_eq!(fixture.engine.generation, 2);
    fixture.engine.begin(200, 3).unwrap();
}

#[test]
fn cloud_keyring_read_and_write_failures_are_fail_closed() {
    let mut fixture = Fixture::new();
    fixture.vault.fail_write.store(true, Ordering::Release);
    assert!(matches!(
        fixture.engine.begin(100, 0),
        Err(Failure::CredentialFailed)
    ));
    fixture.vault.fail_write.store(false, Ordering::Release);
    fixture.sign_in();
    fixture.vault.fail_read.store(true, Ordering::Release);
    assert_eq!(
        fixture.engine.refresh(200, || true),
        Err(Failure::CredentialFailed)
    );
    assert!(fixture.engine.session().is_none());
    fixture.vault.fail_read.store(false, Ordering::Release);
    fixture.restart();
    fixture.engine.refresh(201, || true).unwrap();
    assert!(fixture.engine.status == AuthStatus::ReauthRequired);
}

#[test]
fn cloud_logout_marker_blocks_restart_when_remote_revocation_is_unconfirmed() {
    let mut fixture = Fixture::new();
    fixture.sign_in();
    fixture.transport.fail_revoke.store(true, Ordering::Release);
    fixture.engine.sign_out().unwrap();
    assert!(fixture.engine.status == AuthStatus::RevocationUnconfirmed);
    assert!(fixture.engine.session().is_none());
    fixture.restart();
    fixture.engine.refresh(102, || true).unwrap();
    assert!(fixture.engine.status == AuthStatus::RevocationUnconfirmed);
    assert!(fixture.engine.session().is_none());
    fixture
        .transport
        .fail_revoke
        .store(false, Ordering::Release);
    fixture.engine.sign_out().unwrap();
    assert!(fixture.engine.status == AuthStatus::SignedOut);
    assert!(fixture.vault.read().unwrap().is_none());
}

#[test]
fn cloud_logout_keyring_delete_failure_cannot_restore_an_account() {
    let mut fixture = Fixture::new();
    fixture.sign_in();
    fixture.vault.fail_write.store(true, Ordering::Release);
    assert_eq!(fixture.engine.sign_out(), Err(Failure::CredentialFailed));
    fixture.vault.fail_write.store(false, Ordering::Release);
    fixture.restart();
    fixture.engine.refresh(200, || true).unwrap();
    assert!(fixture.engine.status == AuthStatus::SignedOut);
    assert!(fixture.engine.session().is_none());
    assert_eq!(fixture.transport.revocations.load(Ordering::Acquire), 1);
    fixture.engine.sign_out().unwrap();
    assert!(fixture.engine.status == AuthStatus::SignedOut);
}

#[test]
fn cloud_lifetime_owner_rejects_a_second_writer_until_restart() {
    let mut fixture = Fixture::new();
    fixture.sign_in();
    assert_eq!(fixture.reopen().sign_out(), Err(Failure::Unavailable));
    assert!(fixture.engine.session().is_some());
    fixture.restart();
    fixture.engine.sign_out().unwrap();
    assert!(fixture.engine.session().is_none());
}

#[test]
fn cloud_process_lock_child() {
    let Ok(directory) = std::env::var("ANTIBURN_SYNTHETIC_CLOUD_LOCK_TEST") else {
        return;
    };
    let store = state::StateStore::new(Path::new(&directory));
    if std::env::var_os("ANTIBURN_SYNTHETIC_CLOUD_EXPECT_LOCKED").is_some() {
        assert!(matches!(store.lock(), Err(Failure::Unavailable)));
        return;
    }
    let mut guard = store.lock().unwrap();
    let record = guard.read().unwrap();
    assert!(record.phase == state::Phase::SignedOut);
    guard
        .replace(state::Phase::ReauthRequired, state::Cleanup::None)
        .unwrap();
}

#[test]
fn cloud_lock_serializes_separate_processes() {
    let directory = tempfile::tempdir().unwrap();
    let store = state::StateStore::new(directory.path());
    let mut guard = store.lock().unwrap();
    guard
        .replace(state::Phase::SignedOut, state::Cleanup::None)
        .unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "cloud::tests::cloud_process_lock_child",
            "--nocapture",
        ])
        .env("ANTIBURN_SYNTHETIC_CLOUD_LOCK_TEST", directory.path());
    assert!(
        command
            .env("ANTIBURN_SYNTHETIC_CLOUD_EXPECT_LOCKED", "1")
            .status()
            .unwrap()
            .success()
    );
    drop(guard);
    drop(store);
    assert!(
        command
            .env_remove("ANTIBURN_SYNTHETIC_CLOUD_EXPECT_LOCKED")
            .status()
            .unwrap()
            .success()
    );
    let store = state::StateStore::new(directory.path());
    assert!(store.lock().unwrap().read().unwrap().phase == state::Phase::ReauthRequired);
}

#[test]
fn cloud_snapshot_does_not_include_native_secrets() {
    let value = Snapshot {
        auth_enabled: true,
        capture_enabled: true,
        sync_enabled: false,
        status: AuthStatus::SignedIn,
        identity: Some(Identity::from(&session())),
        expires_at: None,
        grant_expires_at: Some(10_000),
        destination: Some(session().workspace),
        revocation_pending: false,
        local_cleanup_pending: false,
        local_reset_available: false,
        remote_revocation_unconfirmed: false,
        setup_error: None,
        pending_sync: None,
        draft: Draft::default(),
    };
    let encoded = serde_json::to_string(&value).unwrap();
    for field in [
        "access_token",
        "refresh_token",
        "verifier",
        "state",
        "grant_id",
        "device_id",
    ] {
        assert!(!encoded.contains(field));
    }
}

#[test]
fn cloud_folder_writes_require_the_exact_current_personal_context() {
    let binding = DestinationBinding {
        account_id: session().user.id,
        destination_id: session().workspace.id,
    };
    assert!(binding.matches(&session()));
    let mut other = session();
    other.user.id = "another-account".into();
    assert!(!binding.matches(&other));
    other = session();
    other.workspace.id = "another-destination".into();
    assert!(!binding.matches(&other));
    other = session();
    other.workspace.kind = WorkspaceKind::Team;
    assert!(!binding.matches(&other));
}

#[test]
fn cloud_state_read_failure_clears_cached_access_and_logout_repairs_only_closed_state() {
    let mut fixture = Fixture::new();
    fixture.sign_in();
    std::fs::write(
        fixture.directory.path().join("cloud-auth-state-v2"),
        b"{torn",
    )
    .unwrap();
    assert_eq!(
        fixture.engine.refresh(102, || true),
        Err(Failure::CredentialFailed)
    );
    assert!(fixture.engine.session().is_none());
    fixture.engine.sign_out().unwrap();
    fixture.restart();
    fixture.engine.refresh(103, || true).unwrap();
    assert!(fixture.engine.status == AuthStatus::SignedOut);
    assert!(fixture.vault.read().unwrap().is_none());
}

#[test]
fn cloud_browser_cancellation_during_established_refresh_does_not_revoke_the_account() {
    let mut fixture = Fixture::new();
    fixture.sign_in();
    let attempt_generation = Arc::new(AtomicU64::new(0));
    let account_generation = AtomicU64::new(0);
    let cancel = attempt_generation.clone();
    let transport = Arc::new(FakeTransport {
        calls: AtomicU64::new(1),
        on_session: Some(Arc::new(move || {
            cancel.fetch_add(1, Ordering::AcqRel);
        })),
        ..FakeTransport::default()
    });
    fixture.engine.transport = Box::new(transport.clone());
    fixture
        .engine
        .refresh(200, || account_generation.load(Ordering::Acquire) == 0)
        .unwrap();
    fixture
        .engine
        .cancel(attempt_generation.load(Ordering::Acquire))
        .unwrap();
    assert!(fixture.engine.session().is_some());
    assert_eq!(transport.revocations.load(Ordering::Acquire), 0);
    assert_eq!(
        fixture.vault.read().unwrap().unwrap().refresh_token,
        "synthetic-refresh-1"
    );
}

#[test]
fn cloud_session_deadline_is_bounded_and_display_names_are_not_grant_bindings() {
    let mut value = session();
    value.expires_at = 100 + auth::MAX_GRANT_SECONDS;
    value.validate(&config(), 100).unwrap();
    value.expires_at += 1;
    assert_eq!(
        value.validate(&config(), 100),
        Err(Failure::InvalidResponse)
    );
    value = session();
    value.user.display_name = "Another display name".into();
    value.workspace.name = "Renamed workspace".into();
    assert!(session().same_grant(&value));
}

#[tokio::test]
async fn cloud_raw_loopback_rejects_unguarded_requests_and_accepts_one_exact_callback() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };
    let (fixture, listener, attempt) = tokio::task::spawn_blocking(|| {
        let mut fixture = Fixture::new();
        let (_, listener, attempt) = fixture.engine.begin(now(), 0).unwrap();
        (fixture, listener, attempt)
    })
    .await
    .unwrap();
    let address = attempt
        .callback
        .trim_start_matches("http://")
        .trim_end_matches("/cloud/callback")
        .to_owned();
    let target = format!(
        "/cloud/callback?state={}&iss=https%3A%2F%2Fapi.example&code=synthetic",
        attempt.state
    );
    let worker = tokio::spawn(async move { listener.wait(&config(), &attempt, || true).await });
    for request in [
        format!("POST {target} HTTP/1.1\r\nHost: {address}\r\n\r\n"),
        format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n"),
        format!("GET {target} HTTP/1.1\r\nHost: {address}\r\nHost: {address}\r\n\r\n"),
        format!(
            "GET {target} HTTP/1.1\r\nHost: {address}\r\nOrigin: https://other.example\r\n\r\n"
        ),
        format!("GET {target} HTTP/1.1\r\nHost: {address}\r\nContent-Length: 1\r\n\r\nx"),
        format!("GET {target} HTTP/1.1\r\nHost: {address}\r\n\r\n"),
    ] {
        let valid = request == format!("GET {target} HTTP/1.1\r\nHost: {address}\r\n\r\n");
        let mut stream = TcpStream::connect(&address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        let read = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            stream.read_to_string(&mut response),
        )
        .await
        .unwrap();
        if valid {
            read.unwrap();
            assert!(response.starts_with("HTTP/1.1 200"));
        } else {
            assert!(
                read.is_ok()
                    || read.is_err_and(|error| error.kind() == std::io::ErrorKind::ConnectionReset)
            );
            assert!(response.is_empty() || response.starts_with("HTTP/1.1 400"));
        }
        assert!(!response.contains("synthetic"));
    }
    assert!(
        matches!(worker.await.unwrap(), Some(auth::Callback::Code(code)) if code == "synthetic")
    );
    tokio::task::spawn_blocking(move || drop(fixture))
        .await
        .unwrap();
}

struct SyntheticFileVault(std::path::PathBuf);
impl Vault for SyntheticFileVault {
    fn read(&self) -> Result<Option<Credential>, Failure> {
        match std::fs::read(&self.0) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| Failure::CredentialFailed),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(Failure::CredentialFailed),
        }
    }
    fn write(&self, credential: Option<&Credential>) -> Result<(), Failure> {
        match credential {
            Some(value) => std::fs::write(&self.0, serde_json::to_vec(value).unwrap()),
            None => match std::fs::remove_file(&self.0) {
                Ok(()) => {
                    if std::env::var_os("ANTIBURN_SYNTHETIC_CLOUD_EXIT_AFTER_DELETE").is_some() {
                        std::process::exit(73);
                    }
                    Ok(())
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
        }
        .map_err(|_| Failure::CredentialFailed)
    }
}

fn process_engine(directory: &Path, count: u64) -> (Engine, Arc<FakeTransport>) {
    let transport = Arc::new(FakeTransport {
        calls: AtomicU64::new(count),
        ..FakeTransport::default()
    });
    (
        Engine::new(
            Some(config()),
            Box::new(SyntheticFileVault(directory.join("synthetic-vault.json"))),
            Box::new(transport.clone()),
            state::StateStore::new(directory),
        )
        .with_clock(|| 100),
        transport,
    )
}

#[test]
fn cloud_process_rotation_child() {
    let Ok(directory) = std::env::var("ANTIBURN_SYNTHETIC_CLOUD_ROTATION_TEST") else {
        return;
    };
    let (mut engine, transport) = process_engine(Path::new(&directory), 1);
    engine.refresh(200, || true).unwrap();
    assert_eq!(*transport.seen.lock().unwrap(), ["synthetic-refresh-0"]);
    assert!(engine.session().is_some());
}

#[test]
fn cloud_process_restart_rotates_the_saved_successor_instead_of_a_predecessor() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, _) = process_engine(directory.path(), 0);
    let (_, _listener, attempt) = engine.begin(100, 0).unwrap();
    engine
        .callback(
            &attempt,
            auth::Callback::Code("synthetic".into()),
            101,
            || true,
        )
        .unwrap();
    drop(engine);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cloud::tests::cloud_process_rotation_child",
            "--nocapture",
        ])
        .env("ANTIBURN_SYNTHETIC_CLOUD_ROTATION_TEST", directory.path())
        .status()
        .unwrap();
    assert!(status.success());
    let (mut engine, transport) = process_engine(directory.path(), 2);
    engine.refresh(300, || true).unwrap();
    assert_eq!(*transport.seen.lock().unwrap(), ["synthetic-refresh-1"]);
}

#[test]
fn cloud_process_cleanup_child() {
    let Ok(directory) = std::env::var("ANTIBURN_SYNTHETIC_CLOUD_CLEANUP_TEST") else {
        return;
    };
    let (mut engine, _) = process_engine(Path::new(&directory), 1);
    engine.sign_out().unwrap();
    panic!("The synthetic crash must occur after deleting the test credential");
}

#[test]
fn cloud_restart_finishes_deletion_after_confirmed_revocation_and_a_process_exit() {
    let directory = tempfile::tempdir().unwrap();
    let (mut engine, _) = process_engine(directory.path(), 0);
    let (_, _listener, attempt) = engine.begin(100, 0).unwrap();
    engine
        .callback(
            &attempt,
            auth::Callback::Code("synthetic".into()),
            101,
            || true,
        )
        .unwrap();
    drop(engine);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cloud::tests::cloud_process_cleanup_child",
            "--nocapture",
        ])
        .env("ANTIBURN_SYNTHETIC_CLOUD_CLEANUP_TEST", directory.path())
        .env("ANTIBURN_SYNTHETIC_CLOUD_EXIT_AFTER_DELETE", "1")
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(73));
    let (mut engine, transport) = process_engine(directory.path(), 2);
    engine.refresh(200, || true).unwrap();
    assert!(engine.status == AuthStatus::SignedOut);
    assert!(engine.cleanup == state::Cleanup::None);
    assert_eq!(transport.revocations.load(Ordering::Acquire), 0);
    engine.begin(201, 1).unwrap();
}

#[test]
fn cloud_corrupt_state_with_no_credential_needs_explicit_reset_and_keeps_its_warning() {
    let mut fixture = Fixture::new();
    let (_, _listener, attempt) = fixture.engine.begin(100, 0).unwrap();
    std::fs::write(
        fixture.directory.path().join("cloud-auth-state-v2"),
        b"{torn",
    )
    .unwrap();
    let retry = fixture.directory.path().join("synthetic-retry.bin");
    std::fs::write(&retry, b"unchanged synthetic compressed retry bytes").unwrap();
    fixture.engine.sign_out().unwrap();
    assert!(fixture.engine.status == AuthStatus::RevocationUnconfirmed);
    assert!(fixture.engine.local_reset_available);
    assert_eq!(fixture.transport.revocations.load(Ordering::Acquire), 0);
    assert!(matches!(
        fixture.engine.begin(101, 1),
        Err(Failure::Unavailable)
    ));
    fixture.engine.reset_local().unwrap();
    assert!(fixture.engine.cleanup == state::Cleanup::RemoteUnconfirmed);
    assert_eq!(
        fixture
            .engine
            .callback(&attempt, auth::Callback::Code("late".into()), 102, || true),
        Err(Failure::Cancelled)
    );
    fixture.restart();
    fixture.engine.refresh(103, || true).unwrap();
    assert!(fixture.engine.status == AuthStatus::SignedOut);
    assert!(fixture.engine.cleanup == state::Cleanup::RemoteUnconfirmed);
    fixture.sign_in();
    assert!(fixture.engine.cleanup == state::Cleanup::RemoteUnconfirmed);
    fixture.engine.sign_out().unwrap();
    fixture.restart();
    fixture.engine.refresh(104, || true).unwrap();
    assert!(fixture.engine.cleanup == state::Cleanup::RemoteUnconfirmed);
    assert_eq!(
        std::fs::read(retry).unwrap(),
        b"unchanged synthetic compressed retry bytes"
    );
}

#[test]
fn cloud_local_reset_does_not_discard_a_recoverable_refresh_credential() {
    let mut fixture = Fixture::new();
    fixture.sign_in();
    fixture.transport.fail_revoke.store(true, Ordering::Release);
    fixture.engine.sign_out().unwrap();
    assert_eq!(fixture.engine.reset_local(), Err(Failure::Unavailable));
    assert!(fixture.vault.read().unwrap().is_some());
}

#[test]
fn cloud_corrupt_state_cannot_clear_an_earlier_connection_warning() {
    let mut fixture = Fixture::new();
    fixture.engine.begin(100, 0).unwrap();
    fixture.engine.cancel(1).unwrap();
    fixture.engine.reset_local().unwrap();
    fixture.sign_in();
    std::fs::write(
        fixture.directory.path().join("cloud-auth-state-v2"),
        b"{torn",
    )
    .unwrap();
    fixture.engine.sign_out().unwrap();
    assert!(fixture.vault.read().unwrap().is_none());
    assert!(fixture.engine.cleanup == state::Cleanup::RemoteUnconfirmed);
    fixture.restart();
    fixture.engine.refresh(200, || true).unwrap();
    assert!(fixture.engine.cleanup == state::Cleanup::RemoteUnconfirmed);
}
