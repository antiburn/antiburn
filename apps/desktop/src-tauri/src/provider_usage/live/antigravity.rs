//! Strict parsers for Antigravity Code Assist account and quota responses.
//!
//! These functions perform no I/O. They retain only provider-stated plan,
//! project, credit, quota fraction, and reset facts. Missing fractions remain
//! unknown, while malformed present values reject the complete response.

use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::model::{
    CreditBalance, ProviderUsageError, SchemaReason, SupplementalUsage, UsageScope, UsageWindow,
    UsageWindowKind, WindowRole,
};

const MAX_MODELS: usize = 128;
const MAX_MODEL_WINDOWS: usize = 512;

#[derive(Debug, PartialEq)]
pub struct CodeAssistAccount {
    pub project: String,
    pub plan: Option<String>,
    pub tier: Option<String>,
    pub credits: Option<SupplementalUsage>,
}

#[derive(Debug, PartialEq)]
pub struct QuotaSummary {
    pub windows: Vec<UsageWindow>,
}

#[derive(Debug, PartialEq)]
pub struct LocalStatus {
    pub account: Option<String>,
    pub plan: Option<String>,
    pub tier: Option<String>,
    pub windows: Vec<UsageWindow>,
}

/// Parse `loadCodeAssist` and require its managed project.
pub fn parse_load_code_assist(input: &str) -> Result<CodeAssistAccount, ProviderUsageError> {
    let root = object(input)?;
    let project = string_at(&root, &["cloudaicompanionProject"])
        .or_else(|| string_at(&root, &["cloudaicompanionProject", "id"]))
        .filter(|value| !value.is_empty())
        .ok_or(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ))?;

    let plan = first_string(
        &root,
        &[
            &["paidTier", "name"],
            &["currentTier", "name"],
            &["planInfo", "planType"],
        ],
    );
    let tier = first_string(
        &root,
        &[
            &["paidTier", "id"],
            &["currentTier", "id"],
            &["planInfo", "tier"],
        ],
    );
    let limit = optional_number(
        root.get("planInfo")
            .and_then(|v| v.get("monthlyPromptCredits")),
    )?;
    let remaining = optional_number(root.get("availablePromptCredits"))?;
    if limit.is_some_and(|value| value < 0.0) || remaining.is_some_and(|value| value < 0.0) {
        return Err(invalid());
    }
    let used = match (limit, remaining) {
        (Some(limit), Some(remaining)) if remaining <= limit => Some(limit - remaining),
        (Some(_), Some(_)) => return Err(invalid()),
        _ => None,
    };
    let credits = (limit.is_some() || remaining.is_some()).then(|| SupplementalUsage {
        enabled: true,
        used_percent: match (used, limit) {
            (Some(used), Some(limit)) if limit > 0.0 => Some(used / limit * 100.0),
            _ => None,
        },
        balance: Some(CreditBalance {
            used,
            remaining,
            limit,
            currency: None,
        }),
    });

    Ok(CodeAssistAccount {
        project: project.to_owned(),
        plan: plan.map(str::to_owned),
        tier: tier.map(str::to_owned),
        credits,
    })
}

/// Parse the recognized semantic pools from `retrieveUserQuotaSummary`.
pub fn parse_quota_summary(input: &str) -> Result<QuotaSummary, ProviderUsageError> {
    let root = object(input)?;
    let envelope = ["quotaSummary", "result", "response"]
        .iter()
        .find_map(|key| root.get(*key).and_then(Value::as_object))
        .unwrap_or(&root);
    let groups =
        envelope
            .get("groups")
            .and_then(Value::as_array)
            .ok_or(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField,
            ))?;

    let mut slots: [Option<UsageWindow>; 4] = [None, None, None, None];
    for group in groups {
        let group = group.as_object().ok_or_else(invalid)?;
        let group_name = first_string(group, &[&["displayName"], &["name"], &["groupId"], &["id"]]);
        let buckets = group
            .get("buckets")
            .and_then(Value::as_array)
            .ok_or_else(invalid)?;
        for bucket in buckets {
            let bucket = bucket.as_object().ok_or_else(invalid)?;
            let Some(index) = semantic_slot(group_name, bucket) else {
                continue;
            };
            let remaining = remaining_fraction(bucket)?;
            let reset = optional_reset(bucket.get("resetTime"))?;
            let (id, role, kind, scope) = slot_model(index);
            let window = UsageWindow {
                id: id.into(),
                role,
                kind,
                scope,
                used_percent: remaining.map(|value| (1.0 - value) * 100.0),
                starts_at: None,
                resets_at: reset,
                authoritative: true,
            };
            if let Some(existing) = &mut slots[index] {
                fold_window(existing, window);
            } else {
                slots[index] = Some(window);
            }
        }
    }
    if slots.iter().all(Option::is_none) {
        return Err(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ));
    }
    Ok(QuotaSummary {
        windows: slots.into_iter().flatten().collect(),
    })
}

