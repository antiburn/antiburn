//! Typed admission for the Burn Check features supported by this release.

use std::collections::HashSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Number;

use crate::config::{ConfigEntry, ConfigValue, ConfigValueKind, YamlDocument, parse_yaml};
use crate::diagnostics::{Diagnostic, DiagnosticCode};
use crate::expression::{Expression, PathSegment, parse_expression};
use crate::fit::{
    FitPathSegment, FitRule, FitStrategy, MAX_FIT_PATH_BYTES, MAX_FIT_PATH_SEGMENTS, MAX_FIT_RULES,
    parse_fit_path, paths_overlap,
};

const MAX_WINDOW_SIZE: u64 = 40;
const SUPPORTED_MODEL: &str = "typesafe/jev-1.13.0";
/// The pinned Jev route accepts at most this number of Choice criteria.
pub const MAX_CHOICE_CRITERIA: usize = 26;

/// One fully validated check that uses only supported capabilities.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledCheck {
    /// The display name from the definition.
    pub name: String,
    /// The optional fixed description.
    pub description: Option<String>,
    /// A pre-scheduling condition, when configured.
    pub condition: Option<Condition>,
    /// The supported finalized-tool-result trigger.
    pub trigger: ToolResultTrigger,
    /// The one supported TypeSafe Choice question.
    pub decision: ChoiceDecision,
    /// Ordered status actions. The runtime evaluates each condition separately.
    pub actions: Vec<StatusAction>,
}

/// A Boolean condition or parsed expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Condition {
    /// A literal YAML Boolean.
    Boolean(bool),
    /// A parsed expression.
    Expression(Expression),
}

/// Filters and rolling selection settings for `tool_result` events.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResultTrigger {
    /// Optional normalized tool categories. An absent list means all categories.
    pub tools: Option<Vec<ToolCategory>>,
    /// Optional final result statuses. An absent list means all statuses.
    pub statuses: Option<Vec<ToolResultStatus>>,
    /// Optional rolling message window.
    pub window: Option<RollingWindow>,
    /// The model-run cooldown. The default is 60 seconds.
    pub cooldown: Duration,
}

/// A normalized tool category accepted by the current filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolCategory {
    /// Shell or command execution.
    Shell,
    /// File or content reads.
    Read,
    /// Search operations.
    Search,
    /// Edits to existing content.
    Edit,
    /// Writes of new or replaced content.
    Write,
    /// Delegated work.
    Delegate,
    /// Skill operations.
    Skill,
    /// Model-context-protocol tools.
    Mcp,
    /// Other normalized tools.
    Other,
}

/// A final tool-result status accepted by the current filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolResultStatus {
    /// The operation completed.
    Completed,
    /// The operation ended with an error.
    Error,
    /// The operation was cancelled.
    Cancelled,
    /// The source cannot classify the result.
    Unknown,
}

/// A bounded rolling window of selected messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RollingWindow {
    /// The maximum number of retained messages, from 1 through 40.
    pub size: u8,
    /// The number of matching messages required before assessment.
    pub minimum: u8,
}

/// The model route supported by this release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum DecisionModel {
    /// TypeSafe's pinned `jev-1.13.0` route.
    TypeSafeJev1130,
}

/// One compiled model-backed Choice decision.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChoiceDecision {
    /// The authored decision identifier.
    pub name: String,
    /// The supported provider route.
    pub model: DecisionModel,
    /// Fixed-key input templates prepared against the frozen candidate.
    pub input: Vec<(String, InputTemplate)>,
    /// Authorized reductions in YAML declaration order.
    pub fit: Vec<FitRule>,
    /// The one output question identifier.
    pub question_id: String,
    /// The fixed Choice question text.
    pub question: String,
    /// Declared labels and their fixed descriptions, in authored order.
    pub criteria: Vec<(String, String)>,
}

/// A recursively prepared decision input value.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum InputTemplate {
    /// A JSON-compatible literal.
    Literal(InputLiteral),
    /// A typed whole-value expression.
    Expression(Expression),
    /// An ordered array template.
    Sequence(Vec<Self>),
    /// A fixed-key object template.
    Object(Vec<(String, Self)>),
}

/// A JSON-compatible literal accepted by an input template.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum InputLiteral {
    /// A null value.
    Null,
    /// A Boolean value.
    Boolean(bool),
    /// A finite JSON number.
    Number(Number),
    /// A string value.
    String(String),
}

/// The exact status value written by a status action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatusResult {
    /// Write a concern status.
    Failure,
    /// Write a no-concern-in-this-assessment status.
    Success,
}

/// One independently gated, ordered status action.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusAction {
    /// The action condition, when configured.
    pub condition: Option<Condition>,
    /// The literal status result to write when the condition passes.
    pub result: StatusResult,
    /// Optional fixed user-facing explanation.
    pub message: Option<String>,
}

/// Parses YAML and compiles it against the application's supported feature set.
///
/// This validates authored configuration. It does not evaluate expressions,
/// read evidence, call TypeSafe, or execute actions.
///
/// # Errors
/// Returns YAML, structural, expression, or unsupported-feature diagnostics.
pub fn compile(source: &str) -> Result<CompiledCheck, Vec<Diagnostic>> {
    let document = parse_yaml(source)?;
    compile_document(&document)
}

/// Compiles an already parsed YAML document against the supported feature set.
///
/// # Errors
/// Returns structural, expression, or unsupported-feature diagnostics.
pub fn compile_document(document: &YamlDocument) -> Result<CompiledCheck, Vec<Diagnostic>> {
    let mut validator = Validator {
        diagnostics: Vec::new(),
        has_window: false,
        decisions: None,
    };
    let compiled = validator.compile_root(document.root());
    if validator.diagnostics.is_empty() {
        Ok(compiled.expect("valid configuration produces a compiled definition"))
    } else {
        Err(validator.diagnostics)
    }
}

