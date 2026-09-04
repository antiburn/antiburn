//! Ask Codex directly for the reader's own plan usage, refreshing its token
//! once if the provider says no, and falling back to the Codex app's own
//! process ([`super::codex_app_server`]) if a direct request never succeeds.
//!
//! # The credential
//!
//! The Codex CLI keeps its tokens in `$CODEX_HOME/auth.json` when that
//! variable is set, and `~/.codex/auth.json` otherwise. Unlike Claude's
//! credential file, Codex's own auth flow *does* rotate its access token
//! under this application's feet, and this source is written expecting that:
//! a rejected token is retried once, with a token this source itself
//! refreshes — see "Retrying with a fresh token" below. A missing or
//! unparseable file is not an error, for the same reason it is not one for
//! Claude: it is the ordinary state of a machine that has never signed in
//! with Codex.
//!
//! # The account id
//!
//! The provider's usage endpoint reads best with a `ChatGPT-Account-Id`
//! header. `auth.json`'s own `tokens.account_id` is used when present;
//! otherwise this source decodes the unsigned payload segment of the access
//! token (falling back to the id token) and reads the
//! `https://api.openai.com/auth/chatgpt_account_id` claim out of it. This is
//! a decode, not a verification — nothing here checks a signature, and
//! nothing needs to: the token still has to clear the provider's own
//! authentication to be worth anything, so a forged claim only ever costs the
//! forger a rejected request.
//!
//! # Retrying with a fresh token
//!
//! A first attempt that fails for *any* reason is retried exactly once, with
//! an access token this source obtains itself from
//! `POST https://auth.openai.com/oauth/token`. The refreshed token is kept
//! in memory only, and only for as long as `auth.json`'s own refresh token
//! keeps matching what it was refreshed against — a changed `auth.json`
//! (a sign-out, a sign-in as someone else) invalidates the cache for free
//! rather than needing its own expiry logic. It is never written back to
//! `auth.json`: that file belongs to the Codex CLI, and this source only
//! ever reads it.
//!
//! # Falling back
//!
//! If the retried attempt also fails, this source asks the same question a
//! different way: through [`super::codex_app_server`], which spawns the
//! `codex` executable itself and asks it over its own JSON-RPC protocol. The
//! two paths can disagree about which error is more informative — see
//! [`preferred_error`] — but only one of them ever contributes a snapshot.
//!
//! # Seeding from the session log
//!
//! When the retried direct attempt and the app-server fallback both fail,
//! this source makes one more offer: the newest rate-limit reading in the
//! reader's own Codex CLI session log, read by
//! [`super::codex_rollout::latest_reading`]. It travels as
//! [`FetchFailure::last_known`](super::cooldown::FetchFailure::last_known),
//! not as a success — [`super::cooldown::Cooldown::poll`] only lets it
//! replace an on-screen reading once that reading is stale enough to need
//! one. This reading is a seed, never a full source, because a CLI rollout
//! event states only the account-wide window. It carries none of the
//! model-scoped `additional_rate_limits` the live endpoint reports, and
//! swapping a snapshot between those two shapes on every poll would make
//! rows come and go on screen for no reason a reader could see.
//!
//! # Testability
//!
//! [`CodexTransport`] is the seam: [`fetch_direct`] is a free function over
//! the trait, exercised in tests with a fake that never opens a socket, so
//! the retry-and-cache logic above is covered without a mockable network at
//! the process level.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::Value;
use time::OffsetDateTime;

use crate::provider_usage::live::codex;
use crate::provider_usage::live::model::{
    Confidence, Freshness, ProviderUsageError, ProviderUsageSnapshot, SchemaReason, UsageSource,
};
use crate::provider_usage::live::{LiveUsageSource, SourceOutcome};

use super::codex_app_server;
use super::cooldown::{Cooldown, FetchFailure};
use super::http;
use super::pi_auth;

/// `auth.json` is a small, purpose-built token store — cap the read
/// defensively rather than trust that.
const MAX_CREDENTIAL_BYTES: u64 = 256 * 1024;

// aislop-ignore-next-line ai-slop/hardcoded-url -- Codex uses this fixed endpoint for plan usage.
const WHAM_USAGE_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";
const TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";

