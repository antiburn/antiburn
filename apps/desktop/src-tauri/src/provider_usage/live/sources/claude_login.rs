//! The Claude login as this app reads it, and the caches that keep a
//! carrier from being read twice at one change marker.
//!
//! [`super::anthropic_fetch`] owns the reads and the refresh policy. This
//! module holds the parsed login, the change markers, and the cached state
//! that detection consults. See that module's doc for the rules.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;
use time::OffsetDateTime;

use crate::provider_usage::live::model::Detection;

#[cfg(target_os = "macos")]
use super::claude_touch;

/// The credentials file is a small, purpose-built OAuth token store, not a
/// general state file — cap the read defensively rather than trust that.
pub(super) const MAX_CREDENTIAL_BYTES: u64 = 256 * 1024;

/// What the last check learned about one native carrier at its current
/// change marker. Detection reads it and never reads a secret itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CachedState {
    /// No check read this carrier at its current marker yet.
    Unknown,
    /// The carrier holds no Claude login, for example an MCP-only item.
    NoLogin,
    /// The carrier holds a login that is live or can still refresh.
    Usable,
    /// The CLI refresh could not recover the login. Only a new sign-in
    /// changes this.
    SignInRequired,
}

impl CachedState {
    /// The detection for a carrier that exists. `None` means "no login
    /// here", so detection goes on to the next rule. A carrier that no check
    /// read yet counts as signed in until the first check corrects it.
    pub(super) fn detection(self) -> Option<Detection> {
        match self {
            CachedState::Unknown | CachedState::Usable => Some(Detection::SignedIn),
            CachedState::SignInRequired => Some(Detection::SignInRequired),
            CachedState::NoLogin => None,
        }
    }
}

/// The cached login states detection may consult. Each method gets only
/// metadata and never reads a secret.
pub(super) trait LoginStates {
    /// The state of the credentials file at `path`, by its file metadata.
    fn file(&self, path: &Path) -> CachedState;
    /// The state of the Keychain item, by its attribute output.
    #[cfg(target_os = "macos")]
    fn keychain(&self, attributes: &[u8]) -> CachedState;
}

/// No check ran yet: every carrier that exists counts as signed in.
#[cfg(test)]
pub(super) struct NoCachedLogins;

#[cfg(test)]
impl LoginStates for NoCachedLogins {
    fn file(&self, _path: &Path) -> CachedState {
        CachedState::Unknown
    }
    #[cfg(target_os = "macos")]
    fn keychain(&self, _attributes: &[u8]) -> CachedState {
        CachedState::Unknown
    }
}

/// What this source needs out of the CLI's own credential file. Nothing more
/// is read. Of `refreshToken`, only its presence is kept: the value stays in
/// the carrier, because this source never redeems it.
#[derive(Clone)]
pub(super) struct ClaudeCredentials {
    pub(super) access_token: String,
    pub(super) expires_at_ms: i64,
    /// Whether the carrier holds a refresh token that is not empty.
    pub(super) has_refresh_token: bool,
    pub(super) subscription_type: Option<String>,
    /// The finer-grained tier within `subscriptionType`, for example
    /// `default_claude_max_5x`.
    pub(super) rate_limit_tier: Option<String>,
}

impl ClaudeCredentials {
    /// A login with no usable token: an empty access token, or an expiry of
    /// zero or less. See the `anthropic_fetch` module doc.
    pub(super) fn is_blank(&self) -> bool {
        self.access_token.is_empty() || self.expires_at_ms <= 0
    }

    pub(super) fn is_live(&self, now: OffsetDateTime) -> bool {
        !self.is_blank() && i128::from(self.expires_at_ms) > now.unix_timestamp_nanos() / 1_000_000
    }

    /// Whether the CLI can refresh this login without a new sign-in.
    pub(super) fn can_refresh(&self) -> bool {
        !self.is_blank() && self.has_refresh_token
    }

    /// Whether this live token expires within `window` from `now`.
    pub(super) fn expires_within(&self, now: OffsetDateTime, window: time::Duration) -> bool {
        i128::from(self.expires_at_ms) <= (now + window).unix_timestamp_nanos() / 1_000_000
    }
}

