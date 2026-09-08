//! The Anthropic usage payload, read strictly.
//!
//! This module contains only the quota parsers.
//!
//! # What this parses
//!
//! The body `GET https://api.anthropic.com/api/oauth/usage` returns.
//! [`sources::anthropic_fetch`](super::sources::anthropic_fetch) calls
//! [`parse_usage`] on that response body directly, so a fixture in a test
//! does not need to pretend to be an HTTP response.
//! [`sources::claude_config_cache`](super::sources::claude_config_cache)
//! calls [`parse_usage_value`] instead: the Claude CLI caches the identical
//! `utilization` object inside a much larger JSON document, already parsed,
//! so [`parse_usage_value`] takes the [`Value`] straight from that document
//! rather than re-serialising it back to a string first.
//!
//! # What the payload looks like
//!
//! Newer payloads carry a `limits` array, each entry naming its `kind`
//! (`session`, `weekly_all`, `weekly_scoped`), a `percent` or `utilization`
//! value, a `resets_at`, and — for a model-scoped weekly limit — a
//! `scope.model.display_name`. Older ones carry flat `five_hour` and
//! `seven_day` objects with a `utilization` key instead. The array wins when
//! present; the flat keys are the fallback, never a supplement, so one
//! payload never yields the same window twice.
//!
//! Two more shape inconsistencies are absorbed rather than treated as errors.
//! A `utilization` figure in the limits array may be a fraction or a percent.
//! The explicit `percent` and legacy flat `utilization` fields are percentages.
//! Also, `resets_at` may be epoch seconds or an RFC 3339 string.

use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::model::{
    CreditBalance, ProviderUsageError, SchemaReason, SupplementalUsage, UsageScope, UsageWindow,
    UsageWindowKind, WindowRole,
};
use super::normalize::{slugify, used_percent, used_percent_or_fraction};

/// What a well-formed payload yielded.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnthropicUsage {
    pub windows: Vec<UsageWindow>,
    pub supplemental: Option<SupplementalUsage>,
}

/// The closed, privacy-safe summary of Claude's limit-reset experiment data.
///
/// Every value is a fixed category. The provider response and its timestamps
/// never leave the machine.
#[cfg(feature = "analytics")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitResetDiagnostic {
    pub request_outcome: &'static str,
    pub usage_band: &'static str,
    pub response_shape: &'static str,
    pub eligibility: Option<&'static str>,
    pub ineligible_reason: Option<&'static str>,
    pub experiment: Option<&'static str>,
    pub arm: Option<&'static str>,
    pub availability: Option<&'static str>,
    pub resets_per_week: Option<&'static str>,
    pub next_available: Option<&'static str>,
}

/// Reduce the `juniper_tide` response member to a closed diagnostic.
#[cfg(feature = "analytics")]
pub fn parse_limit_reset_diagnostic(input: &str) -> LimitResetDiagnostic {
    let Ok(value) = serde_json::from_str::<Value>(input) else {
        return empty_limit_reset_diagnostic("success", "invalid_json");
    };
    let Some(envelope) = value.as_object() else {
        return empty_limit_reset_diagnostic("success", "malformed_envelope");
    };
    let usage_band = usage_band(&value);
    let Some(tide) = envelope.get("juniper_tide") else {
        return empty_limit_reset_diagnostic_with_band("success", usage_band, "missing");
    };
    if tide.is_null() {
        return empty_limit_reset_diagnostic_with_band("success", usage_band, "null");
    }
    let Some(tide) = tide.as_object() else {
        return empty_limit_reset_diagnostic_with_band("success", usage_band, "malformed");
    };

    LimitResetDiagnostic {
        request_outcome: "success",
        usage_band,
        response_shape: "object",
        eligibility: Some(bool_state(tide.get("eligible"), "eligible", "ineligible")),
        ineligible_reason: Some(reason_state(tide.get("ineligible_reason"))),
        experiment: Some(bool_state(
            tide.get("in_experiment"),
            "in_experiment",
            "not_in_experiment",
        )),
        arm: Some(string_state(tide.get("arm"), |value| match value {
            "reset" => "reset",
            "control" => "control",
            _ => "other",
        })),
        availability: Some(bool_state(
            tide.get("available"),
            "available",
            "unavailable",
        )),
        resets_per_week: Some(number_state(tide.get("resets_per_week"))),
        next_available: Some(presence_state(tide.get("next_available_at"))),
    }
}

