//! Ask Claude directly for the reader's own plan usage.
//!
//! # The credential
//!
//! The Claude CLI keeps its OAuth tokens in one of two places, depending on
//! platform, and this source tries both:
//!
//! - **The macOS Keychain**, first, on macOS only — the same generic-password
//!   item the CLI itself reads and writes, service name
//!   `"Claude Code-credentials"`. Read by spawning `security
//!   find-generic-password`, exactly as if the reader had typed it
//!   themselves — the operating system applies the same access control to
//!   that subprocess as it would to the reader's own terminal; see
//!   [`macos_keychain`] for the deadline that keeps a stuck read from ever
//!   hanging the scheduler.
//! - **`$CLAUDE_CONFIG_DIR/.credentials.json`** when that variable is set,
//!   and `~/.claude/.credentials.json` otherwise — on every platform, and as
//!   the fallback on macOS when the Keychain carrier answers nothing.
//!
//! Both carriers hold the identical JSON shape, `{"claudeAiOauth": {...}}`,
//! so one parser reads whichever one this source obtained. This source reads
//! the access token out of it and calls the provider's own usage endpoint
//! with the reader's own credential, the same request the CLI itself would
//! make. No key of ours, no service of ours in between.
//!
//! **This source never refreshes that token itself.** Refreshing is a
//! lifecycle decision — issuing a new credential under the reader's account —
//! and it belongs to the tool that owns the token's lifecycle, which is the
//! CLI, not a background reader of its credential. If `expiresAt` has already
//! passed on every carrier, that is reported as an authentication failure
//! without a network call.
//!
//! A `claudeAiOauth` object that is present but blank (an empty
//! `accessToken`, or an `expiresAt` of zero or less) is an expired login, not
//! "no login". The CLI leaves this shape after its login expires for good. An
//! item without `claudeAiOauth` (for example an item that holds only MCP
//! logins) is "no Claude login". The parser checks only whether
//! `refreshToken` is present and not empty. It keeps no refresh token value.
//!
//! # Delegating refresh to the CLI
//!
//! When every **native** Claude carrier (the Keychain item or the
//! credentials file, not Pi's read-only copy) holds an expired or rejected
//! token, this source asks the CLI to refresh its own credential:
//! [`claude_touch`] spawns `claude` in a PTY, types `/status`, verifies the
//! carrier changed and settled by polling *metadata only* — never the secret
//! — and only then does this source read the secret once, through the normal
//! parser, and retry the usage call once. Background checks do this too, so
//! a reading does not go stale while no usage surface is open. A live token
//! that expires within [`super::claude_login::PRE_EXPIRY_WINDOW`] also starts the touch, before
//! the usage call. A network or 5xx failure never triggers the touch, only
//! the expired/rejected credential state. No native login means no touch:
//! the CLI would only show its login screen. The Pi carrier alone never
//! triggers it either — it recovers when the reader next uses Pi.
//!
//! A login that cannot refresh (a blank login, or a login without a refresh
//! token) gets one touch per change of its carrier. When that touch does not
//! recover it, the check reports [`SourceErrorDetail::SignInRequired`]. A
//! login with no `claude` CLI to refresh it reports
//! [`SourceErrorDetail::CliMissing`].
//!
//! # Reading a carrier only when it changed
//!
//! On macOS each check first reads the Keychain item's attributes, which
//! never raises a prompt. The item's change marker is its `mdat` value and a
//! hash of the whole attribute output. The parsed login is cached with that
//! marker:
//!
//! - The same marker uses the cached login. The secret is not read.
//! - A changed marker reads the secret once and replaces the cache.
//! - An absent item clears the cache.
//! - A failed attribute read is retried ([`claude_touch::with_retries`]).
//!   When every retry fails, the check keeps the cached login and does not
//!   read the secret.
//!
//! The credentials file uses a content hash in the same way: an unchanged
//! file is not parsed again. Detection uses the same caches, so it never
//! reads a secret.
//!
//! Pi's OAuth store is a third, read-only carrier — see [`super::pi_auth`].
//! The same rule holds: its token is never refreshed here. When that entry
//! has expired, the one recovery lever is delegating the refresh to Pi's
//! own `auth check` command through [`super::pi_refresh`], which runs Pi's
//! own locked refresh-and-write; when that lever is unavailable the
//! expired entry reads exactly as it did before the lever existed.
//!
//! Finding neither carrier — no Keychain item, no credentials file, or
//! either one without a Claude login — is not an error here. The source
//! has nothing to report. When Claude Code is installed, [`super::collect`]
//! reports that state as "not signed in", so the meter stays visible.
//!
//! A Keychain read that cannot say whether the item exists is different from
//! one that finds it absent. `security find-generic-password` exits 44 for
//! "item not found"; a timeout, a spawn failure, or any other exit code
//! means this carrier could not diagnose the item at all. That case is
//! reported as [`ProviderUsageError::Unavailable`] instead of falling back
//! to the credentials file, so a transient Keychain failure cannot read as
//! "signed out" and erase a cached reading — see [`Cooldown`]'s doc for what
//! a real failure does to the last good snapshot.
//!
//! # Retrying and going quiet
//!
//! Every other failure — a rejected credential, a rate limit, an
//! unreachable network, a response this build cannot parse — goes through
//! [`Cooldown`], which is what keeps this source from opening a connection on
//! every poll: see that module for the retry and last-good-reading contract
//! every direct-fetch source shares, and for how a caller's own `max_age`
//! decides how often "every poll" actually reaches the network.
//!
//! # The CLI's own cache
//!
//! Before any of the above, this source checks [`claude_config_cache`] — the
//! same reading the Claude CLI itself cached the last time it called the
//! usage endpoint. Two tiers follow, in order:
//!
//! 1. **Cache pre-empts the network.** When the cached reading is no older
//!    than the caller's own `max_age`, it is returned as-is and no request is
//!    made at all — the cache is already at least as fresh as what the
//!    caller would have accepted from a live call.
//! 2. **Cache seeds a failure.** When the cache is not fresh enough to
//!    pre-empt the request, the live endpoint is still asked as normal. If
//!    that call fails and the cache is no older than [`cooldown::MAX_AGE`],
//!    the cached reading rides along as [`cooldown::FetchFailure::last_known`]
//!    — a real figure to show instead of nothing, while the error itself
//!    still reports that the endpoint did not just answer.
//!
//! Both tiers stamp the resulting snapshot with this source's own
//! [`SOURCE_ID`] and a label naming the cache, not the network — one
//! registered source, two ways of answering, the same pattern
//! [`super::codex_app_server`] uses for Codex's fallback.

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(feature = "analytics")]
use std::sync::Mutex;
#[cfg(feature = "analytics")]
use std::time::{Duration, Instant};

use serde_json::Value;
use time::OffsetDateTime;

use crate::provider_usage::live::SourceErrorDetail;
use crate::provider_usage::live::anthropic;
use crate::provider_usage::live::model::{
    Confidence, DesktopApp, Detection, Freshness, LoginCarrier, Presence, ProviderUsageError,
    ProviderUsageSnapshot, UsageSource,
};
use crate::provider_usage::live::{LiveUsageSource, SourceOutcome};

use super::claude_config_cache::{self, CachedUsage};
use super::claude_login::{
    CachedLogin, ClaudeCredentials, FileMarker, LoginStates, MAX_CREDENTIAL_BYTES, NativeLogins,
    RefreshTrigger, expires_soon, lock, parse_credentials_json,
};
#[cfg(test)]
use super::claude_login::{CachedState, NoCachedLogins};
#[cfg(target_os = "macos")]
use super::claude_login::{KeychainMarker, macos_keychain};
use super::claude_touch;
use super::cooldown::{self, Cooldown, FetchFailure};
use super::http;
use super::pi_auth;
use super::pi_refresh::{PiRefresher, PiStatus, Recovery};
#[cfg(target_os = "macos")]
use super::presence::KeychainMetadata;
use super::presence::{self, PresenceProbe, SystemPresenceProbe};

const MAX_CLAUDE_JSON_BYTES: u64 = 8 * 1024 * 1024;

const USAGE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
#[cfg(feature = "analytics")]
const LIMIT_RESET_ENDPOINT: &str =
    "https://api.anthropic.com/api/oauth/usage?at_wall=1&skip_spend=1";
const PROFILE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/profile";
/// A verified Claude Code identity for provider compatibility.
///
/// Keep this pinned until a newer identity is verified against the endpoint.
/// The diagnostic reports `cli_version` if the provider rejects this version.
const CLAUDE_CODE_COMPATIBILITY_USER_AGENT: &str = "claude-cli/2.1.261 (external, cli)";

#[cfg(feature = "analytics")]
const LIMIT_RESET_DIAGNOSTIC_COOLDOWN: Duration = Duration::from_secs(5 * 60);

/// The stable id [`SourceOutcome::error`] and the milestone engine key this
/// source under.
const SOURCE_ID: &str = "claude-usage-fetch";

/// The `max_age` ceiling under which a fetch reads as user-initiated.
///
/// The popover's refresh command asks with fifty seconds
/// (`POPOVER_LIVE_USAGE_MAX_AGE`) and the background monitor with its
/// five-minute tick, so the caller's own `max_age` already states who is
/// asking — threading a separate flag through the shared trait would only
/// restate it. A background check waits longer between CLI refresh attempts
/// — see [`claude_touch::BACKGROUND_TOUCH_COOLDOWN`].
const USER_INITIATED_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(60);

/// Whether this fetch's `max_age` marks it as user-initiated — see
/// [`USER_INITIATED_MAX_AGE`]. A user-initiated check uses the shorter touch
/// cooldown.
fn user_initiated(max_age: std::time::Duration) -> bool {
    max_age < USER_INITIATED_MAX_AGE
}

/// The credential file, at the one documented place it lives.
pub fn default_credentials_path() -> Option<PathBuf> {
    let dir = antiburn_local::paths::non_empty_env_path("CLAUDE_CONFIG_DIR")
        .or_else(|| antiburn_local::paths::home_dir().map(|home| home.join(".claude")))?;
    Some(dir.join(".credentials.json"))
}

/// The Keychain item the Claude CLI keeps its login in.
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// The CLI's own executable name. See [`super::cli_locator`] for where it is searched.
const BINARY: &str = "claude";

/// Presence rules, in order: the credentials file is a login; the Keychain
/// item is a login; the shared Pi file is answered by `pi_status` — Pi's own
/// verdict when the caller may ask, inconclusive otherwise; the config
/// directory or the binary is an install without a login; nothing is no
/// install.
///
/// `cached` qualifies a native carrier that exists: a carrier without a
/// Claude login is skipped, and a login that needs a new sign-in reads as
/// [`Detection::SignInRequired`].
fn detect_presence(
    probe: &impl PresenceProbe,
    cached: &impl LoginStates,
    credentials_path: Option<&Path>,
    pi_auth_path: Option<&Path>,
    pi_status: impl FnOnce() -> PiStatus,
) -> Presence {
    let Some(credentials_path) = credentials_path else {
        return Presence::UNKNOWN;
    };
    match presence::path_exists(probe, credentials_path) {
        Ok(true) => {
            if let Some(detection) = cached.file(credentials_path).detection() {
                return Presence::via(detection, LoginCarrier::ClaudeCredentialsFile);
            }
        }
        Ok(false) => {}
        Err(_) => return Presence::UNKNOWN,
    }
    #[cfg(target_os = "macos")]
    match probe.keychain_metadata(KEYCHAIN_SERVICE, None) {
        KeychainMetadata::Found(attributes) => {
            if let Some(detection) = cached.keychain(&attributes).detection() {
                return Presence::via(detection, LoginCarrier::ClaudeKeychain);
            }
        }
        KeychainMetadata::Absent => {}
        KeychainMetadata::Unreadable => return Presence::UNKNOWN,
    }
    if let Some(path) = pi_auth_path {
        match presence::path_exists(probe, path) {
            Ok(true) => return pi_presence(pi_status()),
            Ok(false) => {}
            Err(_) => return Presence::UNKNOWN,
        }
    }
    let Some(config_dir) = credentials_path.parent() else {
        return Presence::UNKNOWN;
    };
    match presence::path_exists(probe, config_dir) {
        Ok(true) => Presence::new(Detection::InstalledNotSignedIn),
        Err(_) => Presence::UNKNOWN,
        Ok(false) if probe.binary_present(BINARY) => Presence::new(Detection::InstalledNotSignedIn),
        Ok(false) => Presence::new(Detection::NotInstalled),
    }
}

/// Where Claude Desktop may be installed: app folders and a launcher on
/// `PATH`. Empty in tests, so that a test never depends on its machine.
#[derive(Default)]
struct DesktopAppLocations {
    paths: Vec<PathBuf>,
    binary: Option<&'static str>,
}

/// Note whether Claude Desktop is installed, whatever login was found.
///
/// Claude Desktop keeps its own sign-in, which antiburn does not read, so the
/// app never makes the meter signed in. The agent lists name the app even
/// when a login was found, for example a Claude login that only Pi holds.
/// The usage note names it only when no login was found. Only file metadata
/// is read.
fn with_claude_desktop(
    presence: Presence,
    probe: &impl PresenceProbe,
    app: &DesktopAppLocations,
) -> Presence {
    let installed = claude_desktop_installed(probe, app);
    presence.with_desktop_app(installed.then_some(DesktopApp::ClaudeDesktop))
}

/// Whether Claude Desktop is on disk. Only file metadata is read.
fn claude_desktop_installed(probe: &impl PresenceProbe, app: &DesktopAppLocations) -> bool {
    app.paths
        .iter()
        .any(|path| presence::path_exists(probe, path).unwrap_or(false))
        || app
            .binary
            .is_some_and(|binary| probe.binary_present(binary))
}

/// The failure a check reports when it found no Claude login at all and
/// Claude Desktop is the only Claude app here.
///
/// Claude Desktop keeps its own sign-in, which antiburn does not read, so the
/// limits cannot be checked. The views grey the meter and link to the docs.
/// A `claude` on `PATH` means Claude Code is installed but signed out, which
/// keeps the ordinary sign-in note, so this returns `None`.
fn desktop_only_failure(
    probe: &impl PresenceProbe,
    app: &DesktopAppLocations,
) -> Option<FetchFailure> {
    if !claude_desktop_installed(probe, app) || probe.binary_present(BINARY) {
        return None;
    }
    Some(FetchFailure {
        error: ProviderUsageError::Authentication,
        detail: Some(SourceErrorDetail::DesktopOnly),
        last_known: None,
    })
}

/// Claude Desktop on macOS: the app bundle.
#[cfg(target_os = "macos")]
fn claude_desktop_locations(home: Option<&Path>) -> DesktopAppLocations {
    let mut paths = vec![PathBuf::from("/Applications/Claude.app")];
    if let Some(home) = home {
        paths.push(home.join("Applications/Claude.app"));
    }
    DesktopAppLocations {
        paths,
        binary: None,
    }
}

/// Claude Desktop on Windows: the Squirrel install folder, or the per-user
/// data folder of the MSIX package that claude.ai/download installs.
#[cfg(target_os = "windows")]
fn claude_desktop_locations(home: Option<&Path>) -> DesktopAppLocations {
    let paths = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join("AppData").join("Local")))
        .map(|local| {
            vec![
                local.join("AnthropicClaude"),
                local.join("Packages").join("Claude_pzs8sxrjxfjjc"),
            ]
        })
        .unwrap_or_default();
    DesktopAppLocations {
        paths,
        binary: None,
    }
}

/// Claude Desktop on Linux (beta): the `claude-desktop` package's launcher.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn claude_desktop_locations(_home: Option<&Path>) -> DesktopAppLocations {
    DesktopAppLocations {
        paths: Vec::new(),
        binary: Some("claude-desktop"),
    }
}

