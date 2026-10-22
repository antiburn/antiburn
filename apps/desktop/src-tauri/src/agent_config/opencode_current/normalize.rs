use super::{ConfigUnavailableReason, OpenCodeConfigSubject, OperationSelector};
use serde_json::{Value, json};

pub(super) fn permission_allowed(
    entries: &[super::ConfigEntry],
    action: &str,
    resource: &str,
) -> Result<bool, ConfigUnavailableReason> {
    let mut allowed = true;
    for entry in entries {
        let Some(rules) = entry.info.as_ref().and_then(|info| info.get("permissions")) else {
            continue;
        };
        for rule in rules
            .as_array()
            .ok_or(ConfigUnavailableReason::MalformedConfig)?
        {
            let configured_action = rule
                .get("action")
                .and_then(Value::as_str)
                .ok_or(ConfigUnavailableReason::MalformedConfig)?;
            if configured_action != "*" && configured_action.contains(['*', '?']) {
                return Err(ConfigUnavailableReason::InvalidPrecedence);
            }
            if configured_action != "*" && configured_action != action {
                continue;
            }
            let configured_resource = rule
                .get("resource")
                .and_then(Value::as_str)
                .ok_or(ConfigUnavailableReason::MalformedConfig)?;
            if configured_resource != "*" && configured_resource.contains(['*', '?']) {
                return Err(ConfigUnavailableReason::InvalidPrecedence);
            }
            if configured_resource != "*" && configured_resource != resource {
                continue;
            }
            allowed = match rule.get("effect").and_then(Value::as_str) {
                Some("allow" | "ask") => true,
                Some("deny") => false,
                _ => return Err(ConfigUnavailableReason::MalformedConfig),
            };
        }
    }
    Ok(allowed)
}

pub(super) fn reject_unresolved(value: &Value) -> Result<(), ConfigUnavailableReason> {
    if ["policies", "plugins", "plugin"].iter().any(|key| {
        value.get(key).is_some_and(|value| match value {
            Value::Array(items) => !items.is_empty(),
            _ => true,
        })
    }) || value.pointer("/experimental/policies").is_some()
    {
        return Err(ConfigUnavailableReason::RuntimeOverride);
    }
    Ok(())
}

pub(super) fn model(value: &Value) -> Result<Value, ConfigUnavailableReason> {
    let (provider, model, variant) = if let Some(text) = value.as_str() {
        if text.contains("{env:") || text.contains("{file:") {
            return Err(ConfigUnavailableReason::DynamicValue);
        }
        let (provider, tail) = text
            .split_once('/')
            .ok_or(ConfigUnavailableReason::InvalidTarget)?;
        let (model, variant) = tail
            .split_once('#')
            .map_or((tail, None), |(model, variant)| (model, Some(variant)));
        (provider, model, variant)
    } else {
        (
            value
                .get("providerID")
                .and_then(Value::as_str)
                .ok_or(ConfigUnavailableReason::MalformedConfig)?,
            value
                .get("model")
                .and_then(Value::as_str)
                .ok_or(ConfigUnavailableReason::MalformedConfig)?,
            value
                .get("variant")
                .map(|value| {
                    value
                        .as_str()
                        .ok_or(ConfigUnavailableReason::MalformedConfig)
                })
                .transpose()?,
        )
    };
    if provider.contains('/')
        || [provider, model].into_iter().chain(variant).any(|text| {
            text.is_empty()
                || text.len() > 256
                || text.contains(['{', '}', '#'])
                || text.chars().any(char::is_whitespace)
        })
    {
        return Err(ConfigUnavailableReason::InvalidTarget);
    }
    // Unreviewed provider aliases must not select a different physical route.
    if ["azure-cognitive-services", "google-vertex-anthropic"].contains(&provider) {
        return Err(ConfigUnavailableReason::InvalidTarget);
    }
    let mut normalized = json!({"providerID":provider,"model":model});
    if let Some(variant) = variant {
        normalized["variant"] = variant.into();
    }
    Ok(normalized)
}

pub(super) fn canonical_value(
    info: &Value,
    subject: &OpenCodeConfigSubject,
) -> Result<Option<Value>, ConfigUnavailableReason> {
    Ok(match subject {
        OpenCodeConfigSubject::FutureDefaultModel => info.get("model").map(model).transpose()?,
        OpenCodeConfigSubject::AgentModel(id) => info
            .get("agents")
            .and_then(|agents| agents.get(id))
            .and_then(|agent| agent.get("model"))
            .map(model)
            .transpose()?,
        OpenCodeConfigSubject::AutomaticCompaction => info.pointer("/compaction/auto").cloned(),
        OpenCodeConfigSubject::McpServer(id) => info
            .pointer("/mcp/servers")
            .and_then(|servers| servers.get(id))
            .cloned(),
        OpenCodeConfigSubject::SkillAdvertising(_) => None,
    })
}