struct Validator {
    diagnostics: Vec<Diagnostic>,
    has_window: bool,
    decisions: Option<DecisionSymbols>,
}

#[derive(Clone)]
struct DecisionSymbols {
    name: String,
    question: String,
    labels: HashSet<String>,
}

struct CompiledChoiceOutput {
    question_id: String,
    question: String,
    criteria: Vec<(String, String)>,
}

impl Validator {
    fn compile_root(&mut self, root: &ConfigValue) -> Option<CompiledCheck> {
        let fields = self.mapping(root, "")?;
        self.allowed_fields(
            fields,
            &["name", "description", "on", "if", "decisions", "actions"],
            "",
        );

        for required_name in ["name", "on", "decisions", "actions"] {
            if !fields.iter().any(|entry| entry.key == required_name) {
                self.missing_field(root, required_name);
            }
        }
        let name = fields
            .iter()
            .find(|entry| entry.key == "name")
            .and_then(|entry| self.fixed_text(Some(&entry.value), "name"));
        let description = fields
            .iter()
            .find(|entry| entry.key == "description")
            .and_then(|entry| self.fixed_text(Some(&entry.value), "description"));
        let trigger = fields
            .iter()
            .find(|entry| entry.key == "on")
            .and_then(|entry| self.compile_trigger(&entry.value));
        self.has_window = trigger.as_ref().is_some_and(|value| value.window.is_some());
        let condition = fields
            .iter()
            .find(|entry| entry.key == "if")
            .and_then(|entry| self.condition(&entry.value, "if", ExpressionContext::TopLevel));
        let decision = fields
            .iter()
            .find(|entry| entry.key == "decisions")
            .and_then(|entry| self.compile_decision(&entry.value));
        let actions = fields
            .iter()
            .find(|entry| entry.key == "actions")
            .and_then(|entry| self.compile_actions(&entry.value));

        let (Some(name), Some(trigger), Some(decision), Some(actions)) =
            (name, trigger, decision, actions)
        else {
            return None;
        };
        Some(CompiledCheck {
            name,
            description,
            condition,
            trigger,
            decision,
            actions,
        })
    }

    fn compile_trigger(&mut self, value: &ConfigValue) -> Option<ToolResultTrigger> {
        let fields = self.mapping(value, "on")?;
        let event_names = [
            "tool_result",
            "user_message",
            "assistant_message",
            "tool_call",
            "turn_end",
            "session_start",
            "session_idle",
        ];
        let mut allowed_events = vec!["window", "cooldown"];
        allowed_events.extend(event_names);
        self.allowed_fields(fields, &allowed_events, "on");
        for entry in fields {
            if entry.key != "tool_result" && event_names.contains(&entry.key.as_str()) {
                self.unsupported(
                    &entry.value,
                    format!("Trigger `{}` is not supported yet.", entry.key),
                    format!("on.{}", entry.key),
                );
            }
        }
        let event = fields.iter().find(|entry| entry.key == "tool_result");
        if event.is_none() {
            self.push(
                DiagnosticCode::ConfigMissingField,
                "The supported trigger requires `on.tool_result`.",
                value,
                "on.tool_result",
            );
        }
        let mut tools = None;
        let mut statuses = None;
        if let Some(entry) = event {
            match &entry.value.kind {
                ConfigValueKind::Null => {}
                ConfigValueKind::Mapping(filters) => {
                    self.allowed_fields(
                        filters,
                        &[
                            "tools",
                            "status",
                            "models",
                            "fast_mode",
                            "paths",
                            "paths-ignore",
                        ],
                        "on.tool_result",
                    );
                    for filter in filters {
                        if matches!(
                            filter.key.as_str(),
                            "models" | "fast_mode" | "paths" | "paths-ignore"
                        ) {
                            self.unsupported(
                                &filter.value,
                                format!(
                                    "The `tool_result.{}` filter is not supported yet.",
                                    filter.key
                                ),
                                format!("on.tool_result.{}", filter.key),
                            );
                        }
                    }
                    tools = filters
                        .iter()
                        .find(|filter| filter.key == "tools")
                        .and_then(|filter| self.parse_tools(&filter.value, "on.tool_result.tools"));
                    statuses = filters
                        .iter()
                        .find(|filter| filter.key == "status")
                        .and_then(|filter| {
                            self.parse_statuses(&filter.value, "on.tool_result.status")
                        });
                }
                _ => self.invalid_type(&entry.value, "on.tool_result", "null or an object"),
            }
        }
        let window = fields
            .iter()
            .find(|entry| entry.key == "window")
            .and_then(|entry| self.parse_window(&entry.value));
        let cooldown = fields
            .iter()
            .find(|entry| entry.key == "cooldown")
            .and_then(|entry| self.parse_cooldown(&entry.value, "on.cooldown"))
            .or_else(|| {
                (!fields.iter().any(|entry| entry.key == "cooldown"))
                    .then(|| Duration::from_secs(60))
            });
        Some(ToolResultTrigger {
            tools,
            statuses,
            window,
            cooldown: cooldown?,
        })
    }

    fn parse_tools(&mut self, value: &ConfigValue, path: &str) -> Option<Vec<ToolCategory>> {
        let items = self.sequence(value, path)?;
        if items.is_empty() {
            self.invalid_value(value, path, "The tool category list must not be empty.");
            return None;
        }
        let mut output = Vec::new();
        let mut seen = HashSet::new();
        for item in items {
            let Some(text) = self.string(item, path) else {
                continue;
            };
            if !seen.insert(text.to_owned()) {
                self.invalid_value(item, path, "Tool categories must be unique.");
                continue;
            }
            let category = match text {
                "shell" => ToolCategory::Shell,
                "read" => ToolCategory::Read,
                "search" => ToolCategory::Search,
                "edit" => ToolCategory::Edit,
                "write" | "delegate" | "skill" | "mcp" => {
                    self.unsupported(
                        item,
                        format!("The `{text}` tool category is not supported by source adapters."),
                        path,
                    );
                    continue;
                }
                "other" => ToolCategory::Other,
                _ => {
                    self.invalid_value(item, path, "Unknown normalized tool category.");
                    continue;
                }
            };
            output.push(category);
        }
        Some(output)
    }

