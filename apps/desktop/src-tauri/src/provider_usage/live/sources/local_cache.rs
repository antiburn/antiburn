// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Claude's own cached usage reading, as the agent left it on disk.
//!
//! # Why this is the first source, and why it needs no permission
//!
//! Claude Code fetches its own usage and writes the provider's answer into
//! `~/.claude.json`. Reading that file is the same kind of act as everything
//! else this application does: open a documented location an agent wrote,
//! read what is there, close it. No socket, no credential, no child process,
//! no port. It therefore works with the machine disconnected — the numbers
//! were fetched when the agent last had a connection, and every window says
//! how old it is instead of pretending to be current.
//!
//! That is also this source's ceiling, and the reason freshness is on the
//! face of the surface rather than in a tooltip: an agent that has not run
//! since Tuesday has a Tuesday answer, and the reader has to be able to see
//! that at a glance.
//!
//! # Independently authored
//!
//! The private app reads the same file. Its collectors are denied
//! (`docs/oss/source-denylist.toml`, rule `provider-usage`); this module is
//! written for this application, and shares only the allowlisted domain model
//! the parsed result is expressed in.

use std::fs;
use std::path::PathBuf;

use time::{Duration, OffsetDateTime};

use crate::provider_usage::live::anthropic;
use crate::provider_usage::live::model::{
    Confidence, Freshness, ProviderUsageError, ProviderUsageSnapshot, SchemaReason, SupportTier,
    UsageSource,
};
use crate::provider_usage::live::{LiveUsageSource, SourceOutcome};

/// The file is a general-purpose agent state file, not a usage file, and it
/// grows with the reader's project list. Cap the read so a pathological one
/// cannot be pulled into memory on a five-minute timer.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// How old a cached reading may be and still describe the present.
///
/// An hour, matching the shortest window's own granularity: past that, a
/// five-hour figure has had time to move without us hearing about it. Stale
/// readings are still shown — they are the best anyone has — but they are
/// labelled, they never fire a milestone, and they lose to any fresher
/// source.
const MAX_AGE: Duration = Duration::hours(1);

/// Reads `~/.claude.json` for the usage Claude Code cached there.
pub struct ClaudeLocalCache {
    path: Option<PathBuf>,
}

impl ClaudeLocalCache {
    pub fn new() -> ClaudeLocalCache {
        ClaudeLocalCache {
            path: antiburn_local::paths::home_dir().map(|home| home.join(".claude.json")),
        }
    }

    /// A cache rooted at an explicit path, for tests.
    #[cfg(test)]
    pub fn at(path: PathBuf) -> ClaudeLocalCache {
        ClaudeLocalCache { path: Some(path) }
    }
}

impl LiveUsageSource for ClaudeLocalCache {
    fn id(&self) -> &'static str {
        "claude-local-cache"
    }

    fn fetch(&self) -> SourceOutcome {
        let Some(path) = self.path.as_deref() else {
            return SourceOutcome::absent();
        };
        match fs::metadata(path) {
            // No agent state file at all is the ordinary state of a machine
            // that does not run this agent. Not an error, and not worth a
            // line on anyone's screen.
            Err(_) => return SourceOutcome::absent(),
            Ok(metadata) if metadata.len() > MAX_BYTES => {
                return SourceOutcome::failed(ProviderUsageError::Unavailable);
            }
            Ok(_) => {}
        }
        let Ok(contents) = fs::read_to_string(path) else {
            // The file exists but will not open — a permission problem or a
            // partial write. That one *is* worth surfacing: something the
            // reader could fix is in the way.
            return SourceOutcome::failed(ProviderUsageError::Unavailable);
        };

        match snapshot(&contents, OffsetDateTime::now_utc()) {
            Ok(Some(snapshot)) => SourceOutcome::found(vec![snapshot]),
            Ok(None) => SourceOutcome::absent(),
            Err(error) => SourceOutcome::failed(error),
        }
    }
}