/// What Pi's answer about its own entry means for this meter.
fn pi_presence(status: PiStatus) -> Presence {
    match status {
        PiStatus::Ready => Presence::via(Detection::SignedIn, LoginCarrier::Pi),
        PiStatus::NotReady => Presence::via(Detection::InstalledNotSignedIn, LoginCarrier::Pi),
        PiStatus::Unknown => Presence::via(Detection::Unknown, LoginCarrier::Pi),
    }
}

fn default_claude_json_path() -> Option<PathBuf> {
    Some(antiburn_local::paths::home_dir()?.join(".claude.json"))
}

/// Asks `GET /api/oauth/usage` with the CLI's own access token, after first
/// checking the CLI's own cached reading — see the module doc's "The CLI's
/// own cache" section.
pub struct ClaudeDirectFetch {
    credentials_path: Option<PathBuf>,
    pi_auth_path: Option<PathBuf>,
    claude_json_path: Option<PathBuf>,
    /// Whether to consult the macOS Keychain before the file carriers. Always true in production; the `at()` test
    /// constructor disables it so a test exercises the file it names
    /// deterministically, rather than depending on — and risking a live
    /// network call through — whatever this machine's own Keychain happens
    /// to hold. Unused, and absent from the struct, on every other platform.
    #[cfg(target_os = "macos")]
    try_keychain: bool,
    /// Where the Claude CLI's own cached usage reading lives. `None` means
    /// no cache is consulted — the ordinary state for a test that has no
    /// reason to exercise it, not a special mode.
    config_cache_path: Option<PathBuf>,
    transport: Box<dyn AnthropicTransport>,
    cooldown: Cooldown,
    /// The delegated recovery lever for an expired Pi entry — see
    /// `pi_refresh` for the contract, and `read_carriers` for the one
    /// trigger that reaches it.
    pi_refresh: PiRefresher,
    /// The world the expired-credential touch runs in — see [`claude_touch`]
    /// and the module doc's "Delegating refresh to the CLI" section. `None`
    /// disables the touch outright; the ordinary state for a test whose
    /// scenario never reaches it, not a special mode.
    touch_env: Option<Box<dyn claude_touch::TouchEnvironment>>,
    /// The cooldown, in-flight dedup, and terminal state the touch runs
    /// behind, so one expired credential cannot spawn PTYs repeatedly.
    touch_gate: claude_touch::TouchGate,
    /// Where Claude Desktop may be installed.
    desktop_app: DesktopAppLocations,
    /// The cached native logins and their change markers — see the module
    /// doc's "Reading a carrier only when it changed" section.
    logins: NativeLogins,
    #[cfg(feature = "analytics")]
    limit_reset_diagnostic: LimitResetDiagnosticState,
}

#[cfg(feature = "analytics")]
#[derive(Default)]
struct LimitResetDiagnosticState {
    inner: Mutex<LimitResetDiagnosticInner>,
}

#[cfg(feature = "analytics")]
#[derive(Default)]
struct LimitResetDiagnosticInner {
    next_attempt: Option<Instant>,
    blocked_for_run: bool,
}

#[cfg(feature = "analytics")]
impl LimitResetDiagnosticState {
    fn observe(
        &self,
        fetch: impl FnOnce() -> LimitResetFetch,
    ) -> Option<anthropic::LimitResetDiagnostic> {
        let now = Instant::now();
        {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if inner.blocked_for_run || inner.next_attempt.is_some_and(|next| next > now) {
                return None;
            }
            inner.next_attempt = Some(now + LIMIT_RESET_DIAGNOSTIC_COOLDOWN);
        }

        let fetched = fetch();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(retry_after) = fetched.retry_after {
            match now.checked_add(retry_after) {
                Some(retry_at) if inner.next_attempt.is_none_or(|next| retry_at > next) => {
                    inner.next_attempt = Some(retry_at);
                }
                None => inner.blocked_for_run = true,
                Some(_) => {}
            }
        }
        Some(fetched.diagnostic)
    }

    fn defer(&self, delay: Duration) {
        let next = Instant::now().checked_add(delay);
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match next {
            Some(next) if inner.next_attempt.is_none_or(|current| next > current) => {
                inner.next_attempt = Some(next);
            }
            None => inner.blocked_for_run = true,
            Some(_) => {}
        }
    }
}

#[cfg(feature = "analytics")]
struct LimitResetFetch {
    diagnostic: anthropic::LimitResetDiagnostic,
    retry_after: Option<Duration>,
}

impl ClaudeDirectFetch {
    pub fn new() -> ClaudeDirectFetch {
        ClaudeDirectFetch {
            credentials_path: default_credentials_path(),
            pi_auth_path: pi_auth::default_auth_path(),
            claude_json_path: default_claude_json_path(),
            #[cfg(target_os = "macos")]
            try_keychain: true,
            config_cache_path: claude_config_cache::default_config_path(),
            transport: Box::new(LiveAnthropicTransport),
            cooldown: Cooldown::new(),
            pi_refresh: PiRefresher::new(),
            touch_env: Some(Box::new(claude_touch::CliTouchEnvironment::new(
                default_credentials_path(),
            ))),
            touch_gate: claude_touch::TouchGate::new(),
            desktop_app: claude_desktop_locations(antiburn_local::paths::home_dir().as_deref()),
            logins: NativeLogins::default(),
            #[cfg(feature = "analytics")]
            limit_reset_diagnostic: LimitResetDiagnosticState::default(),
        }
    }

    /// A source rooted at an explicit path, with the Keychain carrier
    /// disabled — see the `try_keychain` field doc for why tests need that.
    /// Reads no config cache; see `at_with_config_cache` for a test that
    /// needs one.
    #[cfg(test)]
    pub fn at(path: PathBuf) -> ClaudeDirectFetch {
        ClaudeDirectFetch {
            credentials_path: Some(path),
            pi_auth_path: None,
            claude_json_path: None,
            #[cfg(target_os = "macos")]
            try_keychain: false,
            config_cache_path: None,
            transport: Box::new(LiveAnthropicTransport),
            cooldown: Cooldown::new(),
            pi_refresh: PiRefresher::unavailable(),
            touch_env: None,
            touch_gate: claude_touch::TouchGate::new(),
            desktop_app: DesktopAppLocations::default(),
            logins: NativeLogins::default(),
            #[cfg(feature = "analytics")]
            limit_reset_diagnostic: LimitResetDiagnosticState::default(),
        }
    }

    /// A source rooted at an explicit credentials path with an injected
    /// touch environment and transport — for the delegated-refresh tests.
    /// The Keychain stays disabled for the same reason `at()` disables it.
    #[cfg(test)]
    fn at_with_touch(
        credentials_path: PathBuf,
        transport: Box<dyn AnthropicTransport>,
        touch_env: Box<dyn claude_touch::TouchEnvironment>,
    ) -> ClaudeDirectFetch {
        ClaudeDirectFetch {
            credentials_path: Some(credentials_path),
            pi_auth_path: None,
            claude_json_path: None,
            #[cfg(target_os = "macos")]
            try_keychain: false,
            config_cache_path: None,
            transport,
            cooldown: Cooldown::new(),
            pi_refresh: PiRefresher::unavailable(),
            touch_env: Some(touch_env),
            touch_gate: claude_touch::TouchGate::new(),
            desktop_app: DesktopAppLocations::default(),
            logins: NativeLogins::default(),
            #[cfg(feature = "analytics")]
            limit_reset_diagnostic: LimitResetDiagnosticState::default(),
        }
    }

    /// A source with only the Pi carrier — for the test proving Pi's
    /// read-only entry never triggers a touch, however expired it is.
    #[cfg(test)]
    fn at_pi_only(
        pi_auth_path: PathBuf,
        transport: Box<dyn AnthropicTransport>,
        touch_env: Box<dyn claude_touch::TouchEnvironment>,
    ) -> ClaudeDirectFetch {
        ClaudeDirectFetch {
            credentials_path: None,
            pi_auth_path: Some(pi_auth_path),
            claude_json_path: None,
            #[cfg(target_os = "macos")]
            try_keychain: false,
            config_cache_path: None,
            transport,
            cooldown: Cooldown::new(),
            pi_refresh: PiRefresher::unavailable(),
            touch_env: Some(touch_env),
            touch_gate: claude_touch::TouchGate::new(),
            desktop_app: DesktopAppLocations::default(),
            logins: NativeLogins::default(),
            #[cfg(feature = "analytics")]
            limit_reset_diagnostic: LimitResetDiagnosticState::default(),
        }
    }

    /// A source rooted at an explicit credentials path, also reading the
    /// CLI's own cache from an explicit path — for a test that exercises the
    /// cache through the full source rather than through `fetch_with_cache`
    /// directly.
    ///
    /// The transport is explicit so the test never reaches the network
    /// when its timing assumption fails.
    #[cfg(test)]
    fn at_with_config_cache(
        credentials_path: PathBuf,
        config_cache_path: PathBuf,
        transport: Box<dyn AnthropicTransport>,
    ) -> ClaudeDirectFetch {
        ClaudeDirectFetch {
            credentials_path: Some(credentials_path),
            pi_auth_path: None,
            claude_json_path: None,
            #[cfg(target_os = "macos")]
            try_keychain: false,
            config_cache_path: Some(config_cache_path),
            transport,
            cooldown: Cooldown::new(),
            pi_refresh: PiRefresher::unavailable(),
            touch_env: None,
            touch_gate: claude_touch::TouchGate::new(),
            desktop_app: DesktopAppLocations::default(),
            logins: NativeLogins::default(),
            #[cfg(feature = "analytics")]
            limit_reset_diagnostic: LimitResetDiagnosticState::default(),
        }
    }

    /// A source reading only the Pi carrier, with explicit transport and
    /// refresher — for tests that exercise the delegated refresh lever.
    #[cfg(test)]
    fn with_pi(
        pi_auth_path: PathBuf,
        transport: Box<dyn AnthropicTransport>,
        pi_refresh: PiRefresher,
    ) -> ClaudeDirectFetch {
        ClaudeDirectFetch {
            credentials_path: None,
            pi_auth_path: Some(pi_auth_path),
            claude_json_path: None,
            #[cfg(target_os = "macos")]
            try_keychain: false,
            config_cache_path: None,
            transport,
            cooldown: Cooldown::new(),
            pi_refresh,
            touch_env: None,
            touch_gate: claude_touch::TouchGate::new(),
            desktop_app: DesktopAppLocations::default(),
            logins: NativeLogins::default(),
            #[cfg(feature = "analytics")]
            limit_reset_diagnostic: LimitResetDiagnosticState::default(),
        }
    }

    /// The metadata probe that detection and the Desktop-only check share.
    fn presence_probe(&self) -> SystemPresenceProbe {
        SystemPresenceProbe {
            #[cfg(target_os = "macos")]
            try_keychain: self.try_keychain,
        }
    }

    /// Read Pi's carrier. An expired entry first goes through Pi's own
    /// refresh — see `pi_refresh`.
    fn read_pi_carrier(&self) -> Option<ClaudeCredentials> {
        let path = self.pi_auth_path.as_deref()?;
        let entry = pi_auth::read_entry(path, pi_auth::ANTHROPIC_KEY)
            .filter(|entry| !entry.refresh_token.is_empty())?;
        // An expired entry's one recovery lever is Pi's own SDK — see
        // `pi_refresh`. Expiry is the only trigger: a network or 5xx
        // failure later in the fetch never reaches it.
        let entry = if entry.is_live(OffsetDateTime::now_utc()) {
            entry
        } else {
            match self.pi_refresh.recover(path, pi_auth::ANTHROPIC_KEY) {
                // The rotated entry is the one retry this lever earns.
                Recovery::Fresh(fresh) => fresh,
                // Every other verdict keeps the expired entry as a
                // carrier, exactly as before the lever existed:
                // `fetch_from_carriers` reports an all-expired set as
                // an authentication failure, which is already the
                // sign-in-again state a terminal rejection asks for.
                Recovery::AlreadyValid | Recovery::SignInWithPi | Recovery::Unavailable => entry,
            }
        };
        Some(ClaudeCredentials {
            access_token: entry.access_token,
            expires_at_ms: entry.expires_at_ms,
            has_refresh_token: true,
            subscription_type: None,
            rate_limit_tier: None,
        })
    }

    /// Read all credential carriers in their documented order. A Keychain
    /// read failure stays separate so a later live carrier can suppress it.
    #[cfg(feature = "analytics")]
    fn read_carriers(&self) -> (Vec<ClaudeCredentials>, Option<FetchFailure>) {
        let (mut carriers, error) = self.read_native_carriers();
        carriers.extend(self.read_pi_carrier());
        (carriers, error)
    }

    /// Read the native carriers: the Keychain item on macOS, then the
    /// credentials file. Each one reads its secret only when its change
    /// marker changed.
    fn read_native_carriers(&self) -> (Vec<ClaudeCredentials>, Option<FetchFailure>) {
        let mut carriers = Vec::new();
        #[cfg(target_os = "macos")]
        let mut error = None;
        #[cfg(not(target_os = "macos"))]
        let error = None;
        #[cfg(target_os = "macos")]
        if self.try_keychain {
            match self.read_keychain_login(
                claude_touch::keychain_metadata,
                macos_keychain::read,
                &|delay| std::thread::sleep(delay),
            ) {
                Ok(Some(credentials)) => carriers.push(credentials),
                Ok(None) => {}
                Err(failure) => error = Some(failure),
            }
        }
        if let Some(credentials) = self
            .credentials_path
            .as_deref()
            .and_then(|path| self.read_file_login(path))
        {
            carriers.push(credentials);
        }
        (carriers, error)
    }

    /// Read the credentials file. The file is parsed again only when its
    /// content hash changed. `None` covers "no file" and "a file without a
    /// Claude login" — see the module doc for why neither is an error here.
    fn read_file_login(&self, path: &Path) -> Option<ClaudeCredentials> {
        let mut cache = lock(&self.logins.file);
        let Ok(metadata) = fs::metadata(path) else {
            *cache = None;
            return None;
        };
        if metadata.len() > MAX_CREDENTIAL_BYTES {
            *cache = None;
            return None;
        }
        let bytes = fs::read(path).ok()?;
        let marker = FileMarker {
            content_hash: claude_touch::hash(&bytes),
            modified: metadata.modified().ok(),
            len: metadata.len(),
        };
        if let Some(cached) = cache.as_mut()
            && cached.marker.content_hash == marker.content_hash
        {
            cached.marker = marker;
            return cached.login.clone();
        }
        let login = std::str::from_utf8(&bytes)
            .ok()
            .and_then(parse_credentials_json);
        *cache = Some(CachedLogin {
            marker,
            login: login.clone(),
            sign_in_required: false,
        });
        login
    }