/// Codex CLI's own public OAuth client id. Public in the sense every install
/// of the CLI carries it; it identifies the client application to the
/// authorization server, not the reader.
// aislop-ignore-next-line ai-slop/hardcoded-id -- Codex uses this public client ID for each CLI installation.
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// The unsigned JWT claim that names the account, when `auth.json` did not
/// state one directly.
const ACCOUNT_CLAIM: &str = "https://api.openai.com/auth/chatgpt_account_id";

/// The unsigned JWT claim that names the plan, used only when the usage
/// response itself does not state one.
const PLAN_CLAIM: &str = "https://api.openai.com/auth/chatgpt_plan_type";

/// The Codex CLI's own root directory: `$CODEX_HOME` when it is set and
/// non-empty, otherwise `~/.codex`. Both `auth.json` and the session rollout
/// files live under it.
fn codex_home_dir() -> Option<PathBuf> {
    antiburn_local::paths::non_empty_env_path("CODEX_HOME")
        .or_else(|| antiburn_local::paths::home_dir().map(|home| home.join(".codex")))
}

/// The credential file, at the one documented place it lives.
pub fn default_auth_path() -> Option<PathBuf> {
    Some(codex_home_dir()?.join("auth.json"))
}

/// The session log root the CLI appends `token_count` events under, at the
/// one documented place it lives. See `codex_rollout` for what this source
/// reads out of it.
fn default_sessions_root() -> Option<PathBuf> {
    Some(codex_home_dir()?.join("sessions"))
}

/// What this source needs out of the CLI's own `auth.json`.
struct CodexAuth {
    access_token: String,
    refresh_token: String,
    account_id: Option<String>,
    /// The `chatgpt_plan_type` claim, decoded ahead of time so a later
    /// missing `plan_type` in the usage response has something to fall back
    /// to — see [`PLAN_CLAIM`].
    plan_claim: Option<String>,
}

/// Read and parse `auth.json`. `None` covers both "no file" and "a file that
/// is not this shape" — see the module doc for why neither is an error here.
fn read_auth(path: &Path) -> Option<CodexAuth> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > MAX_CREDENTIAL_BYTES {
        return None;
    }
    let contents = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&contents).ok()?;
    let tokens = value.get("tokens")?;
    let access_token = tokens.get("access_token")?.as_str()?.to_owned();
    let refresh_token = tokens.get("refresh_token")?.as_str()?.to_owned();
    let id_token = tokens.get("id_token").and_then(Value::as_str);
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(|| claim_from_tokens(&access_token, id_token, ACCOUNT_CLAIM));
    let plan_claim = claim_from_tokens(&access_token, id_token, PLAN_CLAIM);
    Some(CodexAuth {
        access_token,
        refresh_token,
        account_id,
        plan_claim,
    })
}

/// Read one claim from the access token, falling back to the id token —
/// the order every unsigned-JWT fallback in this source uses.
fn claim_from_tokens(access_token: &str, id_token: Option<&str>, claim: &str) -> Option<String> {
    decode_jwt_claim(access_token, claim)
        .or_else(|| id_token.and_then(|token| decode_jwt_claim(token, claim)))
}

/// Read one claim out of a JWT's payload segment, without checking its
/// signature — see the module doc for why that is fine here.
fn decode_jwt_claim(token: &str, claim: &str) -> Option<String> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value.get(claim)?.as_str().map(str::to_owned)
}

/// A prior refresh, kept only as long as it still applies. If `auth.json`'s
/// own refresh token no longer matches, the CLI signed in as someone else (or
/// out and back in) since this was cached, and reusing it would ask the
/// provider about the wrong account.
struct RefreshedToken {
    refresh_token: String,
    access_token: String,
}

/// The network calls [`fetch_direct`] needs, as a trait so a test can supply
/// them without a socket.
trait CodexTransport: Send + Sync {
    fn usage(
        &self,
        access_token: &str,
        account_id: Option<&str>,
    ) -> Result<String, ProviderUsageError>;
    fn refresh(&self, refresh_token: &str) -> Result<String, ProviderUsageError>;
}

struct LiveCodexTransport;