/// Turn the agent's state file into a snapshot.
///
/// `Ok(None)` means the file is fine and simply carries no cached usage —
/// an agent installed but never signed in, or a build that predates the
/// cache. Only a payload that *is* there and does not parse is an error.
fn snapshot(
    contents: &str,
    now: OffsetDateTime,
) -> Result<Option<ProviderUsageSnapshot>, ProviderUsageError> {
    let state: serde_json::Value = serde_json::from_str(contents)
        .map_err(|_| ProviderUsageError::Schema(SchemaReason::InvalidJson))?;
    let Some(cached) = state
        .get("cachedUsageUtilization")
        .filter(|cached| !cached.is_null())
    else {
        return Ok(None);
    };
    let Some(utilization) = cached
        .get("utilization")
        .filter(|utilization| !utilization.is_null())
    else {
        return Ok(None);
    };

    // The agent stamps when it fetched. Without that we cannot say how old
    // the figures are, and a limit surface that cannot date itself is one
    // the reader has no way to judge — so it does not ship at all.
    let observed_at = cached
        .get("fetchedAtMs")
        .and_then(serde_json::Value::as_i64)
        .and_then(|millis| OffsetDateTime::from_unix_timestamp(millis / 1_000).ok())
        .ok_or(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ))?;

    let usage = anthropic::parse_usage(&utilization.to_string())?;

    Ok(Some(ProviderUsageSnapshot {
        provider: crate::provider_usage::providers::ANTHROPIC,
        // Local-only: nothing is uploaded, so the provider's own account id
        // is the discriminator rather than a hash of it. It never leaves the
        // process — it keys the milestone ledger and the sample history, and
        // is not part of any payload the views receive.
        account: cached
            .get("accountUuid")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        plan: None,
        observed_at,
        source: UsageSource {
            id: "claude-local-cache",
            label: "Claude's cached usage".into(),
            confidence: Confidence::High,
            freshness: Freshness::at(observed_at, now, MAX_AGE),
        },
        support: SupportTier::Live,
        windows: usage.windows,
        supplemental: usage.supplemental,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_usage::live::model::{UsageScope, UsageWindowKind, WindowRole};

    /// Synthetic, hand-authored, and matching the observed payload shape.
    /// Never a captured file: `CONTRIBUTING.md` forbids machine output as a
    /// fixture, and a real one would carry a real account id.
    fn state(fetched_at_ms: i64) -> String {
        format!(
            r#"{{
              "numStartups": 3,
              "cachedUsageUtilization": {{
                "fetchedAtMs": {fetched_at_ms},
                "accountUuid": "00000000-0000-4000-8000-000000000000",
                "utilization": {{
                  "limits": [
                    {{"kind": "session", "group": "session", "percent": 40,
                      "severity": "normal", "resets_at": "2027-01-15T12:00:00+00:00",
                      "scope": null, "is_active": true}},
                    {{"kind": "weekly_all", "group": "weekly", "percent": 65,
                      "severity": "normal", "resets_at": "2027-01-19T18:00:00+00:00",
                      "scope": null, "is_active": false}},
                    {{"kind": "weekly_scoped", "group": "weekly", "percent": 95,
                      "severity": "critical", "resets_at": "2027-01-19T18:00:00+00:00",
                      "scope": {{"model": {{"id": null, "display_name": "Some Model"}}, "surface": null}},
                      "is_active": true}}
                  ],
                  "extra_usage": {{"is_enabled": false, "monthly_limit": null,
                    "used_credits": null, "utilization": null, "currency": null}}
                }}
              }}
            }}"#
        )
    }

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn the_cached_reading_becomes_three_windows_with_stable_ids() {
        let now = OffsetDateTime::from_unix_timestamp(NOW).unwrap();
        let snapshot = snapshot(&state(NOW * 1_000), now).unwrap().unwrap();

        let ids: Vec<&str> = snapshot
            .windows
            .iter()
            .map(|window| window.id.as_str())
            .collect();
        assert_eq!(ids, ["five-hour", "seven-day", "weekly-some-model"]);
        assert_eq!(snapshot.windows[0].role, WindowRole::PrimaryShort);
        assert_eq!(snapshot.windows[0].kind, UsageWindowKind::Rolling);
        assert_eq!(snapshot.windows[0].used_percent, Some(40.0));
        assert_eq!(snapshot.windows[1].used_percent, Some(65.0));
        assert_eq!(
            snapshot.windows[2].scope,
            UsageScope::Model("Some Model".into())
        );
        assert!(snapshot.windows.iter().all(|window| window.authoritative));
        assert_eq!(snapshot.support, SupportTier::Live);
        assert_eq!(snapshot.source.freshness, Freshness::Fresh);
    }

    #[test]
    fn a_reading_older_than_the_age_budget_is_stale_but_still_reported() {
        let now = OffsetDateTime::from_unix_timestamp(NOW).unwrap();
        // Two hours old against a one-hour budget.
        let snapshot = snapshot(&state((NOW - 7_200) * 1_000), now)
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.source.freshness, Freshness::Stale);
        // Stale, but the figures are still the best anyone has.
        assert_eq!(snapshot.windows.len(), 3);
    }

    #[test]
    fn a_file_without_cached_usage_is_absent_rather_than_broken() {
        let now = OffsetDateTime::from_unix_timestamp(NOW).unwrap();
        assert_eq!(snapshot(r#"{"numStartups": 3}"#, now), Ok(None));
        assert_eq!(
            snapshot(r#"{"cachedUsageUtilization": null}"#, now),
            Ok(None)
        );
    }

    #[test]
    fn a_payload_that_is_present_but_unreadable_fails_rather_than_showing_nothing() {
        let now = OffsetDateTime::from_unix_timestamp(NOW).unwrap();
        assert_eq!(
            snapshot("not json at all", now),
            Err(ProviderUsageError::Schema(SchemaReason::InvalidJson))
        );
        // Cached usage with no fetch stamp: we cannot date it, so we refuse
        // it rather than render an undateable meter.
        let undated = r#"{"cachedUsageUtilization": {"utilization": {"limits": []}}}"#;
        assert_eq!(
            snapshot(undated, now),
            Err(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField
            ))
        );
    }

    #[test]
    fn a_missing_file_reports_absence_not_failure() {
        let source = ClaudeLocalCache::at(PathBuf::from("/nonexistent/.claude.json"));
        let outcome = source.fetch();
        assert!(outcome.snapshots.is_empty());
        assert_eq!(outcome.error, None);
    }
}
