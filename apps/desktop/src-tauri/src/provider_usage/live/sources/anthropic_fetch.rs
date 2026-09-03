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
//! **This source never refreshes that token.** Refreshing is a lifecycle
//! decision — issuing a new credential under the reader's account — and it
//! belongs to the tool that owns the token's lifecycle, which is the CLI, not
//! a background reader of its credential. If `expiresAt` has already passed,
//! that is reported as an authentication failure without a network call: the
//! fix is signing in again with the CLI, not a retry from here.
//!
//! Finding neither carrier — no Keychain item, no credentials file, or
//! either one in a shape this parser does not recognize — is not an error.
//! It is the ordinary state of a machine where the CLI has never signed in.
//! The source simply has nothing to report.
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

use serde_json::Value;
use time::OffsetDateTime;

use crate::provider_usage::live::anthropic;
use crate::provider_usage::live::model::{
    Confidence, Freshness, ProviderUsageError, ProviderUsageSnapshot, SchemaReason, UsageSource,
};
use crate::provider_usage::live::{LiveUsageSource, SourceOutcome};

use super::claude_config_cache::{self, CachedUsage};
use super::cooldown::{self, Cooldown, FetchFailure};
use super::http;

/// The credentials file is a small, purpose-built OAuth token store, not a
/// general state file — cap the read defensively rather than trust that.
const MAX_CREDENTIAL_BYTES: u64 = 256 * 1024;

const USAGE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/profile";

/// The stable id [`SourceOutcome::error`] and the milestone engine key this
/// source under.
const SOURCE_ID: &str = "claude-usage-fetch";

/// The credential file, at the one documented place it lives.
pub fn default_credentials_path() -> Option<PathBuf> {
    let dir = antiburn_local::paths::non_empty_env_path("CLAUDE_CONFIG_DIR")
        .or_else(|| antiburn_local::paths::home_dir().map(|home| home.join(".claude")))?;
    Some(dir.join(".credentials.json"))
}

/// What this source needs out of the CLI's own credential file. Nothing more
/// is read — in particular, never `refreshToken`, since this source has no
/// use for it and no business holding it.
struct ClaudeCredentials {
    access_token: String,
    expires_at_ms: i64,
    subscription_type: Option<String>,
    /// The finer-grained tier within `subscriptionType`, for example
    /// `default_claude_max_5x`.
    rate_limit_tier: Option<String>,
}

/// Read and parse the credentials file. `None` covers both "no file" and "a
/// file that is not this shape" — see the module doc for why neither is an
/// error here.
fn read_credentials_file(path: &Path) -> Option<ClaudeCredentials> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > MAX_CREDENTIAL_BYTES {
        return None;
    }
    let contents = fs::read_to_string(path).ok()?;
    parse_credentials_json(&contents)
}