impl CodexTransport for LiveCodexTransport {
    fn usage(
        &self,
        access_token: &str,
        account_id: Option<&str>,
    ) -> Result<String, ProviderUsageError> {
        let mut request = http::client()
            .get(WHAM_USAGE_ENDPOINT)
            .bearer_auth(access_token)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, "codex-cli");
        if let Some(account_id) = account_id {
            request = request.header("ChatGPT-Account-Id", account_id);
        }
        let response = request
            .send()
            .map_err(|_| ProviderUsageError::Unavailable)?;
        if let Some(error) = http::status_error(response.status()) {
            return Err(error);
        }
        http::read_capped_body(response)
    }

    fn refresh(&self, refresh_token: &str) -> Result<String, ProviderUsageError> {
        let response = http::client()
            .post(TOKEN_ENDPOINT)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", CLIENT_ID),
            ])
            .send()
            .map_err(|_| ProviderUsageError::Unavailable)?;
        if let Some(error) = http::status_error(response.status()) {
            return Err(error);
        }
        let body = http::read_capped_body(response)?;
        let value: Value = serde_json::from_str(&body)
            .map_err(|_| ProviderUsageError::Schema(SchemaReason::InvalidJson))?;
        value
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_owned)
            .ok_or(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField,
            ))
    }
}

/// Asks `GET /backend-api/wham/usage`, retrying once with a refreshed token,
/// then falling back to the app-server RPC if neither attempt lands.
pub struct CodexDirectFetch {
    auth_path: Option<PathBuf>,
    pi_auth_path: Option<PathBuf>,
    /// The Codex CLI's session log root, for the seed read described in
    /// "Seeding from the session log" above. `None` skips that seed
    /// entirely — the state every test constructor below starts from.
    sessions_root: Option<PathBuf>,
    transport: Box<dyn CodexTransport>,
    cached_refresh: Mutex<Option<RefreshedToken>>,
    cooldown: Cooldown,
}

impl CodexDirectFetch {
    pub fn new() -> CodexDirectFetch {
        CodexDirectFetch {
            auth_path: default_auth_path(),
            pi_auth_path: pi_auth::default_auth_path(),
            sessions_root: default_sessions_root(),
            transport: Box::new(LiveCodexTransport),
            cached_refresh: Mutex::new(None),
            cooldown: Cooldown::new(),
        }
    }

    /// A source rooted at an explicit path, for tests.
    #[cfg(test)]
    pub fn at(path: PathBuf) -> CodexDirectFetch {
        CodexDirectFetch {
            auth_path: Some(path),
            pi_auth_path: None,
            sessions_root: None,
            transport: Box::new(LiveCodexTransport),
            cached_refresh: Mutex::new(None),
            cooldown: Cooldown::new(),
        }
    }

    #[cfg(test)]
    fn with_transport(path: PathBuf, transport: Box<dyn CodexTransport>) -> CodexDirectFetch {
        Self::with_paths(Some(path), None, transport)
    }

    #[cfg(test)]
    fn with_paths(
        auth_path: Option<PathBuf>,
        pi_auth_path: Option<PathBuf>,
        transport: Box<dyn CodexTransport>,
    ) -> CodexDirectFetch {
        CodexDirectFetch {
            auth_path,
            pi_auth_path,
            sessions_root: None,
            transport,
            cached_refresh: Mutex::new(None),
            cooldown: Cooldown::new(),
        }
    }

    /// The same source, reading its seed from `sessions_root` instead of
    /// skipping the rollout tier — for tests that exercise it.
    #[cfg(test)]
    fn with_sessions_root(mut self, sessions_root: PathBuf) -> CodexDirectFetch {
        self.sessions_root = Some(sessions_root);
        self
    }

    /// The newest rollout reading, when this source has a session log root
    /// to look under and a recent enough event is there. See
    /// `codex_rollout::latest_reading`.
    fn rollout_reading(&self, now: OffsetDateTime) -> Option<super::codex_rollout::RolloutReading> {
        let sessions_root = self.sessions_root.as_ref()?;
        let reading = super::codex_rollout::latest_reading(sessions_root, now)?;
        // `debug`, not `warn`: the seed is the ordinary answer to a failure.
        ::tracing::debug!(
            event = "live_cache_reading_used",
            provider = crate::provider_usage::providers::OPENAI,
            age_secs = (now - reading.observed_at).whole_seconds(),
            reason = "seed"
        );
        Some(reading)
    }
}

impl Default for CodexDirectFetch {
    fn default() -> CodexDirectFetch {
        CodexDirectFetch::new()
    }
}

