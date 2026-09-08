//! Ask Codex directly for the reader's own plan usage, delegating an expired
//! token to the Codex CLI's own refresh, and falling back to the Codex app's
//! own process ([`super::codex_app_server`]) if a direct request never
//! succeeds.
//!
//! # The credential
//!
//! The Codex CLI keeps its tokens in `$CODEX_HOME/auth.json` when that
//! variable is set, and `~/.codex/auth.json` otherwise. That file belongs to
//! the CLI, end to end: this source reads it — exactly one in-memory
//! snapshot per fetch attempt, never re-read mid-attempt — and never writes
//! it. A missing or unparseable file is not an error, for the same reason it
//! is not one for Claude: it is the ordinary state of a machine that has
//! never signed in with Codex.
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
//! # The refresh lifecycle belongs to the CLI
//!
//! This source never redeems `auth.json`'s refresh token. The provider
//! rotates the refresh token on every redemption, so an in-process
//! redemption would either discard the rotated token — stranding the CLI
//! with a stale one — or mean writing a credential store this application
//! does not own, racing the CLI's own writes. When the provider rejects the
//! access token as expired, this source instead asks the CLI to run its own
//! refresh: it spawns `codex app-server` with `"refreshToken": true` — see
//! [`super::codex_app_server::trigger_refresh`] — and lets the CLI refresh
//! and persist its own `auth.json`.
//!
//! The RPC response is only a hint that the refresh ran; the proof is the
//! file. This source fingerprints `auth.json`'s content before the spawn,
//! then polls the file until it is *quiescent* — changed from the pre-spawn
//! fingerprint and stable for [`RefreshWait::stable_for`] — before
//! re-reading it through the normal parser and retrying the usage call
//! exactly once. That re-read is the one sanctioned second read of a fetch
//! attempt.
//!
//! Only an expired-credential rejection (HTTP 401) triggers this recovery.
//! A network failure, a 5xx, a 403, or a rate limit does not: a fresh token
//! would not change any of those answers — see [`UsageCallError`]. Before
//! spawning, this source probes that a `codex` executable exists at all; an
//! expired token with no CLI to refresh it is an authentication problem,
//! not an availability one. A refresh the CLI itself rejects is terminal
//! the same way: the reader has to run `codex` and sign in again. A file
//! that never changes or never settles inside [`RefreshWait::deadline`] is
//! only transient. The whole recovery runs inside the same [`Cooldown`]
//! gate as everything else here, so a terminally broken login cannot
//! respawn the CLI on every poll.
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
//! [`CodexTransport`] and [`CodexCli`] are the seams: [`fetch_direct`] is a
//! free function over the two traits, exercised in tests with fakes that
//! never open a socket or spawn a process, so the recovery logic above is
//! covered without a mockable network at the process level.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;
use time::OffsetDateTime;

