use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::store::burn_checks::trace_failure::{FailureCause, FailureLog};
use crate::store::burn_checks::trace_refs;
use antiburn_checks::diagnostics::{Diagnostic, DiagnosticCode};
use antiburn_checks::domain::ActivationId;
use sha2::{Digest, Sha256};

use super::MAX_DEFINITIONS;
use crate::insights_report::installed_checks::{
    InstalledCheckDiagnostic, InstalledCheckDiagnosticDetail, InstalledCheckDiagnosticSeverity,
    InstalledCheckDiagnosticSource,
};

const MAX_DIAGNOSTICS_PER_FILE: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeStage {
    Worker,
    WorkerStartup,
    IdleDeadline,
    IdleCatchUp,
    DisableSettlement,
    TaskJoin,
    DirectoryWatch,
    Reconcile,
    SourceCatchUp,
    AssessmentSettlement,
    Clock,
    WorkProbe,
    PreparationStart,
    CurrentContext,
    AuthenticationWrite,
    UsageWrite,
}

impl RuntimeStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Worker => "worker",
            Self::WorkerStartup => "worker_startup",
            Self::IdleDeadline => "idle_deadline",
            Self::IdleCatchUp => "idle_catch_up",
            Self::DisableSettlement => "disable_settlement",
            Self::TaskJoin => "task_join",
            Self::DirectoryWatch => "directory_watch",
            Self::Reconcile => "reconcile",
            Self::SourceCatchUp => "source_catch_up",
            Self::AssessmentSettlement => "assessment_settlement",
            Self::Clock => "clock",
            Self::WorkProbe => "work_probe",
            Self::PreparationStart => "preparation_start",
            Self::CurrentContext => "current_context",
            Self::AuthenticationWrite => "authentication_write",
            Self::UsageWrite => "usage_write",
        }
    }
}

pub(super) fn job_result<T>(
    activation: &ActivationId,
    stage: RuntimeStage,
    result: anyhow::Result<T>,
) -> Result<T, antiburn_checks::check_store::StoreError> {
    static PREVIOUS: std::sync::OnceLock<FailureLog> = std::sync::OnceLock::new();
    let cause = result.as_ref().err().map(FailureCause::classify);
    if PREVIOUS.get_or_init(Default::default).changed(
        trace_refs::activation_ref(activation),
        stage.as_str(),
        cause,
    ) {
        let cause = cause.expect("failed result has a category");
        ::tracing::error!(event = "installed_check_settlement_failed", stage = stage.as_str(),
            reason = cause.category, sqlite_code = cause.sqlite_code, io_kind = ?cause.io_kind,
            check_ref = trace_refs::check_ref(&activation.definition_id), activation_ref = trace_refs::activation_ref(activation));
    }
    runtime_result(activation, stage, result)
}

pub(super) fn worker_result<T>(
    stage: RuntimeStage,
    result: anyhow::Result<T>,
) -> anyhow::Result<T> {
    result.map_err(|error| {
        let failure = RuntimeFailure::classify(stage, &error);
        error.context(failure)
    })
}

pub(super) fn trace_worker_result(result: anyhow::Result<()>) {
    if let Err(error) = result {
        error
            .downcast_ref::<RuntimeFailure>()
            .copied()
            .unwrap_or_else(|| RuntimeFailure::classify(RuntimeStage::Worker, &error))
            .trace("installed_checks_stopped");
    }
}

pub(super) fn runtime_result<T>(
    activation: &ActivationId,
    stage: RuntimeStage,
    result: anyhow::Result<T>,
) -> Result<T, antiburn_checks::check_store::StoreError> {
    static PREVIOUS: std::sync::OnceLock<FailureLog> = std::sync::OnceLock::new();
    let cause = result.as_ref().err().map(FailureCause::classify);
    if PREVIOUS.get_or_init(Default::default).changed(
        trace_refs::activation_ref(activation),
        stage.as_str(),
        cause,
    ) {
        let cause = cause.expect("failed result has a category");
        ::tracing::error!(event = "burn_check_runtime_operation_failed", stage = stage.as_str(),
            reason = cause.category, sqlite_code = cause.sqlite_code, io_kind = ?cause.io_kind,
            check_ref = trace_refs::check_ref(&activation.definition_id), activation_ref = trace_refs::activation_ref(activation));
    }
    result.map_err(|error| antiburn_checks::check_store::StoreError {
        message: error.to_string(),
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct RuntimeFailure {
    stage: RuntimeStage,
    cause: FailureCause,
}

impl std::fmt::Display for RuntimeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.stage.as_str())
    }
}

