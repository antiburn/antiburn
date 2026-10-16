use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use antiburn_checks::diagnostics::{Diagnostic, DiagnosticCode};
use antiburn_checks::domain::ActivationId;
use sha2::{Digest, Sha256};

use super::MAX_DEFINITIONS;
use crate::insights_report::installed_checks::{
    InstalledCheckDiagnostic, InstalledCheckDiagnosticDetail, InstalledCheckDiagnosticSeverity,
    InstalledCheckDiagnosticSource,
};

const MAX_DIAGNOSTICS_PER_FILE: usize = 8;

pub(super) fn source(path: &Path) -> InstalledCheckDiagnosticSource {
    InstalledCheckDiagnosticSource {
        reference: format!(
            "check-source:{}",
            super::hex(&Sha256::digest(path.as_os_str().as_encoded_bytes()))
        ),
        path: path.to_string_lossy().into_owned(),
        line: None,
        column: None,
    }
}

pub(crate) fn detail(label: &str, value: impl Into<String>) -> InstalledCheckDiagnosticDetail {
    InstalledCheckDiagnosticDetail {
        label: label.into(),
        value: value.into(),
    }
}

pub(crate) fn payload(
    code: &str,
    severity: InstalledCheckDiagnosticSeverity,
    title: &str,
    message: &str,
    source: Option<InstalledCheckDiagnosticSource>,
    mut details: Vec<InstalledCheckDiagnosticDetail>,
) -> InstalledCheckDiagnostic {
    details.push(detail("Diagnostic code", code));
    InstalledCheckDiagnostic {
        code: code.into(),
        severity,
        title: title.into(),
        message: message.into(),
        source,
        details,
        affected_session_count: 0,
    }
}

