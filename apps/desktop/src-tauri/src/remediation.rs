//! Private orchestration for native remediation previews and history.

use std::collections::VecDeque;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use antiburn_local::insights::{DetectorId, ReportCatalogs, ReportWindow};
use antiburn_local::model::AgentKind;
use antiburn_local::remediation::{
    AutomaticUnavailableReason, ChangeOperation, DETECTOR_REVISION, FINDING_SCHEMA_REVISION,
    FindingCause, FindingDisplay, PROMPT_TEMPLATE_REVISION, REMEDIATION_POLICY_REVISION,
    Recommendation,
};
use anyhow::{Context, Result};
use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Sha256;

use crate::agent_config::{
    ActivationBoundary, AgentConfigEditor, ConfigChange, ConfigContext, ConfigInspection,
    PrepareOutcome, PreparedChange,
};
use crate::insights_report::{self, CurrentFinding, CurrentFindingsCursor, CurrentFindingsRequest};
use crate::store::{Remediation, RemediationCursor, RemediationOrigin, RemediationRecord, Store};

const ID_TTL: Duration = Duration::from_secs(10 * 60);
const FINDING_LIMIT: usize = 512;
const PRIVATE_LIMIT: usize = 128;
const DEFAULT_PAGE_SIZE: usize = 50;
const MAX_PAGE_SIZE: usize = 100;
const TARGET_DOMAIN: &[u8] = b"antiburn/remediation-target/v1\0";

/// Revisions shared by persistence and startup reconciliation.
pub fn revisions_json() -> String {
    json!({
        "version": 1,
        "detector": DETECTOR_REVISION,
        "catalog": ReportCatalogs::default().revision,
        "findingSchema": FINDING_SCHEMA_REVISION,
        "policy": REMEDIATION_POLICY_REVISION,
        "promptTemplate": PROMPT_TEMPLATE_REVISION,
    })
    .to_string()
}

/// One current finding safe for a command response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingResult {
    pub finding_id: String,
    pub finding: FindingDisplay,
    pub expires_at_epoch: i64,
}

/// A bounded page with an opaque controller-owned cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingPageResult {
    pub findings: Vec<FindingResult>,
    pub next_cursor: Option<String>,
}

/// Public facts about an inspected automatic edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectionResult {
    pub summary: String,
    pub current_value: Option<String>,
    pub proposed_value: String,
    pub activation_boundary: &'static str,
}

/// One recommendation after current evidence and local configuration checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecommendationResult {
    pub finding: FindingDisplay,
    pub prompt: Option<String>,
    pub automatic_unavailable: Option<String>,
    pub preview_id: Option<String>,
    pub operation: Option<ChangeOperation>,
    pub inspection: Option<InspectionResult>,
    pub expires_at_epoch: Option<i64>,
    pub already_configured: bool,
}

/// The result of consuming one explicitly approved preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyResult {
    Applied { remediation_id: String },
    AppliedTrackingFailed,
}

/// One external change claim accepted for later verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalClaimResult {
    pub remediation_id: String,
}

/// One reviewed remediation history item without storage-only identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationHistoryItem {
    pub remediation_id: String,
    pub state: String,
    pub origin: String,
    pub agent: String,
    pub source_format: String,
    pub finding: HistoryFinding,
    pub change: HistoryChange,
    pub boundary: String,
    pub verification_status: String,
    pub savings_status: String,
    pub created_at_epoch: i64,
    pub applied_at_epoch: Option<i64>,
    pub verified_at_epoch: Option<i64>,
    pub recurred_at_epoch: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryFinding {
    version: u32,
    pub detector: String,
    pub observation: String,
    pub labels: Vec<String>,
    pub omitted: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryChange {
    version: u32,
    pub operation: String,
    pub summary: String,
    pub current_value: Option<String>,
    pub proposed_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemediationHistoryPage {
    pub remediations: Vec<RemediationHistoryItem>,
    pub next_cursor: Option<String>,
}

/// A closed error vocabulary suitable for the IPC boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerError {
    FindingNotFound,
    FindingExpired,
    FindingChanged,
    CursorInvalid,
    CursorExpired,
    PreviewNotFound,
    PreviewExpired,
    PreviewChanged,
    HomeUnavailable,
    RecommendationUnavailable(String),
    ConfigUnavailable(String),
    ApplyFailed(String),
    PersistenceFailed,
    InvalidHistory,
    Internal,
}

impl fmt::Display for ControllerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FindingNotFound => formatter.write_str("finding_not_found"),
            Self::FindingExpired => formatter.write_str("finding_expired"),
            Self::FindingChanged => formatter.write_str("finding_changed"),
            Self::CursorInvalid => formatter.write_str("cursor_invalid"),
            Self::CursorExpired => formatter.write_str("cursor_expired"),
            Self::PreviewNotFound => formatter.write_str("preview_not_found"),
            Self::PreviewExpired => formatter.write_str("preview_expired"),
            Self::PreviewChanged => formatter.write_str("preview_changed"),
            Self::HomeUnavailable => formatter.write_str("home_unavailable"),
            Self::RecommendationUnavailable(reason) => {
                write!(formatter, "recommendation_unavailable:{reason}")
            }
            Self::ConfigUnavailable(reason) => write!(formatter, "config_unavailable:{reason}"),
            Self::ApplyFailed(reason) => write!(formatter, "apply_failed:{reason}"),
            Self::PersistenceFailed => formatter.write_str("persistence_failed"),
            Self::InvalidHistory => formatter.write_str("invalid_history"),
            Self::Internal => formatter.write_str("internal_error"),
        }
    }
}