/// Parse the model quota windows from `fetchAvailableModels`.
pub fn parse_available_models(input: &str) -> Result<QuotaSummary, ProviderUsageError> {
    let root = object(input)?;
    let models =
        root.get("models")
            .and_then(Value::as_object)
            .ok_or(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField,
            ))?;
    if models.is_empty() || models.len() > MAX_MODELS {
        return Err(invalid());
    }

    let mut windows: Vec<UsageWindow> = Vec::new();
    for (model_id, model) in models {
        let model = model.as_object().ok_or_else(invalid)?;
        let display_name = optional_string(model.get("displayName"))?.unwrap_or(model_id);
        let mut quotas = Vec::new();
        collect_quota_values(model.get("quotaInfo"), None, &mut quotas)?;
        collect_quota_values(model.get("quotaInfos"), None, &mut quotas)?;
        if let Some(by_tier) = model
            .get("quotaInfoByTier")
            .filter(|value| !value.is_null())
        {
            let by_tier = by_tier.as_object().ok_or_else(invalid)?;
            for (tier, value) in by_tier {
                collect_quota_values(Some(value), Some(tier.as_str()), &mut quotas)?;
            }
        }

        for (quota, inherited_tier) in quotas {
            let quota = quota.as_object().ok_or_else(invalid)?;
            let tier = optional_string(quota.get("tier"))?.or(inherited_tier);
            let window_id = optional_string(quota.get("windowId"))?;
            let window_label = optional_string(quota.get("windowLabel"))?;
            let remaining = remaining_fraction(quota)?;
            let resets_at = optional_reset(quota.get("resetTime"))?;
            let mut label = display_name.to_owned();
            if let Some(tier) = tier {
                label.push_str(" (");
                label.push_str(tier);
                label.push(')');
            }
            let mut id = format!("antigravity-model-{}", stable_slug(model_id));
            if let Some(tier) = tier {
                id.push('-');
                id.push_str(&stable_slug(tier));
            }
            if let Some(window_id) = window_id {
                id.push('-');
                id.push_str(&stable_slug(window_id));
            }
            let weekly = window_id
                .into_iter()
                .chain(window_label)
                .any(|value| value.to_ascii_lowercase().contains("week"));
            let window = UsageWindow {
                id,
                role: WindowRole::Supplemental,
                kind: if weekly {
                    UsageWindowKind::Weekly
                } else {
                    UsageWindowKind::Rolling
                },
                scope: UsageScope::Model(label),
                used_percent: remaining.map(|value| (1.0 - value) * 100.0),
                starts_at: None,
                resets_at,
                authoritative: true,
            };
            if let Some(existing) = windows.iter_mut().find(|existing| existing.id == window.id) {
                fold_window(existing, window);
            } else {
                if windows.len() == MAX_MODEL_WINDOWS {
                    return Err(invalid());
                }
                windows.push(window);
            }
        }
    }
    if windows.is_empty() {
        return Err(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ));
    }
    Ok(QuotaSummary { windows })
}

