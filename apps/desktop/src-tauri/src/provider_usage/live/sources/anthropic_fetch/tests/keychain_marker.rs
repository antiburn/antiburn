//! Keychain failures keep usable credentials; background reads never prompt.
use super::*;
use std::cell::Cell;

struct PublishedKeychain {
    source: ClaudeDirectFetch,
    material: Mutex<(Option<&'static [u8]>, String)>,
}
impl LiveUsageSource for PublishedKeychain {
    fn id(&self) -> &'static str {
        self.source.id()
    }
    fn provider(&self) -> &'static str {
        self.source.provider()
    }
    fn detect(&self, _: bool) -> Presence {
        let material = lock(&self.material);
        let mut probe = RecordingPresence {
            keychain: Some(material.0.map_or(KeychainMetadata::Unreadable, |bytes| {
                KeychainMetadata::Found(bytes.to_vec())
            })),
            ..Default::default()
        };
        probe.paths.insert(PRESENCE_CONFIG.into(), Ok(true));
        detect_presence(
            &probe,
            &self.source.logins,
            Some(Path::new(PRESENCE_CREDENTIALS)),
            None,
            || PiStatus::Unknown,
        )
    }
    fn fetch(&self, max_age: Duration) -> SourceOutcome {
        let now = OffsetDateTime::now_utc();
        self.source.cooldown.poll(now, max_age, || {
            let material = lock(&self.material);
            let login = self.source.read_keychain_login(
                || {
                    material.0.map_or(KeychainMetadata::Unreadable, |bytes| {
                        KeychainMetadata::Found(bytes.to_vec())
                    })
                },
                secret(material.1.clone()),
                &|_| {},
                true,
            )?;
            let Some(login) = login else {
                return Err(auth_failure(SourceErrorDetail::NotSignedIn));
            };
            if !login.is_live(now) {
                self.source.logins.set_sign_in_required(true);
                return Err(auth_failure(SourceErrorDetail::SignInRequired));
            }
            fetch_from_carriers(&OnlyToken("synthetic-token"), vec![login], None, now)
                .map_err(FetchFailure::from)
        })
    }
}

#[test]
fn changed_malformed_keychain_preserves_published_detection_then_recovers() {
    for initial in [
        BLANK_LOGIN.to_owned(),
        MCP_ONLY.to_owned(),
        credentials_file(i64::MAX, "max"),
    ] {
        let expected = if initial == BLANK_LOGIN {
            Detection::SignInRequired
        } else if initial == MCP_ONLY {
            Detection::InstalledNotSignedIn
        } else {
            Detection::SignedIn
        };
        let live = expected == Detection::SignedIn;
        let mock = Arc::new(PublishedKeychain {
            source: source(),
            material: Mutex::new((Some(ATTRIBUTES_A), initial)),
        });
        reliability::assert_published_detection(
            mock.clone(),
            expected,
            if live {
                None
            } else {
                Some(ProviderUsageError::Authentication)
            },
        );
        *lock(&mock.material) = (Some(ATTRIBUTES_B), "{".into());
        mock.source.cooldown.open_for_test();
        reliability::assert_published_detection(
            mock.clone(),
            expected,
            if live {
                None
            } else {
                Some(ProviderUsageError::Unavailable)
            },
        );
        backdate_secret_failure(&mock.source);
        *lock(&mock.material) = (Some(ATTRIBUTES_B), credentials_file(i64::MAX, "max"));
        mock.source.cooldown.open_for_test();
        reliability::assert_published_detection(mock, Detection::SignedIn, None);
    }
}

#[test]
fn metadata_failure_preserves_confirmed_detection_before_and_after_collection() {
    for initial in [BLANK_LOGIN.to_owned(), credentials_file(i64::MAX, "max")] {
        let live = initial != BLANK_LOGIN;
        let expected = if live {
            Detection::SignedIn
        } else {
            Detection::SignInRequired
        };
        let mock = Arc::new(PublishedKeychain {
            source: source(),
            material: Mutex::new((Some(ATTRIBUTES_A), initial)),
        });
        reliability::assert_published_detection(
            mock.clone(),
            expected,
            if live {
                None
            } else {
                Some(ProviderUsageError::Authentication)
            },
        );
        lock(&mock.material).0 = None;
        mock.source.cooldown.open_for_test();
        assert_eq!(mock.detect(false).detection, expected);
        reliability::assert_published_detection(
            mock.clone(),
            expected,
            if live {
                None
            } else {
                Some(ProviderUsageError::Unavailable)
            },
        );
        assert_eq!(mock.detect(false).detection, expected);
    }
}