impl std::error::Error for ControllerError {}

struct Timed<T> {
    id: String,
    value: T,
    created_at_epoch: i64,
}

struct FindingCursorEntry {
    detector: DetectorId,
    cursor: CurrentFindingsCursor,
}

enum CursorEntry {
    Findings(FindingCursorEntry),
    History(RemediationCursor),
}

struct PreviewEntry {
    finding: CurrentFinding,
    operation: ChangeOperation,
    change: ConfigChange,
    prepared: Box<PreparedChange>,
    inspection: InspectionResult,
}

#[derive(Default)]
struct ControllerState {
    findings: VecDeque<Timed<CurrentFinding>>,
    cursors: VecDeque<Timed<CursorEntry>>,
    previews: VecDeque<Timed<PreviewEntry>>,
}

/// Owns private finding selectors, cursors, and prepared configuration bytes.
pub struct RemediationController {
    data_dir: PathBuf,
    editor: AgentConfigEditor,
    state: Mutex<ControllerState>,
}

impl RemediationController {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            editor: AgentConfigEditor::new(),
            state: Mutex::new(ControllerState::default()),
        }
    }

    /// Lists current native findings from the last 30 days.
    pub fn list_current_findings(
        &self,
        detector: DetectorId,
        cursor_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<FindingPageResult, ControllerError> {
        self.list_current_findings_at(detector, cursor_id, limit, now_epoch())
    }

    fn list_current_findings_at(
        &self,
        detector: DetectorId,
        cursor_id: Option<&str>,
        limit: Option<usize>,
        now: i64,
    ) -> Result<FindingPageResult, ControllerError> {
        let cursor = cursor_id
            .map(|id| self.finding_cursor(id, detector, now))
            .transpose()?;
        let page = insights_report::list_current_findings(
            &self.data_dir,
            CurrentFindingsRequest {
                environment_key: "native".to_owned(),
                window: ReportWindow {
                    start_epoch: now.saturating_sub(30 * 24 * 60 * 60),
                    end_epoch: now,
                },
                detector,
                cursor,
                limit: limit.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE),
            },
        )
        .map_err(|_| ControllerError::Internal)?;
        let expires = now + ID_TTL.as_secs() as i64;
        let mut results = Vec::with_capacity(page.findings.len());
        for finding in page.findings {
            let display = finding.finding.display().map_err(|reason| {
                ControllerError::RecommendationUnavailable(format_reason(reason))
            })?;
            let id = self.insert_finding(finding, now)?;
            results.push(FindingResult {
                finding_id: id,
                finding: display,
                expires_at_epoch: expires,
            });
        }
        let next_cursor = page
            .next_cursor
            .map(|cursor| {
                self.insert_cursor(
                    CursorEntry::Findings(FindingCursorEntry { detector, cursor }),
                    now,
                )
            })
            .transpose()?;
        Ok(FindingPageResult {
            findings: results,
            next_cursor,
        })
    }

    /// Returns a prompt and, when safe, an exact automatic preview.
    pub fn recommendation(
        &self,
        store: &Store,
        finding_id: &str,
    ) -> Result<RecommendationResult, ControllerError> {
        let now = now_epoch();
        let finding = self.cached_finding(finding_id, now)?;
        self.require_current(&finding)?;
        let display = finding
            .finding
            .display()
            .map_err(|reason| ControllerError::RecommendationUnavailable(format_reason(reason)))?;
        let (prompt, _engine_unavailable) =
            recommendation_prompt(finding.finding.recommendation())?;
        let Some(change) = automatic_change(&display.agent, finding.finding.cause()) else {
            let automatic_unavailable =
                automatic_unavailable_reason(display.agent, finding.finding.cause());
            return Ok(RecommendationResult {
                finding: display,
                prompt,
                automatic_unavailable: Some(automatic_unavailable),
                preview_id: None,
                operation: None,
                inspection: None,
                expires_at_epoch: None,
                already_configured: false,
            });
        };
        let context = config_context(store, &finding, display.agent)?;
        match self.editor.prepare(&context, &change) {
            Ok(PrepareOutcome::NoOp(inspection)) => Ok(RecommendationResult {
                finding: display,
                prompt,
                automatic_unavailable: Some("already_configured".to_owned()),
                preview_id: None,
                operation: Some(change.operation()),
                inspection: Some(inspection_result(&inspection, proposed_value(&change))),
                expires_at_epoch: None,
                already_configured: true,
            }),
            Ok(PrepareOutcome::Ready(prepared)) => {
                let inspection =
                    inspection_result(&prepared.inspection(), prepared.proposed_value());
                let operation = change.operation();
                let preview_id = self.insert_preview(
                    PreviewEntry {
                        finding,
                        operation,
                        change,
                        prepared,
                        inspection: inspection.clone(),
                    },
                    now,
                )?;
                Ok(RecommendationResult {
                    finding: display,
                    prompt,
                    automatic_unavailable: None,
                    preview_id: Some(preview_id),
                    operation: Some(operation),
                    inspection: Some(inspection),
                    expires_at_epoch: Some(now + ID_TTL.as_secs() as i64),
                    already_configured: false,
                })
            }
            Err(reason) => Ok(RecommendationResult {
                finding: display,
                prompt,
                automatic_unavailable: Some(format!("config:{reason}")),
                preview_id: None,
                operation: Some(change.operation()),
                inspection: None,
                expires_at_epoch: None,
                already_configured: false,
            }),
        }
    }

    /// Consumes and applies one preview. A conflict always requires a new preview.
    pub fn apply_approved_preview(
        &self,
        store: &Store,
        preview_id: &str,
    ) -> Result<ApplyResult, ControllerError> {
        let now = now_epoch();
        let preview = self.take_preview(preview_id, now)?;
        self.require_current(&preview.finding)?;
        let agent = preview
            .finding
            .finding
            .display()
            .map_err(|_| ControllerError::PreviewChanged)?
            .agent;
        let context = config_context(store, &preview.finding, agent)?;
        let fresh = self
            .editor
            .prepare(&context, &preview.change)
            .map_err(|_| ControllerError::PreviewChanged)?;
        let PrepareOutcome::Ready(fresh) = fresh else {
            return Err(ControllerError::PreviewChanged);
        };
        if preview.operation != preview.change.operation()
            || preview.inspection != inspection_result(&fresh.inspection(), fresh.proposed_value())
        {
            return Err(ControllerError::PreviewChanged);
        }
        self.editor
            .apply(&preview.prepared)
            .map_err(|error| ControllerError::ApplyFailed(error.to_string()))?;
        if self.require_current(&preview.finding).is_err() {
            return Ok(ApplyResult::AppliedTrackingFailed);
        }
        let remediation = build_remediation(
            store,
            &preview.finding,
            RemediationOrigin::Antiburn,
            preview.change.operation(),
            &preview.inspection,
            Some(now),
            now,
        )
        .map_err(|_| ControllerError::PersistenceFailed)?;
        match store.insert_awaiting_remediation(&remediation) {
            Ok(true) => Ok(ApplyResult::Applied {
                remediation_id: remediation.remediation_id,
            }),
            Ok(false) | Err(_) => Ok(ApplyResult::AppliedTrackingFailed),
        }
    }

    /// Records a claimed external change without claiming an application time.
    pub fn record_external_claim(
        &self,
        store: &Store,
        finding_id: &str,
        operation: ChangeOperation,
        activation_boundary: ActivationBoundary,
    ) -> Result<ExternalClaimResult, ControllerError> {
        let now = now_epoch();
        let finding = self.cached_finding(finding_id, now)?;
        self.require_current(&finding)?;
        let inspection = InspectionResult {
            summary: "External change claimed".to_owned(),
            current_value: None,
            proposed_value: String::new(),
            activation_boundary: boundary_name(activation_boundary),
        };
        self.require_current(&finding)?;
        let remediation = build_remediation(
            store,
            &finding,
            RemediationOrigin::External,
            operation,
            &inspection,
            None,
            now,
        )
        .map_err(|_| ControllerError::PersistenceFailed)?;
        if !store
            .insert_awaiting_remediation(&remediation)
            .map_err(|_| ControllerError::PersistenceFailed)?
        {
            return Err(ControllerError::FindingChanged);
        }
        Ok(ExternalClaimResult {
            remediation_id: remediation.remediation_id,
        })
    }

    /// Lists reviewed native history documents without returning stored JSON.
    pub fn list_native_remediations(
        &self,
        store: &Store,
        cursor_id: Option<&str>,
        limit: Option<u32>,
    ) -> Result<RemediationHistoryPage, ControllerError> {
        let now = now_epoch();
        let cursor = cursor_id
            .map(|id| self.history_cursor(id, now))
            .transpose()?;
        let page = store
            .remediations("native", cursor.as_ref(), limit)
            .map_err(|_| ControllerError::Internal)?;
        let remediations = page
            .remediations
            .iter()
            .map(parse_history)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = page
            .next_cursor
            .map(|cursor| self.insert_cursor(CursorEntry::History(cursor), now))
            .transpose()?;
        Ok(RemediationHistoryPage {
            remediations,
            next_cursor,
        })
    }

    fn require_current(&self, finding: &CurrentFinding) -> Result<(), ControllerError> {
        match insights_report::revalidate_current_finding(&self.data_dir, finding) {
            Ok(true) => Ok(()),
            Ok(false) => Err(ControllerError::FindingChanged),
            Err(_) => Err(ControllerError::Internal),
        }
    }

    fn insert_finding(&self, finding: CurrentFinding, now: i64) -> Result<String, ControllerError> {
        let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
        prune(&mut state.findings, now);
        insert_timed(&mut state.findings, finding, now, FINDING_LIMIT)
    }

    fn insert_cursor(&self, cursor: CursorEntry, now: i64) -> Result<String, ControllerError> {
        let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
        prune(&mut state.cursors, now);
        insert_timed(&mut state.cursors, cursor, now, PRIVATE_LIMIT)
    }

    fn insert_preview(&self, preview: PreviewEntry, now: i64) -> Result<String, ControllerError> {
        let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
        prune(&mut state.previews, now);
        insert_timed(&mut state.previews, preview, now, PRIVATE_LIMIT)
    }

    fn cached_finding(&self, id: &str, now: i64) -> Result<CurrentFinding, ControllerError> {
        let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
        take_or_clone(&mut state.findings, id, now, |value| value.clone()).map_err(|expired| {
            if expired {
                ControllerError::FindingExpired
            } else {
                ControllerError::FindingNotFound
            }
        })
    }

    fn finding_cursor(
        &self,
        id: &str,
        detector: DetectorId,
        now: i64,
    ) -> Result<CurrentFindingsCursor, ControllerError> {
        let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
        let cursor = take_or_clone(&mut state.cursors, id, now, |value| match value {
            CursorEntry::Findings(entry) if cursor_detector_matches(entry.detector, detector) => {
                Some(entry.cursor.clone())
            }
            _ => None,
        })
        .map_err(|expired| {
            if expired {
                ControllerError::CursorExpired
            } else {
                ControllerError::CursorInvalid
            }
        })?;
        cursor.ok_or(ControllerError::CursorInvalid)
    }

    fn history_cursor(&self, id: &str, now: i64) -> Result<RemediationCursor, ControllerError> {
        let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
        let cursor = take_or_clone(&mut state.cursors, id, now, |value| match value {
            CursorEntry::History(cursor) => Some(cursor.clone()),
            _ => None,
        })
        .map_err(|expired| {
            if expired {
                ControllerError::CursorExpired
            } else {
                ControllerError::CursorInvalid
            }
        })?;
        cursor.ok_or(ControllerError::CursorInvalid)
    }

    fn take_preview(&self, id: &str, now: i64) -> Result<PreviewEntry, ControllerError> {
        let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
        take_timed(&mut state.previews, id, now).map_err(|expired| {
            if expired {
                ControllerError::PreviewExpired
            } else {
                ControllerError::PreviewNotFound
            }
        })
    }
}