impl LiveUsageSource for CodexDirectFetch {
    fn id(&self) -> &'static str {
        super::CODEX_SOURCE_ID
    }

    fn provider(&self) -> &'static str {
        crate::provider_usage::providers::OPENAI
    }

    fn requires_online_opt_in(&self) -> bool {
        true
    }

    fn fetch(&self, max_age: std::time::Duration) -> SourceOutcome {
        let now = OffsetDateTime::now_utc();
        // Read inside the cooldown gate so skipped polls do not touch disk.
        self.cooldown.poll(now, max_age, || {
            let auth = self.auth_path.as_deref().and_then(read_auth);
            let direct_error = match &auth {
                Some(auth) => {
                    match fetch_direct(self.transport.as_ref(), &self.cached_refresh, auth, now) {
                        Ok(snapshot) => return Ok(Some(snapshot)),
                        Err(error) => Some(error),
                    }
                }
                None => None,
            };

            let pi_entry = self
                .pi_auth_path
                .as_deref()
                .and_then(|path| pi_auth::read_entry(path, pi_auth::CODEX_KEY))
                .filter(|entry| !entry.refresh_token.is_empty())
                .filter(|entry| {
                    i128::from(entry.expires_at_ms) > now.unix_timestamp_nanos() / 1_000_000
                });
            let pi_error = match &pi_entry {
                Some(entry) => match fetch_pi(self.transport.as_ref(), entry, now) {
                    Ok(snapshot) => return Ok(Some(snapshot)),
                    Err(error) => Some(error),
                },
                None => None,
            };

            let carrier_error = match (direct_error, pi_error) {
                (None, None) => return Ok(None),
                (Some(error), None) | (None, Some(error)) => error,
                (Some(first), Some(second)) => carrier_verdict(first, second),
            };
            match codex_app_server::fetch(now) {
                Ok(snapshot) => Ok(snapshot),
                Err(fallback_error) => Err(FetchFailure {
                    error: preferred_error(carrier_error, fallback_error),
                    last_known: auth.as_ref().and_then(|auth| {
                        self.rollout_reading(now)
                            .map(|reading| Box::new(rollout_snapshot(reading, auth)))
                    }),
                }),
            }
        })
    }
}

/// Try Pi's access token once. Pi owns refresh, so this path never refreshes.
fn fetch_pi(
    transport: &dyn CodexTransport,
    entry: &pi_auth::PiOauth,
    now: OffsetDateTime,
) -> Result<ProviderUsageSnapshot, ProviderUsageError> {
    let body = transport.usage(&entry.access_token, entry.account_id.as_deref())?;
    let plan_claim = claim_from_tokens(&entry.access_token, None, PLAN_CLAIM);
    build_snapshot(&body, entry.account_id.clone(), plan_claim, now)
}

fn carrier_verdict(first: ProviderUsageError, second: ProviderUsageError) -> ProviderUsageError {
    if matches!(first, ProviderUsageError::Unavailable)
        && !matches!(second, ProviderUsageError::Unavailable)
    {
        second
    } else {
        first
    }
}

/// Which of two failures is worth telling the reader about, when both the
/// direct request and the app-server fallback failed.
///
/// [`ProviderUsageError::Unavailable`] is the least specific category — "we
/// could not reach it" covers everything from no network to the executable
/// missing — so a more specific verdict from either attempt outranks it.
/// Between two equally specific verdicts, the fallback's is kept: it is the
/// one that had the other's failure as context and still landed on it.
fn preferred_error(direct: ProviderUsageError, fallback: ProviderUsageError) -> ProviderUsageError {
    if matches!(fallback, ProviderUsageError::Unavailable)
        && !matches!(direct, ProviderUsageError::Unavailable)
    {
        direct
    } else {
        fallback
    }
}

/// Turn a rollout reading into the snapshot [`FetchFailure::last_known`]
/// carries — see "Seeding from the session log" in the module doc.
fn rollout_snapshot(
    reading: super::codex_rollout::RolloutReading,
    auth: &CodexAuth,
) -> ProviderUsageSnapshot {
    ProviderUsageSnapshot {
        provider: crate::provider_usage::providers::OPENAI,
        account: auth.account_id.clone(),
        account_uuid: auth.account_id.clone(),
        account_email: None,
        // The rollout event's own plan label wins; the JWT claim is only a
        // fallback for an event that omits it.
        plan: reading.plan.or_else(|| auth.plan_claim.clone()),
        // Codex does not report a finer-grained tier below the plan itself.
        plan_tier: None,
        observed_at: reading.observed_at,
        source: UsageSource {
            id: super::CODEX_SOURCE_ID,
            label: "Read from the Codex CLI's own session log".into(),
            confidence: Confidence::High,
            freshness: Freshness::Fresh,
        },
        windows: reading.windows,
        supplemental: None,
        reset_credits: None,
    }
}