use crate::provider_usage::live::codex;
use crate::provider_usage::live::model::{
    Confidence, Freshness, ProviderUsageError, ProviderUsageSnapshot, UsageSource,
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
    // The refresh token is never read — the CLI owns it, see the module doc
    // — but its presence is part of recognizing the CLI's own file shape.
    tokens.get("refresh_token")?.as_str()?;
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

/// A failed usage call, split where the recovery rule needs it split: the
/// one outcome that may trigger CLI recovery, and everything else.
///
/// The shared [`http::status_error`] mapping folds 401 and 403 into one
/// `Authentication` category — right for reporting, too coarse for recovery:
/// a 403 is a refusal a fresh token would not change, so only a 401 becomes
/// [`UsageCallError::ExpiredCredential`].
#[derive(Debug, PartialEq)]
enum UsageCallError {
    /// The provider rejected the credential as expired or invalid (HTTP
    /// 401). The one shape CLI recovery may answer.
    ExpiredCredential,
    /// Every other failure, in the shared taxonomy.
    Other(ProviderUsageError),
}

impl UsageCallError {
    /// The shared-taxonomy verdict this failure reports when recovery is
    /// not (or no longer) an option.
    fn into_error(self) -> ProviderUsageError {
        match self {
            UsageCallError::ExpiredCredential => ProviderUsageError::Authentication,
            UsageCallError::Other(error) => error,
        }
    }
}

/// The network calls [`fetch_direct`] needs, as a trait so a test can supply
/// them without a socket.
trait CodexTransport: Send + Sync {
    fn usage(&self, access_token: &str, account_id: Option<&str>)
    -> Result<String, UsageCallError>;
}

/// The Codex CLI itself, as the recovery path sees it: a probe for whether
/// it is installed at all, and one way to ask it to refresh its own
/// credential. A trait so tests can fake both without a process.
trait CodexCli: Send + Sync {
    /// Whether a `codex` executable is present to spawn at all.
    fn is_installed(&self) -> bool;
    /// Ask the CLI to refresh and persist its own `auth.json`. The return
    /// value is a hint — see [`codex_app_server::trigger_refresh`].
    fn trigger_refresh(&self) -> codex_app_server::RefreshHint;
}

/// Timing for the post-spawn quiescence poll — see "The refresh lifecycle
/// belongs to the CLI" in the module doc. A struct so tests can shrink every
/// duration instead of sleeping through the live ones.
#[derive(Clone, Copy)]
struct RefreshWait {
    /// How long to sleep between fingerprint reads.
    poll_interval: Duration,
    /// How long a changed fingerprint must hold still before the file
    /// counts as quiescent.
    stable_for: Duration,
    /// The bound on the whole recovery, measured from just before the
    /// spawn. Sized so the CLI's own [`codex_app_server`] refresh timeout
    /// fits inside it with room for the file to settle.
    deadline: Duration,
}

impl RefreshWait {
    /// The live timings: a quarter-second poll, a 1.5s stable window, and a
    /// 12s overall deadline — inside the 10–15s the verification protocol
    /// allows, and behind the [`Cooldown`]'s one-minute failure floor.
    const LIVE: RefreshWait = RefreshWait {
        poll_interval: Duration::from_millis(250),
        stable_for: Duration::from_millis(1_500),
        deadline: Duration::from_secs(12),
    };

    /// Timings small enough for a test suite to wait out for real.
    #[cfg(test)]
    const FAST: RefreshWait = RefreshWait {
        poll_interval: Duration::from_millis(5),
        stable_for: Duration::from_millis(25),
        deadline: Duration::from_millis(500),
    };
}

struct LiveCodexTransport;

impl CodexTransport for LiveCodexTransport {
    fn usage(
        &self,
        access_token: &str,
        account_id: Option<&str>,
    ) -> Result<String, UsageCallError> {
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
            .map_err(|_| UsageCallError::Other(ProviderUsageError::Unavailable))?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(UsageCallError::ExpiredCredential);
        }
        if let Some(error) = http::status_error(response.status()) {
            return Err(UsageCallError::Other(error));
        }
        http::read_capped_body(response).map_err(UsageCallError::Other)
    }
}

struct LiveCodexCli;

impl CodexCli for LiveCodexCli {
    fn is_installed(&self) -> bool {
        codex_app_server::binary_present()
    }

    fn trigger_refresh(&self) -> codex_app_server::RefreshHint {
        codex_app_server::trigger_refresh()
    }
}

/// The default CLI seam for test constructors: reaching it at all means a
/// test triggered recovery it did not mean to.
#[cfg(test)]
struct NeverSpawns;

#[cfg(test)]
impl CodexCli for NeverSpawns {
    fn is_installed(&self) -> bool {
        unreachable!("this test never reaches CLI recovery")
    }

    fn trigger_refresh(&self) -> codex_app_server::RefreshHint {
        unreachable!("this test never reaches CLI recovery")
    }
}

