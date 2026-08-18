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
//! anything else is a rolling window. The plan label sits at the response's
//! own root as `plan_type`; older payloads carried it nested inside
//! `rate_limit` instead, which this parser still reads as a fallback.
//!
//! Sibling to `rate_limit`, an optional root-level `additional_rate_limits`
//! array carries model- or feature-scoped allowances alongside the
//! account-wide ones — a `limit_name` (its display name) plus a `rate_limit`
//! object in the identical `primary_window`/`secondary_window` shape. Each of
//! those becomes a [`WindowRole::Supplemental`] window scoped to that name,
//! the same relationship Anthropic's `weekly_scoped` limits have to its
//! account-wide ones. A missing or `null` `additional_rate_limits` key is an
//! older payload shape, not an error; an entry inside it that is missing its
//! name or its own `rate_limit` object is malformed, and — like every other
//! half-understood shape this parser meets — fails the whole payload rather
//! than being silently dropped.
//!
//! # Window ids never claim a duration the payload did not state
//!
//! An id of `five-hour` or `seven-day` is a claim about that window's exact
//! length, and the frontend derives elapsed-marker arithmetic from it (see
//! `IMPLIED_PERIOD_MS` in `liveUsage.ts`) — so this parser only ever uses
//! those ids when the stated duration matches exactly (`18000` or `604800`
//! seconds). Anything else gets a duration-derived id instead
//! (`rolling-<minutes>m`), which draws no elapsed marker at all: an honest
//! absence, not a wrong one.
//!
//! # The sliding-reset discard
//!
//! A window can come back with `used_percent: 0` and a `reset_at` that lands
//! almost exactly one window-length from now — a server saying "a window this
//! long, currently empty" rather than a committed reset. [`is_sliding_reset_projection`]
//! is the shared rule the `codex_app_server` fallback module also applies to
//! its own reading of the same account; both drop a reset shaped like that
//! rather than let the runway math treat a rolling estimate as a hard
//! deadline. It applies here to every window this parser produces,
//! account-wide and model-scoped alike.

use serde_json::Value;
use time::OffsetDateTime;

use super::model::{
    ProviderUsageError, SchemaReason, UsageScope, UsageWindow, UsageWindowKind, WindowRole,
};
use super::normalize::{slugify, used_percent};

/// Seven days, in seconds — the one duration that makes a window weekly
/// regardless of which key it arrived under.
const WEEKLY_WINDOW_SECONDS: i64 = 604_800;

/// Five hours, in seconds — the one duration a window is allowed to carry the
/// `five-hour` id, whose exact length the frontend's elapsed-marker math
/// assumes.
const FIVE_HOUR_WINDOW_SECONDS: i64 = 18_000;

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
///
/// `observed_at` is the instant this response is being read as of — the same
/// value the caller stamps the resulting snapshot with — and it is what the
/// sliding-reset discard measures every window's stated reset against.
pub fn parse_wham_usage(
    input: &str,
    observed_at: OffsetDateTime,
) -> Result<CodexUsage, ProviderUsageError> {
    let value: Value = serde_json::from_str(input)
        .map_err(|_| ProviderUsageError::Schema(SchemaReason::InvalidJson))?;
    let rate_limit = value
        .get("rate_limit")
        .filter(|rate_limit| !rate_limit.is_null())
        .ok_or(ProviderUsageError::Schema(SchemaReason::MissingEnvelope))?;

    let mut windows = Vec::new();
    for key in ["primary_window", "secondary_window"] {
        if let Some(window_value) = rate_limit.get(key).filter(|value| !value.is_null()) {
            windows.push(account_window(window_value, observed_at)?);
        }
    }
    if windows.is_empty() {
        return Err(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ));
    }
    windows.extend(additional_windows(&value, observed_at)?);

    // The root is the current shape; `rate_limit.plan_type` is what older
    // payloads carried instead, kept as a fallback rather than dropped.
    let plan = value
        .get("plan_type")
        .and_then(Value::as_str)
        .or_else(|| rate_limit.get("plan_type").and_then(Value::as_str))
        .filter(|plan| !plan.trim().is_empty())
        .map(str::to_owned);

    Ok(CodexUsage { windows, plan })
}

