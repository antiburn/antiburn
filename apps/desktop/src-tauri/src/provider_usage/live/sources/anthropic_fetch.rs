// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
    Confidence, Freshness, ProviderUsageError, ProviderUsageSnapshot, SchemaReason, UsageSource,
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

    /// The raw JSON `security` printed to stdout, or `None` for every way
    /// this can fail to produce one — the item does not exist, `security`
    /// is not on this machine, a nonzero exit, an empty answer, a timeout.
    /// All of it reads as "nothing to report from this carrier", exactly
    /// like a missing file: this carrier says nothing about whether the
    /// account itself has usage to report.
    pub fn read() -> Option<String> {
        let mut child = antiburn_local::platform::process::headless_std_command("security")
            .args(["find-generic-password", "-s", SERVICE_NAME, "-w"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let mut stdout = child.stdout.take()?;
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
                return None;
            }
        };
        let status = child.wait().ok()?;
        if !status.success() || bytes.is_empty() || bytes.len() > MAX_BYTES {
            return None;
        }
        String::from_utf8(bytes).ok()
    }
}

/// Asks `GET /api/oauth/usage` with the CLI's own access token.
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
    cooldown: Cooldown,
}

impl ClaudeDirectFetch {
    pub fn new() -> ClaudeDirectFetch {
        ClaudeDirectFetch {
            credentials_path: default_credentials_path(),
            #[cfg(target_os = "macos")]
            try_keychain: true,
            cooldown: Cooldown::new(),
        }
    }

    /// A source rooted at an explicit path, with the Keychain carrier
    /// disabled — see the `try_keychain` field doc for why tests need that.
    #[cfg(test)]
    pub fn at(path: PathBuf) -> ClaudeDirectFetch {
        ClaudeDirectFetch {
            credentials_path: Some(path),
            #[cfg(target_os = "macos")]
            try_keychain: false,
            cooldown: Cooldown::new(),
        }
    }

    /// The credential from whichever carrier answers first: the Keychain,
    /// where this source is built to try it, then the credentials file.
    fn read_credentials(&self) -> Option<ClaudeCredentials> {
        #[cfg(target_os = "macos")]
        if self.try_keychain
            && let Some(credentials) = macos_keychain::read()
                .as_deref()
                .and_then(parse_credentials_json)
        {
            return Some(credentials);
        }
        read_credentials_file(self.credentials_path.as_deref()?)
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
        // The credential read sits inside the cooldown gate on purpose: on
        // macOS it spawns a `security` subprocess, and a scheduler tick that
        // the cooldown is going to skip anyway should not pay for one — nor
        // re-raise a Keychain access prompt the reader has already seen.
        self.cooldown.poll(now, || {
            let Some(credentials) = self.read_credentials() else {
                return Ok(None);
            };
            fetch_live(&credentials, now).map(Some)
        })
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
    }

    /// The Keychain and the file carrier hold the identical JSON shape, so
    /// this exercises `parse_credentials_json` directly against a synthetic
    /// value shaped exactly like what `security find-generic-password -w`
    /// prints — including the fields this source never reads
    /// (`refreshTokenExpiresAt`, `scopes`, `rateLimitTier`), to confirm they
    /// are ignored rather than tripping the parser.
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
    }

    #[test]
    fn unparseable_keychain_shaped_text_reads_as_absent() {
        assert!(parse_credentials_json("not json at all").is_none());
        assert!(parse_credentials_json(r#"{"somethingElse": true}"#).is_none());
    }
}