/// Parse the `{"claudeAiOauth": {...}}` shape both carriers hold — the
/// Keychain's raw value and the credentials file's contents are the same
/// JSON, so one function reads either. `None` means "no Claude login": the
/// input is not JSON, or it has no `claudeAiOauth` object. An object with
/// missing or blank fields is a blank login — see the `anthropic_fetch` module doc.
pub(super) fn parse_credentials_json(contents: &str) -> Option<ClaudeCredentials> {
    let value: Value = serde_json::from_str(contents).ok()?;
    let oauth = value.get("claudeAiOauth")?.as_object()?;
    let access_token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let expires_at_ms = oauth
        .get("expiresAt")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let has_refresh_token = oauth
        .get("refreshToken")
        .and_then(Value::as_str)
        .is_some_and(|token| !token.is_empty());
    Some(ClaudeCredentials {
        access_token,
        expires_at_ms,
        has_refresh_token,
        subscription_type: oauth
            .get("subscriptionType")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        rate_limit_tier: oauth
            .get("rateLimitTier")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
    })
}

/// The macOS Keychain carrier.
///
/// On macOS, the Claude CLI does not always write
/// `~/.claude/.credentials.json` — its credential can instead live only in
/// the login keychain, as a generic-password item this module reads the same
/// way the reader themselves would: by spawning `security
/// find-generic-password`. The operating system applies its own access
/// control to that read exactly as it would to the reader typing the same
/// command, prompting if it judges a prompt is owed — the subprocess is the
/// ordinary way to ask, not a way around being asked.
#[cfg(target_os = "macos")]
pub(super) mod macos_keychain {
    use std::io::Read as _;
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A Keychain read is normally instant. The one way it is not is a
    /// stale access-control prompt waiting on a person who is not there to
    /// answer it — this is a background scheduler, not an interactive
    /// terminal — so this carrier is abandoned, not awaited, past this
    /// deadline, and the file carrier is tried instead.
    const TIMEOUT: Duration = Duration::from_secs(3);

    /// Matches the credentials-file cap: this is a small OAuth token store
    /// wherever it lives, not a reason to trust an unbounded read.
    const MAX_BYTES: usize = super::MAX_CREDENTIAL_BYTES as usize;

    const SERVICE_NAME: &str = "Claude Code-credentials";

    /// The exit code `security find-generic-password` returns when the named
    /// item does not exist in the keychain — `errSecItemNotFound`.
    const ITEM_NOT_FOUND_EXIT_CODE: i32 = 44;

    /// What one Keychain read found.
    #[derive(Debug, PartialEq, Eq)]
    pub enum KeychainRead {
        /// The item does not exist. The ordinary state of a machine where the
        /// CLI has never signed in through the Keychain.
        Absent,
        /// The read failed for a reason other than "item not found": a
        /// timeout, a spawn failure, or an exit this carrier does not
        /// recognize. This carrier cannot say whether a credential exists.
        Unreadable,
        /// The raw JSON `security` printed to stdout.
        Found(String),
    }