/// One attempt, with the single retry-with-a-refreshed-token this source
/// allows. A free function over [`CodexTransport`] rather than a method, so a
/// test can call it with a fake transport and nothing else this source owns.
fn fetch_direct(
    transport: &dyn CodexTransport,
    cached_refresh: &Mutex<Option<RefreshedToken>>,
    auth: &CodexAuth,
    now: OffsetDateTime,
) -> Result<ProviderUsageSnapshot, ProviderUsageError> {
    let primary = effective_access_token(cached_refresh, auth);
    if let Ok(body) = transport.usage(&primary, auth.account_id.as_deref()) {
        return build_snapshot(&body, auth.account_id.clone(), auth.plan_claim.clone(), now);
    }

    let refreshed = transport.refresh(&auth.refresh_token)?;
    {
        let mut cache = cached_refresh
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *cache = Some(RefreshedToken {
            refresh_token: auth.refresh_token.clone(),
            access_token: refreshed.clone(),
        });
    }
    let body = transport.usage(&refreshed, auth.account_id.as_deref())?;
    build_snapshot(&body, auth.account_id.clone(), auth.plan_claim.clone(), now)
}

/// The access token to try first: a still-applicable cached refresh, or
/// `auth.json`'s own token when there is no such cache.
fn effective_access_token(
    cached_refresh: &Mutex<Option<RefreshedToken>>,
    auth: &CodexAuth,
) -> String {
    let cache = cached_refresh
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match cache.as_ref() {
        Some(cached) if cached.refresh_token == auth.refresh_token => cached.access_token.clone(),
        _ => auth.access_token.clone(),
    }
}