pub(super) fn definition_diagnostic(
    diagnostic: &Diagnostic,
    source: Option<InstalledCheckDiagnosticSource>,
) -> InstalledCheckDiagnostic {
    use DiagnosticCode::*;
    let (title, message) = match diagnostic.code {
        YamlSyntax => (
            "Fix the YAML syntax",
            "The file is not valid YAML. Check indentation, quotes, and brackets at the indicated location.",
        ),
        YamlDuplicateKey => (
            "Remove the duplicate key",
            "An object defines the same key more than once. Keep one value for each key.",
        ),
        YamlDirective => (
            "Remove the YAML directive",
            "Burn Checks accept YAML without directives. Remove the directive and save the file.",
        ),
        YamlDocumentCount => (
            "Use one YAML document",
            "Each check file must contain exactly one YAML document. Remove extra document separators or move each check to its own file.",
        ),
        YamlRootType => (
            "Use a YAML object",
            "Start the check with named fields such as name, on, decisions, and actions.",
        ),
        YamlNestingLimit => (
            "Reduce YAML nesting",
            "The file exceeds the nesting limit. Simplify nested objects and arrays.",
        ),
        YamlAnchor => (
            "Remove the YAML anchor",
            "Burn Checks do not support YAML anchors. Write each value directly without an anchor name.",
        ),
        YamlAlias => (
            "Replace the YAML alias",
            "Burn Checks do not support YAML aliases. Replace the alias with the value it references.",
        ),
        YamlMergeKey => (
            "Replace the YAML merge key",
            "Burn Checks do not support YAML merge keys. Write each merged field directly in the object.",
        ),
        YamlUnsupportedTag => (
            "Remove the YAML tag",
            "Use plain JSON-compatible values instead of custom YAML tags.",
        ),
        YamlNonStringKey => (
            "Use text keys",
            "Object keys must be strings. Quote numeric or boolean keys.",
        ),
        YamlNonFiniteNumber => (
            "Use a finite number",
            "Replace infinity or NaN with a finite JSON number.",
        ),
        YamlInvalidScalar => (
            "Fix the YAML value",
            "The value does not match its declared YAML type. Correct the value or remove its type tag.",
        ),
        ConfigMissingField => (
            "Add the required field",
            "The check is missing a required field. Add the field shown in Details and save the file.",
        ),
        ConfigUnknownField => (
            "Remove the unsupported field",
            "This version does not recognize the field shown in Details. Remove it or use a supported field.",
        ),
        ConfigInvalidType => (
            "Correct the field type",
            "The field shown in Details has the wrong type. Use the object, array, text, number, or boolean required by the check format.",
        ),
        ConfigInvalidValue => (
            "Correct the field value",
            "The field shown in Details has an invalid value. Check its allowed values and relationships.",
        ),
        CompileUnsupportedFeature => (
            "Use a supported check feature",
            "This feature is not supported in v1-prealpha. Update the definition to the supported authoring format.",
        ),
        CompileInvalidExpression => (
            "Fix the expression",
            "The expression does not use supported syntax. Check the expression in the field shown in Details.",
        ),
        CompileUnknownReference => (
            "Correct the expression reference",
            "The expression refers to a field that is unavailable here. Use a field from this expression's input context.",
        ),
        CompileInvalidWindow => (
            "Correct the result window",
            "Set size from 1 to 40 and min from 1 to size.",
        ),
        CompileInvalidCooldown => (
            "Correct the cooldown",
            "Use a supported integer duration ending in s, m, or h.",
        ),
        CompileInvalidDecision => (
            "Correct the decision",
            "Use the supported Choice decision format, including its model, input, question, and criteria.",
        ),
        CompileInvalidAction => (
            "Correct the status action",
            "Use a supported status action with a success or failure result.",
        ),
        DiscoveryInvalidId => (
            "Rename the check file",
            "Start the filename with a lowercase letter. Use lowercase letters, digits, and hyphens, followed by .yml or .yaml.",
        ),
        DiscoveryDuplicateId => (
            "Resolve duplicate check files",
            "Multiple files use the same check ID in this folder. Keep one file or give each check a distinct filename.",
        ),
        DiscoveryReadFailed => (
            "Check file access",
            "antiburn cannot read this check file. Check its permissions, use UTF-8 text, and keep the file within 256 KiB.",
        ),
        DiscoveryPaused => (
            "Check is paused",
            "Enable the check or its repository to use it for new activity.",
        ),
        DiscoveryShadowed => (
            "Project definition takes priority",
            "The project definition replaces the global check with the same filename stem.",
        ),
        RuntimeInvalidDecision => (
            "The model response is invalid",
            "The response does not match the answers defined by this check. Review the decision in the YAML file and test the Jev connection in Settings → Checks.",
        ),
        RuntimeInvalidContext => (
            "Check context is unavailable",
            "The selected session data cannot provide the required context. Check the input references and session source access.",
        ),
        EvaluationLimitExceeded => (
            "Simplify the expression",
            "Expression evaluation exceeded a local resource limit. Reduce the selected input or simplify the expression.",
        ),
    };
    let mut details = Vec::new();
    if let Some(path) = &diagnostic.config_path {
        details.push(detail("Field", path));
    }
    let mut source = source;
    if let Some(source) = &mut source {
        source.line = diagnostic.span.map(|span| span.line);
        source.column = diagnostic.span.map(|span| span.column);
    }
    let fit_field = diagnostic
        .config_path
        .as_deref()
        .is_some_and(|path| path.split('.').any(|segment| segment == "fit"));
    let criteria_field = diagnostic.config_path.as_deref().is_some_and(|path| {
        path.starts_with("decisions.") && path.contains(".output.") && path.ends_with(".criteria")
    });
    let (code, title, message) = if criteria_field && diagnostic.code == ConfigInvalidValue {
        (
            diagnostic.code.as_str(),
            "Correct the Choice criteria",
            "Use 2 to 26 Choice criteria. Add or remove labels to stay within that range.",
        )
    } else if fit_field && matches!(diagnostic.code, ConfigInvalidType | ConfigInvalidValue) {
        (
            "fit-config-invalid",
            "Correct the input fitting rules",
            "Use recent for arrays and truncate for text. Select valid input paths and remove overlapping rules. Check the field shown in Details.",
        )
    } else {
        (diagnostic.code.as_str(), title, message)
    };
    payload(
        code,
        if matches!(diagnostic.code, DiscoveryPaused | DiscoveryShadowed) {
            InstalledCheckDiagnosticSeverity::Info
        } else {
            InstalledCheckDiagnosticSeverity::Error
        },
        title,
        message,
        source,
        details,
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SafeDiagnostic {
    code: &'static str,
    line: Option<usize>,
    column: Option<usize>,
    location: String,
    message: String,
}

impl SafeDiagnostic {
    fn new(diagnostic: &Diagnostic) -> Self {
        use DiagnosticCode::*;
        let message = match diagnostic.code {
            YamlSyntax => "Malformed YAML.",
            YamlDirective => "YAML directives are not allowed.",
            YamlDocumentCount => "Expected one YAML document.",
            YamlRootType => "The YAML root must be an object.",
            YamlNestingLimit => "YAML exceeds the nesting limit.",
            YamlAnchor | YamlAlias => "YAML anchors and aliases are not allowed.",
            YamlUnsupportedTag => "This YAML tag is not supported.",
            YamlNonStringKey => "Object keys must be strings.",
            YamlDuplicateKey => "Object keys must be unique.",
            YamlMergeKey => "YAML merge keys are not allowed.",
            YamlNonFiniteNumber => "Numbers must be finite JSON numbers.",
            YamlInvalidScalar => "The scalar does not match its YAML type.",
            ConfigMissingField => "A required field is missing.",
            ConfigUnknownField => "This field is not supported.",
            ConfigInvalidType => "The field has an incorrect type.",
            ConfigInvalidValue => "The field has an invalid value.",
            CompileUnsupportedFeature => "Burn Checks do not support this feature in pre-alpha.",
            CompileInvalidExpression => "The expression syntax is invalid.",
            CompileUnknownReference => "The expression references an unavailable field.",
            CompileInvalidWindow => "Window size must be 1 through 40, with 1 <= min <= size.",
            CompileInvalidCooldown => "Cooldown must be an integer duration ending in s, m, or h.",
            CompileInvalidDecision => "The decision does not match the Choice contract.",
            CompileInvalidAction => "The action does not match the status-action contract.",
            DiscoveryInvalidId => "The filename must match [a-z][a-z0-9-]*.yml or .yaml.",
            DiscoveryDuplicateId => "Multiple files define the same check ID in this scope.",
            DiscoveryReadFailed => &diagnostic.message,
            _ => "The check file is invalid.",
        };
        Self {
            code: diagnostic.code.as_str(),
            line: diagnostic.span.map(|span| span.line),
            column: diagnostic.span.map(|span| span.column),
            location: safe_location(diagnostic.config_path.as_deref().unwrap_or("")),
            message: message.to_owned(),
        }
    }

    pub(super) fn display(diagnostic: &Diagnostic) -> String {
        let safe = Self::new(diagnostic);
        let position = match (safe.line, safe.column) {
            (Some(line), Some(column)) => format!(" at {line}:{column}"),
            _ => String::new(),
        };
        format!(
            "{}{} [{}]: {}",
            safe.code, position, safe.location, safe.message
        )
    }
}

fn safe_location(path: &str) -> String {
    const FIELDS: &[&str] = &[
        "name",
        "description",
        "on",
        "if",
        "decisions",
        "actions",
        "tool_result",
        "tools",
        "status",
        "models",
        "fast_mode",
        "paths",
        "paths-ignore",
        "window",
        "size",
        "min",
        "cooldown",
        "model",
        "input",
        "output",
        "type",
        "question",
        "criteria",
        "result",
        "message",
    ];
    if path.is_empty() {
        return "root".into();
    }
    path.split('.')
        .take(16)
        .map(|segment| {
            let name = segment.split('[').next().unwrap_or(segment);
            if FIELDS.contains(&name)
                && segment.len() <= 32
                && segment[name.len()..]
                    .chars()
                    .all(|ch| ch.is_ascii_digit() || ch == '[' || ch == ']')
            {
                segment
            } else {
                "<name>"
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

#[derive(Default)]
pub(super) struct DiagnosticLog {
    previous: BTreeMap<PathBuf, Vec<SafeDiagnostic>>,
    conditions: BTreeMap<ActivationId, ConditionState>,
}

#[derive(Default, PartialEq, Eq)]
struct ConditionState {
    condition_false: bool,
    condition_unavailable: bool,
    condition_failed: bool,
}

pub(super) type DiagnosticBatch = BTreeMap<PathBuf, Vec<SafeDiagnostic>>;

pub(super) fn collect(batch: &mut DiagnosticBatch, diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        if matches!(
            diagnostic.code,
            DiagnosticCode::DiscoveryPaused | DiagnosticCode::DiscoveryShadowed
        ) {
            continue;
        }
        let Some(path) = &diagnostic.source_path else {
            continue;
        };
        if batch.len() >= MAX_DEFINITIONS * 2 && !batch.contains_key(path) {
            continue;
        }
        let errors = batch.entry(path.clone()).or_default();
        let safe = SafeDiagnostic::new(diagnostic);
        if errors.len() < MAX_DIAGNOSTICS_PER_FILE && !errors.contains(&safe) {
            errors.push(safe);
        }
    }
}

impl DiagnosticLog {
    pub(super) fn condition_diagnostic(
        &self,
        id: &ActivationId,
        source: Option<InstalledCheckDiagnosticSource>,
    ) -> Option<InstalledCheckDiagnostic> {
        let condition = self.conditions.get(id)?;
        let (code, title, message) = if condition.condition_failed {
            (
                "condition-failed",
                "The check condition could not run",
                "Check the condition expression and its input types in the YAML file. No model request was sent for this result.",
            )
        } else if condition.condition_unavailable {
            (
                "condition-unavailable",
                "The check condition needs unavailable data",
                "The session does not provide a field used by the condition. Check its input references and source coverage. No model request was sent for this result.",
            )
        } else {
            return None;
        };
        Some(payload(
            code,
            InstalledCheckDiagnosticSeverity::Error,
            title,
            message,
            source,
            vec![detail("Field", "if")],
        ))
    }

    pub(super) fn publish_conditions(
        &mut self,
        activation: &antiburn_checks::definition_lifecycle::EffectiveActivation,
        outcome: &antiburn_checks::engine::BatchOutcome,
    ) {
        use antiburn_checks::preparation::ConditionOutcome;
        let mut current = ConditionState::default();
        for (_, condition) in &outcome.condition_outcomes {
            match condition {
                ConditionOutcome::Matched => {}
                ConditionOutcome::False => current.condition_false = true,
                ConditionOutcome::Unavailable => current.condition_unavailable = true,
                ConditionOutcome::Failed(_) => current.condition_failed = true,
            }
        }
        if current == ConditionState::default() {
            if outcome.candidates > 0 {
                self.conditions.remove(&activation.id);
            }
            return;
        }
        if self.conditions.get(&activation.id) == Some(&current)
            || (self.conditions.len() >= MAX_DEFINITIONS
                && !self.conditions.contains_key(&activation.id))
        {
            return;
        }
        ::tracing::debug!(
            event = "installed_check_condition_filtered",
            check_id = %activation.id.definition_id,
            generation = activation.id.generation.0,
            reason = if current.condition_failed { "condition_failed" }
                else if current.condition_unavailable { "condition_unavailable" }
                else { "condition_false" },
            condition_false = current.condition_false,
            condition_unavailable = current.condition_unavailable,
            condition_failed = current.condition_failed,
            provider_attempts = 0,
        );
        self.conditions.insert(activation.id.clone(), current);
    }

    pub(super) fn retain_conditions(&mut self, mut keep: impl FnMut(&ActivationId) -> bool) {
        self.conditions.retain(|id, _| keep(id));
    }

    pub(super) fn publish(&mut self, current: DiagnosticBatch) {
        for (path, diagnostics) in &current {
            if self.previous.get(path) == Some(diagnostics) {
                continue;
            }
            for diagnostic in diagnostics {
                ::tracing::error!(
                    event = "installed_check_invalid",
                    path = %path.display(),
                    code = diagnostic.code,
                    line = diagnostic.line,
                    column = diagnostic.column,
                    config_location = diagnostic.location,
                    message = diagnostic.message,
                );
            }
        }
        self.previous = current;
    }
}

pub(super) fn lifecycle(
    check_id: &str,
    transition: &antiburn_checks::definition_lifecycle::LifecycleTransition,
) {
    use antiburn_checks::definition_lifecycle::LifecycleTransition;
    match transition {
        LifecycleTransition::Activated(active) => ::tracing::debug!(
            event = "installed_check_activated",
            check_id,
            generation = active.id.generation.0,
            bootstrap_publication = active.bootstrap_publication.0,
            repository_root_available = active.project_root.is_some(),
        ),
        LifecycleTransition::Deactivated {
            next_generation,
            inactive_kind,
            ..
        } => ::tracing::debug!(
            event = "installed_check_deactivated",
            check_id,
            generation = next_generation.0,
            reason = match inactive_kind {
                antiburn_checks::definition_lifecycle::InactiveKind::Invalid => "invalid",
                antiburn_checks::definition_lifecycle::InactiveKind::Paused => "paused",
            },
        ),
        LifecycleTransition::Removed { next_generation } => ::tracing::debug!(
            event = "installed_check_removed",
            check_id,
            generation = next_generation.0,
        ),
        LifecycleTransition::Unchanged(_) | LifecycleTransition::UnchangedInactive => {}
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SourceState {
    waiting_for_data: u64,
    ready_sessions: u64,
    waiting_for_window: usize,
    partial_branches: usize,
    queued: u64,
    in_flight: u64,
    last_status_write: Option<(u64, String)>,
}

impl SourceState {
    pub(super) fn read(
        store: &crate::store::Store,
        activation: &antiburn_checks::definition_lifecycle::EffectiveActivation,
        progress: &antiburn_checks::check_store::SourceProgress,
    ) -> anyhow::Result<Self> {
        use antiburn_local::analysis::{
            ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION,
        };
        use rusqlite::{OptionalExtension, params};
        let ready = crate::insights_report::installed_checks::source_ready_sql(3, 4, 5);
        let can_arrive = crate::insights_report::installed_checks::source_can_arrive_sql(3);
        let connection = store.lock();
        let (waiting_for_data, ready_sessions): (u64, u64) = connection.query_row(
            &format!("SELECT COALESCE(SUM(NOT ready AND data_can_arrive), 0), COALESCE(SUM(ready), 0)
              FROM (SELECT ({ready}) AS ready, ({can_arrive}) AS data_can_arrive
                FROM session s LEFT JOIN session_evidence e USING(environment_key, agent, session_id)
               WHERE s.environment_key = ?1 AND s.cwd = ?2)"),
            params![
                activation.id.scope.environment,
                activation.id.scope.session_cwd,
                PARSER_REVISION,
                ANALYZER_REVISION,
                EVIDENCE_SCHEMA_REVISION
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let activation_scope_key =
            crate::check_store::StoreCheckStore::activation_scope_key(&activation.id.scope);
        let (queued, in_flight) = connection.query_row(
            "SELECT COALESCE(SUM(state = 'pending'), 0),
                    COALESCE(SUM(state IN ('claimed', 'dispatching', 'answered')), 0)
               FROM burn_check_candidate WHERE check_id = ?1 AND activation_scope_key = ?2 AND generation = ?3
                 AND state IN ('pending', 'claimed', 'dispatching', 'answered')",
            params![activation.id.definition_id, activation_scope_key, activation.id.generation.0],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let last_status_write = connection
            .query_row(
                "SELECT h.rowid, st.status FROM burn_check_current_status st
               JOIN burn_check_assessment_history h USING(assessment_id)
              WHERE st.check_id = ?1 AND st.activation_scope_key = ?2 AND st.generation = ?3
              ORDER BY h.rowid DESC LIMIT 1",
                params![
                    activation.id.definition_id,
                    activation_scope_key,
                    activation.id.generation.0
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let minimum = activation
            .compiled
            .trigger
            .window
            .map_or(1, |window| usize::from(window.minimum));
        Ok(Self {
            waiting_for_data,
            ready_sessions,
            waiting_for_window: progress
                .selection
                .branches
                .values()
                .filter(|branch| !branch.results.is_empty() && branch.results.len() < minimum)
                .count(),
            partial_branches: progress
                .selection
                .branches
                .values()
                .filter(|branch| !branch.coverage_complete)
                .count(),
            queued,
            in_flight,
            last_status_write,
        })
    }

    pub(super) fn publish(
        &self,
        activation: &antiburn_checks::definition_lifecycle::EffectiveActivation,
    ) {
        ::tracing::debug!(
            event = "installed_check_source_state",
            check_id = %activation.id.definition_id,
            generation = activation.id.generation.0,
            state = if self.queued > 0 || self.in_flight > 0 { "checking" }
                else if self.waiting_for_data > 0 { "waiting_for_data" }
                else if self.ready_sessions == 0 { "source_unavailable" }
                else if self.waiting_for_window > 0 { "waiting_for_window" }
                else { "waiting_for_results" },
            waiting_for_data = self.waiting_for_data,
            ready_sessions = self.ready_sessions,
            waiting_for_window = self.waiting_for_window,
            partial_branches = self.partial_branches,
            queued = self.queued,
            in_flight = self.in_flight,
            status_write_revision = self.last_status_write.as_ref().map(|(revision, _)| *revision),
            last_written_status = self.last_status_write.as_ref().map(|(_, status)| status.as_str()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_many_choice_criteria_has_located_actionable_definition_copy() {
        let criteria = (0..27)
            .map(|index| format!("          label{index}: Criterion {index}.\n"))
            .collect::<String>();
        let yaml = format!(
            "name: Criteria\non:\n  tool_result:\ndecisions:\n  review:\n    model: typesafe/jev-1.13.0\n    input: {{activity: available}}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria:\n{criteria}actions:\n  - type: status\n    input: {{result: failure}}\n"
        );
        let errors = antiburn_checks::compiler::compile(&yaml).unwrap_err();
        let error = errors
            .iter()
            .find(|error| {
                error.config_path.as_deref() == Some("decisions.review.output.verdict.criteria")
            })
            .unwrap();
        let diagnostic = definition_diagnostic(
            error,
            Some(source(Path::new("/workspace/demo/checks/criteria.yml"))),
        );
        assert_eq!(diagnostic.code, "config-invalid-value");
        assert_eq!(diagnostic.title, "Correct the Choice criteria");
        assert!(diagnostic.message.contains("2 to 26"));
        assert!(diagnostic.source.unwrap().line.is_some());
    }
}