    impl KeychainRead {
        /// A fixed name for this outcome. It contains no secret.
        pub(super) fn outcome(&self) -> &'static str {
            match self {
                Self::Absent => "absent",
                Self::Unreadable => "unreadable",
                Self::Found(_) => "found",
            }
        }
    }

    /// Reads one Keychain item. See [`KeychainRead`] for what each outcome
    /// means. Every outcome is logged once, with the `security` exit code
    /// when the process reported one. The log never contains the secret.
    pub fn read() -> KeychainRead {
        let (read, exit_code) = read_with_exit_code();
        ::tracing::debug!(
            event = "claude_keychain_read",
            outcome = read.outcome(),
            exit_code
        );
        read
    }

    fn read_with_exit_code() -> (KeychainRead, Option<i32>) {
        let mut child = match antiburn_local::platform::process::headless_std_command("security")
            .args(["find-generic-password", "-s", SERVICE_NAME, "-w"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return (KeychainRead::Unreadable, None),
        };

        let Some(mut stdout) = child.stdout.take() else {
            return (KeychainRead::Unreadable, None);
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = Vec::with_capacity(4096);
            let mut chunk = [0_u8; 4096];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        buffer.extend_from_slice(&chunk[..read]);
                        if buffer.len() > MAX_BYTES {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(buffer);
        });

        let bytes = match rx.recv_timeout(TIMEOUT) {
            Ok(bytes) => bytes,
            Err(_) => {
                // Abandoned, not awaited further: kill it, and do not wait
                // for the reader thread — it will unblock on its own once
                // the pipe closes and simply have nowhere left to send.
                let _ = child.kill();
                let exit_code = child.wait().ok().and_then(|status| status.code());
                return (KeychainRead::Unreadable, exit_code);
            }
        };
        let Ok(status) = child.wait() else {
            return (KeychainRead::Unreadable, None);
        };
        if !status.success() {
            return (classify_failed_exit(status.code()), status.code());
        }
        if bytes.is_empty() || bytes.len() > MAX_BYTES {
            return (KeychainRead::Unreadable, status.code());
        }
        let read = match String::from_utf8(bytes) {
            Ok(text) => KeychainRead::Found(text),
            Err(_) => KeychainRead::Unreadable,
        };
        (read, status.code())
    }

    /// Whether a nonzero exit from `security find-generic-password` means
    /// "item not found" or something this carrier could not diagnose.
    ///
    /// A pure function so the one distinction this fix depends on — exit
    /// code 44 versus everything else — has a test that does not need to
    /// spawn `security` itself.
    fn classify_failed_exit(exit_code: Option<i32>) -> KeychainRead {
        if exit_code == Some(ITEM_NOT_FOUND_EXIT_CODE) {
            KeychainRead::Absent
        } else {
            KeychainRead::Unreadable
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn item_not_found_reads_as_absent() {
            assert_eq!(
                classify_failed_exit(Some(ITEM_NOT_FOUND_EXIT_CODE)),
                KeychainRead::Absent
            );
        }

        #[test]
        fn any_other_exit_reads_as_unreadable() {
            assert_eq!(classify_failed_exit(Some(1)), KeychainRead::Unreadable);
            assert_eq!(classify_failed_exit(None), KeychainRead::Unreadable);
        }
    }
}

/// The change marker of the Keychain item: its `mdat` value and a hash of
/// the whole attribute output. `mdat` has a one-second precision, so the
/// hash also catches two writes in one second.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct KeychainMarker {
    pub(super) mdat: Option<String>,
    pub(super) attributes_hash: u64,
}

#[cfg(target_os = "macos")]
impl KeychainMarker {
    pub(super) fn from_attributes(attributes: &[u8]) -> KeychainMarker {
        KeychainMarker {
            mdat: parse_mdat(attributes),
            attributes_hash: claude_touch::hash(attributes),
        }
    }
}

/// The `mdat` value from `security find-generic-password` attribute output,
/// for example `20260101120000Z` from
/// `"mdat"<timedate>=0x3230… "20260101120000Z\000"`.
#[cfg(target_os = "macos")]
pub(super) fn parse_mdat(attributes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(attributes);
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with("\"mdat\""))?;
    let (_, value) = line.split_once('=')?;
    let (_, quoted) = value.split_once('"')?;
    let (date, _) = quoted.split_once('"')?;
    let date = date.trim_end_matches("\\000");
    (!date.is_empty()).then(|| date.to_owned())
}

/// The change marker of the credentials file. The content hash decides
/// whether the file changed. Detection compares only `modified` and `len`,
/// because detection reads no file contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FileMarker {
    pub(super) content_hash: u64,
    pub(super) modified: Option<std::time::SystemTime>,
    pub(super) len: u64,
}

/// One native carrier's parsed login at one change marker.
pub(super) struct CachedLogin<M> {
    pub(super) marker: M,
    /// `None` when the carrier holds no Claude login.
    pub(super) login: Option<ClaudeCredentials>,
    /// Set when the CLI refresh could not recover this login. A new marker
    /// starts without it.
    pub(super) sign_in_required: bool,
}

impl<M> CachedLogin<M> {
    pub(super) fn state(&self) -> CachedState {
        match &self.login {
            None => CachedState::NoLogin,
            Some(_) if self.sign_in_required => CachedState::SignInRequired,
            Some(_) => CachedState::Usable,
        }
    }
}