    fn parse_statuses(&mut self, value: &ConfigValue, path: &str) -> Option<Vec<ToolResultStatus>> {
        let items = self.sequence(value, path)?;
        if items.is_empty() {
            self.invalid_value(value, path, "The result status list must not be empty.");
            return None;
        }
        let mut output = Vec::new();
        let mut seen = HashSet::new();
        for item in items {
            let Some(text) = self.string(item, path) else {
                continue;
            };
            if !seen.insert(text.to_owned()) {
                self.invalid_value(item, path, "Result statuses must be unique.");
                continue;
            }
            let status = match text {
                "completed" => ToolResultStatus::Completed,
                "error" => ToolResultStatus::Error,
                "cancelled" => ToolResultStatus::Cancelled,
                "unknown" => ToolResultStatus::Unknown,
                _ => {
                    self.invalid_value(item, path, "Unknown tool-result status.");
                    continue;
                }
            };
            output.push(status);
        }
        Some(output)
    }

    fn parse_window(&mut self, value: &ConfigValue) -> Option<RollingWindow> {
        let path = "on.window";
        let (size_value, minimum_value) = match &value.kind {
            ConfigValueKind::Number(number) => (Some((value, number)), None),
            ConfigValueKind::Mapping(fields) => {
                self.allowed_fields(fields, &["size", "min"], path);
                let size = fields.iter().find(|entry| entry.key == "size");
                if size.is_none() {
                    self.missing_field(value, "on.window.size");
                }
                (
                    size.and_then(|entry| match &entry.value.kind {
                        ConfigValueKind::Number(number) => Some((&entry.value, number)),
                        _ => {
                            self.invalid_type(&entry.value, "on.window.size", "a positive integer");
                            None
                        }
                    }),
                    fields
                        .iter()
                        .find(|entry| entry.key == "min")
                        .and_then(|entry| match &entry.value.kind {
                            ConfigValueKind::Number(number) => Some((&entry.value, number)),
                            _ => {
                                self.invalid_type(
                                    &entry.value,
                                    "on.window.min",
                                    "a positive integer",
                                );
                                None
                            }
                        }),
                )
            }
            _ => {
                self.invalid_type(
                    value,
                    path,
                    "a positive integer or an object with size and min",
                );
                return None;
            }
        };
        let (size_span, size_number) = size_value?;
        let Some(size) = size_number.as_u64() else {
            self.invalid_window(size_span, "Window size must be a positive integer.");
            return None;
        };
        let minimum = minimum_value
            .and_then(|(minimum_span, number)| {
                number.as_u64().or_else(|| {
                    self.invalid_window(minimum_span, "Window minimum must be a positive integer.");
                    None
                })
            })
            .unwrap_or(1);
        if size == 0 || size > MAX_WINDOW_SIZE {
            self.invalid_window(size_span, "Window size must be between 1 and 40 messages.");
        }
        if minimum == 0 || minimum > size {
            self.invalid_window(
                minimum_value.map_or(value, |(span, _)| span),
                "Window minimum must be positive and must not exceed size.",
            );
        }
        if size == 0 || size > MAX_WINDOW_SIZE || minimum == 0 || minimum > size {
            return None;
        }
        Some(RollingWindow {
            size: size as u8,
            minimum: minimum as u8,
        })
    }

    fn parse_cooldown(&mut self, value: &ConfigValue, path: &str) -> Option<Duration> {
        let text = self.string(value, path)?;
        let (digits, multiplier) = match text.as_bytes().last() {
            Some(b's') => (&text[..text.len() - 1], 1_u64),
            Some(b'm') => (&text[..text.len() - 1], 60),
            Some(b'h') => (&text[..text.len() - 1], 3_600),
            _ => {
                self.invalid_cooldown(value, path, "Cooldown must end in s, m, or h.");
                return None;
            }
        };
        let canonical = digits == "0"
            || (!digits.is_empty()
                && !digits.starts_with('0')
                && digits.bytes().all(|byte| byte.is_ascii_digit()));
        let seconds = digits
            .parse::<u64>()
            .ok()
            .and_then(|number| number.checked_mul(multiplier));
        if !canonical || seconds.is_none() {
            self.invalid_cooldown(
                value,
                path,
                "Cooldown must be a bounded integer duration such as 60s or 5m.",
            );
            return None;
        }
        Some(Duration::from_secs(seconds.expect("checked above")))
    }

