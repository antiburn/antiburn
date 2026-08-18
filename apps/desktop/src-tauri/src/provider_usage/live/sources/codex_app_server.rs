// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The Codex app-server RPC fallback.
//!
//! [`super::codex_fetch`] asks the provider's usage endpoint directly, and
//! that is enough on its own for almost every reading. This module exists
//! for the rest: a network shaped in a way that endpoint cannot get through
//! (a captive portal, a corporate proxy that only trusts the Codex CLI's own
//! traffic) or an account whose token this application cannot obtain on its
//! own. It asks the same question a different way — through the `codex`
//! executable's own app-server, over the same JSON-RPC protocol the CLI's
//! other surfaces use, so it inherits whatever authentication the CLI itself
//! already has working.
//!
//! # The exchange
//!
//! Newline-delimited JSON-RPC over the child's stdin and stdout, stderr
//! discarded. Four messages, all written before this module waits for
//! anything back — the protocol does not require waiting for `initialize`'s
//! own response before sending what comes after it, and pipelining them
//! keeps this module to one write and one read loop instead of three of
//! each:
//!
//! 1. `initialize` (id 1)
//! 2. `initialized` (a notification — no id, no response)
//! 3. `account/read` (id 2)
//! 4. `account/rateLimits/read` (id 3)
//!
//! This module reads lines until it has seen responses for both id 2 and id
//! 3 (id 1's response, and anything else, is read and discarded — this is
//! not a general-purpose client, only enough of one to ask these two
//! questions), under a [`TIMEOUT`] and a [`MAX_STDOUT_BYTES`] cap, then kills
//! the child. It is never left running: nothing here is a client of a
//! long-lived server.
//!
//! # What "no rate limits" means
//!
//! `account/read` names the account's own type. An OAuth ChatGPT sign-in is
//! the only shape this module knows how to read a rate limit for; an API-key
//! or Bedrock-backed account is asked about correctly but has no plan
//! allowance to report, which is [`SourceOutcome::absent`] rather than a
//! failure — the same distinction every other source in this application
//! draws between "nothing to say" and "something went wrong".

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write as _};
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::provider_usage::live::codex::is_sliding_reset_projection;
use crate::provider_usage::live::model::{
    Confidence, ProviderUsageError, ProviderUsageSnapshot, SchemaReason, SupportTier, UsageScope,
    UsageSource, UsageWindow, UsageWindowKind, WindowRole,
};

use super::CODEX_SOURCE_ID;

/// The whole exchange — spawn, both round trips, teardown — must land inside
/// this. Generous enough for a cold-started process and a real request; past
/// it, whatever is wrong is not going to resolve itself by waiting longer.
const TIMEOUT: Duration = Duration::from_secs(4);

/// A cap on accumulated stdout, matching the shared HTTP response cap. This
/// is a JSON-RPC line stream, not a usage payload, but the same reasoning
/// applies: nothing this protocol legitimately sends is anywhere near this
/// large.
const MAX_STDOUT_BYTES: usize = 512 * 1024;

/// Codex's own OAuth account type. The only one this module can read a rate
/// limit for.
const OAUTH_ACCOUNT_TYPE: &str = "chatgpt";

/// Account types with no plan allowance to report — a real answer, not a
/// failure.
const NO_RATE_LIMIT_ACCOUNT_TYPES: [&str; 2] = ["apiKey", "amazonBedrock"];

/// Ask the local `codex` app-server for the reader's own rate limits.
///
/// `Ok(None)` covers exactly one case: an account type with no rate limit to
/// report (see the module doc's "What 'no rate limits' means"). Every other
/// absence — the executable missing, the process refusing to talk, a
/// malformed response — is an error, because unlike a merely-unconfigured
/// file, a reader who reaches this fallback already has Codex installed and
/// signed in: the direct fetch only calls here after finding `auth.json` in
/// the first place.
pub fn fetch(now: OffsetDateTime) -> Result<Option<ProviderUsageSnapshot>, ProviderUsageError> {
    let deadline = Instant::now() + TIMEOUT;
    let mut child = spawn()?;
    let result = converse(&mut child, deadline);
    // Always torn down, whatever `converse` returned — this is never a
    // client of a long-lived server.
    let _ = child.kill();
    let _ = child.wait();
    let (account_response, rate_limits_response) = result?;

    let account =
        rpc_result(&account_response)?
            .get("account")
            .ok_or(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField,
            ))?;
    let plan = match classify_account(account)? {
        AccountVerdict::NoRateLimits => return Ok(None),
        AccountVerdict::HasRateLimits(plan) => plan,
    };

    let rate_limits = rpc_result(&rate_limits_response)?;
    let windows = parse_rate_limits(rate_limits, now)?;
    if windows.is_empty() {
        return Err(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ));
    }

    Ok(Some(ProviderUsageSnapshot {
        provider: crate::provider_usage::providers::OPENAI,
        account: None,
        plan,
        observed_at: now,
        source: UsageSource {
            id: CODEX_SOURCE_ID,
            label: "Read from the Codex app".into(),
            confidence: Confidence::High,
            freshness: crate::provider_usage::live::model::Freshness::Fresh,
        },
        support: SupportTier::Live,
        windows,
        supplemental: None,
    }))
}