    /// Read the Keychain login through its change marker.
    ///
    /// `metadata` reads the attributes (no prompt), `secret` reads the
    /// secret, and `sleep` waits between attribute retries. The secret is
    /// read only when the marker changed. When the attribute read fails
    /// after every retry, the cached login stays and the secret is not read.
    #[cfg(target_os = "macos")]
    fn read_keychain_login(
        &self,
        metadata: impl FnMut() -> KeychainMetadata,
        secret: impl FnOnce() -> macos_keychain::KeychainRead,
        sleep: &dyn Fn(std::time::Duration),
    ) -> Result<Option<ClaudeCredentials>, FetchFailure> {
        let attributes = match claude_touch::retry_metadata(sleep, metadata) {
            Ok(KeychainMetadata::Found(attributes)) => attributes,
            Ok(KeychainMetadata::Absent) => {
                *lock(&self.logins.keychain) = None;
                return Ok(None);
            }
            Ok(KeychainMetadata::Unreadable) | Err(_) => {
                let attempts = 1 + claude_touch::RETRY_DELAYS.len();
                ::tracing::warn!(event = "claude_keychain_metadata_failed", attempts);
                self.logins.observe("keychain_metadata_failed", None);
                return match lock(&self.logins.keychain).as_ref() {
                    Some(cached) => Ok(cached.login.clone()),
                    None => Err(keychain_unreadable()),
                };
            }
        };
        let marker = KeychainMarker::from_attributes(&attributes);
        if let Some(cached) = lock(&self.logins.keychain).as_ref()
            && cached.marker == marker
        {
            return Ok(cached.login.clone());
        }
        let login = match secret() {
            macos_keychain::KeychainRead::Found(text) => parse_credentials_json(&text),
            macos_keychain::KeychainRead::Absent => {
                // The attribute read found the item, but the secret read did
                // not. Report a Keychain failure: an empty success would
                // remove the provider from every usage surface.
                ::tracing::warn!(event = "claude_keychain_secret_missing");
                self.logins.observe("keychain_secret_missing", None);
                return Err(keychain_unreadable());
            }
            macos_keychain::KeychainRead::Unreadable => return Err(keychain_unreadable()),
        };
        if login.is_none() {
            ::tracing::debug!(
                event = "claude_keychain_read",
                outcome = "found_without_login"
            );
        }
        *lock(&self.logins.keychain) = Some(CachedLogin {
            marker,
            login: login.clone(),
            sign_in_required: false,
        });
        Ok(login)
    }

    /// The touch request for this check: the cooldown by caller, and one
    /// attempt per marker for a login that cannot refresh.
    fn touch_request(user: bool, trigger: RefreshTrigger) -> claude_touch::TouchRequest {
        claude_touch::TouchRequest {
            cooldown: if user {
                claude_touch::TOUCH_COOLDOWN
            } else {
                claude_touch::BACKGROUND_TOUCH_COOLDOWN
            },
            single_attempt: trigger == RefreshTrigger::CannotRefresh,
        }
    }

    /// Refresh a live login that expires soon. Returns the native carriers
    /// read again when the CLI wrote a new login, and `None` otherwise: the
    /// check then uses the login it has, which is still live.
    fn refresh_before_expiry(
        &self,
        user: bool,
    ) -> Option<(Vec<ClaudeCredentials>, Option<FetchFailure>)> {
        let env = self.touch_env.as_deref()?;
        let trigger = RefreshTrigger::PreExpiry;
        let outcome =
            claude_touch::touch(env, &self.touch_gate, Self::touch_request(user, trigger));
        let label = match &outcome {
            claude_touch::TouchOutcome::Settled(_) => "refreshed",
            claude_touch::TouchOutcome::CliMissing => "cli_missing",
            claude_touch::TouchOutcome::Terminal => "sign_in_required",
            claude_touch::TouchOutcome::Skipped => "gated",
            claude_touch::TouchOutcome::NotRefreshed => "unchanged",
        };
        log_refresh(trigger, user, label);
        self.logins.observe(label, Some(trigger.name()));
        match outcome {
            claude_touch::TouchOutcome::Settled(_) => Some(self.read_native_carriers()),
            _ => None,
        }
    }

    /// The delegated-refresh path the module doc's "Delegating refresh to
    /// the CLI" section describes: touch, verify by metadata, read the
    /// secret once, retry the usage call once.
    ///
    /// Every credential-shaped failure returns
    /// [`ProviderUsageError::Authentication`] with a detail that says which
    /// one: no CLI to refresh the login, a refresh that can still recover,
    /// or a login that only a new sign-in can fix. A retry that failed for
    /// another reason reports its own error.
    fn touch_then_retry(
        &self,
        now: OffsetDateTime,
        user: bool,
        trigger: RefreshTrigger,
    ) -> Result<Option<ProviderUsageSnapshot>, FetchFailure> {
        let result = self.touch_and_read(now, user, trigger);
        let label = match &result {
            Ok(_) => "refreshed",
            Err(failure) => match failure.detail {
                Some(SourceErrorDetail::CliMissing) => "cli_missing",
                Some(SourceErrorDetail::SignInRequired) => "sign_in_required",
                Some(SourceErrorDetail::RefreshPending) => "pending",
                _ => "retry_failed",
            },
        };
        log_refresh(trigger, user, label);
        match label {
            "refreshed" => self.logins.set_sign_in_required(false),
            "cli_missing" | "sign_in_required" => self.logins.set_sign_in_required(true),
            _ => {}
        }
        result
    }

    fn touch_and_read(
        &self,
        now: OffsetDateTime,
        user: bool,
        trigger: RefreshTrigger,
    ) -> Result<Option<ProviderUsageSnapshot>, FetchFailure> {
        let observe = |label| self.logins.observe(label, Some(trigger.name()));
        let Some(env) = self.touch_env.as_deref() else {
            ::tracing::debug!(
                event = "claude_refresh_outcome",
                outcome = "environment_unavailable"
            );
            return Err(auth_failure(SourceErrorDetail::RefreshPending));
        };
        let settled =
            match claude_touch::touch(env, &self.touch_gate, Self::touch_request(user, trigger)) {
                claude_touch::TouchOutcome::Settled(settled) => settled,
                claude_touch::TouchOutcome::CliMissing => {
                    observe("cli_missing");
                    return Err(auth_failure(SourceErrorDetail::CliMissing));
                }
                claude_touch::TouchOutcome::Terminal => {
                    observe("sign_in_required");
                    return Err(auth_failure(SourceErrorDetail::SignInRequired));
                }
                claude_touch::TouchOutcome::Skipped => {
                    observe("gated");
                    return Err(auth_failure(SourceErrorDetail::RefreshPending));
                }
                // A login that cannot refresh had its one attempt; the gate
                // now blocks the same marker. Another login can still recover.
                claude_touch::TouchOutcome::NotRefreshed
                    if trigger == RefreshTrigger::CannotRefresh =>
                {
                    observe("sign_in_required");
                    return Err(auth_failure(SourceErrorDetail::SignInRequired));
                }
                claude_touch::TouchOutcome::NotRefreshed => {
                    observe("unchanged");
                    return Err(auth_failure(SourceErrorDetail::RefreshPending));
                }
            };
        // The change has settled: the one permitted secret read, through the
        // normal carriers. The changed marker makes this read the secret.
        let (native_carriers, read_error) = self.read_native_carriers();
        let Some(credentials) = native_carriers
            .into_iter()
            .find(|credentials| credentials.is_live(now))
        else {
            if let Some(failure) = read_error {
                return Err(failure);
            }
            // The CLI wrote its carrier and the credential is still dead: an
            // `invalid_grant`-style terminal failure. The fix is `claude
            // /login`, so the touch stays blocked until the material changes
            // again — see [`claude_touch::TouchGate::mark_terminal`].
            self.touch_gate.mark_terminal(settled);
            observe("sign_in_required");
            return Err(auth_failure(SourceErrorDetail::SignInRequired));
        };
        self.touch_gate.clear_terminal();
        match fetch_live(
            self.transport.as_ref(),
            &credentials,
            self.claude_json_path.as_deref(),
            now,
        ) {
            Ok(snapshot) => {
                observe("refreshed");
                Ok(Some(snapshot))
            }
            Err(ProviderUsageError::Authentication) => {
                // A freshly refreshed token the endpoint still rejects is
                // the same terminal state, observed one hop later.
                self.touch_gate.mark_terminal(settled);
                observe("sign_in_required");
                Err(auth_failure(SourceErrorDetail::SignInRequired))
            }
            Err(error) => Err(error.into()),
        }
    }
}

/// Log the result of one CLI refresh for one trigger. Fixed values only.
fn log_refresh(trigger: RefreshTrigger, user: bool, result: &'static str) {
    ::tracing::debug!(
        event = "claude_refresh_result",
        trigger = trigger.name(),
        origin = if user { "user" } else { "background" },
        result
    );
}

/// A Keychain read that could not say whether a login exists.
#[cfg(target_os = "macos")]
fn keychain_unreadable() -> FetchFailure {
    FetchFailure {
        error: ProviderUsageError::Unavailable,
        detail: Some(SourceErrorDetail::KeychainUnreadable),
        last_known: None,
    }
}

/// An authentication failure qualified by which sign-in state it is.
fn auth_failure(detail: SourceErrorDetail) -> FetchFailure {
    FetchFailure {
        error: ProviderUsageError::Authentication,
        detail: Some(detail),
        last_known: None,
    }
}

impl Default for ClaudeDirectFetch {
    fn default() -> ClaudeDirectFetch {
        ClaudeDirectFetch::new()
    }
}

impl LiveUsageSource for ClaudeDirectFetch {
    fn id(&self) -> &'static str {
        SOURCE_ID
    }

    fn provider(&self) -> &'static str {
        crate::provider_usage::providers::ANTHROPIC
    }

    /// This source makes a request of its own, on the reader's own account —
    /// exactly the traffic the online opt-in exists to gate.
    fn requires_online_opt_in(&self) -> bool {
        true
    }

    fn detect(&self, online: bool) -> Presence {
        let probe = self.presence_probe();
        let presence = detect_presence(
            &probe,
            &self.logins,
            self.credentials_path.as_deref(),
            self.pi_auth_path.as_deref(),
            || match (online, self.pi_auth_path.as_deref()) {
                (true, Some(path)) => self.pi_refresh.status(path, pi_auth::ANTHROPIC_KEY),
                _ => PiStatus::Unknown,
            },
        );
        with_claude_desktop(presence, &probe, &self.desktop_app)
    }

    fn fetch(&self, max_age: std::time::Duration) -> SourceOutcome {
        let now = OffsetDateTime::now_utc();
        let user = user_initiated(max_age);
        // The credential read, and the config-cache read, both sit inside
        // the cooldown gate on purpose: on macOS the former spawns a
        // `security` subprocess, and a poll that the cooldown is going to
        // skip anyway should not pay for either.
        let outcome = self.cooldown.poll(now, max_age, || {
            let (mut native_carriers, mut carrier_error) = self.read_native_carriers();
            if expires_soon(&native_carriers, now)
                && let Some((refreshed, error)) = self.refresh_before_expiry(user)
            {
                native_carriers = refreshed;
                carrier_error = error;
            }
            let mut carriers = native_carriers.clone();
            carriers.extend(self.read_pi_carrier());
            // The touch below requires a *native* carrier: Pi's read-only
            // entry alone never triggers one — see the module doc's
            // "Delegating refresh to the CLI" section.
            let native_present = !native_carriers.is_empty();
            let trigger = RefreshTrigger::after_rejection(&native_carriers);
            let native = native_carriers
                .into_iter()
                .find(|credentials| credentials.is_live(now));
            let cached = self
                .config_cache_path
                .as_deref()
                .and_then(claude_config_cache::read_cached_usage);
            if let Some(native) = native.as_ref()
                && let Some(cached) = cached.as_ref()
                && cache_covers(now, cached.observed_at, max_age)
            {
                log_cache_reading_used(cached, now, "fresh");
                return Ok(Some(snapshot_from_cache(cached.clone(), native)));
            }
            let previous_tokens: Vec<_> = carriers
                .iter()
                .map(|carrier| carrier.access_token.clone())
                .collect();
            let mut fetched = fetch_from_carriers(
                self.transport.as_ref(),
                carriers,
                self.claude_json_path.as_deref(),
                now,
            );
            // Check the native carriers once more before CLI recovery.
            // Another Claude process can replace a token before its recorded
            // expiry. The change marker keeps this from reading a secret
            // that did not change.
            if matches!(fetched, Err(ProviderUsageError::Authentication)) && native_present {
                let (current, read_error) = self.read_native_carriers();
                let changed: Vec<_> = current
                    .into_iter()
                    .filter(|credential| {
                        credential.is_live(now)
                            && !previous_tokens.contains(&credential.access_token)
                    })
                    .collect();
                let outcome = if !changed.is_empty() {
                    "changed"
                } else if read_error.is_some() {
                    "unreadable"
                } else {
                    "unchanged"
                };
                ::tracing::debug!(
                    event = "claude_credential_reread",
                    outcome,
                    keychain_read_failed = read_error.is_some()
                );
                carrier_error = read_error;
                if !changed.is_empty() {
                    fetched = fetch_from_carriers(
                        self.transport.as_ref(),
                        changed,
                        self.claude_json_path.as_deref(),
                        now,
                    );
                }
            }
            // Only the expired/rejected credential state — never a network
            // or 5xx failure — reaches for the CLI. Background checks do
            // too, so the reading recovers while no surface is open.
            let fetched = match fetched {
                Err(ProviderUsageError::Authentication) if native_present => {
                    if let Some(failure) = carrier_error.take() {
                        Err(failure)
                    } else {
                        self.touch_then_retry(now, user, trigger)
                    }
                }
                Ok(Some(snapshot)) => {
                    if native.is_some() {
                        self.logins.set_sign_in_required(false);
                    }
                    Ok(Some(snapshot))
                }
                other => other.map_err(FetchFailure::from),
            };
            let fetched = match with_carrier_error(fetched, carrier_error) {
                Ok(None) => desktop_only_failure(&self.presence_probe(), &self.desktop_app)
                    .map_or(Ok(None), Err),
                other => other,
            };
            fetched.map_err(|mut failure| {
                failure.last_known = native.as_ref().and_then(|native| {
                    cached
                        .filter(|cached| now - cached.observed_at <= cooldown::MAX_AGE)
                        .map(|cached| {
                            log_cache_reading_used(&cached, now, "seed");
                            Box::new(snapshot_from_cache(cached, native))
                        })
                });
                failure
            })
        });
        #[cfg(feature = "analytics")]
        if let Some(delay) = self.cooldown.rate_limit_retry_after(max_age) {
            self.limit_reset_diagnostic.defer(delay);
        }
        outcome
    }

    /// Lets a Desktop-only reader see their plan above the note that limits
    /// are not available.
    fn local_plan(&self) -> Option<crate::dto::LiveProviderPlan> {
        self.claude_json_path
            .as_deref()
            .and_then(read_claude_json_plan)
    }

    #[cfg(feature = "analytics")]
    fn take_login_observations(&self) -> Vec<crate::provider_usage::live::LoginObservation> {
        std::mem::take(&mut *lock(&self.logins.observations))
    }

    #[cfg(feature = "analytics")]
    fn analytics_diagnostic(&self) -> Option<crate::provider_usage::live::AnalyticsDiagnostic> {
        self.limit_reset_diagnostic
            .observe(|| {
                let now = OffsetDateTime::now_utc();
                let (carriers, carrier_error) = self.read_carriers();
                let live = carriers.iter().find(|credentials| credentials.is_live(now));
                match live {
                    Some(credentials) => self.transport.limit_reset(&credentials.access_token),
                    None if !carriers.is_empty() => LimitResetFetch {
                        diagnostic: anthropic::empty_limit_reset_diagnostic(
                            "credential_expired",
                            "not_requested",
                        ),
                        retry_after: None,
                    },
                    None if carrier_error.is_some() => LimitResetFetch {
                        diagnostic: anthropic::empty_limit_reset_diagnostic(
                            "credential_unavailable",
                            "not_requested",
                        ),
                        retry_after: None,
                    },
                    None => LimitResetFetch {
                        diagnostic: anthropic::empty_limit_reset_diagnostic(
                            "credential_absent",
                            "not_requested",
                        ),
                        retry_after: None,
                    },
                }
            })
            .map(crate::provider_usage::live::AnalyticsDiagnostic::ClaudeLimitReset)
    }
}