#[cfg(feature = "analytics")]
pub fn empty_limit_reset_diagnostic(
    request_outcome: &'static str,
    response_shape: &'static str,
) -> LimitResetDiagnostic {
    empty_limit_reset_diagnostic_with_band(request_outcome, "unknown", response_shape)
}

#[cfg(feature = "analytics")]
fn empty_limit_reset_diagnostic_with_band(
    request_outcome: &'static str,
    usage_band: &'static str,
    response_shape: &'static str,
) -> LimitResetDiagnostic {
    LimitResetDiagnostic {
        request_outcome,
        usage_band,
        response_shape,
        eligibility: None,
        ineligible_reason: None,
        experiment: None,
        arm: None,
        availability: None,
        resets_per_week: None,
        next_available: None,
    }
}

#[cfg(feature = "analytics")]
fn usage_band(value: &Value) -> &'static str {
    let Ok(usage) = parse_usage_value(value) else {
        return "unknown";
    };
    let percent = usage
        .windows
        .iter()
        .find(|window| matches!(window.role, WindowRole::PrimaryShort))
        .and_then(|window| window.used_percent);
    super::model::band_for_percent(percent)
}

#[cfg(feature = "analytics")]
fn missing_null_or_malformed(value: Option<&Value>) -> Option<&'static str> {
    match value {
        None => Some("missing"),
        Some(value) if value.is_null() => Some("null"),
        Some(_) => None,
    }
}

#[cfg(feature = "analytics")]
fn bool_state(
    value: Option<&Value>,
    when_true: &'static str,
    when_false: &'static str,
) -> &'static str {
    if let Some(state) = missing_null_or_malformed(value) {
        return state;
    }
    match value.and_then(Value::as_bool) {
        Some(true) => when_true,
        Some(false) => when_false,
        None => "malformed",
    }
}

#[cfg(feature = "analytics")]
fn string_state(
    value: Option<&Value>,
    classify: impl FnOnce(&str) -> &'static str,
) -> &'static str {
    if let Some(state) = missing_null_or_malformed(value) {
        return state;
    }
    match value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        Some(value) => classify(value),
        None => "malformed",
    }
}

#[cfg(feature = "analytics")]
fn reason_state(value: Option<&Value>) -> &'static str {
    string_state(value, |reason| match reason {
        "tier" => "tier",
        "tenure" => "tenure",
        "surface" => "surface",
        "mobile" => "mobile",
        "cli_version" => "cli_version",
        "not_at_wall" => "not_at_wall",
        "weekly_limit" => "weekly_limit",
        "no_weekly_limit" => "no_weekly_limit",
        "other_experiment" => "other_experiment",
        "extra_usage" => "extra_usage",
        "unavailable" => "unavailable",
        "unknown" => "unknown",
        _ => "other",
    })
}

#[cfg(feature = "analytics")]
fn number_state(value: Option<&Value>) -> &'static str {
    if let Some(state) = missing_null_or_malformed(value) {
        return state;
    }
    match value.and_then(Value::as_u64) {
        Some(0) => "0",
        Some(1) => "1",
        Some(_) => "2_plus",
        None => "malformed",
    }
}

#[cfg(feature = "analytics")]
fn presence_state(value: Option<&Value>) -> &'static str {
    if let Some(state) = missing_null_or_malformed(value) {
        return state;
    }
    let Some(value) = value else {
        return "missing";
    };
    if value
        .as_str()
        .is_some_and(|raw| OffsetDateTime::parse(raw, &Rfc3339).is_ok())
        || value.as_f64().is_some_and(|seconds| {
            seconds.is_finite()
                && OffsetDateTime::from_unix_timestamp(seconds.trunc() as i64).is_ok()
        })
    {
        "present"
    } else {
        "malformed"
    }
}