fn build_snapshot(
    body: &str,
    account_id: Option<String>,
    plan_claim: Option<String>,
    now: OffsetDateTime,
) -> Result<ProviderUsageSnapshot, ProviderUsageError> {
    let usage = codex::parse_wham_usage(body, now)?;
    Ok(ProviderUsageSnapshot {
        provider: crate::provider_usage::providers::OPENAI,
        account: account_id.clone(),
        account_uuid: account_id,
        account_email: None,
        // The usage response's own `plan_type` wins; the JWT claim is only a
        // fallback for a response shape that omits it.
        plan: usage.plan.or(plan_claim),
        // Codex does not report a finer-grained tier below the plan itself.
        plan_tier: None,
        observed_at: now,
        source: UsageSource {
            id: super::CODEX_SOURCE_ID,
            label: "Asked Codex directly".into(),
            confidence: Confidence::High,
            // Recomputed on every read by `Cooldown::poll`.
            freshness: Freshness::Fresh,
        },
        windows: usage.windows,
        supplemental: None,
        reset_credits: usage.reset_credits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use time::format_description::well_known::Rfc3339;

    const NOW: i64 = 1_800_000_000;

    /// A background-caller-shaped `max_age` for tests that only exercise
    /// whether a reading is found, not the cooldown's freshness budget —
    /// `cooldown.rs`'s own suite owns that.
    const TEST_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(600);

    const WHAM_BODY: &str = r#"{"rate_limit": {
      "primary_window": {"used_percent": 20, "limit_window_seconds": 18000},
      "secondary_window": {"used_percent": 55, "limit_window_seconds": 604800},
      "plan_type": "plus"
    }}"#;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(NOW).unwrap()
    }

    fn auth(refresh_token: &str) -> CodexAuth {
        CodexAuth {
            access_token: "stale-token".into(),
            refresh_token: refresh_token.into(),
            account_id: Some("acct-123".into()),
            plan_claim: None,
        }
    }

    /// A transport whose `usage` call fails until it is called with
    /// `refreshed-token`, and whose `refresh` call always succeeds.
    struct RefreshSucceeds {
        usage_calls: AtomicUsize,
        refresh_calls: AtomicUsize,
    }

    impl CodexTransport for RefreshSucceeds {
        fn usage(
            &self,
            access_token: &str,
            _account_id: Option<&str>,
        ) -> Result<String, ProviderUsageError> {
            self.usage_calls.fetch_add(1, Ordering::SeqCst);
            if access_token == "refreshed-token" {
                Ok(WHAM_BODY.to_string())
            } else {
                Err(ProviderUsageError::Authentication)
            }
        }
        fn refresh(&self, _refresh_token: &str) -> Result<String, ProviderUsageError> {
            self.refresh_calls.fetch_add(1, Ordering::SeqCst);
            Ok("refreshed-token".into())
        }
    }

    #[test]
    fn a_rejected_token_is_retried_once_with_a_refreshed_one() {
        let transport = RefreshSucceeds {
            usage_calls: AtomicUsize::new(0),
            refresh_calls: AtomicUsize::new(0),
        };
        let cache = Mutex::new(None);
        let snapshot = fetch_direct(&transport, &cache, &auth("refresh-a"), now()).expect("ok");

        assert_eq!(transport.usage_calls.load(Ordering::SeqCst), 2);
        assert_eq!(transport.refresh_calls.load(Ordering::SeqCst), 1);
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.plan.as_deref(), Some("plus"));
    }

    #[test]
    fn a_refreshed_token_is_cached_and_reused_without_refreshing_again() {
        let transport = RefreshSucceeds {
            usage_calls: AtomicUsize::new(0),
            refresh_calls: AtomicUsize::new(0),
        };
        let cache = Mutex::new(None);
        fetch_direct(&transport, &cache, &auth("refresh-a"), now()).expect("ok");
        fetch_direct(&transport, &cache, &auth("refresh-a"), now()).expect("ok");

        // The second call's `usage` succeeds on the very first try because the
        // cached refreshed token from the first call is tried first.
        assert_eq!(transport.usage_calls.load(Ordering::SeqCst), 3);
        assert_eq!(transport.refresh_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_changed_refresh_token_invalidates_the_cached_one() {
        let transport = RefreshSucceeds {
            usage_calls: AtomicUsize::new(0),
            refresh_calls: AtomicUsize::new(0),
        };
        let cache = Mutex::new(None);
        fetch_direct(&transport, &cache, &auth("refresh-a"), now()).expect("ok");
        // A different `auth.json` — a sign-out and back in as someone else —
        // must not reuse the previous account's cached token.
        fetch_direct(&transport, &cache, &auth("refresh-b"), now()).expect("ok");

        assert_eq!(transport.refresh_calls.load(Ordering::SeqCst), 2);
    }

    struct AlwaysFails;
    impl CodexTransport for AlwaysFails {
        fn usage(&self, _: &str, _: Option<&str>) -> Result<String, ProviderUsageError> {
            Err(ProviderUsageError::RateLimited)
        }
        fn refresh(&self, _: &str) -> Result<String, ProviderUsageError> {
            Err(ProviderUsageError::Unavailable)
        }
    }

    #[test]
    fn a_failed_refresh_surfaces_the_refresh_failure() {
        let cache = Mutex::new(None);
        let result = fetch_direct(&AlwaysFails, &cache, &auth("refresh-a"), now());
        assert_eq!(result, Err(ProviderUsageError::Unavailable));
    }

    /// Writes a rollout file under `sessions_root` carrying one qualifying
    /// `token_count` event, dated `now`, at 20% of a seven-day window.
    ///
    /// `codex_app_server::fetch` also fails in this suite — there is no
    /// `codex` binary on the test machine's `PATH` — so a fetch through
    /// [`CodexDirectFetch`] with an always-failing transport reaches the
    /// rollout seed the same way a real failed pair of attempts would.
    fn write_sample_rollout(sessions_root: &Path, now: OffsetDateTime) {
        let day_dir = sessions_root
            .join(format!("{:04}", now.year()))
            .join(format!("{:02}", u8::from(now.month())))
            .join(format!("{:02}", now.day()));
        fs::create_dir_all(&day_dir).expect("mkdir");
        let line = serde_json::json!({
            "timestamp": now.format(&Rfc3339).expect("format"),
            "ordinal": 1,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {},
                "rate_limits": {
                    "limit_id": "codex",
                    "primary": {"used_percent": 20.0, "window_minutes": 10_080, "resets_at": null},
                    "secondary": null,
                    "plan_type": "pro",
                },
            },
        })
        .to_string();
        fs::write(day_dir.join("rollout-a.jsonl"), line).expect("write rollout");
    }

    #[test]
    fn a_failure_with_a_sessions_root_carries_the_rollout_reading_as_last_known() {
        let dir = tempfile::tempdir().expect("tempdir");
        let auth_path = dir.path().join("auth.json");
        fs::write(
            &auth_path,
            r#"{"tokens": {"access_token": "a", "refresh_token": "r", "account_id": "acct-1"}}"#,
        )
        .expect("write");
        let sessions_root = dir.path().join("sessions");
        write_sample_rollout(&sessions_root, now());

        let source = CodexDirectFetch::with_transport(auth_path, Box::new(AlwaysFails))
            .with_sessions_root(sessions_root);
        let outcome = source.fetch(TEST_MAX_AGE);

        assert!(outcome.error.is_some());
        assert_eq!(outcome.snapshots.len(), 1);
        assert_eq!(outcome.snapshots[0].windows[0].id, "seven-day");
        assert_eq!(outcome.snapshots[0].windows[0].used_percent, Some(20.0));
        assert_eq!(
            outcome.snapshots[0].source.label,
            "Read from the Codex CLI's own session log"
        );
    }

    #[test]
    fn a_failure_with_no_sessions_root_carries_no_last_known() {
        let dir = tempfile::tempdir().expect("tempdir");
        let auth_path = dir.path().join("auth.json");
        fs::write(
            &auth_path,
            r#"{"tokens": {"access_token": "a", "refresh_token": "r", "account_id": "acct-1"}}"#,
        )
        .expect("write");

        let source = CodexDirectFetch::with_transport(auth_path, Box::new(AlwaysFails));
        let outcome = source.fetch(TEST_MAX_AGE);

        assert!(outcome.error.is_some());
        assert!(outcome.snapshots.is_empty());
    }

    #[test]
    fn a_missing_credential_file_is_absent_not_an_error() {
        let source = CodexDirectFetch::at(PathBuf::from("/nonexistent/auth.json"));
        let outcome = source.fetch(TEST_MAX_AGE);
        assert!(outcome.snapshots.is_empty());
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn an_unparseable_auth_file_reads_as_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("auth.json");
        fs::write(&path, "not json").expect("write");
        let outcome = CodexDirectFetch::at(path).fetch(TEST_MAX_AGE);
        assert!(outcome.snapshots.is_empty());
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn a_working_transport_produces_a_live_codex_snapshot_end_to_end() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("auth.json");
        fs::write(
            &path,
            r#"{"tokens": {"access_token": "a", "refresh_token": "r", "account_id": "acct-1"}}"#,
        )
        .expect("write");

        struct WorksFirstTry;
        impl CodexTransport for WorksFirstTry {
            fn usage(&self, _: &str, _: Option<&str>) -> Result<String, ProviderUsageError> {
                Ok(WHAM_BODY.to_string())
            }
            fn refresh(&self, _: &str) -> Result<String, ProviderUsageError> {
                unreachable!("a successful first attempt never refreshes")
            }
        }

        let source = CodexDirectFetch::with_transport(path, Box::new(WorksFirstTry));
        let outcome = source.fetch(TEST_MAX_AGE);
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.snapshots.len(), 1);
        assert_eq!(
            outcome.snapshots[0].provider,
            crate::provider_usage::providers::OPENAI
        );
        assert_eq!(outcome.snapshots[0].account.as_deref(), Some("acct-1"));
    }

    #[test]
    fn account_id_falls_back_to_decoding_the_access_tokens_own_claim() {
        use base64::Engine as _;
        let claim_payload = serde_json::json!({
            "https://api.openai.com/auth/chatgpt_account_id": "acct-from-jwt"
        });
        let payload_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claim_payload.to_string());
        let token = format!("header.{payload_b64}.signature");

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("auth.json");
        fs::write(
            &path,
            format!(r#"{{"tokens": {{"access_token": "{token}", "refresh_token": "r"}}}}"#),
        )
        .expect("write");

        let auth = read_auth(&path).expect("parses");
        assert_eq!(auth.account_id.as_deref(), Some("acct-from-jwt"));
    }

    /// Builds a JWT-shaped string carrying exactly the claims given, in the
    /// `header.payload.signature` form this source's decoder expects.
    fn jwt_with_claims(claims: serde_json::Value) -> String {
        use base64::Engine as _;
        let payload_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        format!("header.{payload_b64}.signature")
    }

    #[test]
    fn read_auth_decodes_the_plan_claim_off_the_access_token() {
        let token = jwt_with_claims(serde_json::json!({
            "https://api.openai.com/auth/chatgpt_plan_type": "pro"
        }));
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("auth.json");
        fs::write(
            &path,
            format!(r#"{{"tokens": {{"access_token": "{token}", "refresh_token": "r"}}}}"#),
        )
        .expect("write");

        let auth = read_auth(&path).expect("parses");
        assert_eq!(auth.plan_claim.as_deref(), Some("pro"));
    }

    #[test]
    fn a_snapshots_plan_falls_back_to_the_jwt_claim_when_the_usage_body_has_none() {
        const WHAM_BODY_WITHOUT_PLAN: &str = r#"{"rate_limit": {
          "primary_window": {"used_percent": 20, "limit_window_seconds": 18000},
          "secondary_window": {"used_percent": 55, "limit_window_seconds": 604800}
        }}"#;

        struct WorksFirstTry;
        impl CodexTransport for WorksFirstTry {
            fn usage(&self, _: &str, _: Option<&str>) -> Result<String, ProviderUsageError> {
                Ok(WHAM_BODY_WITHOUT_PLAN.to_string())
            }
            fn refresh(&self, _: &str) -> Result<String, ProviderUsageError> {
                unreachable!("a successful first attempt never refreshes")
            }
        }

        let token = jwt_with_claims(serde_json::json!({
            "https://api.openai.com/auth/chatgpt_plan_type": "team"
        }));
        let mut auth = auth("refresh-a");
        auth.access_token = token;
        auth.plan_claim = Some("team".into());

        let cache = Mutex::new(None);
        let snapshot = fetch_direct(&WorksFirstTry, &cache, &auth, now()).expect("ok");
        assert_eq!(snapshot.plan.as_deref(), Some("team"));
    }

    #[test]
    fn a_live_pi_entry_uses_one_usage_call_without_refresh() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pi_path = dir.path().join("pi-auth.json");
        fs::write(
            &pi_path,
            r#"{"openai-codex":{"type":"oauth","access":"pi-access","refresh":"pi-refresh","expires":9223372036854775807,"accountId":"synthetic-account"}}"#,
        )
        .expect("write");
        struct PiOnly(Arc<AtomicUsize>);
        impl CodexTransport for PiOnly {
            fn usage(
                &self,
                token: &str,
                account: Option<&str>,
            ) -> Result<String, ProviderUsageError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                if token == "pi-access" && account == Some("synthetic-account") {
                    Ok(WHAM_BODY.to_owned())
                } else {
                    Err(ProviderUsageError::Authentication)
                }
            }
            fn refresh(&self, _: &str) -> Result<String, ProviderUsageError> {
                unreachable!("Pi owns token refresh")
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let source =
            CodexDirectFetch::with_paths(None, Some(pi_path), Box::new(PiOnly(Arc::clone(&calls))));
        let outcome = source.fetch(TEST_MAX_AGE);
        assert_eq!(outcome.error, None);
        assert_eq!(outcome.snapshots.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            outcome.snapshots[0].account.as_deref(),
            Some("synthetic-account")
        );
    }

    #[test]
    fn an_expired_pi_entry_is_absent_without_network_or_refresh() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pi_path = dir.path().join("pi-auth.json");
        fs::write(
            &pi_path,
            r#"{"openai-codex":{"type":"oauth","access":"pi-access","refresh":"pi-refresh","expires":1}}"#,
        )
        .expect("write");
        struct Never;
        impl CodexTransport for Never {
            fn usage(&self, _: &str, _: Option<&str>) -> Result<String, ProviderUsageError> {
                unreachable!("expired Pi credentials are absent")
            }
            fn refresh(&self, _: &str) -> Result<String, ProviderUsageError> {
                unreachable!("Pi credentials are never refreshed")
            }
        }
        let source = CodexDirectFetch::with_paths(None, Some(pi_path), Box::new(Never));
        let outcome = source.fetch(TEST_MAX_AGE);
        assert!(outcome.snapshots.is_empty());
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn the_source_declares_itself_online_so_the_gate_can_find_it() {
        assert!(CodexDirectFetch::new().requires_online_opt_in());
    }
}