/// Merge model fallbacks into the shared pools shown by Antigravity.
pub fn merge_windows(
    mut windows: Vec<UsageWindow>,
    additional: Vec<UsageWindow>,
) -> Vec<UsageWindow> {
    let supplied: [bool; 4] = std::array::from_fn(|index| {
        let id = slot_model(index).0;
        windows.iter().any(|window| window.id == id)
    });
    for window in additional {
        let Some(index) = fallback_slot(&window) else {
            continue;
        };
        if supplied[index] {
            continue;
        }
        let (id, role, kind, scope) = slot_model(index);
        let pooled = UsageWindow {
            id: id.into(),
            role,
            kind,
            scope,
            used_percent: window.used_percent,
            starts_at: window.starts_at,
            resets_at: window.resets_at,
            authoritative: window.authoritative,
        };
        if let Some(existing) = windows.iter_mut().find(|existing| existing.id == pooled.id) {
            fold_window(existing, pooled);
        } else {
            windows.push(pooled);
        }
    }
    windows.sort_by_key(|window| shared_slot(&window.id).unwrap_or(usize::MAX));
    windows
}

/// Parse Antigravity's local `GetUserStatus` response.
pub fn parse_get_user_status(input: &str) -> Result<LocalStatus, ProviderUsageError> {
    let root = object(input)?;
    let status = root
        .get("userStatus")
        .or_else(|| root.get("result").and_then(|value| value.get("userStatus")))
        .and_then(Value::as_object)
        .ok_or(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ))?;
    let configs = status
        .get("cascadeModelConfigData")
        .and_then(|value| value.get("clientModelConfigs"))
        .or_else(|| status.get("clientModelConfigs"))
        .and_then(Value::as_array)
        .ok_or(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ))?;
    if configs.is_empty() || configs.len() > MAX_MODELS {
        return Err(invalid());
    }

    let mut windows: Vec<UsageWindow> = Vec::with_capacity(configs.len());
    for config in configs {
        let config = config.as_object().ok_or_else(invalid)?;
        let Some(quota_value) = config.get("quotaInfo").filter(|value| !value.is_null()) else {
            continue;
        };
        let quota = quota_value.as_object().ok_or_else(invalid)?;
        let label = first_string(
            config,
            &[
                &["label"],
                &["displayName"],
                &["modelOrAlias", "model"],
                &["model"],
            ],
        )
        .ok_or(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ))?;
        let remaining = remaining_fraction(quota)?;
        let resets_at = optional_reset(quota.get("resetTime"))?;
        let stable_name = first_string(
            config,
            &[&["modelOrAlias", "model"], &["model"], &["label"]],
        )
        .unwrap_or(label);
        let id = format!("antigravity-model-{}", stable_slug(stable_name));
        let window = UsageWindow {
            id,
            role: WindowRole::Supplemental,
            kind: UsageWindowKind::Rolling,
            scope: UsageScope::Model(label.to_owned()),
            used_percent: remaining.map(|value| (1.0 - value) * 100.0),
            starts_at: None,
            resets_at,
            authoritative: true,
        };
        if let Some(existing) = windows.iter_mut().find(|existing| existing.id == window.id) {
            fold_window(existing, window);
        } else {
            windows.push(window);
        }
    }
    if windows.is_empty() {
        return Err(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField,
        ));
    }

    Ok(LocalStatus {
        account: first_string(
            status,
            &[&["user", "email"], &["account", "email"], &["email"]],
        )
        .map(str::to_owned),
        plan: first_string(
            status,
            &[
                &["planStatus", "planInfo", "planName"],
                &["planInfo", "planName"],
            ],
        )
        .map(str::to_owned),
        tier: first_string(
            status,
            &[
                &["planStatus", "planInfo", "teamsTier"],
                &["planInfo", "teamsTier"],
            ],
        )
        .map(str::to_owned),
        windows,
    })
}

fn collect_quota_values<'a>(
    value: Option<&'a Value>,
    tier: Option<&'a str>,
    quotas: &mut Vec<(&'a Value, Option<&'a str>)>,
) -> Result<(), ProviderUsageError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    match value {
        Value::Object(_) => quotas.push((value, tier)),
        Value::Array(values) => {
            if quotas.len().saturating_add(values.len()) > MAX_MODEL_WINDOWS {
                return Err(invalid());
            }
            for value in values {
                if !value.is_object() {
                    return Err(invalid());
                }
                quotas.push((value, tier));
            }
        }
        _ => return Err(invalid()),
    }
    if quotas.len() > MAX_MODEL_WINDOWS {
        return Err(invalid());
    }
    Ok(())
}

