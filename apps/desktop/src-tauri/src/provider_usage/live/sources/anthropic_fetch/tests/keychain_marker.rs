//! Tests for the Keychain change marker. The source reads the secret only
//! when the item changes.

use super::*;
use std::cell::{Cell, RefCell};

const ATTRIBUTES_A: &[u8] = b"keychain: \"/fixture/login.keychain-db\"\nattributes:\n    \"mdat\"<timedate>=0x32303236303130313132303030305A00  \"20260101120000Z\\000\"\n    \"svce\"<blob>=\"Claude Code-credentials\"\n";
const ATTRIBUTES_B: &[u8] = b"keychain: \"/fixture/login.keychain-db\"\nattributes:\n    \"mdat\"<timedate>=0x32303236303130313132303030315A00  \"20260101120001Z\\000\"\n    \"svce\"<blob>=\"Claude Code-credentials\"\n";

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
    panic!("the same marker must not read the secret")
}

#[test]
fn the_same_marker_uses_the_cached_login_without_a_secret_read() {
    let source = source();
    let sleep = |_| panic!("a read that answers needs no retry");
    // An expired login is cached as well as a live one.
    let expired = credentials_file((real_now_secs() - 60) * 1_000, "max");
    let first = source
        .read_keychain_login(found(ATTRIBUTES_A), secret(expired), &sleep, false)
        .unwrap()
        .unwrap();
    assert!(!first.is_live(OffsetDateTime::now_utc()));
    let again = source
        .read_keychain_login(found(ATTRIBUTES_A), no_secret_read, &sleep, false)
        .unwrap()
        .unwrap();
    assert_eq!(again.expires_at_ms, first.expires_at_ms);
    // A blank login is cached, too.
    let source = self::source();
    source
        .read_keychain_login(
            found(ATTRIBUTES_A),
            secret(BLANK_LOGIN.into()),
            &sleep,
            false,
        )
        .unwrap()
        .unwrap();
    let blank = source
        .read_keychain_login(found(ATTRIBUTES_A), no_secret_read, &sleep, false)
        .unwrap()
        .unwrap();
    assert!(blank.is_blank());
}

#[test]
fn a_changed_marker_reads_the_secret_once() {
    let source = source();
    let reads = Cell::new(0);
    let read = |body: String| {
        reads.set(reads.get() + 1);
        macos_keychain::KeychainRead::Found(body)
    };
    let sleep = |_| {};
    let old = credentials_file((real_now_secs() + 3_600) * 1_000, "max");
    let new = credentials_file((real_now_secs() + 7_200) * 1_000, "pro");
    source
        .read_keychain_login(found(ATTRIBUTES_A), || read(old), &sleep, false)
        .unwrap();
    let login = source
        .read_keychain_login(found(ATTRIBUTES_B), || read(new), &sleep, false)
        .unwrap()
        .unwrap();
    assert_eq!(login.subscription_type.as_deref(), Some("pro"));
    source
        .read_keychain_login(found(ATTRIBUTES_B), no_secret_read, &sleep, false)
        .unwrap();
    assert_eq!(reads.get(), 2);
}

#[test]
fn an_absent_item_clears_the_cache_without_a_secret_read() {
    let source = source();
    let sleep = |_| {};
    source
        .read_keychain_login(
            found(ATTRIBUTES_A),
            secret(credentials_file(i64::MAX, "max")),
            &sleep,
            false,
        )
        .unwrap();
    let absent = source
        .read_keychain_login(|| KeychainMetadata::Absent, no_secret_read, &sleep, false)
        .unwrap();
    assert!(absent.is_none());
    assert!(lock(&source.logins.keychain).is_none());
}

