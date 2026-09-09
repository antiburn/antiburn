//! Exact burn-check targets and direct remediation actions.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use antiburn_local::analysis::{ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION};
use antiburn_local::insights::{DetectorId, ReportWindow};
use antiburn_local::model::AgentKind;
use antiburn_local::model_catalog::{
    ModelCatalog, ModelState, ModelTarget, ReviewedModelCatalog, Support, fixed_route_target,
};
use antiburn_local::pricing::ModelPricing;
use antiburn_local::remediation::{
    FindingCause, FindingDisplay, OldModelSavingsEstimate, OldModelSavingsInput,
    OldModelSavingsUnknownReason, OldModelVerificationTarget, RemediationUnavailableReason,
    SAVINGS_METHOD_REVISION, SavingsInterval, TargetAssessment, VERIFICATION_METHOD_REVISION,
    VerificationOutcome, VerificationStage, VerificationUnknownReason, estimate_old_model_savings,
    remediation_prompt, verify_old_model, verify_prompt_watch,
};
use anyhow::{Context, Result};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;

use crate::agent_config::{AgentConfigEditor, ConfigChange, ConfigContext, ConfigScope};
use crate::insights_report::{self, CurrentFinding, CurrentFindingsRequest};
use crate::store::{
    Remediation, RemediationEvidenceGuard, RemediationRecord, RemediationResult, RemediationState,
    Store,
};

