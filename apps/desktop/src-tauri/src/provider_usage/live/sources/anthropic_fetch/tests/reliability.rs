//! Approved owner-refresh policy with synthetic carriers and transports.

use super::*;
use crate::provider_usage::live::sources::pi_refresh::{RefreshRunner, RunOutcome};

pub(super) fn assert_published_detection<T: LiveUsageSource + 'static>(
    source: Arc<T>,
    expected: Detection,
    error: Option<ProviderUsageError>,
) {
    use crate::provider_usage::live::{detect_all, roster, sources::collect, summarize_collected};
    use crate::store::HiddenMeters;

    struct Shared<T>(Arc<T>);
    impl<T: LiveUsageSource> LiveUsageSource for Shared<T> {
        fn id(&self) -> &'static str {
            self.0.id()
        }
        fn provider(&self) -> &'static str {
            self.0.provider()
        }
        fn detect(&self, online: bool) -> Presence {
            self.0.detect(online)
        }
        fn fetch(&self, max_age: std::time::Duration) -> SourceOutcome {
            self.0.fetch(max_age)
        }
    }

    let sources: Vec<Box<dyn LiveUsageSource>> = vec![Box::new(Shared(source))];
    let hidden = HiddenMeters::default();
    let detection = detect_all(&sources, true);
    let collected = collect(&sources, true, &hidden, USER_MAX_AGE);
    assert_eq!(collected.errors.first().map(|error| error.error), error);
    let meters = roster(&sources, &hidden, &detection);
    let summary = summarize_collected(collected, meters, None, None, real_now_secs(), 0);
    assert_eq!(summary.meters[0].detection, expected);
}

#[test]
fn changed_malformed_file_preserves_published_detection_then_recovers() {
    for initial in [BLANK_LOGIN, r#"{"mcpOAuth":{}}"#] {
        for malformed in [b"{\"claudeAiOauth\":".as_slice(), &[0xff, 0xfe]] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("credentials.json");
            fs::write(&path, initial).unwrap();
            let source = Arc::new(ClaudeDirectFetch::at_with_touch(
                path.clone(),
                Box::new(OnlyToken("synthetic-token")),
                Box::new(UntouchableEnv),
            ));
            let expected = if initial == BLANK_LOGIN {
                Detection::SignInRequired
            } else {
                Detection::InstalledNotSignedIn
            };
            assert_published_detection(
                source.clone(),
                expected,
                Some(ProviderUsageError::Authentication),
            );
            fs::write(&path, malformed).unwrap();
            source.cooldown.open_for_test();
            assert_published_detection(
                source.clone(),
                expected,
                Some(ProviderUsageError::Unavailable),
            );
            fs::write(&path, credentials_file(i64::MAX, "pro")).unwrap();
            source.cooldown.open_for_test();
            assert_published_detection(source.clone(), Detection::SignedIn, None);
            assert_eq!(source.detect(false).detection, Detection::SignedIn);
        }
    }
}

struct NoPi;
impl RefreshRunner for NoPi {
    fn run(&self, _: &str) -> RunOutcome {
        panic!("this login must not launch Pi")
    }
    fn check(&self, _: &str) -> RunOutcome {
        panic!("fetch must not run detection")
    }
}

#[test]
fn native_missing_access_with_refresh_recovers_through_the_owner() {
    for access in [None, Some(""), Some("   ")] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let mut login = serde_json::json!({"claudeAiOauth":{"refreshToken":"synthetic-refresh"}});
        if let Some(access) = access {
            login["claudeAiOauth"]["accessToken"] = access.into();
        }
        fs::write(&path, login.to_string()).unwrap();
        let spawns = Arc::new(AtomicUsize::new(0));
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(OnlyToken("synthetic-token")),
            Box::new(FileTouchEnv {
                path,
                binary_present: true,
                writes_on_spawn: Some(credentials_file(i64::MAX, "max")),
                spawns: spawns.clone(),
            }),
        );
        assert_eq!(source.fetch(USER_MAX_AGE).error, None);
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn native_live_without_refresh_uses_access_even_near_expiry() {
    for refresh in [None, Some(""), Some("   ")] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let mut login = serde_json::json!({"claudeAiOauth":{
            "accessToken":"synthetic-access", "expiresAt":(real_now_secs()+120)*1000
        }});
        if let Some(refresh) = refresh {
            login["claudeAiOauth"]["refreshToken"] = refresh.into();
        }
        fs::write(&path, login.to_string()).unwrap();
        let source = ClaudeDirectFetch::at_with_touch(
            path,
            Box::new(OnlyToken("synthetic-access")),
            Box::new(UntouchableEnv),
        );
        assert_eq!(source.fetch(USER_MAX_AGE).error, None);
    }
}

#[test]
fn pi_without_refresh_never_launches_and_live_access_still_works() {
    for live in [false, true] {
        for refresh in [None, Some(""), Some("   ")] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("auth.json");
            let mut login = serde_json::json!({"anthropic":{
                "type":"oauth", "access":if live {"synthetic-access"} else {""},
                "expires":if live {i64::MAX} else {0}
            }});
            if let Some(refresh) = refresh {
                login["anthropic"]["refresh"] = refresh.into();
            }
            fs::write(&path, login.to_string()).unwrap();
            let source = ClaudeDirectFetch::with_pi(
                path,
                Box::new(OnlyToken("synthetic-access")),
                PiRefresher::with_runner(Box::new(NoPi)),
            );
            let outcome = source.fetch(USER_MAX_AGE);
            assert_eq!(
                outcome.detail,
                if live {
                    None
                } else {
                    Some(SourceErrorDetail::SignInRequired)
                }
            );
            assert_eq!(outcome.snapshots.len(), usize::from(live));
        }
    }
}