fn spawn() -> Result<Child, ProviderUsageError> {
    let mut command = antiburn_local::platform::process::headless_std_command("codex");
    command
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command.spawn().map_err(|_| ProviderUsageError::Unavailable)
}

/// Write every request, then read until both `account/read` and
/// `account/rateLimits/read` have answered or the deadline passes.
fn converse(child: &mut Child, deadline: Instant) -> Result<(Value, Value), ProviderUsageError> {
    let mut stdin = child.stdin.take().ok_or(ProviderUsageError::Unavailable)?;
    let stdout = child.stdout.take().ok_or(ProviderUsageError::Unavailable)?;

    for message in requests() {
        writeln!(stdin, "{message}").map_err(|_| ProviderUsageError::Unavailable)?;
    }
    stdin.flush().map_err(|_| ProviderUsageError::Unavailable)?;
    drop(stdin);

    let lines = spawn_line_reader(stdout);
    let mut account_response = None;
    let mut rate_limits_response = None;
    while account_response.is_none() || rate_limits_response.is_none() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ProviderUsageError::Unavailable);
        }
        let line = match lines.recv_timeout(remaining) {
            Ok(Ok(line)) => line,
            Ok(Err(_)) | Err(_) => return Err(ProviderUsageError::Unavailable),
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(trimmed)
            .map_err(|_| ProviderUsageError::Schema(SchemaReason::InvalidJson))?;
        match value.get("id").and_then(Value::as_i64) {
            Some(2) => account_response = Some(value),
            Some(3) => rate_limits_response = Some(value),
            _ => {}
        }
    }
    Ok((
        account_response.expect("loop only exits once both are set"),
        rate_limits_response.expect("loop only exits once both are set"),
    ))
}

/// The four newline-delimited JSON-RPC messages this module sends, in order.
fn requests() -> [String; 4] {
    [
        json!({
            "id": 1,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "antiburn",
                    "title": "antiburn",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": { "experimentalApi": false },
            },
        })
        .to_string(),
        json!({ "method": "initialized" }).to_string(),
        json!({
            "id": 2,
            "method": "account/read",
            "params": { "refreshToken": false },
        })
        .to_string(),
        json!({ "id": 3, "method": "account/rateLimits/read" }).to_string(),
    ]
}

/// Read lines off the child's stdout on a dedicated thread, so the caller can
/// wait on them with a deadline instead of blocking indefinitely on a
/// synchronous read.
fn spawn_line_reader(stdout: std::process::ChildStdout) -> mpsc::Receiver<std::io::Result<String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut total = 0usize;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(bytes_read) => {
                    total += bytes_read;
                    if total > MAX_STDOUT_BYTES {
                        let _ = tx.send(Err(std::io::Error::other(
                            "codex app-server stdout exceeded its cap",
                        )));
                        break;
                    }
                    if tx.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(error));
                    break;
                }
            }
        }
    });
    rx
}

/// The `result` field of a JSON-RPC response, or a schema error — an
/// `error` field, or neither, both mean this exchange did not get the answer
/// it needed.
fn rpc_result(response: &Value) -> Result<&Value, ProviderUsageError> {
    response
        .get("result")
        .filter(|result| !result.is_null())
        .ok_or(ProviderUsageError::Schema(SchemaReason::MissingEnvelope))
}

/// What `account/read`'s result implies for whether this module can go on to
/// ask about rate limits, and how.
#[derive(Debug, PartialEq)]
enum AccountVerdict {
    /// An OAuth ChatGPT sign-in: proceed, with the plan label the account
    /// stated, if it stated one.
    HasRateLimits(Option<String>),
    /// A real answer, but this account type has no plan allowance to report.
    NoRateLimits,
}

