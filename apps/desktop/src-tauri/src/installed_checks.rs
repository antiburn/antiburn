//! Desktop lifetime for Burn Check definitions. Core owns execution policy.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use antiburn_checks::check_store::{AttemptFailure, CheckStore, StoreError};
use antiburn_checks::definition_lifecycle::{
    DefinitionLifecycle, EffectiveActivation, LifecycleState,
};
use antiburn_checks::diagnostics::DiagnosticCode;
use antiburn_checks::discovery::{
    CheckFile, DefinitionScope, DefinitionState, DiscoveryConfig, EffectiveDefinition,
    discover_definitions,
};
use antiburn_checks::domain::{ActivationScope, ObservationPosition, RunScope};
use antiburn_checks::engine::{
    CheckEngine, DecisionEvaluator, DecisionFuture, EvaluationFailure, EvaluationFailureCode,
    RunOutcome, RunSettlement, StartOutcome,
};
use antiburn_checks::source::FileEventSource;
use antiburn_local::paths::protected;
use anyhow::{Context, Result, ensure};
use tauri::{Emitter, Manager};
use tokio::sync::Notify;
use tokio::task::JoinSet;

use crate::check_store::StoreCheckStore;
use crate::insights_report::installed_checks::{
    InstalledCheckAvailability, InstalledCheckDefinition, InstalledCheckEvent, InstalledCheckScope,
    InstalledCheckSource, InstalledCheckTrigger,
};
use crate::jev::config::SystemOneProvider;
use crate::jev::decision_evaluator::TypeSafeDecisionEvaluator;
use crate::published_evidence_reader::StorePublishedEvidenceReader;
use crate::store::Store;

pub(crate) mod diagnostics;
mod loader;
pub(crate) mod opener;
mod watch;

const MAX_DEFINITIONS: usize = 128;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const SOURCE_PAGE_SIZE: u16 = 64;
const SOURCE_PAGES_PER_PASS: usize = 4;
const EVALUATIONS: usize = 4;
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(30);

/// Only metadata crosses the worker boundary. Engines and credentials stay private.
#[derive(Default)]
pub(crate) struct InstalledChecks {
    metadata: RwLock<Vec<InstalledCheckDefinition>>,
    wake: Notify,
    completed: Notify,
    stopping: AtomicBool,
    credentials_restored: AtomicBool,
    delivery_ready: AtomicBool,
    requested_turn: AtomicU64,
    completed_turn: AtomicU64,
    disabled_turn: AtomicU64,
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

pub(crate) fn metadata(app: &tauri::AppHandle) -> Vec<InstalledCheckDefinition> {
    let mut definitions = app
        .try_state::<InstalledChecks>()
        .map(|state| {
            state
                .metadata
                .read()
                .expect("installed metadata lock")
                .clone()
        })
        .unwrap_or_default();
    if !crate::jev::settings::yaml_discovery_enabled(&app.state::<Store>()) {
        pause_metadata(&mut definitions);
    }
    definitions
}

fn pause_metadata(definitions: &mut [InstalledCheckDefinition]) {
    for definition in definitions {
        if definition.availability == InstalledCheckAvailability::Available {
            definition.availability = InstalledCheckAvailability::Paused {
                reason: Some("Model-based Checks are turned off in Settings → Checks.".into()),
            };
        }
    }
}

fn publish_metadata(state: &InstalledChecks, definitions: &[InstalledCheckDefinition]) -> bool {
    let mut current = state.metadata.write().expect("installed metadata lock");
    if *current == definitions {
        return false;
    }
    *current = definitions.to_vec();
    true
}

pub(crate) fn wake(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<InstalledChecks>() {
        state.requested_turn.fetch_add(1, Ordering::AcqRel);
        state.wake.notify_one();
    }
}

pub(crate) async fn settle_disable(app: &tauri::AppHandle) -> Result<()> {
    let state = app.state::<InstalledChecks>();
    let turn = state.requested_turn.fetch_add(1, Ordering::AcqRel) + 1;
    state.wake.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let completed = state.completed.notified();
            if state.disabled_turn.load(Ordering::Acquire) >= turn {
                break;
            }
            completed.await;
        }
    })
    .await
    .context("Could not finish pausing model-based checks.")
}

impl InstalledChecks {
    fn complete_turn(&self, turn: u64) -> bool {
        self.completed_turn.fetch_max(turn, Ordering::AcqRel) < turn
    }

    fn complete_disable(&self, turn: u64) -> bool {
        if self.disabled_turn.fetch_max(turn, Ordering::AcqRel) < turn {
            self.completed.notify_waiters();
        }
        self.complete_turn(turn)
    }
}

pub(crate) fn has_priority_work(app: &tauri::AppHandle) -> bool {
    app.try_state::<InstalledChecks>().is_some_and(|state| {
        state.requested_turn.load(Ordering::Acquire) > state.completed_turn.load(Ordering::Acquire)
    })
}

pub(crate) fn credentials_restored(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<InstalledChecks>() {
        state.credentials_restored.store(true, Ordering::Release);
        state.wake.notify_one();
    }
}

pub(crate) fn live_delivery_ready(app: &tauri::AppHandle) -> bool {
    app.try_state::<InstalledChecks>().is_some_and(|state| {
        state.delivery_ready.load(Ordering::Acquire)
            && crate::jev::settings::yaml_discovery_enabled(&app.state::<Store>())
            && admission(app) == Admission::Ready
    })
}