fn fold_window(existing: &mut UsageWindow, candidate: UsageWindow) {
    existing.used_percent = match (existing.used_percent, candidate.used_percent) {
        (Some(existing), Some(candidate)) => Some(existing.max(candidate)),
        (existing, candidate) => existing.or(candidate),
    };
    existing.resets_at = match (existing.resets_at, candidate.resets_at) {
        (Some(existing), Some(candidate)) => Some(existing.max(candidate)),
        (existing, candidate) => existing.or(candidate),
    };
    existing.starts_at = match (existing.starts_at, candidate.starts_at) {
        (Some(existing), Some(candidate)) => Some(existing.min(candidate)),
        (existing, candidate) => existing.or(candidate),
    };
    existing.authoritative &= candidate.authoritative;
}

fn semantic_slot(group: Option<&str>, bucket: &serde_json::Map<String, Value>) -> Option<usize> {
    let id = first_string(
        bucket,
        &[&["bucketId"], &["id"], &["window"], &["displayName"]],
    )?;
    let text = format!("{} {id}", group.unwrap_or_default()).to_ascii_lowercase();
    let gemini = text.contains("gemini");
    let third_party = text.contains("3p")
        || text.contains("third")
        || text.contains("claude")
        || text.contains("gpt");
    let weekly = text.contains("weekly") || text.contains("week") || text.contains("7d");
    let five_hour = text.contains("5h") || text.contains("five hour") || text.contains("five-hour");
    match (gemini, third_party, five_hour, weekly) {
        (true, false, true, false) => Some(0),
        (true, false, false, true) => Some(1),
        (false, true, true, false) => Some(2),
        (false, true, false, true) => Some(3),
        _ => None,
    }
}

fn fallback_slot(window: &UsageWindow) -> Option<usize> {
    let UsageScope::Model(model) = &window.scope else {
        return None;
    };
    let text = format!("{} {model}", window.id).to_ascii_lowercase();
    let group = if text.contains("gemini") {
        0
    } else if text.contains("claude") || text.contains("gpt") {
        2
    } else {
        return None;
    };
    Some(group + usize::from(window.kind == UsageWindowKind::Weekly))
}

fn shared_slot(id: &str) -> Option<usize> {
    (0..4).find(|index| slot_model(*index).0 == id)
}

fn slot_model(index: usize) -> (&'static str, WindowRole, UsageWindowKind, UsageScope) {
    match index {
        0 => (
            "antigravity-gemini-5h",
            WindowRole::PrimaryShort,
            UsageWindowKind::Rolling,
            UsageScope::Model("Gemini".into()),
        ),
        1 => (
            "antigravity-gemini-weekly",
            WindowRole::PrimaryLong,
            UsageWindowKind::Weekly,
            UsageScope::Model("Gemini".into()),
        ),
        2 => (
            "antigravity-claude-gpt-5h",
            WindowRole::PrimaryShort,
            UsageWindowKind::Rolling,
            UsageScope::Model("Claude + GPT".into()),
        ),
        _ => (
            "antigravity-claude-gpt-weekly",
            WindowRole::PrimaryLong,
            UsageWindowKind::Weekly,
            UsageScope::Model("Claude + GPT".into()),
        ),
    }
}

fn remaining_fraction(
    bucket: &serde_json::Map<String, Value>,
) -> Result<Option<f64>, ProviderUsageError> {
    let value = bucket
        .get("remainingFraction")
        .or_else(|| {
            bucket
                .get("quotaInfo")
                .and_then(|v| v.get("remainingFraction"))
        })
        .or_else(|| {
            bucket.get("remaining").and_then(|remaining| {
                remaining
                    .get("remainingFraction")
                    .or_else(|| remaining.get("fraction"))
            })
        });
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let number = match value {
        Value::Number(number) => number.as_f64(),
        Value::Object(tagged) => {
            let case = tagged.get("case").and_then(Value::as_str);
            if case.is_some_and(|case| !case.eq_ignore_ascii_case("remainingFraction")) {
                return Err(invalid());
            }
            tagged
                .get("value")
                .or_else(|| tagged.get("remainingFraction"))
                .and_then(Value::as_f64)
        }
        _ => None,
    }
    .filter(|number| number.is_finite())
    .ok_or_else(invalid)?;
    if !(0.0..=1.0).contains(&number) {
        return Err(invalid());
    }
    Ok(Some(number))
}