impl std::fmt::Debug for RuntimeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for RuntimeFailure {}

impl RuntimeFailure {
    pub(super) fn classify(stage: RuntimeStage, error: &anyhow::Error) -> Self {
        Self {
            stage,
            cause: FailureCause::classify(error),
        }
    }

    pub(super) fn trace(&self, event: &'static str) {
        ::tracing::error!(event, stage = self.stage.as_str(), reason = self.cause.category,
            sqlite_code = self.cause.sqlite_code, io_kind = ?self.cause.io_kind);
    }

    pub(super) fn trace_shutdown(&self) {
        ::tracing::warn!(event = "installed_check_shutdown_failed", stage = self.stage.as_str(),
            reason = self.cause.category, sqlite_code = self.cause.sqlite_code,
            io_kind = ?self.cause.io_kind);
    }

    pub(super) fn trace_if_changed(&self, previous: &mut Option<Self>, event: &'static str) {
        if previous.as_ref() != Some(self) {
            self.trace(event);
            *previous = Some(*self);
        }
    }

    pub(super) fn diagnostic(&self) -> InstalledCheckDiagnostic {
        let mut details = vec![
            detail("Runtime stage", self.stage.as_str()),
            detail("Failure category", self.cause.category),
        ];
        if let Some(code) = self.cause.sqlite_code {
            details.push(detail("SQLite code", code.to_string()));
        }
        payload(
            "storage-failed",
            InstalledCheckDiagnosticSeverity::Error,
            "The check could not finish updating",
            if matches!(self.cause.category, "sqlite" | "io") {
                "Check available disk space and local data access, then refresh Checks. Earlier saved findings remain available when their evidence is valid."
            } else {
                "Refresh Checks. If the problem continues, collect local diagnostics for support. Earlier saved findings remain available when their evidence is valid."
            },
            None,
            details,
        )
    }
}

pub(super) struct SlowPhase {
    phase: &'static str,
    started: std::time::Instant,
}

impl SlowPhase {
    pub(super) fn start(phase: &'static str) -> Self {
        Self {
            phase,
            started: std::time::Instant::now(),
        }
    }
}

impl Drop for SlowPhase {
    fn drop(&mut self) {
        if self.started.elapsed() >= std::time::Duration::from_secs(1) {
            ::tracing::debug!(
                event = "installed_check_slow_phase",
                phase = self.phase,
                elapsed_ms = self.started.elapsed().as_millis() as u64
            );
        }
    }
}

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
            "The check is missing a required field. Add the field shown in Technical details and save the file.",
        ),
        ConfigUnknownField => (
            "Remove the unsupported field",
            "This version does not recognize the field shown in Technical details. Remove it or use a supported field.",
        ),
        ConfigInvalidType => (
            "Correct the field type",
            "The field shown in Technical details has the wrong type. Use the object, array, text, number, or boolean required by the check format.",
        ),
        ConfigInvalidValue => (
            "Correct the field value",
            "The field shown in Technical details has an invalid value. Check its allowed values and relationships.",
        ),
        CompileUnsupportedFeature => (
            "Use a supported check feature",
            "This feature is not supported in v1-alpha. Update the definition to the supported authoring format.",
        ),
        CompileInvalidExpression => (
            "Fix the expression",
            "The expression does not use supported syntax. Check the expression in the field shown in Technical details.",
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
            "Use a decision with input and typed output questions. Each question must follow its Choice, Score, or Noul format. Select the model in Settings → Checks.",
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
            "Cannot read this check file. Check its permissions, use UTF-8 text, and keep the file within 256 KiB.",
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
            "The response does not match the answers defined by this check. Review the decision in the YAML file and test the selected connection in Settings → Checks.",
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
            "Correct the question criteria",
            "For Choice, use at least two distinct labels with nonblank criteria. For Score, use an ordered array with at least two nonblank descriptions. The selected connection sets the supported upper limit in Settings → Checks.",
        )
    } else if fit_field && matches!(diagnostic.code, ConfigInvalidType | ConfigInvalidValue) {
        (
            "fit-config-invalid",
            "Correct the input fitting rules",
            "Use recent for arrays and truncate for text. Select valid input paths and remove overlapping rules. Check the field shown in Technical details.",
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
            CompileUnsupportedFeature => "Burn Checks do not support this feature in v1-alpha.",
            CompileInvalidExpression => "The expression syntax is invalid.",
            CompileUnknownReference => "The expression references an unavailable field.",
            CompileInvalidWindow => "Window size must be 1 through 40, with 1 <= min <= size.",
            CompileInvalidCooldown => "Cooldown must be an integer duration ending in s, m, or h.",
            CompileInvalidDecision => "The decision does not match its typed question format.",
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
            activation_ref = trace_refs::activation_ref(&activation.id),
            generation = activation.id.generation.0,
            reason = if current.condition_failed {
                "condition_failed"
            } else if current.condition_unavailable {
                "condition_unavailable"
            } else {
                "condition_false"
            },
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
                    code = diagnostic.code,
                    line = diagnostic.line,
                    column = diagnostic.column,
                );
            }
        }
        self.previous = current;
    }
}