/// The network calls the live path needs, as a trait so a test can supply
/// them without a socket — the same seam `codex_fetch`'s `CodexTransport`
/// gives that source.
trait AnthropicTransport: Send + Sync {
    fn usage(&self, access_token: &str) -> Result<String, ProviderUsageError>;
    #[cfg(feature = "analytics")]
    fn limit_reset(&self, _access_token: &str) -> LimitResetFetch {
        LimitResetFetch {
            diagnostic: anthropic::empty_limit_reset_diagnostic("unavailable", "not_received"),
            retry_after: None,
        }
    }
    /// The profile body, or `None` when enrichment fails.
    fn profile(&self, access_token: &str) -> Option<String>;
}

struct LiveAnthropicTransport;

impl AnthropicTransport for LiveAnthropicTransport {
    fn usage(&self, access_token: &str) -> Result<String, ProviderUsageError> {
        let response = claude_request(USAGE_ENDPOINT, access_token)
            .send()
            .map_err(|_| ProviderUsageError::Unavailable)?;
        if let Some(error) = http::status_error(response.status()) {
            return Err(error);
        }
        http::read_capped_body(response)
    }

    #[cfg(feature = "analytics")]
    fn limit_reset(&self, access_token: &str) -> LimitResetFetch {
        let response = match claude_request(LIMIT_RESET_ENDPOINT, access_token).send() {
            Ok(response) => response,
            Err(_) => {
                return LimitResetFetch {
                    diagnostic: anthropic::empty_limit_reset_diagnostic(
                        "unavailable",
                        "not_received",
                    ),
                    retry_after: None,
                };
            }
        };
        let retry_after = retry_after(&response);
        if let Some(error) = http::status_error(response.status()) {
            return LimitResetFetch {
                diagnostic: anthropic::empty_limit_reset_diagnostic(
                    error.category(),
                    "not_received",
                ),
                retry_after,
            };
        }
        let diagnostic = match http::read_capped_body(response) {
            Ok(body) => anthropic::parse_limit_reset_diagnostic(&body),
            Err(error) => anthropic::empty_limit_reset_diagnostic(error.category(), "unreadable"),
        };
        LimitResetFetch {
            diagnostic,
            retry_after: None,
        }
    }

    fn profile(&self, access_token: &str) -> Option<String> {
        let response = claude_request(PROFILE_ENDPOINT, access_token).send().ok()?;
        if http::status_error(response.status()).is_some() {
            return None;
        }
        http::read_capped_body(response).ok()
    }
}

#[cfg(feature = "analytics")]
fn retry_after(response: &reqwest::blocking::Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

fn claude_request(endpoint: &str, access_token: &str) -> reqwest::blocking::RequestBuilder {
    http::client()
        .get(endpoint)
        .bearer_auth(access_token)
        .header(
            reqwest::header::USER_AGENT,
            CLAUDE_CODE_COMPATIBILITY_USER_AGENT,
        )
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("anthropic-beta", "oauth-2025-04-20")
}

/// The two-tier decision the module doc's "The CLI's own cache" section
/// describes. A free function over [`AnthropicTransport`] rather than a
/// method, so a test calls it with a fake transport and nothing else this
/// source owns — mirrors `codex_fetch::fetch_direct`.
#[cfg(test)]
fn fetch_with_cache(
    transport: &dyn AnthropicTransport,
    credentials: &ClaudeCredentials,
    cached: Option<CachedUsage>,
    max_age: std::time::Duration,
    now: OffsetDateTime,
) -> Result<Option<ProviderUsageSnapshot>, FetchFailure> {
    fetch_with_cache_at(transport, credentials, cached, None, max_age, now)
}

#[cfg(test)]
fn fetch_with_cache_at(
    transport: &dyn AnthropicTransport,
    credentials: &ClaudeCredentials,
    cached: Option<CachedUsage>,
    claude_json_path: Option<&Path>,
    max_age: std::time::Duration,
    now: OffsetDateTime,
) -> Result<Option<ProviderUsageSnapshot>, FetchFailure> {
    if let Some(cached) = cached.clone()
        && cache_covers(now, cached.observed_at, max_age)
    {
        log_cache_reading_used(&cached, now, "fresh");
        return Ok(Some(snapshot_from_cache(cached, credentials)));
    }

    match fetch_live(transport, credentials, claude_json_path, now) {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(error) => {
            let last_known = cached
                .filter(|cached| now - cached.observed_at <= cooldown::MAX_AGE)
                .map(|cached| {
                    log_cache_reading_used(&cached, now, "seed");
                    Box::new(snapshot_from_cache(cached, credentials))
                });
            Err(FetchFailure {
                error,
                detail: None,
                last_known,
            })
        }
    }
}

/// Try live carriers in order. Expired carriers are absent, while an
/// all-expired set reports authentication because re-sign-in is actionable.
fn fetch_from_carriers(
    transport: &dyn AnthropicTransport,
    carriers: Vec<ClaudeCredentials>,
    claude_json_path: Option<&Path>,
    now: OffsetDateTime,
) -> Result<Option<ProviderUsageSnapshot>, ProviderUsageError> {
    if carriers.is_empty() {
        return Ok(None);
    }
    let mut live = false;
    let mut error = None;
    for credentials in carriers
        .iter()
        .filter(|credentials| credentials.is_live(now))
    {
        live = true;
        match fetch_live(transport, credentials, claude_json_path, now) {
            Ok(snapshot) => return Ok(Some(snapshot)),
            Err(next) => {
                error = Some(match error {
                    Some(current) => preferred_error(current, next),
                    None => next,
                });
            }
        }
    }
    if !live {
        return Err(ProviderUsageError::Authentication);
    }
    Err(error.unwrap_or(ProviderUsageError::Unavailable))
}

fn with_carrier_error(
    fetched: Result<Option<ProviderUsageSnapshot>, FetchFailure>,
    carrier_error: Option<FetchFailure>,
) -> Result<Option<ProviderUsageSnapshot>, FetchFailure> {
    match fetched {
        Ok(Some(snapshot)) => Ok(Some(snapshot)),
        Ok(None) => carrier_error.map_or(Ok(None), Err),
        Err(failure) => Err(match carrier_error {
            Some(carrier) if preferred_error(carrier.error, failure.error) == carrier.error => {
                carrier
            }
            _ => failure,
        }),
    }
}

fn preferred_error(current: ProviderUsageError, next: ProviderUsageError) -> ProviderUsageError {
    if matches!(current, ProviderUsageError::Unavailable)
        && !matches!(next, ProviderUsageError::Unavailable)
    {
        next
    } else {
        current
    }
}

/// Whether `observed_at` is no older than `max_age`, converting the
/// caller's `std::time::Duration` into `time`'s own type for the
/// comparison. A `max_age` too large to convert cannot be satisfied, so the
/// cache does not pre-empt the network in that case either.
fn cache_covers(
    now: OffsetDateTime,
    observed_at: OffsetDateTime,
    max_age: std::time::Duration,
) -> bool {
    time::Duration::try_from(max_age)
        .map(|max_age| now - observed_at <= max_age)
        .unwrap_or(false)
}

/// The `live_cache_reading_used` tracing event, at the two points the
/// cached reading is used — see the module doc's "The CLI's own cache"
/// section. `debug`, not `warn`: reading a cache is the ordinary path, not a
/// problem.
fn log_cache_reading_used(cached: &CachedUsage, now: OffsetDateTime, reason: &'static str) {
    ::tracing::debug!(
        event = "live_cache_reading_used",
        provider = crate::provider_usage::providers::ANTHROPIC,
        age_secs = (now - cached.observed_at).whole_seconds(),
        reason
    );
}

/// Build a snapshot from the CLI's own cached reading. Used both when the
/// cache pre-empts the network and when it seeds a failure — see the module
/// doc's "The CLI's own cache" section.
fn snapshot_from_cache(
    cached: CachedUsage,
    credentials: &ClaudeCredentials,
) -> ProviderUsageSnapshot {
    ProviderUsageSnapshot {
        refusal_kind: None,
        provider: crate::provider_usage::providers::ANTHROPIC,
        account: Some(cached.account.clone()),
        account_uuid: Some(cached.account),
        account_email: None,
        plan: credentials.subscription_type.clone(),
        plan_tier: credentials.rate_limit_tier.clone(),
        observed_at: cached.observed_at,
        source: UsageSource {
            id: SOURCE_ID,
            label: "Read from the Claude CLI's own cache".into(),
            confidence: Confidence::High,
            freshness: Freshness::Fresh,
        },
        windows: cached.usage.windows,
        supplemental: cached.usage.supplemental,
        reset_credits: None,
    }
}

fn fetch_live(
    transport: &dyn AnthropicTransport,
    credentials: &ClaudeCredentials,
    claude_json_path: Option<&Path>,
    now: OffsetDateTime,
) -> Result<ProviderUsageSnapshot, ProviderUsageError> {
    let body = transport.usage(&credentials.access_token)?;
    let usage = anthropic::parse_usage(&body)?;
    let identity = resolve_identity(transport, &credentials.access_token, claude_json_path);

    Ok(ProviderUsageSnapshot {
        refusal_kind: None,
        provider: crate::provider_usage::providers::ANTHROPIC,
        account: identity.uuid.clone(),
        account_uuid: identity.uuid,
        account_email: identity.email,
        plan: identity
            .plan
            .or_else(|| credentials.subscription_type.clone()),
        plan_tier: identity
            .tier
            .or_else(|| credentials.rate_limit_tier.clone()),
        observed_at: now,
        source: UsageSource {
            id: SOURCE_ID,
            label: "Asked Claude directly".into(),
            confidence: Confidence::High,
            // Recomputed on every read by `Cooldown::poll`; a snapshot this
            // function returns is always describing the instant it was built.
            freshness: Freshness::Fresh,
        },
        windows: usage.windows,
        supplemental: usage.supplemental,
        reset_credits: None,
    })
}

#[derive(Default)]
struct ClaudeIdentity {
    uuid: Option<String>,
    email: Option<String>,
    plan: Option<String>,
    tier: Option<String>,
}

fn non_empty_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 512)
        .map(str::to_owned)
}

fn parse_profile(body: &str) -> Option<ClaudeIdentity> {
    let value: Value = serde_json::from_str(body).ok()?;
    let account = value.get("account")?;
    let plan = if account
        .get("has_claude_max")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        Some("max".to_owned())
    } else if account
        .get("has_claude_pro")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        Some("pro".to_owned())
    } else {
        None
    };
    Some(ClaudeIdentity {
        uuid: non_empty_str(account.get("uuid")),
        email: non_empty_str(account.get("email")),
        plan,
        tier: non_empty_str(
            value
                .get("organization")
                .and_then(|organization| organization.get("rate_limit_tier")),
        ),
    })
}

/// The `oauthAccount` object from `~/.claude.json`, if the file is small
/// enough to read and parses.
fn read_claude_json_account(path: &Path) -> Option<Value> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > MAX_CLAUDE_JSON_BYTES {
        return None;
    }
    let contents = fs::read_to_string(path).ok()?;
    let mut value: Value = serde_json::from_str(&contents).ok()?;
    Some(value.get_mut("oauthAccount")?.take())
}

fn read_claude_json_identity(path: &Path) -> Option<ClaudeIdentity> {
    let account = read_claude_json_account(path)?;
    Some(ClaudeIdentity {
        uuid: non_empty_str(account.get("accountUuid")),
        email: non_empty_str(account.get("emailAddress")),
        plan: None,
        tier: non_empty_str(account.get("userRateLimitTier")),
    })
}

/// The plan Claude's own tools last wrote to `~/.claude.json`.
///
/// Claude Desktop writes `oauthAccount` there when the reader signs in, even
/// with no Claude Code login. Only `organizationType` (for example
/// `claude_max`) and `organizationRateLimitTier` (for example
/// `default_claude_max_20x`) are read. The file holds no token.
fn read_claude_json_plan(path: &Path) -> Option<crate::dto::LiveProviderPlan> {
    plan_from_oauth_account(&read_claude_json_account(path)?)
}

/// `claude_max` names the plan `max`, matching the profile endpoint's names.
fn plan_from_oauth_account(account: &Value) -> Option<crate::dto::LiveProviderPlan> {
    let kind = non_empty_str(account.get("organizationType"))?;
    let name = kind.strip_prefix("claude_").unwrap_or(&kind).to_owned();
    Some(crate::dto::LiveProviderPlan {
        name,
        tier: non_empty_str(account.get("organizationRateLimitTier")),
    })
}