pub(super) fn select(
    raw: &Value,
    subject: &OpenCodeConfigSubject,
) -> Result<Option<(OperationSelector, Value)>, ConfigUnavailableReason> {
    match subject {
        OpenCodeConfigSubject::FutureDefaultModel => raw
            .get("model")
            .map(|value| Ok((OperationSelector::OpenCodeDefaultModel, model(value)?)))
            .transpose(),
        OpenCodeConfigSubject::AutomaticCompaction => raw
            .pointer("/compaction/auto")
            .map(|value| {
                if !value.is_boolean() {
                    return Err(ConfigUnavailableReason::MalformedConfig);
                }
                Ok((
                    OperationSelector::OpenCodeAutomaticCompaction,
                    value.clone(),
                ))
            })
            .transpose(),
        OpenCodeConfigSubject::AgentModel(id) => {
            let mut selected = None;
            for field in ["agent", "mode", "agents"] {
                if let Some(agent) = raw.get(field).and_then(|agents| agents.get(id)) {
                    // Native entries replace a whole migrated entry within one document.
                    selected = if let Some(value) = agent.get("model") {
                        let mut model = model(value)?;
                        if field != "agents"
                            && let Some(variant) = agent.get("variant")
                        {
                            model["variant"] = variant
                                .as_str()
                                .filter(|value| !value.is_empty() && !value.contains('#'))
                                .ok_or(ConfigUnavailableReason::InvalidTarget)?
                                .into();
                        }
                        Some((
                            OperationSelector::OpenCodeAgentModel {
                                field: field.into(),
                                agent: id.clone(),
                            },
                            model,
                        ))
                    } else {
                        None
                    };
                    if agent.get("disabled").and_then(Value::as_bool) == Some(true)
                        || agent.get("disable").and_then(Value::as_bool) == Some(true)
                    {
                        return Err(ConfigUnavailableReason::MissingTarget);
                    }
                }
            }
            Ok(selected)
        }
        OpenCodeConfigSubject::McpServer(id) => {
            let native = raw
                .pointer("/mcp/servers")
                .and_then(|servers| servers.get(id));
            let legacy = raw.get("mcp").and_then(|mcp| mcp.get(id));
            let (value, native) = match (native, legacy) {
                (Some(value), _) => (value, true),
                (None, Some(value)) => (value, false),
                _ => return Ok(None),
            };
            let mut server = value
                .as_object()
                .ok_or(ConfigUnavailableReason::MalformedConfig)?
                .clone();
            let valid = match server.get("type").and_then(Value::as_str) {
                Some("local") => server
                    .get("command")
                    .and_then(Value::as_array)
                    .is_some_and(|args| args.iter().all(Value::is_string)),
                Some("remote") => server.get("url").is_some_and(Value::is_string),
                _ => false,
            };
            if !valid {
                return Err(ConfigUnavailableReason::MalformedConfig);
            }
            let local = server.get("type").and_then(Value::as_str) == Some("local");
            let fields: &[&str] = match (native, local) {
                (true, true) => &[
                    "type",
                    "command",
                    "cwd",
                    "environment",
                    "disabled",
                    "codemode",
                    "timeout",
                    "protocol",
                ],
                (true, false) => &[
                    "type", "url", "headers", "oauth", "disabled", "codemode", "timeout",
                    "protocol",
                ],
                (false, true) => &[
                    "type",
                    "command",
                    "cwd",
                    "environment",
                    "enabled",
                    "timeout",
                ],
                (false, false) => &["type", "url", "headers", "oauth", "enabled", "timeout"],
            };
            server.retain(|key, _| fields.contains(&key.as_str()));
            if !native {
                if let Some(enabled) = server.remove("enabled") {
                    server.insert(
                        "disabled".into(),
                        (!enabled
                            .as_bool()
                            .ok_or(ConfigUnavailableReason::MalformedConfig)?)
                        .into(),
                    );
                }
                if let Some(oauth) = server.get_mut("oauth")
                    && oauth.is_object()
                {
                    let mut migrated = serde_json::Map::new();
                    for (old, new) in [
                        ("clientId", "client_id"),
                        ("clientSecret", "client_secret"),
                        ("scope", "scope"),
                        ("callbackPort", "callback_port"),
                        ("redirectUri", "redirect_uri"),
                    ] {
                        if let Some(value) = oauth.get(old) {
                            migrated.insert(new.into(), value.clone());
                        }
                    }
                    *oauth = Value::Object(migrated);
                }
            }
            if let Some(timeout) = server.get_mut("timeout")
                && let Some(number) = timeout.as_u64()
            {
                *timeout = json!({"startup":number,"catalog":number,"execution":number});
            }
            if server
                .get("disabled")
                .is_some_and(|value| !value.is_boolean())
            {
                return Err(ConfigUnavailableReason::MalformedConfig);
            }
            if Value::Object(server.clone()).to_string().contains("{env:")
                || Value::Object(server.clone()).to_string().contains("{file:")
            {
                return Err(ConfigUnavailableReason::DynamicValue);
            }
            Ok(Some((
                OperationSelector::OpenCodeMcpServer {
                    server: id.clone(),
                    native,
                },
                Value::Object(server),
            )))
        }
        OpenCodeConfigSubject::SkillAdvertising(_) => {
            Err(ConfigUnavailableReason::UnsupportedSetting)
        }
    }
}
