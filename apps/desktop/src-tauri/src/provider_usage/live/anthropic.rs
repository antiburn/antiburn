// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The Anthropic usage payload, read strictly.
//!
//! Adapted from the private app under D-023 (allowlist rule
//! `usage-limit-domain`): the quota parsers only. The attribution parsers,
//! the status-line parser, and every pricing-signal coupling stay behind.
//!
//! # What this parses
//!
//! The body `GET https://api.anthropic.com/api/oauth/usage` returns —
//! [`sources::anthropic_fetch`](super::sources::anthropic_fetch) is this
//! module's only caller. It takes a bare `&str` regardless, so a fixture in a
//! test does not need to pretend to be an HTTP response.
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
//! Two more shape inconsistencies are absorbed rather than treated as errors,
//! because the provider is not consistent about either across its own
//! payload family: a usage figure may be a `0..=1` fraction or an
//! already-multiplied percent (see
//! [`normalize::used_percent_or_fraction`](super::normalize::used_percent_or_fraction)),
//! and `resets_at` may be epoch seconds or an RFC 3339 string.

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
        used_percent: used_percent_or_fraction(
            value
                .get("percent")
                .or_else(|| value.get("utilization"))
                .and_then(Value::as_f64),
        )?,
        starts_at: None,
        resets_at: reset_at(value.get("resets_at"))?,
        authoritative: true,
    })
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
