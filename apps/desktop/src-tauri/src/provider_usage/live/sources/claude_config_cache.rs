//! Reads the Claude CLI's own cached usage reading off disk.
//!
//! The Claude CLI (Claude Code) caches the last reading it fetched from the
//! very endpoint [`super::anthropic_fetch`] calls, in its global config file.
//! That endpoint returns HTTP 429 intermittently, and the CLI and antiburn
//! draw from one per-account bucket. Reading the CLI's cached copy avoids a
//! request when the cache is fresh enough, and gives a real reading to show
//! when the endpoint says 429. See [`super::anthropic_fetch`]'s "The CLI's
//! own cache" section for how the two-tier fetch uses this.
//!
//! # Where the cache lives
//!
//! `$CLAUDE_CONFIG_DIR/.claude.json` when that variable is set and
//! non-empty, otherwise `~/.claude.json`. This sits next to `~/.claude/`, not
//! inside it — a different file from the credentials
//! [`super::anthropic_fetch`] reads.
//!
//! # What this reads out of it
//!
//! The file is a large JSON object with many keys this module has no use
//! for. Only two matter: `oauthAccount.accountUuid`, the signed-in account,
//! and `cachedUsageUtilization`, the cached reading itself —
//! `fetchedAtMs`, its own `accountUuid`, and a `utilization` object that is
//! byte-for-byte the body the usage endpoint returns, so
//! [`anthropic::parse_usage_value`] already understands it. Nothing else in
//! the file is read, and no other key of `cachedUsageUtilization` or
//! `oauthAccount` is read either — in particular never `emailAddress`.
//!
//! # Every failure reads as "no cache", never as an error
//!
//! A missing file, an unreadable one, one that is not JSON, one missing
//! `cachedUsageUtilization`, a missing or invalid `fetchedAtMs`, a missing
//! `accountUuid` on either side, a `cachedUsageUtilization.accountUuid` that
//! does not match `oauthAccount.accountUuid`, or a `utilization` object
//! [`anthropic::parse_usage_value`] rejects — every one of these yields
//! [`None`], nothing logged. The cache is an optimisation on top of the
//! endpoint, which stays the source of truth; a reader who has never opened
//! the CLI, or whose cache is stale or mid-write, is not an error case.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use time::OffsetDateTime;

use crate::provider_usage::live::anthropic::{self, AnthropicUsage};

/// `.claude.json` is a general CLI state file, not a purpose-built store —
/// it grows with use, so the read is capped defensively rather than trusted.
/// It is ordinarily under 1 MB.
const MAX_CONFIG_BYTES: u64 = 16 * 1024 * 1024;

/// The CLI's own cached usage reading, and the account it belongs to.
#[derive(Debug, Clone)]
pub struct CachedUsage {
    /// The moment the CLI received this reading — `cachedUsageUtilization`'s
    /// own `fetchedAtMs`, not when this module read the file.
    pub observed_at: OffsetDateTime,
    /// `cachedUsageUtilization.accountUuid`, already confirmed to match
    /// `oauthAccount.accountUuid`.
    pub account: String,
    pub usage: AnthropicUsage,
}

/// The config file, at the one documented place it lives.
pub fn default_config_path() -> Option<PathBuf> {
    let dir = antiburn_local::paths::non_empty_env_path("CLAUDE_CONFIG_DIR")
        .or_else(antiburn_local::paths::home_dir)?;
    Some(dir.join(".claude.json"))
}

/// Read and parse the CLI's cached usage reading from `path`. See the module
/// doc for why every failure yields [`None`] rather than an error.
pub fn read_cached_usage(path: &Path) -> Option<CachedUsage> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > MAX_CONFIG_BYTES {
        return None;
    }
    let contents = fs::read_to_string(path).ok()?;
    parse_cached_usage(&contents)
}