/// Classify `account/read`'s `result.account`, failing closed on anything
/// this module was not written to understand.
fn classify_account(account: &Value) -> Result<AccountVerdict, ProviderUsageError> {
    let account_type =
        account
            .get("type")
            .and_then(Value::as_str)
            .ok_or(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField,
            ))?;
    if NO_RATE_LIMIT_ACCOUNT_TYPES.contains(&account_type) {
        return Ok(AccountVerdict::NoRateLimits);
    }
    if account_type != OAUTH_ACCOUNT_TYPE {
        return Err(ProviderUsageError::Schema(SchemaReason::InvalidValue));
    }
    let plan = account
        .get("planType")
        .and_then(Value::as_str)
        .filter(|plan| !plan.is_empty())
        .map(plan_label);
    Ok(AccountVerdict::HasRateLimits(plan))
}

/// The label a `planType` slug is shown under. An unrecognized slug keeps its
/// own spelling rather than being dropped — the provider stated a plan, and
/// showing it verbatim is more honest than showing nothing.
fn plan_label(plan_type: &str) -> String {
    match plan_type {
        "free" => "ChatGPT Free".to_string(),
        "plus" => "ChatGPT Plus".to_string(),
        "pro" => "ChatGPT Pro".to_string(),
        "team" => "ChatGPT Team".to_string(),
        "business" => "ChatGPT Business".to_string(),
        "enterprise" => "ChatGPT Enterprise".to_string(),
        "edu" => "ChatGPT Edu".to_string(),
        "go" => "ChatGPT Go".to_string(),
        other => other.to_string(),
    }
}

/// `account/rateLimits/read`'s result: `rateLimits` (the "default" bucket)
/// plus `rateLimitsByLimitId`, one bucket per key.
fn parse_rate_limits(
    result: &Value,
    observed_at: OffsetDateTime,
) -> Result<Vec<UsageWindow>, ProviderUsageError> {
    let mut windows = Vec::new();
    let mut seen: HashSet<(String, i64, Option<i64>)> = HashSet::new();

    if let Some(default_bucket) = result.get("rateLimits").filter(|value| !value.is_null()) {
        parse_bucket(
            "default",
            default_bucket,
            observed_at,
            &mut windows,
            &mut seen,
        )?;
    }
    if let Some(by_limit_id) = result
        .get("rateLimitsByLimitId")
        .filter(|value| !value.is_null())
    {
        let buckets = by_limit_id
            .as_object()
            .ok_or(ProviderUsageError::Schema(SchemaReason::InvalidValue))?;
        for (key, bucket) in buckets {
            parse_bucket(key, bucket, observed_at, &mut windows, &mut seen)?;
        }
    }
    Ok(windows)
}

fn parse_bucket(
    bucket_key: &str,
    bucket: &Value,
    observed_at: OffsetDateTime,
    windows: &mut Vec<UsageWindow>,
    seen: &mut HashSet<(String, i64, Option<i64>)>,
) -> Result<(), ProviderUsageError> {
    let bucket_id = bucket
        .get("limitId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .unwrap_or(bucket_key);

    for sub_key in ["primary", "secondary"] {
        let Some(sub) = bucket.get(sub_key).filter(|value| !value.is_null()) else {
            continue;
        };
        // `windowDurationMins` is the one field this shape leaves genuinely
        // optional (see the module doc's contract). Without it there is no
        // duration to classify the window by and no minutes to put in its
        // id, so the window is skipped rather than the whole payload
        // failing — an incomplete entry is not evidence the rest is wrong.
        let Some(minutes) = bucket_duration_minutes(sub)? else {
            continue;
        };
        let used_percent = sub
            .get("usedPercent")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
            .ok_or(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField,
            ))?;
        if !(0.0..=100.0).contains(&used_percent) {
            return Err(ProviderUsageError::Schema(SchemaReason::InvalidValue));
        }
        let raw_reset = raw_epoch_seconds(sub.get("resetsAt"))?;

        let dedup_key = (bucket_id.to_string(), minutes, raw_reset);
        if !seen.insert(dedup_key) {
            continue;
        }

        let resets_at = raw_reset
            .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
            .filter(|reset| {
                !is_sliding_reset_projection(*reset, observed_at, minutes * 60, used_percent)
            });

        let weekly = minutes == 10_080;
        let long = minutes >= 1_440;
        windows.push(UsageWindow {
            id: format!("{bucket_id}-{minutes}m"),
            role: if long {
                WindowRole::PrimaryLong
            } else {
                WindowRole::PrimaryShort
            },
            kind: if weekly {
                UsageWindowKind::Weekly
            } else {
                UsageWindowKind::Rolling
            },
            scope: UsageScope::Account,
            used_percent: Some(used_percent),
            starts_at: None,
            resets_at,
            authoritative: true,
        });
    }
    Ok(())
}