pub(super) fn lifecycle(transition: &antiburn_checks::definition_lifecycle::LifecycleTransition) {
    use antiburn_checks::definition_lifecycle::LifecycleTransition;
    match transition {
        LifecycleTransition::Activated(active) => ::tracing::debug!(
            event = "installed_check_activated",
            check_ref = trace_refs::check_ref(&active.id.definition_id),
            activation_ref = trace_refs::activation_ref(&active.id),
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
            generation = next_generation.0,
            reason = match inactive_kind {
                antiburn_checks::definition_lifecycle::InactiveKind::Invalid => "invalid",
                antiburn_checks::definition_lifecycle::InactiveKind::Paused => "paused",
            },
        ),
        LifecycleTransition::Removed { next_generation } => ::tracing::debug!(
            event = "installed_check_removed",
            generation = next_generation.0,
        ),
        LifecycleTransition::Unchanged(_) | LifecycleTransition::UnchangedInactive => {}
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SourceState {
    waiting_for_data: u64,
    ready_sessions: u64,
    inactive_sessions: u64,
    attachments: u64,
    missing_receipts: u64,
    pending_delivery: u64,
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
        let can_arrive = crate::insights_report::installed_checks::source_can_arrive_sql(3, 6);
        let main_session =
            "s.wsl_distro IS NULL AND s.agent IN ('claude-code', 'codex', 'pi', 'opencode')
          AND NOT EXISTS(SELECT 1 FROM session_relation r
            WHERE r.environment_key = s.environment_key AND r.agent = s.agent
              AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
                OR (r.related_id = s.session_id AND r.kind = 'subagent')))";
        let connection = store.lock();
        let (waiting_for_data, ready_sessions, inactive_sessions): (u64, u64, u64) = connection.query_row(
            &format!("SELECT COALESCE(SUM(NOT ready AND data_can_arrive), 0),
                COALESCE(SUM(ready AND eligible), 0), COALESCE(SUM(NOT eligible), 0)
              FROM (SELECT ({ready}) AS ready, ({can_arrive}) AS data_can_arrive,
                  (({can_arrive}) OR s.updated_at_epoch >= ?6) AS eligible
                FROM session s LEFT JOIN session_evidence e USING(environment_key, agent, session_id)
               WHERE s.environment_key = ?1 AND s.cwd = ?2 AND ({main_session}))"),
            params![
                activation.id.scope.environment,
                activation.id.scope.session_cwd,
                PARSER_REVISION,
                ANALYZER_REVISION,
                EVIDENCE_SCHEMA_REVISION,
                super::active_since()
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let activation_scope_key =
            crate::check_store::StoreCheckStore::activation_scope_key(&activation.id.scope);
        let missing_receipts = connection.query_row(
            &format!(
                "SELECT COUNT(*) FROM session s WHERE s.environment_key = ?1 AND s.cwd = ?2
              AND ({main_session})
              AND ((s.agent = 'opencode' AND s.source_kind = 'providerDb'
                AND s.source_label = 'opencode:' || s.session_id
                AND NOT EXISTS(SELECT 1 FROM opencode_journal_delivery d
                  WHERE d.environment_key = s.environment_key AND d.agent = s.agent
                    AND d.session_id = s.session_id))
                OR (s.agent IN ('claude-code', 'codex', 'pi') AND s.source_kind = 'file'
                  AND NOT EXISTS(SELECT 1 FROM native_file_delivery d
                    WHERE d.environment_key = s.environment_key AND d.agent = s.agent
                      AND d.session_id = s.session_id)))"
            ),
            params![
                activation.id.scope.environment,
                activation.id.scope.session_cwd
            ],
            |row| row.get(0),
        )?;
        let attachments = connection.query_row(
            "SELECT COUNT(*) FROM burn_check_session_attachment
              WHERE check_id = ?1 AND activation_scope_key = ?2 AND generation = ?3",
            params![
                activation.id.definition_id,
                activation_scope_key,
                activation.id.generation.0
            ],
            |row| row.get(0),
        )?;
        let pending_delivery = connection.query_row(
            &format!(
                "SELECT COUNT(*) FROM session s
              LEFT JOIN session_evidence e USING(environment_key, agent, session_id)
              WHERE s.environment_key = ?1 AND s.cwd = ?2 AND ({can_arrive})
                AND (EXISTS(SELECT 1 FROM opencode_journal_delivery d
                    WHERE d.environment_key = s.environment_key AND d.agent = s.agent
                      AND d.session_id = s.session_id AND d.parser_revision = ?3
                      AND (d.pending = 1 OR d.observed_generation <> s.source_generation))
                  OR EXISTS(SELECT 1 FROM native_file_delivery d
                    WHERE d.environment_key = s.environment_key AND d.agent = s.agent
                      AND d.session_id = s.session_id AND d.parser_revision = ?3
                      AND (d.pending = 1 OR d.observed_generation <> s.source_generation)))"
            ),
            params![
                activation.id.scope.environment,
                activation.id.scope.session_cwd,
                PARSER_REVISION,
                ANALYZER_REVISION,
                EVIDENCE_SCHEMA_REVISION,
                super::active_since()
            ],
            |row| row.get(0),
        )?;
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
            inactive_sessions,
            attachments,
            missing_receipts,
            pending_delivery,
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
            check_ref = trace_refs::check_ref(&activation.id.definition_id),
            activation_ref = trace_refs::activation_ref(&activation.id),
            generation = activation.id.generation.0,
            state = if self.queued > 0 || self.in_flight > 0 {
                "checking"
            } else if self.waiting_for_data > 0 || self.pending_delivery > 0 {
                "waiting_for_data"
            } else if self.ready_sessions == 0 && self.inactive_sessions > 0 {
                "no_eligible_source"
            } else if self.ready_sessions == 0 {
                "source_unavailable"
            } else if self.waiting_for_window > 0 {
                "waiting_for_window"
            } else {
                "waiting_for_results"
            },
            waiting_for_data = self.waiting_for_data,
            ready_sessions = self.ready_sessions,
            inactive_sessions = self.inactive_sessions,
            attachments = self.attachments,
            missing_receipts = self.missing_receipts,
            pending_delivery = self.pending_delivery,
            reset_activity_window_secs = antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS,
            waiting_for_window = self.waiting_for_window,
            partial_branches = self.partial_branches,
            queued = self.queued,
            in_flight = self.in_flight,
            status_write_revision = self
                .last_status_write
                .as_ref()
                .map(|(revision, _)| *revision),
            last_written_status =
                self.last_status_write
                    .as_ref()
                    .map(|(_, status)| match status.as_str() {
                        "success" => "success",
                        "failure" => "failure",
                        _ => "unknown",
                    }),
        );
    }
}

#[cfg(test)]
#[path = "tracing_tests.rs"]
mod tracing_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_few_choice_criteria_has_located_actionable_definition_copy() {
        let criteria = (0..1)
            .map(|index| format!("          label{index}: Criterion {index}.\n"))
            .collect::<String>();
        let yaml = format!(
            "name: Criteria\non:\n  tool_result:\ndecisions:\n  review:\n    input: {{activity: available}}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria:\n{criteria}actions:\n  - type: status\n    input: {{result: failure}}\n"
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
        assert_eq!(diagnostic.title, "Correct the question criteria");
        assert!(diagnostic.message.contains("at least two"));
        assert!(diagnostic.source.unwrap().line.is_some());
    }
}
