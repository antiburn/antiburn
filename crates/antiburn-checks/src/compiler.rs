//! Typed admission for the Burn Check features supported by this release.

use std::collections::HashSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Number;

use crate::config::{ConfigEntry, ConfigValue, ConfigValueKind, YamlDocument, parse_yaml};
use crate::decision::{DecisionBlock, NamedQuestion, Question, QuestionKind, QuestionRequirements};
use crate::diagnostics::{Diagnostic, DiagnosticCode};
use crate::expression::{Expression, parse_expression};
use crate::filters::{RepositoryGlob, is_model_identifier};
use crate::fit::{
    FitPathSegment, FitRule, FitStrategy, MAX_FIT_PATH_BYTES, MAX_FIT_PATH_SEGMENTS, MAX_FIT_RULES,
    parse_fit_path, paths_overlap,
};

const MAX_WINDOW_SIZE: u64 = 40;
const PUBLIC_ROOTS: &[&str] = &[
    "session",
    "turn",
    "message",
    "window",
    "global",
    "repo",
    "agent",
    "decisions",
];

mod access;
mod inventories;
mod requirements;
mod triggers;
use access::ValueShape;
pub use inventories::CurrentInventoryDemand;

/// One fully validated check that uses only supported capabilities.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledCheck {
    /// The display name from the definition.
    pub name: String,
    /// The optional fixed description.
    pub description: Option<String>,
    /// A pre-scheduling condition, when configured.
    pub condition: Option<Condition>,
    /// The selected event family, its filters, and scheduling settings.
    pub trigger: ToolResultTrigger,
    /// Independent model decisions in authored order. Empty for local checks.
    pub decisions: Vec<DecisionBlock>,
    /// Ordered status actions. The runtime evaluates each condition separately.
    pub actions: Vec<StatusAction>,
}

/// A Boolean condition or parsed expression.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Condition {
    /// A literal YAML Boolean.
    Boolean(bool),
    /// A parsed expression.
    Expression(Expression),
}

/// Event alternatives and scheduling settings. Legacy fields filter result events.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResultTrigger {
    /// Exactly one event family, with alternatives inside that family.
    pub family: TriggerFamily,
    /// Optional normalized tool categories. An absent list means all categories.
    pub tools: Option<Vec<ToolCategory>>,
    /// Optional final result statuses. An absent list means all statuses.
    pub statuses: Option<Vec<ToolResultStatus>>,
    /// Exact requester model identities at message scope.
    pub models: Option<Vec<String>>,
    /// Proven effective requester mode at message scope.
    pub fast_mode: Option<bool>,
    /// At least one recorded repository-relative path must match.
    pub paths: Option<Vec<RepositoryGlob>>,
    /// Any matching recorded path vetoes the event.
    pub paths_ignore: Option<Vec<RepositoryGlob>>,
    /// Optional rolling message window.
    pub window: Option<RollingWindow>,
    /// The run cooldown. Defaults to zero locally and 60 seconds for models.
    pub cooldown: Duration,
}

/// Event alternatives from one authored family.
#[derive(Clone, Debug, PartialEq)]
pub enum TriggerFamily {
    Messages {
        tool_result: bool,
        user_message: Option<EventFilters>,
        assistant_message: Option<EventFilters>,
        tool_call: Option<EventFilters>,
    },
    ResponseCycle(EventFilters),
    Session {
        session_start: Option<EventFilters>,
        session_idle: Option<EventFilters>,
    },
}

/// Filters for one non-result event alternative.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventFilters {
    pub tools: Option<Vec<ToolCategory>>,
    pub statuses: Option<Vec<ToolResultStatus>>,
    pub models: Option<Vec<String>>,
    pub fast_mode: Option<bool>,
    pub paths: Option<Vec<RepositoryGlob>>,
    pub paths_ignore: Option<Vec<RepositoryGlob>>,
    pub reasons: Option<Vec<crate::domain::SessionStartReason>>,
}

/// A normalized tool category accepted by the current filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

