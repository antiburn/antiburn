// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
    Confidence, Freshness, ProviderUsageError, ProviderUsageSnapshot, SchemaReason, SupportTier,
    UsageSource,
};
use crate::provider_usage::live::{LiveUsageSource, SourceOutcome};

use super::codex_app_server;
use super::cooldown::Cooldown;
use super::http;

/// `auth.json` is a small, purpose-built token store — cap the read
/// defensively rather than trust that.
const MAX_CREDENTIAL_BYTES: u64 = 256 * 1024;

const WHAM_USAGE_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";
const TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";

/// Codex CLI's own public OAuth client id. Public in the sense every install
/// of the CLI carries it; it identifies the client application to the
/// authorization server, not the reader.
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// The unsigned JWT claim that names the account, when `auth.json` did not
/// state one directly.
const ACCOUNT_CLAIM: &str = "https://api.openai.com/auth/chatgpt_account_id";

/// The credential file, at the one documented place it lives.
pub fn default_auth_path() -> Option<PathBuf> {
    let dir = antiburn_local::paths::non_empty_env_path("CODEX_HOME")
        .or_else(|| antiburn_local::paths::home_dir().map(|home| home.join(".codex")))?;
    Some(dir.join("auth.json"))
}

/// What this source needs out of the CLI's own `auth.json`.
struct CodexAuth {
    access_token: String,
    refresh_token: String,
    account_id: Option<String>,
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
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(|| decode_jwt_claim(&access_token, ACCOUNT_CLAIM))
        .or_else(|| {
            tokens
                .get("id_token")
                .and_then(Value::as_str)
                .and_then(|token| decode_jwt_claim(token, ACCOUNT_CLAIM))
        });
    Some(CodexAuth {
        access_token,
        refresh_token,
        account_id,
    })
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
    transport: Box<dyn CodexTransport>,
    cached_refresh: Mutex<Option<RefreshedToken>>,
    cooldown: Cooldown,
}

impl CodexDirectFetch {
    pub fn new() -> CodexDirectFetch {
        CodexDirectFetch {
            auth_path: default_auth_path(),
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
            transport: Box::new(LiveCodexTransport),
            cached_refresh: Mutex::new(None),
            cooldown: Cooldown::new(),
        }
    }

    #[cfg(test)]
    fn with_transport(path: PathBuf, transport: Box<dyn CodexTransport>) -> CodexDirectFetch {
        CodexDirectFetch {
            auth_path: Some(path),
            transport,
            cached_refresh: Mutex::new(None),
            cooldown: Cooldown::new(),
        }
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

    fn requires_online_opt_in(&self) -> bool {
        true
    }

    fn fetch(&self) -> SourceOutcome {
        let now = OffsetDateTime::now_utc();
        let Some(path) = &self.auth_path else {
            return SourceOutcome::absent();
        };
        // Read inside the cooldown gate, mirroring the Claude source: a tick
        // the cooldown is going to skip should not touch the disk either.
        self.cooldown.poll(now, || {
            let Some(auth) = read_auth(path) else {
                return Ok(None);
            };
            match fetch_direct(self.transport.as_ref(), &self.cached_refresh, &auth, now) {
                Ok(snapshot) => Ok(Some(snapshot)),
                Err(direct_error) => match codex_app_server::fetch(now) {
                    Ok(snapshot) => Ok(snapshot),
                    Err(fallback_error) => Err(preferred_error(direct_error, fallback_error)),
                },
            }
        })
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
        return build_snapshot(&body, auth.account_id.clone(), now);
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
    build_snapshot(&body, auth.account_id.clone(), now)
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
    now: OffsetDateTime,
) -> Result<ProviderUsageSnapshot, ProviderUsageError> {
    let usage = codex::parse_wham_usage(body, now)?;
    Ok(ProviderUsageSnapshot {
        provider: crate::provider_usage::providers::OPENAI,
        account: account_id,
        plan: usage.plan,
        observed_at: now,
        source: UsageSource {
            id: super::CODEX_SOURCE_ID,
            label: "Asked Codex directly".into(),
            confidence: Confidence::High,
            // Recomputed on every read by `Cooldown::poll`.
            freshness: Freshness::Fresh,
        },
        support: SupportTier::Live,
        windows: usage.windows,
        supplemental: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const NOW: i64 = 1_800_000_000;
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

    #[test]
    fn a_missing_credential_file_is_absent_not_an_error() {
        let source = CodexDirectFetch::at(PathBuf::from("/nonexistent/auth.json"));
        let outcome = source.fetch();
        assert!(outcome.snapshots.is_empty());
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn an_unparseable_auth_file_reads_as_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("auth.json");
        fs::write(&path, "not json").expect("write");
        let outcome = CodexDirectFetch::at(path).fetch();
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
        let outcome = source.fetch();
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

    #[test]
    fn the_source_declares_itself_online_so_the_gate_can_find_it() {
        assert!(CodexDirectFetch::new().requires_online_opt_in());
    }
}