const ID_TTL: Duration = Duration::from_secs(10 * 60);
const TARGET_CACHE_LIMIT: usize = 100;
const MAX_TARGETS: usize = 100;
const TARGET_DOMAIN: &[u8] = b"antiburn/remediation-target/v2\0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnCheckTargetContext {
    pub environment_key: String,
    pub window: ReportWindow,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BurnCheckTarget {
    pub target_id: String,
    pub finding: FindingDisplay,
    pub occurrences: usize,
    pub auto_fix: AutoFixAvailability,
    pub watch: Option<WatchStatus>,
    pub coverage_limits: Vec<CoverageLimit>,
    pub expires_at_epoch: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoFixAvailability {
    Available,
    Unavailable(AutoFixUnavailableReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoFixUnavailableReason {
    UnsupportedOrUnprovenTarget,
    ActiveWatch,
    SafetyCheckFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageLimit {
    CurrentPublishedEvidenceOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WatchStatus {
    pub watch_id: String,
    pub lifecycle: RemediationState,
    pub verification: VerificationStatus,
    pub savings: SavingsStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum VerificationStatus {
    Reserved,
    Watching {
        #[serde(default)]
        reason: Option<VerificationReason>,
        #[serde(default)]
        method_revision: Option<u32>,
        #[serde(default)]
        evidence_revision: Option<String>,
    },
    Fixed {
        method_revision: u32,
        evidence_revision: String,
    },
    StillUnresolved {
        method_revision: u32,
        evidence_revision: String,
    },
    Recurred {
        method_revision: u32,
        evidence_revision: String,
    },
    RecoveryNeeded {
        reason: VerificationReason,
        #[serde(default)]
        checked_at_epoch: Option<i64>,
    },
    VerificationUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerificationReason {
    MissingPostBoundaryEvidence,
    WriteOutcomeUnknown,
    VerificationUnavailable,
    UnsupportedAgent,
    HomeUnavailable,
    PhysicalTargetChanged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SavingsStatus {
    Pending {
        #[serde(default)]
        method_revision: Option<u32>,
    },
    Unavailable,
    Unknown {
        reason: SavingsUnknownReason,
        method_revision: u32,
    },
    Known {
        method: SavingsMethod,
        method_revision: u32,
        pricing_revision: String,
        api_equivalent_cost_avoided_usd: f64,
        measured_through_ms: i64,
        recurrence_ms: Option<i64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SavingsMethod {
    OldModelPriceDifference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SavingsUnknownReason {
    MissingRates,
    MissingEvidence,
    MissingRevision,
    ArithmeticOverflow,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BurnCheckTargetList {
    pub targets: Vec<BurnCheckTarget>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PromptFixResult {
    pub prompt: String,
    pub watch: WatchStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoFixResult {
    pub watch_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerError {
    TargetNotFound,
    TargetExpired,
    TargetChanged,
    Conflict,
    AutoFixUnavailable(AutoFixUnavailableReason),
    PromptUnavailable(RemediationUnavailableReason),
    ApplyFailed(crate::agent_config::ApplyError),
    RecoveryNeeded { watch_id: String },
    PersistenceFailed,
    Internal,
}

impl fmt::Display for ControllerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TargetNotFound => formatter.write_str("target_not_found"),
            Self::TargetExpired => formatter.write_str("target_expired"),
            Self::TargetChanged => formatter.write_str("target_changed"),
            Self::Conflict => formatter.write_str("conflict"),
            Self::AutoFixUnavailable(reason) => {
                write!(formatter, "auto_fix_unavailable:{reason:?}")
            }
            Self::PromptUnavailable(reason) => write!(formatter, "prompt_unavailable:{reason:?}"),
            Self::ApplyFailed(_) => formatter.write_str("apply_failed"),
            Self::RecoveryNeeded { .. } => formatter.write_str("recovery_needed"),
            Self::PersistenceFailed => formatter.write_str("persistence_failed"),
            Self::Internal => formatter.write_str("internal_error"),
        }
    }
}

impl std::error::Error for ControllerError {}

#[derive(Clone)]
struct CachedTarget {
    findings: Vec<CurrentFinding>,
    target_key: String,
    canonical_identity: String,
    workspace_key: Option<String>,
    agent: AgentKind,
    scope_kind: String,
    scope_key: String,
    physical_target_key: Option<String>,
    config: Option<CachedConfig>,
}

#[derive(Clone)]
struct CachedConfig {
    context: ConfigContext,
    change: ConfigChange,
    physical_key: String,
}

struct TimedTarget {
    id: String,
    value: CachedTarget,
    created_at_epoch: i64,
}

#[derive(Default)]
struct ControllerState {
    targets: VecDeque<TimedTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WatchDefinition {
    pub version: u32,
    pub detector: String,
    pub canonical_identity: String,
    pub source_format: String,
    pub workspace_key: Option<String>,
    pub provider: Option<String>,
    pub api: Option<String>,
    pub old_model: Option<String>,
    pub replacement: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
    pub physical_target_key: Option<String>,
    pub verification_method_revision: u32,
    pub savings_method_revision: u32,
    pub pricing_revision: Option<String>,
    pub old_pricing: Option<ModelPricing>,
    pub replacement_pricing: Option<ModelPricing>,
}

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

    pub fn list_burn_check_targets(
        &self,
        store: &Store,
        detector: DetectorId,
        context: BurnCheckTargetContext,
    ) -> Result<BurnCheckTargetList, ControllerError> {
        self.list_burn_check_targets_at(
            store,
            detector,
            context,
            now_epoch(),
            antiburn_local::paths::home_dir().as_deref(),
        )
    }

    fn list_burn_check_targets_at(
        &self,
        store: &Store,
        detector: DetectorId,
        context: BurnCheckTargetContext,
        now: i64,
        home: Option<&Path>,
    ) -> Result<BurnCheckTargetList, ControllerError> {
        let page = insights_report::list_current_findings(
            &self.data_dir,
            CurrentFindingsRequest {
                environment_key: context.environment_key,
                window: context.window,
                detector,
            },
        )
        .map_err(|_| ControllerError::Internal)?;
        let mut grouped: BTreeMap<String, CachedTarget> = BTreeMap::new();
        for finding in page.findings {
            if remediation_prompt(&finding.finding).is_err() {
                continue;
            }
            let display = finding
                .finding
                .display()
                .map_err(ControllerError::PromptUnavailable)?;
            let (group_key, target) = self.resolve_target(store, finding, display.agent, home)?;
            grouped
                .entry(group_key)
                .and_modify(|entry| entry.findings.extend(target.findings.clone()))
                .or_insert(target);
        }
        let truncated = page.truncated || grouped.len() > MAX_TARGETS;
        let expires = now.saturating_add(ID_TTL.as_secs() as i64);
        let mut targets = Vec::new();
        let mut cached = Vec::new();
        for target in grouped.into_values().take(MAX_TARGETS) {
            let display = target.findings[0]
                .finding
                .display()
                .map_err(|_| ControllerError::Internal)?;
            let watch = store
                .latest_remediation_for_target(
                    &target.findings[0].environment_key,
                    target.agent.slug(),
                    &target.target_key,
                )
                .map_err(|_| ControllerError::Internal)?
                .as_ref()
                .map(public_watch)
                .transpose()?;
            let auto_fix = match (&target.config, watch.as_ref()) {
                (Some(_), Some(watch))
                    if !matches!(
                        watch.lifecycle,
                        RemediationState::Watching | RemediationState::Recurred
                    ) =>
                {
                    AutoFixAvailability::Unavailable(AutoFixUnavailableReason::ActiveWatch)
                }
                (Some(_), _) => AutoFixAvailability::Available,
                (None, _) => AutoFixAvailability::Unavailable(
                    AutoFixUnavailableReason::UnsupportedOrUnprovenTarget,
                ),
            };
            let id = random_id().map_err(|_| ControllerError::Internal)?;
            targets.push(BurnCheckTarget {
                target_id: id.clone(),
                finding: display,
                occurrences: target.findings.len(),
                auto_fix,
                watch,
                coverage_limits: vec![CoverageLimit::CurrentPublishedEvidenceOnly],
                expires_at_epoch: expires,
            });
            cached.push(TimedTarget {
                id,
                value: target,
                created_at_epoch: now,
            });
        }
        let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
        state
            .targets
            .retain(|entry| now.saturating_sub(entry.created_at_epoch) <= ID_TTL.as_secs() as i64);
        for entry in cached {
            while state.targets.len() >= TARGET_CACHE_LIMIT {
                state.targets.pop_front();
            }
            state.targets.push_back(entry);
        }
        Ok(BurnCheckTargetList { targets, truncated })
    }

    pub fn copy_prompt_fix_burn_check_target(
        &self,
        store: &Store,
        target_id: &str,
    ) -> Result<PromptFixResult, ControllerError> {
        let now = now_epoch();
        let target = self.cached_target(target_id, now)?;
        self.revalidate(&target)?;
        let prompt = remediation_prompt(&target.findings[0].finding)
            .map_err(ControllerError::PromptUnavailable)?
            .into_string();
        let watch = self.start_watch(
            store,
            &target,
            RemediationState::Watching,
            Some(now.saturating_mul(1_000)),
            now,
        )?;
        Ok(PromptFixResult {
            prompt,
            watch: public_watch(&watch)?,
        })
    }

    pub fn auto_fix_burn_check_target(
        &self,
        store: &Store,
        target_id: &str,
    ) -> Result<AutoFixResult, ControllerError> {
        self.auto_fix_at(store, target_id, now_epoch())
    }

    fn auto_fix_at(
        &self,
        store: &Store,
        target_id: &str,
        now: i64,
    ) -> Result<AutoFixResult, ControllerError> {
        let target = self.cached_target(target_id, now)?;
        self.revalidate(&target)?;
        let config = target.config.as_ref().ok_or({
            ControllerError::AutoFixUnavailable(
                AutoFixUnavailableReason::UnsupportedOrUnprovenTarget,
            )
        })?;
        let prepared = self
            .editor
            .prepare(&config.context, &config.change)
            .map_err(|reason| {
                if matches!(
                    reason,
                    crate::agent_config::ConfigUnavailableReason::CurrentValueMismatch
                        | crate::agent_config::ConfigUnavailableReason::ChangedIdentity
                ) {
                    ControllerError::Conflict
                } else {
                    ControllerError::AutoFixUnavailable(AutoFixUnavailableReason::SafetyCheckFailed)
                }
            })?;
        if physical_key(store, target.agent, prepared.physical_identity())
            .map_err(|_| ControllerError::Internal)?
            != config.physical_key
            || prepared.scope()
                != scope_from_name(&target.scope_kind).ok_or(ControllerError::TargetChanged)?
        {
            return Err(ControllerError::TargetChanged);
        }
        let watch = self.start_watch(store, &target, RemediationState::Reserved, None, now)?;
        if watch.state != RemediationState::Reserved {
            return Err(ControllerError::AutoFixUnavailable(
                AutoFixUnavailableReason::ActiveWatch,
            ));
        }
        if !store
            .begin_remediation_write(&watch.remediation_id, now)
            .map_err(|_| ControllerError::PersistenceFailed)?
        {
            let _ = store.cancel_remediation_reservation(&watch.remediation_id);
            return Err(ControllerError::PersistenceFailed);
        }
        match self.editor.apply(&prepared) {
            Ok(()) => {
                let readback_epoch = now_epoch().max(now);
                let boundary_ms = readback_epoch.saturating_mul(1_000);
                if !store
                    .finalize_remediation_write(&watch.remediation_id, boundary_ms, readback_epoch)
                    .map_err(|_| ControllerError::RecoveryNeeded {
                        watch_id: watch.remediation_id.clone(),
                    })?
                {
                    return Err(ControllerError::RecoveryNeeded {
                        watch_id: watch.remediation_id,
                    });
                }
                Ok(AutoFixResult {
                    watch_id: watch.remediation_id,
                })
            }
            Err(error) if error.replacement_may_have_occurred() => {
                let _ = store.mark_remediation_recovery_needed(
                    &watch.remediation_id,
                    "writeOutcomeUnknown",
                    now_epoch(),
                );
                Err(ControllerError::RecoveryNeeded {
                    watch_id: watch.remediation_id,
                })
            }
            Err(error) => {
                // The editor reports these failures only before the atomic replacement.
                let _ = store.cancel_pre_replacement_write(&watch.remediation_id);
                Err(ControllerError::ApplyFailed(error))
            }
        }
    }

    fn resolve_target(
        &self,
        store: &Store,
        finding: CurrentFinding,
        agent: AgentKind,
        home: Option<&Path>,
    ) -> Result<(String, CachedTarget), ControllerError> {
        let project_root = finding
            .workspace_candidate()
            .and_then(|path| trusted_workspace(store, path).ok().flatten());
        let workspace_key = project_root
            .as_deref()
            .map(|root| hashed_workspace_key(store, root))
            .transpose()
            .map_err(|_| ControllerError::Internal)?;
        let mut config = None;
        let project_scoped = matches!(
            finding.finding.cause(),
            FindingCause::ModelOverthinking { .. }
                | FindingCause::UnusedMcpServer { .. }
                | FindingCause::UnusedBuiltInTool { .. }
                | FindingCause::UnusedSkill { .. }
                | FindingCause::OldModelUsage { .. }
                | FindingCause::OveruseOfFastMode { .. }
        );
        let (mut scope_kind, mut scope_key) =
            if project_scoped && let Some(project) = project_root.as_deref() {
                (
                    "project".to_owned(),
                    hashed_workspace_key(store, project).map_err(|_| ControllerError::Internal)?,
                )
            } else {
                (
                    "session".to_owned(),
                    hashed_value(store, "session", finding.finding.session_id())
                        .map_err(|_| ControllerError::Internal)?,
                )
            };
        let mut physical_target_key = None;
        if let FindingCause::OldModelUsage { model, .. } = finding.finding.cause()
            && finding.effective_model.as_deref() == Some(model.as_str())
            && let (Some(attributed_scope), Some(attributed_target)) = (
                finding.effective_model_scope.as_deref(),
                finding.effective_model_target_hash.as_ref(),
            )
        {
            scope_kind = attributed_scope.to_owned();
            physical_target_key = Some(attributed_target.clone());
            scope_key = if attributed_scope == "global" {
                attributed_target.clone()
            } else {
                project_root
                    .as_deref()
                    .map(|root| hashed_workspace_key(store, root))
                    .transpose()
                    .map_err(|_| ControllerError::Internal)?
                    .unwrap_or_else(|| attributed_target.clone())
            };
        }
        if let (
            Some(home),
            FindingCause::OldModelUsage {
                model, replacement, ..
            },
        ) = (home, finding.finding.cause())
            && matches!(agent, AgentKind::Claude | AgentKind::Codex)
            && (scope_kind == "global" || project_root.is_some())
            && reviewed_replacement(agent, finding.finding.cause())
        {
            let mut context = ConfigContext::native(agent, home, project_root.clone());
            context.runtime_override_present = runtime_override_present(agent);
            context.managed_configuration_present = managed_configuration_present(agent, home);
            if let Ok(effective) = self.editor.effective_model(&context)
                && effective.value == *model
            {
                let key = physical_key(store, agent, effective.physical_identity())
                    .map_err(|_| ControllerError::Internal)?;
                if physical_target_key.as_deref() == Some(key.as_str())
                    && scope_kind == scope_name(effective.scope)
                {
                    config = Some(CachedConfig {
                        context,
                        change: ConfigChange {
                            expected_value: model.clone(),
                            proposed_value: replacement.clone(),
                        },
                        physical_key: key,
                    });
                }
            }
        }
        let canonical_identity = finding.finding.canonical_identity(&scope_key);
        let group_key = hashed_parts(
            store,
            TARGET_DOMAIN,
            &[
                &finding.environment_key,
                agent.slug(),
                &scope_kind,
                &scope_key,
                &canonical_identity,
            ],
        )
        .map_err(|_| ControllerError::Internal)?;
        let target_key = if let Some(physical) = physical_target_key.as_ref() {
            hashed_parts(
                store,
                TARGET_DOMAIN,
                &[
                    &finding.environment_key,
                    agent.slug(),
                    physical,
                    &canonical_identity,
                ],
            )
            .map_err(|_| ControllerError::Internal)?
        } else {
            group_key.clone()
        };
        Ok((
            group_key,
            CachedTarget {
                findings: vec![finding],
                target_key,
                canonical_identity,
                workspace_key,
                agent,
                scope_kind,
                scope_key,
                physical_target_key,
                config,
            },
        ))
    }

    fn cached_target(&self, id: &str, now: i64) -> Result<CachedTarget, ControllerError> {
        let state = self.state.lock().map_err(|_| ControllerError::Internal)?;
        let entry = state
            .targets
            .iter()
            .find(|entry| entry.id == id)
            .ok_or(ControllerError::TargetNotFound)?;
        if now.saturating_sub(entry.created_at_epoch) > ID_TTL.as_secs() as i64 {
            return Err(ControllerError::TargetExpired);
        }
        Ok(entry.value.clone())
    }

    fn revalidate(&self, target: &CachedTarget) -> Result<(), ControllerError> {
        for finding in &target.findings {
            match insights_report::revalidate_current_finding(&self.data_dir, finding) {
                Ok(true) => {}
                Ok(false) => return Err(ControllerError::TargetChanged),
                Err(_) => return Err(ControllerError::Internal),
            }
        }
        Ok(())
    }

    fn start_watch(
        &self,
        store: &Store,
        target: &CachedTarget,
        state: RemediationState,
        boundary_ms: Option<i64>,
        now: i64,
    ) -> Result<RemediationRecord, ControllerError> {
        let definition = watch_definition(target);
        let result = if state == RemediationState::Reserved {
            json!({"version": 1, "verification": {"status": "reserved"}, "savings": {"status": "pending"}})
        } else if definition.old_model.is_none() {
            json!({"version": 1, "verification": {"status": "watching", "methodRevision": VERIFICATION_METHOD_REVISION}, "savings": {"status": "unavailable"}})
        } else if definition.physical_target_key.is_some() {
            json!({"version": 1, "verification": {"status": "watching", "methodRevision": VERIFICATION_METHOD_REVISION}, "savings": {"status": "pending", "methodRevision": SAVINGS_METHOD_REVISION}})
        } else {
            json!({"version": 1, "verification": {"status": "verificationUnavailable"}, "savings": {"status": "unavailable"}})
        };
        let remediation = Remediation {
            remediation_id: random_id().map_err(|_| ControllerError::Internal)?,
            target_key: target.target_key.clone(),
            environment_key: target.findings[0].environment_key.clone(),
            agent: target.agent.slug().into(),
            scope_kind: target.scope_kind.clone(),
            scope_key: target.scope_key.clone(),
            state,
            definition_json: serde_json::to_string(&definition)
                .map_err(|_| ControllerError::Internal)?,
            result_json: result.to_string(),
            created_at_epoch: now,
            effective_boundary_ms: boundary_ms,
        };
        let guards = target
            .findings
            .iter()
            .map(evidence_guard)
            .collect::<Vec<_>>();
        store
            .create_or_reuse_remediation(&remediation, &guards)
            .map_err(|_| ControllerError::PersistenceFailed)?
            .ok_or(ControllerError::TargetChanged)
    }
}

pub(crate) fn evaluate_dirty_remediation(
    data_dir: &Path,
    store: &Store,
    record: &RemediationRecord,
    now: i64,
) -> Result<bool> {
    let definition: WatchDefinition = serde_json::from_str(&record.definition_json)?;
    if definition.old_model.is_none() {
        return evaluate_generic_remediation(data_dir, store, record, &definition, now);
    }
    let (Some(old_model), Some(replacement), Some(boundary_ms), Some(_)) = (
        definition.old_model.as_ref(),
        definition.replacement.as_ref(),
        record.effective_boundary_ms,
        definition.physical_target_key.as_ref(),
    ) else {
        let result = RemediationResult {
            state: record.state,
            result_json: json!({"version": 1, "verification": {"status": "verificationUnavailable"}, "savings": {"status": "unavailable"}}).to_string(),
            evaluated_at_epoch: now,
            transition_at_epoch: None,
        };
        return store.replace_remediation_result(
            &record.remediation_id,
            record.dirty_revision,
            &result,
        );
    };
    let target = OldModelVerificationTarget {
        scope: record.scope_key.clone(),
        provider: definition.provider.clone(),
        api: definition.api.clone(),
        old_model: old_model.clone(),
        replacement: replacement.clone(),
    };
    let stage = if record.state == RemediationState::Fixed {
        VerificationStage::Fixed
    } else {
        VerificationStage::Watching
    };
    let verification_boundary = if stage == VerificationStage::Fixed {
        stored_observed_at_ms(&record.result_json).unwrap_or(boundary_ms)
    } else {
        boundary_ms
    };
    let fixed_at_ms = (stage == VerificationStage::Fixed)
        .then(|| stored_observed_at_ms(&record.result_json))
        .flatten();
    let snapshot = insights_report::old_model_remediation_evidence(
        data_dir,
        record,
        &definition,
        boundary_ms,
        fixed_at_ms,
    )?;
    let verification = verify_old_model(
        &target,
        stage,
        verification_boundary,
        &snapshot.observations,
    );
    let state = match verification.outcome {
        VerificationOutcome::Fixed => RemediationState::Fixed,
        VerificationOutcome::Recurred => RemediationState::Recurred,
        _ => record.state,
    };
    let savings = old_model_savings(
        &definition,
        boundary_ms,
        snapshot.measured_through_ms,
        snapshot.recurrence_ms,
        snapshot.replacement_tokens,
        snapshot.token_overflow,
    );
    let verification_status = match verification.outcome {
        VerificationOutcome::Fixed => VerificationStatus::Fixed {
            method_revision: verification.method_revision,
            evidence_revision: snapshot.evidence_revision,
        },
        VerificationOutcome::StillUnresolved => VerificationStatus::StillUnresolved {
            method_revision: verification.method_revision,
            evidence_revision: snapshot.evidence_revision,
        },
        VerificationOutcome::Recurred => VerificationStatus::Recurred {
            method_revision: verification.method_revision,
            evidence_revision: snapshot.evidence_revision,
        },
        VerificationOutcome::Unknown(VerificationUnknownReason::MissingPostBoundaryEvidence) => {
            VerificationStatus::Watching {
                reason: Some(VerificationReason::MissingPostBoundaryEvidence),
                method_revision: Some(verification.method_revision),
                evidence_revision: Some(snapshot.evidence_revision),
            }
        }
    };
    let stored_fixed_at_ms = if stage == VerificationStage::Fixed
        && !matches!(verification.outcome, VerificationOutcome::Recurred)
    {
        fixed_at_ms
    } else {
        verification.observed_at_ms
    };
    store.replace_remediation_result(
        &record.remediation_id,
        record.dirty_revision,
        &RemediationResult {
            state,
            result_json: json!({
                "version": 1,
                "verification": verification_status,
                "savings": savings,
                "observedAtMs": stored_fixed_at_ms,
            })
            .to_string(),
            evaluated_at_epoch: verification
                .observed_at_ms
                .map_or(now, |value| value.saturating_div(1_000))
                .max(record.created_at_epoch),
            transition_at_epoch: verification
                .observed_at_ms
                .map(|value| value.saturating_div(1_000)),
        },
    )
}

fn evaluate_generic_remediation(
    data_dir: &Path,
    store: &Store,
    record: &RemediationRecord,
    definition: &WatchDefinition,
    now: i64,
) -> Result<bool> {
    let Some(detector) = DetectorId::ALL
        .into_iter()
        .find(|detector| detector.key() == definition.detector)
    else {
        return Ok(false);
    };
    let boundary_ms = record.effective_boundary_ms.unwrap_or(0);
    let stage = if record.state == RemediationState::Fixed {
        VerificationStage::Fixed
    } else {
        VerificationStage::Watching
    };
    let verification_boundary = if stage == VerificationStage::Fixed {
        stored_observed_at_ms(&record.result_json).unwrap_or(boundary_ms)
    } else {
        boundary_ms
    };
    let resource = definition.resource.as_deref();
    let page = insights_report::remediation_assessments(
        data_dir,
        &record.environment_key,
        &record.agent,
        detector,
        resource,
        verification_boundary,
    )?;
    let mut assessments = Vec::new();
    for assessment in page.assessments {
        if format!("{:?}", assessment.source_format) != definition.source_format {
            continue;
        }
        let project_scope = assessment
            .workspace_candidate
            .as_deref()
            .and_then(|path| trusted_workspace(store, path).ok().flatten())
            .and_then(|path| hashed_workspace_key(store, &path).ok());
        let session_scope = hashed_value(store, "session", &assessment.session_id).ok();
        let scope_matches = scope_identity_matches(
            &record.scope_kind,
            &record.scope_key,
            project_scope.as_deref(),
            session_scope.as_deref(),
        );
        if !scope_matches {
            continue;
        }
        let mut target_present = false;
        if let antiburn_local::remediation::FindingAssessment::Findings(findings) =
            assessment.assessment
        {
            for finding in findings {
                let identity = finding.canonical_identity(&record.scope_key);
                let present = identity == definition.canonical_identity;
                target_present |= present;
                assessments.push(TargetAssessment {
                    observed_at_ms: assessment.observed_at_ms,
                    identity,
                    target_present: present,
                    complete: false,
                });
            }
        }
        if !target_present {
            assessments.push(TargetAssessment {
                observed_at_ms: assessment.observed_at_ms,
                identity: definition.canonical_identity.clone(),
                target_present: false,
                complete: bounded_absence_complete(assessment.absence_complete, page.truncated),
            });
        }
    }
    let verification = verify_prompt_watch(
        &definition.canonical_identity,
        stage,
        verification_boundary,
        &assessments,
    );
    let state = match verification.outcome {
        VerificationOutcome::Fixed => RemediationState::Fixed,
        VerificationOutcome::Recurred => RemediationState::Recurred,
        _ => record.state,
    };
    let verification_status = verification_status(
        &verification,
        format!(
            "evidence-{}-{}",
            ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION
        ),
    );
    let stored_fixed_at_ms = if stage == VerificationStage::Fixed
        && !matches!(verification.outcome, VerificationOutcome::Recurred)
    {
        Some(verification_boundary)
    } else {
        verification.observed_at_ms
    };
    store.replace_remediation_result(
        &record.remediation_id,
        record.dirty_revision,
        &RemediationResult {
            state,
            result_json: json!({
                "version": 1,
                "verification": verification_status,
                "savings": {"status": "unavailable"},
                "observedAtMs": stored_fixed_at_ms,
            })
            .to_string(),
            evaluated_at_epoch: verification
                .observed_at_ms
                .map_or(now, |value| value.saturating_div(1_000)),
            transition_at_epoch: verification
                .observed_at_ms
                .map(|value| value.saturating_div(1_000)),
        },
    )
}

fn stored_observed_at_ms(result_json: &str) -> Option<i64> {
    serde_json::from_str::<serde_json::Value>(result_json)
        .ok()?
        .get("observedAtMs")?
        .as_i64()
}

fn scope_identity_matches(
    kind: &str,
    expected: &str,
    project: Option<&str>,
    session: Option<&str>,
) -> bool {
    match kind {
        "project" => project == Some(expected),
        "session" => session == Some(expected),
        _ => false,
    }
}

const fn bounded_absence_complete(assessment_complete: bool, truncated: bool) -> bool {
    assessment_complete && !truncated
}

fn verification_status(
    verification: &antiburn_local::remediation::VerificationResult,
    evidence_revision: String,
) -> VerificationStatus {
    match verification.outcome {
        VerificationOutcome::Fixed => VerificationStatus::Fixed {
            method_revision: verification.method_revision,
            evidence_revision,
        },
        VerificationOutcome::StillUnresolved => VerificationStatus::StillUnresolved {
            method_revision: verification.method_revision,
            evidence_revision,
        },
        VerificationOutcome::Recurred => VerificationStatus::Recurred {
            method_revision: verification.method_revision,
            evidence_revision,
        },
        VerificationOutcome::Unknown(_) => VerificationStatus::Watching {
            reason: Some(VerificationReason::MissingPostBoundaryEvidence),
            method_revision: Some(verification.method_revision),
            evidence_revision: Some(evidence_revision),
        },
    }
}

pub(crate) fn recover_uncertain_write(
    store: &Store,
    record: &RemediationRecord,
    now: i64,
) -> Result<bool> {
    let definition: WatchDefinition = serde_json::from_str(&record.definition_json)?;
    let Some(replacement) = definition.replacement else {
        return store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        );
    };
    let Some(agent) = crate::agents::kind_from_slug(&record.agent) else {
        return store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "unsupportedAgent",
            now,
        );
    };
    let Some(home) = antiburn_local::paths::home_dir() else {
        return store.defer_remediation_recovery(&record.remediation_id, "homeUnavailable", now);
    };
    let project = if record.scope_kind == "project" {
        store
            .repositories()?
            .into_iter()
            .filter_map(|repository| repository.repo_root)
            .filter_map(|root| PathBuf::from(root).canonicalize().ok())
            .find(|project| {
                definition.workspace_key.as_deref()
                    == hashed_workspace_key(store, project).ok().as_deref()
            })
    } else {
        None
    };
    if record.scope_kind == "project" && project.is_none() {
        return store.defer_remediation_recovery(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        );
    }
    let context = ConfigContext::native(agent, &home, project);
    let Ok(effective) = AgentConfigEditor::new().effective_model(&context) else {
        return store.defer_remediation_recovery(
            &record.remediation_id,
            "verificationUnavailable",
            now,
        );
    };
    let key = physical_key(store, agent, effective.physical_identity())?;
    if !recovery_target_matches(
        definition.physical_target_key.as_deref(),
        &record.scope_kind,
        &key,
        effective.scope,
    ) {
        return store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "physicalTargetChanged",
            now,
        );
    }
    match effective.value {
        value if value == replacement => {
            store.finalize_remediation_write(&record.remediation_id, now.saturating_mul(1_000), now)
        }
        _ => store.mark_remediation_recovery_checked(
            &record.remediation_id,
            "writeOutcomeUnknown",
            now,
        ),
    }
}

fn recovery_target_matches(
    stored_target: Option<&str>,
    stored_scope: &str,
    effective_target: &str,
    effective_scope: ConfigScope,
) -> bool {
    stored_target == Some(effective_target) && stored_scope == scope_name(effective_scope)
}

fn old_model_savings(
    definition: &WatchDefinition,
    boundary_ms: i64,
    measured: Option<i64>,
    recurrence: Option<i64>,
    tokens: Option<antiburn_local::pricing::ModelTokens>,
    token_overflow: bool,
) -> SavingsStatus {
    if token_overflow {
        return SavingsStatus::Unknown {
            reason: SavingsUnknownReason::ArithmeticOverflow,
            method_revision: SAVINGS_METHOD_REVISION,
        };
    }
    let estimate = measured
        .map(|measured_through_ms| {
            estimate_old_model_savings(&OldModelSavingsInput {
                interval: SavingsInterval {
                    boundary_ms,
                    measured_through_ms: recurrence.unwrap_or(measured_through_ms),
                    recurrence_ms: recurrence,
                },
                tokens,
                old_pricing: definition.old_pricing.clone(),
                replacement_pricing: definition.replacement_pricing.clone(),
                pricing_revision: definition.pricing_revision.clone(),
            })
        })
        .unwrap_or(OldModelSavingsEstimate::Unknown(
            OldModelSavingsUnknownReason::MissingEvidence,
        ));
    match estimate {
        OldModelSavingsEstimate::Known(value) => SavingsStatus::Known {
            method: SavingsMethod::OldModelPriceDifference,
            method_revision: value.method_revision,
            pricing_revision: value.pricing_revision,
            api_equivalent_cost_avoided_usd: value.api_equivalent_cost_avoided_usd,
            measured_through_ms: recurrence
                .or(measured)
                .expect("known savings has an interval"),
            recurrence_ms: recurrence,
        },
        OldModelSavingsEstimate::Unknown(reason) => SavingsStatus::Unknown {
            reason: match reason {
                OldModelSavingsUnknownReason::MissingRates => SavingsUnknownReason::MissingRates,
                OldModelSavingsUnknownReason::MissingEvidence => {
                    SavingsUnknownReason::MissingEvidence
                }
                OldModelSavingsUnknownReason::MissingRevision => {
                    SavingsUnknownReason::MissingRevision
                }
                OldModelSavingsUnknownReason::ArithmeticOverflow => {
                    SavingsUnknownReason::ArithmeticOverflow
                }
            },
            method_revision: SAVINGS_METHOD_REVISION,
        },
    }
}

fn watch_definition(target: &CachedTarget) -> WatchDefinition {
    let (provider, api, old_model, replacement) = match target.findings[0].finding.cause() {
        FindingCause::OldModelUsage {
            provider,
            api,
            model,
            replacement,
            ..
        } => (
            provider.clone(),
            api.clone(),
            Some(model.clone()),
            Some(replacement.clone()),
        ),
        _ => (None, None, None, None),
    };
    let old_pricing = old_model.as_deref().and_then(|model| {
        reviewed_pricing(target.agent, provider.as_deref(), api.as_deref(), model)
    });
    let replacement_pricing = replacement.as_deref().and_then(|model| {
        reviewed_pricing(target.agent, provider.as_deref(), api.as_deref(), model)
    });
    WatchDefinition {
        version: 1,
        detector: target.findings[0].finding.detector.key().into(),
        canonical_identity: target.canonical_identity.clone(),
        source_format: format!("{:?}", target.findings[0].finding.source_format),
        workspace_key: target.workspace_key.clone(),
        provider,
        api,
        old_model,
        replacement,
        resource: match target.findings[0].finding.cause() {
            FindingCause::UnusedMcpServer { server } => Some(server.clone()),
            FindingCause::UnusedBuiltInTool { tool, .. } => Some(tool.clone()),
            FindingCause::UnusedSkill { skill } => Some(skill.clone()),
            _ => None,
        },
        physical_target_key: target.physical_target_key.clone(),
        verification_method_revision: VERIFICATION_METHOD_REVISION,
        savings_method_revision: SAVINGS_METHOD_REVISION,
        pricing_revision: (old_pricing.is_some() && replacement_pricing.is_some()).then(|| {
            format!(
                "pricing-generation-{}",
                antiburn_local::analysis::pricing_generation()
            )
        }),
        old_pricing,
        replacement_pricing,
    }
}

fn reviewed_pricing(
    agent: AgentKind,
    provider: Option<&str>,
    api: Option<&str>,
    model: &str,
) -> Option<ModelPricing> {
    let target = ModelTarget::new(
        agent.slug(),
        provider.unwrap_or_default(),
        api.unwrap_or_default(),
        model,
    );
    match ReviewedModelCatalog::default().resolve(&target) {
        Support::Supported(definition) => match definition.pricing {
            Support::Supported(pricing) => Some(pricing),
            _ => None,
        },
        _ => None,
    }
}

fn reviewed_replacement(agent: AgentKind, cause: &FindingCause) -> bool {
    let FindingCause::OldModelUsage {
        provider,
        api,
        model,
        replacement,
        ..
    } = cause
    else {
        return false;
    };
    let catalog = ReviewedModelCatalog::default();
    let route_target = |model: &str| {
        ModelTarget::new(
            agent.slug(),
            provider.as_deref().unwrap_or_default(),
            api.as_deref().unwrap_or_default(),
            model,
        )
    };
    let (Support::Supported(old), Support::Supported(new)) = (
        catalog.resolve(&route_target(model)),
        catalog.resolve(&route_target(replacement)),
    ) else {
        return false;
    };
    matches!(old.state, ModelState::Obsolete(rule) if rule.replacement == new.canonical_model_key)
}

fn evidence_guard(finding: &CurrentFinding) -> RemediationEvidenceGuard {
    RemediationEvidenceGuard {
        environment_key: finding.environment_key.clone(),
        agent: finding.agent.clone(),
        session_id: finding.session_id.clone(),
        source_generation: finding.source_generation,
        published_fence: finding.published_fence,
        source_fingerprint: finding.source_fingerprint.clone(),
        processed_fingerprint: finding.processed_fingerprint.clone(),
        parser_revision: finding.parser_revision,
        analyzer_revision: finding.analyzer_revision,
        evidence_schema_revision: finding.evidence_schema_revision,
    }
}

fn public_watch(record: &RemediationRecord) -> Result<WatchStatus, ControllerError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct StoredResult {
        verification: VerificationStatus,
        savings: SavingsStatus,
    }

    let result: StoredResult =
        serde_json::from_str(&record.result_json).map_err(|_| ControllerError::Internal)?;
    Ok(WatchStatus {
        watch_id: record.remediation_id.clone(),
        lifecycle: record.state,
        verification: result.verification,
        savings: result.savings,
    })
}

fn trusted_workspace(store: &Store, candidate: &Path) -> Result<Option<PathBuf>> {
    let candidate = candidate.canonicalize()?;
    Ok(store
        .repositories()?
        .into_iter()
        .filter(|repository| repository.enabled && repository.status == "accessible")
        .filter_map(|repository| PathBuf::from(repository.repo_root?).canonicalize().ok())
        .filter(|root| candidate.starts_with(root))
        .max_by_key(|root| root.components().count()))
}

pub(crate) fn hashed_workspace_key(store: &Store, workspace: &Path) -> Result<String> {
    hashed_value(store, "workspace", &workspace.to_string_lossy())
}

pub(crate) fn publication_model_attribution(
    store: &Store,
    key: &crate::store::SessionKey,
    status: crate::store::PublishedEvidence,
    evidence_json: &str,
) -> Result<Option<(String, String, String)>> {
    if status != crate::store::PublishedEvidence::Ready {
        return Ok(None);
    }
    let Some(agent) = crate::agents::kind_from_slug(&key.agent)
        .filter(|agent| matches!(agent, AgentKind::Claude | AgentKind::Codex))
    else {
        return Ok(None);
    };
    let Some(session) = store.session(key)? else {
        return Ok(None);
    };
    let workspace = session
        .cwd
        .as_deref()
        .map(Path::new)
        .and_then(|path| trusted_workspace(store, path).ok().flatten());
    let Some(home) = antiburn_local::paths::home_dir() else {
        return Ok(None);
    };
    let mut context = ConfigContext::native(agent, home, workspace);
    context.runtime_override_present = runtime_override_present(agent);
    context.managed_configuration_present =
        managed_configuration_present(agent, &context.home_root);
    let Ok(effective) = AgentConfigEditor::new().effective_model(&context) else {
        return Ok(None);
    };
    let evidence: antiburn_local::analysis::SessionEvidence = serde_json::from_str(evidence_json)?;
    let observed = match evidence.models {
        antiburn_local::analysis::EvidenceValue::Complete(models) => {
            effective_model_observed(agent, &effective.value, &models.control_observations)
        }
        _ => false,
    };
    if !observed {
        return Ok(None);
    }
    Ok(Some((
        physical_key(store, agent, effective.physical_identity())?,
        scope_name(effective.scope).to_owned(),
        effective.value,
    )))
}

fn effective_model_observed(
    agent: AgentKind,
    model: &str,
    observations: &[antiburn_local::analysis::ModelControlObservation],
) -> bool {
    let Some(route) = fixed_route_target(agent.slug(), model) else {
        return false;
    };
    observations.iter().any(|observation| {
        observation.provider.as_deref() == Some(route.provider.as_str())
            && observation.api.as_deref() == Some(route.api.as_str())
            && observation.model == model
            && observation.turns.main_loop > 0
    })
}

fn physical_key(
    store: &Store,
    agent: AgentKind,
    (path, semantic): (&Path, &'static str),
) -> Result<String> {
    hashed_parts(
        store,
        TARGET_DOMAIN,
        &[agent.slug(), &path.to_string_lossy(), semantic],
    )
}

fn hashed_value(store: &Store, domain: &str, value: &str) -> Result<String> {
    hashed_parts(store, domain.as_bytes(), &[value])
}

fn hashed_parts(store: &Store, domain: &[u8], values: &[&str]) -> Result<String> {
    let secret = store.provider_account_secret()?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&secret).context("invalid target secret")?;
    mac.update(domain);
    for value in values {
        mac.update(&(value.len() as u32).to_be_bytes());
        mac.update(value.as_bytes());
    }
    Ok(hex(&mac.finalize().into_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

fn scope_name(scope: ConfigScope) -> &'static str {
    match scope {
        ConfigScope::Global => "global",
        ConfigScope::Project => "project",
    }
}
fn scope_from_name(value: &str) -> Option<ConfigScope> {
    match value {
        "global" => Some(ConfigScope::Global),
        "project" => Some(ConfigScope::Project),
        _ => None,
    }
}

pub(crate) fn runtime_override_present(agent: AgentKind) -> bool {
    let names: &[&str] = match agent {
        AgentKind::Claude => &[
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "CLAUDE_CONFIG_DIR",
        ],
        AgentKind::Codex => &["CODEX_HOME", "CODEX_MODEL", "OPENAI_MODEL"],
        _ => &[],
    };
    names.iter().any(|name| std::env::var_os(name).is_some())
}

pub(crate) fn managed_configuration_present(agent: AgentKind, home: &Path) -> bool {
    let paths = match agent {
        AgentKind::Claude => vec![
            home.join(".claude/managed-settings.json"),
            PathBuf::from("/etc/claude-code/managed-settings.json"),
            PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json"),
        ],
        AgentKind::Codex => vec![
            home.join(".codex/managed_config.toml"),
            home.join(".codex/requirements.toml"),
            PathBuf::from("/etc/codex/managed_config.toml"),
        ],
        _ => Vec::new(),
    };
    paths.iter().any(|path| path.try_exists().unwrap_or(true))
}

fn random_id() -> Result<String> {
    let mut bytes = [0_u8; 24];
    getrandom::fill(&mut bytes).context("random id generation failed")?;
    Ok(hex(&bytes))
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remediation_pricing_requires_the_reviewed_provider_route() {
        assert!(
            reviewed_pricing(
                AgentKind::Claude,
                Some("anthropic"),
                Some("messages"),
                "claude-opus-5",
            )
            .is_some()
        );
        assert!(
            reviewed_pricing(
                AgentKind::Claude,
                Some("openai"),
                Some("responses"),
                "claude-opus-5",
            )
            .is_none()
        );
    }

    #[test]
    fn automatic_replacement_requires_a_reviewed_exact_route() {
        let cause = FindingCause::OldModelUsage {
            provider: Some("anthropic".into()),
            api: Some("messages".into()),
            model: "claude-opus-4-8".into(),
            replacement: "claude-opus-5".into(),
            turns: 1,
        };
        assert!(reviewed_replacement(AgentKind::Claude, &cause));
        let mut gateway = cause.clone();
        let FindingCause::OldModelUsage { provider, .. } = &mut gateway else {
            unreachable!()
        };
        *provider = Some("gateway".into());
        assert!(!reviewed_replacement(AgentKind::Claude, &gateway));
    }

    #[test]
    fn publication_attribution_requires_the_fixed_route_and_a_main_loop_turn() {
        use antiburn_local::analysis::{ModelControlObservation, TurnCounts};

        let observation = |provider: &str, api: &str, main_loop| ModelControlObservation {
            provider: Some(provider.into()),
            api: Some(api.into()),
            model: "claude-opus-4-8".into(),
            effort: None,
            speed: None,
            last_ts_ms: 100,
            turns: TurnCounts {
                main_loop,
                delegated: 1,
            },
        };
        assert!(!effective_model_observed(
            AgentKind::Claude,
            "claude-opus-4-8",
            &[observation("gateway", "messages", 1)],
        ));
        assert!(!effective_model_observed(
            AgentKind::Claude,
            "claude-opus-4-8",
            &[observation("anthropic", "messages", 0)],
        ));
        assert!(effective_model_observed(
            AgentKind::Claude,
            "claude-opus-4-8",
            &[observation("anthropic", "messages", 1)],
        ));
    }

    #[test]
    fn recovery_rejects_a_changed_physical_target_or_scope() {
        assert!(recovery_target_matches(
            Some("target-a"),
            "global",
            "target-a",
            ConfigScope::Global,
        ));
        assert!(!recovery_target_matches(
            Some("target-a"),
            "global",
            "target-b",
            ConfigScope::Global,
        ));
        assert!(!recovery_target_matches(
            Some("target-a"),
            "global",
            "target-a",
            ConfigScope::Project,
        ));
    }

    #[test]
    fn generic_watch_scope_rejects_other_projects_and_sessions() {
        assert!(scope_identity_matches(
            "project",
            "project-a",
            Some("project-a"),
            Some("session-b")
        ));
        assert!(!scope_identity_matches(
            "project",
            "project-a",
            Some("project-b"),
            Some("session-a")
        ));
        assert!(scope_identity_matches(
            "session",
            "session-a",
            Some("project-b"),
            Some("session-a")
        ));
        assert!(!scope_identity_matches(
            "session",
            "session-a",
            Some("project-a"),
            Some("session-b")
        ));
    }

    #[test]
    fn a_truncated_assessment_cannot_prove_a_fix() {
        assert!(!bounded_absence_complete(true, true));
        let result = verify_prompt_watch(
            "target",
            VerificationStage::Watching,
            100,
            &[TargetAssessment {
                observed_at_ms: 101,
                identity: "target".into(),
                target_present: false,
                complete: bounded_absence_complete(true, true),
            }],
        );
        assert!(matches!(result.outcome, VerificationOutcome::Unknown(_)));
    }
}