/// Stop new work and let numbered attempts settle. The final bound preserves
/// unresolved dispatch intents for Store recovery instead of permitting a resend.
pub(crate) fn shutdown(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<InstalledChecks>() else {
        return;
    };
    state.stopping.store(true, Ordering::Release);
    state.wake.notify_one();
    let task = state.task.lock().expect("installed task lock").take();
    if let Some(mut task) = task {
        tauri::async_runtime::block_on(async {
            if tokio::time::timeout(Duration::from_secs(5), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        });
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Admission {
    Ready,
    Paused(String),
    Unsupported(String),
    ProviderNotConfigured,
    AuthenticationRejected,
}

fn admission(app: &tauri::AppHandle) -> Admission {
    let worker = app.state::<crate::jev::worker::WorkerHandle>();
    if worker.system_one_connection().provider != SystemOneProvider::Jev {
        Admission::Unsupported("Burn Check definitions require the Jev connection.".into())
    } else if worker.authentication_rejected() {
        Admission::AuthenticationRejected
    } else if !worker.is_available() {
        Admission::ProviderNotConfigured
    } else {
        Admission::Ready
    }
}

struct InstalledDecisionEvaluator {
    app: tauri::AppHandle,
}

impl DecisionEvaluator for InstalledDecisionEvaluator {
    fn preparation_recorded(&self, preparation: &antiburn_checks::fit::PreparationRecord) {
        record_preparation(preparation, |outcome| {
            crate::analytics::record_burn_check_input_prepared(&self.app, outcome);
        });
    }

    fn prepare(
        &self,
        activation: &EffectiveActivation,
        input: serde_json::Value,
    ) -> std::result::Result<
        antiburn_checks::fit::PreparedDecision,
        antiburn_checks::fit::PreparationFailure,
    > {
        crate::jev::decision_evaluator::prepare_decision(
            &activation.compiled.decision,
            input,
            &antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default(),
        )
    }

    fn validate_prepared(
        &self,
        activation: &EffectiveActivation,
        prepared: &antiburn_checks::fit::PreparedDecision,
    ) -> std::result::Result<(), antiburn_checks::fit::PreparationFailure> {
        crate::jev::decision_evaluator::validate_prepared_decision(
            &activation.compiled.decision,
            prepared,
            &antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default(),
        )
    }

    fn evaluate<'a>(
        &'a self,
        activation: &'a EffectiveActivation,
        candidate: &'a antiburn_checks::domain::FrozenCandidate,
        input: &'a serde_json::Value,
        attempt_number: u8,
    ) -> DecisionFuture<'a> {
        Box::pin(async move {
            let prepared =
                self.prepare(activation, input.clone())
                    .map_err(|error| EvaluationFailure {
                        code: EvaluationFailureCode::InvalidRequest,
                        kind: AttemptFailure::Permanent,
                        detail: error.detail,
                        attempts: 0,
                    })?;
            self.evaluate_prepared(activation, candidate, &prepared, attempt_number)
                .await
        })
    }

    fn evaluate_prepared<'a>(
        &'a self,
        activation: &'a EffectiveActivation,
        candidate: &'a antiburn_checks::domain::FrozenCandidate,
        prepared: &'a antiburn_checks::fit::PreparedDecision,
        attempt_number: u8,
    ) -> DecisionFuture<'a> {
        Box::pin(async move {
            let worker = self.app.state::<crate::jev::worker::WorkerHandle>();
            let (client, generation) = worker
                .execution_client()
                .filter(|_| admission(&self.app) == Admission::Ready)
                .ok_or_else(|| EvaluationFailure {
                    code: EvaluationFailureCode::Cancelled,
                    kind: AttemptFailure::Permanent,
                    detail: "Burn Check admission changed before evaluation.".into(),
                    attempts: 0,
                })?;
            let app = self.app.clone();
            let activation_id = activation.id.clone();
            let activation_scope_key = StoreCheckStore::activation_scope_key(&activation.id.scope);
            let is_cancelled = move || {
                let worker = app.state::<crate::jev::worker::WorkerHandle>();
                let state = app.state::<InstalledChecks>();
                state.stopping.load(Ordering::Acquire)
                    || !worker.key_is_current(generation)
                    || admission(&app) != Admission::Ready
                    || !state
                        .metadata
                        .read()
                        .expect("installed metadata lock")
                        .iter()
                        .any(|definition| {
                            definition.check_id == activation_id.definition_id
                                && definition.activation_scope_key == activation_scope_key
                                && definition.generation
                                    == i64::try_from(activation_id.generation.0).ok()
                                && matches!(
                                    definition.availability,
                                    InstalledCheckAvailability::Available
                                )
                        })
            };
            if is_cancelled() {
                return Err(EvaluationFailure {
                    code: EvaluationFailureCode::Cancelled,
                    kind: AttemptFailure::Permanent,
                    detail: "Burn Check admission changed before evaluation.".into(),
                    attempts: 0,
                });
            }
            ::tracing::debug!(
                event = "installed_check_dispatch_intent",
                check_id = %activation.id.definition_id,
                generation = activation.id.generation.0,
                attempt_number,
            );
            let usage_store = self.app.state::<Store>().inner().clone();
            let rejection_app = self.app.clone();
            let rejection_store = usage_store.clone();
            let rejection_generation = generation;
            let check_id = activation.id.definition_id.clone();
            let evaluator = TypeSafeDecisionEvaluator::new(client)
                .with_cancellation(is_cancelled)
                .with_authentication_rejection(move || {
                    crate::jev::worker::settle_authentication_rejection(
                        &rejection_app,
                        &rejection_store,
                        &rejection_app.state::<crate::jev::worker::WorkerHandle>(),
                        rejection_generation,
                        true,
                    )
                    .map_err(|error| error.to_string())
                })
                .with_usage(Arc::new(move |candidate_id, attempt_number, usage| {
                    usage_store
                        .record_burn_check_decision_usage(
                            candidate_id,
                            attempt_number,
                            &check_id,
                            usage,
                            time::OffsetDateTime::now_utc().unix_timestamp(),
                        )
                        .map_err(|_| "Could not record Burn Check provider usage.".to_owned())
                }));
            evaluator
                .evaluate_prepared(activation, candidate, prepared, attempt_number)
                .await
        })
    }
}

fn record_preparation(
    preparation: &antiburn_checks::fit::PreparationRecord,
    record: impl FnOnce(crate::analytics::event::BurnCheckPreparationOutcome),
) {
    use antiburn_checks::fit::{FitStrategy, PreparationRecord};
    let Some(outcome) =
        crate::analytics::event::BurnCheckPreparationOutcome::from_record(preparation)
    else {
        return;
    };
    let report = match preparation {
        PreparationRecord::Ready(prepared) => Some(&prepared.report),
        PreparationRecord::Rejected(error) => error.report.as_ref(),
    };
    let (mut removed_items, mut removed_text_bytes) = (0_usize, 0_usize);
    for omission in report.into_iter().flat_map(|report| &report.omissions) {
        let removed = omission
            .original_count
            .saturating_sub(omission.retained_count);
        match omission.strategy {
            FitStrategy::Recent => removed_items = removed_items.saturating_add(removed),
            FitStrategy::Truncate => {
                removed_text_bytes = removed_text_bytes.saturating_add(removed)
            }
        }
    }
    ::tracing::debug!(
        event = "installed_check_input_prepared",
        outcome = outcome.as_str(),
        shortened_targets = report.map_or(0, |report| report.omissions.len()),
        removed_items,
        removed_text_bytes,
    );
    record(outcome);
}

struct ProjectChecks<E> {
    store: Arc<StoreCheckStore>,
    engine: Arc<CheckEngine<StoreCheckStore, E>>,
    active: Vec<EffectiveActivation>,
}

