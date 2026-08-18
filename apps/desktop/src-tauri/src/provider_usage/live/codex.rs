// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The Codex "wham" usage payload, read strictly.
//!
//! This is what `GET https://chatgpt.com/backend-api/wham/usage` returns —
//! [`sources::codex_fetch`](super::sources::codex_fetch) is this module's
//! only caller. Unlike the Anthropic parser this application also carries,
//! there is exactly one carrier here: no cached copy of this payload exists
//! on disk anywhere, so there is nothing to reuse this parser for besides the
//! live response.
//!
//! # What the payload looks like
//!
//! A `rate_limit` object is the envelope; its absence is a schema error, not
//! an empty reading, because a response with no `rate_limit` key at all means
//! the endpoint's shape moved, not that the account has no limits. Inside it,
//! `primary_window` and `secondary_window` each carry `used_percent`,
//! `limit_window_seconds`, and an optional `reset_at` (epoch seconds). Which
//! one is the weekly window is not decided by its key name — it is decided by
//! its own stated duration: exactly `604800` seconds (seven days) is weekly,
//! anything else is the short rolling window. `rate_limit` may also carry a
//! `plan_type` label.

use serde_json::Value;
use time::OffsetDateTime;

use super::model::{
    ProviderUsageError, SchemaReason, UsageScope, UsageWindow, UsageWindowKind, WindowRole,
};
use super::normalize::used_percent;

/// Seven days, in seconds — the one number that tells `primary_window` and
/// `secondary_window` apart from each other.
const WEEKLY_WINDOW_SECONDS: i64 = 604_800;

/// What a well-formed `wham/usage` payload yielded.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CodexUsage {
    pub windows: Vec<UsageWindow>,
    pub plan: Option<String>,
}

/// Parse a `wham/usage` response body.
///
/// Fails rather than returning a partial reading, for the same reason the
/// Anthropic parser does: a payload only half understood is exactly the
/// input that would put a confident, wrong meter on screen.
pub fn parse_wham_usage(input: &str) -> Result<CodexUsage, ProviderUsageError> {
    let value: Value = serde_json::from_str(input)
        .map_err(|_| ProviderUsageError::Schema(SchemaReason::InvalidJson))?;
    let rate_limit = value
        .get("rate_limit")
        .filter(|rate_limit| !rate_limit.is_null())
        .ok_or(ProviderUsageError::Schema(SchemaReason::MissingEnvelope))?;

    let mut windows = Vec::new();
    for key in ["primary_window", "secondary_window"] {
        if let Some(window_value) = rate_limit.get(key).filter(|value| !value.is_null()) {
            windows.push(window(window_value)?);
        }
    }
    if windows.is_empty() {
        return Err(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ));
    }

    let plan = rate_limit
        .get("plan_type")
        .and_then(Value::as_str)
        .filter(|plan| !plan.trim().is_empty())
        .map(str::to_owned);

    Ok(CodexUsage { windows, plan })
}

fn window(value: &Value) -> Result<UsageWindow, ProviderUsageError> {
    let used_percent = used_percent(value.get("used_percent").and_then(Value::as_f64))?.ok_or(
        ProviderUsageError::Schema(SchemaReason::MissingRequiredField),
    )?;
    let window_seconds = value
        .get("limit_window_seconds")
        .and_then(Value::as_f64)
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .ok_or(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ))?;
    let weekly = window_seconds.round() as i64 == WEEKLY_WINDOW_SECONDS;
    let (role, kind, id) = if weekly {
        (
            WindowRole::PrimaryLong,
            UsageWindowKind::Weekly,
            "seven-day".to_string(),
        )
    } else {
        (
            WindowRole::PrimaryShort,
            UsageWindowKind::Rolling,
            "five-hour".to_string(),
        )
    };

    Ok(UsageWindow {
        id,
        role,
        kind,
        scope: UsageScope::Account,
        used_percent: Some(used_percent),
        starts_at: None,
        resets_at: epoch_seconds(value.get("reset_at"))?,
        authoritative: true,
    })
}