/// Asks `GET /backend-api/wham/usage`, recovering an expired token through
/// the CLI's own refresh, then falling back to the app-server RPC if neither
/// attempt lands.
pub struct CodexDirectFetch {
    auth_path: Option<PathBuf>,
    pi_auth_path: Option<PathBuf>,
    /// The Codex CLI's session log root, for the seed read described in
    /// "Seeding from the session log" above. `None` skips that seed
    /// entirely — the state every test constructor below starts from.
    sessions_root: Option<PathBuf>,
    transport: Box<dyn CodexTransport>,
    cli: Box<dyn CodexCli>,
    wait: RefreshWait,
    cooldown: Cooldown,
}

impl CodexDirectFetch {
    pub fn new() -> CodexDirectFetch {
        CodexDirectFetch {
            auth_path: default_auth_path(),
            pi_auth_path: pi_auth::default_auth_path(),
            sessions_root: default_sessions_root(),
            transport: Box::new(LiveCodexTransport),
            cli: Box::new(LiveCodexCli),
            wait: RefreshWait::LIVE,
            cooldown: Cooldown::new(),
        }
    }

    /// A source rooted at an explicit path, for tests.
    #[cfg(test)]
    pub fn at(path: PathBuf) -> CodexDirectFetch {
        Self::with_paths(Some(path), None, Box::new(LiveCodexTransport))
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
            // Recovery is exercised through `fetch_direct` in tests; a
            // source-level test that reaches the CLI seam is a test bug.
            cli: Box::new(NeverSpawns),
            wait: RefreshWait::FAST,
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
            let direct_error = match (&auth, self.auth_path.as_deref()) {
                (Some(auth), Some(path)) => {
                    match fetch_direct(
                        self.transport.as_ref(),
                        self.cli.as_ref(),
                        path,
                        &self.wait,
                        auth,
                        now,
                    ) {
                        Ok(snapshot) => return Ok(Some(snapshot)),
                        Err(error) => Some(error),
                    }
                }
                _ => None,
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

/// Try Pi's access token once. Pi owns refresh, so this path never refreshes
/// — not in-process, and not through the Codex CLI either: an expired Pi
/// credential folds straight into the shared error taxonomy.
fn fetch_pi(
    transport: &dyn CodexTransport,
    entry: &pi_auth::PiOauth,
    now: OffsetDateTime,
) -> Result<ProviderUsageSnapshot, ProviderUsageError> {
    let body = transport
        .usage(&entry.access_token, entry.account_id.as_deref())
        .map_err(UsageCallError::into_error)?;
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

/// One attempt, with the single CLI-delegated recovery this source allows —
/// see "The refresh lifecycle belongs to the CLI" in the module doc. A free
/// function over [`CodexTransport`] and [`CodexCli`] rather than a method,
/// so a test can call it with fakes and nothing else this source owns.
fn fetch_direct(
    transport: &dyn CodexTransport,
    cli: &dyn CodexCli,
    auth_path: &Path,
    wait: &RefreshWait,
    auth: &CodexAuth,
    now: OffsetDateTime,
) -> Result<ProviderUsageSnapshot, ProviderUsageError> {
    match transport.usage(&auth.access_token, auth.account_id.as_deref()) {
        Ok(body) => {
            return build_snapshot(&body, auth.account_id.clone(), auth.plan_claim.clone(), now);
        }
        // The one failure CLI recovery may answer. Everything else —
        // network, 5xx, 403, rate limit — returns as-is: a fresh token
        // would not change those answers, and spawning the CLI for them
        // would burn a process on a problem it cannot fix.
        Err(UsageCallError::ExpiredCredential) => {}
        Err(other) => return Err(other.into_error()),
    }

    let recovered = recover_expired(cli, auth_path, wait)?;
    let body = transport
        .usage(&recovered.access_token, recovered.account_id.as_deref())
        .map_err(UsageCallError::into_error)?;
    build_snapshot(
        &body,
        recovered.account_id.clone(),
        recovered.plan_claim.clone(),
        now,
    )
}

/// Ask the CLI to refresh its own `auth.json`, verify against the file that
/// it did, and re-read the result — the recovery half of [`fetch_direct`].
///
/// [`ProviderUsageError::Authentication`] here is terminal: no CLI to spawn,
/// or a CLI that rejected its own refresh — either way the reader has to run
/// `codex` and sign in again. [`ProviderUsageError::Unavailable`] is
/// transient: the file never changed or never settled inside the deadline,
/// and the next off-cooldown poll may do better.
fn recover_expired(
    cli: &dyn CodexCli,
    auth_path: &Path,
    wait: &RefreshWait,
) -> Result<CodexAuth, ProviderUsageError> {
    if !cli.is_installed() {
        return Err(ProviderUsageError::Authentication);
    }
    let before = fingerprint(auth_path);
    let deadline = Instant::now() + wait.deadline;
    match cli.trigger_refresh() {
        codex_app_server::RefreshHint::Rejected => return Err(ProviderUsageError::Authentication),
        // A hint either way; the file poll below is the real verdict.
        codex_app_server::RefreshHint::Answered | codex_app_server::RefreshHint::Unknown => {}
    }
    wait_for_quiescent_change(auth_path, before, deadline, wait)?;
    // The CLI wrote something this parser cannot read: transient, the same
    // category as "never settled" — a later poll sees whatever it writes
    // next.
    read_auth(auth_path).ok_or(ProviderUsageError::Unavailable)
}

/// Poll `auth.json` until it is quiescent: its fingerprint has changed from
/// `before` *and* held still for [`RefreshWait::stable_for`]. The CLI may
/// write the file more than once around its RPC response, so one changed
/// read is not enough to trust.
fn wait_for_quiescent_change(
    auth_path: &Path,
    before: Option<u64>,
    deadline: Instant,
    wait: &RefreshWait,
) -> Result<(), ProviderUsageError> {
    let mut observed: Option<(Option<u64>, Instant)> = None;
    loop {
        let read_at = Instant::now();
        let current = fingerprint(auth_path);
        match observed {
            Some((fingerprint, since)) if fingerprint == current => {
                if current.is_some() && current != before && read_at - since >= wait.stable_for {
                    return Ok(());
                }
            }
            _ => observed = Some((current, read_at)),
        }
        if Instant::now() >= deadline {
            return Err(ProviderUsageError::Unavailable);
        }
        std::thread::sleep(wait.poll_interval);
    }
}

/// A content fingerprint of `auth.json`: a hash of its bytes, `None` when
/// the file is missing, unreadable, or over the credential size cap.
///
/// Content, not mtime alone: the CLI may rewrite the file faster than the
/// filesystem's timestamp granularity can distinguish.
fn fingerprint(path: &Path) -> Option<u64> {
    use std::hash::{Hash as _, Hasher as _};
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > MAX_CREDENTIAL_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some(hasher.finish())
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

    fn auth() -> CodexAuth {
        CodexAuth {
            access_token: "stale-token".into(),
            account_id: Some("acct-123".into()),
            plan_claim: None,
        }
    }

    /// The `auth.json` shape [`read_auth`] parses, carrying `access_token`.
    fn auth_json(access_token: &str) -> String {
        format!(
            r#"{{"tokens": {{"access_token": "{access_token}", "refresh_token": "r", "account_id": "acct-1"}}}}"#
        )
    }

    /// A transport that answers only `fresh-token`, rejecting every other
    /// access token as expired — the shape of a real 401 on a stale token.
    struct AnswersFreshTokenOnly {
        usage_calls: AtomicUsize,
    }

    impl AnswersFreshTokenOnly {
        fn new() -> AnswersFreshTokenOnly {
            AnswersFreshTokenOnly {
                usage_calls: AtomicUsize::new(0),
            }
        }
    }

    impl CodexTransport for AnswersFreshTokenOnly {
        fn usage(
            &self,
            access_token: &str,
            _account_id: Option<&str>,
        ) -> Result<String, UsageCallError> {
            self.usage_calls.fetch_add(1, Ordering::SeqCst);
            if access_token == "fresh-token" {
                Ok(WHAM_BODY.to_string())
            } else {
                Err(UsageCallError::ExpiredCredential)
            }
        }
    }

    /// A CLI fake that, when asked to refresh, rewrites `auth.json` with
    /// `fresh-token` — the observable effect a real CLI refresh has.
    struct WritesFreshAuth {
        auth_path: PathBuf,
        trigger_calls: AtomicUsize,
    }

    impl CodexCli for WritesFreshAuth {
        fn is_installed(&self) -> bool {
            true
        }
        fn trigger_refresh(&self) -> codex_app_server::RefreshHint {
            self.trigger_calls.fetch_add(1, Ordering::SeqCst);
            fs::write(&self.auth_path, auth_json("fresh-token")).expect("write refreshed auth");
            codex_app_server::RefreshHint::Answered
        }
    }

    /// A CLI fake that answers the RPC but never touches the file — the
    /// shape of a refresh whose write never lands.
    struct NeverWrites;
    impl CodexCli for NeverWrites {
        fn is_installed(&self) -> bool {
            true
        }
        fn trigger_refresh(&self) -> codex_app_server::RefreshHint {
            codex_app_server::RefreshHint::Answered
        }
    }

    /// A CLI fake for a machine without the `codex` binary at all.
    struct NotInstalled {
        trigger_calls: AtomicUsize,
    }
    impl CodexCli for NotInstalled {
        fn is_installed(&self) -> bool {
            false
        }
        fn trigger_refresh(&self) -> codex_app_server::RefreshHint {
            self.trigger_calls.fetch_add(1, Ordering::SeqCst);
            codex_app_server::RefreshHint::Unknown
        }
    }

    /// A CLI fake whose own refresh reports failure — a broken login.
    struct RejectsRefresh;
    impl CodexCli for RejectsRefresh {
        fn is_installed(&self) -> bool {
            true
        }
        fn trigger_refresh(&self) -> codex_app_server::RefreshHint {
            codex_app_server::RefreshHint::Rejected
        }
    }

    #[test]
    fn an_expired_token_recovers_through_the_cli_and_retries_exactly_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let auth_path = dir.path().join("auth.json");
        fs::write(&auth_path, auth_json("stale-token")).expect("write");
        let transport = AnswersFreshTokenOnly::new();
        let cli = WritesFreshAuth {
            auth_path: auth_path.clone(),
            trigger_calls: AtomicUsize::new(0),
        };

        let auth = read_auth(&auth_path).expect("parses");
        let snapshot = fetch_direct(
            &transport,
            &cli,
            &auth_path,
            &RefreshWait::FAST,
            &auth,
            now(),
        )
        .expect("recovers");

        assert_eq!(cli.trigger_calls.load(Ordering::SeqCst), 1);
        assert_eq!(transport.usage_calls.load(Ordering::SeqCst), 2);
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.plan.as_deref(), Some("plus"));
    }

    #[test]
    fn a_file_that_never_changes_times_out_as_transient_not_terminal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let auth_path = dir.path().join("auth.json");
        fs::write(&auth_path, auth_json("stale-token")).expect("write");
        let transport = AnswersFreshTokenOnly::new();

        let auth = read_auth(&auth_path).expect("parses");
        let result = fetch_direct(
            &transport,
            &NeverWrites,
            &auth_path,
            &RefreshWait::FAST,
            &auth,
            now(),
        );

        assert_eq!(result, Err(ProviderUsageError::Unavailable));
        // The one usage call is the expired first attempt; a retry with the
        // same stale token would only burn a request.
        assert_eq!(transport.usage_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_expired_token_without_a_codex_binary_is_an_authentication_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let auth_path = dir.path().join("auth.json");
        fs::write(&auth_path, auth_json("stale-token")).expect("write");
        let transport = AnswersFreshTokenOnly::new();
        let cli = NotInstalled {
            trigger_calls: AtomicUsize::new(0),
        };

        let auth = read_auth(&auth_path).expect("parses");
        let result = fetch_direct(
            &transport,
            &cli,
            &auth_path,
            &RefreshWait::FAST,
            &auth,
            now(),
        );

        // Not `Unavailable`: nothing was unreachable, the reader has to run
        // `codex` and sign in again.
        assert_eq!(result, Err(ProviderUsageError::Authentication));
        assert_eq!(cli.trigger_calls.load(Ordering::SeqCst), 0);
        assert_eq!(transport.usage_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_refresh_the_cli_rejects_is_terminal_without_waiting_on_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let auth_path = dir.path().join("auth.json");
        fs::write(&auth_path, auth_json("stale-token")).expect("write");
        let transport = AnswersFreshTokenOnly::new();

        let auth = read_auth(&auth_path).expect("parses");
        let started = std::time::Instant::now();
        let result = fetch_direct(
            &transport,
            &RejectsRefresh,
            &auth_path,
            &RefreshWait::FAST,
            &auth,
            now(),
        );

        assert_eq!(result, Err(ProviderUsageError::Authentication));
        // Terminal means no quiescence poll: well inside `FAST.deadline`.
        assert!(started.elapsed() < RefreshWait::FAST.deadline);
        assert_eq!(transport.usage_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_non_expired_failure_never_touches_the_cli() {
        struct RateLimits;
        impl CodexTransport for RateLimits {
            fn usage(&self, _: &str, _: Option<&str>) -> Result<String, UsageCallError> {
                Err(UsageCallError::Other(ProviderUsageError::RateLimited))
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let auth_path = dir.path().join("auth.json");
        fs::write(&auth_path, auth_json("stale-token")).expect("write");

        let auth = read_auth(&auth_path).expect("parses");
        // `NeverSpawns` panics on any CLI call, so passing means no spawn.
        let result = fetch_direct(
            &RateLimits,
            &NeverSpawns,
            &auth_path,
            &RefreshWait::FAST,
            &auth,
            now(),
        );
        assert_eq!(result, Err(ProviderUsageError::RateLimited));
    }

    struct AlwaysFails;
    impl CodexTransport for AlwaysFails {
        fn usage(&self, _: &str, _: Option<&str>) -> Result<String, UsageCallError> {
            Err(UsageCallError::Other(ProviderUsageError::RateLimited))
        }
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
            fn usage(&self, _: &str, _: Option<&str>) -> Result<String, UsageCallError> {
                Ok(WHAM_BODY.to_string())
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
            fn usage(&self, _: &str, _: Option<&str>) -> Result<String, UsageCallError> {
                Ok(WHAM_BODY_WITHOUT_PLAN.to_string())
            }
        }

        let token = jwt_with_claims(serde_json::json!({
            "https://api.openai.com/auth/chatgpt_plan_type": "team"
        }));
        let mut auth = auth();
        auth.access_token = token;
        auth.plan_claim = Some("team".into());

        let snapshot = fetch_direct(
            &WorksFirstTry,
            &NeverSpawns,
            Path::new("/nonexistent/auth.json"),
            &RefreshWait::FAST,
            &auth,
            now(),
        )
        .expect("ok");
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
            fn usage(&self, token: &str, account: Option<&str>) -> Result<String, UsageCallError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                if token == "pi-access" && account == Some("synthetic-account") {
                    Ok(WHAM_BODY.to_owned())
                } else {
                    // An expired verdict here must still never trigger CLI
                    // recovery on the Pi path: `fetch_pi` folds it into the
                    // shared taxonomy, and the `NeverSpawns` CLI seam every
                    // test constructor installs panics on any spawn.
                    Err(UsageCallError::ExpiredCredential)
                }
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
            fn usage(&self, _: &str, _: Option<&str>) -> Result<String, UsageCallError> {
                unreachable!("expired Pi credentials are absent")
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
