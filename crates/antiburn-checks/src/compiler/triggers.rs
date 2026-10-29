use std::time::Duration;

use crate::config::{ConfigValue, ConfigValueKind};
use crate::domain::SessionStartReason;

use super::{EventFilters, ToolResultStatus, ToolResultTrigger, TriggerFamily, Validator};

impl Validator {
    pub(super) fn compile_trigger(
        &mut self,
        value: &ConfigValue,
        default_cooldown: Duration,
    ) -> Option<ToolResultTrigger> {
        let fields = self.mapping(value, "on")?;
        let events = [
            "tool_result",
            "user_message",
            "assistant_message",
            "tool_call",
            "turn_end",
            "session_start",
            "session_idle",
        ];
        let mut allowed = vec!["window", "cooldown"];
        allowed.extend(events);
        self.allowed_fields(fields, &allowed, "on");
        let selected = fields
            .iter()
            .filter(|entry| events.contains(&entry.key.as_str()))
            .collect::<Vec<_>>();
        if selected.is_empty() {
            self.missing_field(value, "on event");
            return None;
        }
        let family = |name: &str| match name {
            "turn_end" => 1,
            "session_start" | "session_idle" => 2,
            _ => 0,
        };
        let scope = family(&selected[0].key);
        if selected.iter().any(|entry| family(&entry.key) != scope) {
            self.invalid_value(value, "on", "Select events from exactly one family.");
        }
        if scope != 0
            && let Some(window) = fields.iter().find(|entry| entry.key == "window")
        {
            self.invalid_value(
                &window.value,
                "on.window",
                "Only message-family checks can configure a window.",
            );
        }
        let mut alternatives = std::collections::BTreeMap::new();
        for entry in &selected {
            alternatives.insert(
                entry.key.as_str(),
                self.event_filters(&entry.value, &entry.key),
            );
        }
        let result = alternatives
            .remove("tool_result")
            .flatten()
            .unwrap_or_default();
        let trigger_family = match scope {
            0 => TriggerFamily::Messages {
                tool_result: selected.iter().any(|entry| entry.key == "tool_result"),
                user_message: alternatives.remove("user_message").flatten(),
                assistant_message: alternatives.remove("assistant_message").flatten(),
                tool_call: alternatives.remove("tool_call").flatten(),
            },
            1 => TriggerFamily::ResponseCycle(
                alternatives
                    .remove("turn_end")
                    .flatten()
                    .unwrap_or_default(),
            ),
            _ => TriggerFamily::Session {
                session_start: alternatives.remove("session_start").flatten(),
                session_idle: alternatives.remove("session_idle").flatten(),
            },
        };
        let window = fields
            .iter()
            .find(|entry| entry.key == "window")
            .and_then(|entry| self.parse_window(&entry.value));
        let cooldown = fields
            .iter()
            .find(|entry| entry.key == "cooldown")
            .map_or(Some(default_cooldown), |entry| {
                self.parse_cooldown(&entry.value, "on.cooldown")
            })?;
        Some(ToolResultTrigger {
            family: trigger_family,
            tools: result.tools,
            statuses: result.statuses,
            models: result.models,
            fast_mode: result.fast_mode,
            paths: result.paths,
            paths_ignore: result.paths_ignore,
            window,
            cooldown,
        })
    }

    fn event_filters(&mut self, value: &ConfigValue, event: &str) -> Option<EventFilters> {
        let path = format!("on.{event}");
        if matches!(value.kind, ConfigValueKind::Null) {
            return Some(EventFilters::default());
        }
        let fields = self.mapping(value, &path)?;
        let mut allowed = vec!["models", "fast_mode"];
        if matches!(event, "tool_call" | "tool_result") {
            allowed.extend(["tools", "paths", "paths-ignore"]);
        }
        if matches!(event, "tool_result" | "turn_end") {
            allowed.push("status");
        }
        if event == "session_start" {
            allowed.push("reason");
        }
        self.allowed_fields(fields, &allowed, &path);
        let mut result = EventFilters::default();
        for field in fields {
            let path = format!("{path}.{}", field.key);
            match field.key.as_str() {
                "tools" => result.tools = self.parse_tools(&field.value, &path),
                "status" => {
                    result.statuses = self.parse_statuses(&field.value, &path);
                    if event == "turn_end"
                        && result
                            .statuses
                            .as_ref()
                            .is_some_and(|statuses| statuses.contains(&ToolResultStatus::Unknown))
                    {
                        self.invalid_value(
                            &field.value,
                            &path,
                            "A response-cycle status must be completed, cancelled, or error.",
                        );
                    }
                }
                "models" => result.models = self.parse_models(&field.value, &path),
                "fast_mode" => match field.value.kind {
                    ConfigValueKind::Boolean(mode) => result.fast_mode = Some(mode),
                    _ => self.invalid_type(&field.value, &path, "a literal Boolean"),
                },
                "paths" => result.paths = self.parse_path_globs(&field.value, &path),
                "paths-ignore" => result.paths_ignore = self.parse_path_globs(&field.value, &path),
                "reason" => {
                    result.reasons = self.unique_strings(&field.value, &path).map(|items| {
                        items
                            .into_iter()
                            .filter_map(|(value, reason)| match reason.as_str() {
                                "new" => Some(SessionStartReason::New),
                                "resumed" => Some(SessionStartReason::Resumed),
                                "attached" => Some(SessionStartReason::Attached),
                                _ => {
                                    self.invalid_value(
                                        value,
                                        &path,
                                        "Unknown session-start reason.",
                                    );
                                    None
                                }
                            })
                            .collect()
                    });
                }
                _ => {}
            }
        }
        Some(result)
    }
}
