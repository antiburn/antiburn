//! Exact burn-check targets and direct remediation actions.

mod config;
mod display;
mod models;
mod recovery;
mod stored;
mod target;
mod vendors;
mod watch;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use antiburn_local::analysis::{ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, SourceFormat};
use antiburn_local::insights::DetectorId;
use antiburn_local::model::AgentKind;
use antiburn_local::model_catalog::{
    ModelCatalog, ModelState, ModelTarget, ReviewedModelCatalog, Support, model_control_target,
};
use antiburn_local::pricing::ModelPricing;
use antiburn_local::remediation::{
    FindingAssessment, FindingCause, FindingUnavailableReason, OldModelSavingsEstimate,
    OldModelSavingsInput, OldModelSavingsUnknownReason, OldModelVerificationTarget,
    REMEDIATION_POLICY_REVISION, RemediationUnavailableReason, SAVINGS_METHOD_REVISION,
    SavingsEstimateInput, SavingsEstimateMethod, SavingsInterval, SavingsValue, TargetAssessment,
    VERIFICATION_METHOD_REVISION, VerificationOutcome, VerificationStage,
    VerificationUnknownReason, estimate_old_model_savings, estimate_savings,
    fallback_remediation_prompt, remediation_prompt, verification_evidence_supported,
    verify_old_model, verify_prompt_watch,
};
use anyhow::{Context, Result};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::json;
use sha2::Sha256;

use crate::agent_config::{
    AgentConfigEditor, ConfigContext, ConfigOperation, ConfigScope, ConfigSetting, PreparedChange,
};
use crate::insights_report::{self, CurrentFinding, CurrentFindingsRequest};
use crate::store::{PassiveRemediation, SessionKey};
use crate::store::{
    Remediation, RemediationContribution, RemediationDisplaySnapshot, RemediationEvidenceGuard,
    RemediationRecord, RemediationResult, RemediationState, Store,
};
use vendors::{ActionSupport, RemediationAction, vendor_policy};

pub(crate) use config::publication_config_attribution;
use config::*;
#[cfg(test)]
pub(crate) use config::{hashed_workspace_key, publication_config_attribution_with_home};
use display::*;
pub use models::*;
pub(crate) use recovery::recover_uncertain_write;
pub(crate) use stored::WatchDefinition;
use stored::{
    StoredDisplaySnapshot, parse_display_snapshot, parse_watch_definition, stored_result,
    validate_envelope_version,
};
pub(crate) use target::passive_remediations;
use target::*;
pub(crate) use watch::evaluate_dirty_remediation;

fn representative_paths(
    store: &Store,
    keys: impl IntoIterator<Item = SessionKey>,
) -> Result<Vec<String>, ControllerError> {
    let keys = keys
        .into_iter()
        .take(MAX_PASSIVE_CANDIDATES)
        .collect::<Vec<_>>();
    let records = store
        .session_records_for_session_keys(&keys)
        .map_err(|_| ControllerError::Internal)?;
    let mut paths = Vec::new();
    for key in keys {
        let Some(record) = records.iter().find(|record| record.key == key) else {
            continue;
        };
        if record.source_kind != "file"
            || !(Path::new(&record.source_label).is_absolute()
                || record.source_label.starts_with('/'))
        {
            continue;
        }
        if !paths.contains(&record.source_label) {
            paths.push(record.source_label.clone());
            if paths.len() == 3 {
                break;
            }
        }
    }
    Ok(paths)
}

const ID_TTL: Duration = Duration::from_secs(10 * 60);
const TARGET_CACHE_LIMIT: usize = 100;
const MAX_TARGETS: usize = 100;
const MAX_CHECK_PROMPT_TARGETS: usize = 100;
const MAX_PASSIVE_CANDIDATES: usize = 512;
const PREPARED_CACHE_LIMIT: usize = 8;
const PREPARED_CACHE_BYTES: usize = 4 * 1024 * 1024;
const TARGET_DOMAIN: &[u8] = b"antiburn/remediation-target/v2\0";
const PROMPT_REFERENCE_PREFIX: &str = "Remediation reference: ABR-";

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
    operation: ConfigOperation,
    physical_key: String,
}

struct TargetIdentity {
    group_key: String,
    target_key: String,
    canonical_identity: String,
    workspace_key: Option<String>,
    scope_kind: String,
    scope_key: String,
    physical_target_key: Option<String>,
}