fn automatic_change(agent: &AgentKind, cause: &FindingCause) -> Option<ConfigChange> {
    match (agent, cause) {
        (
            AgentKind::Claude | AgentKind::Codex | AgentKind::OpenCode | AgentKind::Pi,
            FindingCause::OldModelUsage { replacement, .. },
        ) => Some(ConfigChange::SetModel {
            proposed_value: replacement.clone(),
        }),
        (
            AgentKind::Claude | AgentKind::Codex | AgentKind::Pi,
            FindingCause::ModelOverthinking { model, .. },
        ) => Some(ConfigChange::SetReasoning {
            model: Some(model.clone()),
            proposed_value: "high".to_owned(),
        }),
        (AgentKind::Claude | AgentKind::Codex, FindingCause::UnusedMcpServer { server }) => {
            Some(ConfigChange::DisableMcpServer {
                server: server.clone(),
            })
        }
        _ => None,
    }
}

fn cursor_detector_matches(cached: DetectorId, requested: DetectorId) -> bool {
    cached == requested
}

fn automatic_unavailable_reason(agent: AgentKind, cause: &FindingCause) -> String {
    match cause {
        FindingCause::OverpoweredSubagents { .. } | FindingCause::OveruseOfFastMode { .. } => {
            "persistent_worker_identity_unavailable".to_owned()
        }
        FindingCause::CacheChurn { .. } => "causal_setting_unknown".to_owned(),
        FindingCause::SessionsOverDepth { .. }
        | FindingCause::UnusedBuiltInTool { .. }
        | FindingCause::UnusedSkill { .. } => "review_required".to_owned(),
        FindingCause::UnusedMcpServer { .. } => {
            format!("native_editor_unavailable_for_{}", agent.slug())
        }
        FindingCause::OldModelUsage { .. } | FindingCause::ModelOverthinking { .. } => {
            format!("safe_value_or_editor_unavailable_for_{}", agent.slug())
        }
    }
}

