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
#[cfg(test)]
use antiburn_checks::engine::RunOutcome;
use antiburn_checks::engine::{
    CheckEngine, DecisionEvaluator, DecisionFuture, EvaluationFailure, EvaluationFailureCode,
    RunSettlement, StartOutcome,
};
use antiburn_checks::source::PublishedEvidenceSource;
use antiburn_local::analysis::jev::capabilities::ModelCapabilities;
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
use crate::store::burn_checks::trace_refs;
use execution::{AppRuntime, InstalledDecisionEvaluator, SelectedJobRuntime};

mod current_context;
pub(crate) mod diagnostics;
pub(crate) mod directories;
mod execution;
mod loader;
pub(crate) mod opener;
mod watch;

const MAX_DEFINITIONS: usize = 128;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const SOURCE_PAGE_SIZE: u16 = 64;
const SOURCE_PAGES_PER_PASS: usize = 4;
const EVALUATIONS: usize = 4;
const DISCOVERY_INTERVAL: Duration = Duration::from_secs(30);
const WORKER_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
struct IdleQuery {
    retry_at: Option<tokio::time::Instant>,
    failure: Option<diagnostics::RuntimeFailure>,
}

impl IdleQuery {
    fn run<T>(
        &mut self,
        stage: diagnostics::RuntimeStage,
        query: impl FnOnce() -> Result<T>,
    ) -> Option<T> {
        if self
            .retry_at
            .is_some_and(|retry_at| tokio::time::Instant::now() < retry_at)
        {
            return None;
        }
        match query() {
            Ok(value) => {
                self.retry_at = None;
                self.failure = None;
                Some(value)
            }
            Err(error) => {
                let failure = diagnostics::RuntimeFailure::classify(stage, &error);
                failure.trace_if_changed(&mut self.failure, "installed_check_update_failed");
                self.retry_at = Some(tokio::time::Instant::now() + WORKER_POLL_INTERVAL);
                None
            }
        }
    }
}

trait IdleQueries {
    fn next_idle_deadline(&self) -> Result<Option<i64>>;
    fn request_idle_catch_up(&self, now_ms: i64) -> Result<usize>;
}

type AssessmentTaskResult = Result<Option<RunSettlement>, StoreError>;

enum WorkerWake {
    Update {
        discover: bool,
        idle_requested: bool,
    },
    Settled(Option<Result<AssessmentTaskResult, tokio::task::JoinError>>),
    Closed,
}

struct WorkerTurns {
    tasks: JoinSet<AssessmentTaskResult>,
    poll: tokio::time::Interval,
    lifecycle: tokio::sync::broadcast::Receiver<crate::session_lifecycle::Sequenced>,
    idle_deadline: IdleQuery,
    idle_catch_up: IdleQuery,
}