/// Parse `.claude.json`'s contents. A free function separate from
/// [`read_cached_usage`] so a test can hand it a string directly.
fn parse_cached_usage(contents: &str) -> Option<CachedUsage> {
    let document: Value = serde_json::from_str(contents).ok()?;
    let oauth_account_uuid = document.pointer("/oauthAccount/accountUuid")?.as_str()?;
    let cached = document.get("cachedUsageUtilization")?;
    let account_uuid = cached.get("accountUuid")?.as_str()?;
    if account_uuid != oauth_account_uuid {
        return None;
    }
    let fetched_at_ms = cached.get("fetchedAtMs")?.as_i64()?;
    let observed_at = OffsetDateTime::from_unix_timestamp(fetched_at_ms / 1_000).ok()?;
    let usage = anthropic::parse_usage_value(cached.get("utilization")?).ok()?;

    Some(CachedUsage {
        observed_at,
        account: account_uuid.to_owned(),
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A well-formed `.claude.json`, shaped like the real file: the two keys
    /// this module reads, plus unrelated top-level and nested keys it must
    /// ignore.
    fn well_formed_document(account_uuid: &str, cached_account_uuid: &str) -> String {
        format!(
            r#"{{
              "numStartups": 42,
              "oauthAccount": {{
                "accountUuid": "{account_uuid}",
                "emailAddress": "reader@example.test",
                "organizationUuid": "org-0000"
              }},
              "cachedUsageUtilization": {{
                "fetchedAtMs": 1788419487009,
                "accountUuid": "{cached_account_uuid}",
                "utilization": {{
                  "seven_day_opus": null,
                  "seven_day_sonnet": null,
                  "extra_usage": {{
                    "is_enabled": false,
                    "monthly_limit": null,
                    "used_credits": null,
                    "utilization": null,
                    "currency": null
                  }},
                  "limits": [
                    {{"kind": "session", "percent": 54,
                      "resets_at": "2026-09-03T10:29:59.945982+00:00",
                      "scope": null, "is_active": true}},
                    {{"kind": "weekly_all", "percent": 12,
                      "resets_at": "2026-09-07T00:59:59+00:00",
                      "scope": null, "is_active": true}},
                    {{"kind": "weekly_scoped", "percent": 10,
                      "resets_at": "2026-09-07T00:59:59+00:00",
                      "scope": {{"model": {{"display_name": "Fable"}}}},
                      "is_active": true}}
                  ],
                  "spend": {{"totalCents": 500}}
                }}
              }}
            }}"#
        )
    }

    #[test]
    fn a_well_formed_file_yields_the_cached_reading() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".claude.json");
        fs::write(
            &path,
            well_formed_document(
                "8a4d1c2e-0000-4000-8000-000000000001",
                "8a4d1c2e-0000-4000-8000-000000000001",
            ),
        )
        .expect("write");

        let cached = read_cached_usage(&path).expect("parses");
        assert_eq!(
            cached.observed_at,
            OffsetDateTime::from_unix_timestamp(1_788_419_487).unwrap()
        );
        assert_eq!(cached.account, "8a4d1c2e-0000-4000-8000-000000000001");
        let ids: Vec<&str> = cached
            .usage
            .windows
            .iter()
            .map(|window| window.id.as_str())
            .collect();
        assert_eq!(ids, vec!["five-hour", "seven-day", "weekly-fable"]);
        assert!(!cached.usage.supplemental.expect("supplemental").enabled);
    }

    #[test]
    fn a_mismatched_account_uuid_yields_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".claude.json");
        fs::write(
            &path,
            well_formed_document(
                "8a4d1c2e-0000-4000-8000-000000000001",
                "different-account-uuid",
            ),
        )
        .expect("write");

        assert!(read_cached_usage(&path).is_none());
    }

    #[test]
    fn a_missing_cached_usage_utilization_key_yields_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".claude.json");
        fs::write(
            &path,
            r#"{"oauthAccount": {"accountUuid": "8a4d1c2e-0000-4000-8000-000000000001"}}"#,
        )
        .expect("write");

        assert!(read_cached_usage(&path).is_none());
    }

    #[test]
    fn a_missing_file_yields_none() {
        assert!(read_cached_usage(Path::new("/nonexistent/.claude.json")).is_none());
    }

    #[test]
    fn a_file_over_the_cap_is_not_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".claude.json");
        let padding = "x".repeat((MAX_CONFIG_BYTES + 1) as usize);
        fs::write(&path, format!(r#"{{"pad": "{padding}"}}"#)).expect("write");

        assert!(read_cached_usage(&path).is_none());
    }

    #[test]
    fn a_utilization_object_parse_usage_rejects_yields_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(".claude.json");
        fs::write(
            &path,
            r#"{
              "oauthAccount": {"accountUuid": "account-uuid"},
              "cachedUsageUtilization": {
                "fetchedAtMs": 1788419487009,
                "accountUuid": "account-uuid",
                "utilization": {}
              }
            }"#,
        )
        .expect("write");

        assert!(read_cached_usage(&path).is_none());
    }
}