    fn compile_decision(&mut self, value: &ConfigValue) -> Option<ChoiceDecision> {
        let path = "decisions";
        let entries = self.mapping(value, path)?;
        if entries.len() != 1 {
            self.unsupported(
                value,
                "Exactly one decision block is supported.".to_owned(),
                path,
            );
        }
        let entry = entries.first()?;
        if !is_identifier(&entry.key) {
            self.invalid_value(value, path, "Decision names must match [a-z][a-z0-9_]*.");
        }
        let fields = self.mapping(&entry.value, &format!("decisions.{}", entry.key))?;
        let block_path = format!("decisions.{}", entry.key);
        self.allowed_fields(fields, &["model", "input", "fit", "output"], &block_path);
        self.require_fields(
            fields,
            &["model", "input", "output"],
            &entry.value,
            &block_path,
        );
        let model_entry = fields.iter().find(|field| field.key == "model");
        let model = model_entry
            .and_then(|field| self.string(&field.value, &format!("{block_path}.model")))
            .map(|model| {
                if model != SUPPORTED_MODEL {
                    self.unsupported(
                        &model_entry.expect("model entry exists").value,
                        format!("Model route must be exactly `{SUPPORTED_MODEL}`."),
                        format!("{block_path}.model"),
                    );
                }
                DecisionModel::TypeSafeJev1130
            });
        let input_value = fields.iter().find(|field| field.key == "input");
        let input = input_value.and_then(|field| {
            let input_path = format!("{block_path}.input");
            let input_fields = self.mapping(&field.value, &input_path)?;
            if input_fields.is_empty() {
                self.invalid_value(
                    &field.value,
                    &input_path,
                    "Decision input must not be empty.",
                );
            }
            let mut output = Vec::new();
            for input_field in input_fields {
                if !is_identifier(&input_field.key) {
                    self.invalid_value(
                        &field.value,
                        &input_path,
                        "Decision input names must match [a-z][a-z0-9_]*.",
                    );
                }
                let field_path = format!("{input_path}.{}", input_field.key);
                if let Some(template) = self.input_template(&input_field.value, &field_path) {
                    output.push((input_field.key.clone(), template));
                }
            }
            Some(output)
        });
        let fit = fields
            .iter()
            .find(|field| field.key == "fit")
            .map_or_else(Vec::new, |field| {
                self.compile_fit(
                    &field.value,
                    &format!("{block_path}.fit"),
                    input.as_deref().unwrap_or_default(),
                )
            });
        let output_value = fields.iter().find(|field| field.key == "output");
        let output =
            output_value.and_then(|field| self.compile_choice_output(&field.value, &block_path));
        if let Some(output) = output.as_ref() {
            self.decisions = Some(DecisionSymbols {
                name: entry.key.clone(),
                question: output.question_id.clone(),
                labels: output
                    .criteria
                    .iter()
                    .map(|(label, _)| label.clone())
                    .collect(),
            });
        }
        let question_id = output.as_ref().map(|output| output.question_id.clone());
        let question = output.as_ref().map(|output| output.question.clone());
        let criteria = output.map_or_else(Vec::new, |output| output.criteria);
        Some(ChoiceDecision {
            name: entry.key.clone(),
            model: model?,
            input: input?,
            fit,
            question_id: question_id?,
            question: question?,
            criteria,
        })
    }

    fn compile_fit(
        &mut self,
        value: &ConfigValue,
        path: &str,
        input: &[(String, InputTemplate)],
    ) -> Vec<FitRule> {
        let Some(entries) = self.mapping(value, path) else {
            return Vec::new();
        };
        if entries.len() > MAX_FIT_RULES {
            self.invalid_value(
                value,
                path,
                &format!("Fit supports at most {MAX_FIT_RULES} rules."),
            );
            return Vec::new();
        }
        let mut rules: Vec<FitRule> = Vec::new();
        for entry in entries {
            let field_path = format!("{path}.{}", entry.key);
            if entry.key.len() > MAX_FIT_PATH_BYTES {
                self.invalid_value(
                    &entry.value,
                    path,
                    &format!("Fit paths support at most {MAX_FIT_PATH_BYTES} bytes."),
                );
                continue;
            }
            let Some(segments) = parse_fit_path(&entry.key) else {
                self.invalid_value(
                    &entry.value,
                    &field_path,
                    "Fit paths require dot fields and optional [*] array traversals.",
                );
                continue;
            };
            if segments.len() > MAX_FIT_PATH_SEGMENTS {
                self.invalid_value(
                    &entry.value,
                    &field_path,
                    &format!("Fit paths support at most {MAX_FIT_PATH_SEGMENTS} segments."),
                );
                continue;
            }
            if !matches!(segments.first(), Some(FitPathSegment::Field(key)) if input.iter().any(|(name, _)| name == key))
            {
                self.invalid_value(
                    &entry.value,
                    &field_path,
                    "Fit paths must start at a declared decision input.",
                );
                continue;
            }
            let strategy = match self.string(&entry.value, &field_path) {
                Some("recent") => FitStrategy::Recent,
                Some("truncate") => FitStrategy::Truncate,
                Some(_) => {
                    self.invalid_value(
                        &entry.value,
                        &field_path,
                        "Fit strategy must be recent or truncate.",
                    );
                    continue;
                }
                None => continue,
            };
            if rules
                .iter()
                .any(|rule| paths_overlap(&rule.segments, &segments))
            {
                self.invalid_value(&entry.value, &field_path, "Fit targets must not overlap.");
                continue;
            }
            rules.push(FitRule {
                path: entry.key.clone(),
                segments,
                strategy,
            });
        }
        rules
    }