struct TimedTarget {
    id: String,
    value: CachedTarget,
    created_at_epoch: i64,
}

struct PreparedOperation {
    id: String,
    target: CachedTarget,
    prepared: Option<PreparedChange>,
    retained_bytes: usize,
    created_at_epoch: i64,
    completed: Option<AutoFixResult>,
}

#[derive(Default)]
struct ControllerState {
    targets: VecDeque<TimedTarget>,
    prepared: VecDeque<PreparedOperation>,
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
            let display = finding
                .finding
                .display()
                .map_err(|_| ControllerError::Internal)?;
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
            let display_facts = burn_check_display_facts(&target);
            let watch = store
                .latest_remediation_for_target(
                    &target.findings[0].environment_key,
                    target.agent.slug(),
                    &target.target_key,
                )
                .map_err(|_| ControllerError::Internal)?
                .as_ref()
                .map(|watch| public_watch(store, watch))
                .transpose()?
                .flatten();
            let auto_fix = match (&target.config, watch.as_ref()) {
                (Some(_), Some(watch))
                    if watch.lifecycle != RemediationState::Recurred
                        && !(watch.lifecycle == RemediationState::Watching
                            && watch.origin == RemediationOrigin::Passive) =>
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
                finding_id: stable_finding_id(&target),
                action_id: id.clone(),
                finding: display,
                display: display_facts,
                occurrences: target.findings.len(),
                auto_fix,
                prompt_fix: match remediation_prompt(&target.findings[0].finding) {
                    Ok(_) => PromptFixAvailability::Available,
                    Err(reason) => PromptFixAvailability::Unavailable(reason),
                },
                watch,
                coverage_limits: vec![CoverageLimit::CurrentPublishedEvidenceOnly],
                sample_sessions: sample_sessions(&target.findings),
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

    #[cfg(all(test, not(windows)))]
    pub(crate) fn list_burn_check_targets_with_home(
        &self,
        store: &Store,
        detector: DetectorId,
        context: BurnCheckTargetContext,
        home: &Path,
    ) -> Result<BurnCheckTargetList, ControllerError> {
        self.list_burn_check_targets_at(store, detector, context, now_epoch(), Some(home))
    }

    pub fn copy_prompt_fix_burn_check_target(
        &self,
        store: &Store,
        action_id: &str,
    ) -> Result<PromptFixResult, ControllerError> {
        let now = now_epoch();
        let target = self.cached_target(action_id, now)?;
        self.revalidate(&target)?;
        let base_prompt = remediation_prompt(&target.findings[0].finding)
            .map_err(ControllerError::PromptUnavailable)?
            .into_string();
        let paths = representative_paths(
            store,
            target.findings.iter().map(|finding| {
                SessionKey::new(
                    finding.environment_key.as_str(),
                    finding.agent.as_str(),
                    finding.session_id.as_str(),
                )
            }),
        )?;
        let watch = self.start_watch(
            store,
            &target,
            RemediationState::Watching,
            Some(now.saturating_mul(1_000)),
            now,
        )?;
        let action_at_ms = watch
            .effective_boundary_ms
            .unwrap_or_default()
            .max(now.saturating_mul(1_000));
        self.persist_display_snapshot(store, &watch, &target, "action", action_at_ms)?;
        store
            .mark_remediation_action_joined(&watch.remediation_id, action_at_ms)
            .map_err(|_| ControllerError::PersistenceFailed)?;
        let prompt = prompt_with_evidence_paths(&base_prompt, &paths, Some(&watch.remediation_id))
            .map_err(ControllerError::PromptUnavailable)?;
        Ok(PromptFixResult {
            prompt,
            watch: public_watch(store, &watch)?.ok_or(ControllerError::PersistenceFailed)?,
        })
    }

    pub fn copy_prompt_fix_burn_check(
        &self,
        store: &Store,
        detector: DetectorId,
        context: BurnCheckTargetContext,
    ) -> Result<CheckPromptFixResult, ControllerError> {
        let report = insights_report::reduce_report_blocking(
            &self.data_dir,
            insights_report::ReportRequest {
                environment_key: context.environment_key.clone(),
                window: context.window,
                computed_at_epoch: context.window.end_epoch,
            },
        )
        .map_err(|_| ControllerError::Internal)?;
        let antiburn_local::insights::DetectorStatus::Findings(findings) =
            &report.report.detector_statuses[detector.index()]
        else {
            return Err(ControllerError::CheckPromptUnavailable);
        };
        if findings.finding_sessions == 0 {
            return Err(ControllerError::CheckPromptUnavailable);
        }
        let current = insights_report::list_current_findings(
            &self.data_dir,
            CurrentFindingsRequest {
                environment_key: context.environment_key.clone(),
                window: context.window,
                detector,
            },
        )
        .map_err(|_| ControllerError::Internal)?;
        if !current.findings.is_empty() {
            return Err(ControllerError::CheckPromptUnavailable);
        }
        let paths = representative_paths(
            store,
            findings.examples.iter().map(|example| {
                SessionKey::new(
                    context.environment_key.as_str(),
                    example.agent.as_str(),
                    example.session_id.as_str(),
                )
            }),
        )?;
        let base = fallback_remediation_prompt(detector)
            .map_err(ControllerError::PromptUnavailable)?
            .into_string();
        let prompt = prompt_with_evidence_paths(&base, &paths, None)
            .map_err(ControllerError::PromptUnavailable)?;
        Ok(CheckPromptFixResult { prompt })
    }

    pub fn copy_prompt_fix_burn_check_targets(
        &self,
        store: &Store,
        action_ids: &[String],
    ) -> Result<CheckPromptFixResult, ControllerError> {
        if action_ids.is_empty() || action_ids.len() > MAX_CHECK_PROMPT_TARGETS {
            return Err(ControllerError::CheckPromptUnavailable);
        }
        let now = now_epoch();
        let mut targets = Vec::with_capacity(action_ids.len());
        for action_id in action_ids {
            if action_ids.iter().filter(|id| *id == action_id).count() != 1 {
                return Err(ControllerError::CheckPromptUnavailable);
            }
            targets.push(self.cached_target(action_id, now)?);
        }
        let detector = targets[0].findings[0].finding.detector;
        if targets
            .iter()
            .any(|target| target.findings[0].finding.detector != detector)
        {
            return Err(ControllerError::CheckPromptUnavailable);
        }

        // Validate every selected identity before this action records any watch.
        for target in &targets {
            self.revalidate(target)?;
        }

        let mut sections = Vec::with_capacity(targets.len());
        for (index, target) in targets.iter().enumerate() {
            let base = remediation_prompt(&target.findings[0].finding)
                .map_err(ControllerError::PromptUnavailable)?
                .into_string();
            let paths = representative_paths(
                store,
                target.findings.iter().map(|finding| {
                    SessionKey::new(
                        finding.environment_key.as_str(),
                        finding.agent.as_str(),
                        finding.session_id.as_str(),
                    )
                }),
            )?;
            let prompt = prompt_with_evidence_paths(&base, &paths, None)
                .map_err(ControllerError::PromptUnavailable)?;
            sections.push(format!("Exact target {}\n{prompt}", index + 1));
        }
        let prompt = sections.join("\n\n");
        if prompt.len() > antiburn_local::remediation::MAX_PROMPT_BYTES {
            return Err(ControllerError::PromptUnavailable(
                RemediationUnavailableReason::PromptSizeLimit,
            ));
        }

        for target in &targets {
            let watch = self.start_watch(
                store,
                target,
                RemediationState::Watching,
                Some(now.saturating_mul(1_000)),
                now,
            )?;
            let action_at_ms = watch
                .effective_boundary_ms
                .unwrap_or_default()
                .max(now.saturating_mul(1_000));
            self.persist_display_snapshot(store, &watch, target, "action", action_at_ms)?;
            store
                .mark_remediation_action_joined(&watch.remediation_id, action_at_ms)
                .map_err(|_| ControllerError::PersistenceFailed)?;
        }
        Ok(CheckPromptFixResult { prompt })
    }

    pub fn prepare_auto_fix_burn_check_target(
        &self,
        store: &Store,
        action_id: &str,
    ) -> Result<AutoFixReview, ControllerError> {
        self.prepare_auto_fix_at(store, action_id, now_epoch())
    }

    fn prepare_auto_fix_at(
        &self,
        store: &Store,
        action_id: &str,
        now: i64,
    ) -> Result<AutoFixReview, ControllerError> {
        let target = self.cached_target(action_id, now)?;
        self.revalidate(&target)?;
        if let Some(watch) = store
            .latest_remediation_for_target(
                &target.findings[0].environment_key,
                target.agent.slug(),
                &target.target_key,
            )
            .map_err(|_| ControllerError::Internal)?
            .as_ref()
            .map(|watch| public_watch(store, watch))
            .transpose()?
            .flatten()
            && watch.lifecycle != RemediationState::Recurred
            && !(watch.lifecycle == RemediationState::Watching
                && watch.origin == RemediationOrigin::Passive)
        {
            return Err(ControllerError::AutoFixUnavailable(
                AutoFixUnavailableReason::ActiveWatch,
            ));
        }
        let config = target.config.as_ref().ok_or({
            ControllerError::AutoFixUnavailable(
                AutoFixUnavailableReason::UnsupportedOrUnprovenTarget,
            )
        })?;
        let context = refreshed_config_context(&config.context);
        let prepared = self
            .editor
            .prepare_operation(&context, &config.operation)
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
        let selector_qualifier =
            physical_selector_qualifier(&self.editor, &context, prepared.physical_identity().1);
        if physical_key(
            store,
            target.agent,
            prepared.physical_identity(),
            selector_qualifier.as_deref(),
        )
        .map_err(|_| ControllerError::Internal)?
            != config.physical_key
            || prepared.scope()
                != scope_from_name(&target.scope_kind).ok_or(ControllerError::TargetChanged)?
        {
            return Err(ControllerError::TargetChanged);
        }
        let prepared_operation_id = random_id().map_err(|_| ControllerError::Internal)?;
        let retained_bytes = prepared.retained_bytes();
        if retained_bytes > PREPARED_CACHE_BYTES {
            return Err(ControllerError::AutoFixUnavailable(
                AutoFixUnavailableReason::SafetyCheckFailed,
            ));
        }
        let review = AutoFixReview {
            prepared_operation_id: prepared_operation_id.clone(),
            expires_at_epoch: now.saturating_add(ID_TTL.as_secs() as i64),
            agent: target.agent,
            scope: scope_display(&target.scope_kind),
            setting: match config.operation.setting {
                ConfigSetting::Model => AutoFixSetting::Model,
                ConfigSetting::Reasoning => AutoFixSetting::Reasoning,
            },
            config_file: display_config_file(prepared.physical_identity().0, &context.home_root),
            current_value: config.operation.expected_value.clone(),
            proposed_value: config.operation.proposed_value.clone(),
            effect: match config.operation.setting {
                ConfigSetting::Model => AutoFixEffect::FutureModelSelection,
                ConfigSetting::Reasoning => AutoFixEffect::FutureReasoningEffort,
            },
            side_effect: match config.operation.setting {
                ConfigSetting::Model => AutoFixSideEffect::ModelBehaviorMayChange,
                ConfigSetting::Reasoning => AutoFixSideEffect::ResponsesMayUseLessReasoning,
            },
        };
        let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
        prune_prepared(&mut state, now);
        while state.prepared.len() >= PREPARED_CACHE_LIMIT
            || prepared_retained_bytes(&state).saturating_add(retained_bytes) > PREPARED_CACHE_BYTES
        {
            if state.prepared.pop_front().is_none() {
                return Err(ControllerError::AutoFixUnavailable(
                    AutoFixUnavailableReason::SafetyCheckFailed,
                ));
            }
        }
        state.prepared.push_back(PreparedOperation {
            id: prepared_operation_id,
            target,
            prepared: Some(prepared),
            retained_bytes,
            created_at_epoch: now,
            completed: None,
        });
        Ok(review)
    }

    pub fn apply_prepared_burn_check_operation(
        &self,
        store: &Store,
        prepared_operation_id: &str,
    ) -> Result<AutoFixResult, ControllerError> {
        self.apply_prepared_at(store, prepared_operation_id, now_epoch())
    }

    fn apply_prepared_at(
        &self,
        store: &Store,
        prepared_operation_id: &str,
        now: i64,
    ) -> Result<AutoFixResult, ControllerError> {
        let (target, prepared) = {
            let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
            let index = state
                .prepared
                .iter()
                .position(|entry| entry.id == prepared_operation_id)
                .ok_or(ControllerError::TargetNotFound)?;
            if now.saturating_sub(state.prepared[index].created_at_epoch) > ID_TTL.as_secs() as i64
            {
                state.prepared.remove(index);
                return Err(ControllerError::TargetExpired);
            }
            let entry = &mut state.prepared[index];
            if let Some(result) = &entry.completed {
                return Ok(result.clone());
            }
            let prepared = entry.prepared.take().ok_or(ControllerError::Conflict)?;
            (entry.target.clone(), prepared)
        };
        if let Err(error) = self.revalidate_prepared(store, &target, &prepared) {
            self.remove_prepared(prepared_operation_id);
            return Err(error);
        }
        let watch = match self.start_watch(store, &target, RemediationState::Reserved, None, now) {
            Ok(watch) => watch,
            Err(error) => {
                self.remove_prepared(prepared_operation_id);
                return Err(error);
            }
        };
        if watch.state != RemediationState::Reserved {
            self.remove_prepared(prepared_operation_id);
            return Err(ControllerError::AutoFixUnavailable(
                AutoFixUnavailableReason::ActiveWatch,
            ));
        }
        if let Err(error) = self.persist_display_snapshot(
            store,
            &watch,
            &target,
            "action",
            now.saturating_mul(1_000),
        ) {
            let _ = store.cancel_remediation_reservation(&watch.remediation_id);
            self.remove_prepared(prepared_operation_id);
            return Err(error);
        }
        if !store
            .begin_remediation_write(&watch.remediation_id, now)
            .map_err(|_| ControllerError::PersistenceFailed)?
        {
            let _ = store.cancel_remediation_reservation(&watch.remediation_id);
            return Err(ControllerError::PersistenceFailed);
        }
        let result = (|| match apply_prepared_change(&self.editor, &prepared) {
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
        })();
        match &result {
            Ok(completed) => {
                let mut state = self.state.lock().map_err(|_| ControllerError::Internal)?;
                if let Some(entry) = state
                    .prepared
                    .iter_mut()
                    .find(|entry| entry.id == prepared_operation_id)
                {
                    entry.completed = Some(completed.clone());
                    entry.retained_bytes = 0;
                }
            }
            Err(_) => self.remove_prepared(prepared_operation_id),
        }
        result
    }

    pub fn aggregate_wins(&self, store: &Store) -> Result<AggregateWins, ControllerError> {
        let rows = store
            .remediation_contributions(1_000)
            .map_err(|_| ControllerError::PersistenceFailed)?;
        let wins = rows
            .into_iter()
            .map(|row| {
                let snapshot: StoredDisplaySnapshot =
                    serde_json::from_str(&row.display_snapshot_json)
                        .map_err(|_| ControllerError::Internal)?;
                let savings: AggregateSavings =
                    serde_json::from_str(&row.facts_json).map_err(|_| ControllerError::Internal)?;
                if snapshot.version != 1 || savings.version != 1 {
                    return Err(ControllerError::Internal);
                }
                let detector = DetectorId::ALL
                    .into_iter()
                    .find(|detector| detector.key() == row.detector_id)
                    .ok_or(ControllerError::Internal)?;
                Ok(AggregateWin {
                    finding_id: snapshot.finding_id,
                    detector,
                    origin: row.origin,
                    display: snapshot.display,
                    savings,
                    starts_at_ms: row.starts_at_ms,
                    ends_at_ms: row.ends_at_ms,
                })
            })
            .collect::<Result<Vec<_>, ControllerError>>()?;
        Ok(AggregateWins { wins })
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
        let secret = store
            .provider_account_secret()
            .map_err(|_| ControllerError::Internal)?;
        let identity = target_identity(&secret, &finding, project_root.as_deref());
        let mut config = None;
        if let (Some(home), Some(operation)) = (
            home,
            reviewed_config_operation(agent, finding.finding.cause()),
        ) && automatic_editor_supported(
            agent,
            operation.setting,
            finding.finding.source_format,
            &identity.scope_kind,
            &finding.environment_key,
            current_editor_platform(),
        ) && (finding.workspace_candidate().is_none() || project_root.is_some())
            && workspace_precedence_supported(
                agent,
                finding.workspace_candidate(),
                project_root.as_deref(),
            )
        {
            let Some(mut context) = config_context(
                agent,
                home,
                finding.workspace_candidate(),
                project_root.as_deref(),
            ) else {
                return Ok((
                    identity.group_key,
                    CachedTarget {
                        findings: vec![finding],
                        target_key: identity.target_key,
                        canonical_identity: identity.canonical_identity,
                        workspace_key: identity.workspace_key,
                        agent,
                        scope_kind: identity.scope_kind,
                        scope_key: identity.scope_key,
                        physical_target_key: identity.physical_target_key,
                        config: None,
                    },
                ));
            };
            context.runtime_override_present = runtime_override_present(agent);
            context.managed_configuration_present = managed_configuration_present(agent, home);
            if let Ok(effective) = self.editor.effective(&context, operation.setting)
                && effective.value == operation.expected_value
            {
                let selector_qualifier = physical_selector_qualifier(
                    &self.editor,
                    &context,
                    effective.physical_identity().1,
                );
                let key = physical_key(
                    store,
                    agent,
                    effective.physical_identity(),
                    selector_qualifier.as_deref(),
                )
                .map_err(|_| ControllerError::Internal)?;
                if identity.physical_target_key.as_deref() == Some(key.as_str())
                    && identity.scope_kind == scope_name(effective.scope)
                {
                    config = Some(CachedConfig {
                        context,
                        operation,
                        physical_key: key,
                    });
                }
            }
        }
        Ok((
            identity.group_key,
            CachedTarget {
                findings: vec![finding],
                target_key: identity.target_key,
                canonical_identity: identity.canonical_identity,
                workspace_key: identity.workspace_key,
                agent,
                scope_kind: identity.scope_kind,
                scope_key: identity.scope_key,
                physical_target_key: identity.physical_target_key,
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

    fn revalidate_prepared(
        &self,
        store: &Store,
        target: &CachedTarget,
        prepared: &PreparedChange,
    ) -> Result<(), ControllerError> {
        self.revalidate(target)?;
        let config = target
            .config
            .as_ref()
            .ok_or(ControllerError::TargetChanged)?;
        let effective = self
            .editor
            .effective(
                &refreshed_config_context(&config.context),
                config.operation.setting,
            )
            .map_err(|_| ControllerError::Conflict)?;
        let selector_qualifier = physical_selector_qualifier(
            &self.editor,
            &config.context,
            effective.physical_identity().1,
        );
        let current_key = physical_key(
            store,
            target.agent,
            effective.physical_identity(),
            selector_qualifier.as_deref(),
        )
        .map_err(|_| ControllerError::Internal)?;
        if effective.value != config.operation.expected_value
            || effective.setting != prepared.setting()
            || effective.scope != prepared.scope()
            || effective.scope
                != scope_from_name(&target.scope_kind).ok_or(ControllerError::TargetChanged)?
            || current_key != config.physical_key
            || physical_key(
                store,
                target.agent,
                prepared.physical_identity(),
                selector_qualifier.as_deref(),
            )
            .map_err(|_| ControllerError::Internal)?
                != config.physical_key
        {
            return Err(ControllerError::Conflict);
        }
        Ok(())
    }

    fn persist_display_snapshot(
        &self,
        store: &Store,
        watch: &RemediationRecord,
        target: &CachedTarget,
        origin: &str,
        fallback_boundary_ms: i64,
    ) -> Result<(), ControllerError> {
        let current = store
            .remediation(&watch.remediation_id)
            .map_err(|_| ControllerError::PersistenceFailed)?
            .ok_or(ControllerError::PersistenceFailed)?;
        validate_envelope_version(&current.result_json, "remediation result")
            .map_err(|_| ControllerError::Internal)?;
        let reservation: serde_json::Value =
            serde_json::from_str(&current.result_json).map_err(|_| ControllerError::Internal)?;
        let boundary = current
            .effective_boundary_ms
            .or_else(|| {
                reservation
                    .get("priorBoundaryMs")
                    .and_then(serde_json::Value::as_i64)
            })
            .unwrap_or(fallback_boundary_ms);
        let snapshot = StoredDisplaySnapshot {
            version: 1,
            finding_id: stable_finding_id(target),
            display: burn_check_display_facts(target),
        };
        let existing = store
            .remediation_display_snapshot(&current.remediation_id)
            .map_err(|_| ControllerError::PersistenceFailed)?;
        let saved = RemediationDisplaySnapshot {
            remediation_id: current.remediation_id,
            origin: existing
                .as_ref()
                .map(|snapshot| snapshot.origin.clone())
                .or_else(|| {
                    reservation
                        .get("priorOrigin")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| origin.to_owned()),
            display_snapshot_json: reservation
                .get("priorDisplaySnapshot")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .unwrap_or(
                    serde_json::to_string(&snapshot).map_err(|_| ControllerError::Internal)?,
                ),
            effective_boundary_ms: boundary,
            verified_boundary_ms: existing
                .as_ref()
                .and_then(|snapshot| snapshot.verified_boundary_ms)
                .or_else(|| {
                    reservation
                        .get("priorVerifiedBoundaryMs")
                        .and_then(serde_json::Value::as_i64)
                }),
            recurred_boundary_ms: existing
                .as_ref()
                .and_then(|snapshot| snapshot.recurred_boundary_ms)
                .or_else(|| {
                    reservation
                        .get("priorRecurredBoundaryMs")
                        .and_then(serde_json::Value::as_i64)
                }),
        };
        if !store
            .upsert_remediation_display_snapshot(&saved)
            .map_err(|_| ControllerError::PersistenceFailed)?
        {
            return Err(ControllerError::PersistenceFailed);
        }
        Ok(())
    }

    fn remove_prepared(&self, id: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.prepared.retain(|entry| entry.id != id);
        }
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
        } else if !watch_verification_available(
            &definition,
            &target.scope_kind,
            target.agent.slug(),
            target.findings[0].finding.detector,
        ) {
            json!({"version": 1, "verification": {"status": "verificationUnavailable"}, "savings": {"status": "unavailable"}})
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

fn display_config_file(path: &Path, home: &Path) -> String {
    path.strip_prefix(home)
        .map(|relative| format!("~/{}", relative.display()))
        .unwrap_or_else(|_| path.display().to_string())
}

const fn remediation_policy_is_current(definition: &WatchDefinition) -> bool {
    definition.version == 1
        && matches!(definition.remediation_policy_revision, Some(value) if value == REMEDIATION_POLICY_REVISION)
        && definition.verification_method_revision == VERIFICATION_METHOD_REVISION
        && definition.savings_method_revision == SAVINGS_METHOD_REVISION
}

fn watch_verification_available(
    definition: &WatchDefinition,
    scope_kind: &str,
    agent: &str,
    detector: DetectorId,
) -> bool {
    if !matches!(scope_kind, "global" | "project")
        || definition.detector != detector.key()
        || definition.resource.is_some()
        || !verification_evidence_supported(detector, definition.source_format.value())
        || !verification_source_matches_agent(agent, definition.source_format.value())
    {
        return false;
    }
    match detector {
        DetectorId::OldModelUsage => {
            definition.old_model.is_some()
                && definition.replacement.is_some()
                && definition.physical_target_key.is_some()
                && definition.config_setting.as_deref() == Some("model")
        }
        DetectorId::ModelOverthinking | DetectorId::OveruseOfFastMode => {
            definition.resource.is_none()
                && definition.target_model.is_some()
                && definition.target_control.is_some()
        }
        _ => false,
    }
}

fn verification_source_matches_agent(agent: &str, source_format: SourceFormat) -> bool {
    matches!(
        (agent, source_format),
        ("claude-code", SourceFormat::ClaudeJsonl)
            | ("codex", SourceFormat::CodexRolloutJsonl)
            | (
                "opencode",
                SourceFormat::OpenCodeJsonl | SourceFormat::OpenCodeSqliteV2
            )
            | ("pi", SourceFormat::PiV3Jsonl)
    )
}

fn prune_prepared(state: &mut ControllerState, now: i64) {
    state
        .prepared
        .retain(|entry| now.saturating_sub(entry.created_at_epoch) <= ID_TTL.as_secs() as i64);
}

fn prepared_retained_bytes(state: &ControllerState) -> usize {
    state
        .prepared
        .iter()
        .map(|entry| entry.retained_bytes)
        .fold(0, usize::saturating_add)
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
mod tests;