impl WorkerTurns {
    fn new(
        lifecycle: tokio::sync::broadcast::Receiver<crate::session_lifecycle::Sequenced>,
    ) -> Self {
        let mut poll = tokio::time::interval(WORKER_POLL_INTERVAL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Self {
            tasks: JoinSet::new(),
            poll,
            lifecycle,
            idle_deadline: IdleQuery::default(),
            idle_catch_up: IdleQuery::default(),
        }
    }

    fn idle_failure(&self) -> Option<&diagnostics::RuntimeFailure> {
        self.idle_deadline
            .failure
            .as_ref()
            .or(self.idle_catch_up.failure.as_ref())
    }

    async fn next(
        &mut self,
        queries: &impl IdleQueries,
        state: &InstalledChecks,
        catch_up_more: bool,
        request_idle: impl FnOnce() -> bool,
    ) -> WorkerWake {
        let now_ms = time::OffsetDateTime::now_utc().unix_timestamp() * 1000;
        let idle_delay = self
            .idle_deadline
            .run(diagnostics::RuntimeStage::IdleDeadline, || {
                queries.next_idle_deadline()
            })
            .flatten()
            .map_or(Duration::from_secs(60), |deadline| {
                Duration::from_millis(
                    u64::try_from(deadline.saturating_sub(now_ms).max(1000)).unwrap_or(60_000),
                )
            });
        let discover = tokio::select! {
            () = state.wake.notified() => true,
            _ = self.poll.tick() => false,
            () = tokio::time::sleep(idle_delay) => false,
            () = tokio::task::yield_now(), if catch_up_more => false,
            event = self.lifecycle.recv() => match event {
                Ok(message) => lifecycle_changes_projects(&message.event),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return WorkerWake::Closed,
            },
            settled = self.tasks.join_next(), if !self.tasks.is_empty() => {
                return WorkerWake::Settled(settled);
            },
        };
        let idle_requested = !state.stopping.load(Ordering::Acquire)
            && request_idle()
            && self
                .idle_catch_up
                .run(diagnostics::RuntimeStage::IdleCatchUp, || {
                    queries.request_idle_catch_up(
                        time::OffsetDateTime::now_utc().unix_timestamp() * 1000,
                    )
                })
                .is_some_and(|requested| requested > 0);
        WorkerWake::Update {
            discover,
            idle_requested,
        }
    }
}

/// Only metadata crosses the worker boundary. Engines and credentials stay private.
#[derive(Default)]
pub(crate) struct InstalledChecks {
    metadata: RwLock<Vec<InstalledCheckDefinition>>,
    directories: Arc<directories::DirectoryInventory>,
    capabilities: RwLock<Option<execution::CapabilityState>>,
    wake: Notify,
    completed: Notify,
    stopping: AtomicBool,
    credentials_restored: AtomicBool,
    delivery_ready: AtomicBool,
    requested_turn: AtomicU64,
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
                reason: Some("Model-based checks are turned off in Settings → Checks.".into()),
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

fn persist_runtime_definitions(
    store: &Store,
    definitions: &[InstalledCheckDefinition],
) -> Result<()> {
    let runtime_definitions = definitions
        .iter()
        .map(|definition| antiburn_checks_store::RuntimeDefinition {
            check_id: definition.check_id.clone(),
            activation_scope_key: definition.activation_scope_key.clone(),
            generation: definition.generation,
            definition_revision: definition.definition_revision.clone(),
            name: definition.name.clone(),
            availability: match definition.availability {
                InstalledCheckAvailability::Available => {
                    antiburn_checks_store::DefinitionAvailability::Available
                }
                InstalledCheckAvailability::Paused { .. } => {
                    antiburn_checks_store::DefinitionAvailability::Paused
                }
                InstalledCheckAvailability::Invalid { .. } => {
                    antiburn_checks_store::DefinitionAvailability::Invalid
                }
                InstalledCheckAvailability::Unsupported { .. } => {
                    antiburn_checks_store::DefinitionAvailability::Unsupported
                }
            },
        })
        .collect::<Vec<_>>();
    antiburn_checks_store::replace_runtime_definitions(&mut store.lock(), &runtime_definitions)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InventoryEvent {
    Availability,
    Report,
}

struct CheckDirectoryReadFailure {
    directories: BTreeMap<PathBuf, loader::DirectoryChecks>,
}

impl std::fmt::Display for CheckDirectoryReadFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("one or more installed check folders could not be read")
    }
}

impl std::fmt::Debug for CheckDirectoryReadFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for CheckDirectoryReadFailure {}

fn publish_inventory(
    state: &InstalledChecks,
    store: &Store,
    definitions: &[InstalledCheckDefinition],
    mut emit: impl FnMut(InventoryEvent),
) -> Result<bool> {
    persist_runtime_definitions(store, definitions)?;
    let changed = publish_metadata(state, definitions);
    if changed {
        emit(InventoryEvent::Availability);
        emit(InventoryEvent::Report);
    }
    Ok(changed)
}

fn pause_saved_inventory(
    state: &InstalledChecks,
    store: &Store,
    mut emit: impl FnMut(InventoryEvent),
) -> Result<()> {
    let saved_changed = store.lock().execute(
        "UPDATE burn_check_runtime_definition SET availability = 'paused'
         WHERE availability = 'available'",
        [],
    )?;
    let mut definitions = state
        .metadata
        .read()
        .expect("installed metadata lock")
        .clone();
    for definition in &mut definitions {
        if definition.availability == InstalledCheckAvailability::Available {
            definition.availability = InstalledCheckAvailability::Paused {
                reason: Some(
                    "Installed checks could not be refreshed. Refresh Checks to try again.".into(),
                ),
            };
        }
    }
    let changed = publish_metadata(state, &definitions);
    if changed || saved_changed > 0 {
        emit(InventoryEvent::Availability);
        emit(InventoryEvent::Report);
    }
    Ok(())
}

fn finish_inventory_update<E>(
    state: &InstalledChecks,
    host: &InstalledCheckHost<E>,
    update: Result<()>,
    stage: diagnostics::RuntimeStage,
    previous_failure: &mut Option<diagnostics::RuntimeFailure>,
    mut emit: impl FnMut(InventoryEvent),
) -> bool {
    let result = update.and_then(|()| {
        ensure!(
            host.inventory_ready,
            "installed check inventory has not completed discovery"
        );
        publish_inventory(state, &host.store, &host.definitions, &mut emit)?;
        Ok(())
    });
    let Err(error) = result else {
        return true;
    };
    state.delivery_ready.store(false, Ordering::Release);
    let mut failure = diagnostics::RuntimeFailure::classify(stage, &error);
    let paused = pause_saved_inventory(state, &host.store, &mut emit).and_then(|()| {
        if let Some(failure) = error.downcast_ref::<CheckDirectoryReadFailure>() {
            // Report folder failures without replacing the saved discovery snapshot.
            let mut definitions = state
                .metadata
                .read()
                .expect("installed metadata lock")
                .clone();
            let mut folders = Vec::new();
            attach_directory_diagnostics(&mut folders, &failure.directories)?;
            let diagnostics = folders
                .iter()
                .flat_map(|folder| folder.diagnostics.clone())
                .collect::<Vec<_>>();
            if definitions.is_empty() {
                let mut folder = folders.into_iter().next().expect("unreadable check folder");
                folder.diagnostics = diagnostics;
                definitions.push(folder);
            } else {
                for definition in &mut definitions {
                    definition
                        .diagnostics
                        .retain(|diagnostic| diagnostic.code != "discovery-directory-unavailable");
                    definition.diagnostics.extend(diagnostics.iter().cloned());
                }
            }
            if publish_metadata(state, &definitions) {
                emit(InventoryEvent::Availability);
                emit(InventoryEvent::Report);
            }
        }
        Ok(())
    });
    if let Err(error) = paused {
        failure.trace_if_changed(previous_failure, "installed_check_update_failed");
        failure = diagnostics::RuntimeFailure::classify(stage, &error);
    }
    failure.trace_if_changed(previous_failure, "installed_check_update_failed");
    publish_runtime_failure(state, &failure);
    emit(InventoryEvent::Report);
    false
}

fn finish_app_inventory_update<E>(
    app: &tauri::AppHandle,
    state: &InstalledChecks,
    host: &mut InstalledCheckHost<E>,
    update: Result<()>,
    stage: diagnostics::RuntimeStage,
    previous_failure: &mut Option<diagnostics::RuntimeFailure>,
    idle_failure: Option<&diagnostics::RuntimeFailure>,
) -> bool {
    set_runtime_failure(&mut host.definitions, idle_failure);
    finish_inventory_update(
        state,
        host,
        update,
        stage,
        previous_failure,
        |event| match event {
            InventoryEvent::Availability => crate::jev::settings::progress_changed(app),
            InventoryEvent::Report => {
                let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
            }
        },
    )
}

fn update_delivery_readiness<E>(
    state: &InstalledChecks,
    host: &InstalledCheckHost<E>,
    provider_ready: bool,
) {
    // Capability retries need the source to keep producing eligible work.
    let definitions_ready = host.inventory_ready
        && host
            .projects
            .values()
            .any(|project| !project.active.is_empty());
    state.delivery_ready.store(
        state.credentials_restored.load(Ordering::Acquire)
            && live_delivery_permitted(
                &host.store,
                definitions_ready,
                state.stopping.load(Ordering::Acquire),
                provider_ready,
            ),
        Ordering::Release,
    );
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
    fn complete_disable(&self, turn: u64) {
        if self.disabled_turn.fetch_max(turn, Ordering::AcqRel) < turn {
            self.completed.notify_waiters();
        }
    }
}

pub(crate) fn credentials_restored(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<InstalledChecks>() {
        state.credentials_restored.store(true, Ordering::Release);
        state.wake.notify_one();
    }
}

pub(crate) fn live_delivery_ready(app: &tauri::AppHandle) -> bool {
    app.try_state::<InstalledChecks>().is_some_and(|state| {
        live_delivery_permitted(
            &app.state::<Store>(),
            state.delivery_ready.load(Ordering::Acquire),
            state.stopping.load(Ordering::Acquire),
            admission(app) == Admission::Ready,
        )
    })
}

pub(crate) fn live_delivery_permitted(
    store: &Store,
    definitions_ready: bool,
    stopping: bool,
    provider_ready: bool,
) -> bool {
    !stopping
        && definitions_ready
        && provider_ready
        && crate::jev::settings::yaml_discovery_enabled(store)
}

pub(crate) fn live_delivery_stopping(app: &tauri::AppHandle) -> bool {
    app.try_state::<InstalledChecks>()
        .is_some_and(|state| state.stopping.load(Ordering::Acquire))
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
    admission_for_worker(&worker)
}

fn admission_for_worker(worker: &crate::jev::worker::WorkerHandle) -> Admission {
    if worker.authentication_rejected() {
        Admission::AuthenticationRejected
    } else if !worker.is_available() {
        Admission::ProviderNotConfigured
    } else {
        Admission::Ready
    }
}

fn usage_provider(provider: SystemOneProvider) -> &'static str {
    match provider {
        SystemOneProvider::Jev => "typesafe-systemone",
        SystemOneProvider::Ollama => "ollama-systemone",
        SystemOneProvider::Cloudflare => "cloudflare-systemone",
        SystemOneProvider::Custom => "custom-systemone",
    }
}