fn optional_reset(value: Option<&Value>) -> Result<Option<OffsetDateTime>, ProviderUsageError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let text = value.as_str().ok_or_else(invalid)?;
    OffsetDateTime::parse(text, &Rfc3339)
        .map(Some)
        .map_err(|_| invalid())
}

fn object(input: &str) -> Result<serde_json::Map<String, Value>, ProviderUsageError> {
    let value: Value = serde_json::from_str(input)
        .map_err(|_| ProviderUsageError::Schema(SchemaReason::InvalidJson))?;
    value
        .as_object()
        .cloned()
        .ok_or(ProviderUsageError::Schema(SchemaReason::MissingEnvelope))
}

fn string_at<'a>(root: &'a serde_json::Map<String, Value>, path: &[&str]) -> Option<&'a str> {
    let mut value = root.get(*path.first()?)?;
    for key in &path[1..] {
        value = value.get(*key)?;
    }
    value.as_str().filter(|value| !value.is_empty())
}

fn first_string<'a>(
    root: &'a serde_json::Map<String, Value>,
    paths: &[&[&str]],
) -> Option<&'a str> {
    paths.iter().find_map(|path| string_at(root, path))
}

fn optional_number(value: Option<&Value>) -> Result<Option<f64>, ProviderUsageError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    value
        .as_f64()
        .filter(|number| number.is_finite())
        .map(Some)
        .ok_or_else(invalid)
}

fn optional_string(value: Option<&Value>) -> Result<Option<&str>, ProviderUsageError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    value
        .as_str()
        .filter(|value| !value.is_empty())
        .map(Some)
        .ok_or_else(invalid)
}

fn invalid() -> ProviderUsageError {
    ProviderUsageError::Schema(SchemaReason::InvalidValue)
}