/// The account-wide `primary_window` / `secondary_window` shape.
fn account_window(
    value: &Value,
    observed_at: OffsetDateTime,
) -> Result<UsageWindow, ProviderUsageError> {
    let used_percent = read_used_percent(value)?;
    let window_seconds = window_duration_seconds(value)?;
    let weekly = window_seconds == WEEKLY_WINDOW_SECONDS;
    let five_hour = window_seconds == FIVE_HOUR_WINDOW_SECONDS;

    let (role, kind, id) = if weekly {
        (
            WindowRole::PrimaryLong,
            UsageWindowKind::Weekly,
            "seven-day".to_string(),
        )
    } else if five_hour {
        (
            WindowRole::PrimaryShort,
            UsageWindowKind::Rolling,
            "five-hour".to_string(),
        )
    } else {
        (
            WindowRole::PrimaryShort,
            UsageWindowKind::Rolling,
            format!("rolling-{}m", window_seconds / 60),
        )
    };

    Ok(UsageWindow {
        id,
        role,
        kind,
        scope: UsageScope::Account,
        used_percent: Some(used_percent),
        starts_at: None,
        resets_at: reset_at(value, observed_at, window_seconds, used_percent)?,
        authoritative: true,
    })
}

/// The optional `additional_rate_limits` array: model- or feature-scoped
/// allowances alongside the account-wide ones. Absent or `null` is an older
/// payload shape and yields no windows; present but malformed — the wrong
/// JSON type, an entry missing its name or its own `rate_limit` — fails the
/// whole payload, the same as every other shape this parser does not
/// recognize.
fn additional_windows(
    value: &Value,
    observed_at: OffsetDateTime,
) -> Result<Vec<UsageWindow>, ProviderUsageError> {
    let Some(entries) = value
        .get("additional_rate_limits")
        .filter(|value| !value.is_null())
    else {
        return Ok(Vec::new());
    };
    let entries = entries
        .as_array()
        .ok_or(ProviderUsageError::Schema(SchemaReason::InvalidValue))?;

    let mut windows = Vec::new();
    for entry in entries {
        let limit_name = entry
            .get("limit_name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .ok_or(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField,
            ))?;
        let rate_limit = entry
            .get("rate_limit")
            .filter(|value| !value.is_null())
            .ok_or(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField,
            ))?;
        for key in ["primary_window", "secondary_window"] {
            if let Some(window_value) = rate_limit.get(key).filter(|value| !value.is_null()) {
                windows.push(supplemental_window(limit_name, window_value, observed_at)?);
            }
        }
    }
    Ok(windows)
}

/// One `additional_rate_limits` entry's window, scoped to the feature or
/// model it names — the same relationship Anthropic's `weekly_scoped` limits
/// have to its account-wide ones, down to the `weekly-<slug>` id scheme.
fn supplemental_window(
    limit_name: &str,
    value: &Value,
    observed_at: OffsetDateTime,
) -> Result<UsageWindow, ProviderUsageError> {
    let used_percent = read_used_percent(value)?;
    let window_seconds = window_duration_seconds(value)?;
    let weekly = window_seconds == WEEKLY_WINDOW_SECONDS;
    let slug = slugify(limit_name);
    let id = if weekly {
        format!("weekly-{slug}")
    } else {
        format!("{slug}-{}m", window_seconds / 60)
    };

    Ok(UsageWindow {
        id,
        role: WindowRole::Supplemental,
        kind: if weekly {
            UsageWindowKind::Weekly
        } else {
            UsageWindowKind::Rolling
        },
        scope: UsageScope::Model(limit_name.to_string()),
        used_percent: Some(used_percent),
        starts_at: None,
        resets_at: reset_at(value, observed_at, window_seconds, used_percent)?,
        authoritative: true,
    })
}

fn read_used_percent(value: &Value) -> Result<f64, ProviderUsageError> {
    used_percent(value.get("used_percent").and_then(Value::as_f64))?.ok_or(
        ProviderUsageError::Schema(SchemaReason::MissingRequiredField),
    )
}

fn window_duration_seconds(value: &Value) -> Result<i64, ProviderUsageError> {
    value
        .get("limit_window_seconds")
        .and_then(Value::as_f64)
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(|seconds| seconds.round() as i64)
        .ok_or(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ))
}