/// Parse the `{"claudeAiOauth": {...}}` shape both carriers hold — the
/// Keychain's raw value and the credentials file's contents are the same
/// JSON, so one function reads either. `None` covers every way the input is
/// not this shape; see the module doc for why that is not an error.
fn parse_credentials_json(contents: &str) -> Option<ClaudeCredentials> {
    let value: Value = serde_json::from_str(contents).ok()?;
    let oauth = value.get("claudeAiOauth")?;
    Some(ClaudeCredentials {
        access_token: oauth.get("accessToken")?.as_str()?.to_owned(),
        expires_at_ms: oauth.get("expiresAt")?.as_i64()?,
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
mod macos_keychain {
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

    /// Reads one Keychain item. See [`KeychainRead`] for what each outcome
    /// means.
    pub fn read() -> KeychainRead {
        let mut child = match antiburn_local::platform::process::headless_std_command("security")
            .args(["find-generic-password", "-s", SERVICE_NAME, "-w"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return KeychainRead::Unreadable,
        };

        let Some(mut stdout) = child.stdout.take() else {
            return KeychainRead::Unreadable;
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
                let _ = child.wait();
                return KeychainRead::Unreadable;
            }
        };
        let Ok(status) = child.wait() else {
            return KeychainRead::Unreadable;
        };
        if !status.success() {
            return classify_failed_exit(status.code());
        }
        if bytes.is_empty() || bytes.len() > MAX_BYTES {
            return KeychainRead::Unreadable;
        }
        match String::from_utf8(bytes) {
            Ok(text) => KeychainRead::Found(text),
            Err(_) => KeychainRead::Unreadable,
        }
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

/// Asks `GET /api/oauth/usage` with the CLI's own access token, after first
/// checking the CLI's own cached reading — see the module doc's "The CLI's
/// own cache" section.
pub struct ClaudeDirectFetch {
    credentials_path: Option<PathBuf>,
    /// Whether to consult the macOS Keychain before falling back to the
    /// credentials file. Always true in production; the `at()` test
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
}

impl ClaudeDirectFetch {
    pub fn new() -> ClaudeDirectFetch {
        ClaudeDirectFetch {
            credentials_path: default_credentials_path(),
            #[cfg(target_os = "macos")]
            try_keychain: true,
            config_cache_path: claude_config_cache::default_config_path(),
            transport: Box::new(LiveAnthropicTransport),
            cooldown: Cooldown::new(),
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
            #[cfg(target_os = "macos")]
            try_keychain: false,
            config_cache_path: None,
            transport: Box::new(LiveAnthropicTransport),
            cooldown: Cooldown::new(),
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
            #[cfg(target_os = "macos")]
            try_keychain: false,
            config_cache_path: Some(config_cache_path),
            transport,
            cooldown: Cooldown::new(),
        }
    }

    /// The credential from whichever carrier answers first: the Keychain,
    /// where this source is built to try it, then the credentials file.
    ///
    /// `Err` means the Keychain carrier could not say whether a credential
    /// exists — a timeout, a spawn failure, or an exit this carrier does not
    /// recognize — and the credentials-file fallback is skipped: falling
    /// back would read a transient failure as "signed out". It is always
    /// `ProviderUsageError::Unavailable`, reported as a real failure so
    /// `Cooldown` keeps the last good snapshot instead of clearing it.
    fn read_credentials(&self) -> Result<Option<ClaudeCredentials>, ProviderUsageError> {
        #[cfg(target_os = "macos")]
        if self.try_keychain {
            match macos_keychain::read() {
                macos_keychain::KeychainRead::Found(text) => {
                    if let Some(credentials) = parse_credentials_json(&text) {
                        return Ok(Some(credentials));
                    }
                }
                macos_keychain::KeychainRead::Unreadable => {
                    return Err(ProviderUsageError::Unavailable);
                }
                macos_keychain::KeychainRead::Absent => {}
            }
        }
        Ok(self
            .credentials_path
            .as_deref()
            .and_then(read_credentials_file))
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

    fn fetch(&self, max_age: std::time::Duration) -> SourceOutcome {
        let now = OffsetDateTime::now_utc();
        // The credential read, and the config-cache read, both sit inside
        // the cooldown gate on purpose: on macOS the former spawns a
        // `security` subprocess, and a poll that the cooldown is going to
        // skip anyway should not pay for either — nor re-raise a Keychain
        // access prompt the reader has already seen.
        self.cooldown.poll(now, max_age, || {
            let Some(credentials) = self.read_credentials()? else {
                return Ok(None);
            };
            let cached = self
                .config_cache_path
                .as_deref()
                .and_then(claude_config_cache::read_cached_usage);
            fetch_with_cache(self.transport.as_ref(), &credentials, cached, max_age, now)
        })
    }
}

/// The network calls the live path needs, as a trait so a test can supply
/// them without a socket — the same seam `codex_fetch`'s `CodexTransport`
/// gives that source.
trait AnthropicTransport: Send + Sync {
    fn usage(&self, access_token: &str) -> Result<String, ProviderUsageError>;
    /// The account id from the profile endpoint. `None` on any failure: this
    /// is a best-effort enrichment, never a reason to fail the snapshot.
    fn profile(&self, access_token: &str) -> Option<String>;
}

struct LiveAnthropicTransport;

impl AnthropicTransport for LiveAnthropicTransport {
    fn usage(&self, access_token: &str) -> Result<String, ProviderUsageError> {
        let response = http::client()
            .get(USAGE_ENDPOINT)
            .bearer_auth(access_token)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("anthropic-beta", "oauth-2025-04-20")
            .send()
            .map_err(|_| ProviderUsageError::Unavailable)?;
        if let Some(error) = http::status_error(response.status()) {
            return Err(error);
        }
        http::read_capped_body(response)
    }

    fn profile(&self, access_token: &str) -> Option<String> {
        let response = http::client()
            .get(PROFILE_ENDPOINT)
            .bearer_auth(access_token)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("anthropic-beta", "oauth-2025-04-20")
            .send()
            .ok()?;
        if http::status_error(response.status()).is_some() {
            return None;
        }
        let body = http::read_capped_body(response).ok()?;
        profile_subject(&body)
    }
}

/// The two-tier decision the module doc's "The CLI's own cache" section
/// describes. A free function over [`AnthropicTransport`] rather than a
/// method, so a test calls it with a fake transport and nothing else this
/// source owns — mirrors `codex_fetch::fetch_direct`.
fn fetch_with_cache(
    transport: &dyn AnthropicTransport,
    credentials: &ClaudeCredentials,
    cached: Option<CachedUsage>,
    max_age: std::time::Duration,
    now: OffsetDateTime,
) -> Result<Option<ProviderUsageSnapshot>, FetchFailure> {
    if let Some(cached) = cached.clone()
        && cache_covers(now, cached.observed_at, max_age)
    {
        log_cache_reading_used(&cached, now, "fresh");
        return Ok(Some(snapshot_from_cache(cached, credentials)));
    }

    match fetch_live(transport, credentials, now) {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(error) => {
            let last_known = cached
                .filter(|cached| now - cached.observed_at <= cooldown::MAX_AGE)
                .map(|cached| {
                    log_cache_reading_used(&cached, now, "seed");
                    Box::new(snapshot_from_cache(cached, credentials))
                });
            Err(FetchFailure { error, last_known })
        }
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
        provider: crate::provider_usage::providers::ANTHROPIC,
        account: Some(cached.account),
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
    now: OffsetDateTime,
) -> Result<ProviderUsageSnapshot, ProviderUsageError> {
    let expires_at = OffsetDateTime::from_unix_timestamp(credentials.expires_at_ms / 1_000)
        .map_err(|_| ProviderUsageError::Schema(SchemaReason::InvalidValue))?;
    if expires_at <= now {
        return Err(ProviderUsageError::Authentication);
    }

    let body = transport.usage(&credentials.access_token)?;
    let usage = anthropic::parse_usage(&body)?;
    let account = transport.profile(&credentials.access_token);

    Ok(ProviderUsageSnapshot {
        provider: crate::provider_usage::providers::ANTHROPIC,
        account,
        plan: credentials.subscription_type.clone(),
        plan_tier: credentials.rate_limit_tier.clone(),
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

fn profile_subject(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("account")?
        .get("uuid")?
        .as_str()
        .map(str::trim)
        .filter(|subject| !subject.is_empty() && subject.len() <= 512)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            subscription_type: Some("max".into()),
            rate_limit_tier: Some("default_claude_max_5x".into()),
        }
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
            profile_subject(
                r#"{"account":{"uuid":"account-uuid","email":"private@example.test"},"organization":{"uuid":"organization-uuid"}}"#
            )
            .as_deref(),
            Some("account-uuid")
        );
        assert_eq!(
            profile_subject(r#"{"account":{"email":"private@example.test"}}"#),
            None
        );
        assert_eq!(
            profile_subject(&format!(
                r#"{{"account":{{"uuid":"{}"}}}}"#,
                "a".repeat(513)
            )),
            None
        );
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
    fn an_expired_token_is_an_authentication_failure_with_no_network_call() {
        let now = OffsetDateTime::from_unix_timestamp(NOW).unwrap();
        let credentials = ClaudeCredentials {
            access_token: "synthetic-token".into(),
            expires_at_ms: (NOW - 3_600) * 1_000,
            subscription_type: Some("max".into()),
            rate_limit_tier: None,
        };
        assert_eq!(
            fetch_live(&UnreachableTransport, &credentials, now),
            Err(ProviderUsageError::Authentication)
        );
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
}