/// Parse an Anthropic usage payload into windows and supplemental metering.
///
/// Fails rather than returning a partial reading: a payload we only half
/// understand is exactly the input that would put a confident, wrong meter on
/// screen. An envelope carrying neither a window nor a supplemental meter is
/// also a failure — it means the shape changed under us, which is worth
/// surfacing rather than rendering as "no limits".
pub fn parse_usage(input: &str) -> Result<AnthropicUsage, ProviderUsageError> {
    let value: Value = serde_json::from_str(input)
        .map_err(|_| ProviderUsageError::Schema(SchemaReason::InvalidJson))?;
    parse_usage_value(&value)
}

/// Parse an already-decoded usage payload. Same rules as [`parse_usage`],
/// which is `serde_json::from_str` followed by this function — see the
/// module doc for the other caller this split serves.
pub fn parse_usage_value(value: &Value) -> Result<AnthropicUsage, ProviderUsageError> {
    if !value.is_object() {
        return Err(ProviderUsageError::Schema(SchemaReason::MissingEnvelope));
    }

    let mut windows = Vec::new();
    if let Some(limits) = value.get("limits").filter(|limits| !limits.is_null()) {
        let limits = limits
            .as_array()
            .ok_or(ProviderUsageError::Schema(SchemaReason::InvalidValue))?;
        for limit in limits {
            let name = limit
                .get("kind")
                .or_else(|| limit.get("name"))
                .and_then(Value::as_str)
                .ok_or(ProviderUsageError::Schema(
                    SchemaReason::MissingRequiredField,
                ))?;
            let (role, kind) = window_semantics(name);
            windows.push(window(name, role, kind, limit)?);
        }
    }

    // Only when the array was absent. A payload carrying both would otherwise
    // report the five-hour window twice under two ids.
    if windows.is_empty() {
        for (key, role, kind) in [
            (
                "five_hour",
                WindowRole::PrimaryShort,
                UsageWindowKind::Rolling,
            ),
            (
                "seven_day",
                WindowRole::PrimaryLong,
                UsageWindowKind::Weekly,
            ),
        ] {
            if let Some(limit) = value.get(key).filter(|limit| !limit.is_null()) {
                windows.push(window(key, role, kind, limit)?);
            }
        }
    }

    let supplemental = value
        .get("extra_usage")
        .filter(|extra| !extra.is_null())
        .map(extra_usage)
        .transpose()?;

    if windows.is_empty() && supplemental.is_none() {
        return Err(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ));
    }

    Ok(AnthropicUsage {
        windows,
        supplemental,
    })
}

/// The role and reset behaviour a provider window name implies.
///
/// An unrecognized name keeps its own spelling in both slots rather than
/// being forced into the nearest known one — a new limit the provider adds
/// should show up as itself, not as a mislabelled weekly.
fn window_semantics(name: &str) -> (WindowRole, UsageWindowKind) {
    match name {
        "session" | "five_hour" => (WindowRole::PrimaryShort, UsageWindowKind::Rolling),
        "weekly_all" | "seven_day" => (WindowRole::PrimaryLong, UsageWindowKind::Weekly),
        "weekly_scoped" => (WindowRole::Supplemental, UsageWindowKind::Weekly),
        other => (
            WindowRole::Other(other.to_string()),
            UsageWindowKind::Other(other.to_string()),
        ),
    }
}