    fn compile_choice_output(
        &mut self,
        value: &ConfigValue,
        decision_path: &str,
    ) -> Option<CompiledChoiceOutput> {
        let path = format!("{decision_path}.output");
        let fields = self.mapping(value, &path)?;
        if fields.len() != 1 {
            self.unsupported(
                value,
                "Exactly one Choice question is supported.".to_owned(),
                &path,
            );
        }
        let output = fields.first()?;
        if !is_identifier(&output.key) {
            self.invalid_value(value, &path, "Question names must match [a-z][a-z0-9_]*.");
        }
        let question_path = format!("{path}.{}", output.key);
        let question_fields = self.mapping(&output.value, &question_path)?;
        self.require_fields(
            question_fields,
            &["type", "question", "criteria"],
            &output.value,
            &question_path,
        );
        self.allowed_fields(
            question_fields,
            &["type", "question", "criteria"],
            &question_path,
        );
        let kind_field = question_fields.iter().find(|field| field.key == "type");
        let kind = kind_field
            .and_then(|field| self.string(&field.value, &format!("{question_path}.type")));
        if let Some(kind) = kind
            && kind != "choice"
        {
            self.unsupported(
                &kind_field.expect("type entry exists").value,
                "Only Choice questions are supported.".to_owned(),
                format!("{question_path}.type"),
            );
        }
        let question = question_fields
            .iter()
            .find(|field| field.key == "question")
            .and_then(|field| {
                self.fixed_text(Some(&field.value), &format!("{question_path}.question"))
            });
        let criteria_value = question_fields
            .iter()
            .find(|field| field.key == "criteria")
            .map(|field| &field.value);
        let criteria_fields = criteria_value
            .and_then(|value| self.mapping(value, &format!("{question_path}.criteria")));
        let mut criteria = Vec::new();
        if let Some(criteria_fields) = criteria_fields {
            if criteria_fields.len() > MAX_CHOICE_CRITERIA {
                self.invalid_value(criteria_value.expect("criteria mapping has a source value"),
                    &format!("{question_path}.criteria"),
                    &format!("The pinned Jev model supports at most {MAX_CHOICE_CRITERIA} Choice criteria."));
            }
            if criteria_fields.len() < 2 {
                self.invalid_value(
                    &output.value,
                    &format!("{question_path}.criteria"),
                    "Choice criteria must contain at least two labels.",
                );
            }
            for criterion in criteria_fields {
                if !is_identifier(&criterion.key) {
                    self.invalid_value(
                        &criterion.value,
                        &format!("{question_path}.criteria.{}", criterion.key),
                        "Choice labels must match [a-z][a-z0-9_]*.",
                    );
                }
                if let Some(description) = self.fixed_text(
                    Some(&criterion.value),
                    &format!("{question_path}.criteria.{}", criterion.key),
                ) {
                    criteria.push((criterion.key.clone(), description));
                }
            }
        }
        Some(CompiledChoiceOutput {
            question_id: output.key.clone(),
            question: question?,
            criteria,
        })
    }

    fn input_template(&mut self, value: &ConfigValue, path: &str) -> Option<InputTemplate> {
        match &value.kind {
            ConfigValueKind::Null => Some(InputTemplate::Literal(InputLiteral::Null)),
            ConfigValueKind::Boolean(value) => {
                Some(InputTemplate::Literal(InputLiteral::Boolean(*value)))
            }
            ConfigValueKind::Number(value) => {
                Some(InputTemplate::Literal(InputLiteral::Number(value.clone())))
            }
            ConfigValueKind::String(text) => self.string_template(text, value, path),
            ConfigValueKind::Sequence(items) => self.sequence_template(items, path),
            ConfigValueKind::Mapping(entries) => self.object_template(entries, value, path),
        }
    }

    fn string_template(
        &mut self,
        text: &str,
        value: &ConfigValue,
        path: &str,
    ) -> Option<InputTemplate> {
        if !text.contains("${{") {
            return Some(InputTemplate::Literal(InputLiteral::String(
                text.to_owned(),
            )));
        }
        let Some(expression_text) = whole_expression(text) else {
            self.unsupported(
                value,
                "Embedded text templates are not supported yet.".to_owned(),
                path,
            );
            return None;
        };
        self.expression(
            expression_text,
            value,
            path,
            ExpressionContext::DecisionInput,
        )
        .map(InputTemplate::Expression)
    }

    fn sequence_template(&mut self, items: &[ConfigValue], path: &str) -> Option<InputTemplate> {
        let templates = items
            .iter()
            .enumerate()
            .map(|(index, item)| self.input_template(item, &format!("{path}[{index}]")))
            .collect::<Option<Vec<_>>>()?;
        Some(InputTemplate::Sequence(templates))
    }

    fn object_template(
        &mut self,
        entries: &[ConfigEntry],
        value: &ConfigValue,
        path: &str,
    ) -> Option<InputTemplate> {
        if entries.iter().any(|entry| entry.key == "map") {
            self.unsupported(value, "Map directives are not supported yet.", path);
            return None;
        }
        let mut fields = Vec::with_capacity(entries.len());
        for entry in entries {
            if entry.key.contains("${{") {
                self.invalid_value(
                    value,
                    path,
                    "Input object keys must be fixed strings without interpolation.",
                );
            }
            let field_path = format!("{path}.{}", entry.key);
            fields.push((
                entry.key.clone(),
                self.input_template(&entry.value, &field_path)?,
            ));
        }
        Some(InputTemplate::Object(fields))
    }

    fn compile_actions(&mut self, value: &ConfigValue) -> Option<Vec<StatusAction>> {
        let items = self.sequence(value, "actions")?;
        if items.is_empty() {
            self.invalid_value(value, "actions", "At least one action is required.");
        }
        let mut actions = Vec::new();
        for (index, item) in items.iter().enumerate() {
            let path = format!("actions[{index}]");
            let Some(fields) = self.mapping(item, &path) else {
                continue;
            };
            self.require_fields(fields, &["type", "input"], item, &path);
            let kind_entry = fields.iter().find(|field| field.key == "type");
            let kind =
                kind_entry.and_then(|field| self.string(&field.value, &format!("{path}.type")));
            if kind == Some("message") {
                self.unsupported(
                    item,
                    "Message actions are not supported yet.".to_owned(),
                    &path,
                );
                continue;
            }
            if kind != Some("status") {
                self.invalid_action(
                    item,
                    &path,
                    "Action type must be `status` or a supported v1 action.",
                );
                continue;
            }
            self.allowed_fields(fields, &["type", "if", "input"], &path);
            let condition = fields
                .iter()
                .find(|field| field.key == "if")
                .and_then(|field| {
                    self.condition(
                        &field.value,
                        &format!("{path}.if"),
                        ExpressionContext::ActionCondition,
                    )
                });
            let input_field = fields.iter().find(|field| field.key == "input");
            let Some(input_field) = input_field else {
                self.missing_field(item, &format!("{path}.input"));
                continue;
            };
            let Some(input_fields) = self.mapping(&input_field.value, &format!("{path}.input"))
            else {
                continue;
            };
            self.allowed_fields(
                input_fields,
                &["result", "message"],
                &format!("{path}.input"),
            );
            self.require_fields(
                input_fields,
                &["result"],
                &input_field.value,
                &format!("{path}.input"),
            );
            let result_field = input_fields.iter().find(|field| field.key == "result");
            let Some(result_field) = result_field else {
                self.missing_field(&input_field.value, &format!("{path}.input.result"));
                continue;
            };
            let result = match self.string(&result_field.value, &format!("{path}.input.result")) {
                Some("failure") => Some(StatusResult::Failure),
                Some("success") => Some(StatusResult::Success),
                Some(result) if result.contains("${{") => {
                    self.unsupported(
                        &result_field.value,
                        "Dynamic status results are not supported yet.".to_owned(),
                        format!("{path}.input.result"),
                    );
                    None
                }
                Some(_) => {
                    self.invalid_action(
                        &result_field.value,
                        &format!("{path}.input.result"),
                        "Status result must be exactly `success` or `failure`.",
                    );
                    None
                }
                None => None,
            };
            let message = input_fields
                .iter()
                .find(|field| field.key == "message")
                .and_then(|field| {
                    let message_path = format!("{path}.input.message");
                    if matches!(&field.value.kind, ConfigValueKind::String(text) if text.contains("${{")) {
                        self.unsupported(
                            &field.value,
                            "Dynamic status messages are not supported yet.",
                            message_path,
                        );
                        None
                    } else {
                        self.fixed_text(Some(&field.value), &message_path)
                    }
                });
            if let Some(result) = result {
                actions.push(StatusAction {
                    condition,
                    result,
                    message,
                });
            }
        }
        Some(actions)
    }