const ATTRIBUTES_A: &[u8] = b"attributes: mdat=A";
const ATTRIBUTES_B: &[u8] = b"attributes: mdat=B";

fn source() -> ClaudeDirectFetch {
    ClaudeDirectFetch::at(PathBuf::from("/nonexistent/.credentials.json"))
}
fn found(attributes: &[u8]) -> impl FnMut() -> KeychainMetadata + '_ {
    move || KeychainMetadata::Found(attributes.to_vec())
}
fn secret(body: String) -> impl FnOnce() -> macos_keychain::KeychainRead {
    move || macos_keychain::KeychainRead::Found(body)
}
fn no_secret_read() -> macos_keychain::KeychainRead {
    panic!("this check must not read a secret")
}
fn seed(source: &ClaudeDirectFetch, body: String) {
    source
        .read_keychain_login(found(ATTRIBUTES_A), secret(body), &|_| {}, true)
        .unwrap();
}
fn backdate_secret_failure(source: &ClaudeDirectFetch) {
    let mut failed = lock(&source.logins.keychain_secret_failed);
    let (marker, _) = failed.take().expect("a failed secret read");
    *failed = Some((marker, std::time::Instant::now() - SECRET_RETRY_BACKOFF));
}

#[test]
fn the_same_marker_uses_cached_live_expired_and_blank_logins() {
    for body in [
        credentials_file(i64::MAX, "max"),
        credentials_file(1, "max"),
        BLANK_LOGIN.into(),
    ] {
        let source = source();
        seed(&source, body);
        for user in [false, true] {
            assert!(
                source
                    .read_keychain_login(found(ATTRIBUTES_A), no_secret_read, &|_| {}, user)
                    .unwrap()
                    .is_some()
            );
        }
    }
}

#[test]
fn a_changed_marker_reads_the_secret_once_in_user_context() {
    let source = source();
    seed(&source, credentials_file(i64::MAX, "max"));
    let login = source
        .read_keychain_login(
            found(ATTRIBUTES_B),
            secret(credentials_file(i64::MAX, "pro")),
            &|_| {},
            true,
        )
        .unwrap()
        .unwrap();
    assert_eq!(login.subscription_type.as_deref(), Some("pro"));
    source
        .read_keychain_login(found(ATTRIBUTES_B), no_secret_read, &|_| {}, true)
        .unwrap();
}

#[test]
fn explicit_absence_clears_cache_and_never_uses_fallback() {
    let source = source();
    seed(&source, credentials_file(i64::MAX, "max"));
    assert!(
        source
            .read_keychain_login(|| KeychainMetadata::Absent, no_secret_read, &|_| {}, false)
            .unwrap()
            .is_none()
    );
    assert!(lock(&source.logins.keychain).is_none());
    assert!(
        source
            .read_keychain_login(
                || KeychainMetadata::Unreadable,
                no_secret_read,
                &|_| {},
                true
            )
            .is_err()
    );
}

#[test]
fn metadata_failure_returns_only_a_still_live_cached_credential() {
    for body in [
        None,
        Some(BLANK_LOGIN.into()),
        Some(MCP_ONLY.into()),
        Some(credentials_file(1, "max")),
        Some(credentials_file(i64::MAX, "max")),
    ] {
        let source = source();
        let live = body.as_ref().is_some_and(|body| {
            parse_credentials_json(body)
                .unwrap()
                .is_some_and(|login| login.is_live(OffsetDateTime::now_utc()))
        });
        if let Some(body) = body {
            seed(&source, body);
        }
        let attempts = Cell::new(0);
        let result = source.read_keychain_login(
            || {
                attempts.set(attempts.get() + 1);
                KeychainMetadata::Unreadable
            },
            no_secret_read,
            &|_| {},
            true,
        );
        assert_eq!(attempts.get(), 4);
        if live {
            let login = result.unwrap().unwrap();
            let snapshot = fetch_from_carriers(
                &OnlyToken("synthetic-token"),
                vec![login],
                None,
                OffsetDateTime::now_utc(),
            )
            .unwrap();
            assert!(snapshot.is_some());
        } else {
            assert_eq!(
                result.err().unwrap().detail,
                Some(SourceErrorDetail::KeychainUnreadable)
            );
        }
    }
}