impl ToolCategory {
    /// Returns the check-facing category without changing native metric categories.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Read => "read",
            Self::Search => "search",
            Self::Edit => "edit",
            Self::Write => "write",
            Self::Delegate => "delegate",
            Self::Skill => "skill",
            Self::Mcp => "mcp",
            Self::Other => "other",
        }
    }
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

/// A recursively prepared decision input value.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum InputTemplate {
    /// A JSON-compatible literal.
    Literal(InputLiteral),
    /// A typed whole-value expression.
    Expression(Expression),
    /// Text with literal parts and expressions evaluated once.
    Text(Vec<TemplatePart>),
    /// An ordered array template.
    Sequence(Vec<Self>),
    /// A fixed-key object template.
    Object(Vec<(String, Self)>),
    /// An ordered, optionally filtered array mapping.
    Map {
        /// The expression that selects the source array.
        collection: Expression,
        /// The item name visible only to the condition and value.
        alias: String,
        /// The optional item filter.
        condition: Option<Condition>,
        /// The template evaluated for each included item.
        value: Box<Self>,
    },
}

/// One literal or expression part of an embedded text template.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum TemplatePart {
    /// Fixed authored text.
    Literal(String),
    /// An expression converted with scalar text rules.
    Expression(Expression),
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

/// An authored fixed status or a whole-value expression.
#[derive(Clone, Debug, PartialEq)]
pub enum StatusResultTemplate {
    /// A validated fixed result.
    Literal(StatusResult),
    /// An expression that must produce exactly `success` or `failure`.
    Expression(Expression),
}

impl From<StatusResult> for StatusResultTemplate {
    fn from(value: StatusResult) -> Self {
        Self::Literal(value)
    }
}

/// One independently gated, ordered status action.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusAction {
    /// The action condition, when configured.
    pub condition: Option<Condition>,
    /// The status value evaluated when the condition passes.
    pub result: StatusResultTemplate,
    /// Optional user-facing explanation template.
    pub message: Option<InputTemplate>,
}

impl CompiledCheck {
    /// Returns where current file inventories can affect this check.
    #[must_use]
    pub fn current_inventory_demand(&self) -> CurrentInventoryDemand {
        inventories::inspect(self)
    }

    /// Returns whether actions run without model decisions.
    #[must_use]
    pub fn is_local(&self) -> bool {
        self.decisions.is_empty()
    }

    /// Returns provider requirements without evaluating action conditions.
    ///
    /// The host must check every reference, including references in branches
    /// that can be false. This does not admit unsupported language features.
    pub fn question_requirements(&self) -> Vec<QuestionRequirements> {
        let mut required = Vec::new();
        for decision in &self.decisions {
            for question in &decision.outputs {
                let mut requirements = QuestionRequirements {
                    decision: decision.name.clone(),
                    question: question.name.clone(),
                    kind: question.question.kind(),
                    criteria_count: question.question.criteria_count(),
                    probabilities: false,
                    confidence: false,
                };
                for action in &self.actions {
                    let mut inspect = |expression: &Expression| {
                        requirements::inspect(expression, decision, question, &mut requirements);
                    };
                    if let Some(Condition::Expression(expression)) = &action.condition {
                        inspect(expression);
                    }
                    if let StatusResultTemplate::Expression(expression) = &action.result {
                        inspect(expression);
                    }
                    if let Some(template) = &action.message {
                        template.visit_expressions(&mut inspect);
                    }
                }
                required.push(requirements);
            }
        }
        required
    }
}

impl InputTemplate {
    fn visit_expressions(&self, visitor: &mut impl FnMut(&Expression)) {
        match self {
            Self::Literal(_) => {}
            Self::Expression(expression) => visitor(expression),
            Self::Text(parts) => {
                for part in parts {
                    if let TemplatePart::Expression(expression) = part {
                        visitor(expression);
                    }
                }
            }
            Self::Sequence(items) => {
                for item in items {
                    item.visit_expressions(visitor);
                }
            }
            Self::Object(items) => {
                for (_, item) in items {
                    item.visit_expressions(visitor);
                }
            }
            Self::Map {
                collection,
                condition,
                value,
                ..
            } => {
                visitor(collection);
                if let Some(Condition::Expression(expression)) = condition {
                    visitor(expression);
                }
                value.visit_expressions(visitor);
            }
        }
    }
}