    fn condition(
        &mut self,
        value: &ConfigValue,
        path: &str,
        context: ExpressionContext,
    ) -> Option<Condition> {
        match &value.kind {
            ConfigValueKind::Boolean(value) => Some(Condition::Boolean(*value)),
            ConfigValueKind::String(text) => {
                let expression_text = if let Some(body) = whole_expression(text) {
                    body
                } else if text.contains("${{") {
                    self.unsupported(
                        value,
                        "Embedded text templates are not supported yet.".to_owned(),
                        path,
                    );
                    return None;
                } else if text.starts_with("${{") {
                    self.invalid_expression(value, path, "Expression wrapper must end with `}}`.");
                    return None;
                } else {
                    text
                };
                self.expression(expression_text, value, path, context)
                    .map(Condition::Expression)
            }
            _ => {
                self.invalid_type(value, path, "a Boolean or expression string");
                None
            }
        }
    }

    fn expression(
        &mut self,
        expression_text: &str,
        value: &ConfigValue,
        path: &str,
        context: ExpressionContext,
    ) -> Option<Expression> {
        let expression = match parse_expression(expression_text) {
            Ok(expression) => expression,
            Err(error) => {
                self.diagnostics.push(
                    Diagnostic::at(error.code, error.message, value.span)
                        .with_config_path(path.to_owned()),
                );
                return None;
            }
        };
        let mut paths = Vec::new();
        expression.visit_paths(&mut |segments| paths.push(segments.to_vec()));
        for segments in paths {
            if let Err((code, message)) = self.validate_path(&segments, context) {
                self.diagnostics.push(
                    Diagnostic::at(code, message, value.span).with_config_path(path.to_owned()),
                );
            }
        }
        Some(expression)
    }

    fn validate_path(
        &self,
        path: &[PathSegment],
        context: ExpressionContext,
    ) -> Result<(), (DiagnosticCode, String)> {
        let properties = path
            .iter()
            .map(|segment| match segment {
                PathSegment::Property(value) => Some(value.as_str()),
                PathSegment::Index(_) => None,
            })
            .collect::<Vec<_>>();
        let Some(Some(root)) = properties.first() else {
            return Err(reference_error(
                "Expression paths must start with a context root.",
            ));
        };
        let root_name = *root;
        if root_name == "window" {
            if context == ExpressionContext::TopLevel {
                return Err(unsupported_reference(
                    "The `window` root is unavailable to top-level conditions.",
                ));
            }
            if !self.has_window {
                return Err(reference_error("The `window` root requires `on.window`."));
            }
            return validate_window_path(path);
        }
        if root_name == "decisions" {
            if context != ExpressionContext::ActionCondition {
                return Err(unsupported_reference(
                    "The `decisions` root is available only to action conditions.",
                ));
            }
            return self.validate_decision_path(&properties);
        }
        match root_name {
            "session" => validate_session_path(path),
            "message" => validate_message_path(path),
            "agent" => validate_agent_path(path),
            "repo" => validate_repo_path(path),
            "global" | "turn" => Err(unsupported_reference(&format!(
                "The `{root_name}` root is not supported yet."
            ))),
            _ => Err(reference_error(&format!(
                "Unknown context root `{root_name}`."
            ))),
        }
    }

    fn validate_decision_path(
        &self,
        properties: &[Option<&str>],
    ) -> Result<(), (DiagnosticCode, String)> {
        let Some(symbols) = &self.decisions else {
            return Err(reference_error(
                "Decision results are unavailable without a compiled decision.",
            ));
        };
        if properties.get(1).copied().flatten() != Some(symbols.name.as_str()) {
            return Err(reference_error("The expression names an unknown decision."));
        }
        match properties.get(2).copied().flatten() {
            Some("outputs") => {
                if properties.get(3).copied().flatten() == Some(symbols.question.as_str())
                    && properties.len() == 4
                {
                    Ok(())
                } else {
                    Err(reference_error(
                        "The expression names an unknown Choice output.",
                    ))
                }
            }
            Some("details") => {
                if properties.get(3).copied().flatten() == Some(symbols.question.as_str())
                    && properties.get(4).copied().flatten() == Some("probabilities")
                    && properties
                        .get(5)
                        .copied()
                        .flatten()
                        .is_some_and(|label| symbols.labels.contains(label))
                    && properties.len() == 6
                {
                    Ok(())
                } else {
                    Err(unsupported_reference(
                        "Only declared Choice probabilities are available in action conditions.",
                    ))
                }
            }
            _ => Err(reference_error(
                "Decision paths must read the declared output or Choice probabilities.",
            )),
        }
    }