fn config_context(
    store: &Store,
    finding: &CurrentFinding,
    agent: AgentKind,
) -> Result<ConfigContext, ControllerError> {
    let home = antiburn_local::paths::home_dir().ok_or(ControllerError::HomeUnavailable)?;
    let workspace = trusted_workspace(store, finding.workspace_candidate());
    let mut context = ConfigContext::native(agent, &home, workspace);
    context.runtime_override_present = runtime_override_present(agent);
    context.managed_configuration_present = managed_configuration_present(agent, &home);
    Ok(context)
}

fn trusted_workspace(store: &Store, candidate: Option<&Path>) -> Option<PathBuf> {
    let candidate = candidate?.canonicalize().ok()?;
    store
        .repositories()
        .ok()?
        .into_iter()
        .find_map(|repository| {
            if repository.status != "accessible" || !repository.enabled {
                return None;
            }
            let root = PathBuf::from(repository.repo_root?).canonicalize().ok()?;
            candidate.starts_with(&root).then_some(root)
        })
}

fn runtime_override_present(agent: AgentKind) -> bool {
    let names: &[&str] = match agent {
        AgentKind::Claude => &[
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "CLAUDE_CONFIG_DIR",
        ],
        AgentKind::Codex => &["CODEX_HOME", "CODEX_MODEL", "OPENAI_MODEL"],
        AgentKind::OpenCode => &["OPENCODE_CONFIG", "OPENCODE_CONFIG_DIR"],
        AgentKind::Pi => &["PI_AGENT_DIR", "PI_CODING_AGENT_DIR", "PI_MODEL"],
        _ => &[],
    };
    names.iter().any(|name| std::env::var_os(name).is_some())
}

fn managed_configuration_present(agent: AgentKind, home: &Path) -> bool {
    let mut paths = Vec::new();
    match agent {
        AgentKind::Claude => {
            paths.push(home.join(".claude/managed-settings.json"));
            paths.push(PathBuf::from("/etc/claude-code/managed-settings.json"));
            paths.push(PathBuf::from(
                "/Library/Application Support/ClaudeCode/managed-settings.json",
            ));
        }
        AgentKind::Codex => {
            paths.push(home.join(".codex/managed_config.toml"));
            paths.push(home.join(".codex/requirements.toml"));
            paths.push(PathBuf::from("/etc/codex/managed_config.toml"));
        }
        _ => {}
    }
    paths.iter().any(|path| path.try_exists().unwrap_or(true))
}

fn recommendation_prompt(
    recommendation: &Recommendation,
) -> Result<(Option<String>, Option<String>), ControllerError> {
    match recommendation {
        Recommendation::Automatic {
            fallback_prompt, ..
        } => Ok((Some(fallback_prompt.as_str().to_owned()), None)),
        Recommendation::Prompt {
            prompt,
            automatic_unavailable,
        } => Ok((
            Some(prompt.as_str().to_owned()),
            Some(automatic_reason(*automatic_unavailable).to_owned()),
        )),
        Recommendation::Unavailable { reason } => Err(ControllerError::RecommendationUnavailable(
            format_reason(*reason),
        )),
    }
}