/// A window's `reset_at`, with the sliding-reset discard applied — see the
/// module doc.
fn reset_at(
    value: &Value,
    observed_at: OffsetDateTime,
    window_seconds: i64,
    used_percent: f64,
) -> Result<Option<OffsetDateTime>, ProviderUsageError> {
    Ok(epoch_seconds(value.get("reset_at"))?.filter(|reset| {
        !is_sliding_reset_projection(*reset, observed_at, window_seconds, used_percent)
    }))
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

/// Whether a stated reset is a rolling projection rather than a committed
/// one: unused (`used_percent == 0`) and landing within two minutes of
/// exactly `observed_at + window_seconds` — the shape a server produces when
/// it is reporting "a window this long, currently empty" rather than an
/// actual scheduled reset. Keeping a timestamp like that would let the
/// runway math treat a rolling estimate as a hard deadline.
///
/// Shared with the `codex_app_server` fallback module, which reads the same
/// account's rate limits over a different protocol and meets the identical
/// projection shape there.
pub fn is_sliding_reset_projection(
    reset: OffsetDateTime,
    observed_at: OffsetDateTime,
    window_seconds: i64,
    used_percent: f64,
) -> bool {
    if used_percent != 0.0 {
        return false;
    }
    let projected = observed_at + time::Duration::seconds(window_seconds);
    (reset - projected).abs() <= time::Duration::minutes(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(seconds).expect("valid timestamp")
    }

    const NOW: i64 = 1_800_000_000;

    const BOTH_WINDOWS: &str = r#"{
      "plan_type": "plus",
      "rate_limit": {
        "primary_window": {"used_percent": 42, "limit_window_seconds": 18000, "reset_at": 1800003600},
        "secondary_window": {"used_percent": 71, "limit_window_seconds": 604800, "reset_at": 1800500000}
      }
    }"#;

    #[test]
    fn the_weekly_window_is_identified_by_its_own_duration_not_its_key_name() {
        let usage = parse_wham_usage(BOTH_WINDOWS, at(NOW)).expect("parses");
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
    fn plan_type_is_read_from_the_response_root() {
        assert_eq!(
            parse_wham_usage(BOTH_WINDOWS, at(NOW))
                .expect("parses")
                .plan
                .as_deref(),
            Some("plus")
        );
    }

    #[test]
    fn a_root_level_plan_type_is_preferred_over_a_nested_one() {
        let both = r#"{
          "plan_type": "pro",
          "rate_limit": {"plan_type": "plus", "primary_window":
            {"used_percent": 10, "limit_window_seconds": 18000}}
        }"#;
        assert_eq!(
            parse_wham_usage(both, at(NOW))
                .expect("parses")
                .plan
                .as_deref(),
            Some("pro")
        );
    }

    #[test]
    fn an_older_payload_without_a_root_plan_type_falls_back_to_the_nested_one() {
        let nested_only = r#"{
          "rate_limit": {"plan_type": "plus", "primary_window":
            {"used_percent": 10, "limit_window_seconds": 18000}}
        }"#;
        assert_eq!(
            parse_wham_usage(nested_only, at(NOW))
                .expect("parses")
                .plan
                .as_deref(),
            Some("plus")
        );
    }

    /// The real shape a Pro account's endpoint returns: only a primary
    /// (weekly) window, no secondary, and a single per-feature entry in
    /// `additional_rate_limits` whose own reset lands exactly on the
    /// projected sliding-reset boundary — which the discard must clear
    /// without dropping the window itself.
    #[test]
    fn a_pro_accounts_real_world_shape_parses_end_to_end() {
        let observed_at = at(NOW);
        let projected_epoch = (observed_at + time::Duration::seconds(604_800)).unix_timestamp();
        let payload = format!(
            r#"{{
              "plan_type": "pro",
              "rate_limit": {{
                "allowed": true, "limit_reached": false,
                "primary_window": {{"used_percent": 17, "limit_window_seconds": 604800, "reset_at": 1800168831}},
                "secondary_window": null
              }},
              "additional_rate_limits": [
                {{"limit_name": "Nightjar Spark", "metered_feature": "codex_nightjar",
                  "rate_limit": {{"allowed": true, "limit_reached": false,
                    "primary_window": {{"used_percent": 0, "limit_window_seconds": 604800, "reset_at": {projected_epoch}}},
                    "secondary_window": null}}}}
              ]
            }}"#
        );
        let usage = parse_wham_usage(&payload, observed_at).expect("parses");
        assert_eq!(usage.plan.as_deref(), Some("pro"));
        assert_eq!(usage.windows.len(), 2);

        assert_eq!(usage.windows[0].id, "seven-day");
        assert_eq!(usage.windows[0].role, WindowRole::PrimaryLong);
        assert_eq!(usage.windows[0].used_percent, Some(17.0));

        assert_eq!(usage.windows[1].id, "weekly-nightjar-spark");
        assert_eq!(usage.windows[1].role, WindowRole::Supplemental);
        assert_eq!(
            usage.windows[1].scope,
            UsageScope::Model("Nightjar Spark".to_string())
        );
        assert_eq!(usage.windows[1].used_percent, Some(0.0));
        // The sliding-reset discard fired: the stated reset landed on the
        // projected boundary of an unused window, so it is cleared.
        assert_eq!(usage.windows[1].resets_at, None);
    }

    #[test]
    fn a_window_whose_duration_matches_neither_known_length_gets_a_duration_derived_id() {
        // A duration close to, but not exactly, a week must not be guessed at
        // and must not silently claim `five-hour`'s exact-length semantics
        // either.
        let almost_weekly = r#"{"rate_limit": {"primary_window":
          {"used_percent": 10, "limit_window_seconds": 604799, "reset_at": null}}}"#;
        let usage = parse_wham_usage(almost_weekly, at(NOW)).expect("parses");
        assert_eq!(usage.windows[0].id, "rolling-10079m");
        assert_eq!(usage.windows[0].role, WindowRole::PrimaryShort);
    }

    #[test]
    fn a_missing_rate_limit_envelope_is_a_schema_error() {
        assert_eq!(
            parse_wham_usage("{}", at(NOW)),
            Err(ProviderUsageError::Schema(SchemaReason::MissingEnvelope))
        );
        assert_eq!(
            parse_wham_usage(r#"{"rate_limit": null}"#, at(NOW)),
            Err(ProviderUsageError::Schema(SchemaReason::MissingEnvelope))
        );
    }

    #[test]
    fn a_rate_limit_object_with_neither_window_is_a_schema_error() {
        assert_eq!(
            parse_wham_usage(r#"{"rate_limit": {"plan_type": "plus"}}"#, at(NOW)),
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
            parse_wham_usage(absurd, at(NOW)),
            Err(ProviderUsageError::Schema(SchemaReason::InvalidValue))
        );
    }

    #[test]
    fn epoch_seconds_resets_are_read_directly() {
        let usage = parse_wham_usage(BOTH_WINDOWS, at(NOW)).expect("parses");
        assert_eq!(
            usage.windows[0].resets_at,
            OffsetDateTime::from_unix_timestamp(1_800_003_600i64).ok()
        );
    }

    #[test]
    fn a_missing_plan_type_is_absent_rather_than_an_empty_string() {
        let no_plan = r#"{"rate_limit": {"primary_window":
          {"used_percent": 10, "limit_window_seconds": 18000}}}"#;
        assert_eq!(
            parse_wham_usage(no_plan, at(NOW)).expect("parses").plan,
            None
        );
    }

    /* ---------------------------------------------------------------------
     * `additional_rate_limits`
     * ------------------------------------------------------------------- */

    #[test]
    fn a_missing_additional_rate_limits_key_is_the_older_payload_shape_not_an_error() {
        let usage = parse_wham_usage(BOTH_WINDOWS, at(NOW)).expect("parses");
        assert_eq!(usage.windows.len(), 2);
    }

    #[test]
    fn a_null_additional_rate_limits_key_is_also_fine() {
        let with_null = r#"{"rate_limit": {"primary_window":
          {"used_percent": 10, "limit_window_seconds": 18000}}, "additional_rate_limits": null}"#;
        let usage = parse_wham_usage(with_null, at(NOW)).expect("parses");
        assert_eq!(usage.windows.len(), 1);
    }

    #[test]
    fn a_weekly_additional_limit_gets_the_same_id_scheme_as_anthropics_weekly_scoped_windows() {
        let payload = r#"{"rate_limit": {"primary_window":
          {"used_percent": 10, "limit_window_seconds": 18000}},
          "additional_rate_limits": [
            {"limit_name": "Fable Reasoning", "rate_limit": {"primary_window":
              {"used_percent": 5, "limit_window_seconds": 604800}}}
          ]}"#;
        let usage = parse_wham_usage(payload, at(NOW)).expect("parses");
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[1].id, "weekly-fable-reasoning");
        assert_eq!(usage.windows[1].role, WindowRole::Supplemental);
        assert_eq!(
            usage.windows[1].scope,
            UsageScope::Model("Fable Reasoning".to_string())
        );
        assert_eq!(usage.windows[1].kind, UsageWindowKind::Weekly);
    }

    #[test]
    fn a_non_weekly_additional_limit_gets_a_slug_and_minutes_id() {
        let payload = r#"{"rate_limit": {"primary_window":
          {"used_percent": 10, "limit_window_seconds": 18000}},
          "additional_rate_limits": [
            {"limit_name": "Fable Mini", "rate_limit": {"primary_window":
              {"used_percent": 5, "limit_window_seconds": 1800}}}
          ]}"#;
        let usage = parse_wham_usage(payload, at(NOW)).expect("parses");
        assert_eq!(usage.windows[1].id, "fable-mini-30m");
        assert_eq!(usage.windows[1].kind, UsageWindowKind::Rolling);
    }

    #[test]
    fn a_null_secondary_window_inside_an_additional_limit_contributes_nothing() {
        let payload = r#"{"rate_limit": {"primary_window":
          {"used_percent": 10, "limit_window_seconds": 18000}},
          "additional_rate_limits": [
            {"limit_name": "Fable", "rate_limit": {
              "primary_window": {"used_percent": 5, "limit_window_seconds": 604800},
              "secondary_window": null
            }}
          ]}"#;
        let usage = parse_wham_usage(payload, at(NOW)).expect("parses");
        assert_eq!(usage.windows.len(), 2);
    }

    #[test]
    fn an_additional_limit_missing_its_name_fails_the_whole_payload() {
        let payload = r#"{"rate_limit": {"primary_window":
          {"used_percent": 10, "limit_window_seconds": 18000}},
          "additional_rate_limits": [
            {"rate_limit": {"primary_window": {"used_percent": 5, "limit_window_seconds": 604800}}}
          ]}"#;
        assert_eq!(
            parse_wham_usage(payload, at(NOW)),
            Err(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField
            ))
        );
    }

    #[test]
    fn an_additional_limit_missing_its_rate_limit_object_fails_the_whole_payload() {
        let payload = r#"{"rate_limit": {"primary_window":
          {"used_percent": 10, "limit_window_seconds": 18000}},
          "additional_rate_limits": [
            {"limit_name": "Fable"}
          ]}"#;
        assert_eq!(
            parse_wham_usage(payload, at(NOW)),
            Err(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField
            ))
        );
    }

    #[test]
    fn additional_rate_limits_that_is_not_an_array_fails_the_whole_payload() {
        let payload = r#"{"rate_limit": {"primary_window":
          {"used_percent": 10, "limit_window_seconds": 18000}},
          "additional_rate_limits": {"not": "an array"}}"#;
        assert_eq!(
            parse_wham_usage(payload, at(NOW)),
            Err(ProviderUsageError::Schema(SchemaReason::InvalidValue))
        );
    }

    /* ---------------------------------------------------------------------
     * The sliding-reset discard
     * ------------------------------------------------------------------- */

    #[test]
    fn a_zero_usage_reset_landing_on_the_projected_boundary_is_dropped() {
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
    fn the_discard_applies_to_an_account_wide_window_end_to_end() {
        let observed_at = at(NOW);
        let projected_epoch = (observed_at + time::Duration::seconds(18_000)).unix_timestamp();
        let payload = format!(
            r#"{{"rate_limit": {{"primary_window":
              {{"used_percent": 0, "limit_window_seconds": 18000, "reset_at": {projected_epoch}}}}}}}"#
        );
        let usage = parse_wham_usage(&payload, observed_at).expect("parses");
        assert_eq!(usage.windows[0].resets_at, None);
        assert_eq!(usage.windows[0].used_percent, Some(0.0));
    }
}