/// Parses YAML and compiles it against the application's supported feature set.
///
/// This validates authored configuration. It does not evaluate expressions,
/// read evidence, call a provider, or execute actions.
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
        decisions: Vec::new(),
        aliases: Vec::new(),
        trigger_family: None,
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
    decisions: Vec<DecisionSymbols>,
    aliases: Vec<(String, ValueShape)>,
    trigger_family: Option<TriggerFamily>,
}

#[derive(Clone)]
struct DecisionSymbols {
    name: String,
    outputs: Vec<QuestionSymbols>,
}

#[derive(Clone)]
struct QuestionSymbols {
    name: String,
    kind: QuestionKind,
    labels: HashSet<String>,
    criteria_count: usize,
}

impl Validator {
    fn compile_root(&mut self, root: &ConfigValue) -> Option<CompiledCheck> {
        let fields = self.mapping(root, "")?;
        self.allowed_fields(
            fields,
            &["name", "description", "on", "if", "decisions", "actions"],
            "",
        );

        for required_name in ["name", "on", "actions"] {
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
        let decision_entry = fields.iter().find(|entry| entry.key == "decisions");
        let default_cooldown = if decision_entry.is_some() {
            Duration::from_secs(60)
        } else {
            Duration::ZERO
        };
        let trigger = fields
            .iter()
            .find(|entry| entry.key == "on")
            .and_then(|entry| self.compile_trigger(&entry.value, default_cooldown));
        self.has_window = trigger.as_ref().is_some_and(|value| value.window.is_some());
        self.trigger_family = trigger.as_ref().map(|value| value.family.clone());
        let condition = fields
            .iter()
            .find(|entry| entry.key == "if")
            .and_then(|entry| self.condition(&entry.value, "if", ExpressionContext::TopLevel));
        let decisions = match decision_entry {
            Some(entry) => self.compile_decisions(&entry.value)?,
            None => Vec::new(),
        };
        let actions = fields
            .iter()
            .find(|entry| entry.key == "actions")
            .and_then(|entry| self.compile_actions(&entry.value));

        let (Some(name), Some(trigger), Some(actions)) = (name, trigger, actions) else {
            return None;
        };
        Some(CompiledCheck {
            name,
            description,
            condition,
            trigger,
            decisions,
            actions,
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
                "write" => ToolCategory::Write,
                "delegate" => ToolCategory::Delegate,
                "skill" => ToolCategory::Skill,
                "mcp" => ToolCategory::Mcp,
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

    fn parse_models(&mut self, value: &ConfigValue, path: &str) -> Option<Vec<String>> {
        let items = self.unique_strings(value, path)?;
        for (item, text) in &items {
            if text.contains("${{") {
                self.invalid_value(
                    item,
                    path,
                    "Model filters must be fixed text without `${{`.",
                );
            } else if !is_model_identifier(text) {
                self.invalid_value(item, path, "Use an exact provider/model identifier with nonempty segments and no whitespace.");
            }
        }
        Some(items.into_iter().map(|(_, text)| text).collect())
    }

    fn parse_path_globs(&mut self, value: &ConfigValue, path: &str) -> Option<Vec<RepositoryGlob>> {
        let items = self.unique_strings(value, path)?;
        Some(
            items
                .into_iter()
                .filter_map(|(item, text)| match RepositoryGlob::parse(&text) {
                    Ok(glob) => Some(glob),
                    Err(message) => {
                        self.invalid_value(item, path, message);
                        None
                    }
                })
                .collect(),
        )
    }

    fn unique_strings<'a>(
        &mut self,
        value: &'a ConfigValue,
        path: &str,
    ) -> Option<Vec<(&'a ConfigValue, String)>> {
        let items = self.sequence(value, path)?;
        if items.is_empty() {
            self.invalid_value(value, path, "The filter list must not be empty.");
        }
        let mut seen = HashSet::new();
        let mut output = Vec::new();
        for item in items {
            let Some(text) = self.string(item, path) else {
                continue;
            };
            if !seen.insert(text.to_owned()) {
                self.invalid_value(item, path, "Filter values must be unique.");
                continue;
            }
            output.push((item, text.to_owned()));
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

    fn compile_decisions(&mut self, value: &ConfigValue) -> Option<Vec<DecisionBlock>> {
        let path = "decisions";
        let entries = self.mapping(value, path)?;
        if entries.is_empty() {
            self.invalid_value(value, path, "Decisions must contain at least one block.");
            return None;
        }
        let decisions = entries
            .iter()
            .filter_map(|entry| self.compile_decision(entry))
            .collect::<Vec<_>>();
        self.decisions = decisions
            .iter()
            .map(|decision| DecisionSymbols {
                name: decision.name.clone(),
                outputs: decision
                    .outputs
                    .iter()
                    .map(|output| QuestionSymbols {
                        name: output.name.clone(),
                        kind: output.question.kind(),
                        labels: match &output.question {
                            Question::Choice { criteria, .. } => {
                                criteria.iter().map(|(label, _)| label.clone()).collect()
                            }
                            _ => HashSet::new(),
                        },
                        criteria_count: output.question.criteria_count(),
                    })
                    .collect(),
            })
            .collect();
        Some(decisions)
    }

    fn compile_decision(&mut self, entry: &ConfigEntry) -> Option<DecisionBlock> {
        if !is_identifier(&entry.key) {
            self.invalid_value(
                &entry.value,
                "decisions",
                "Decision names must match [a-z][a-z0-9_]*.",
            );
        }
        let fields = self.mapping(&entry.value, &format!("decisions.{}", entry.key))?;
        let block_path = format!("decisions.{}", entry.key);
        self.allowed_fields(fields, &["input", "fit", "output"], &block_path);
        self.require_fields(fields, &["input", "output"], &entry.value, &block_path);
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
        let outputs =
            output_value.and_then(|field| self.compile_outputs(&field.value, &block_path));
        Some(DecisionBlock {
            name: entry.key.clone(),
            input: InputTemplate::Object(input?),
            fit,
            outputs: outputs?,
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

    fn compile_outputs(
        &mut self,
        value: &ConfigValue,
        decision_path: &str,
    ) -> Option<Vec<NamedQuestion>> {
        let path = format!("{decision_path}.output");
        let fields = self.mapping(value, &path)?;
        if fields.is_empty() || fields.len() > crate::decision::MAX_QUESTIONS_PER_DECISION {
            self.invalid_value(
                value,
                &path,
                "Each decision requires one to eight questions.",
            );
        }
        Some(
            fields
                .iter()
                .filter_map(|output| self.compile_question(output, &path))
                .collect(),
        )
    }

    fn compile_question(&mut self, output: &ConfigEntry, path: &str) -> Option<NamedQuestion> {
        if !is_identifier(&output.key) {
            self.invalid_value(
                &output.value,
                path,
                "Question names must match [a-z][a-z0-9_]*.",
            );
        }
        let question_path = format!("{path}.{}", output.key);
        let question_fields = self.mapping(&output.value, &question_path)?;
        self.require_fields(
            question_fields,
            &["type", "question"],
            &output.value,
            &question_path,
        );
        let kind_field = question_fields.iter().find(|field| field.key == "type");
        let kind = kind_field
            .and_then(|field| self.string(&field.value, &format!("{question_path}.type")));
        let kind = kind?;
        self.allowed_fields(
            question_fields,
            if kind == "noul" {
                &["type", "question"]
            } else {
                &["type", "question", "criteria"]
            },
            &question_path,
        );
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
        if matches!(kind, "choice" | "score") && criteria_value.is_none() {
            self.missing_field(&output.value, &format!("{question_path}.criteria"));
        }
        let question = question?;
        let criteria_path = format!("{question_path}.criteria");
        let question = match kind {
            "choice" => Question::Choice {
                question,
                criteria: self.compile_choice_criteria(criteria_value?, &criteria_path)?,
            },
            "score" => {
                let criteria_value = criteria_value?;
                let items = self.sequence(criteria_value, &criteria_path)?;
                if items.len() < 2 {
                    self.invalid_value(
                        criteria_value,
                        &criteria_path,
                        "Score criteria must contain at least two descriptions.",
                    );
                }
                let criteria = items
                    .iter()
                    .enumerate()
                    .filter_map(|(index, item)| {
                        self.fixed_text(Some(item), &format!("{criteria_path}[{index}]"))
                    })
                    .collect();
                Question::Score { question, criteria }
            }
            "noul" => Question::Noul { question },
            _ => {
                self.unsupported(
                    &kind_field.expect("type entry exists").value,
                    "Question type must be choice, score, or noul.".to_owned(),
                    format!("{question_path}.type"),
                );
                return None;
            }
        };
        Some(NamedQuestion {
            name: output.key.clone(),
            question,
        })
    }

    fn compile_choice_criteria(
        &mut self,
        value: &ConfigValue,
        path: &str,
    ) -> Option<Vec<(String, String)>> {
        let criteria_fields = self.mapping(value, path)?;
        let mut criteria = Vec::new();
        if criteria_fields.len() < 2 {
            self.invalid_value(
                value,
                path,
                "Choice criteria must contain at least two labels.",
            );
        }
        for criterion in criteria_fields {
            if !is_identifier(&criterion.key) {
                self.invalid_value(
                    &criterion.value,
                    &format!("{path}.{}", criterion.key),
                    "Choice labels must match [a-z][a-z0-9_]*.",
                );
            }
            if let Some(description) =
                self.fixed_text(Some(&criterion.value), &format!("{path}.{}", criterion.key))
            {
                criteria.push((criterion.key.clone(), description));
            }
        }
        Some(criteria)
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
            ConfigValueKind::String(text) => {
                self.string_template(text, value, path, ExpressionContext::DecisionInput)
            }
            ConfigValueKind::Sequence(items) => self.sequence_template(items, path),
            ConfigValueKind::Mapping(entries) => self.object_template(entries, value, path),
        }
    }

    fn string_template(
        &mut self,
        text: &str,
        value: &ConfigValue,
        path: &str,
        context: ExpressionContext,
    ) -> Option<InputTemplate> {
        if !text.contains("${{") {
            return Some(InputTemplate::Literal(InputLiteral::String(
                text.to_owned(),
            )));
        }
        if let Some(expression_text) = whole_expression(text) {
            return self
                .expression(expression_text, value, path, context)
                .map(InputTemplate::Expression);
        }
        let mut parts = Vec::new();
        let mut remaining = text;
        while let Some(start) = remaining.find("${{") {
            if start > 0 {
                parts.push(TemplatePart::Literal(remaining[..start].to_owned()));
            }
            let body = &remaining[start + 3..];
            let Some(end) = expression_end(body) else {
                self.invalid_expression(value, path, "Expression wrapper must end with `}}`.");
                return None;
            };
            parts.push(TemplatePart::Expression(self.expression(
                &body[..end],
                value,
                path,
                context,
            )?));
            remaining = &body[end + 2..];
        }
        if !remaining.is_empty() {
            parts.push(TemplatePart::Literal(remaining.to_owned()));
        }
        Some(InputTemplate::Text(parts))
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
            return self.map_template(entries, value, path);
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

    fn map_template(
        &mut self,
        entries: &[ConfigEntry],
        value: &ConfigValue,
        path: &str,
    ) -> Option<InputTemplate> {
        self.allowed_fields(entries, &["map", "as", "if", "value"], path);
        self.require_fields(entries, &["map", "as", "value"], value, path);
        let collection_field = entries.iter().find(|entry| entry.key == "map")?;
        let collection_path = format!("{path}.map");
        let text = self.string(&collection_field.value, &collection_path)?;
        let Some(body) = whole_expression(text) else {
            self.invalid_expression(
                &collection_field.value,
                &collection_path,
                "Map collection must be a whole `${{ expression }}`.",
            );
            return None;
        };
        let collection = self.expression(
            body,
            &collection_field.value,
            &collection_path,
            ExpressionContext::DecisionInput,
        )?;
        let alias_field = entries.iter().find(|entry| entry.key == "as")?;
        let alias = self
            .string(&alias_field.value, &format!("{path}.as"))?
            .to_owned();
        if !is_identifier(&alias)
            || PUBLIC_ROOTS.contains(&alias.as_str())
            || self.aliases.iter().any(|(name, _)| name == &alias)
        {
            self.invalid_value(
                &alias_field.value,
                &format!("{path}.as"),
                "Map aliases must be identifiers and must not shadow roots or enclosing aliases.",
            );
            return None;
        }
        let collection_shape = self
            .resolve_expression(&collection, ExpressionContext::DecisionInput)
            .ok()?;
        let item_shape = match collection_shape.map_item() {
            Some(item) => item,
            None => {
                self.invalid_value(
                    &collection_field.value,
                    &collection_path,
                    "Map collection must resolve to an array.",
                );
                return None;
            }
        };
        self.aliases.push((alias.clone(), item_shape));
        let condition = entries
            .iter()
            .find(|entry| entry.key == "if")
            .and_then(|entry| {
                self.condition(
                    &entry.value,
                    &format!("{path}.if"),
                    ExpressionContext::DecisionInput,
                )
            });
        let item = entries
            .iter()
            .find(|entry| entry.key == "value")
            .and_then(|entry| self.input_template(&entry.value, &format!("{path}.value")));
        self.aliases.pop();
        Some(InputTemplate::Map {
            collection,
            alias,
            condition,
            value: Box::new(item?),
        })
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
                Some("failure") => Some(StatusResultTemplate::Literal(StatusResult::Failure)),
                Some("success") => Some(StatusResultTemplate::Literal(StatusResult::Success)),
                Some(result) if result.contains("${{") => {
                    if let Some(body) = whole_expression(result) {
                        self.expression(
                            body,
                            &result_field.value,
                            &format!("{path}.input.result"),
                            ExpressionContext::ActionCondition,
                        )
                        .map(StatusResultTemplate::Expression)
                    } else {
                        self.invalid_action(
                            &result_field.value,
                            &format!("{path}.input.result"),
                            "Status result must be a fixed result or a whole expression.",
                        );
                        None
                    }
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
                    let text = self.string(&field.value, &message_path)?;
                    if text.trim().is_empty() {
                        self.invalid_action(
                            &field.value,
                            &message_path,
                            "Status message must be a nonblank string.",
                        );
                        return None;
                    }
                    self.string_template(
                        text,
                        &field.value,
                        &message_path,
                        ExpressionContext::ActionCondition,
                    )
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
        if let Err((code, message)) = self.resolve_expression(&expression, context) {
            self.diagnostics
                .push(Diagnostic::at(code, message, value.span).with_config_path(path.to_owned()));
        }
        Some(expression)
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
    let body = value.strip_prefix("${{")?;
    let end = expression_end(body)?;
    (end + 2 == body.len()).then_some(&body[..end])
}

fn expression_end(body: &str) -> Option<usize> {
    let mut quoted = false;
    let mut characters = body.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if character == '\'' {
            if quoted && characters.peek().is_some_and(|(_, next)| *next == '\'') {
                characters.next();
            } else {
                quoted = !quoted;
            }
        } else if !quoted
            && character == '}'
            && characters.peek().is_some_and(|(_, next)| *next == '}')
        {
            return Some(index);
        }
    }
    None
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