#[test]
fn a_failed_attribute_read_retries_then_keeps_the_cache() {
    let source = source();
    let sleeps = RefCell::new(Vec::new());
    let sleep = |delay| sleeps.borrow_mut().push(delay);
    let attempts = Cell::new(0);
    let unreadable = || {
        attempts.set(attempts.get() + 1);
        KeychainMetadata::Unreadable
    };
    // No cache: the check cannot tell, so it reports the Keychain.
    let failure = source
        .read_keychain_login(unreadable, no_secret_read, &sleep, false)
        .err()
        .expect("a Keychain failure");
    assert_eq!(failure.detail, Some(SourceErrorDetail::KeychainUnreadable));
    assert_eq!(attempts.get(), 4);
    assert_eq!(*sleeps.borrow(), claude_touch::RETRY_DELAYS.to_vec());

    // With a cache: the cached login stays and no secret is read.
    source
        .read_keychain_login(
            found(ATTRIBUTES_A),
            secret(credentials_file(i64::MAX, "max")),
            &sleep,
            false,
        )
        .unwrap();
    let kept = source
        .read_keychain_login(
            || KeychainMetadata::Unreadable,
            no_secret_read,
            &sleep,
            false,
        )
        .unwrap();
    assert!(kept.is_some());

    // A read that recovers on a retry continues as normal.
    let calls = Cell::new(0);
    let flaky = || {
        calls.set(calls.get() + 1);
        if calls.get() < 3 {
            KeychainMetadata::Unreadable
        } else {
            KeychainMetadata::Found(ATTRIBUTES_A.to_vec())
        }
    };
    assert!(
        source
            .read_keychain_login(flaky, no_secret_read, &sleep, false)
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_listed_item_whose_secret_reads_as_absent_is_a_keychain_failure() {
    let source = source();
    let sleep = |_| {};
    let failure = source
        .read_keychain_login(
            found(ATTRIBUTES_A),
            || macos_keychain::KeychainRead::Absent,
            &sleep,
            false,
        )
        .err()
        .expect("a Keychain failure");
    assert_eq!(failure.error, ProviderUsageError::Unavailable);
    assert_eq!(failure.detail, Some(SourceErrorDetail::KeychainUnreadable));
    // With no other carrier, the failure reaches the outcome instead
    // of an empty success that removes the provider.
    let outcome = with_carrier_error(Ok(None), Some(failure)).unwrap_err();
    assert_eq!(outcome.detail, Some(SourceErrorDetail::KeychainUnreadable));
    // A background check does not read the secret again at the same marker,
    // so a denied prompt does not come back.
    let again = source
        .read_keychain_login(found(ATTRIBUTES_A), no_secret_read, &sleep, false)
        .err()
        .expect("the same Keychain failure");
    assert_eq!(again.detail, Some(SourceErrorDetail::KeychainUnreadable));
    // A check that the reader started waits for the backoff, too.
    assert!(
        source
            .read_keychain_login(found(ATTRIBUTES_A), no_secret_read, &sleep, true)
            .is_err()
    );
    // After the backoff, a check that the reader started reads again.
    backdate_secret_failure(&source);
    assert!(
        source
            .read_keychain_login(found(ATTRIBUTES_A), no_secret_read, &sleep, false)
            .is_err()
    );
    assert!(
        source
            .read_keychain_login(
                found(ATTRIBUTES_A),
                secret(credentials_file(i64::MAX, "max")),
                &sleep,
                true,
            )
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_changed_marker_reads_the_secret_again_after_a_failed_read() {
    let source = source();
    let sleep = |_| {};
    assert!(
        source
            .read_keychain_login(
                found(ATTRIBUTES_A),
                || macos_keychain::KeychainRead::Unreadable,
                &sleep,
                false,
            )
            .is_err()
    );
    assert!(
        source
            .read_keychain_login(
                found(ATTRIBUTES_B),
                secret(credentials_file(i64::MAX, "max")),
                &sleep,
                false,
            )
            .unwrap()
            .is_some()
    );
}

/// Move the last failed secret read back past the backoff.
fn backdate_secret_failure(source: &ClaudeDirectFetch) {
    let mut failed = lock(&source.logins.keychain_secret_failed);
    let (marker, _) = failed.take().expect("a failed secret read");
    let at = std::time::Instant::now()
        .checked_sub(SECRET_RETRY_BACKOFF)
        .expect("a past instant");
    *failed = Some((marker, at));
}

#[test]
fn an_mcp_only_item_is_no_login_and_detection_skips_it() {
    let source = source();
    let sleep = |_| {};
    let login = source
        .read_keychain_login(found(ATTRIBUTES_A), secret(MCP_ONLY.into()), &sleep, false)
        .unwrap();
    assert!(login.is_none());
    assert_eq!(source.logins.keychain(ATTRIBUTES_A), CachedState::NoLogin);
    // A changed item counts as signed in until a check reads it.
    assert_eq!(source.logins.keychain(ATTRIBUTES_B), CachedState::Unknown);

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

    // A login that needs a new sign-in reads as such.
    let source = self::source();
    source
        .read_keychain_login(
            found(ATTRIBUTES_A),
            secret(BLANK_LOGIN.into()),
            &sleep,
            false,
        )
        .unwrap();
    source.logins.set_sign_in_required(true);
    let presence = detect_presence(
        &probe,
        &source.logins,
        Some(Path::new(PRESENCE_CREDENTIALS)),
        None,
        || PiStatus::Unknown,
    );
    assert_eq!(
        presence,
        Presence::via(Detection::SignInRequired, LoginCarrier::ClaudeKeychain)
    );
}