#[test]
fn failed_secret_and_backoff_return_the_live_cached_token() {
    for failure in [
        macos_keychain::KeychainRead::Unreadable,
        macos_keychain::KeychainRead::Absent,
        macos_keychain::KeychainRead::Found("{".into()),
    ] {
        let source = source();
        seed(&source, credentials_file(i64::MAX, "max"));
        let login = source
            .read_keychain_login(found(ATTRIBUTES_B), || failure, &|_| {}, true)
            .unwrap()
            .unwrap();
        assert_eq!(login.subscription_type.as_deref(), Some("max"));
        let login = source
            .read_keychain_login(found(ATTRIBUTES_B), no_secret_read, &|_| {}, true)
            .unwrap()
            .unwrap();
        assert!(login.is_live(OffsetDateTime::now_utc()));
        backdate_secret_failure(&source);
        let login = source
            .read_keychain_login(
                found(ATTRIBUTES_B),
                secret(credentials_file(i64::MAX, "pro")),
                &|_| {},
                true,
            )
            .unwrap()
            .unwrap();
        assert_eq!(login.subscription_type.as_deref(), Some("pro"));
    }
}

#[test]
fn unreadable_secret_without_live_cache_is_not_signed_out() {
    let source = source();
    seed(&source, BLANK_LOGIN.into());
    source.logins.set_sign_in_required(true);
    let error = source
        .read_keychain_login(
            found(ATTRIBUTES_B),
            || macos_keychain::KeychainRead::Unreadable,
            &|_| {},
            true,
        )
        .err()
        .unwrap();
    assert_eq!(error.detail, Some(SourceErrorDetail::KeychainUnreadable));
    assert_eq!(source.logins.keychain(), CachedState::SignInRequired);
    assert!(
        source
            .read_keychain_login(found(ATTRIBUTES_B), no_secret_read, &|_| {}, true)
            .is_err()
    );
}

#[test]
fn background_never_reads_a_new_secret_even_after_backoff() {
    let source = source();
    assert!(
        source
            .read_keychain_login(found(ATTRIBUTES_A), no_secret_read, &|_| {}, false)
            .is_err()
    );
    seed(&source, credentials_file(i64::MAX, "max"));
    let kept = source
        .read_keychain_login(found(ATTRIBUTES_B), no_secret_read, &|_| {}, false)
        .unwrap()
        .unwrap();
    assert!(kept.is_live(OffsetDateTime::now_utc()));
    source
        .read_keychain_login(
            found(ATTRIBUTES_B),
            || macos_keychain::KeychainRead::Unreadable,
            &|_| {},
            true,
        )
        .unwrap();
    backdate_secret_failure(&source);
    assert!(
        source
            .read_keychain_login(found(ATTRIBUTES_B), no_secret_read, &|_| {}, false)
            .unwrap()
            .is_some()
    );
}

#[test]
fn an_mcp_only_item_is_no_login_and_detection_skips_it() {
    let source = source();
    seed(&source, MCP_ONLY.into());
    assert_eq!(source.logins.keychain(), CachedState::NoLogin);
    assert!(
        source
            .read_keychain_login(found(ATTRIBUTES_A), no_secret_read, &|_| {}, false)
            .unwrap()
            .is_none()
    );
    let mut probe = RecordingPresence {
        keychain: Some(KeychainMetadata::Found(ATTRIBUTES_A.to_vec())),
        ..Default::default()
    };
    probe.paths.insert(PRESENCE_CONFIG.into(), Ok(true));
    let presence = detect_presence(
        &probe,
        &source.logins,
        Some(Path::new(PRESENCE_CREDENTIALS)),
        None,
        || PiStatus::Unknown,
    );
    assert_eq!(presence.detection, Detection::InstalledNotSignedIn);
}

#[test]
fn metadata_failure_preserves_confirmed_detection() {
    let source = source();
    seed(&source, BLANK_LOGIN.into());
    source.logins.set_sign_in_required(true);
    let probe = RecordingPresence {
        keychain: Some(KeychainMetadata::Unreadable),
        ..Default::default()
    };
    let detected = || {
        detect_presence(
            &probe,
            &source.logins,
            Some(Path::new(PRESENCE_CREDENTIALS)),
            None,
            || panic!("must not ask Pi"),
        )
    };
    assert_eq!(
        detected(),
        Presence::via(Detection::SignInRequired, LoginCarrier::ClaudeKeychain)
    );
    assert!(
        source
            .read_keychain_login(
                || KeychainMetadata::Unreadable,
                no_secret_read,
                &|_| {},
                false
            )
            .is_err()
    );
    assert_eq!(
        detected(),
        Presence::via(Detection::SignInRequired, LoginCarrier::ClaudeKeychain)
    );
}