fn automatic_reason(reason: AutomaticUnavailableReason) -> &'static str {
    match reason {
        AutomaticUnavailableReason::ReviewRequired => "review_required",
        AutomaticUnavailableReason::ExactTargetUnavailable => "exact_target_unavailable",
        AutomaticUnavailableReason::NativeEditorUnavailable => "native_editor_unavailable",
        AutomaticUnavailableReason::CausalSettingUnknown => "causal_setting_unknown",
    }
}

fn format_reason(reason: impl fmt::Debug) -> String {
    format!("{reason:?}").to_ascii_lowercase()
}

fn inspection_result(inspection: &ConfigInspection, proposed_value: &str) -> InspectionResult {
    InspectionResult {
        summary: inspection.summary().to_owned(),
        current_value: inspection.current_value().map(ToOwned::to_owned),
        proposed_value: proposed_value.to_owned(),
        activation_boundary: boundary_name(inspection.activation_boundary()),
    }
}

fn boundary_name(boundary: ActivationBoundary) -> &'static str {
    match boundary {
        ActivationBoundary::NextRequest => "nextRequest",
        ActivationBoundary::NextSession => "nextSession",
        ActivationBoundary::AgentRestart => "agentRestart",
    }
}

fn proposed_value(change: &ConfigChange) -> &str {
    match change {
        ConfigChange::SetModel { proposed_value }
        | ConfigChange::SetReasoning { proposed_value, .. }
        | ConfigChange::SetWorkerModel { proposed_value, .. }
        | ConfigChange::SetWorkerServiceTier { proposed_value, .. } => proposed_value,
        ConfigChange::DisableMcpServer { .. } => "disabled",
    }
}

fn build_remediation(
    store: &Store,
    finding: &CurrentFinding,
    origin: RemediationOrigin,
    operation: ChangeOperation,
    inspection: &InspectionResult,
    applied_at_epoch: Option<i64>,
    now: i64,
) -> Result<Remediation> {
    let display = finding
        .finding
        .display()
        .map_err(|_| anyhow::anyhow!("display"))?;
    let source_format = serde_json::to_value(display.source_format)?
        .as_str()
        .context("source format is not a string")?
        .to_owned();
    Ok(Remediation {
        remediation_id: random_id()?,
        target_key: target_key(store, finding)?,
        origin,
        environment_key: "native".to_owned(),
        agent: display.agent.slug().to_owned(),
        source_format,
        workspace_key: workspace_key(store, finding)?,
        baseline_session_id: finding.session_id.clone(),
        baseline_source_generation: finding.source_generation,
        baseline_published_fence: finding.published_fence,
        baseline_source_fingerprint: finding.source_fingerprint.clone(),
        baseline_processed_fingerprint: finding.processed_fingerprint.clone(),
        baseline_parser_revision: finding.parser_revision,
        baseline_analyzer_revision: finding.analyzer_revision,
        baseline_evidence_schema_revision: finding.evidence_schema_revision,
        finding_json: json!({
            "version": 1,
            "detector": display.detector_key,
            "observation": display.observation,
            "labels": display.facts.labels,
            "omitted": display.facts.omitted,
        }).to_string(),
        change_json: json!({
            "version": 1,
            "operation": operation_name(operation),
            "summary": inspection.summary,
            "currentValue": inspection.current_value,
            "proposedValue": (!inspection.proposed_value.is_empty()).then_some(&inspection.proposed_value),
        }).to_string(),
        boundary_json: json!({
            "version": 1,
            "activation": inspection.activation_boundary,
        }).to_string(),
        verification_json: json!({"version": 1, "status": "pending"}).to_string(),
        savings_json: json!({"version": 1, "status": "pending"}).to_string(),
        revisions_json: revisions_json(),
        created_at_epoch: now,
        applied_at_epoch,
    })
}

fn target_key(store: &Store, finding: &CurrentFinding) -> Result<String> {
    let secret = store.provider_account_secret()?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&secret).context("invalid target secret")?;
    mac.update(TARGET_DOMAIN);
    update_target(&mut mac, finding.finding.agent());
    let source_format = serde_json::to_string(&finding.finding.source_format)?;
    update_target(&mut mac, &source_format);
    let cause = finding.finding.cause();
    mac.update(cause.detector().key().as_bytes());
    match cause {
        FindingCause::ModelOverthinking { model, .. } => update_target(&mut mac, model),
        FindingCause::OldModelUsage { model, .. } => update_target(&mut mac, model),
        FindingCause::UnusedMcpServer { server } => update_target(&mut mac, server),
        FindingCause::OverpoweredSubagents {
            worker_model,
            worker_ordinal,
            parent_call_id,
            ..
        } => {
            update_target(&mut mac, worker_model);
            mac.update(&worker_ordinal.to_be_bytes());
            if let Some(call_id) = parent_call_id {
                update_target(&mut mac, call_id);
            }
        }
        FindingCause::UnusedBuiltInTool { tool, .. } => update_target(&mut mac, tool),
        FindingCause::UnusedSkill { skill } => update_target(&mut mac, skill),
        FindingCause::OveruseOfFastMode { model, .. } | FindingCause::CacheChurn { model, .. } => {
            update_target(&mut mac, model)
        }
        FindingCause::SessionsOverDepth { .. } => mac.update(b"session-depth"),
    }
    Ok(hex(&mac.finalize().into_bytes()))
}