    fn mapping<'a>(&mut self, value: &'a ConfigValue, path: &str) -> Option<&'a [ConfigEntry]> {
        match &value.kind {
            ConfigValueKind::Mapping(entries) => Some(entries),
            _ => {
                self.invalid_type(value, path, "an object");
                None
            }
        }
    }

    fn sequence<'a>(&mut self, value: &'a ConfigValue, path: &str) -> Option<&'a [ConfigValue]> {
        match &value.kind {
            ConfigValueKind::Sequence(items) => Some(items),
            _ => {
                self.invalid_type(value, path, "an array");
                None
            }
        }
    }

    fn string<'a>(&mut self, value: &'a ConfigValue, path: &str) -> Option<&'a str> {
        match &value.kind {
            ConfigValueKind::String(text) => Some(text),
            _ => {
                self.invalid_type(value, path, "a string");
                None
            }
        }
    }

    fn fixed_text(&mut self, value: Option<&ConfigValue>, path: &str) -> Option<String> {
        let value = value?;
        let text = self.string(value, path)?;
        if text.trim().is_empty() || text.contains("${{") {
            self.invalid_value(
                value,
                path,
                "Fixed text must be nonblank and cannot contain `${{`.",
            );
            None
        } else {
            Some(text.to_owned())
        }
    }

    fn allowed_fields(&mut self, fields: &[ConfigEntry], allowed: &[&str], prefix: &str) {
        for field in fields {
            if !allowed.contains(&field.key.as_str()) {
                self.diagnostics.push(
                    Diagnostic::at(
                        DiagnosticCode::ConfigUnknownField,
                        format!("Unknown field `{}`.", field.key),
                        field.key_span,
                    )
                    .with_config_path(join_path(prefix, &field.key)),
                );
            }
        }
    }

    fn require_fields(
        &mut self,
        fields: &[ConfigEntry],
        required: &[&str],
        parent: &ConfigValue,
        prefix: &str,
    ) {
        for field in required {
            if !fields.iter().any(|entry| entry.key == *field) {
                self.missing_field(parent, &join_path(prefix, field));
            }
        }
    }

    fn invalid_type(&mut self, value: &ConfigValue, path: &str, expected: &str) {
        self.push(
            DiagnosticCode::ConfigInvalidType,
            format!("Expected {expected} at `{path}`."),
            value,
            path,
        );
    }

    fn invalid_value(&mut self, value: &ConfigValue, path: &str, message: &str) {
        self.push(DiagnosticCode::ConfigInvalidValue, message, value, path);
    }

    fn unsupported(
        &mut self,
        value: &ConfigValue,
        message: impl Into<String>,
        path: impl Into<String>,
    ) {
        self.diagnostics.push(
            Diagnostic::at(
                DiagnosticCode::CompileUnsupportedFeature,
                message,
                value.span,
            )
            .with_config_path(path),
        );
    }

    fn missing_field(&mut self, parent: &ConfigValue, path: &str) {
        self.push(
            DiagnosticCode::ConfigMissingField,
            format!("Required field `{path}` is missing."),
            parent,
            path,
        );
    }

    fn invalid_window(&mut self, value: &ConfigValue, message: &str) {
        self.push(
            DiagnosticCode::CompileInvalidWindow,
            message,
            value,
            "on.window",
        );
    }

    fn invalid_cooldown(&mut self, value: &ConfigValue, path: &str, message: &str) {
        self.push(DiagnosticCode::CompileInvalidCooldown, message, value, path);
    }

    fn invalid_expression(&mut self, value: &ConfigValue, path: &str, message: &str) {
        self.push(
            DiagnosticCode::CompileInvalidExpression,
            message,
            value,
            path,
        );
    }

    fn invalid_action(&mut self, value: &ConfigValue, path: &str, message: &str) {
        self.push(DiagnosticCode::CompileInvalidAction, message, value, path);
    }

    fn push(
        &mut self,
        code: DiagnosticCode,
        message: impl Into<String>,
        value: &ConfigValue,
        path: &str,
    ) {
        self.diagnostics
            .push(Diagnostic::at(code, message, value.span).with_config_path(path.to_owned()));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExpressionContext {
    TopLevel,
    DecisionInput,
    ActionCondition,
}

fn whole_expression(value: &str) -> Option<&str> {
    let body = value.strip_prefix("${{")?.strip_suffix("}}")?;
    if body.contains("${{") || body.contains("}}") {
        return None;
    }
    Some(body)
}

fn is_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn join_path(prefix: &str, field: &str) -> String {
    if prefix.is_empty() {
        field.to_owned()
    } else {
        format!("{prefix}.{field}")
    }
}

fn reference_error(message: &str) -> (DiagnosticCode, String) {
    (DiagnosticCode::CompileUnknownReference, message.to_owned())
}

fn unsupported_reference(message: &str) -> (DiagnosticCode, String) {
    (
        DiagnosticCode::CompileUnsupportedFeature,
        message.to_owned(),
    )
}

fn validate_agent_path(path: &[PathSegment]) -> Result<(), (DiagnosticCode, String)> {
    match path {
        [PathSegment::Property(root), PathSegment::Property(field)]
            if root == "agent" && field == "name" =>
        {
            Ok(())
        }
        [PathSegment::Property(root), PathSegment::Property(field)]
            if root == "agent" && field == "skills" =>
        {
            Err(unsupported_reference(
                "`agent.skills` is not supported yet.",
            ))
        }
        _ => Err(reference_error("Only `agent.name` is supported.")),
    }
}

fn validate_repo_path(path: &[PathSegment]) -> Result<(), (DiagnosticCode, String)> {
    match path {
        [PathSegment::Property(root), PathSegment::Property(field)]
            if root == "repo" && field == "root" =>
        {
            Ok(())
        }
        [PathSegment::Property(root), PathSegment::Property(field)]
            if root == "repo" && matches!(field.as_str(), "branch" | "instructions" | "skills") =>
        {
            Err(unsupported_reference(&format!(
                "`repo.{field}` is not supported yet."
            )))
        }
        _ => Err(reference_error("Only `repo.root` is supported.")),
    }
}

fn validate_session_path(path: &[PathSegment]) -> Result<(), (DiagnosticCode, String)> {
    match path {
        [PathSegment::Property(root), PathSegment::Property(field)]
            if root == "session"
                && matches!(
                    field.as_str(),
                    "id" | "requests_complete" | "source_coverage"
                ) =>
        {
            Ok(())
        }
        [PathSegment::Property(root), PathSegment::Property(field)]
            if root == "session" && field == "requests" =>
        {
            Ok(())
        }
        [
            PathSegment::Property(root),
            PathSegment::Property(field),
            PathSegment::Property(length),
        ] if root == "session" && field == "requests" && length == "length" => Ok(()),
        [
            PathSegment::Property(root),
            PathSegment::Property(field),
            PathSegment::Index(_),
        ] if root == "session" && field == "requests" => Ok(()),
        [
            PathSegment::Property(root),
            PathSegment::Property(field),
            PathSegment::Index(_),
            PathSegment::Property(request_field),
        ] if root == "session"
            && field == "requests"
            && matches!(
                request_field.as_str(),
                "id" | "source" | "kind" | "text" | "in_reply_to"
            ) =>
        {
            Ok(())
        }
        [PathSegment::Property(root), PathSegment::Property(field)]
            if root == "session"
                && matches!(
                    field.as_str(),
                    "model" | "fast_mode" | "start_reason" | "idle_seconds" | "last_activity_at"
                ) =>
        {
            Err(unsupported_reference(&format!(
                "`session.{field}` is not supported yet."
            )))
        }
        _ => Err(reference_error(
            "The expression reads a field outside the supported session data.",
        )),
    }
}

fn validate_window_path(path: &[PathSegment]) -> Result<(), (DiagnosticCode, String)> {
    match path {
        [PathSegment::Property(root), PathSegment::Property(messages)]
            if root == "window" && messages == "messages" =>
        {
            Ok(())
        }
        [
            PathSegment::Property(root),
            PathSegment::Property(messages),
            PathSegment::Property(length),
        ] if root == "window" && messages == "messages" && length == "length" => Ok(()),
        [
            PathSegment::Property(root),
            PathSegment::Property(messages),
            PathSegment::Index(_),
        ] if root == "window" && messages == "messages" => Ok(()),
        [
            PathSegment::Property(root),
            PathSegment::Property(messages),
            PathSegment::Index(_),
            ..,
        ] if root == "window" && messages == "messages" => validate_message_path_from(path, 3),
        _ => Err(reference_error(
            "`window.messages` is available only when a rolling window is configured.",
        )),
    }
}

fn validate_message_path(path: &[PathSegment]) -> Result<(), (DiagnosticCode, String)> {
    validate_message_path_from(path, 1)
}

fn validate_message_path_from(
    path: &[PathSegment],
    start: usize,
) -> Result<(), (DiagnosticCode, String)> {
    let rest = &path[start..];
    if rest.is_empty() {
        return Ok(());
    }
    let field = match &rest[0] {
        PathSegment::Property(field) => field.as_str(),
        PathSegment::Index(_) => {
            return Err(reference_error(
                "A message object cannot use an array index.",
            ));
        }
    };
    if matches!(
        field,
        "id" | "source"
            | "occurred_at"
            | "kind"
            | "role"
            | "model"
            | "fast_mode"
            | "content"
            | "quality"
    ) {
        return if rest.len() == 1 || field == "content" {
            Ok(())
        } else {
            Err(reference_error(&format!(
                "`message.{field}` has no supported child fields."
            )))
        };
    }
    if field != "tool" {
        return Err(reference_error(&format!(
            "Unknown normalized message field `message.{field}`."
        )));
    }
    validate_tool_path(&rest[1..])
}

fn validate_tool_path(path: &[PathSegment]) -> Result<(), (DiagnosticCode, String)> {
    if path.is_empty() {
        return Ok(());
    }
    let field = match &path[0] {
        PathSegment::Property(field) => field.as_str(),
        PathSegment::Index(_) => {
            return Err(reference_error("A tool object cannot use an array index."));
        }
    };
    match field {
        "name" | "category" | "call_id" => {
            if path.len() == 1 {
                Ok(())
            } else {
                Err(reference_error(&format!(
                    "`message.tool.{field}` has no supported child fields."
                )))
            }
        }
        "arguments" => Ok(()),
        "paths" => match path {
            [_] => Ok(()),
            [_, PathSegment::Property(length)] if length == "length" => Ok(()),
            [_, PathSegment::Index(_)] => Ok(()),
            _ => Err(reference_error(
                "Only length and fixed indexes are supported on `message.tool.paths`.",
            )),
        },
        "result" => validate_result_path(&path[1..]),
        _ => Err(reference_error(&format!(
            "Unknown normalized tool field `message.tool.{field}`."
        ))),
    }
}

fn validate_result_path(path: &[PathSegment]) -> Result<(), (DiagnosticCode, String)> {
    if path.is_empty() {
        return Ok(());
    }
    let field = match &path[0] {
        PathSegment::Property(field) => field.as_str(),
        PathSegment::Index(_) => {
            return Err(reference_error("A tool result cannot use an array index."));
        }
    };
    match field {
        "status" | "output_bytes" => {
            if path.len() == 1 {
                Ok(())
            } else {
                Err(reference_error(&format!(
                    "`message.tool.result.{field}` has no supported child fields."
                )))
            }
        }
        "content" => Ok(()),
        _ => Err(reference_error(&format!(
            "Unknown normalized tool result field `message.tool.result.{field}`."
        ))),
    }
}