/// The cached logins of the native carriers. See the `anthropic_fetch` module doc's
/// "Reading a carrier only when it changed" section.
#[derive(Default)]
pub(super) struct NativeLogins {
    #[cfg(target_os = "macos")]
    pub(super) keychain: Mutex<Option<CachedLogin<KeychainMarker>>>,
    /// The marker where the last secret read failed, and when. A denied or
    /// ignored Keychain prompt must not come back on every check.
    #[cfg(target_os = "macos")]
    pub(super) keychain_secret_failed: Mutex<Option<(KeychainMarker, std::time::Instant)>>,
    pub(super) file: Mutex<Option<CachedLogin<FileMarker>>>,
    /// Login and refresh outcomes that wait for the analytics pass.
    #[cfg(feature = "analytics")]
    pub(super) observations: Mutex<Vec<crate::provider_usage::live::LoginObservation>>,
}

/// The most outcomes that wait for the analytics pass. Later outcomes are
/// dropped until the pass takes them.
#[cfg(feature = "analytics")]
pub(super) const MAX_PENDING_OBSERVATIONS: usize = 16;

impl NativeLogins {
    /// Record whether the CLI refresh could not recover the cached logins.
    pub(super) fn set_sign_in_required(&self, required: bool) {
        #[cfg(target_os = "macos")]
        if let Some(cached) = lock(&self.keychain).as_mut() {
            cached.sign_in_required = required && cached.login.is_some();
        }
        if let Some(cached) = lock(&self.file).as_mut() {
            cached.sign_in_required = required && cached.login.is_some();
        }
    }

    /// Keep one login or refresh outcome for the analytics pass. Without the
    /// analytics feature, this does nothing.
    pub(super) fn observe(&self, label: &'static str, detail: Option<&'static str>) {
        #[cfg(feature = "analytics")]
        {
            let mut observations = lock(&self.observations);
            if observations.len() < MAX_PENDING_OBSERVATIONS {
                observations.push(crate::provider_usage::live::LoginObservation { label, detail });
            }
        }
        #[cfg(not(feature = "analytics"))]
        let _ = (label, detail);
    }
}

impl LoginStates for NativeLogins {
    fn file(&self, path: &Path) -> CachedState {
        let Ok(metadata) = fs::metadata(path) else {
            return CachedState::Unknown;
        };
        match lock(&self.file).as_ref() {
            Some(cached)
                if cached.marker.len == metadata.len()
                    && cached.marker.modified.is_some()
                    && cached.marker.modified == metadata.modified().ok() =>
            {
                cached.state()
            }
            _ => CachedState::Unknown,
        }
    }

    #[cfg(target_os = "macos")]
    fn keychain(&self, attributes: &[u8]) -> CachedState {
        let marker = KeychainMarker::from_attributes(attributes);
        match lock(&self.keychain).as_ref() {
            Some(cached) if cached.marker == marker => cached.state(),
            _ => CachedState::Unknown,
        }
    }
}

/// Lock a mutex. A poisoned lock keeps its value: every value here is a
/// cache that a later check can replace.
pub(super) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A live token that expires within this time starts the CLI refresh before
/// the usage call. The token then does not expire between two checks.
pub(super) const PRE_EXPIRY_WINDOW: time::Duration = time::Duration::minutes(10);

/// What the refresh policy is for the native logins of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RefreshTrigger {
    /// A login that can refresh has expired or was rejected.
    Expired,
    /// A live login that can refresh expires within [`PRE_EXPIRY_WINDOW`].
    PreExpiry,
    /// No native login can refresh: each is blank or has no refresh token.
    CannotRefresh,
}

impl RefreshTrigger {
    /// The analytics detail for this trigger. A fixed value.
    pub(super) fn name(self) -> &'static str {
        match self {
            RefreshTrigger::Expired => "expired",
            RefreshTrigger::PreExpiry => "pre_expiry",
            RefreshTrigger::CannotRefresh => "cannot_refresh",
        }
    }

    /// The trigger for native logins that the usage endpoint did not accept.
    pub(super) fn after_rejection(native: &[ClaudeCredentials]) -> RefreshTrigger {
        if native.iter().any(ClaudeCredentials::can_refresh) {
            RefreshTrigger::Expired
        } else {
            RefreshTrigger::CannotRefresh
        }
    }
}