fn workspace_key(store: &Store, finding: &CurrentFinding) -> Result<String> {
    let Some(workspace) = trusted_workspace(store, finding.workspace_candidate()) else {
        return Ok("global".to_owned());
    };
    let secret = store.provider_account_secret()?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&secret).context("invalid workspace secret")?;
    mac.update(b"antiburn/remediation-workspace/v1\0");
    update_target(&mut mac, &workspace.to_string_lossy());
    Ok(hex(&mac.finalize().into_bytes()))
}

fn update_target(mac: &mut Hmac<Sha256>, value: &str) {
    mac.update(&(value.len() as u32).to_be_bytes());
    mac.update(value.as_bytes());
}

fn parse_history(record: &RemediationRecord) -> Result<RemediationHistoryItem, ControllerError> {
    let finding: HistoryFinding = parse_versioned(&record.finding_json)?;
    let change: HistoryChange = parse_versioned(&record.change_json)?;
    let boundary: BoundaryDocument = parse_versioned(&record.boundary_json)?;
    let verification: StatusDocument = parse_versioned(&record.verification_json)?;
    let savings: StatusDocument = parse_versioned(&record.savings_json)?;
    let _: RevisionsDocument = parse_versioned(&record.revisions_json)?;
    Ok(RemediationHistoryItem {
        remediation_id: record.remediation_id.clone(),
        state: record.state.as_str().to_owned(),
        origin: record.origin.as_str().to_owned(),
        agent: record.agent.clone(),
        source_format: record.source_format.clone(),
        finding,
        change,
        boundary: boundary.activation,
        verification_status: verification.status,
        savings_status: savings.status,
        created_at_epoch: record.created_at_epoch,
        applied_at_epoch: record.applied_at_epoch,
        verified_at_epoch: record.verified_at_epoch,
        recurred_at_epoch: record.recurred_at_epoch,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundaryDocument {
    #[serde(rename = "version")]
    _version: u32,
    activation: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusDocument {
    #[serde(rename = "version")]
    _version: u32,
    status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RevisionsDocument {
    #[serde(rename = "version")]
    _version: u32,
    #[serde(rename = "detector")]
    _detector: u32,
    #[serde(rename = "catalog")]
    _catalog: i64,
    #[serde(rename = "findingSchema")]
    _finding_schema: u32,
    #[serde(rename = "policy")]
    _policy: u32,
    #[serde(rename = "promptTemplate")]
    _prompt_template: u32,
}

fn parse_versioned<T: for<'de> Deserialize<'de>>(value: &str) -> Result<T, ControllerError> {
    let parsed: Value = serde_json::from_str(value).map_err(|_| ControllerError::InvalidHistory)?;
    if parsed.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(ControllerError::InvalidHistory);
    }
    serde_json::from_value(parsed).map_err(|_| ControllerError::InvalidHistory)
}

fn operation_name(operation: ChangeOperation) -> &'static str {
    match operation {
        ChangeOperation::SetModel => "setModel",
        ChangeOperation::SetReasoning => "setReasoning",
        ChangeOperation::SetWorkerModel => "setWorkerModel",
        ChangeOperation::SetWorkerServiceTier => "setWorkerServiceTier",
        ChangeOperation::DisableMcpServer => "disableMcpServer",
    }
}

fn insert_timed<T>(
    entries: &mut VecDeque<Timed<T>>,
    value: T,
    now: i64,
    limit: usize,
) -> Result<String, ControllerError> {
    while entries.len() >= limit {
        entries.pop_front();
    }
    let id = loop {
        let id = random_id().map_err(|_| ControllerError::Internal)?;
        if entries.iter().all(|entry| entry.id != id) {
            break id;
        }
    };
    entries.push_back(Timed {
        id: id.clone(),
        value,
        created_at_epoch: now,
    });
    Ok(id)
}

fn prune<T>(entries: &mut VecDeque<Timed<T>>, now: i64) {
    while entries
        .front()
        .is_some_and(|entry| expired(entry.created_at_epoch, now))
    {
        entries.pop_front();
    }
}

fn take_timed<T>(entries: &mut VecDeque<Timed<T>>, id: &str, now: i64) -> Result<T, bool> {
    let Some(index) = entries.iter().position(|entry| entry.id == id) else {
        return Err(false);
    };
    let entry = entries.remove(index).expect("the located entry exists");
    if expired(entry.created_at_epoch, now) {
        Err(true)
    } else {
        Ok(entry.value)
    }
}

fn take_or_clone<T, U>(
    entries: &mut VecDeque<Timed<T>>,
    id: &str,
    now: i64,
    map: impl FnOnce(&T) -> U,
) -> Result<U, bool> {
    let Some(entry) = entries.iter().find(|entry| entry.id == id) else {
        return Err(false);
    };
    if expired(entry.created_at_epoch, now) {
        return Err(true);
    }
    Ok(map(&entry.value))
}

fn expired(created_at_epoch: i64, now: i64) -> bool {
    now.saturating_sub(created_at_epoch) >= ID_TTL.as_secs() as i64
}

fn random_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).context("random id generation failed")?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[usize::from(byte >> 4)] as char);
        output.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    output
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{
        AnalysisRecord, EvidenceCompletion, PublishedEvidence, SessionKey, SessionRecord,
    };
    use antiburn_local::analysis::{
        ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, EvidenceSource, EvidenceValue, LoadedSource,
        METRICS_SCHEMA_REVISION, PARSER_REVISION, SessionEvidenceAccumulator, SourceCapabilities,
        SourceFormat, SourceKind, TurnFacts,
    };

    fn publish_mcp_finding(store: &Store, session_id: &str, started_at_epoch: i64) {
        let fingerprint = format!("sv1:{session_id}");
        let session = SessionRecord {
            key: SessionKey::new("native", "claude-code", session_id),
            source_kind: "file".to_owned(),
            source_label: format!("/private/{session_id}.jsonl"),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: None,
            surface: "cli".to_owned(),
            updated_at_epoch: Some(started_at_epoch),
            activity_cursor: String::new(),
            activity_source: "event".to_owned(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: Some(fingerprint.clone()),
        };
        store
            .upsert_sessions(std::slice::from_ref(&session), &["claude-code"])
            .unwrap();
        let claim = store
            .claim_next_evidence(&["claude-code"], started_at_epoch, 60)
            .unwrap()
            .unwrap();
        let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude-code".to_owned(),
            session_id: session_id.to_owned(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence(&TurnFacts::default());
        let EvidenceValue::Complete(eligibility) = &mut evidence.eligibility else {
            panic!("the fixture has complete eligibility");
        };
        eligibility.assistant_turns = 1;
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            panic!("the fixture has complete context sources");
        };
        sources.mcp_coverage = EvidenceValue::Complete(());
        sources.mcp_servers.insert(
            "private-server".to_owned(),
            LoadedSource {
                description: None,
                configured: true,
                available: true,
                injected: true,
                invoked: false,
                token_count: None,
                origin: EvidenceValue::Unsupported,
            },
        );
        let analysis = AnalysisRecord {
            key: session.key,
            model_breakdown_json: "{}".to_owned(),
            pricing_breakdown_json: "{}".to_owned(),
            inclusive_models_json: "[]".to_owned(),
            initial_context_json: None,
            source_summaries_json: None,
            provider_hints_json: None,
            source_fingerprint: fingerprint,
            pricing_generation: 1,
            analyzed_generation: claim.source_generation,
            parser_revision: PARSER_REVISION,
            analyzer_revision: ANALYZER_REVISION,
            metrics_schema_revision: METRICS_SCHEMA_REVISION,
        };
        let completion = EvidenceCompletion {
            claim_fence: claim.claim_fence,
            status: PublishedEvidence::Ready,
            evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
            evidence_json: serde_json::to_string(&evidence).unwrap(),
        };
        assert!(
            store
                .publish_projections(&analysis, Some(started_at_epoch), &completion, &[], &[])
                .unwrap()
        );
    }

    #[test]
    fn ids_are_unpredictable_hex_and_expire_at_ten_minutes() {
        let first = random_id().unwrap();
        let second = random_id().unwrap();
        assert_eq!(first.len(), 32);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
        assert!(!expired(100, 699));
        assert!(expired(100, 700));
    }

    #[test]
    fn bounded_cache_evicts_the_oldest_entry_deterministically() {
        let mut entries = VecDeque::new();
        let first = insert_timed(&mut entries, 1, 100, 2).unwrap();
        let second = insert_timed(&mut entries, 2, 101, 2).unwrap();
        let third = insert_timed(&mut entries, 3, 102, 2).unwrap();
        assert!(take_or_clone(&mut entries, &first, 102, |value| *value).is_err());
        assert_eq!(
            take_or_clone(&mut entries, &second, 102, |value| *value),
            Ok(2)
        );
        assert_eq!(
            take_or_clone(&mut entries, &third, 102, |value| *value),
            Ok(3)
        );
    }

    #[test]
    fn forged_ids_do_not_select_cache_entries() {
        let mut entries = VecDeque::new();
        insert_timed(&mut entries, 1, 100, 2).unwrap();
        assert_eq!(
            take_or_clone(&mut entries, "0", 100, |value| *value),
            Err(false)
        );
    }

    #[test]
    fn cursors_cannot_cross_detectors() {
        assert!(cursor_detector_matches(
            DetectorId::OldModelUsage,
            DetectorId::OldModelUsage
        ));
        assert!(!cursor_detector_matches(
            DetectorId::OldModelUsage,
            DetectorId::ModelOverthinking
        ));
    }

    #[test]
    fn consumed_preview_entries_reject_expiry_and_reuse() {
        let mut previews = VecDeque::new();
        let id = insert_timed(&mut previews, "private bytes", 100, PRIVATE_LIMIT).unwrap();
        assert_eq!(take_timed(&mut previews, &id, 700), Err(true));
        assert_eq!(take_timed(&mut previews, &id, 700), Err(false));
    }

    #[test]
    fn no_op_and_changed_preview_are_distinct_editor_results() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir(&workspace).unwrap();
        let settings = home.join(".claude/settings.json");
        std::fs::write(&settings, r#"{"model":"new"}"#).unwrap();
        let context = ConfigContext::native(AgentKind::Claude, &home, Some(workspace));
        let editor = AgentConfigEditor::new();
        let change = ConfigChange::SetModel {
            proposed_value: "new".to_owned(),
        };
        assert!(matches!(
            editor.prepare(&context, &change).unwrap(),
            PrepareOutcome::NoOp(_)
        ));

        let change = ConfigChange::SetModel {
            proposed_value: "next".to_owned(),
        };
        let PrepareOutcome::Ready(prepared) = editor.prepare(&context, &change).unwrap() else {
            panic!("the changed value prepares a preview");
        };
        std::fs::write(settings, r#"{"model":"external"}"#).unwrap();
        assert!(matches!(
            editor.apply(&prepared),
            Err(crate::agent_config::ApplyError::Conflict(_))
        ));
    }

    #[test]
    fn reviewed_history_rejects_unknown_or_private_fields() {
        let value = r#"{"version":1,"detector":"old_model_usage","observation":"old","labels":[],"omitted":0,"sessionId":"secret"}"#;
        assert!(parse_versioned::<HistoryFinding>(value).is_err());
    }

    #[test]
    fn source_format_uses_serde_spelling() {
        assert_eq!(
            serde_json::to_value(SourceFormat::OpenCodeSqliteV2).unwrap(),
            "open_code_sqlite_v2"
        );
    }

    #[test]
    fn revision_document_is_deterministic_and_reviewed() {
        let first = revisions_json();
        assert_eq!(first, revisions_json());
        assert!(parse_versioned::<RevisionsDocument>(&first).is_ok());
    }

    #[test]
    fn operation_names_cover_exact_automatic_operations() {
        assert_eq!(operation_name(ChangeOperation::SetModel), "setModel");
        assert_eq!(
            operation_name(ChangeOperation::SetReasoning),
            "setReasoning"
        );
        assert_eq!(
            operation_name(ChangeOperation::DisableMcpServer),
            "disableMcpServer"
        );
    }

    #[test]
    fn automatic_mapping_uses_only_exact_safe_evidence() {
        let old_model = FindingCause::OldModelUsage {
            model: "old".to_owned(),
            replacement: "new".to_owned(),
            turns: 2,
        };
        for agent in [
            AgentKind::Claude,
            AgentKind::Codex,
            AgentKind::OpenCode,
            AgentKind::Pi,
        ] {
            assert_eq!(
                automatic_change(&agent, &old_model),
                Some(ConfigChange::SetModel {
                    proposed_value: "new".to_owned()
                })
            );
        }
        assert!(automatic_change(&AgentKind::Antigravity, &old_model).is_none());

        let reasoning = FindingCause::ModelOverthinking {
            provider: None,
            api: None,
            model: "model".to_owned(),
            reasoning: "xhigh".to_owned(),
            turns: 1,
        };
        for agent in [AgentKind::Claude, AgentKind::Codex, AgentKind::Pi] {
            assert_eq!(
                automatic_change(&agent, &reasoning),
                Some(ConfigChange::SetReasoning {
                    model: Some("model".to_owned()),
                    proposed_value: "high".to_owned()
                })
            );
        }
        assert!(automatic_change(&AgentKind::OpenCode, &reasoning).is_none());

        let mcp = FindingCause::UnusedMcpServer {
            server: "optional".to_owned(),
        };
        for agent in [AgentKind::Claude, AgentKind::Codex] {
            assert_eq!(
                automatic_change(&agent, &mcp),
                Some(ConfigChange::DisableMcpServer {
                    server: "optional".to_owned()
                })
            );
        }
        assert!(automatic_change(&AgentKind::Pi, &mcp).is_none());
    }

    #[test]
    fn worker_changes_are_prompt_only_without_persistent_identity() {
        let cause = FindingCause::OverpoweredSubagents {
            parent_model: "parent".to_owned(),
            worker_model: "worker".to_owned(),
            worker_ordinal: 1,
            parent_call_id: Some("private-call".to_owned()),
        };
        assert!(automatic_change(&AgentKind::Claude, &cause).is_none());
        assert_eq!(
            automatic_unavailable_reason(AgentKind::Claude, &cause),
            "persistent_worker_identity_unavailable"
        );
    }

    #[test]
    fn external_claim_is_awaiting_without_an_application_time() {
        let data_dir = tempfile::tempdir().unwrap();
        let store = Store::open(data_dir.path()).unwrap();
        let now = now_epoch();
        publish_mcp_finding(&store, "private-session", now - 10);
        let controller = RemediationController::new(data_dir.path().to_owned());
        let page = controller
            .list_current_findings_at(DetectorId::UnusedMcpServers, None, Some(10), now)
            .unwrap();
        let finding_id = &page.findings[0].finding_id;

        let result = controller
            .record_external_claim(
                &store,
                finding_id,
                ChangeOperation::DisableMcpServer,
                ActivationBoundary::NextSession,
            )
            .unwrap();
        let stored = store.remediation(&result.remediation_id).unwrap().unwrap();
        assert_eq!(stored.state.as_str(), "awaitingVerification");
        assert_eq!(stored.origin, RemediationOrigin::External);
        assert_eq!(stored.applied_at_epoch, None);

        let public = controller
            .list_native_remediations(&store, None, Some(10))
            .unwrap();
        let rendered = format!("{:?}", public.remediations[0]);
        assert!(!rendered.contains("private-session"));
        assert!(!rendered.contains("/private/"));
        assert!(!rendered.contains(&stored.target_key));
    }

    #[test]
    fn changed_source_rejects_a_cached_finding_before_persistence() {
        let data_dir = tempfile::tempdir().unwrap();
        let store = Store::open(data_dir.path()).unwrap();
        let now = now_epoch();
        let key = SessionKey::new("native", "claude-code", "stale-session");
        publish_mcp_finding(&store, "stale-session", now - 10);
        let controller = RemediationController::new(data_dir.path().to_owned());
        let page = controller
            .list_current_findings_at(DetectorId::UnusedMcpServers, None, Some(10), now)
            .unwrap();
        let mut changed = store.session(&key).unwrap().unwrap();
        changed.source_fingerprint = Some("sv1:changed".to_owned());
        store.upsert_sessions(&[changed], &["claude-code"]).unwrap();

        assert_eq!(
            controller.record_external_claim(
                &store,
                &page.findings[0].finding_id,
                ChangeOperation::DisableMcpServer,
                ActivationBoundary::NextSession,
            ),
            Err(ControllerError::FindingChanged)
        );
        assert!(
            store
                .remediations("native", None, None)
                .unwrap()
                .remediations
                .is_empty()
        );
    }
}
