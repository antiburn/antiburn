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
        || [provider, model]
            .into_iter()
            .chain(variant)
            .any(invalid_model_component)
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

fn invalid_model_component(text: &str) -> bool {
    text.is_empty()
        || text.len() > 256
        || text.contains(['{', '}', '#'])
        || text.chars().any(char::is_whitespace)
}

fn agent_disabled(agent: &Value) -> bool {
    ["disabled", "disable"]
        .iter()
        .any(|key| agent.get(key).and_then(Value::as_bool) == Some(true))
}

fn agent_model(
    agent: &Value,
    separate_variant: bool,
) -> Result<Option<Value>, ConfigUnavailableReason> {
    if agent_disabled(agent) {
        return Err(ConfigUnavailableReason::MissingTarget);
    }
    let variant = if separate_variant {
        agent
            .get("variant")
            .map(|value| {
                let variant = value
                    .as_str()
                    .ok_or(ConfigUnavailableReason::MalformedConfig)?;
                if invalid_model_component(variant) {
                    return Err(ConfigUnavailableReason::InvalidTarget);
                }
                Ok(variant)
            })
            .transpose()?
    } else {
        None
    };
    let Some(value) = agent.get("model") else {
        return Ok(None);
    };
    let mut normalized = model(value)?;
    if let Some(variant) = variant {
        normalized["variant"] = variant.into();
    }
    Ok(Some(normalized))
}

pub(super) fn markdown_agent_model(
    header: &serde_yaml_ng::Value,
) -> Result<Option<Value>, ConfigUnavailableReason> {
    for (key, value) in header
        .as_mapping()
        .ok_or(ConfigUnavailableReason::MalformedConfig)?
    {
        let valid = match key.as_str() {
            Some("model" | "variant") => true,
            Some("description") => value.is_string(),
            Some("mode") => matches!(value.as_str(), Some("primary" | "subagent" | "all")),
            Some("hidden" | "disabled" | "disable") => value.is_bool(),
            Some("steps") => value.as_u64().is_some_and(|steps| steps > 0),
            Some("color") => value.as_str().is_some_and(|color| {
                color.len() == 7
                    && color.starts_with('#')
                    && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
            }),
            _ => false,
        };
        if !valid {
            return Err(ConfigUnavailableReason::InvalidPrecedence);
        }
    }
    let agent =
        serde_json::to_value(header).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    agent_model(&agent, true)
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
            // Native entries replace a whole migrated entry within one document.
            let Some((field, agent)) = ["agents", "mode", "agent"].into_iter().find_map(|field| {
                raw.get(field)
                    .and_then(|agents| agents.get(id))
                    .map(|agent| (field, agent))
            }) else {
                return Ok(None);
            };
            Ok(agent_model(agent, field != "agents")?.map(|model| {
                (
                    OperationSelector::OpenCodeAgentModel {
                        field: field.into(),
                        agent: id.clone(),
                    },
                    model,
                )
            }))
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
