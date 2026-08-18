// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Ask Claude directly for the reader's own plan usage.
//!
//! # The credential
//!
//! The Claude CLI keeps its OAuth tokens in `$CLAUDE_CONFIG_DIR/.credentials.json`
//! when that variable is set, and `~/.claude/.credentials.json` otherwise —
//! the same file and the same lifecycle it already owns for every other
//! request it makes. This source reads the access token out of it and calls
//! the provider's own usage endpoint with the reader's own credential, the
//! same request the CLI itself would make. No key of ours, no service of
//! ours in between.
//!
//! **This source never refreshes that token.** Refreshing is a lifecycle
//! decision — issuing a new credential under the reader's account — and it
//! belongs to the tool that owns the token's lifecycle, which is the CLI, not
//! a background reader of its credential file. If `expiresAt` has already
//! passed, that is reported as an authentication failure without a network
//! call: the fix is signing in again with the CLI, not a retry from here.
//!
//! A missing or unparseable credentials file is not an error — it is the
//! ordinary state of a machine where the CLI has never signed in, or keeps
//! its tokens somewhere this file format does not describe (the system
//! keychain, on some platforms). The source simply has nothing to report.
//!
//! # Retrying and going quiet
//!
//! Every other failure — a rejected credential, a rate limit, an
//! unreachable network, a response this build cannot parse — goes through
//! [`Cooldown`], which is what keeps this source from opening a connection on
//! every five-minute scheduler tick: see that module for the retry and
//! last-good-reading contract every direct-fetch source shares.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use time::OffsetDateTime;

use crate::provider_usage::live::anthropic;
use crate::provider_usage::live::model::{
    Confidence, Freshness, ProviderUsageError, ProviderUsageSnapshot, SchemaReason, SupportTier,
    UsageSource,
};
use crate::provider_usage::live::{LiveUsageSource, SourceOutcome};

use super::cooldown::Cooldown;
use super::http;

/// The credentials file is a small, purpose-built OAuth token store, not a
/// general state file — cap the read defensively rather than trust that.
const MAX_CREDENTIAL_BYTES: u64 = 256 * 1024;

const USAGE_ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";

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
}

/// Read and parse the credentials file. `None` covers both "no file" and "a
/// file that is not this shape" — see the module doc for why neither is an
/// error here.
fn read_credentials(path: &Path) -> Option<ClaudeCredentials> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > MAX_CREDENTIAL_BYTES {
        return None;
    }
    let contents = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&contents).ok()?;
    let oauth = value.get("claudeAiOauth")?;
    Some(ClaudeCredentials {
        access_token: oauth.get("accessToken")?.as_str()?.to_owned(),
        expires_at_ms: oauth.get("expiresAt")?.as_i64()?,
        subscription_type: oauth
            .get("subscriptionType")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
    })
}

/// Asks `GET /api/oauth/usage` with the CLI's own access token.
pub struct ClaudeDirectFetch {
    credentials_path: Option<PathBuf>,
    cooldown: Cooldown,
}

impl ClaudeDirectFetch {
    pub fn new() -> ClaudeDirectFetch {
        ClaudeDirectFetch {
            credentials_path: default_credentials_path(),
            cooldown: Cooldown::new(),
        }
    }

    /// A source rooted at an explicit path, for tests.
    #[cfg(test)]
    pub fn at(path: PathBuf) -> ClaudeDirectFetch {
        ClaudeDirectFetch {
            credentials_path: Some(path),
            cooldown: Cooldown::new(),
        }
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

    /// This source makes a request of its own, on the reader's own account —
    /// exactly the traffic the online opt-in exists to gate.
    fn requires_online_opt_in(&self) -> bool {
        true
    }

    fn fetch(&self) -> SourceOutcome {
        let now = OffsetDateTime::now_utc();
        let Some(path) = &self.credentials_path else {
            return SourceOutcome::absent();
        };
        let Some(credentials) = read_credentials(path) else {
            return SourceOutcome::absent();
        };

        self.cooldown
            .poll(now, || fetch_live(&credentials, now).map(Some))
    }
}

fn fetch_live(
    credentials: &ClaudeCredentials,
    now: OffsetDateTime,
) -> Result<ProviderUsageSnapshot, ProviderUsageError> {
    let expires_at = OffsetDateTime::from_unix_timestamp(credentials.expires_at_ms / 1_000)
        .map_err(|_| ProviderUsageError::Schema(SchemaReason::InvalidValue))?;
    if expires_at <= now {
        return Err(ProviderUsageError::Authentication);
    }

    let response = http::client()
        .get(USAGE_ENDPOINT)
        .bearer_auth(&credentials.access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("anthropic-beta", "oauth-2025-04-20")
        .send()
        .map_err(|_| ProviderUsageError::Unavailable)?;

    if let Some(error) = http::status_error(response.status()) {
        return Err(error);
    }
    let body = http::read_capped_body(response)?;
    let usage = anthropic::parse_usage(&body)?;

    Ok(ProviderUsageSnapshot {
        provider: crate::provider_usage::providers::ANTHROPIC,
        // Not disclosed by this endpoint, and there is only ever one
        // credential to have asked with, so there is nothing to disambiguate
        // between.
        account: None,
        plan: credentials.subscription_type.clone(),
        observed_at: now,
        source: UsageSource {
            id: SOURCE_ID,
            label: "Asked Claude directly".into(),
            confidence: Confidence::High,
            // Recomputed on every read by `Cooldown::poll`; a snapshot this
            // function returns is always describing the instant it was built.
            freshness: Freshness::Fresh,
        },
        support: SupportTier::Live,
        windows: usage.windows,
        supplemental: usage.supplemental,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn credentials_file(expires_at_ms: i64, subscription_type: &str) -> String {
        format!(
            r#"{{"claudeAiOauth": {{"accessToken": "synthetic-token",
              "refreshToken": "synthetic-refresh", "expiresAt": {expires_at_ms},
              "subscriptionType": "{subscription_type}"}}}}"#
        )
    }

    #[test]
    fn a_missing_credentials_file_is_absent_not_an_error() {
        let source = ClaudeDirectFetch::at(PathBuf::from("/nonexistent/.credentials.json"));
        let outcome = source.fetch();
        assert!(outcome.snapshots.is_empty());
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn an_unparseable_credentials_file_reads_as_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(&path, "not json at all").expect("write");
        let outcome = ClaudeDirectFetch::at(path).fetch();
        assert!(outcome.snapshots.is_empty());
        assert_eq!(outcome.error, None);
    }

    #[test]
    fn credentials_missing_the_oauth_object_read_as_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(&path, r#"{"somethingElse": true}"#).expect("write");
        let outcome = ClaudeDirectFetch::at(path).fetch();
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
        };
        assert_eq!(
            fetch_live(&credentials, now),
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
        assert!(read_credentials(&path).is_none());
    }

    #[test]
    fn a_well_formed_file_yields_the_fields_this_source_uses_and_nothing_else() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".credentials.json");
        fs::write(&path, credentials_file(NOW * 1_000, "max")).expect("write");
        let credentials = read_credentials(&path).expect("parses");
        assert_eq!(credentials.access_token, "synthetic-token");
        assert_eq!(credentials.expires_at_ms, NOW * 1_000);
        assert_eq!(credentials.subscription_type.as_deref(), Some("max"));
    }
}