struct RotatePi(PathBuf, Arc<AtomicUsize>);
impl RefreshRunner for RotatePi {
    fn run(&self, _: &str) -> RunOutcome {
        self.1.fetch_add(1, Ordering::SeqCst);
        fs::write(&self.0, r#"{"anthropic":{"type":"oauth","access":"rotated","refresh":"synthetic-refresh","expires":9223372036854775807}}"#).unwrap();
        RunOutcome::Completed
    }
    fn check(&self, _: &str) -> RunOutcome {
        panic!("fetch must not run detection")
    }
}

#[test]
fn pi_missing_access_with_refresh_delegates_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    fs::write(
        &path,
        r#"{"anthropic":{"type":"oauth","refresh":"synthetic-refresh"}}"#,
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let source = ClaudeDirectFetch::with_pi(
        path.clone(),
        Box::new(OnlyToken("rotated")),
        PiRefresher::with_runner(Box::new(RotatePi(path, calls.clone()))),
    );
    assert_eq!(source.fetch(USER_MAX_AGE).error, None);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn unreadable_native_file_preserves_login_and_reading_until_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    fs::write(&path, credentials_file(i64::MAX, "max")).unwrap();
    let source = ClaudeDirectFetch::at_with_touch(
        path.clone(),
        Box::new(OnlyToken("synthetic-token")),
        Box::new(UntouchableEnv),
    );
    assert_eq!(source.fetch(USER_MAX_AGE).snapshots.len(), 1);
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    source.cooldown.open_for_test();
    let failed = source.fetch(USER_MAX_AGE);
    assert_eq!(failed.error, Some(ProviderUsageError::Unavailable));
    assert_eq!(failed.snapshots.len(), 1);
    assert!(lock(&source.logins.file).as_ref().unwrap().login.is_some());
    fs::remove_dir(&path).unwrap();
    fs::write(&path, credentials_file(i64::MAX, "max")).unwrap();
    source.cooldown.open_for_test();
    assert_eq!(source.fetch(USER_MAX_AGE).error, None);
}

#[test]
fn unreadable_pi_store_is_unknown_and_never_launches() {
    let dir = tempfile::tempdir().unwrap();
    let source = ClaudeDirectFetch::with_pi(
        dir.path().to_path_buf(),
        Box::new(UnreachableTransport),
        PiRefresher::with_runner(Box::new(NoPi)),
    );
    assert_eq!(
        source.fetch(USER_MAX_AGE).error,
        Some(ProviderUsageError::Unavailable)
    );
}

#[test]
fn profile_success_and_http_failure_are_requested_once_until_token_changes() {
    for (response, expected) in [
        (
            Some(Some("synthetic-profile".to_owned())),
            Some("synthetic-profile".to_owned()),
        ),
        (Some(None), None),
    ] {
        let cache = Mutex::new(None);
        assert_eq!(
            cached_profile(&cache, "first", || response.clone()),
            expected
        );
        assert_eq!(
            cached_profile(&cache, "first", || panic!("duplicate profile request")),
            expected
        );
        assert_eq!(
            cached_profile(&cache, "second", || Some(Some("second-profile".into()))),
            Some("second-profile".into())
        );
    }
}

#[test]
fn profile_transport_failure_is_retried_until_a_response_arrives() {
    let cache = Mutex::new(None);
    assert_eq!(cached_profile(&cache, "token", || None), None);
    assert_eq!(
        cached_profile(&cache, "token", || Some(Some("recovered-profile".into()))),
        Some("recovered-profile".into())
    );
}

#[test]
fn unreadable_file_preserves_the_last_sign_in_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    fs::write(&path, BLANK_LOGIN).unwrap();
    let source = ClaudeDirectFetch::at_with_touch(
        path.clone(),
        Box::new(UnreachableTransport),
        Box::new(UntouchableEnv),
    );
    assert_eq!(
        source.fetch(USER_MAX_AGE).detail,
        Some(SourceErrorDetail::SignInRequired)
    );
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    source.cooldown.open_for_test();
    assert_eq!(
        source.fetch(USER_MAX_AGE).error,
        Some(ProviderUsageError::Unavailable)
    );
    assert_eq!(source.detect(false).detection, Detection::SignInRequired);
}

#[test]
fn malformed_native_file_preserves_state_and_reading_then_recovers() {
    for malformed in [b"{\"claudeAiOauth\":".as_slice(), &[0xff, 0xfe]] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        fs::write(&path, credentials_file(i64::MAX, "max")).unwrap();
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(OnlyToken("synthetic-token")),
            Box::new(UntouchableEnv),
        );
        assert_eq!(source.fetch(USER_MAX_AGE).snapshots.len(), 1);
        source.logins.set_sign_in_required(true);
        fs::write(&path, malformed).unwrap();
        source.cooldown.open_for_test();
        let failed = source.fetch(USER_MAX_AGE);
        assert_eq!(failed.error, Some(ProviderUsageError::Unavailable));
        assert_eq!(failed.snapshots.len(), 1);
        assert_eq!(source.detect(false).detection, Detection::SignInRequired);
        fs::write(&path, credentials_file(i64::MAX, "pro")).unwrap();
        source.cooldown.open_for_test();
        assert_eq!(source.fetch(USER_MAX_AGE).error, None);
        assert_eq!(source.detect(false).detection, Detection::SignedIn);
    }
}