fn epoch_seconds(value: Option<&Value>) -> Result<Option<OffsetDateTime>, ProviderUsageError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let seconds = value
        .as_f64()
        .filter(|seconds| seconds.is_finite())
        .ok_or(ProviderUsageError::Schema(SchemaReason::InvalidValue))?;
    OffsetDateTime::from_unix_timestamp(seconds.trunc() as i64)
        .map(Some)
        .map_err(|_| ProviderUsageError::Schema(SchemaReason::InvalidValue))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH_WINDOWS: &str = r#"{
      "rate_limit": {
        "primary_window": {"used_percent": 42, "limit_window_seconds": 18000, "reset_at": 1800003600},
        "secondary_window": {"used_percent": 71, "limit_window_seconds": 604800, "reset_at": 1800500000},
        "plan_type": "plus"
      }
    }"#;

    #[test]
    fn the_weekly_window_is_identified_by_its_own_duration_not_its_key_name() {
        let usage = parse_wham_usage(BOTH_WINDOWS).expect("parses");
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[0].id, "five-hour");
        assert_eq!(usage.windows[0].role, WindowRole::PrimaryShort);
        assert_eq!(usage.windows[0].used_percent, Some(42.0));
        assert_eq!(usage.windows[1].id, "seven-day");
        assert_eq!(usage.windows[1].role, WindowRole::PrimaryLong);
        assert_eq!(usage.windows[1].used_percent, Some(71.0));
        assert_eq!(usage.plan.as_deref(), Some("plus"));
    }

    #[test]
    fn a_window_whose_duration_is_not_exactly_a_week_is_the_short_window_regardless_of_size() {
        // A duration close to, but not exactly, a week must not be guessed at.
        let almost_weekly = r#"{"rate_limit": {"primary_window":
          {"used_percent": 10, "limit_window_seconds": 604799, "reset_at": null}}}"#;
        let usage = parse_wham_usage(almost_weekly).expect("parses");
        assert_eq!(usage.windows[0].id, "five-hour");
        assert_eq!(usage.windows[0].role, WindowRole::PrimaryShort);
    }

    #[test]
    fn a_missing_rate_limit_envelope_is_a_schema_error() {
        assert_eq!(
            parse_wham_usage("{}"),
            Err(ProviderUsageError::Schema(SchemaReason::MissingEnvelope))
        );
        assert_eq!(
            parse_wham_usage(r#"{"rate_limit": null}"#),
            Err(ProviderUsageError::Schema(SchemaReason::MissingEnvelope))
        );
    }

    #[test]
    fn a_rate_limit_object_with_neither_window_is_a_schema_error() {
        assert_eq!(
            parse_wham_usage(r#"{"rate_limit": {"plan_type": "plus"}}"#),
            Err(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField
            ))
        );
    }

    #[test]
    fn a_percentage_outside_the_domain_rejects_the_whole_payload() {
        let absurd = r#"{"rate_limit": {"primary_window":
          {"used_percent": 140, "limit_window_seconds": 18000}}}"#;
        assert_eq!(
            parse_wham_usage(absurd),
            Err(ProviderUsageError::Schema(SchemaReason::InvalidValue))
        );
    }

    #[test]
    fn epoch_seconds_resets_are_read_directly() {
        let usage = parse_wham_usage(BOTH_WINDOWS).expect("parses");
        assert_eq!(
            usage.windows[0].resets_at,
            OffsetDateTime::from_unix_timestamp(1_800_003_600i64).ok()
        );
    }

    #[test]
    fn a_missing_plan_type_is_absent_rather_than_an_empty_string() {
        let no_plan = r#"{"rate_limit": {"primary_window":
          {"used_percent": 10, "limit_window_seconds": 18000}}}"#;
        assert_eq!(parse_wham_usage(no_plan).expect("parses").plan, None);
    }
}