/// The production helper also runs in deterministic Store/source integration tests.
struct InstalledCheckHost<E> {
    store: Arc<Store>,
    evaluator: Arc<E>,
    owner: String,
    global_directory: Option<PathBuf>,
    watch_directories: Vec<PathBuf>,
    diagnostic_log: diagnostics::DiagnosticLog,
    source_states: BTreeMap<antiburn_checks::domain::ActivationId, diagnostics::SourceState>,
    projects: BTreeMap<ActivationScope, ProjectChecks<E>>,
    definitions: Vec<InstalledCheckDefinition>,
    source_turn: usize,
    evaluation_turn: usize,
}

impl<E: DecisionEvaluator + 'static> InstalledCheckHost<E> {
    fn new(
        store: Arc<Store>,
        evaluator: Arc<E>,
        global_directory: Option<PathBuf>,
    ) -> Result<Self> {
        let mut owner = [0_u8; 16];
        getrandom::fill(&mut owner).context("create installed check owner")?;
        Ok(Self {
            store,
            evaluator,
            owner: hex(&owner),
            global_directory,
            watch_directories: Vec::new(),
            diagnostic_log: diagnostics::DiagnosticLog::default(),
            source_states: BTreeMap::new(),
            projects: BTreeMap::new(),
            definitions: Vec::new(),
            source_turn: 0,
            evaluation_turn: 0,
        })
    }

    fn reconcile(&mut self, admission: &Admission) -> Result<()> {
        let scopes = live_projects(&self.store)?;
        let repositories = self
            .store
            .repositories()?
            .into_iter()
            .filter(|repository| repository.wsl_distro.is_none())
            .filter_map(|repository| {
                repository
                    .repo_root
                    .map(|root| (PathBuf::from(root), repository.enabled))
            })
            .filter(|(root, _)| root.is_absolute())
            .collect::<BTreeMap<_, _>>();
        let mut granted = self.store.granted_dirs()?;
        let mut directories = BTreeMap::new();
        let mut errors = diagnostics::DiagnosticBatch::new();
        self.watch_directories.clear();
        for (directory, enabled) in self
            .global_directory
            .iter()
            .cloned()
            .map(|path| (path, true))
            .chain(
                repositories
                    .iter()
                    .map(|(root, enabled)| (root.join(".antiburn/checks"), *enabled)),
            )
        {
            let checks = if enabled {
                loader::load_directory(&self.store, &directory, &granted)
            } else {
                loader::disabled(&directory)
            };
            granted = self.store.granted_dirs()?;
            if enabled && !protected::cwd_resolution_blocked(&directory, &granted) {
                self.watch_directories.push(directory.clone());
            }
            diagnostics::collect(&mut errors, checks.error.as_slice());
            directories.insert(directory, checks);
        }
        let mut discovered = Vec::new();
        let mut metadata = Vec::new();
        let mut definition_count = 0;
        let mut represented_roots = BTreeSet::new();
        for scope in &scopes {
            self.ensure_project(scope)?;
            let ids = self.projects[scope]
                .store
                .definition_ids()?
                .into_iter()
                .collect::<BTreeSet<_>>();
            let root = if scope.environment == "native" {
                repositories
                    .keys()
                    .filter(|root| Path::new(&scope.session_cwd).starts_with(root))
                    .max_by_key(|root| root.components().count())
                    .cloned()
            } else {
                None
            };
            represented_roots.extend(root.iter().cloned());
            let config = DiscoveryConfig {
                global_checks_directory: self.global_directory.clone(),
                project_root: root.clone(),
            };
            let files = definition_files(&config, &directories, &ids);
            let result = discover_definitions(&config, &files);
            diagnostics::collect(&mut errors, &result.diagnostics);
            append_invalid_filenames(&mut metadata, scope, &config, &result.diagnostics)?;
            let mut definitions = result.definitions;
            let scope_admission = if root.as_ref().is_some_and(|root| !repositories[root]) {
                for definition in &mut definitions {
                    definition.state = DefinitionState::Paused(Vec::new());
                }
                Admission::Paused("Repository is disabled.".into())
            } else {
                admission.clone()
            };
            definition_count += definitions.len();
            ensure!(
                definition_count <= MAX_DEFINITIONS,
                "installed checks exceed the {MAX_DEFINITIONS}-definition report limit"
            );
            discovered.push((scope.clone(), root, definitions, scope_admission));
        }
        for (scope, root, definitions, scope_admission) in discovered {
            let project = self.projects.get_mut(&scope).expect("project inserted");
            let head = project.store.publication_head()?;
            let ids = project
                .store
                .definition_ids()?
                .into_iter()
                .chain(definitions.iter().map(|definition| definition.id.clone()))
                .collect::<BTreeSet<_>>();
            project.active.clear();
            for id in ids {
                let definition = definitions.iter().find(|definition| definition.id == id);
                let effective =
                    definition.map(|definition| admitted_definition(definition, &scope_admission));
                let compiled = effective
                    .as_ref()
                    .and_then(|definition| match &definition.state {
                        DefinitionState::Ready(compiled) => Some(Arc::new((**compiled).clone())),
                        _ => None,
                    });
                let previous = project.store.load_lifecycle(&id, compiled)?;
                let transition = DefinitionLifecycle.reconcile_scoped(
                    &previous,
                    effective.as_ref(),
                    root.clone(),
                    scope.clone(),
                    ObservationPosition(0),
                    head,
                );
                let next = previous.apply(&transition);
                let active = match &next {
                    LifecycleState::Active(active) => Some(active),
                    _ => None,
                };
                project
                    .engine
                    .persist_activation(&id, &transition, active)?;
                if next != previous {
                    diagnostics::lifecycle(&id, &transition);
                }
                if let Some(active) = active {
                    project.active.push(active.clone());
                }
                if let Some(definition) = definition {
                    metadata.push(definition_metadata(
                        &scope,
                        definition,
                        &next,
                        &scope_admission,
                    )?);
                }
            }
            project.engine.recover_pending()?;
        }
        let unindexed_roots = repositories
            .keys()
            .filter(|root| !represented_roots.contains(*root))
            .cloned()
            .map(Some)
            .chain((scopes.is_empty() && repositories.is_empty()).then_some(None));
        for root in unindexed_roots {
            let config = DiscoveryConfig {
                global_checks_directory: self.global_directory.clone(),
                project_root: root.clone(),
            };
            let files = definition_files(&config, &directories, &BTreeSet::new());
            let result = discover_definitions(&config, &files);
            diagnostics::collect(&mut errors, &result.diagnostics);
            append_invalid_filenames(
                &mut metadata,
                &ActivationScope {
                    environment: "native".into(),
                    session_cwd: root
                        .as_ref()
                        .map(|root| root.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    run: RunScope::Live,
                },
                &config,
                &result.diagnostics,
            )?;
            let disabled = root.as_ref().is_some_and(|root| !repositories[root]);
            for mut definition in result.definitions {
                if disabled {
                    definition.state = DefinitionState::Paused(Vec::new());
                }
                metadata.push(definition_metadata(
                    &ActivationScope {
                        environment: "native".into(),
                        session_cwd: root
                            .as_ref()
                            .map(|root| root.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        run: RunScope::Live,
                    },
                    &definition,
                    &LifecycleState::Removed {
                        generation: antiburn_checks::domain::Generation(0),
                    },
                    &if !disabled && *admission != Admission::Ready {
                        admission.clone()
                    } else {
                        Admission::Paused(
                            if disabled {
                                "Repository is disabled."
                            } else {
                                "No sessions found. Start an agent session in this project."
                            }
                            .into(),
                        )
                    },
                )?);
            }
        }
        attach_directory_diagnostics(&mut metadata, &directories)?;
        self.diagnostic_log.publish(errors);
        ensure!(
            metadata.len() <= MAX_DEFINITIONS,
            "installed checks exceed the {MAX_DEFINITIONS}-definition report limit"
        );
        self.definitions = metadata;
        self.source_states.retain(|id, _| {
            self.projects
                .get(&id.scope)
                .is_some_and(|project| project.active.iter().any(|active| active.id == *id))
        });
        self.diagnostic_log.retain_conditions(|id| {
            self.projects
                .get(&id.scope)
                .is_some_and(|project| project.active.iter().any(|active| active.id == *id))
        });
        for project in self.projects.values() {
            for activation in &project.active {
                update_condition_diagnostic(
                    &mut self.definitions,
                    &self.diagnostic_log,
                    activation,
                );
            }
        }
        Ok(())
    }

    /// Apply a bounded number of pages. Evaluation tasks run on the async runtime
    /// while this helper catches up on a blocking worker.
    fn catch_up(&mut self) -> Result<(bool, bool)> {
        let scopes = self
            .projects
            .keys()
            .map(|scope| Ok((scope.clone(), scope_has_work(&self.store, scope)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let active = self
            .projects
            .iter()
            .filter(|(scope, _)| scopes[*scope])
            .flat_map(|(scope, project)| {
                project
                    .active
                    .iter()
                    .map(move |activation| (scope.clone(), activation.clone()))
            })
            .collect::<Vec<_>>();
        let mut changed = false;
        let mut more = false;
        for _ in 0..SOURCE_PAGES_PER_PASS.min(active.len()) {
            let (scope, activation) = &active[self.source_turn % active.len()];
            self.source_turn = self.source_turn.wrapping_add(1);
            let project = &self.projects[scope];
            let source = FileEventSource::new(
                StorePublishedEvidenceReader::for_live_project(
                    &self.store,
                    &scope.environment,
                    &scope.session_cwd,
                )
                .with_activation(&activation.id)
                .with_repository_root(
                    activation
                        .project_root
                        .as_ref()
                        .and_then(|root| root.to_str()),
                ),
                SOURCE_PAGE_SIZE,
            )
            .map_err(|error| anyhow::anyhow!("installed source: {error:?}"))?;
            let progress = project.store.load_source_progress(activation)?;
            let page = match source.read_batch(progress.publication_cursor) {
                Ok(page) => {
                    update_source_diagnostic(&mut self.definitions, activation, false);
                    page
                }
                Err(error) => {
                    ::tracing::error!(event = "installed_check_source_failed", error = ?error);
                    changed |= update_source_diagnostic(&mut self.definitions, activation, true);
                    continue;
                }
            };
            more |= page.has_more;
            if !page.events.is_empty() {
                let upserts = page
                    .events
                    .iter()
                    .filter(|event| matches!(event, antiburn_checks::source::SourceEvent::Observation { observation, .. } if matches!(&observation.kind, antiburn_checks::domain::ObservationKind::ToolResult { .. })))
                    .count();
                let gaps = page
                    .events
                    .iter()
                    .filter(|event| {
                        matches!(event, antiburn_checks::source::SourceEvent::Gap { .. })
                    })
                    .count();
                let outcome = project.engine.apply_source_page(activation, &page)?;
                self.diagnostic_log.publish_conditions(activation, &outcome);
                update_condition_diagnostic(
                    &mut self.definitions,
                    &self.diagnostic_log,
                    activation,
                );
                let window = activation.compiled.trigger.window;
                ::tracing::debug!(
                    event = "installed_check_source_page",
                    check_id = %activation.id.definition_id,
                    generation = activation.id.generation.0,
                    source_events = page.events.len(),
                    tool_results = upserts,
                    coverage_gaps = gaps,
                    candidates = outcome.candidates,
                    coalesced = outcome.coalesced,
                    overflowed = outcome.overflowed,
                    window_size = window.map_or(1, |window| window.size),
                    window_minimum = window.map_or(1, |window| window.minimum),
                    has_more = page.has_more,
                );
                if upserts > 0 && outcome.candidates == 0 {
                    ::tracing::debug!(
                        event = "installed_check_no_candidate",
                        check_id = %activation.id.definition_id,
                        generation = activation.id.generation.0,
                        tool_results = upserts,
                        reason = if gaps > 0 { "partial_source_coverage" } else { "window_or_filter_not_ready" },
                    );
                }
                changed = true;
            }
            for session in project.store.startup_sessions(activation, active_since())? {
                let events = StorePublishedEvidenceReader::for_project(
                    &self.store,
                    &scope.environment,
                    &scope.session_cwd,
                )
                .with_repository_root(
                    activation
                        .project_root
                        .as_ref()
                        .and_then(|root| root.to_str()),
                )
                .recent_results(&session.key)
                .map_err(|error| anyhow::anyhow!("startup source: {error:?}"))?;
                if let Some(outcome) = project
                    .store
                    .initialize_recent(activation, &session, &events)?
                {
                    self.diagnostic_log.publish_conditions(activation, &outcome);
                    update_condition_diagnostic(
                        &mut self.definitions,
                        &self.diagnostic_log,
                        activation,
                    );
                    project.engine.refresh_source_progress(activation)?;
                    changed = true;
                }
            }
            project.engine.recover_pending()?;
            let progress = project.store.load_source_progress(activation)?;
            let current = diagnostics::SourceState::read(&self.store, activation, &progress)?;
            if self.source_states.get(&activation.id) != Some(&current) {
                current.publish(activation);
                self.source_states.insert(activation.id.clone(), current);
            }
        }
        Ok((changed, more))
    }

    fn evaluation_jobs(&mut self, slots: usize) -> Vec<EvaluationJob<E>> {
        let active = self
            .projects
            .values()
            .flat_map(|project| {
                project.active.iter().map(move |activation| EvaluationJob {
                    engine: Arc::clone(&project.engine),
                    store: Arc::clone(&project.store),
                    activation: activation.clone(),
                })
            })
            .collect::<Vec<_>>();
        if active.is_empty() {
            return Vec::new();
        }
        (0..slots)
            .map(|_| {
                let job = &active[self.evaluation_turn % active.len()];
                self.evaluation_turn = self.evaluation_turn.wrapping_add(1);
                EvaluationJob {
                    engine: Arc::clone(&job.engine),
                    store: Arc::clone(&job.store),
                    activation: job.activation.clone(),
                }
            })
            .collect()
    }

    fn ensure_project(&mut self, scope: &ActivationScope) -> Result<()> {
        if !self.projects.contains_key(scope) {
            let store = Arc::new(StoreCheckStore::new(
                Arc::clone(&self.store),
                self.owner.clone(),
                scope.clone(),
            )?);
            let engine = Arc::new(CheckEngine::new(
                Arc::clone(&store),
                Arc::clone(&self.evaluator),
            ));
            self.projects.insert(
                scope.clone(),
                ProjectChecks {
                    store,
                    engine,
                    active: Vec::new(),
                },
            );
        }
        Ok(())
    }

    fn deactivate_active(&mut self) -> Result<()> {
        for scope in live_projects(&self.store)? {
            self.ensure_project(&scope)?;
            let project = self.projects.get_mut(&scope).expect("project inserted");
            for (id, transition) in project.store.active_pause_transitions()? {
                project.engine.persist_activation(&id, &transition, None)?;
                diagnostics::lifecycle(&id, &transition);
                let next = project.store.load_lifecycle(&id, None)?;
                let LifecycleState::Inactive { generation, .. } = next else {
                    anyhow::bail!("disabled activation is not paused");
                };
                if let Some(definition) = self.definitions.iter_mut().find(|definition| {
                    definition.check_id == id
                        && definition.activation_scope_key
                            == StoreCheckStore::activation_scope_key(&scope)
                }) {
                    definition.generation = Some(i64::try_from(generation.0)?);
                }
            }
            project.active.clear();
        }
        pause_metadata(&mut self.definitions);
        self.source_states.clear();
        self.diagnostic_log.retain_conditions(|_| false);
        Ok(())
    }
}

struct EvaluationJob<E> {
    engine: Arc<CheckEngine<StoreCheckStore, E>>,
    store: Arc<StoreCheckStore>,
    activation: EffectiveActivation,
}

impl<E: DecisionEvaluator> EvaluationJob<E> {
    #[cfg(test)]
    async fn run(self) -> Result<Option<RunSettlement>, StoreError> {
        self.run_with_notification(|| {}).await
    }

    async fn run_with_notification(
        self,
        on_started: impl FnOnce(),
    ) -> Result<Option<RunSettlement>, StoreError> {
        let now = u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
            .unwrap_or_default();
        let branches = self
            .store
            .initialized_branches(&self.activation)
            .map_err(|error| StoreError {
                message: error.to_string(),
            })?;
        match self
            .engine
            .start_one_due_for_branches(&self.activation, now, &branches)?
        {
            StartOutcome::Started(task) => {
                ::tracing::debug!(
                    event = "installed_check_run_started",
                    check_id = %self.activation.id.definition_id,
                    generation = self.activation.id.generation.0,
                );
                on_started();
                let result = task.await;
                match &result {
                    Ok(settlement) => ::tracing::debug!(
                        event = "installed_check_run_settled",
                        check_id = %self.activation.id.definition_id,
                        generation = self.activation.id.generation.0,
                        outcome = match &settlement.outcome {
                            RunOutcome::Accepted => "accepted",
                            RunOutcome::Deferred => "deferred",
                            RunOutcome::Stale => "stale",
                            RunOutcome::Skipped => "skipped",
                            RunOutcome::NotAssessed(_) => "not_assessed",
                            RunOutcome::Failed(_) => "failed",
                        },
                        attempts = settlement.attempts,
                    ),
                    Err(_) => ::tracing::warn!(
                        event = "installed_check_run_settlement_error",
                        check_id = %self.activation.id.definition_id,
                        generation = self.activation.id.generation.0,
                    ),
                }
                result.map(Some)
            }
            StartOutcome::Stale | StartOutcome::NotDue => Ok(None),
        }
    }
}

fn live_projects(store: &Store) -> Result<Vec<ActivationScope>> {
    let connection = store.lock();
    let mut statement = connection.prepare(
        "SELECT DISTINCT s.environment_key, s.cwd FROM session s
         WHERE s.environment_key = 'native' AND s.wsl_distro IS NULL
           AND ((s.agent IN ('claude-code', 'codex', 'pi') AND s.source_kind = 'file')
             OR (s.agent = 'opencode' AND s.source_kind = 'providerDb'))
           AND s.cwd IS NOT NULL AND s.cwd <> '' AND s.updated_at_epoch >= ?1
           AND NOT EXISTS(SELECT 1 FROM session_relation r
             WHERE r.environment_key = s.environment_key AND r.agent = s.agent
               AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
                 OR (r.related_id = s.session_id AND r.kind = 'subagent')))
         ORDER BY s.environment_key, s.cwd LIMIT 129",
    )?;
    let mut scopes = statement
        .query_map([active_since()], |row| {
            Ok(ActivationScope {
                environment: row.get(0)?,
                session_cwd: row.get(1)?,
                run: RunScope::Live,
            })
        })?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    let mut statement = connection.prepare(
        "SELECT DISTINCT activation_scope_key FROM burn_check_activation WHERE json_extract(activation_scope_key, '$.run') = 'Live' ORDER BY activation_scope_key LIMIT 129",
    )?;
    for key in statement.query_map([], |row| row.get::<_, String>(0))? {
        let scope: ActivationScope = serde_json::from_str(&key?)?;
        if scope.run == RunScope::Live && scope.environment == "native" {
            scopes.insert(scope);
        }
    }
    ensure!(
        scopes.len() <= MAX_DEFINITIONS,
        "too many installed check projects"
    );
    Ok(scopes.into_iter().collect())
}

fn active_since() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
        - antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS
}

fn scope_has_work(store: &Store, scope: &ActivationScope) -> Result<bool> {
    if scope.environment != "native" || scope.run != RunScope::Live {
        return Ok(false);
    }
    Ok(store.lock().query_row(
        "SELECT EXISTS(SELECT 1 FROM session s
           WHERE s.environment_key = ?1 AND s.cwd = ?2
             AND s.agent IN ('claude-code', 'codex', 'pi', 'opencode')
             AND s.wsl_distro IS NULL AND (s.updated_at_epoch >= ?3
               OR EXISTS(SELECT 1 FROM burn_check_candidate c
                  WHERE c.session_key_json = json_array(s.environment_key, s.agent, s.session_id)
                    AND c.activation_scope_key = ?4
                    AND c.state IN ('pending', 'claimed', 'dispatching', 'answered'))
               OR EXISTS(SELECT 1 FROM native_tool_result_publication p
                  JOIN burn_check_activation a ON a.activation_scope_key = ?4 AND a.enabled = 1
                  WHERE p.environment_key = s.environment_key AND p.agent = s.agent
                    AND p.session_id = s.session_id AND p.publication_sequence > a.applied_sequence
                    AND (p.session_incarnation IS NULL OR
                      (p.session_incarnation = s.incarnation AND p.project_cwd = s.cwd)))))",
        rusqlite::params![
            scope.environment,
            scope.session_cwd,
            active_since(),
            StoreCheckStore::activation_scope_key(scope)
        ],
        |row| row.get(0),
    )?)
}

fn definition_files(
    config: &DiscoveryConfig,
    directories: &BTreeMap<PathBuf, loader::DirectoryChecks>,
    previous_ids: &BTreeSet<String>,
) -> Vec<CheckFile> {
    let mut files = Vec::new();
    if let Some(directory) = &config.global_checks_directory {
        files.extend(directory_files(
            directory,
            &directories[directory],
            previous_ids,
        ));
    }
    if let Some(root) = &config.project_root {
        let directory = root.join(".antiburn/checks");
        let ids = previous_ids
            .iter()
            .cloned()
            .chain(
                files
                    .iter()
                    .filter_map(|file| file.path.file_stem()?.to_str().map(str::to_owned)),
            )
            .collect();
        files.extend(directory_files(&directory, &directories[&directory], &ids));
    }
    files
}

fn directory_files(
    directory: &Path,
    checks: &loader::DirectoryChecks,
    ids: &BTreeSet<String>,
) -> Vec<CheckFile> {
    match &checks.error {
        None => checks.files.clone(),
        // An incomplete listing cannot prove that a previous override was removed.
        Some(error) => ids
            .iter()
            .map(|id| CheckFile {
                path: directory.join(format!("{id}.yml")),
                content: Err(error.clone()),
                enabled: true,
            })
            .collect(),
    }
}

fn admitted_definition(
    definition: &EffectiveDefinition,
    admission: &Admission,
) -> EffectiveDefinition {
    let mut effective = definition.clone();
    if matches!(effective.state, DefinitionState::Ready(_)) && *admission != Admission::Ready {
        effective.state = DefinitionState::Paused(Vec::new());
    }
    effective
}

fn definition_metadata(
    scope: &ActivationScope,
    definition: &EffectiveDefinition,
    state: &LifecycleState,
    admission: &Admission,
) -> Result<InstalledCheckDefinition> {
    let activation_scope_key = StoreCheckStore::activation_scope_key(scope);
    let (name, description, mut availability, diagnostic) = match &definition.state {
        DefinitionState::Ready(compiled) => (
            compiled.name.clone(),
            compiled.description.clone(),
            InstalledCheckAvailability::Available,
            None,
        ),
        DefinitionState::Paused(diagnostics) => (
            definition.id.clone(),
            None,
            InstalledCheckAvailability::Paused {
                reason: Some(match admission {
                    Admission::Paused(reason) => reason.clone(),
                    _ => "This check is turned off for this project.".into(),
                }),
            },
            (!diagnostics.is_empty()).then(|| {
                diagnostics
                    .iter()
                    .take(8)
                    .map(diagnostics::SafeDiagnostic::display)
                    .collect::<Vec<_>>()
                    .join("\n")
            }),
        ),
        DefinitionState::Invalid(diagnostics) => {
            let diagnostic = diagnostics
                .iter()
                .take(8)
                .map(diagnostics::SafeDiagnostic::display)
                .collect::<Vec<_>>()
                .join("\n");
            let availability = if diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == DiagnosticCode::CompileUnsupportedFeature)
            {
                InstalledCheckAvailability::Unsupported {
                    reason: Some(diagnostic.clone()),
                }
            } else {
                InstalledCheckAvailability::Invalid {
                    reason: Some(diagnostic.clone()),
                }
            };
            (
                definition
                    .source_paths
                    .first()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| definition.id.clone()),
                None,
                availability,
                Some(diagnostic),
            )
        }
    };
    if matches!(availability, InstalledCheckAvailability::Available) {
        availability = match admission {
            Admission::Ready => availability,
            Admission::Paused(reason) => InstalledCheckAvailability::Paused {
                reason: Some(reason.clone()),
            },
            Admission::Unsupported(reason) => InstalledCheckAvailability::Unsupported {
                reason: Some(reason.clone()),
            },
            Admission::ProviderNotConfigured | Admission::AuthenticationRejected => {
                InstalledCheckAvailability::Paused { reason: None }
            }
        };
    }
    let generation = match state {
        LifecycleState::Active(active) => active.id.generation,
        LifecycleState::Inactive { generation, .. }
        | LifecycleState::Removed { generation }
        | LifecycleState::Uncompiled { generation } => *generation,
    };
    let definition_sources = definition
        .source_paths
        .iter()
        .map(|path| diagnostics::source(path))
        .collect::<Vec<_>>();
    let mut structured = match &definition.state {
        DefinitionState::Ready(_) => Vec::new(),
        DefinitionState::Invalid(errors) | DefinitionState::Paused(errors) => errors
            .iter()
            .take(8)
            .flat_map(|error| {
                let sources = if let Some(path) = &error.source_path {
                    vec![diagnostics::source(path)]
                } else {
                    definition_sources.clone()
                };
                sources
                    .into_iter()
                    .map(|source| diagnostics::definition_diagnostic(error, Some(source)))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>(),
    };
    if matches!(definition.state, DefinitionState::Ready(_)) {
        if let Admission::Unsupported(_) = admission {
            structured.push(diagnostics::payload("provider-unavailable", crate::insights_report::installed_checks::InstalledCheckDiagnosticSeverity::Error,
                "Select the Jev connection", "This check requires Jev. Select and configure the Jev connection in Settings → Checks.", definition_sources.first().cloned(), Vec::new()));
        } else if *admission == Admission::ProviderNotConfigured {
            structured.push(diagnostics::payload(
                "provider-not-configured",
                crate::insights_report::installed_checks::InstalledCheckDiagnosticSeverity::Error,
                "Configure the decision model",
                "Enable Model-based Checks and configure the Jev connection in Settings → Checks.",
                None,
                Vec::new(),
            ));
        } else if *admission == Admission::AuthenticationRejected {
            structured.push(diagnostics::payload(
                "provider-authentication",
                crate::insights_report::installed_checks::InstalledCheckDiagnosticSeverity::Error,
                "Reconnect Jev",
                "Jev rejected the saved credential. Update the API key in Settings → Checks.",
                None,
                Vec::new(),
            ));
        }
    }
    match &mut availability {
        InstalledCheckAvailability::Invalid { reason }
        | InstalledCheckAvailability::Unsupported { reason }
            if !structured.is_empty() =>
        {
            *reason = None
        }
        _ => {}
    }
    Ok(InstalledCheckDefinition {
        effective_id: serde_json::to_string(&(scope, &definition.id))?,
        check_id: definition.id.clone(),
        activation_scope_key,
        generation: if generation.0 == 0 {
            None
        } else {
            Some(i64::try_from(generation.0)?)
        },
        definition_revision: hex(&definition.content_revision),
        name,
        aliases: vec![definition.id.clone()],
        description,
        decision_name: match &definition.state {
            DefinitionState::Ready(compiled) => Some(compiled.decision.name.clone()),
            _ => None,
        },
        trigger: match &definition.state {
            DefinitionState::Ready(compiled) => Some(InstalledCheckTrigger {
                event: InstalledCheckEvent::ToolResult,
                minimum: compiled.trigger.window.map_or(1, |window| window.minimum),
            }),
            _ => None,
        },
        source: InstalledCheckSource {
            scope: match definition.scope {
                DefinitionScope::Global => InstalledCheckScope::Global,
                DefinitionScope::Project => InstalledCheckScope::Project,
            },
            path: definition
                .source_paths
                .first()
                .map(|path| path.to_string_lossy().into_owned()),
        },
        availability,
        diagnostic,
        diagnostics: structured,
        definition_sources,
    })
}

fn update_condition_diagnostic(
    definitions: &mut [InstalledCheckDefinition],
    log: &diagnostics::DiagnosticLog,
    activation: &EffectiveActivation,
) {
    let scope_key = StoreCheckStore::activation_scope_key(&activation.id.scope);
    for definition in definitions.iter_mut().filter(|definition| {
        definition.check_id == activation.id.definition_id
            && definition.activation_scope_key == scope_key
    }) {
        definition.diagnostics.retain(|diagnostic| {
            !matches!(
                diagnostic.code.as_str(),
                "condition-failed" | "condition-unavailable"
            )
        });
        if let Some(diagnostic) = log.condition_diagnostic(
            &activation.id,
            definition.definition_sources.first().cloned(),
        ) {
            definition.diagnostics.push(diagnostic);
        }
    }
}

fn update_source_diagnostic(
    definitions: &mut [InstalledCheckDefinition],
    activation: &EffectiveActivation,
    failed: bool,
) -> bool {
    let scope_key = StoreCheckStore::activation_scope_key(&activation.id.scope);
    let mut changed = false;
    for definition in definitions.iter_mut().filter(|definition| {
        definition.check_id == activation.id.definition_id
            && definition.activation_scope_key == scope_key
    }) {
        let previous = definition.diagnostics.clone();
        definition
            .diagnostics
            .retain(|diagnostic| diagnostic.code != "source-read-failed");
        if failed {
            definition.diagnostics.push(diagnostics::payload("source-read-failed", crate::insights_report::installed_checks::InstalledCheckDiagnosticSeverity::Error,
                "Session evidence could not be read", "Check source access in Settings → Sources, then refresh the session index. This check cannot assess the affected data until it can be read.",
                definition.definition_sources.first().cloned(), Vec::new()));
        }
        changed |= definition.diagnostics != previous;
    }
    changed
}

fn publish_storage_failure(state: &InstalledChecks) {
    let mut definitions = state.metadata.write().expect("installed metadata lock");
    for definition in definitions.iter_mut() {
        definition
            .diagnostics
            .retain(|diagnostic| diagnostic.code != "storage-failed");
        definition.diagnostics.push(diagnostics::payload("storage-failed", crate::insights_report::installed_checks::InstalledCheckDiagnosticSeverity::Error,
            "The check could not update local data", "Check available disk space and local data access, then refresh Checks. Earlier saved findings remain available when their evidence is valid.", None, Vec::new()));
    }
}

fn append_invalid_filenames(
    metadata: &mut Vec<InstalledCheckDefinition>,
    scope: &ActivationScope,
    config: &DiscoveryConfig,
    errors: &[antiburn_checks::diagnostics::Diagnostic],
) -> Result<()> {
    for error in errors
        .iter()
        .filter(|error| error.code == DiagnosticCode::DiscoveryInvalidId)
    {
        let Some(path) = &error.source_path else {
            continue;
        };
        let source = diagnostics::source(path);
        let check_id = format!("invalid-file:{}", source.reference);
        if metadata.iter().any(|definition| {
            definition.check_id == check_id
                && definition.activation_scope_key == StoreCheckStore::activation_scope_key(scope)
        }) {
            continue;
        }
        metadata.push(InstalledCheckDefinition {
            effective_id: serde_json::to_string(&(scope, &check_id))?,
            check_id,
            activation_scope_key: StoreCheckStore::activation_scope_key(scope),
            generation: None,
            definition_revision: String::new(),
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            aliases: Vec::new(),
            description: None,
            decision_name: None,
            trigger: None,
            source: InstalledCheckSource {
                scope: if config.global_checks_directory.as_deref() == path.parent() {
                    InstalledCheckScope::Global
                } else {
                    InstalledCheckScope::Project
                },
                path: Some(source.path.clone()),
            },
            availability: InstalledCheckAvailability::Invalid { reason: None },
            diagnostic: None,
            diagnostics: vec![diagnostics::definition_diagnostic(
                error,
                Some(source.clone()),
            )],
            definition_sources: vec![source],
        });
    }
    Ok(())
}

fn attach_directory_diagnostics(
    metadata: &mut Vec<InstalledCheckDefinition>,
    directories: &BTreeMap<PathBuf, loader::DirectoryChecks>,
) -> Result<()> {
    use crate::insights_report::installed_checks::InstalledCheckDiagnosticSeverity;
    for (directory, checks) in directories {
        if checks
            .error
            .as_ref()
            .is_some_and(|error| error.code == DiagnosticCode::DiscoveryPaused)
        {
            for definition in metadata.iter_mut().filter(|definition| {
                definition
                    .source
                    .path
                    .as_deref()
                    .is_some_and(|path| Path::new(path).parent() == Some(directory.as_path()))
            }) {
                definition.definition_sources.clear();
            }
        }
        let unavailable = checks
            .error
            .as_ref()
            .is_some_and(|error| error.code == DiagnosticCode::DiscoveryReadFailed);
        if !unavailable {
            continue;
        }
        let diagnostic = diagnostics::payload(
            "discovery-directory-unavailable",
            InstalledCheckDiagnosticSeverity::Error,
            "Allow access to the checks folder",
            "antiburn cannot read this folder. Check folder permissions and grant access in Settings → Sources if the folder is protected.",
            None,
            vec![diagnostics::detail("Folder", directory.to_string_lossy())],
        );
        let mut represented = false;
        for definition in metadata.iter_mut().filter(|definition| {
            definition
                .source
                .path
                .as_deref()
                .is_some_and(|path| Path::new(path).parent() == Some(directory.as_path()))
        }) {
            definition.definition_sources.clear();
            definition.diagnostics = vec![diagnostic.clone()];
            represented = true;
        }
        if !represented {
            let id = format!("directory:{}", diagnostics::source(directory).reference);
            let scope = ActivationScope {
                environment: "native".into(),
                session_cwd: String::new(),
                run: RunScope::Live,
            };
            metadata.push(InstalledCheckDefinition {
                effective_id: id.clone(),
                check_id: id,
                activation_scope_key: StoreCheckStore::activation_scope_key(&scope),
                generation: None,
                definition_revision: String::new(),
                name: "Checks folder".into(),
                aliases: Vec::new(),
                description: None,
                decision_name: None,
                trigger: None,
                source: InstalledCheckSource {
                    scope: InstalledCheckScope::Global,
                    path: None,
                },
                availability: InstalledCheckAvailability::Invalid { reason: None },
                diagnostic: None,
                diagnostics: vec![diagnostic],
                definition_sources: Vec::new(),
            });
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn start(app: &tauri::AppHandle) {
    let state = app.state::<InstalledChecks>();
    let mut task = state.task.lock().expect("installed task lock");
    assert!(task.is_none(), "start installed checks once");
    state.requested_turn.fetch_add(1, Ordering::AcqRel);
    let worker_app = app.clone();
    *task = Some(tauri::async_runtime::spawn(async move {
        if let Err(error) = run(&worker_app).await {
            ::tracing::error!(event = "installed_checks_stopped", error = %error);
        }
    }));
}

async fn run(app: &tauri::AppHandle) -> Result<()> {
    let state = app.state::<InstalledChecks>();
    let store = Arc::new(app.state::<Store>().inner().clone());
    let evaluator = Arc::new(InstalledDecisionEvaluator { app: app.clone() });
    let global = antiburn_local::paths::home_dir().map(|home| home.join(".antiburn/checks"));
    let mut host = InstalledCheckHost::new(store, evaluator, global)?;
    let mut tasks = JoinSet::new();
    let mut poll = tokio::time::interval(Duration::from_secs(1));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut lifecycle = app
        .state::<crate::session_lifecycle::SessionEvents>()
        .subscribe();
    let mut last_discovery = None::<Instant>;
    let mut previous_admission = None;
    let mut catch_up_more = false;
    let mut watcher: Option<watch::DirectoryWatcher> = None;
    let mut processed_turn = 0;
    loop {
        let mut discover = false;
        tokio::select! {
            () = state.wake.notified() => discover = true,
            _ = poll.tick() => {},
            () = tokio::task::yield_now(), if catch_up_more => {},
            event = lifecycle.recv() => {
                if matches!(event, Err(tokio::sync::broadcast::error::RecvError::Closed)) { break; }
                discover = true;
            },
            settled = tasks.join_next(), if !tasks.is_empty() => {
                match settled {
                    Some(Ok(Ok(Some(_)))) => { let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ()); },
                    Some(Ok(Err(error))) => {
                        ::tracing::error!(event = "installed_check_settlement_failed", error = %error);
                        publish_storage_failure(&state);
                        let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
                    },
                    Some(Err(error)) => ::tracing::error!(event = "installed_check_task_failed", error = %error),
                    _ => {},
                }
                if tasks.is_empty() && !catch_up_more {
                    state.complete_turn(processed_turn);
                    crate::jev::worker::wake(app);
                }
                continue;
            },
        }
        if state.stopping.load(Ordering::Acquire) {
            break;
        }
        if !crate::jev::settings::yaml_discovery_enabled(&app.state::<Store>()) {
            state.delivery_ready.store(false, Ordering::Release);
            host.deactivate_active()?;
            if publish_metadata(&state, &host.definitions) {
                crate::jev::settings::progress_changed(app);
                let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
            }
            if let Some(watcher) = &mut watcher {
                watcher.refresh(&[]);
            }
            catch_up_more = false;
            last_discovery = None;
            previous_admission = None;
            let requested = state.requested_turn.load(Ordering::Acquire);
            if state.complete_disable(requested) {
                crate::jev::worker::wake(app);
            }
            continue;
        }
        if !state.credentials_restored.load(Ordering::Acquire) {
            continue;
        }
        processed_turn = state.requested_turn.load(Ordering::Acquire);
        let current_admission = admission(app);
        discover |= previous_admission.as_ref() != Some(&current_admission)
            || last_discovery.is_none_or(|last| last.elapsed() >= DISCOVERY_INTERVAL);
        let work_admission = current_admission.clone();
        let (returned_host, result, reconciled) = tauri::async_runtime::spawn_blocking(move || {
            let mut reconciled = false;
            let result = (|| {
                if discover {
                    host.reconcile(&work_admission)?;
                    reconciled = true;
                }
                host.catch_up()
            })();
            (host, result, reconciled)
        })
        .await?;
        host = returned_host;
        let metadata_changed = publish_metadata(&state, &host.definitions);
        if metadata_changed {
            crate::jev::settings::progress_changed(app);
            let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
        }
        if reconciled {
            state.delivery_ready.store(
                host.projects
                    .values()
                    .any(|project| !project.active.is_empty()),
                Ordering::Release,
            );
            last_discovery = Some(Instant::now());
            previous_admission = Some(current_admission);
            if watcher.is_none() {
                let watch_app = app.clone();
                match watch::DirectoryWatcher::new(move || wake(&watch_app)) {
                    Ok(created) => watcher = Some(created),
                    Err(error) => {
                        ::tracing::error!(event = "installed_check_watch_failed", error = %error)
                    }
                }
            }
            if let Some(watcher) = &mut watcher
                && watcher.refresh(&host.watch_directories)
            {
                state.wake.notify_one();
            }
        }
        match result {
            Ok((changed, more)) => {
                if changed {
                    let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
                }
                for job in host.evaluation_jobs(EVALUATIONS.saturating_sub(tasks.len())) {
                    let report_app = app.clone();
                    tasks.spawn(job.run_with_notification(move || {
                        let _ = report_app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
                    }));
                }
                catch_up_more = more;
                if tasks.is_empty() && !more {
                    state.complete_turn(processed_turn);
                    crate::jev::worker::wake(app);
                }
            }
            Err(error) => {
                catch_up_more = false;
                if !reconciled {
                    last_discovery = None;
                }
                ::tracing::error!(event = "installed_check_update_failed", error = %error);
                publish_storage_failure(&state);
                let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
                state.complete_turn(processed_turn);
                crate::jev::worker::wake(app);
            }
        }
    }
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            ::tracing::warn!(event = "installed_check_shutdown_failed", error = %error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