fn stable_slug(value: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            separator = false;
        } else if !separator && !slug.is_empty() {
            slug.push('-');
            separator = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "unknown".into()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMMARY: &str = r#"{
      "groups": [
        {"displayName":"Gemini models","unknown":true,"buckets":[
          {"bucketId":"gemini-5h","remainingFraction":0.75,"resetTime":"2027-01-15T12:00:00Z"},
          {"bucketId":"gemini-weekly","remainingFraction":{"value":0.5},"resetTime":"2027-01-20T12:00:00+00:00"}
        ]},
        {"displayName":"Claude and GPT models","buckets":[
          {"window":"5h","quotaInfo":{"remainingFraction":0.25}},
          {"window":"weekly","remainingFraction":{"case":"RemainingFraction","value":0.1}}
        ]}
      ]
    }"#;

    #[test]
    fn summary_parses_all_four_semantic_pools() {
        let parsed = parse_quota_summary(SUMMARY).expect("summary");
        assert_eq!(parsed.windows.len(), 4);
        assert_eq!(parsed.windows[0].id, "antigravity-gemini-5h");
        assert_eq!(parsed.windows[0].used_percent, Some(25.0));
        assert_eq!(parsed.windows[1].used_percent, Some(50.0));
        assert_eq!(parsed.windows[2].used_percent, Some(75.0));
        assert!((parsed.windows[3].used_percent.unwrap() - 90.0).abs() < f64::EPSILON);
        assert!(parsed.windows.iter().all(|window| window.authoritative));
    }

    #[test]
    fn wrapped_groups_and_flat_nested_and_tagged_fractions_parse() {
        let wrapped = format!(r#"{{"quotaSummary":{SUMMARY}}}"#);
        assert_eq!(parse_quota_summary(&wrapped).unwrap().windows.len(), 4);
    }

    #[test]
    fn a_missing_fraction_stays_unknown() {
        let input = SUMMARY.replacen("\"remainingFraction\":0.75,", "", 1);
        assert_eq!(
            parse_quota_summary(&input).unwrap().windows[0].used_percent,
            None
        );
    }

    #[test]
    fn partial_summary_succeeds_and_duplicate_buckets_fold_conservatively() {
        let parsed = parse_quota_summary(
            r#"{"groups":[{"displayName":"Gemini models","buckets":[
              {"bucketId":"weekly","remainingFraction":0.8,"resetTime":"2027-01-20T12:00:00Z"},
              {"bucketId":"weekly","remainingFraction":0.5,"resetTime":"2027-01-21T12:00:00Z"}
            ]}]}"#,
        )
        .unwrap();
        assert_eq!(parsed.windows.len(), 1);
        assert_eq!(parsed.windows[0].id, "antigravity-gemini-weekly");
        assert_eq!(parsed.windows[0].used_percent, Some(50.0));
        assert_eq!(
            parsed.windows[0].resets_at.unwrap(),
            OffsetDateTime::parse("2027-01-21T12:00:00Z", &Rfc3339).unwrap()
        );
    }

    #[test]
    fn available_models_preserve_default_tiered_and_weekly_windows() {
        let parsed = parse_available_models(
            r#"{"models":{
              "gemini-3-pro-high":{"displayName":"Gemini 3 Pro (High)","quotaInfo":{"remainingFraction":0.75,"resetTime":"2027-01-15T12:00:00Z"}},
              "claude-sonnet":{"displayName":"Claude Sonnet","quotaInfoByTier":{"standard":[
                {"windowId":"rolling","remainingFraction":0.4},
                {"windowId":"weekly-limit","windowLabel":"Weekly","remainingFraction":0.2}
              ]}}
            }}"#,
        )
        .unwrap();
        assert_eq!(parsed.windows.len(), 3);
        let gemini = parsed
            .windows
            .iter()
            .find(|window| window.id == "antigravity-model-gemini-3-pro-high")
            .unwrap();
        assert_eq!(gemini.used_percent, Some(25.0));
        let weekly = parsed
            .windows
            .iter()
            .find(|window| window.id.contains("standard-weekly-limit"))
            .unwrap();
        assert_eq!(weekly.kind, UsageWindowKind::Weekly);
    }

    #[test]
    fn model_fallbacks_collapse_into_two_shared_pools() {
        let models = parse_available_models(
            r#"{"models":{
              "gemini-3-pro":{"displayName":"Gemini 3 Pro","quotaInfo":{"remainingFraction":0.8}},
              "gemini-3-flash":{"displayName":"Gemini 3 Flash","quotaInfo":{"remainingFraction":0.6}},
              "claude-sonnet":{"displayName":"Claude Sonnet","quotaInfo":{"remainingFraction":0.9}},
              "chat_20706":{"displayName":"chat_20706","quotaInfo":{"remainingFraction":0.1}}
            }}"#,
        )
        .unwrap();

        let windows = merge_windows(Vec::new(), models.windows);

        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].id, "antigravity-gemini-5h");
        assert!((windows[0].used_percent.unwrap() - 40.0).abs() < 1e-9);
        assert_eq!(windows[0].role, WindowRole::PrimaryShort);
        assert_eq!(windows[1].id, "antigravity-claude-gpt-5h");
        assert!((windows[1].used_percent.unwrap() - 10.0).abs() < 1e-9);
        assert_eq!(windows[1].role, WindowRole::PrimaryShort);
    }

    #[test]
    fn available_models_reject_malformed_present_quota_values() {
        for input in [
            r#"{"models":{"model":{"quotaInfo":{"remainingFraction":"many"}}}}"#,
            r#"{"models":{"model":{"quotaInfo":{"resetTime":"later"}}}}"#,
            r#"{"models":{"model":{"quotaInfo":true}}}"#,
        ] {
            assert_eq!(parse_available_models(input), Err(invalid()));
        }
    }

    #[test]
    fn invalid_fractions_and_timestamps_reject_the_response() {
        for input in [
            SUMMARY.replacen("0.75", "1.5", 1),
            SUMMARY.replacen("2027-01-15T12:00:00Z", "tomorrow", 1),
            SUMMARY.replacen("0.75", "\"many\"", 1),
        ] {
            assert_eq!(parse_quota_summary(&input), Err(invalid()));
        }
    }

    #[test]
    fn unknown_fields_do_not_change_semantic_pool_identity() {
        let parsed = parse_quota_summary(SUMMARY).unwrap();
        assert_eq!(
            parsed
                .windows
                .iter()
                .map(|window| window.id.as_str())
                .collect::<Vec<_>>(),
            [
                "antigravity-gemini-5h",
                "antigravity-gemini-weekly",
                "antigravity-claude-gpt-5h",
                "antigravity-claude-gpt-weekly"
            ]
        );
    }

    #[test]
    fn load_code_assist_requires_a_managed_project_and_reads_plan_and_credits() {
        let parsed = parse_load_code_assist(
            r#"{
          "cloudaicompanionProject":{"id":"projects/synthetic"},
          "paidTier":{"name":"Google AI Ultra","id":"ultra-tier"},
          "account":{"email":"reader@example.invalid"},
          "planInfo":{"monthlyPromptCredits":1000},
          "availablePromptCredits":750
        }"#,
        )
        .unwrap();
        assert_eq!(parsed.project, "projects/synthetic");
        assert_eq!(parsed.plan.as_deref(), Some("Google AI Ultra"));
        assert_eq!(parsed.tier.as_deref(), Some("ultra-tier"));
        let credits = parsed.credits.unwrap();
        assert_eq!(credits.used_percent, Some(25.0));
        assert_eq!(credits.balance.unwrap().remaining, Some(750.0));

        assert_eq!(
            parse_load_code_assist(r#"{"currentTier":{"name":"Free"}}"#),
            Err(ProviderUsageError::Schema(
                SchemaReason::MissingRequiredField
            ))
        );
    }

    #[test]
    fn get_user_status_keeps_model_quotas_separate_and_stable() {
        let parsed = parse_get_user_status(
            r#"{"userStatus":{
              "email":"reader@example.invalid",
              "planStatus":{"planInfo":{"planName":"Pro","teamsTier":"TEAMS_TIER_PRO"}},
              "cascadeModelConfigData":{"clientModelConfigs":[
                {"label":"Gemini 3 Pro (High)","quotaInfo":{"remainingFraction":0.8,"resetTime":"2027-01-15T12:00:00Z"}},
                {"label":"Claude Sonnet 4.5","quotaInfo":{"resetTime":null}}
              ]}
            }}"#,
        )
        .unwrap();
        assert_eq!(parsed.account.as_deref(), Some("reader@example.invalid"));
        assert_eq!(parsed.plan.as_deref(), Some("Pro"));
        assert_eq!(parsed.tier.as_deref(), Some("TEAMS_TIER_PRO"));
        assert_eq!(parsed.windows[0].id, "antigravity-model-gemini-3-pro-high");
        assert!((parsed.windows[0].used_percent.unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(parsed.windows[1].used_percent, None);
        assert!(
            parsed
                .windows
                .iter()
                .all(|window| window.role == WindowRole::Supplemental)
        );
    }

    #[test]
    fn get_user_status_bounds_models_and_rejects_invalid_quota() {
        let model = serde_json::json!({
            "label": "Model",
            "quotaInfo": {"remainingFraction": 0.5}
        });
        let too_many = serde_json::json!({
            "userStatus": {
                "cascadeModelConfigData": {
                    "clientModelConfigs": vec![model; MAX_MODELS + 1]
                }
            }
        });
        assert_eq!(
            parse_get_user_status(&too_many.to_string()),
            Err(ProviderUsageError::Schema(SchemaReason::InvalidValue))
        );
        assert_eq!(
            parse_get_user_status(
                r#"{"userStatus":{"clientModelConfigs":[{"label":"Model","quotaInfo":{"remainingFraction":-0.1}}]}}"#
            ),
            Err(ProviderUsageError::Schema(SchemaReason::InvalidValue))
        );
    }
}