fn window(
    name: &str,
    role: WindowRole,
    kind: UsageWindowKind,
    value: &Value,
) -> Result<UsageWindow, ProviderUsageError> {
    let scope = value
        .pointer("/scope/model/display_name")
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .map_or(UsageScope::Account, |model| {
            UsageScope::Model(model.to_string())
        });

    // Ids are derived from meaning, not from the provider's spelling, so the
    // same window keeps one identity across both payload shapes. That
    // identity is what the milestone ledger and the sample history join on.
    let id = match (&role, &kind, &scope) {
        (WindowRole::PrimaryShort, _, UsageScope::Account) => "five-hour".to_string(),
        (WindowRole::PrimaryLong, UsageWindowKind::Weekly, UsageScope::Account) => {
            "seven-day".to_string()
        }
        (WindowRole::Supplemental, UsageWindowKind::Weekly, UsageScope::Model(model)) => {
            format!("weekly-{}", slugify(model))
        }
        _ => name.replace('_', "-"),
    };

    Ok(UsageWindow {
        id,
        role,
        kind,
        scope,
        used_percent: window_used_percent(name, value)?,
        starts_at: None,
        resets_at: reset_at(value.get("resets_at"))?,
        authoritative: true,
    })
}

/// Read the unit from the field and payload shape instead of the value.
fn window_used_percent(name: &str, value: &Value) -> Result<Option<f64>, ProviderUsageError> {
    if let Some(percent) = value.get("percent") {
        return used_percent(percent.as_f64());
    }

    let utilization = value.get("utilization").and_then(Value::as_f64);
    if matches!(name, "five_hour" | "seven_day") {
        return used_percent(utilization);
    }
    used_percent_or_fraction(utilization)
}

/// Metered usage beyond the subscription allowance.
///
/// `is_enabled` is required, because "the reader has this switched off" and
/// "we could not tell" produce different copy and must not be confused. Raw
/// amounts without a currency fail: a bare number that might be dollars,
/// cents, or credits is not something to put next to a spend figure.
fn extra_usage(value: &Value) -> Result<SupplementalUsage, ProviderUsageError> {
    let enabled =
        value
            .get("is_enabled")
            .and_then(Value::as_bool)
            .ok_or(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField,
            ))?;
    let used = nonnegative(value.get("used_credits"))?;
    let limit = nonnegative(value.get("monthly_limit"))?;
    let currency = value
        .get("currency")
        .and_then(Value::as_str)
        .filter(|currency| !currency.is_empty())
        .map(str::to_owned);
    if (used.is_some() || limit.is_some()) && currency.is_none() {
        return Err(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ));
    }

    Ok(SupplementalUsage {
        enabled,
        used_percent: used_percent(value.get("utilization").and_then(Value::as_f64))?,
        balance: (used.is_some() || limit.is_some()).then(|| CreditBalance {
            used,
            remaining: match (used, limit) {
                (Some(used), Some(limit)) => Some((limit - used).max(0.0)),
                _ => None,
            },
            limit,
            currency,
        }),
    })
}

fn nonnegative(value: Option<&Value>) -> Result<Option<f64>, ProviderUsageError> {
    let Some(value) = value else { return Ok(None) };
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(Some)
        .ok_or(ProviderUsageError::Schema(SchemaReason::InvalidValue))
}

/// A reset timestamp, in either shape the endpoint uses: an RFC 3339 string
/// (the `limits` array) or epoch seconds (the legacy flat objects, and
/// occasionally the array too). Anything else — the wrong JSON type, a string
/// that is neither shape — rejects the payload rather than dropping the
/// field, the same as every other malformed value in this module.
fn reset_at(value: Option<&Value>) -> Result<Option<OffsetDateTime>, ProviderUsageError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let invalid = || ProviderUsageError::Schema(SchemaReason::InvalidValue);
    if let Some(raw) = value.as_str() {
        return OffsetDateTime::parse(raw, &Rfc3339)
            .map(Some)
            .map_err(|_| invalid());
    }
    if let Some(seconds) = value.as_f64().filter(|seconds| seconds.is_finite()) {
        return OffsetDateTime::from_unix_timestamp(seconds.trunc() as i64)
            .map(Some)
            .map_err(|_| invalid());
    }
    Err(invalid())
}