fn resolve_identity(
    transport: &dyn AnthropicTransport,
    access_token: &str,
    claude_json_path: Option<&Path>,
) -> ClaudeIdentity {
    transport
        .profile(access_token)
        .and_then(|body| parse_profile(&body))
        .or_else(|| claude_json_path.and_then(read_claude_json_identity))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::super::presence::RecordingPresence;
    use std::io;

    const PRESENCE_CREDENTIALS: &str = "/fixture/.claude/.credentials.json";
    const PRESENCE_PI: &str = "/fixture/.pi/agent/auth.json";
    const PRESENCE_CONFIG: &str = "/fixture/.claude";

    fn presence(probe: &impl PresenceProbe) -> Presence {
        detect_presence(
            probe,
            &NoCachedLogins,
            Some(Path::new(PRESENCE_CREDENTIALS)),
            Some(Path::new(PRESENCE_PI)),
            || PiStatus::Unknown,
        )
    }

    fn detected(probe: &impl PresenceProbe) -> Detection {
        presence(probe).detection
    }

    #[test]
    fn detection_without_carriers_or_tool_uses_only_presence_calls() {
        let probe = RecordingPresence::default();
        assert_eq!(detected(&probe), Detection::NotInstalled);
        let expected = vec![
            format!("path_exists:{PRESENCE_CREDENTIALS}"),
            #[cfg(target_os = "macos")]
            "keychain_metadata".into(),
            format!("path_exists:{PRESENCE_PI}"),
            format!("path_exists:{PRESENCE_CONFIG}"),
            "binary_present".into(),
        ];
        assert_eq!(*probe.calls.borrow(), expected);
    }

    #[test]
    fn detection_accepts_the_config_directory_without_a_login() {
        let mut probe = RecordingPresence::default();
        probe.paths.insert(PRESENCE_CONFIG.into(), Ok(true));
        assert_eq!(detected(&probe), Detection::InstalledNotSignedIn);
        assert!(
            !probe
                .calls
                .borrow()
                .iter()
                .any(|call| call == "binary_present")
        );
    }

    #[test]
    fn detection_accepts_the_binary_without_a_login() {
        let probe = RecordingPresence {
            binary: true,
            ..Default::default()
        };
        assert_eq!(detected(&probe), Detection::InstalledNotSignedIn);
    }

    #[test]
    fn detection_short_circuits_on_claude_credentials_metadata() {
        let mut probe = RecordingPresence::default();
        probe.paths.insert(PRESENCE_CREDENTIALS.into(), Ok(true));
        assert_eq!(detected(&probe), Detection::SignedIn);
        assert_eq!(
            *probe.calls.borrow(),
            [format!("path_exists:{PRESENCE_CREDENTIALS}")]
        );
    }

    #[test]
    fn detection_does_not_parse_a_malformed_credentials_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".credentials.json");
        fs::write(&path, "not valid JSON").unwrap();
        assert_eq!(
            ClaudeDirectFetch::at(path).detect(true),
            Presence::via(Detection::SignedIn, LoginCarrier::ClaudeCredentialsFile)
        );
    }

    #[test]
    fn detection_of_the_shared_pi_file_is_inconclusive_but_names_pi() {
        let mut probe = RecordingPresence::default();
        probe.paths.insert(PRESENCE_PI.into(), Ok(true));
        assert_eq!(
            presence(&probe),
            Presence::via(Detection::Unknown, LoginCarrier::Pi)
        );
        // Detection ends at the Pi file: the config directory and the
        // binary would add nothing to an inconclusive answer.
        assert_eq!(
            probe.calls.borrow().last().unwrap(),
            &format!("path_exists:{PRESENCE_PI}")
        );
    }

    #[test]
    fn pi_answers_for_its_own_file_when_asked() {
        for (status, expected) in [
            (PiStatus::Ready, Detection::SignedIn),
            (PiStatus::NotReady, Detection::InstalledNotSignedIn),
            (PiStatus::Unknown, Detection::Unknown),
        ] {
            let mut probe = RecordingPresence::default();
            probe.paths.insert(PRESENCE_PI.into(), Ok(true));
            assert_eq!(
                detect_presence(
                    &probe,
                    &NoCachedLogins,
                    Some(Path::new(PRESENCE_CREDENTIALS)),
                    Some(Path::new(PRESENCE_PI)),
                    || status,
                ),
                Presence::via(expected, LoginCarrier::Pi)
            );
        }
    }

    #[test]
    fn pi_is_never_asked_offline_or_without_its_file() {
        let asked = std::cell::Cell::new(false);
        let mut probe = RecordingPresence::default();
        probe.paths.insert(PRESENCE_PI.into(), Ok(false));
        detect_presence(
            &probe,
            &NoCachedLogins,
            Some(Path::new(PRESENCE_CREDENTIALS)),
            Some(Path::new(PRESENCE_PI)),
            || {
                asked.set(true);
                PiStatus::Ready
            },
        );
        assert!(!asked.get());

        let dir = tempfile::tempdir().unwrap();
        let pi = dir.path().join("auth.json");
        fs::write(&pi, r#"{"anthropic":{}}"#).unwrap();
        struct PanicRunner;
        impl super::super::pi_refresh::RefreshRunner for PanicRunner {
            fn run(&self, _: &str) -> super::super::pi_refresh::RunOutcome {
                panic!("offline detection must not spawn pi")
            }
            fn check(&self, _: &str) -> super::super::pi_refresh::RunOutcome {
                panic!("offline detection must not spawn pi")
            }
        }
        let mut source = ClaudeDirectFetch::with_pi(
            pi,
            Box::new(UnreachableTransport),
            PiRefresher::with_runner(Box::new(PanicRunner)),
        );
        source.credentials_path = Some(dir.path().join(".claude/.credentials.json"));
        assert_eq!(
            source.detect(false),
            Presence::via(Detection::Unknown, LoginCarrier::Pi)
        );
    }

    #[test]
    fn online_detection_lets_pi_answer_for_its_own_entry() {
        struct Answer(super::super::pi_refresh::RunOutcome);
        impl super::super::pi_refresh::RefreshRunner for Answer {
            fn run(&self, _: &str) -> super::super::pi_refresh::RunOutcome {
                panic!("detection must use the no-refresh check")
            }
            fn check(&self, provider: &str) -> super::super::pi_refresh::RunOutcome {
                assert_eq!(provider, pi_auth::ANTHROPIC_KEY);
                self.0
            }
        }
        use super::super::pi_refresh::RunOutcome;
        for (store, outcome, expected) in [
            (
                r#"{"anthropic":{}}"#,
                RunOutcome::Completed,
                Detection::SignedIn,
            ),
            (
                r#"{"anthropic":{}}"#,
                RunOutcome::Rejected,
                Detection::InstalledNotSignedIn,
            ),
            (
                r#"{"anthropic":{}}"#,
                RunOutcome::Unavailable,
                Detection::Unknown,
            ),
            // No anthropic key: answered from the key names alone, no spawn.
            (
                r#"{"openai-codex":{}}"#,
                RunOutcome::Completed,
                Detection::InstalledNotSignedIn,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let pi = dir.path().join("auth.json");
            fs::write(&pi, store).unwrap();
            let mut source = ClaudeDirectFetch::with_pi(
                pi,
                Box::new(UnreachableTransport),
                PiRefresher::with_runner(Box::new(Answer(outcome))),
            );
            source.credentials_path = Some(dir.path().join(".claude/.credentials.json"));
            assert_eq!(
                source.detect(true),
                Presence::via(expected, LoginCarrier::Pi)
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_keychain_login_outranks_the_pi_file() {
        let mut probe = RecordingPresence {
            keychain: Some(KeychainMetadata::Found(b"synthetic attributes".to_vec())),
            ..Default::default()
        };
        probe.paths.insert(PRESENCE_PI.into(), Ok(true));
        assert_eq!(
            presence(&probe),
            Presence::via(Detection::SignedIn, LoginCarrier::ClaudeKeychain)
        );
        assert!(
            !probe
                .calls
                .borrow()
                .iter()
                .any(|c| c.ends_with(PRESENCE_PI))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detection_accepts_keychain_attributes_without_reading_the_secret() {
        let probe = RecordingPresence {
            keychain: Some(KeychainMetadata::Found(b"synthetic attributes".to_vec())),
            ..Default::default()
        };
        assert_eq!(
            presence(&probe),
            Presence::via(Detection::SignedIn, LoginCarrier::ClaudeKeychain)
        );
        assert_eq!(probe.calls.borrow().last().unwrap(), "keychain_metadata");
        assert_eq!(probe.calls.borrow().len(), 2);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detection_keeps_unreadable_keychain_metadata_unknown() {
        let probe = RecordingPresence {
            keychain: Some(KeychainMetadata::Unreadable),
            binary: true,
            ..Default::default()
        };
        assert_eq!(detected(&probe), Detection::Unknown);
        assert_eq!(probe.calls.borrow().last().unwrap(), "keychain_metadata");
        assert_eq!(probe.calls.borrow().len(), 2);
    }

    #[test]
    fn detection_keeps_metadata_errors_unknown() {
        for path in [PRESENCE_CREDENTIALS, PRESENCE_PI, PRESENCE_CONFIG] {
            for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::Other] {
                let mut probe = RecordingPresence::default();
                probe.paths.insert(path.into(), Err(kind));
                assert_eq!(detected(&probe), Detection::Unknown);
                assert_eq!(
                    probe.calls.borrow().last().unwrap(),
                    &format!("path_exists:{path}")
                );
            }
        }
    }

    #[test]
    fn detection_treats_not_found_metadata_errors_as_absence() {
        let mut probe = RecordingPresence::default();
        for path in [PRESENCE_CREDENTIALS, PRESENCE_PI, PRESENCE_CONFIG] {
            probe
                .paths
                .insert(path.into(), Err(io::ErrorKind::NotFound));
        }
        assert_eq!(detected(&probe), Detection::NotInstalled);
    }

    #[test]
    fn claude_desktop_is_noted_whatever_login_was_found() {
        const APP: &str = "/fixture/Applications/Claude.app";
        let mut probe = RecordingPresence::default();
        probe.paths.insert(APP.into(), Ok(true));
        let app = DesktopAppLocations {
            paths: vec![PathBuf::from(APP)],
            binary: None,
        };

        let not_installed =
            with_claude_desktop(Presence::new(Detection::NotInstalled), &probe, &app);
        assert_eq!(not_installed.detection, Detection::NotInstalled);
        assert_eq!(not_installed.desktop_app, Some(DesktopApp::ClaudeDesktop));

        // A login does not hide the app. The agent lists still name it.
        let signed_in = with_claude_desktop(
            Presence::via(Detection::SignedIn, LoginCarrier::ClaudeKeychain),
            &probe,
            &app,
        );
        assert_eq!(signed_in.detection, Detection::SignedIn);
        assert_eq!(signed_in.desktop_app, Some(DesktopApp::ClaudeDesktop));

        let pi_only = with_claude_desktop(
            Presence::via(Detection::SignedIn, LoginCarrier::Pi),
            &probe,
            &app,
        );
        assert_eq!(pi_only.carrier, Some(LoginCarrier::Pi));
        assert_eq!(pi_only.desktop_app, Some(DesktopApp::ClaudeDesktop));

        // No app on disk: nothing is noted.
        let absent = with_claude_desktop(
            Presence::new(Detection::InstalledNotSignedIn),
            &RecordingPresence::default(),
            &app,
        );
        assert_eq!(absent.desktop_app, None);
    }

    #[test]
    fn the_local_plan_comes_from_claude_json_without_reading_tokens() {
        let account = serde_json::json!({
            "organizationType": "claude_max",
            "organizationRateLimitTier": "default_claude_max_20x",
        });
        assert_eq!(
            plan_from_oauth_account(&account),
            Some(crate::dto::LiveProviderPlan {
                name: "max".into(),
                tier: Some("default_claude_max_20x".into()),
            })
        );
        let pro = serde_json::json!({ "organizationType": "claude_pro" });
        assert_eq!(
            plan_from_oauth_account(&pro).map(|plan| plan.name),
            Some("pro".into())
        );
        assert_eq!(plan_from_oauth_account(&serde_json::json!({})), None);

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".claude.json");
        fs::write(&path, format!(r#"{{"oauthAccount": {account}}}"#)).expect("write");
        assert_eq!(
            read_claude_json_plan(&path).map(|plan| plan.name),
            Some("max".into())
        );
        assert_eq!(read_claude_json_plan(&dir.path().join("absent.json")), None);
    }

    #[test]
    fn a_desktop_only_install_fails_with_its_own_detail() {
        const APP: &str = "/fixture/Applications/Claude.app";
        let app = DesktopAppLocations {
            paths: vec![PathBuf::from(APP)],
            binary: None,
        };
        let mut desktop_only = RecordingPresence::default();
        desktop_only.paths.insert(APP.into(), Ok(true));

        let failure = desktop_only_failure(&desktop_only, &app).expect("desktop only");
        assert_eq!(failure.error, ProviderUsageError::Authentication);
        assert_eq!(failure.detail, Some(SourceErrorDetail::DesktopOnly));
        assert!(failure.last_known.is_none());

        // Claude Code on PATH but signed out keeps the ordinary sign-in note.
        let mut with_cli = RecordingPresence {
            binary: true,
            ..Default::default()
        };
        with_cli.paths.insert(APP.into(), Ok(true));
        assert!(desktop_only_failure(&with_cli, &app).is_none());

        // No Claude Desktop: no Claude app at all, so nothing to report.
        assert!(desktop_only_failure(&RecordingPresence::default(), &app).is_none());
    }

    #[test]
    fn claude_desktop_on_linux_is_found_by_its_launcher() {
        let probe = RecordingPresence {
            binary: true,
            ..Default::default()
        };
        let app = DesktopAppLocations {
            paths: Vec::new(),
            binary: Some("claude-desktop"),
        };
        let presence = with_claude_desktop(Presence::new(Detection::NotInstalled), &probe, &app);
        assert_eq!(presence.desktop_app, Some(DesktopApp::ClaudeDesktop));
        assert!(
            probe
                .calls
                .borrow()
                .iter()
                .any(|call| call == "binary_present")
        );
    }

    #[test]
    fn detection_keeps_an_unresolved_credentials_path_unknown() {
        let probe = RecordingPresence::default();
        assert_eq!(
            detect_presence(&probe, &NoCachedLogins, None, None, || PiStatus::Ready),
            Presence::UNKNOWN
        );
        assert!(probe.calls.borrow().is_empty());
    }

    const NOW: i64 = 1_800_000_000;

    /// A background-caller-shaped `max_age` for tests that only exercise
    /// whether a reading is found, not how the cooldown's freshness budget
    /// behaves — `cooldown.rs`'s own suite owns that.
    const TEST_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(600);

    /// A popover-shaped `max_age` — well under a minute, matching
    /// `cooldown.rs`'s own `SHORT_MAX_AGE` — for the `fetch_with_cache`
    /// tests below, which care whether the cache is fresh enough to
    /// pre-empt the network, not the cooldown's own gating.
    const SHORT_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(50);

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(NOW).unwrap()
    }

    fn valid_credentials() -> ClaudeCredentials {
        ClaudeCredentials {
            access_token: "synthetic-token".into(),
            expires_at_ms: (NOW + 3_600) * 1_000,
            has_refresh_token: true,
            subscription_type: Some("max".into()),
            rate_limit_tier: Some("default_claude_max_5x".into()),
        }
    }

    #[test]
    fn claude_requests_use_the_recognised_cli_identity() {
        let request = claude_request(USAGE_ENDPOINT, "synthetic-token")
            .build()
            .expect("request builds");
        assert_eq!(request.url().as_str(), USAGE_ENDPOINT);
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some(CLAUDE_CODE_COMPATIBILITY_USER_AGENT)
        );
        assert_eq!(
            request
                .headers()
                .get("anthropic-beta")
                .and_then(|value| value.to_str().ok()),
            Some("oauth-2025-04-20")
        );
    }

    #[cfg(feature = "analytics")]
    #[test]
    fn the_limit_reset_request_uses_the_separate_probe_query() {
        let request = claude_request(LIMIT_RESET_ENDPOINT, "synthetic-token")
            .build()
            .expect("request builds");
        assert_eq!(
            request.url().as_str(),
            "https://api.anthropic.com/api/oauth/usage?at_wall=1&skip_spend=1"
        );
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::USER_AGENT)
                .and_then(|value| value.to_str().ok()),
            Some(CLAUDE_CODE_COMPATIBILITY_USER_AGENT)
        );
    }

    #[cfg(feature = "analytics")]
    #[test]
    fn the_limit_reset_diagnostic_is_bounded_and_honors_retry_after() {
        let state = LimitResetDiagnosticState::default();
        let first = state.observe(|| LimitResetFetch {
            diagnostic: anthropic::empty_limit_reset_diagnostic("rateLimited", "not_received"),
            retry_after: Some(Duration::from_secs(20 * 60)),
        });
        assert!(first.is_some());

        let second = state.observe(|| panic!("the cooldown must skip this request"));
        assert_eq!(second, None);
        let inner = state.inner.lock().unwrap();
        let delay = inner
            .next_attempt
            .expect("retry deadline")
            .checked_duration_since(Instant::now())
            .expect("future deadline");
        assert!(delay > Duration::from_secs(19 * 60));
    }

    fn cached_usage(observed_at: OffsetDateTime) -> CachedUsage {
        CachedUsage {
            observed_at,
            account: "cached-account-uuid".into(),
            usage: anthropic::AnthropicUsage::default(),
        }
    }

    /// A transport that panics if called — for a test asserting the cache
    /// pre-empted the network entirely.
    struct UnreachableTransport;

    impl AnthropicTransport for UnreachableTransport {
        fn usage(&self, _access_token: &str) -> Result<String, ProviderUsageError> {
            unreachable!("the cache should have pre-empted the network")
        }
        fn profile(&self, _access_token: &str) -> Option<String> {
            unreachable!("the cache should have pre-empted the network")
        }
    }

    const LIVE_USAGE_BODY: &str = r#"{"five_hour": {"utilization": 40}}"#;

    /// A transport whose `usage` call returns a fixed result, for tests that
    /// only care whether the live call was reached and what it answered.
    struct FakeTransport {
        usage_result: Result<String, ProviderUsageError>,
    }

    impl AnthropicTransport for FakeTransport {
        fn usage(&self, _access_token: &str) -> Result<String, ProviderUsageError> {
            self.usage_result.clone()
        }
        fn profile(&self, _access_token: &str) -> Option<String> {
            None
        }
    }

    #[cfg(feature = "analytics")]
    struct RateLimitedTransport {
        limit_reset_calls: Arc<AtomicUsize>,
    }

    #[cfg(feature = "analytics")]
    impl AnthropicTransport for RateLimitedTransport {
        fn usage(&self, _access_token: &str) -> Result<String, ProviderUsageError> {
            Err(ProviderUsageError::RateLimited)
        }

        fn limit_reset(&self, _access_token: &str) -> LimitResetFetch {
            self.limit_reset_calls.fetch_add(1, Ordering::SeqCst);
            LimitResetFetch {
                diagnostic: anthropic::empty_limit_reset_diagnostic("success", "null"),
                retry_after: None,
            }
        }

        fn profile(&self, _access_token: &str) -> Option<String> {
            None
        }
    }

    #[cfg(feature = "analytics")]
    #[test]
    fn an_ordinary_rate_limit_suppresses_the_immediate_reset_probe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        let expires_at_ms = (OffsetDateTime::now_utc().unix_timestamp() + 3_600) * 1_000;
        fs::write(&path, credentials_file(expires_at_ms, "max")).expect("write");
        let calls = Arc::new(AtomicUsize::new(0));
        let mut source = ClaudeDirectFetch::at(path);
        source.transport = Box::new(RateLimitedTransport {
            limit_reset_calls: Arc::clone(&calls),
        });

        let outcome = source.fetch(SHORT_MAX_AGE);
        assert_eq!(outcome.error, Some(ProviderUsageError::RateLimited));
        assert_eq!(source.analytics_diagnostic(), None);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_fresh_cache_pre_empts_the_network() {
        let cached = cached_usage(now() - time::Duration::seconds(10));
        let outcome = fetch_with_cache(
            &UnreachableTransport,
            &valid_credentials(),
            Some(cached.clone()),
            SHORT_MAX_AGE,
            now(),
        )
        .expect("no live call, so no failure")
        .expect("a snapshot from the cache");

        assert_eq!(outcome.source.label, "Read from the Claude CLI's own cache");
        assert_eq!(outcome.observed_at, cached.observed_at);
    }

    #[test]
    fn a_cache_older_than_max_age_falls_through_to_a_live_call_that_succeeds() {
        let cached = cached_usage(now() - time::Duration::seconds(100));
        let transport = FakeTransport {
            usage_result: Ok(LIVE_USAGE_BODY.to_string()),
        };
        let outcome = fetch_with_cache(
            &transport,
            &valid_credentials(),
            Some(cached),
            SHORT_MAX_AGE,
            now(),
        )
        .expect("the live call succeeded")
        .expect("a live snapshot");

        assert_eq!(outcome.source.label, "Asked Claude directly");
        assert_eq!(outcome.observed_at, now());
    }

    #[test]
    fn a_cache_older_than_max_age_but_within_the_hour_seeds_a_failed_live_call() {
        let cached = cached_usage(now() - time::Duration::minutes(30));
        let transport = FakeTransport {
            usage_result: Err(ProviderUsageError::RateLimited),
        };
        let failure = fetch_with_cache(
            &transport,
            &valid_credentials(),
            Some(cached.clone()),
            SHORT_MAX_AGE,
            now(),
        )
        .expect_err("the live call failed");

        assert_eq!(failure.error, ProviderUsageError::RateLimited);
        let last_known = failure.last_known.expect("the cache seeds the failure");
        assert_eq!(last_known.observed_at, cached.observed_at);
    }

    #[test]
    fn a_cache_older_than_the_hour_budget_does_not_seed_a_failed_live_call() {
        let cached = cached_usage(now() - time::Duration::minutes(90));
        let transport = FakeTransport {
            usage_result: Err(ProviderUsageError::Unavailable),
        };
        let failure = fetch_with_cache(
            &transport,
            &valid_credentials(),
            Some(cached),
            SHORT_MAX_AGE,
            now(),
        )
        .expect_err("the live call failed");

        assert!(failure.last_known.is_none());
    }

    #[test]
    fn no_cache_never_seeds_a_failed_live_call() {
        let transport = FakeTransport {
            usage_result: Err(ProviderUsageError::Unavailable),
        };
        let failure =
            fetch_with_cache(&transport, &valid_credentials(), None, SHORT_MAX_AGE, now())
                .expect_err("the live call failed");

        assert!(failure.last_known.is_none());
    }

    #[test]
    fn fetch_prefers_a_fresh_cache_over_a_live_call() {
        let dir = tempfile::tempdir().expect("tempdir");
        let credentials_path = dir.path().join(".credentials.json");
        let real_now = OffsetDateTime::now_utc();
        fs::write(
            &credentials_path,
            credentials_file((real_now.unix_timestamp() + 3_600) * 1_000, "max"),
        )
        .expect("write");

        let config_cache_path = dir.path().join(".claude.json");
        let fetched_at_ms = (real_now.unix_timestamp() - 10) * 1_000;
        fs::write(
            &config_cache_path,
            format!(
                r#"{{
                  "oauthAccount": {{"accountUuid": "cached-account-uuid"}},
                  "cachedUsageUtilization": {{
                    "fetchedAtMs": {fetched_at_ms},
                    "accountUuid": "cached-account-uuid",
                    "utilization": {{"five_hour": {{"utilization": 25}}}}
                  }}
                }}"#
            ),
        )
        .expect("write");

        let outcome = ClaudeDirectFetch::at_with_config_cache(
            credentials_path,
            config_cache_path,
            Box::new(UnreachableTransport),
        )
        .fetch(TEST_MAX_AGE);

        assert_eq!(outcome.error, None);
        assert_eq!(outcome.snapshots.len(), 1);
        assert_eq!(
            outcome.snapshots[0].source.label,
            "Read from the Claude CLI's own cache"
        );
        assert_eq!(
            outcome.snapshots[0].account.as_deref(),
            Some("cached-account-uuid")
        );
    }

    #[test]
    fn profile_identity_uses_only_the_account_uuid() {
        assert_eq!(
            parse_profile(
                r#"{"account":{"uuid":"account-uuid","email":"private@example.test"},"organization":{"uuid":"organization-uuid"}}"#
            )
            .and_then(|identity| identity.uuid),
            Some("account-uuid".to_owned())
        );
        assert_eq!(
            parse_profile(r#"{"account":{"email":"private@example.test"}}"#)
                .and_then(|identity| identity.uuid),
            None
        );
        assert_eq!(
            parse_profile(&format!(
                r#"{{"account":{{"uuid":"{}"}}}}"#,
                "a".repeat(513)
            ))
            .and_then(|identity| identity.uuid),
            None
        );
    }

    /// Read a credentials file through a new source, so no cache applies.
    fn read_credentials_file(path: &Path) -> Option<ClaudeCredentials> {
        ClaudeDirectFetch::at(path.to_path_buf()).read_file_login(path)
    }

    fn credentials_file(expires_at_ms: i64, subscription_type: &str) -> String {
        format!(
            r#"{{"claudeAiOauth": {{"accessToken": "synthetic-token",
              "refreshToken": "synthetic-refresh", "expiresAt": {expires_at_ms},
              "subscriptionType": "{subscription_type}",
              "rateLimitTier": "default_claude_max_5x"}}}}"#
        )
    }

    #[test]
    fn a_missing_credentials_file_is_absent_not_an_error() {
        let source = ClaudeDirectFetch::at(PathBuf::from("/nonexistent/.credentials.json"));
        let outcome = source.fetch(TEST_MAX_AGE);
        assert!(outcome.snapshots.is_empty());
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn an_unparseable_credentials_file_reads_as_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(&path, "not json at all").expect("write");
        let outcome = ClaudeDirectFetch::at(path).fetch(TEST_MAX_AGE);
        assert!(outcome.snapshots.is_empty());
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn credentials_missing_the_oauth_object_read_as_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(&path, r#"{"somethingElse": true}"#).expect("write");
        let outcome = ClaudeDirectFetch::at(path).fetch(TEST_MAX_AGE);
        assert!(outcome.snapshots.is_empty());
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn a_later_live_carrier_success_mutes_an_earlier_failure() {
        struct LaterOnly(AtomicUsize);
        impl AnthropicTransport for LaterOnly {
            fn usage(&self, token: &str) -> Result<String, ProviderUsageError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                if token == "later" {
                    Ok(LIVE_USAGE_BODY.to_owned())
                } else {
                    Err(ProviderUsageError::Unavailable)
                }
            }
            fn profile(&self, _: &str) -> Option<String> {
                None
            }
        }
        let carriers = vec![
            ClaudeCredentials {
                access_token: "earlier".into(),
                expires_at_ms: (NOW + 3_600) * 1_000,
                has_refresh_token: true,
                subscription_type: None,
                rate_limit_tier: None,
            },
            ClaudeCredentials {
                access_token: "later".into(),
                expires_at_ms: (NOW + 3_600) * 1_000,
                has_refresh_token: true,
                subscription_type: None,
                rate_limit_tier: None,
            },
        ];
        let transport = LaterOnly(AtomicUsize::new(0));
        let result = fetch_from_carriers(&transport, carriers, None, now())
            .expect("later carrier succeeds")
            .expect("snapshot");
        assert_eq!(transport.0.load(Ordering::SeqCst), 2);
        assert_eq!(result.source.label, "Asked Claude directly");
    }

    #[test]
    fn only_positive_expiry_expired_carriers_report_authentication() {
        let expired = ClaudeCredentials {
            access_token: "expired-token".into(),
            expires_at_ms: (NOW - 3_600) * 1_000,
            has_refresh_token: true,
            subscription_type: None,
            rate_limit_tier: None,
        };
        assert_eq!(
            fetch_from_carriers(&UnreachableTransport, vec![expired], None, now()),
            Err(ProviderUsageError::Authentication)
        );
        assert_eq!(
            fetch_from_carriers(&UnreachableTransport, Vec::new(), None, now()),
            Ok(None)
        );
    }

    #[test]
    fn profile_supplies_identity_plan_and_tier_without_putting_identity_in_label() {
        let identity = parse_profile(
            r#"{"account":{"uuid":"synthetic-uuid","email":"reader@example.test","has_claude_max":true,"has_claude_pro":false},"organization":{"rate_limit_tier":"synthetic-tier"}}"#,
        )
        .expect("profile");
        assert_eq!(identity.uuid.as_deref(), Some("synthetic-uuid"));
        assert_eq!(identity.email.as_deref(), Some("reader@example.test"));
        assert_eq!(identity.plan.as_deref(), Some("max"));
        assert_eq!(identity.tier.as_deref(), Some("synthetic-tier"));
    }

    #[test]
    fn failed_profile_uses_claude_json_identity_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".claude.json");
        fs::write(
            &path,
            r#"{"oauthAccount":{"accountUuid":"synthetic-uuid","emailAddress":"reader@example.test","organizationName":"Synthetic Org","userRateLimitTier":"synthetic-tier"}}"#,
        )
        .expect("write");
        let identity = read_claude_json_identity(&path).expect("identity");
        assert_eq!(identity.uuid.as_deref(), Some("synthetic-uuid"));
        assert_eq!(identity.email.as_deref(), Some("reader@example.test"));
        assert_eq!(identity.plan, None);
        assert_eq!(identity.tier.as_deref(), Some("synthetic-tier"));
    }

    #[test]
    fn native_rereads_do_not_recover_an_expired_pi_entry() {
        let dir = tempfile::tempdir().unwrap();
        let pi_path = dir.path().join("pi-auth.json");
        fs::write(&pi_path, r#"{"anthropic":{"type":"oauth","access":"stale-access","refresh":"pi-refresh","expires":1}}"#).unwrap();
        struct PanicRunner;
        impl super::super::pi_refresh::RefreshRunner for PanicRunner {
            fn run(&self, _: &str) -> super::super::pi_refresh::RunOutcome {
                panic!("native recovery must not refresh Pi")
            }
            fn check(&self, _: &str) -> super::super::pi_refresh::RunOutcome {
                panic!("native recovery must not check Pi")
            }
        }
        let mut source = ClaudeDirectFetch::with_pi(
            pi_path,
            Box::new(UnreachableTransport),
            PiRefresher::with_runner(Box::new(PanicRunner)),
        );
        let native_path = dir.path().join(".credentials.json");
        fs::write(
            &native_path,
            credentials_file((real_now_secs() + 3_600) * 1_000, "max"),
        )
        .unwrap();
        source.credentials_path = Some(native_path);
        let (native, error) = source.read_native_carriers();
        assert!(error.is_none());
        assert_eq!(native.len(), 1);
        assert!(native[0].is_live(OffsetDateTime::now_utc()));
    }

    #[test]
    fn an_expired_pi_entry_recovers_through_the_delegated_refresh_and_retries_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pi_path = dir.path().join("pi-auth.json");
        fs::write(
            &pi_path,
            r#"{"anthropic":{"type":"oauth","access":"stale-access","refresh":"pi-refresh","expires":1}}"#,
        )
        .expect("write");

        /// Rewrites the Pi store the way Pi's own locked write would, then
        /// answers a clean completion.
        struct RotatingRunner(PathBuf);
        impl super::super::pi_refresh::RefreshRunner for RotatingRunner {
            fn check(&self, _: &str) -> super::super::pi_refresh::RunOutcome {
                super::super::pi_refresh::RunOutcome::Unavailable
            }
            fn run(&self, _: &str) -> super::super::pi_refresh::RunOutcome {
                fs::write(
                    &self.0,
                    r#"{"anthropic":{"type":"oauth","access":"rotated-access","refresh":"rotated-refresh","expires":9223372036854775807}}"#,
                )
                .expect("rewrite pi store");
                super::super::pi_refresh::RunOutcome::Completed
            }
        }

        struct RotatedOnly(Arc<AtomicUsize>);
        impl AnthropicTransport for RotatedOnly {
            fn usage(&self, token: &str) -> Result<String, ProviderUsageError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                if token == "rotated-access" {
                    Ok(LIVE_USAGE_BODY.to_owned())
                } else {
                    Err(ProviderUsageError::Authentication)
                }
            }
            fn profile(&self, _: &str) -> Option<String> {
                None
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let source = ClaudeDirectFetch::with_pi(
            pi_path.clone(),
            Box::new(RotatedOnly(Arc::clone(&calls))),
            PiRefresher::with_runner(Box::new(RotatingRunner(pi_path))),
        );

        let outcome = source.fetch(TEST_MAX_AGE);
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.snapshots.len(), 1);
        // Exactly one usage call: the rotated entry is the single retry.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_live_pi_entry_never_triggers_the_delegated_refresh() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pi_path = dir.path().join("pi-auth.json");
        fs::write(
            &pi_path,
            r#"{"anthropic":{"type":"oauth","access":"pi-access","refresh":"pi-refresh","expires":9223372036854775807}}"#,
        )
        .expect("write");
        struct PanicRunner;
        impl super::super::pi_refresh::RefreshRunner for PanicRunner {
            fn run(&self, _: &str) -> super::super::pi_refresh::RunOutcome {
                panic!("a live Pi entry must never spawn the refresh")
            }
            fn check(&self, _: &str) -> super::super::pi_refresh::RunOutcome {
                panic!("a live Pi entry must never spawn the refresh")
            }
        }
        let source = ClaudeDirectFetch::with_pi(
            pi_path,
            Box::new(FakeTransport {
                usage_result: Ok(LIVE_USAGE_BODY.to_string()),
            }),
            PiRefresher::with_runner(Box::new(PanicRunner)),
        );

        let outcome = source.fetch(TEST_MAX_AGE);
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.snapshots.len(), 1);
    }

    #[test]
    fn the_source_declares_itself_online_so_the_gate_can_find_it() {
        assert!(ClaudeDirectFetch::new().requires_online_opt_in());
    }

    #[test]
    fn a_credentials_file_larger_than_the_cap_is_not_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        let padding = "x".repeat((MAX_CREDENTIAL_BYTES + 1) as usize);
        fs::write(&path, format!(r#"{{"pad": "{padding}"}}"#)).expect("write");
        assert!(read_credentials_file(&path).is_none());
    }

    #[test]
    fn a_well_formed_file_yields_the_fields_this_source_uses_and_nothing_else() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(&path, credentials_file(NOW * 1_000, "max")).expect("write");
        let credentials = read_credentials_file(&path).expect("parses");
        assert_eq!(credentials.access_token, "synthetic-token");
        assert_eq!(credentials.expires_at_ms, NOW * 1_000);
        assert_eq!(credentials.subscription_type.as_deref(), Some("max"));
        assert_eq!(
            credentials.rate_limit_tier.as_deref(),
            Some("default_claude_max_5x")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_unreadable_keychain_reports_detail_until_a_carrier_succeeds() {
        let cooldown = Cooldown::new();
        let failure = || keychain_unreadable();
        let outcome = cooldown.poll(now(), TEST_MAX_AGE, || {
            with_carrier_error(Ok(None), Some(failure()))
        });
        assert_eq!(outcome.error, Some(ProviderUsageError::Unavailable));
        assert_eq!(outcome.detail, Some(SourceErrorDetail::KeychainUnreadable));
        let cached = cooldown.poll(now(), TEST_MAX_AGE, || {
            panic!("cooldown must skip the read")
        });
        assert_eq!(cached.detail, outcome.detail);

        cooldown.open_for_test();
        let outcome = cooldown.poll(now(), TEST_MAX_AGE, || {
            let fetched = fetch_from_carriers(
                &FakeTransport {
                    usage_result: Ok(LIVE_USAGE_BODY.into()),
                },
                vec![valid_credentials()],
                None,
                now(),
            );
            with_carrier_error(fetched.map_err(FetchFailure::from), Some(failure()))
        });
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.detail, None);
        assert_eq!(outcome.snapshots.len(), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn keychain_detail_stays_with_the_selected_error_category() {
        for (error, expected_detail) in [
            (
                ProviderUsageError::Unavailable,
                Some(SourceErrorDetail::KeychainUnreadable),
            ),
            (ProviderUsageError::Authentication, None),
            (ProviderUsageError::RateLimited, None),
        ] {
            let failure =
                with_carrier_error(Err(error.into()), Some(keychain_unreadable())).unwrap_err();
            assert_eq!(failure.error, error);
            assert_eq!(failure.detail, expected_detail);
        }
    }

    /// A popover-shaped `max_age` that reads as user-initiated — see
    /// [`USER_INITIATED_MAX_AGE`].
    const USER_MAX_AGE: std::time::Duration = SHORT_MAX_AGE;

    /// A touch world over the real credentials file: `spawn` plays the CLI,
    /// writing `writes_on_spawn` to the file — which also moves the
    /// fingerprint, since the fingerprint is the file's contents.
    struct FileTouchEnv {
        path: PathBuf,
        binary_present: bool,
        writes_on_spawn: Option<String>,
        spawns: Arc<AtomicUsize>,
    }

    struct NoopChild;

    impl claude_touch::TouchChild for NoopChild {
        fn kill(&mut self) {}
    }

    impl claude_touch::TouchEnvironment for FileTouchEnv {
        fn binary_present(&self) -> bool {
            self.binary_present
        }
        fn fingerprint(&self) -> Option<claude_touch::Fingerprint> {
            let contents = fs::read(&self.path).unwrap_or_default();
            Some(claude_touch::Fingerprint(
                String::from_utf8_lossy(&contents).into_owned(),
            ))
        }
        fn spawn(&self) -> Option<Box<dyn claude_touch::TouchChild>> {
            self.spawns.fetch_add(1, Ordering::SeqCst);
            if let Some(body) = &self.writes_on_spawn {
                fs::write(&self.path, body).expect("write refreshed credentials");
            }
            Some(Box::new(NoopChild))
        }
        fn sleep(&self, _interval: std::time::Duration) {}
    }

    /// A touch world that fails the test if the source so much as looks at
    /// it — for the paths that must never reach the touch.
    struct UntouchableEnv;

    impl claude_touch::TouchEnvironment for UntouchableEnv {
        fn binary_present(&self) -> bool {
            panic!("this fetch must never reach the touch");
        }
        fn fingerprint(&self) -> Option<claude_touch::Fingerprint> {
            panic!("this fetch must never reach the touch");
        }
        fn spawn(&self) -> Option<Box<dyn claude_touch::TouchChild>> {
            panic!("this fetch must never reach the touch");
        }
        fn sleep(&self, _interval: std::time::Duration) {}
    }

    /// A transport that counts `usage` calls, for the retry-once assertions.
    struct CountingTransport {
        usage_calls: Arc<AtomicUsize>,
        usage_result: Result<String, ProviderUsageError>,
    }

    impl AnthropicTransport for CountingTransport {
        fn usage(&self, _access_token: &str) -> Result<String, ProviderUsageError> {
            self.usage_calls.fetch_add(1, Ordering::SeqCst);
            self.usage_result.clone()
        }
        fn profile(&self, _access_token: &str) -> Option<String> {
            None
        }
    }

    /// The seconds-relative-to-now the touch tests stamp credentials with.
    fn real_now_secs() -> i64 {
        OffsetDateTime::now_utc().unix_timestamp()
    }

    struct RotatingTransport {
        path: PathBuf,
        calls: Arc<AtomicUsize>,
        replacement: Option<String>,
        result: Result<String, ProviderUsageError>,
    }

    impl AnthropicTransport for RotatingTransport {
        fn usage(&self, token: &str) -> Result<String, ProviderUsageError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                if let Some(replacement) = &self.replacement {
                    fs::write(&self.path, replacement).unwrap();
                }
                return Err(ProviderUsageError::Authentication);
            }
            assert_eq!(token, "rotated-access");
            self.result.clone()
        }

        fn profile(&self, _token: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn a_replaced_credential_is_retried_before_cli_recovery() {
        for result in [
            Ok(LIVE_USAGE_BODY.to_string()),
            Err(ProviderUsageError::RateLimited),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(".credentials.json");
            let body = credentials_file((real_now_secs() + 3_600) * 1_000, "max");
            fs::write(&path, &body).unwrap();
            let mut replacement: serde_json::Value = serde_json::from_str(&body).unwrap();
            replacement["claudeAiOauth"]["accessToken"] = "rotated-access".into();
            let calls = Arc::new(AtomicUsize::new(0));
            let expected_error = result.as_ref().err().copied();
            let source = ClaudeDirectFetch::at_with_touch(
                path.clone(),
                Box::new(RotatingTransport {
                    path,
                    calls: Arc::clone(&calls),
                    replacement: Some(replacement.to_string()),
                    result,
                }),
                Box::new(UntouchableEnv),
            );
            let outcome = source.fetch(USER_MAX_AGE);
            assert_eq!(outcome.error, expected_error);
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }
    }

    struct TemporarilyRejectedTransport(AtomicUsize);

    impl AnthropicTransport for TemporarilyRejectedTransport {
        fn usage(&self, _token: &str) -> Result<String, ProviderUsageError> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(ProviderUsageError::Authentication)
            } else {
                Ok(LIVE_USAGE_BODY.to_string())
            }
        }
        fn profile(&self, _token: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn an_unchanged_credential_can_recover_on_a_later_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".credentials.json");
        fs::write(
            &path,
            credentials_file((real_now_secs() + 3_600) * 1_000, "max"),
        )
        .unwrap();
        let spawns = Arc::new(AtomicUsize::new(0));
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(TemporarilyRejectedTransport(AtomicUsize::new(0))),
            Box::new(FileTouchEnv {
                path,
                binary_present: true,
                writes_on_spawn: None,
                spawns: Arc::clone(&spawns),
            }),
        );
        assert_eq!(
            source.fetch(USER_MAX_AGE).detail,
            Some(SourceErrorDetail::RefreshPending)
        );
        source.cooldown.open_for_test();
        let recovered = source.fetch(USER_MAX_AGE);
        assert_eq!(recovered.error, None);
        assert_eq!(recovered.snapshots.len(), 1);
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_unchanged_rejected_credential_is_not_retried() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".credentials.json");
        fs::write(
            &path,
            credentials_file((real_now_secs() + 3_600) * 1_000, "max"),
        )
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let spawns = Arc::new(AtomicUsize::new(0));
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(RotatingTransport {
                path: path.clone(),
                calls: Arc::clone(&calls),
                replacement: None,
                result: Ok(LIVE_USAGE_BODY.to_string()),
            }),
            Box::new(FileTouchEnv {
                path,
                binary_present: true,
                writes_on_spawn: None,
                spawns: Arc::clone(&spawns),
            }),
        );
        let outcome = source.fetch(USER_MAX_AGE);
        assert_eq!(outcome.detail, Some(SourceErrorDetail::RefreshPending));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_expired_credential_touches_the_cli_verifies_and_retries_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(
            &path,
            credentials_file((real_now_secs() - 3_600) * 1_000, "max"),
        )
        .expect("write expired credentials");
        let spawns = Arc::new(AtomicUsize::new(0));
        let usage_calls = Arc::new(AtomicUsize::new(0));
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(CountingTransport {
                usage_calls: Arc::clone(&usage_calls),
                usage_result: Ok(LIVE_USAGE_BODY.to_string()),
            }),
            Box::new(FileTouchEnv {
                path: path.clone(),
                binary_present: true,
                writes_on_spawn: Some(credentials_file((real_now_secs() + 3_600) * 1_000, "max")),
                spawns: Arc::clone(&spawns),
            }),
        );

        let outcome = source.fetch(USER_MAX_AGE);

        assert_eq!(outcome.error, None);
        assert_eq!(outcome.snapshots.len(), 1);
        assert_eq!(outcome.snapshots[0].source.label, "Asked Claude directly");
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        // The retry is exactly one usage call — the secret was re-read once
        // and used once.
        assert_eq!(usage_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_touch_that_never_settles_is_a_pending_authentication_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(
            &path,
            credentials_file((real_now_secs() - 3_600) * 1_000, "max"),
        )
        .expect("write expired credentials");
        let spawns = Arc::new(AtomicUsize::new(0));
        let usage_calls = Arc::new(AtomicUsize::new(0));
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(CountingTransport {
                usage_calls: Arc::clone(&usage_calls),
                usage_result: Ok(LIVE_USAGE_BODY.to_string()),
            }),
            // The CLI runs but writes nothing: the fingerprint never moves.
            Box::new(FileTouchEnv {
                path,
                binary_present: true,
                writes_on_spawn: None,
                spawns: Arc::clone(&spawns),
            }),
        );

        let outcome = source.fetch(USER_MAX_AGE);

        assert_eq!(outcome.error, Some(ProviderUsageError::Authentication));
        assert_eq!(outcome.detail, Some(SourceErrorDetail::RefreshPending));
        assert!(outcome.snapshots.is_empty());
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        assert_eq!(usage_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_settled_refresh_that_is_still_dead_requires_a_sign_in() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(
            &path,
            credentials_file((real_now_secs() - 3_600) * 1_000, "max"),
        )
        .expect("write expired credentials");
        let spawns = Arc::new(AtomicUsize::new(0));
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(UnreachableTransport),
            // The CLI rewrites the carrier, but the token it leaves is still
            // expired: the `invalid_grant` shape.
            Box::new(FileTouchEnv {
                path,
                binary_present: true,
                writes_on_spawn: Some(credentials_file((real_now_secs() - 60) * 1_000, "max")),
                spawns: Arc::clone(&spawns),
            }),
        );

        let outcome = source.fetch(USER_MAX_AGE);
        assert_eq!(outcome.error, Some(ProviderUsageError::Authentication));
        assert_eq!(outcome.detail, Some(SourceErrorDetail::SignInRequired));
        assert_eq!(spawns.load(Ordering::SeqCst), 1);

        // The carrier still matches the terminal fingerprint: the next
        // user-initiated poll reports the same state without spawning again.
        source.cooldown.open_for_test();
        source.touch_gate.open_cooldown_for_test();
        let outcome = source.fetch(USER_MAX_AGE);
        assert_eq!(outcome.detail, Some(SourceErrorDetail::SignInRequired));
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_absent_claude_binary_is_a_cli_missing_authentication_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(
            &path,
            credentials_file((real_now_secs() - 3_600) * 1_000, "max"),
        )
        .expect("write expired credentials");
        let spawns = Arc::new(AtomicUsize::new(0));
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(UnreachableTransport),
            Box::new(FileTouchEnv {
                path,
                binary_present: false,
                writes_on_spawn: Some("never written".to_owned()),
                spawns: Arc::clone(&spawns),
            }),
        );

        let outcome = source.fetch(USER_MAX_AGE);

        assert_eq!(outcome.error, Some(ProviderUsageError::Authentication));
        assert_eq!(outcome.detail, Some(SourceErrorDetail::CliMissing));
        assert_eq!(spawns.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_pi_carrier_alone_never_triggers_a_touch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("auth.json");
        // An expired Pi entry: a carrier, but not a native one.
        fs::write(
            &path,
            r#"{"anthropic": {"type": "oauth", "access": "synthetic-access",
              "refresh": "synthetic-refresh", "expires": 1000}}"#,
        )
        .expect("write pi auth");
        let source = ClaudeDirectFetch::at_pi_only(
            path,
            Box::new(UnreachableTransport),
            Box::new(UntouchableEnv),
        );

        let outcome = source.fetch(USER_MAX_AGE);

        assert_eq!(outcome.error, Some(ProviderUsageError::Authentication));
        // No native carrier: nothing here can refresh it, and the plain
        // sign-in-again copy is the true one.
        assert_eq!(outcome.detail, None);
    }

    #[test]
    fn a_refresh_that_settles_dead_is_terminal_until_the_material_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(
            &path,
            credentials_file((real_now_secs() - 3_600) * 1_000, "max"),
        )
        .expect("write expired credentials");
        let spawns = Arc::new(AtomicUsize::new(0));
        let usage_calls = Arc::new(AtomicUsize::new(0));
        // The CLI writes the carrier — the fingerprint settles — but the
        // credential it leaves behind is still expired: `invalid_grant`.
        let dead_body = format!(
            r#"{{"claudeAiOauth": {{"accessToken": "rotated-but-dead",
              "expiresAt": {}}}}}"#,
            (real_now_secs() - 60) * 1_000
        );
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(CountingTransport {
                usage_calls: Arc::clone(&usage_calls),
                usage_result: Ok(LIVE_USAGE_BODY.to_string()),
            }),
            Box::new(FileTouchEnv {
                path: path.clone(),
                binary_present: true,
                writes_on_spawn: Some(dead_body),
                spawns: Arc::clone(&spawns),
            }),
        );

        let outcome = source.fetch(USER_MAX_AGE);
        assert_eq!(outcome.error, Some(ProviderUsageError::Authentication));
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        assert_eq!(usage_calls.load(Ordering::SeqCst), 0);

        // Both cooldowns opened, the material unchanged: the terminal gate
        // alone must keep the next user-initiated fetch from spawning again.
        source.cooldown.open_for_test();
        source.touch_gate.open_cooldown_for_test();
        let outcome = source.fetch(USER_MAX_AGE);
        assert_eq!(outcome.error, Some(ProviderUsageError::Authentication));
        assert_eq!(spawns.load(Ordering::SeqCst), 1);

        // The reader signs in with the CLI — the material changes to a live
        // credential — and the ordinary path recovers without any touch.
        fs::write(
            &path,
            credentials_file((real_now_secs() + 3_600) * 1_000, "max"),
        )
        .expect("write re-login credentials");
        source.cooldown.open_for_test();
        let outcome = source.fetch(USER_MAX_AGE);
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.snapshots.len(), 1);
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        assert_eq!(usage_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_user_initiated_ceiling_splits_the_popover_from_the_monitor() {
        // The popover asks with fifty seconds; the background monitor with
        // its five-minute tick. The ceiling must keep splitting them.
        assert!(user_initiated(std::time::Duration::from_secs(50)));
        assert!(!user_initiated(std::time::Duration::from_secs(300)));
        assert!(!user_initiated(USER_INITIATED_MAX_AGE));
    }

    /// A blank login in the shape the CLI leaves after its login expires
    /// for good. Synthetic values only.
    const BLANK_LOGIN: &str = r#"{"claudeAiOauth":{"accessToken":"","refreshToken":"","expiresAt":0,"scopes":[],"subscriptionType":"max"}}"#;

    /// An item that holds only MCP logins.
    const MCP_ONLY: &str = r#"{"mcpOAuth":{"synthetic-server":{}}}"#;

    #[test]
    fn a_blank_login_does_not_shadow_a_live_carrier() {
        let blank = parse_credentials_json(BLANK_LOGIN).unwrap();
        let live = ClaudeCredentials {
            access_token: "live-token".into(),
            expires_at_ms: (NOW + 3_600) * 1_000,
            has_refresh_token: true,
            subscription_type: None,
            rate_limit_tier: None,
        };
        struct LiveOnly;
        impl AnthropicTransport for LiveOnly {
            fn usage(&self, token: &str) -> Result<String, ProviderUsageError> {
                (token == "live-token")
                    .then_some(LIVE_USAGE_BODY.to_owned())
                    .ok_or(ProviderUsageError::Authentication)
            }
            fn profile(&self, _: &str) -> Option<String> {
                None
            }
        }
        let result = fetch_from_carriers(&LiveOnly, vec![blank.clone(), live], None, now())
            .expect("live carrier")
            .expect("snapshot");
        assert_eq!(result.windows.len(), 1);
        // A blank login alone is an expired login: authentication.
        assert_eq!(
            fetch_from_carriers(&UnreachableTransport, vec![blank], None, now()),
            Err(ProviderUsageError::Authentication)
        );
    }

    #[test]
    fn an_unchanged_credentials_file_is_not_parsed_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".credentials.json");
        fs::write(&path, credentials_file((NOW + 3_600) * 1_000, "max")).unwrap();
        let source = ClaudeDirectFetch::at(path.clone());
        assert!(source.read_file_login(&path).is_some());
        assert_eq!(source.logins.file(&path), CachedState::Usable);
        // The flag stays while the content hash stays.
        source.logins.set_sign_in_required(true);
        assert!(source.read_file_login(&path).is_some());
        assert_eq!(source.logins.file(&path), CachedState::SignInRequired);
        // A new file is parsed again and starts without the flag.
        fs::write(&path, credentials_file((NOW + 7_200) * 1_000, "pro")).unwrap();
        let login = source.read_file_login(&path).unwrap();
        assert_eq!(login.subscription_type.as_deref(), Some("pro"));
        assert_eq!(source.logins.file(&path), CachedState::Usable);
        // An MCP-only file holds no Claude login.
        fs::write(&path, MCP_ONLY).unwrap();
        assert!(source.read_file_login(&path).is_none());
        assert_eq!(source.logins.file(&path), CachedState::NoLogin);
        // A removed file clears the cache.
        fs::remove_file(&path).unwrap();
        assert!(source.read_file_login(&path).is_none());
        assert!(lock(&source.logins.file).is_none());
    }

    #[test]
    fn detection_uses_the_cached_login_state_without_a_secret_read() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".claude");
        fs::create_dir(&config).unwrap();
        let path = config.join(".credentials.json");
        fs::write(&path, MCP_ONLY).unwrap();
        let source = ClaudeDirectFetch::at(path.clone());
        // Before the first check, an existing file counts as signed in.
        assert_eq!(source.detect(false).detection, Detection::SignedIn);
        // The check finds no Claude login: the install is not signed in.
        assert!(source.read_file_login(&path).is_none());
        assert_eq!(
            source.detect(false).detection,
            Detection::InstalledNotSignedIn
        );
        // A login that the refresh could not recover needs a new sign-in.
        fs::write(&path, BLANK_LOGIN).unwrap();
        assert!(source.read_file_login(&path).is_some());
        assert_eq!(source.detect(false).detection, Detection::SignedIn);
        source.logins.set_sign_in_required(true);
        assert_eq!(
            source.detect(false),
            Presence::via(
                Detection::SignInRequired,
                LoginCarrier::ClaudeCredentialsFile
            )
        );
    }

    #[cfg(target_os = "macos")]
    mod keychain_marker {
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
                .read_keychain_login(found(ATTRIBUTES_A), secret(expired), &sleep)
                .unwrap()
                .unwrap();
            assert!(!first.is_live(OffsetDateTime::now_utc()));
            let again = source
                .read_keychain_login(found(ATTRIBUTES_A), no_secret_read, &sleep)
                .unwrap()
                .unwrap();
            assert_eq!(again.expires_at_ms, first.expires_at_ms);
            // A blank login is cached, too.
            let source = self::source();
            source
                .read_keychain_login(found(ATTRIBUTES_A), secret(BLANK_LOGIN.into()), &sleep)
                .unwrap()
                .unwrap();
            let blank = source
                .read_keychain_login(found(ATTRIBUTES_A), no_secret_read, &sleep)
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
                .read_keychain_login(found(ATTRIBUTES_A), || read(old), &sleep)
                .unwrap();
            let login = source
                .read_keychain_login(found(ATTRIBUTES_B), || read(new), &sleep)
                .unwrap()
                .unwrap();
            assert_eq!(login.subscription_type.as_deref(), Some("pro"));
            source
                .read_keychain_login(found(ATTRIBUTES_B), no_secret_read, &sleep)
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
                )
                .unwrap();
            let absent = source
                .read_keychain_login(|| KeychainMetadata::Absent, no_secret_read, &sleep)
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
                .read_keychain_login(unreadable, no_secret_read, &sleep)
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
                )
                .unwrap();
            let kept = source
                .read_keychain_login(|| KeychainMetadata::Unreadable, no_secret_read, &sleep)
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
                    .read_keychain_login(flaky, no_secret_read, &sleep)
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
                )
                .err()
                .expect("a Keychain failure");
            assert_eq!(failure.error, ProviderUsageError::Unavailable);
            assert_eq!(failure.detail, Some(SourceErrorDetail::KeychainUnreadable));
            // With no other carrier, the failure reaches the outcome instead
            // of an empty success that removes the provider.
            let outcome = with_carrier_error(Ok(None), Some(failure)).unwrap_err();
            assert_eq!(outcome.detail, Some(SourceErrorDetail::KeychainUnreadable));
            // The failed read caches nothing, so the next check reads again.
            assert!(
                source
                    .read_keychain_login(
                        found(ATTRIBUTES_A),
                        secret(credentials_file(i64::MAX, "max")),
                        &sleep,
                    )
                    .unwrap()
                    .is_some()
            );
        }

        #[test]
        fn an_mcp_only_item_is_no_login_and_detection_skips_it() {
            let source = source();
            let sleep = |_| {};
            let login = source
                .read_keychain_login(found(ATTRIBUTES_A), secret(MCP_ONLY.into()), &sleep)
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
                .read_keychain_login(found(ATTRIBUTES_A), secret(BLANK_LOGIN.into()), &sleep)
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
    }

    #[test]
    fn a_background_check_refreshes_an_expired_login() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(
            &path,
            credentials_file((real_now_secs() - 3_600) * 1_000, "max"),
        )
        .expect("write expired credentials");
        let spawns = Arc::new(AtomicUsize::new(0));
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(CountingTransport {
                usage_calls: Arc::new(AtomicUsize::new(0)),
                usage_result: Ok(LIVE_USAGE_BODY.to_string()),
            }),
            Box::new(FileTouchEnv {
                path,
                binary_present: true,
                writes_on_spawn: Some(credentials_file((real_now_secs() + 3_600) * 1_000, "max")),
                spawns: Arc::clone(&spawns),
            }),
        );

        // `TEST_MAX_AGE` is the background monitor's shape.
        let outcome = source.fetch(TEST_MAX_AGE);

        assert_eq!(outcome.error, None);
        assert_eq!(outcome.snapshots.len(), 1);
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_background_check_waits_longer_between_attempts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(
            &path,
            credentials_file((real_now_secs() - 3_600) * 1_000, "max"),
        )
        .unwrap();
        let spawns = Arc::new(AtomicUsize::new(0));
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(UnreachableTransport),
            Box::new(FileTouchEnv {
                path,
                binary_present: true,
                writes_on_spawn: None,
                spawns: Arc::clone(&spawns),
            }),
        );
        let outcome = source.fetch(TEST_MAX_AGE);
        // A refreshable login that did not refresh yet stays recoverable.
        assert_eq!(outcome.detail, Some(SourceErrorDetail::RefreshPending));
        source.cooldown.open_for_test();
        let outcome = source.fetch(TEST_MAX_AGE);
        assert_eq!(outcome.detail, Some(SourceErrorDetail::RefreshPending));
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        assert_eq!(
            RefreshTrigger::Expired,
            RefreshTrigger::after_rejection(&[valid_credentials()])
        );
        assert_eq!(
            ClaudeDirectFetch::touch_request(false, RefreshTrigger::Expired).cooldown,
            claude_touch::BACKGROUND_TOUCH_COOLDOWN
        );
        assert_eq!(
            ClaudeDirectFetch::touch_request(true, RefreshTrigger::Expired).cooldown,
            claude_touch::TOUCH_COOLDOWN
        );
    }

    /// A transport that accepts only one token.
    struct OnlyToken(&'static str);

    impl AnthropicTransport for OnlyToken {
        fn usage(&self, token: &str) -> Result<String, ProviderUsageError> {
            assert_eq!(token, self.0, "the check must use the refreshed token");
            Ok(LIVE_USAGE_BODY.to_string())
        }
        fn profile(&self, _token: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn a_login_that_expires_soon_refreshes_before_the_usage_call() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(
            &path,
            credentials_file((real_now_secs() + 5 * 60) * 1_000, "max"),
        )
        .unwrap();
        let mut refreshed: serde_json::Value = serde_json::from_str(&credentials_file(
            (real_now_secs() + 8 * 3_600) * 1_000,
            "max",
        ))
        .unwrap();
        refreshed["claudeAiOauth"]["accessToken"] = "refreshed-access".into();
        let spawns = Arc::new(AtomicUsize::new(0));
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(OnlyToken("refreshed-access")),
            Box::new(FileTouchEnv {
                path,
                binary_present: true,
                writes_on_spawn: Some(refreshed.to_string()),
                spawns: Arc::clone(&spawns),
            }),
        );
        let outcome = source.fetch(TEST_MAX_AGE);
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.snapshots.len(), 1);
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_pre_expiry_refresh_that_does_not_settle_keeps_the_live_token() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(
            &path,
            credentials_file((real_now_secs() + 5 * 60) * 1_000, "max"),
        )
        .unwrap();
        let spawns = Arc::new(AtomicUsize::new(0));
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(OnlyToken("synthetic-token")),
            Box::new(FileTouchEnv {
                path,
                binary_present: true,
                writes_on_spawn: None,
                spawns: Arc::clone(&spawns),
            }),
        );
        let outcome = source.fetch(USER_MAX_AGE);
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.snapshots.len(), 1);
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_blank_login_gets_one_refresh_then_needs_a_new_sign_in() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(&path, BLANK_LOGIN).unwrap();
        let spawns = Arc::new(AtomicUsize::new(0));
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(CountingTransport {
                usage_calls: Arc::new(AtomicUsize::new(0)),
                usage_result: Ok(LIVE_USAGE_BODY.to_string()),
            }),
            Box::new(FileTouchEnv {
                path: path.clone(),
                binary_present: true,
                writes_on_spawn: None,
                spawns: Arc::clone(&spawns),
            }),
        );
        let outcome = source.fetch(TEST_MAX_AGE);
        assert_eq!(outcome.error, Some(ProviderUsageError::Authentication));
        assert_eq!(outcome.detail, Some(SourceErrorDetail::SignInRequired));
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        assert_eq!(
            source.detect(false),
            Presence::via(
                Detection::SignInRequired,
                LoginCarrier::ClaudeCredentialsFile
            )
        );

        // The same material: no second attempt, the same state.
        source.cooldown.open_for_test();
        source.touch_gate.open_cooldown_for_test();
        let outcome = source.fetch(USER_MAX_AGE);
        assert_eq!(outcome.detail, Some(SourceErrorDetail::SignInRequired));
        assert_eq!(spawns.load(Ordering::SeqCst), 1);

        // After `/login` the material changes and the meter returns.
        fs::write(
            &path,
            credentials_file((real_now_secs() + 3_600) * 1_000, "max"),
        )
        .unwrap();
        source.cooldown.open_for_test();
        let outcome = source.fetch(USER_MAX_AGE);
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.snapshots.len(), 1);
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        assert_eq!(source.detect(false).detection, Detection::SignedIn);
    }

    #[test]
    fn an_expired_login_without_a_refresh_token_needs_a_new_sign_in() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(
            &path,
            format!(
                r#"{{"claudeAiOauth":{{"accessToken":"synthetic","expiresAt":{}}}}}"#,
                (real_now_secs() - 3_600) * 1_000
            ),
        )
        .unwrap();
        let spawns = Arc::new(AtomicUsize::new(0));
        let source = ClaudeDirectFetch::at_with_touch(
            path.clone(),
            Box::new(UnreachableTransport),
            Box::new(FileTouchEnv {
                path,
                binary_present: true,
                writes_on_spawn: None,
                spawns: Arc::clone(&spawns),
            }),
        );
        let outcome = source.fetch(USER_MAX_AGE);
        assert_eq!(outcome.detail, Some(SourceErrorDetail::SignInRequired));
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn no_claude_login_never_starts_the_cli() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(&path, MCP_ONLY).unwrap();
        let source = ClaudeDirectFetch::at_with_touch(
            path,
            Box::new(UnreachableTransport),
            Box::new(UntouchableEnv),
        );
        for max_age in [USER_MAX_AGE, TEST_MAX_AGE] {
            source.cooldown.open_for_test();
            let outcome = source.fetch(max_age);
            assert_eq!(outcome.error, None);
            assert!(outcome.snapshots.is_empty());
        }
        let missing = ClaudeDirectFetch::at_with_touch(
            PathBuf::from("/nonexistent/.credentials.json"),
            Box::new(UnreachableTransport),
            Box::new(UntouchableEnv),
        );
        assert_eq!(missing.fetch(USER_MAX_AGE).error, None);
    }

    #[test]
    fn claude_with_an_install_and_no_login_does_not_vanish() {
        // The config folder exists, the file holds no Claude login, and the
        // fetch returns nothing. `collect` must report "not signed in".
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(&path, MCP_ONLY).unwrap();
        let sources: Vec<Box<dyn crate::provider_usage::live::LiveUsageSource>> =
            vec![Box::new(ClaudeDirectFetch::at(path))];
        let collected = super::super::collect(
            &sources,
            true,
            &crate::store::HiddenMeters::default(),
            TEST_MAX_AGE,
        );
        assert!(collected.snapshots.is_empty());
        assert_eq!(collected.errors.len(), 1);
        assert_eq!(
            collected.errors[0].error,
            ProviderUsageError::Authentication
        );
        assert_eq!(
            collected.errors[0].detail,
            Some(SourceErrorDetail::NotSignedIn)
        );
    }
}