async fn resolve_capabilities(
    snapshot: &crate::jev::worker::ExecutionSnapshot,
) -> Result<ModelCapabilities> {
    tokio::time::timeout(
        Duration::from_secs(15),
        crate::jev::config::resolve_execution_capabilities(
            &snapshot.connection,
            snapshot.credential.as_deref(),
        ),
    )
    .await
    .context("The selected connection did not finish model discovery.")?
    .context("The selected connection could not verify the model capabilities.")
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
    let reports = match preparation {
        PreparationRecord::Ready(prepared) => prepared
            .decisions
            .iter()
            .map(|block| &block.report)
            .collect::<Vec<_>>(),
        PreparationRecord::Local(_) => return,
        PreparationRecord::Rejected(error) => error.report.iter().collect(),
    };
    let (mut removed_items, mut removed_text_bytes) = (0_usize, 0_usize);
    for omission in reports.iter().flat_map(|report| &report.omissions) {
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
        shortened_targets = reports
            .iter()
            .map(|report| report.omissions.len())
            .sum::<usize>(),
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
    current_context: current_context::CurrentContextCapture,
    watch_directories: Vec<PathBuf>,
    diagnostic_log: diagnostics::DiagnosticLog,
    source_states: BTreeMap<antiburn_checks::domain::ActivationId, diagnostics::SourceState>,
    scope_work: BTreeMap<ActivationScope, bool>,
    projects: BTreeMap<ActivationScope, ProjectChecks<E>>,
    definitions: Vec<InstalledCheckDefinition>,
    inventory_ready: bool,
    source_turn: usize,
    source_sweep: Vec<antiburn_checks::domain::ActivationId>,
    source_sweep_remaining: usize,
    evaluation_turn: usize,
    capabilities: Option<ModelCapabilities>,
    capability_error: Option<String>,
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
            current_context: current_context::CurrentContextCapture::default(),
            watch_directories: Vec::new(),
            diagnostic_log: diagnostics::DiagnosticLog::default(),
            source_states: BTreeMap::new(),
            scope_work: BTreeMap::new(),
            projects: BTreeMap::new(),
            definitions: Vec::new(),
            inventory_ready: false,
            source_turn: 0,
            source_sweep: Vec::new(),
            source_sweep_remaining: 0,
            evaluation_turn: 0,
            capabilities: None,
            capability_error: None,
        })
    }

    fn reconcile(&mut self, admission: &Admission) -> Result<()> {
        self.inventory_ready = false;
        let _phase = diagnostics::SlowPhase::start("reconcile");
        let scopes = live_projects(&self.store)?;
        let repositories = native_repositories(self.store.repositories()?);
        let mut errors = diagnostics::DiagnosticBatch::new();
        let directories = self.load_directories(&repositories, &mut errors)?;
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
                    diagnostics::lifecycle(&transition);
                }
                if let Some(active) = active {
                    project.active.push(active.clone());
                }
                if let Some(definition) = definition {
                    let definition_admission = capability_admission(
                        definition,
                        &scope_admission,
                        self.capabilities.as_ref(),
                        self.capability_error.as_deref(),
                    );
                    metadata.push(definition_metadata(
                        &scope,
                        definition,
                        &next,
                        &definition_admission,
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
                update_condition_diagnostic(&mut metadata, &self.diagnostic_log, activation);
            }
        }
        persist_runtime_definitions(&self.store, &metadata)?;
        self.definitions = metadata;
        self.inventory_ready = true;
        Ok(())
    }

    fn load_directories(
        &mut self,
        repositories: &BTreeMap<PathBuf, bool>,
        errors: &mut diagnostics::DiagnosticBatch,
    ) -> Result<BTreeMap<PathBuf, loader::DirectoryChecks>> {
        ensure!(
            repositories.len() <= MAX_DEFINITIONS,
            "too many installed check projects"
        );
        let mut granted = self.store.granted_dirs()?;
        let mut directories = BTreeMap::new();
        let mut incomplete = false;
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
            // The global folder keeps its access policy when a repository shares its path.
            if directories.contains_key(&directory) {
                continue;
            }
            let checks = if enabled {
                loader::load_directory(&self.store, &directory, &granted)
            } else {
                loader::disabled(&directory)
            };
            incomplete |= checks
                .error
                .as_ref()
                .is_some_and(|error| error.code == DiagnosticCode::DiscoveryReadFailed);
            granted = self.store.granted_dirs()?;
            if enabled && !protected::cwd_resolution_blocked(&directory, &granted) {
                self.watch_directories.push(directory.clone());
            }
            diagnostics::collect(errors, checks.error.as_slice());
            directories.insert(directory, checks);
        }
        if incomplete {
            return Err(CheckDirectoryReadFailure {
                directories: directories
                    .into_iter()
                    .filter(|(_, checks)| {
                        checks
                            .error
                            .as_ref()
                            .is_some_and(|error| error.code == DiagnosticCode::DiscoveryReadFailed)
                    })
                    .collect(),
            }
            .into());
        }
        Ok(directories)
    }

    fn refresh_paused_inventory(&mut self) -> Result<()> {
        self.inventory_ready = false;
        let repositories = native_repositories(self.store.repositories()?);
        let mut errors = diagnostics::DiagnosticBatch::new();
        let directories = self.load_directories(&repositories, &mut errors)?;
        let mut scopes = live_projects(&self.store)?;
        let mut represented_roots = BTreeSet::new();
        let mut configurations = Vec::new();
        for scope in scopes.drain(..) {
            let root = repositories
                .keys()
                .filter(|root| Path::new(&scope.session_cwd).starts_with(root))
                .max_by_key(|root| root.components().count())
                .cloned();
            represented_roots.extend(root.iter().cloned());
            configurations.push((scope, root));
        }
        for root in repositories
            .keys()
            .filter(|root| !represented_roots.contains(*root))
        {
            configurations.push((
                ActivationScope {
                    environment: "native".into(),
                    session_cwd: root.to_string_lossy().into_owned(),
                    run: RunScope::Live,
                },
                Some(root.clone()),
            ));
        }
        if configurations.is_empty() {
            configurations.push((
                ActivationScope {
                    environment: "native".into(),
                    session_cwd: String::new(),
                    run: RunScope::Live,
                },
                None,
            ));
        }
        ensure!(
            configurations.len() <= MAX_DEFINITIONS,
            "too many installed check projects"
        );
        let mut metadata = Vec::new();
        for (scope, root) in configurations {
            let scope_key = StoreCheckStore::activation_scope_key(&scope);
            let generations = {
                let connection = self.store.lock();
                let mut statement = connection.prepare(
                    "SELECT check_id, generation FROM burn_check_activation
                     WHERE activation_scope_key = ?1 ORDER BY check_id LIMIT 129",
                )?;
                statement
                    .query_map([&scope_key], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
                    })?
                    .collect::<rusqlite::Result<BTreeMap<_, _>>>()?
            };
            ensure!(
                generations.len() <= MAX_DEFINITIONS,
                "too many installed check definitions"
            );
            let ids = generations
                .keys()
                .cloned()
                .chain(
                    self.definitions
                        .iter()
                        .filter(|definition| definition.activation_scope_key == scope_key)
                        .map(|definition| definition.check_id.clone()),
                )
                .collect();
            let config = DiscoveryConfig {
                global_checks_directory: self.global_directory.clone(),
                project_root: root.clone(),
            };
            let result =
                discover_definitions(&config, &definition_files(&config, &directories, &ids));
            diagnostics::collect(&mut errors, &result.diagnostics);
            append_invalid_filenames(&mut metadata, &scope, &config, &result.diagnostics)?;
            let admission = Admission::Paused(
                if root.as_ref().is_some_and(|root| !repositories[root]) {
                    "Repository is disabled."
                } else {
                    "Model-based checks are turned off in Settings → Checks."
                }
                .into(),
            );
            for definition in result.definitions {
                let state = LifecycleState::Removed {
                    generation: antiburn_checks::domain::Generation(
                        generations.get(&definition.id).copied().unwrap_or_default(),
                    ),
                };
                metadata.push(definition_metadata(
                    &scope,
                    &definition,
                    &state,
                    &admission,
                )?);
            }
            ensure!(
                metadata.len() <= MAX_DEFINITIONS,
                "installed checks exceed the {MAX_DEFINITIONS}-definition report limit"
            );
        }
        attach_directory_diagnostics(&mut metadata, &directories)?;
        ensure!(
            metadata.len() <= MAX_DEFINITIONS,
            "installed checks exceed the {MAX_DEFINITIONS}-definition report limit"
        );
        persist_runtime_definitions(&self.store, &metadata)?;
        self.definitions = metadata;
        self.inventory_ready = true;
        self.diagnostic_log.publish(errors);
        Ok(())
    }

    /// Apply a bounded number of pages. Evaluation tasks run on the async runtime
    /// while this helper catches up on a blocking worker.
    fn catch_up(&mut self) -> Result<(bool, bool)> {
        let _phase = diagnostics::SlowPhase::start("catch_up");
        let scopes = self
            .projects
            .keys()
            .map(|scope| Ok((scope.clone(), scope_has_work(&self.store, scope)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        for (scope, has_work) in &scopes {
            if self.scope_work.get(scope) == Some(has_work) {
                continue;
            }
            ::tracing::debug!(
                event = "installed_check_scope_work",
                has_work,
                reason = if *has_work {
                    "eligible_source_or_queued_work"
                } else {
                    "no_eligible_source_or_queued_work"
                }
            );
            if !has_work {
                let project = &self.projects[scope];
                for activation in &project.active {
                    let progress = project.store.load_source_progress(activation)?;
                    let current =
                        diagnostics::SourceState::read(&self.store, activation, &progress)?;
                    current.publish(activation);
                    self.source_states.insert(activation.id.clone(), current);
                }
            }
        }
        self.scope_work = scopes.clone();
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
        let sweep = active
            .iter()
            .map(|(_, activation)| activation.id.clone())
            .collect::<Vec<_>>();
        if self.source_sweep != sweep || self.source_sweep_remaining == 0 {
            self.source_sweep = sweep;
            self.source_sweep_remaining = active.len();
        }
        let mut revisit = None;
        for _ in 0..SOURCE_PAGES_PER_PASS {
            let index = if let Some(index) = revisit.take() {
                index
            } else {
                if self.source_sweep_remaining == 0 {
                    break;
                }
                self.source_sweep_remaining -= 1;
                let index = self.source_turn % active.len();
                self.source_turn = self.source_turn.wrapping_add(1);
                index
            };
            let (scope, activation) = &active[index];
            let project = &self.projects[scope];
            changed |= self.store.initialize_session_observers(
                &activation.id,
                time::OffsetDateTime::now_utc().unix_timestamp() * 1000,
            )? > 0;
            let source = PublishedEvidenceSource::new(
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
                Err(_) => {
                    let source_changed =
                        update_source_diagnostic(&mut self.definitions, activation, true);
                    if source_changed {
                        ::tracing::error!(
                            event = "installed_check_source_failed",
                            activation_ref = trace_refs::activation_ref(&activation.id),
                            reason = "publication_read_failed",
                            generation = activation.id.generation.0
                        );
                    }
                    changed |= source_changed;
                    continue;
                }
            };
            more |= page.has_more;
            if !page.events.is_empty() || page.next > progress.publication_cursor {
                let upserts = page
                    .events
                    .iter()
                    .filter(|event| matches!(event, antiburn_checks::source::SourceEvent::Observation { observation, .. } if matches!(&observation.kind, antiburn_checks::domain::ObservationKind::ToolResult { .. })))
                    .count();
                let gaps = page
                    .events
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            antiburn_checks::source::SourceEvent::Gap { .. }
                                | antiburn_checks::source::SourceEvent::SegmentReset { .. }
                                | antiburn_checks::source::SourceEvent::UnsafeUpdate { .. }
                        )
                    })
                    .count();
                let capture = &mut self.current_context;
                let outcome = project.engine.apply_source_page_with_inventories(
                    activation,
                    &page,
                    &mut |cursor, observation| {
                        capture.capture(&self.store, activation, cursor, observation)
                    },
                )?;
                self.diagnostic_log.publish_conditions(activation, &outcome);
                update_condition_diagnostic(
                    &mut self.definitions,
                    &self.diagnostic_log,
                    activation,
                );
                let window = activation.compiled.trigger.window;
                ::tracing::debug!(
                    event = "installed_check_source_page",
                    activation_ref = trace_refs::activation_ref(&activation.id),
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
                        activation_ref = trace_refs::activation_ref(&activation.id),
                        generation = activation.id.generation.0,
                        tool_results = upserts,
                        reason = if gaps > 0 {
                            "partial_source_coverage"
                        } else {
                            "window_or_filter_not_ready"
                        },
                    );
                }
                changed |= !page.events.is_empty();
            }
            for session in project.store.startup_sessions(activation, active_since())? {
                let _phase = diagnostics::SlowPhase::start("attach_session");
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
                .with_activation(&activation.id)
                .recent_results_before(&session.key, session.before_publication)
                .map_err(|error| anyhow::anyhow!("startup source: {error:?}"))?;
                if let Some(outcome) = project.store.initialize_recent_with_inventories(
                    activation,
                    &session,
                    &events,
                    &mut |cursor, observation| {
                        self.current_context
                            .capture(&self.store, activation, cursor, observation)
                    },
                )? {
                    ::tracing::debug!(
                        event = "installed_check_session_attached",
                        activation_ref = trace_refs::activation_ref(&activation.id),
                        generation = activation.id.generation.0,
                        source_events = events.len(),
                        candidates = outcome.candidates
                    );
                    self.diagnostic_log.publish_conditions(activation, &outcome);
                    update_condition_diagnostic(
                        &mut self.definitions,
                        &self.diagnostic_log,
                        activation,
                    );
                    project.engine.refresh_source_progress(activation)?;
                    revisit = Some(index);
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
        Ok((
            changed,
            more || revisit.is_some() || self.source_sweep_remaining > 0,
        ))
    }

    fn evaluation_jobs(&mut self, slots: usize) -> Vec<EvaluationJob<E>> {
        let active = self
            .projects
            .values()
            .flat_map(|project| {
                project
                    .active
                    .iter()
                    .filter(|activation| {
                        self.definitions.iter().any(|definition| {
                            definition.check_id == activation.id.definition_id
                                && definition.activation_scope_key
                                    == StoreCheckStore::activation_scope_key(&activation.id.scope)
                                && matches!(
                                    definition.availability,
                                    InstalledCheckAvailability::Available
                                        | InstalledCheckAvailability::Unsupported { .. }
                                )
                        })
                    })
                    .map(move |activation| EvaluationJob {
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
            let evaluator = Arc::clone(&self.evaluator);
            let store = Arc::new(
                StoreCheckStore::new(Arc::clone(&self.store), self.owner.clone(), scope.clone())?
                    .with_publication_gate(Arc::new(move |activation| {
                        evaluator.local_execution_is_current(activation)
                    })),
            );
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
                diagnostics::lifecycle(&transition);
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
        self.scope_work.clear();
        self.diagnostic_log.retain_conditions(|_| false);
        Ok(())
    }

    fn pause_disabled(&mut self) -> Result<()> {
        let unfinished: bool = self.store.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM burn_check_candidate WHERE state IN ('pending', 'claimed', 'dispatching', 'answered'))",
            [], |row| row.get(0),
        )?;
        if unfinished {
            // An incomplete cancellation must not reset source or accepted state.
            pause_metadata(&mut self.definitions);
            Ok(())
        } else {
            self.deactivate_active()
        }
    }

    fn next_idle_deadline(&self) -> Result<Option<i64>> {
        diagnostics::worker_result(
            diagnostics::RuntimeStage::IdleDeadline,
            self.store.next_session_idle_deadline(),
        )
    }

    fn request_idle_catch_up(&self, now_ms: i64) -> Result<usize> {
        diagnostics::worker_result(
            diagnostics::RuntimeStage::IdleCatchUp,
            self.store.request_due_session_idle_catchup(now_ms),
        )
    }
}

impl<E: DecisionEvaluator + 'static> IdleQueries for InstalledCheckHost<E> {
    fn next_idle_deadline(&self) -> Result<Option<i64>> {
        InstalledCheckHost::next_idle_deadline(self)
    }

    fn request_idle_catch_up(&self, now_ms: i64) -> Result<usize> {
        InstalledCheckHost::request_idle_catch_up(self, now_ms)
    }
}

struct EvaluationJob<E> {
    engine: Arc<CheckEngine<StoreCheckStore, E>>,
    store: Arc<StoreCheckStore>,
    activation: EffectiveActivation,
}

impl EvaluationJob<InstalledDecisionEvaluator> {
    async fn run_selected(
        self,
        runtime: Arc<dyn SelectedJobRuntime>,
    ) -> Result<Option<RunSettlement>, StoreError> {
        let store = Arc::clone(&self.store);
        let activation = self.activation.clone();
        let trace_activation = self.activation.id.clone();
        let now = diagnostics::job_result(
            &trace_activation,
            diagnostics::RuntimeStage::Clock,
            runtime.now_unix_ms(),
        )?;
        let due = crate::jev::worker::run_blocking_preparation(
            crate::jev::worker::BlockingPreparationStage::WorkProbe,
            move || store.has_due_candidate(&activation, i64::try_from(now / 1000)?),
        )
        .await;
        let due =
            diagnostics::job_result(&trace_activation, diagnostics::RuntimeStage::WorkProbe, due)?;
        if !due {
            return Ok(None);
        }
        let evaluator = match InstalledDecisionEvaluator::for_assessment(
            Arc::clone(&runtime),
            &self.activation,
        )
        .await
        {
            Ok(Some(evaluator)) => evaluator,
            Ok(None) => return Ok(None),
            Err(_) => {
                ::tracing::warn!(
                    event = "installed_check_connection_failed",
                    activation_ref = trace_refs::activation_ref(&self.activation.id),
                    generation = self.activation.id.generation.0,
                    reason = "connection_unavailable"
                );
                return Ok(None);
            }
        };
        let now = diagnostics::job_result(
            &trace_activation,
            diagnostics::RuntimeStage::Clock,
            runtime.now_unix_ms(),
        )?;
        let outcome = crate::jev::worker::run_blocking_preparation(
            crate::jev::worker::BlockingPreparationStage::PreparationStart,
            move || {
                let branches = self.store.initialized_branches(&self.activation)?;
                self.engine
                    .start_one_due_for_branches_with_evaluator(
                        &self.activation,
                        now,
                        &branches,
                        evaluator,
                    )
                    .map_err(anyhow::Error::from)
            },
        )
        .await;
        let outcome = diagnostics::job_result(
            &trace_activation,
            diagnostics::RuntimeStage::PreparationStart,
            outcome,
        )?;
        match outcome {
            StartOutcome::Started(task) => {
                runtime.report_changed();
                diagnostics::job_result(
                    &trace_activation,
                    diagnostics::RuntimeStage::AssessmentSettlement,
                    task.await.map(Some).map_err(|error| error.source.into()),
                )
            }
            StartOutcome::Stale | StartOutcome::NotDue => Ok(None),
        }
    }
}

impl<E: DecisionEvaluator> EvaluationJob<E> {
    #[cfg(test)]
    async fn run(self) -> Result<Option<RunSettlement>, StoreError> {
        self.run_with_notification(|| {}).await
    }

    #[cfg(test)]
    async fn run_with_notification(
        self,
        on_started: impl FnOnce(),
    ) -> Result<Option<RunSettlement>, StoreError> {
        let now = u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
            .unwrap_or_default();
        let branches = diagnostics::runtime_result(
            &self.activation.id,
            diagnostics::RuntimeStage::PreparationStart,
            self.store.initialized_branches(&self.activation),
        )?;
        match self
            .engine
            .start_one_due_for_branches(&self.activation, now, &branches)?
        {
            StartOutcome::Started(task) => {
                ::tracing::debug!(
                    event = "installed_check_run_started",
                    activation_ref = trace_refs::activation_ref(&self.activation.id),
                    generation = self.activation.id.generation.0,
                );
                on_started();
                let result = task.await;
                match &result {
                    Ok(settlement) => ::tracing::debug!(
                        event = "installed_check_run_settled",
                        activation_ref = trace_refs::activation_ref(&self.activation.id),
                        generation = self.activation.id.generation.0,
                        outcome = match &settlement.outcome {
                            RunOutcome::Accepted => "accepted",
                            RunOutcome::Deferred => "deferred",
                            RunOutcome::Stale => "stale",
                            RunOutcome::Skipped => "skipped",
                            RunOutcome::LimitReached => "limit_reached",
                            RunOutcome::NotAssessed(_) => "not_assessed",
                            RunOutcome::Failed(_) => "failed",
                        },
                        attempts = settlement.attempts,
                    ),
                    Err(_) => ::tracing::warn!(
                        event = "installed_check_run_settlement_error",
                        activation_ref = trace_refs::activation_ref(&self.activation.id),
                        generation = self.activation.id.generation.0,
                    ),
                }
                result.map(Some).map_err(|error| error.source)
            }
            StartOutcome::Stale | StartOutcome::NotDue => Ok(None),
        }
    }
}

fn native_repositories(
    repositories: Vec<crate::store::RepositoryRecord>,
) -> BTreeMap<PathBuf, bool> {
    repositories
        .into_iter()
        .filter_map(|repository| {
            native_repository(
                repository.repo_root,
                repository.wsl_distro,
                repository.enabled,
            )
        })
        .collect()
}

fn native_repository(
    root: Option<String>,
    wsl_distro: Option<String>,
    enabled: bool,
) -> Option<(PathBuf, bool)> {
    if wsl_distro.is_some() {
        return None;
    }
    root.map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .map(|root| (root, enabled))
}

fn live_projects(store: &Store) -> Result<Vec<ActivationScope>> {
    let _phase = diagnostics::SlowPhase::start("discover_scopes");
    let connection = store.lock();
    let mut statement = connection.prepare(
        "SELECT DISTINCT s.environment_key, s.cwd FROM session s
         WHERE s.environment_key = 'native' AND s.wsl_distro IS NULL
           AND ((s.agent IN ('claude-code', 'codex', 'pi') AND s.source_kind = 'file')
              OR (s.agent = 'opencode' AND s.source_kind = 'providerDb'
                AND s.source_label IN ('opencode:' || s.session_id,
                  'opencode:' || s.session_id || ':open_code_v2_sqlite'))
             OR (s.agent = 'cursor' AND s.source_kind = 'inline'
               AND s.source_label GLOB 'cursor-desktop:*'))
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
    let _phase = diagnostics::SlowPhase::start("probe_scope_work");
    if scope.environment != "native" || scope.run != RunScope::Live {
        return Ok(false);
    }
    let can_arrive = crate::insights_report::installed_checks::source_can_arrive_sql(5, 3);
    Ok(store.lock().query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM session s
            LEFT JOIN session_evidence e USING(environment_key, agent, session_id)
           WHERE s.environment_key = ?1 AND s.cwd = ?2
              AND s.agent IN ('claude-code', 'codex', 'pi', 'opencode', 'cursor')
              AND (s.agent <> 'cursor' OR (s.source_kind = 'inline'
                AND s.source_label GLOB 'cursor-desktop:*'))
              AND NOT EXISTS(SELECT 1 FROM session_relation r
                WHERE r.environment_key = s.environment_key AND r.agent = s.agent
                  AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
                    OR (r.related_id = s.session_id AND r.kind = 'subagent')))
              AND s.wsl_distro IS NULL AND (({can_arrive}) OR s.updated_at_epoch >= ?3
               OR EXISTS(SELECT 1 FROM burn_check_candidate c
                  WHERE c.session_key_json = json_array(s.environment_key, s.agent, s.session_id)
                    AND c.activation_scope_key = ?4
                    AND EXISTS(SELECT 1 FROM burn_check_activation a
                      WHERE a.check_id = c.check_id AND a.activation_scope_key = c.activation_scope_key
                        AND a.generation = c.generation AND a.enabled = 1)
                    AND c.state IN ('pending', 'claimed', 'dispatching', 'answered'))
               OR EXISTS(SELECT 1 FROM native_tool_result_publication p
                  JOIN burn_check_activation a ON a.activation_scope_key = ?4 AND a.enabled = 1
                  WHERE p.environment_key = s.environment_key AND p.agent = s.agent
                     AND p.session_id = s.session_id AND p.publication_sequence > a.applied_sequence
                     AND (p.target_check_id IS NULL OR (p.target_check_id = a.check_id
                       AND p.target_scope_key = a.activation_scope_key AND p.target_generation = a.generation
                       AND EXISTS(SELECT 1 FROM burn_check_session_attachment t
                         WHERE t.check_id = a.check_id AND t.activation_scope_key = a.activation_scope_key
                           AND t.generation = a.generation AND t.branch_key = p.target_branch_key
                           AND t.incarnation = p.target_incarnation AND t.observer_epoch = p.target_attachment_epoch)))
                    AND (p.session_incarnation IS NULL OR
                       (p.session_incarnation = s.incarnation AND p.project_cwd = s.cwd)))))"),
        rusqlite::params![
            scope.environment,
            scope.session_cwd,
            active_since(),
            StoreCheckStore::activation_scope_key(scope),
            antiburn_local::analysis::PARSER_REVISION
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
        if config.global_checks_directory.as_ref() == Some(&directory) {
            return files;
        }
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
    if matches!(effective.state, DefinitionState::Ready(_))
        && matches!(admission, Admission::Paused(_))
    {
        effective.state = DefinitionState::Paused(Vec::new());
    }
    effective
}

fn capability_admission(
    definition: &EffectiveDefinition,
    admission: &Admission,
    capabilities: Option<&ModelCapabilities>,
    capability_error: Option<&str>,
) -> Admission {
    let DefinitionState::Ready(compiled) = &definition.state else {
        return admission.clone();
    };
    if *admission != Admission::Ready {
        return admission.clone();
    }
    if compiled.is_local() {
        return Admission::Ready;
    }
    let requirements = compiled.question_requirements();
    if let Some(reason) = capability_error {
        return Admission::Unsupported(reason.into());
    }
    capabilities
        .and_then(|capabilities| question_admission(&requirements, capabilities).err())
        .map_or(Admission::Ready, Admission::Unsupported)
}

fn question_admission(
    requirements: &[antiburn_checks::decision::QuestionRequirements],
    capabilities: &ModelCapabilities,
) -> std::result::Result<(), String> {
    use antiburn_checks::decision::QuestionKind;
    let mut counts = BTreeMap::new();
    for requirement in requirements {
        let count = counts.entry(&requirement.decision).or_insert(0_usize);
        *count += 1;
        if *count
            > capabilities
                .questions_per_request
                .value
                .unwrap_or_default()
                .min(8) as usize
        {
            return Err("This decision has more questions than the selected connection supports. Select a compatible connection in Settings → Checks.".into());
        }
        let support = match requirement.kind {
            QuestionKind::Choice => capabilities.output.choice,
            QuestionKind::Score => capabilities.output.score,
            QuestionKind::Noul if capabilities.output.noul => continue,
            QuestionKind::Noul => return Err("The selected connection does not provide verified Noul answers. Select a compatible connection in Settings → Checks.".into()),
        };
        let Some(support) = support else {
            return Err(format!(
                "The selected connection does not provide verified {:?} answers. Select a compatible connection in Settings → Checks.",
                requirement.kind
            ));
        };
        if requirement.probabilities && !support.probabilities {
            return Err("This check requires probabilities. Select a compatible connection in Settings → Checks.".into());
        }
        if requirement.confidence && !support.confidence {
            return Err("This check requires confidence values. Select a compatible connection in Settings → Checks.".into());
        }
        let maximum = support
            .maximum_criteria
            .min(capabilities.criteria_per_question.value.unwrap_or_default());
        if requirement.criteria_count < support.minimum_criteria as usize
            || requirement.criteria_count > maximum as usize
        {
            return Err(format!(
                "This check has {} {:?} criteria. The selected connection supports {} to {}. Select a compatible model in Settings → Checks.",
                requirement.criteria_count, requirement.kind, support.minimum_criteria, maximum
            ));
        }
    }
    Ok(())
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
        if let Admission::Unsupported(reason) = admission {
            structured.push(diagnostics::payload(
                "provider-unavailable",
                crate::insights_report::installed_checks::InstalledCheckDiagnosticSeverity::Error,
                "Check the selected connection",
                reason,
                definition_sources.first().cloned(),
                Vec::new(),
            ));
        } else if *admission == Admission::ProviderNotConfigured {
            structured.push(diagnostics::payload(
                "provider-not-configured",
                crate::insights_report::installed_checks::InstalledCheckDiagnosticSeverity::Error,
                "Set up a decision model",
                "Set up a decision model in Settings → Checks before this check can run.",
                None,
                Vec::new(),
            ));
        } else if *admission == Admission::AuthenticationRejected {
            structured.push(diagnostics::payload(
                "provider-authentication",
                crate::insights_report::installed_checks::InstalledCheckDiagnosticSeverity::Error,
                "Reconnect the selected provider",
                "The selected provider rejected the saved credential. Update it in Settings → Checks.",
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
        is_local: matches!(&definition.state, DefinitionState::Ready(compiled) if compiled.is_local()),
        decision_name: match &definition.state {
            DefinitionState::Ready(compiled) => match compiled.decisions.as_slice() {
                [decision] => Some(decision.name.clone()),
                _ => None,
            },
            _ => None,
        },
        trigger: match &definition.state {
            DefinitionState::Ready(compiled) => Some(InstalledCheckTrigger {
                event: match &compiled.trigger.family {
                    antiburn_checks::compiler::TriggerFamily::Messages {
                        tool_result,
                        user_message,
                        assistant_message,
                        tool_call,
                    } => {
                        match (
                            *tool_result,
                            user_message.is_some(),
                            assistant_message.is_some(),
                            tool_call.is_some(),
                        ) {
                            (true, false, false, false) => InstalledCheckEvent::ToolResult,
                            (false, true, false, false) => InstalledCheckEvent::UserMessage,
                            (false, false, true, false) => InstalledCheckEvent::AssistantMessage,
                            (false, false, false, true) => InstalledCheckEvent::ToolCall,
                            _ => InstalledCheckEvent::Message,
                        }
                    }
                    antiburn_checks::compiler::TriggerFamily::ResponseCycle(_) => {
                        InstalledCheckEvent::TurnEnd
                    }
                    antiburn_checks::compiler::TriggerFamily::Session {
                        session_start,
                        session_idle,
                    } => match (session_start.is_some(), session_idle.is_some()) {
                        (true, false) => InstalledCheckEvent::SessionStart,
                        (false, true) => InstalledCheckEvent::SessionIdle,
                        _ => InstalledCheckEvent::SessionEvent,
                    },
                },
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

fn publish_runtime_failure(state: &InstalledChecks, failure: &diagnostics::RuntimeFailure) {
    let mut definitions = state.metadata.write().expect("installed metadata lock");
    set_runtime_failure(&mut definitions, Some(failure));
}

fn set_runtime_failure(
    definitions: &mut [InstalledCheckDefinition],
    failure: Option<&diagnostics::RuntimeFailure>,
) {
    for definition in definitions {
        definition
            .diagnostics
            .retain(|diagnostic| diagnostic.code != "storage-failed");
        if let Some(failure) = failure {
            definition.diagnostics.push(failure.diagnostic());
        }
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
            is_local: false,
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
            "Cannot read this folder. Check folder permissions and grant access in Settings → Sources if the folder is protected.",
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
                is_local: false,
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
        diagnostics::trace_worker_result(run(&worker_app).await);
    }));
}

pub(crate) fn lifecycle_changes_projects(event: &crate::session_lifecycle::SessionEvent) -> bool {
    use crate::session_lifecycle::SessionEvent;
    match event {
        SessionEvent::Started { .. }
        | SessionEvent::Activity { resumed: true, .. }
        | SessionEvent::Idle { .. }
        | SessionEvent::Removed { .. }
        | SessionEvent::IndexChanged { .. }
        | SessionEvent::Resync => true,
        SessionEvent::Updated { facets, .. } => facets.project_scope,
        _ => false,
    }
}

fn refresh_watcher(
    app: &tauri::AppHandle,
    watcher: &mut Option<watch::DirectoryWatcher>,
    directories: &[PathBuf],
    previous_failure: &mut Option<diagnostics::RuntimeFailure>,
) {
    if watcher.is_none() {
        let watch_app = app.clone();
        match watch::DirectoryWatcher::new(move || wake(&watch_app)) {
            Ok(created) => {
                *watcher = Some(created);
                *previous_failure = None;
            }
            Err(error) => {
                diagnostics::RuntimeFailure::classify(
                    diagnostics::RuntimeStage::DirectoryWatch,
                    &error.into(),
                )
                .trace_if_changed(previous_failure, "installed_check_watch_failed");
            }
        }
    }
    if let Some(watcher) = watcher
        && watcher.refresh(directories)
    {
        app.state::<InstalledChecks>().wake.notify_one();
    }
}

async fn run(app: &tauri::AppHandle) -> Result<()> {
    let started = Instant::now();
    let state = app.state::<InstalledChecks>();
    let store = Arc::new(app.state::<Store>().inner().clone());
    let runtime: Arc<dyn SelectedJobRuntime> = Arc::new(AppRuntime(app.clone()));
    let evaluator = Arc::new(InstalledDecisionEvaluator::factory(Arc::clone(&runtime)));
    let native_home = antiburn_local::paths::home_dir();
    let global = native_home
        .as_ref()
        .map(|home| home.join(".antiburn/checks"));
    let mut host = diagnostics::worker_result(
        diagnostics::RuntimeStage::WorkerStartup,
        InstalledCheckHost::new(store, evaluator, global),
    )?;
    host.current_context = current_context::CurrentContextCapture::new(native_home);
    let mut turns = WorkerTurns::new(
        app.state::<crate::session_lifecycle::SessionEvents>()
            .subscribe(),
    );
    let mut last_discovery = None::<Instant>;
    let mut previous_admission = None;
    let mut previous_connection_generation = None;
    let mut catch_up_more = false;
    let mut watcher: Option<watch::DirectoryWatcher> = None;
    let mut previous_gate = None;
    let mut previous_reconciliation = None;
    let mut previous_update_failure = None;
    let mut previous_watch_failure = None;
    loop {
        let (mut discover, idle_requested) = match turns
            .next(&host, &state, catch_up_more, || {
                crate::jev::settings::yaml_discovery_enabled(&app.state::<Store>())
                    && state.credentials_restored.load(Ordering::Acquire)
            })
            .await
        {
            WorkerWake::Update {
                discover,
                idle_requested,
            } => (discover, idle_requested),
            WorkerWake::Closed => break,
            WorkerWake::Settled(settled) => {
                match settled {
                    Some(Ok(Ok(Some(_)))) => {
                        let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
                    }
                    Some(Ok(Err(error))) => {
                        let failure = diagnostics::RuntimeFailure::classify(
                            diagnostics::RuntimeStage::AssessmentSettlement,
                            &anyhow::Error::new(error),
                        );
                        publish_runtime_failure(&state, &failure);
                        let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
                    }
                    Some(Err(error)) => diagnostics::RuntimeFailure::classify(
                        diagnostics::RuntimeStage::TaskJoin,
                        &error.into(),
                    )
                    .trace("installed_check_task_failed"),
                    _ => {}
                }
                continue;
            }
        };
        if state.stopping.load(Ordering::Acquire) {
            break;
        }
        let gate = if !crate::jev::settings::yaml_discovery_enabled(&app.state::<Store>()) {
            "disabled"
        } else if !state.credentials_restored.load(Ordering::Acquire) {
            "restoring_credentials"
        } else {
            match admission(app) {
                Admission::Ready => "ready",
                Admission::Paused(_) => "paused",
                Admission::Unsupported(_) => "unsupported_provider",
                Admission::ProviderNotConfigured => "provider_not_configured",
                Admission::AuthenticationRejected => "authentication_rejected",
            }
        };
        if previous_gate != Some(gate) {
            ::tracing::debug!(
                event = "installed_check_admission",
                state = gate,
                since_start_ms = started.elapsed().as_millis() as u64
            );
            previous_gate = Some(gate);
        }
        if !crate::jev::settings::yaml_discovery_enabled(&app.state::<Store>()) {
            state.delivery_ready.store(false, Ordering::Release);
            let requested = state.requested_turn.load(Ordering::Acquire);
            diagnostics::worker_result(
                diagnostics::RuntimeStage::DisableSettlement,
                host.pause_disabled(),
            )?;
            discover |= previous_admission.is_some()
                || last_discovery.is_none_or(|last| last.elapsed() >= DISCOVERY_INTERVAL);
            let update = if discover {
                let (returned_host, result) = diagnostics::worker_result(
                    diagnostics::RuntimeStage::Reconcile,
                    tauri::async_runtime::spawn_blocking(move || {
                        let result = host.refresh_paused_inventory();
                        (host, result)
                    })
                    .await
                    .map_err(Into::into),
                )?;
                host = returned_host;
                result
            } else {
                Ok(())
            };
            if finish_app_inventory_update(
                app,
                &state,
                &mut host,
                update,
                diagnostics::RuntimeStage::Reconcile,
                &mut previous_update_failure,
                turns.idle_failure(),
            ) {
                if discover {
                    last_discovery = Some(Instant::now());
                }
                previous_update_failure = None;
            } else {
                last_discovery = None;
            }
            refresh_watcher(
                app,
                &mut watcher,
                &host.watch_directories,
                &mut previous_watch_failure,
            );
            catch_up_more = false;
            previous_admission = None;
            previous_connection_generation = None;
            state.complete_disable(requested);
            continue;
        }
        if !state.credentials_restored.load(Ordering::Acquire) {
            continue;
        }
        if idle_requested {
            crate::insights_worker::wake(app);
        }
        let snapshot = app
            .state::<crate::jev::worker::WorkerHandle>()
            .execution_snapshot();
        let connection_generation = snapshot.as_ref().map(|snapshot| snapshot.generation);
        let current_admission = admission(app);
        discover |= previous_admission.as_ref() != Some(&current_admission)
            || previous_connection_generation != connection_generation
            || last_discovery.is_none_or(|last| last.elapsed() >= DISCOVERY_INTERVAL);
        if previous_admission.as_ref() != Some(&current_admission)
            || previous_connection_generation != connection_generation
        {
            state.delivery_ready.store(false, Ordering::Release);
        }
        let reconciliation = if discover {
            let (capabilities, capability_error) =
                execution::coordinator_capabilities(&state, connection_generation);
            host.capabilities = capabilities;
            host.capability_error = capability_error;
            let work_admission = current_admission.clone();
            let (returned_host, result) = diagnostics::worker_result(
                diagnostics::RuntimeStage::Reconcile,
                tauri::async_runtime::spawn_blocking(move || {
                    let result = host.reconcile(&work_admission);
                    (host, result)
                })
                .await
                .map_err(Into::into),
            )?;
            host = returned_host;
            result
        } else {
            Ok(())
        };
        let reconciled = discover && reconciliation.is_ok();
        if !finish_app_inventory_update(
            app,
            &state,
            &mut host,
            reconciliation,
            diagnostics::RuntimeStage::Reconcile,
            &mut previous_update_failure,
            turns.idle_failure(),
        ) {
            catch_up_more = false;
            last_discovery = None;
            continue;
        }
        if reconciled {
            update_delivery_readiness(
                &state,
                &host,
                current_admission == Admission::Ready && admission(app) == Admission::Ready,
            );
            let summary = (
                host.definitions.len(),
                host.projects.len(),
                host.projects
                    .values()
                    .map(|project| project.active.len())
                    .sum::<usize>(),
                state.delivery_ready.load(Ordering::Acquire),
            );
            if previous_reconciliation != Some(summary) {
                ::tracing::debug!(
                    event = "installed_check_reconciled",
                    definitions = summary.0,
                    scopes = summary.1,
                    active_checks = summary.2,
                    delivery_ready = summary.3
                );
                previous_reconciliation = Some(summary);
            }
            last_discovery = Some(Instant::now());
            previous_admission = Some(current_admission);
            previous_connection_generation = connection_generation;
            refresh_watcher(
                app,
                &mut watcher,
                &host.watch_directories,
                &mut previous_watch_failure,
            );
        }
        let (returned_host, result) = diagnostics::worker_result(
            diagnostics::RuntimeStage::SourceCatchUp,
            tauri::async_runtime::spawn_blocking(move || {
                let result = host.catch_up();
                (host, result)
            })
            .await
            .map_err(Into::into),
        )?;
        host = returned_host;
        if !finish_app_inventory_update(
            app,
            &state,
            &mut host,
            Ok(()),
            diagnostics::RuntimeStage::SourceCatchUp,
            &mut previous_update_failure,
            turns.idle_failure(),
        ) {
            if let Err(error) = result {
                diagnostics::RuntimeFailure::classify(
                    diagnostics::RuntimeStage::SourceCatchUp,
                    &error,
                )
                .trace("installed_check_update_failed");
            }
            catch_up_more = false;
            last_discovery = None;
            continue;
        }
        let result = result.map_err(|error| (diagnostics::RuntimeStage::SourceCatchUp, error));
        match result {
            Ok((changed, more)) => {
                previous_update_failure = None;
                if changed {
                    let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
                }
                for job in host.evaluation_jobs(EVALUATIONS.saturating_sub(turns.tasks.len())) {
                    turns.tasks.spawn(job.run_selected(Arc::clone(&runtime)));
                }
                catch_up_more = more;
            }
            Err((stage, error)) => {
                catch_up_more = false;
                if !reconciled {
                    last_discovery = None;
                }
                let failure = diagnostics::RuntimeFailure::classify(stage, &error);
                failure.trace_if_changed(
                    &mut previous_update_failure,
                    "installed_check_update_failed",
                );
                publish_runtime_failure(&state, &failure);
                let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
            }
        }
    }
    while let Some(result) = turns.tasks.join_next().await {
        if let Err(error) = result {
            diagnostics::RuntimeFailure::classify(
                diagnostics::RuntimeStage::TaskJoin,
                &error.into(),
            )
            .trace_shutdown();
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