/// Whether the live native logins all expire soon, and one of them can
/// refresh. See [`PRE_EXPIRY_WINDOW`].
pub(super) fn expires_soon(native: &[ClaudeCredentials], now: OffsetDateTime) -> bool {
    let mut live = native.iter().filter(|login| login.is_live(now)).peekable();
    live.peek().is_some()
        && native
            .iter()
            .filter(|login| login.is_live(now))
            .all(|login| login.expires_within(now, PRE_EXPIRY_WINDOW))
        && live.any(ClaudeCredentials::can_refresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    /// A blank login in the shape the CLI leaves after its login expires
    /// for good. Synthetic values only.
    const BLANK_LOGIN: &str = r#"{"claudeAiOauth":{"accessToken":"","refreshToken":"","expiresAt":0,"scopes":[],"subscriptionType":"max"}}"#;

    /// An item that holds only MCP logins.
    const MCP_ONLY: &str = r#"{"mcpOAuth":{"synthetic-server":{}}}"#;

    /// The Keychain and the file carrier hold the identical JSON shape, so
    /// this exercises `parse_credentials_json` directly against a synthetic
    /// value shaped exactly like what `security find-generic-password -w`
    /// prints — including the fields this source never reads
    /// (`refreshTokenExpiresAt`, `scopes`), to confirm they are ignored
    /// rather than tripping the parser. `rateLimitTier` is read, into
    /// `plan_tier` on the resulting snapshot.
    #[test]
    fn keychain_shaped_json_parses_through_the_same_function_as_the_file() {
        let keychain_value = format!(
            r#"{{"claudeAiOauth": {{"accessToken": "synthetic-token",
              "refreshToken": "synthetic-refresh", "expiresAt": {},
              "refreshTokenExpiresAt": {}, "scopes": ["user:inference"],
              "subscriptionType": "max", "rateLimitTier": "default_claude_max_5x"}}}}"#,
            NOW * 1_000,
            (NOW + 30_000_000) * 1_000
        );
        let credentials = parse_credentials_json(&keychain_value).expect("parses");
        assert_eq!(credentials.access_token, "synthetic-token");
        assert_eq!(credentials.expires_at_ms, NOW * 1_000);
        assert_eq!(credentials.subscription_type.as_deref(), Some("max"));
        assert_eq!(
            credentials.rate_limit_tier.as_deref(),
            Some("default_claude_max_5x")
        );
    }

    #[test]
    fn unparseable_keychain_shaped_text_reads_as_absent() {
        assert!(parse_credentials_json("not json at all").is_none());
        assert!(parse_credentials_json(r#"{"somethingElse": true}"#).is_none());
    }

    #[test]
    fn a_blank_login_is_an_expired_login_and_mcp_only_is_no_login() {
        let blank = parse_credentials_json(BLANK_LOGIN).expect("a blank login is a login");
        assert!(blank.is_blank());
        assert!(!blank.is_live(OffsetDateTime::from_unix_timestamp(NOW).unwrap()));
        assert!(!blank.can_refresh());

        // An object with missing fields is blank, too.
        let missing = parse_credentials_json(r#"{"claudeAiOauth":{}}"#).expect("a login");
        assert!(missing.is_blank());

        // Only the presence of the refresh token is kept.
        let refreshable = parse_credentials_json(&format!(
            r#"{{"claudeAiOauth":{{"accessToken":"synthetic","refreshToken":"synthetic-refresh","expiresAt":{}}}}}"#,
            NOW * 1_000
        )).unwrap();
        assert!(refreshable.has_refresh_token);
        assert!(refreshable.can_refresh());
        let no_refresh = parse_credentials_json(&format!(
            r#"{{"claudeAiOauth":{{"accessToken":"synthetic","expiresAt":{}}}}}"#,
            NOW * 1_000
        ))
        .unwrap();
        assert!(!no_refresh.has_refresh_token);
        assert!(!no_refresh.can_refresh());

        assert!(parse_credentials_json(MCP_ONLY).is_none());
        assert!(parse_credentials_json(r#"{"claudeAiOauth":null}"#).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_marker_holds_mdat_and_the_attribute_hash() {
        let first: &[u8] = b"attributes:\n    \"mdat\"<timedate>=0x3230323630313031313230303030305A00  \"20260101120000Z\\000\"\n";
        let second: &[u8] = b"attributes:\n    \"mdat\"<timedate>=0x3230323630313031313230303031305A00  \"20260101120001Z\\000\"\n";
        let marker = KeychainMarker::from_attributes(first);
        assert_eq!(marker.mdat.as_deref(), Some("20260101120000Z"));
        assert_ne!(marker, KeychainMarker::from_attributes(second));
        assert_eq!(parse_mdat(b"no attributes"), None);
    }
}