fn bucket_duration_minutes(sub: &Value) -> Result<Option<i64>, ProviderUsageError> {
    let Some(value) = sub
        .get("windowDurationMins")
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    let minutes = value
        .as_i64()
        .filter(|minutes| *minutes > 0)
        .ok_or(ProviderUsageError::Schema(SchemaReason::InvalidValue))?;
    Ok(Some(minutes))
}

fn raw_epoch_seconds(value: Option<&Value>) -> Result<Option<i64>, ProviderUsageError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    value
        .as_i64()
        .map(Some)
        .ok_or(ProviderUsageError::Schema(SchemaReason::InvalidValue))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(seconds).expect("valid timestamp")
    }

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn a_limit_id_override_replaces_the_map_key_in_the_window_id() {
        let mut windows = Vec::new();
        let mut seen = HashSet::new();
        let bucket = serde_json::json!({
            "limitId": "gpt-5-weekly",
            "primary": {"usedPercent": 12.0, "windowDurationMins": 300}
        });
        parse_bucket("some-opaque-key", &bucket, at(NOW), &mut windows, &mut seen).unwrap();
        assert_eq!(windows[0].id, "gpt-5-weekly-300m");
    }

    #[test]
    fn duration_thresholds_decide_role_and_kind() {
        let mut windows = Vec::new();
        let mut seen = HashSet::new();
        let bucket = serde_json::json!({
            "primary": {"usedPercent": 10.0, "windowDurationMins": 300},
            "secondary": {"usedPercent": 20.0, "windowDurationMins": 10080}
        });
        parse_bucket("default", &bucket, at(NOW), &mut windows, &mut seen).unwrap();

        assert_eq!(windows[0].id, "default-300m");
        assert_eq!(windows[0].role, WindowRole::PrimaryShort);
        assert_eq!(windows[0].kind, UsageWindowKind::Rolling);

        assert_eq!(windows[1].id, "default-10080m");
        assert_eq!(windows[1].role, WindowRole::PrimaryLong);
        assert_eq!(windows[1].kind, UsageWindowKind::Weekly);
    }

    #[test]
    fn a_long_window_short_of_exactly_a_week_is_long_but_not_weekly() {
        let mut windows = Vec::new();
        let mut seen = HashSet::new();
        let bucket = serde_json::json!({
            "primary": {"usedPercent": 5.0, "windowDurationMins": 1440}
        });
        parse_bucket("default", &bucket, at(NOW), &mut windows, &mut seen).unwrap();
        assert_eq!(windows[0].role, WindowRole::PrimaryLong);
        assert_eq!(windows[0].kind, UsageWindowKind::Rolling);
    }

    #[test]
    fn identical_bucket_minutes_and_reset_triples_are_deduplicated() {
        let mut windows = Vec::new();
        let mut seen = HashSet::new();
        let bucket = serde_json::json!({
            "primary": {"usedPercent": 10.0, "windowDurationMins": 300, "resetsAt": NOW + 1000}
        });
        parse_bucket("default", &bucket, at(NOW), &mut windows, &mut seen).unwrap();
        // A second, identical entry for the same bucket must not double the
        // window up.
        parse_bucket("default", &bucket, at(NOW), &mut windows, &mut seen).unwrap();
        assert_eq!(windows.len(), 1);
    }

    #[test]
    fn a_window_missing_its_duration_is_skipped_not_a_payload_failure() {
        let mut windows = Vec::new();
        let mut seen = HashSet::new();
        let bucket = serde_json::json!({
            "primary": {"usedPercent": 10.0}
        });
        parse_bucket("default", &bucket, at(NOW), &mut windows, &mut seen).unwrap();
        assert!(windows.is_empty());
    }

    #[test]
    fn a_percentage_outside_the_domain_rejects_the_bucket() {
        let mut windows = Vec::new();
        let mut seen = HashSet::new();
        let bucket = serde_json::json!({
            "primary": {"usedPercent": 140.0, "windowDurationMins": 300}
        });
        assert_eq!(
            parse_bucket("default", &bucket, at(NOW), &mut windows, &mut seen),
            Err(ProviderUsageError::Schema(SchemaReason::InvalidValue))
        );
    }

    /* ---------------------------------------------------------------------
     * The sliding-reset discard
     * ------------------------------------------------------------------- */

    #[test]
    fn a_zero_usage_reset_landing_on_the_projected_boundary_is_dropped() {
        // Five-hour (18,000-second) window, currently unused, whose stated
        // reset lands almost exactly 18,000 seconds from now: a rolling
        // projection, not a commitment. This is the shared helper
        // `codex.rs` also exercises against its own account-wide windows —
        // covered here as well because `parse_bucket` calls it with a
        // minutes-to-seconds conversion this test is also checking.
        let observed_at = at(NOW);
        let projected = observed_at + time::Duration::seconds(18_000);
        assert!(is_sliding_reset_projection(
            projected + time::Duration::seconds(30),
            observed_at,
            18_000,
            0.0
        ));
    }

    #[test]
    fn a_zero_usage_reset_far_from_the_projected_boundary_is_kept() {
        let observed_at = at(NOW);
        let real_reset = observed_at + time::Duration::hours(2);
        assert!(!is_sliding_reset_projection(
            real_reset,
            observed_at,
            18_000,
            0.0
        ));
    }

    #[test]
    fn a_nonzero_usage_reset_is_never_treated_as_a_projection() {
        // Even one that lands exactly on the boundary: a window that has
        // actually been used has a real reset, whatever its timing looks
        // like.
        let observed_at = at(NOW);
        let projected = observed_at + time::Duration::seconds(18_000);
        assert!(!is_sliding_reset_projection(
            projected,
            observed_at,
            18_000,
            4.0
        ));
    }

    #[test]
    fn the_discard_clears_only_the_timestamp_and_keeps_the_window() {
        let mut windows = Vec::new();
        let mut seen = HashSet::new();
        let observed_at = at(NOW);
        let projected_epoch = (observed_at + time::Duration::minutes(300)).unix_timestamp();
        let bucket = serde_json::json!({
            "primary": {"usedPercent": 0.0, "windowDurationMins": 300, "resetsAt": projected_epoch}
        });
        parse_bucket("default", &bucket, observed_at, &mut windows, &mut seen).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].resets_at, None);
        assert_eq!(windows[0].used_percent, Some(0.0));
    }

    /* ---------------------------------------------------------------------
     * `account/read` and the envelope
     * ------------------------------------------------------------------- */

    #[test]
    fn a_chatgpt_account_proceeds_with_its_stated_plan() {
        let account = serde_json::json!({"type": "chatgpt", "planType": "plus"});
        assert_eq!(
            classify_account(&account),
            Ok(AccountVerdict::HasRateLimits(Some(
                "ChatGPT Plus".to_string()
            )))
        );
    }

    #[test]
    fn a_chatgpt_account_with_no_stated_plan_still_proceeds() {
        let account = serde_json::json!({"type": "chatgpt"});
        assert_eq!(
            classify_account(&account),
            Ok(AccountVerdict::HasRateLimits(None))
        );
    }

    #[test]
    fn api_key_and_bedrock_accounts_have_no_rate_limits_to_report() {
        for account_type in ["apiKey", "amazonBedrock"] {
            let account = serde_json::json!({"type": account_type});
            assert_eq!(classify_account(&account), Ok(AccountVerdict::NoRateLimits));
        }
    }

    #[test]
    fn an_unsupported_account_type_is_a_schema_error() {
        let account = serde_json::json!({"type": "somethingElse"});
        assert_eq!(
            classify_account(&account),
            Err(ProviderUsageError::Schema(SchemaReason::InvalidValue))
        );
    }

    #[test]
    fn a_missing_account_type_is_a_schema_error() {
        let account = serde_json::json!({});
        assert_eq!(
            classify_account(&account),
            Err(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField
            ))
        );
    }

    #[test]
    fn a_response_carrying_an_error_instead_of_a_result_is_a_schema_error() {
        let response = serde_json::json!({"id": 2, "error": {"code": -1, "message": "no"}});
        assert_eq!(
            rpc_result(&response),
            Err(ProviderUsageError::Schema(SchemaReason::MissingEnvelope))
        );
    }

    #[test]
    fn plan_type_slugs_map_to_their_display_names() {
        assert_eq!(plan_label("plus"), "ChatGPT Plus");
        assert_eq!(plan_label("enterprise"), "ChatGPT Enterprise");
    }

    #[test]
    fn an_unrecognized_plan_type_keeps_its_own_spelling() {
        assert_eq!(plan_label("mystery-tier"), "mystery-tier");
    }
}
