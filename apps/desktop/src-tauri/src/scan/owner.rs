use std::collections::{HashMap, HashSet};

use antiburn_local::discovery::DiscoveryBatch;
use tokio::sync::mpsc;
use tokio::task::Id;

use super::prepare::{PreparationOutcome, PreparedDescription, prepare_description_with_read};
use super::queue::{QueueInput, ScanQueue, WorkOrigin};
use super::*;

const BATCH_SIZE: usize = 64;
const PASS_PENDING_LIMIT: usize = 1024;
const LIVE_PENDING_LIMIT: usize = 256;

pub(super) async fn recover_fingerprints(app: &AppHandle) -> anyhow::Result<bool> {
    if !on_demand_start(&app.state::<ScanController>()) {
        return Ok(false);
    }
    let _ownership = ScanOwnership::new(app.state::<ScanController>());
    let store = app.state::<Store>();
    let mut writer = IncrementalAdmission {
        io: AdmissionIo::App(app.clone()),
        store: &store,
        previous_records: HashMap::new(),
        evidence_agents: agents::evidence_cohort(),
        write_fence: store.capture_destructive_write_epoch(),
        error: None,
        outcomes: HashMap::new(),
        list_changed: false,
    };
    refresh_logs(app, Vec::new(), &mut writer).await?;
    writer.finish()?;
    Ok(true)
}

struct OwnerPolicy {
    home: PathBuf,
    ignored: HashSet<String>,
    cutoff: Option<i64>,
    include_non_repo_folders: bool,
    progress: bool,
    refresh_repositories: bool,
    #[cfg(test)]
    hooks: Option<Arc<prepare::PreparationHooks>>,
}

enum PreparedTask {
    Identity(Option<DescriptionLog>, Option<SessionRecord>),
    Ready(PreparedDescription, SessionWriteGuard, SessionKey),
}

struct TaskOwner {
    key: String,
    generation: u64,
    origin: WorkOrigin,
    activity: Option<i64>,
    session_key: Option<SessionKey>,
}

enum TitleInput {
    Prepared(
        Box<crate::store::SessionTitleRefreshGuard>,
        Option<ResolvedTitle>,
    ),
    Failed(anyhow::Error),
}

pub(super) struct PassInputs {
    pub now: i64,
    pub since_secs: i64,
    pub history: bool,
    pub progress: bool,
    pub home: PathBuf,
    pub ignored: HashSet<String>,
    pub include_non_repo_folders: bool,
    pub refresh_repositories: bool,
}

pub(super) async fn refresh_titles(
    app: &AppHandle,
    agents: &BTreeSet<AgentKind>,
) -> anyhow::Result<scoped::ScopedSummary> {
    let store = app.state::<Store>();
    let mut writer = IncrementalAdmission {
        io: AdmissionIo::App(app.clone()),
        store: &store,
        previous_records: HashMap::new(),
        evidence_agents: Vec::new(),
        write_fence: store.capture_destructive_write_epoch(),
        error: None,
        outcomes: HashMap::new(),
        list_changed: false,
    };
    let (finite_sender, finite) = mpsc::channel(1);
    let (title_sender, titles) = mpsc::channel(2);
    let agents = agents.clone();
    let app_for_titles = app.clone();
    let mut producer = JoinSet::new();
    producer.spawn(async move {
        let _completion = finite_sender;
        let store = app_for_titles.state::<Store>();
        for agent in agents {
            let guards = match store.native_session_title_refresh_guards(agent.slug()) {
                Ok(guards) => guards,
                Err(error) => {
                    let _ = title_sender.send(TitleInput::Failed(error)).await;
                    return;
                }
            };
            for page in guards.chunks(256) {
                let ids = page
                    .iter()
                    .map(|guard| guard.key.session_id.clone())
                    .collect::<Vec<_>>();
                let mut resolved = Explorers::DISK
                    .indexed_session_titles_for(&agent, &ids)
                    .await;
                for guard in page {
                    let title = resolved.remove(&guard.key.session_id);
                    if title_sender
                        .send(TitleInput::Prepared(Box::new(guard.clone()), title))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
    });
    let (_live_sender, live) = mpsc::channel(1);
    let policy = OwnerPolicy {
        home: home_dir().unwrap_or_default(),
        ignored: HashSet::new(),
        cutoff: None,
        include_non_repo_folders: store.settings_snapshot().include_non_repo_folders,
        progress: false,
        refresh_repositories: false,
        #[cfg(test)]
        hooks: None,
    };
    let described = run_with_titles(finite, live, titles, Some(app), &policy, &mut writer).await;
    if let Some(Err(error)) = producer.join_next().await {
        writer.fail(anyhow::anyhow!("Title lookup task failed: {error}"));
    }
    writer.finish()?;
    Ok(scoped::ScopedSummary {
        sessions: described.records.len(),
        re_described: described.changed.len(),
    })
}

pub(super) async fn discover_and_run(
    app: &AppHandle,
    scope: &PassScope,
    inputs: PassInputs,
    writer: &mut IncrementalAdmission<'_>,
) -> anyhow::Result<Described> {
    let PassInputs {
        now,
        since_secs,
        history,
        progress,
        home,
        ignored,
        include_non_repo_folders,
        refresh_repositories,
    } = inputs;
    let (sender, receiver) = mpsc::channel(2);
    let scope = scope.clone();
    let mut discovery = JoinSet::new();
    discovery.spawn(async move {
        match scope {
            PassScope::Full => {
                Explorers::DISK
                    .discover_recent_batches(now, since_secs, BATCH_SIZE, sender)
                    .await
            }
            PassScope::Agents(agents) => {
                Explorers::DISK
                    .discover_recent_batches_for_agents(
                        &agents.into_iter().collect::<Vec<_>>(),
                        now,
                        since_secs,
                        BATCH_SIZE,
                        sender,
                    )
                    .await
            }
        }
    });
    let (_live_sender, live_receiver) = mpsc::channel(2);
    let policy = OwnerPolicy {
        home,
        ignored,
        cutoff: (!history).then_some(now - CURRENT_WINDOW_SECS),
        include_non_repo_folders,
        progress,
        refresh_repositories,
        #[cfg(test)]
        hooks: None,
    };
    let described = run(receiver, live_receiver, Some(app), &policy, writer).await;
    if let Some(Err(error)) = discovery.try_join_next() {
        writer.fail(anyhow::anyhow!("Session discovery task failed: {error}"));
    }
    Ok(described)
}

pub(super) async fn refresh_logs(
    app: &AppHandle,
    logs: Vec<SessionLog>,
    writer: &mut IncrementalAdmission<'_>,
) -> anyhow::Result<Described> {
    let store = app.state::<Store>();
    let policy = OwnerPolicy {
        home: home_dir().unwrap_or_default(),
        ignored: store.repository_policy_snapshot()?.ignored_paths,
        cutoff: Some(unix_now() - CURRENT_WINDOW_SECS),
        include_non_repo_folders: store.settings_snapshot().include_non_repo_folders,
        progress: false,
        refresh_repositories: false,
        #[cfg(test)]
        hooks: None,
    };
    let (sender, receiver) = mpsc::channel(2);
    let mut discovery = JoinSet::new();
    discovery.spawn(async move {
        for chunk in logs.chunks(BATCH_SIZE) {
            if sender
                .send(DiscoveryBatch {
                    agent: chunk[0].agent_type,
                    candidates: chunk.to_vec(),
                    complete: false,
                })
                .await
                .is_err()
            {
                break;
            }
        }
    });
    let (_sender, live) = mpsc::channel(2);
    Ok(run(receiver, live, Some(app), &policy, writer).await)
}

fn identity(log: &SessionLog) -> String {
    let binding = match &log.source {
        SessionSource::File(path) => serde_json::json!(["file", path]),
        SessionSource::ProviderDb {
            db_path,
            session_id,
            ..
        } => serde_json::json!([
            "database",
            db_path,
            session_id,
            crate::analysis::source_format(log.agent_type, &log.source)
        ]),
        SessionSource::Inline { label, .. } => serde_json::json!(["inline", label]),
    };
    serde_json::json!([log.environment.key(), log.agent_type.slug(), binding]).to_string()
}

fn enqueue(queue: &mut ScanQueue<SessionLog>, log: SessionLog, origin: WorkOrigin) {
    queue.enqueue(QueueInput {
        key: identity(&log),
        activity: log.updated_at,
        payload: log,
        observed_at_epoch: unix_now(),
        observed_at: Instant::now(),
        origin,
        live: origin == WorkOrigin::Live,
    });
}

async fn run(
    finite: mpsc::Receiver<DiscoveryBatch>,
    live: mpsc::Receiver<DiscoveryBatch>,
    app: Option<&AppHandle>,
    policy: &OwnerPolicy,
    writer: &mut IncrementalAdmission<'_>,
) -> Described {
    let (sender, titles) = mpsc::channel(1);
    drop(sender);
    run_with_titles(finite, live, titles, app, policy, writer).await
}

async fn run_with_titles(
    mut finite: mpsc::Receiver<DiscoveryBatch>,
    mut live: mpsc::Receiver<DiscoveryBatch>,
    mut titles: mpsc::Receiver<TitleInput>,
    app: Option<&AppHandle>,
    policy: &OwnerPolicy,
    writer: &mut IncrementalAdmission<'_>,
) -> Described {
    let mut queue: ScanQueue<SessionLog> = ScanQueue::new();
    let mut tasks = JoinSet::new();
    let mut owners: HashMap<Id, TaskOwner> = HashMap::new();
    let mut finite_closed = false;
    let mut finite_keys = HashSet::new();
    let mut selected: HashMap<SessionKey, (Option<i64>, String, bool)> = HashMap::new();
    let mut gate = GateCounts::default();
    let mut probes = JoinSet::new();
    let mut hints = JoinSet::new();
    let mut probing = BTreeSet::new();
    let mut probe_agents = HashMap::new();
    let mut dirty_agents = BTreeSet::new();
    let (probe_sender, mut probe_receiver) = mpsc::channel::<DiscoveryBatch>(2);
    let mut last_burst = None;
    let mut overflow = false;
    let mut completed = 0;
    let mut found = 0;
    let mut poll = tokio::time::interval(Duration::from_millis(100));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut next_probe = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut last_progress = Instant::now();
    let mut finalizers = JoinSet::new();
    let mut finalizing = false;
    let mut fingerprints = JoinSet::new();
    let mut attempted_fingerprints = HashSet::new();
    let mut deferred_fingerprints = Vec::new();
    let fingerprint_pass_started_at = writer.io.now();
    let mut fingerprint_keys: HashMap<Id, crate::store::PendingSourceFingerprint> = HashMap::new();

    loop {
        if writer.io.cancelled() {
            break;
        }
        if fingerprints.len() < 2 {
            match writer.store.claim_source_fingerprints(
                writer.io.now(),
                &attempted_fingerprints,
                2 - fingerprints.len(),
                fingerprint_pass_started_at,
            ) {
                Ok(work) => {
                    for pending in work {
                        let key = pending.key.clone();
                        attempted_fingerprints.insert(key.clone());
                        let log = SessionLog {
                            agent_type: AgentKind::OpenCode,
                            source: SessionSource::ProviderDb {
                                agent: AgentKind::OpenCode,
                                db_path: PathBuf::from(
                                    pending
                                        .source_path
                                        .as_ref()
                                        .expect("claimed source has a path"),
                                ),
                                session_id: key.session_id.clone(),
                                source_format: Some(pending.source_format),
                            },
                            updated_at: pending.updated_at_epoch,
                            environment: DiscoveryEnvironment::Native,
                        };
                        let revision = pending.admission_revision.clone();
                        #[cfg(test)]
                        let hooks = policy.hooks.clone();
                        let task_pending = pending.clone();
                        let handle = fingerprints.spawn(async move {
                            #[cfg(test)]
                            if let Some(hooks) = &hooks {
                                hooks
                                    .wait(&log, prepare::PreparationStage::Fingerprint)
                                    .await;
                            }
                            let fingerprint =
                                fingerprint::strict_fingerprint(&log, &revision).await;
                            #[cfg(test)]
                            if let Some(hooks) = &hooks {
                                hooks
                                    .wait(&log, prepare::PreparationStage::FingerprintCommit)
                                    .await;
                            }
                            (log, pending, fingerprint)
                        });
                        fingerprint_keys.insert(handle.id(), task_pending);
                    }
                }
                Err(error) => writer.fail(error),
            }
        }
        let can_read = app.is_none_or(|app| {
            !policy.progress || app.state::<FirstRunGate>().stage() >= FirstRunStage::Sessions
        });
        if can_read {
            while let Some(dispatch) = queue.dispatch() {
                let work = dispatch.work;
                ::tracing::trace!(event = "scan_preparation_dispatched", lane = ?dispatch.lane,
                    generation = work.generation, ticket = work.ticket,
                    queue_wait_ms = work.enqueued_at.elapsed().as_millis() as u64,
                    dirty_wait_ms = work.dirtied_at.elapsed().as_millis() as u64,
                     activity = work.activity);
                ::tracing::debug!(
                    event = "scan_admission_dispatched",
                    agent = work.payload.agent_type.slug(),
                    queue_wait_ms = work.enqueued_at.elapsed().as_millis() as u64,
                    activity_age_secs =
                        work.activity.map(|at| unix_now().saturating_sub(at).max(0))
                );
                let activity_key = SessionActivityKey::new(
                    work.payload.environment.key(),
                    work.payload.agent_type.slug(),
                    work.payload.source_label(),
                );
                let previous = match writer
                    .store
                    .session_records_for_activity_keys(std::slice::from_ref(&activity_key))
                {
                    Ok(records) => records.get(&activity_key).cloned(),
                    Err(error) => {
                        writer.fail(error);
                        queue.release(&work.key, work.generation);
                        continue;
                    }
                };
                let previous_map = previous
                    .iter()
                    .map(|record| (activity_key.clone(), record.clone()))
                    .collect();
                #[cfg(test)]
                let hooks = policy.hooks.clone();
                let handle = tasks.spawn(async move {
                    #[cfg(test)]
                    if let Some(hooks) = hooks {
                        hooks
                            .wait(&work.payload, prepare::PreparationStage::Identity)
                            .await;
                    }
                    let description = prepare_description_logs(vec![work.payload], &previous_map)
                        .await
                        .expect("identity preparation has no store writes")
                        .pop();
                    PreparedTask::Identity(description, previous)
                });
                owners.insert(
                    handle.id(),
                    TaskOwner {
                        key: work.key,
                        generation: work.generation,
                        origin: work.origin,
                        activity: work.activity,
                        session_key: None,
                    },
                );
            }
        }
        if finite_closed && !queue.has_pass_work() && titles.is_closed() && titles.is_empty() {
            if !finalizing {
                finalizing = true;
                if let Some(app) =
                    app.filter(|_| policy.refresh_repositories || writer.list_changed)
                {
                    let app = app.clone();
                    finalizers.spawn(async move { repositories::refresh(&app).await });
                }
            }
            if finalizers.is_empty() && fingerprints.is_empty() {
                break;
            }
        }
        tokio::select! {
            result = fingerprints.join_next_with_id(), if !fingerprints.is_empty() => {
                let Some(result) = result else { continue; };
                let id = match &result { Ok((id, _)) => *id, Err(error) => error.id() };
                let captured = fingerprint_keys.remove(&id).expect("each hash has an owner");
                match result {
                    Ok((_, (log, pending, fingerprint))) => {
                        let mut completed = false;
                        if let Some(fingerprint) = fingerprint.filter(|_| !writer.io.cancelled())
                            && fingerprint::admission_revision(&log).await.as_deref() == Some(pending.admission_revision.as_str()) {
                                match writer.io.checked(writer.store.complete_source_fingerprint(&pending, &fingerprint, writer.io.now())) {
                                    Ok(true) => { completed = true; writer.io.wake_workers(); },
                                    Ok(false) => {},
                                    Err(error) => writer.fail(error),
                                }
                        }
                        if !completed {
                            if fingerprint::source_is_deleted(&log).await {
                                if let Err(error) = writer.store.quarantine_missing_source_fingerprint(&pending) {
                                    writer.fail(error);
                                }
                            } else {
                                if let Err(error) = writer.store.retry_source_fingerprint(&pending, writer.io.now(), writer.io.cancelled()) {
                                    writer.fail(error);
                                }
                                deferred_fingerprints.push(pending);
                                if !writer.io.cancelled() {
                                    enqueue(&mut queue, log, WorkOrigin::Recovery);
                                }
                            }
                        }
                    }
                    Err(error) => {
                        if let Err(error) = writer.store.retry_source_fingerprint(&captured, writer.io.now(), false) { writer.fail(error); }
                        deferred_fingerprints.push(captured);
                        writer.fail(anyhow::anyhow!("Source fingerprint task failed: {error}"));
                    },
                }
            }
            title = titles.recv(), if !titles.is_closed() || !titles.is_empty() => {
                match title {
                    Some(TitleInput::Prepared(guard, title)) => {
                        finite_keys.insert(guard.key.clone());
                        let records = match writer.store.session_records_for_session_keys(std::slice::from_ref(&guard.key)) {
                            Ok(records) => records,
                            Err(error) => { writer.fail(error); continue; }
                        };
                        let Some(mut record) = records.into_iter().next() else { continue; };
                        let previous = record.clone();
                        let changed = title.is_some_and(|title| AgentKind::from_slug(&record.key.agent).is_some_and(|agent| apply_indexed_title(&mut record, &agent, title)));
                        if !changed {
                            writer.outcomes.insert(record.key.clone(), AttemptOutcome::Accepted(Box::new(record), false));
                            continue;
                        }
                        match writer.io.checked(writer.store.update_session_title_if_current(&guard, record.title.as_deref(), record.title_source.as_deref())) {
                            Ok(true) => {
                                let previous_map = HashMap::from([(SessionActivityKey::new(previous.key.environment_key.clone(), previous.key.agent.clone(), previous.source_label.clone()), previous)]);
                                for (session, facets) in row_change_facets(std::slice::from_ref(&record), std::slice::from_ref(&record.key), &previous_map) {
                                    writer.io.report(session_lifecycle::Observation::RowChanged { session, facets, at: writer.io.now() }).await;
                                }
                                writer.outcomes.insert(record.key.clone(), AttemptOutcome::Accepted(Box::new(record), true));
                            }
                            Ok(false) => {}
                            Err(error) => writer.fail(error),
                        }
                    }
                    Some(TitleInput::Failed(error)) => writer.fail(error),
                    None => {}
                }
            }
            result = finalizers.join_next(), if !finalizers.is_empty() => {
                match result {
                    Some(Ok(Err(error))) => writer.fail(error),
                    Some(Err(error)) => writer.fail(anyhow::anyhow!("Repository refresh task failed: {error}")),
                    _ => {}
                }
            }
            batch = finite.recv(), if !finite_closed && (queue.pending_len() <= PASS_PENDING_LIMIT - BATCH_SIZE || !can_read) => {
                match batch {
                    Some(batch) => {
                        found += batch.candidates.len();
                        if let Some(app) = app {
                            let status = app.state::<ScanController>().update(|status| {
                                status.sessions = found;
                                if policy.progress
                                    && let Some(entry) = status
                                        .found_by_agent
                                        .iter_mut()
                                        .find(|entry| entry.agent == batch.agent.slug())
                                {
                                    entry.sessions += batch.candidates.len();
                                    entry.done |= batch.complete;
                                }
                            });
                            let _ = app.emit(EVENT_PROGRESS, status);
                        }
                        for log in batch.candidates {
                            if queue.pending_len() < PASS_PENDING_LIMIT || queue.contains_pending(&identity(&log)) {
                                enqueue(&mut queue, log, WorkOrigin::Pass);
                            } else {
                                // First-run discovery must finish before its read gate opens.
                                // Reconciliation covers candidates beyond the memory budget.
                                overflow = true;
                                if writer.error.is_none() {
                                    writer.fail(anyhow::anyhow!("Session discovery exceeded its pending limit; reconciliation is required"));
                                }
                            }
                        }
                    }
                    None => finite_closed = true,
                }
            }
            batch = live.recv(), if !live.is_closed() || !live.is_empty() => {
                if let Some(batch) = batch {
                    for log in batch.candidates {
                        if queue.pending_live_len() < LIVE_PENDING_LIMIT || queue.contains_pending(&identity(&log)) { enqueue(&mut queue, log, WorkOrigin::Live); }
                        else { overflow = true; }
                    }
                }
            }
            batch = probe_receiver.recv(), if !probes.is_empty() || !hints.is_empty() || !probe_receiver.is_empty() => {
                if let Some(batch) = batch {
                    for log in batch.candidates {
                        if queue.pending_live_len() < LIVE_PENDING_LIMIT || queue.contains_pending(&identity(&log)) { enqueue(&mut queue, log, WorkOrigin::Live); }
                        else { overflow = true; }
                    }
                }
            }
            result = probes.join_next_with_id(), if !probes.is_empty() => {
                let Some(result) = result else { continue; };
                let id = match &result { Ok((id, _)) => *id, Err(error) => error.id() };
                let agent = probe_agents.remove(&id).expect("each discovery probe has an owner");
                probing.remove(&agent);
                match result {
                    Ok(_) => {
                        if dirty_agents.remove(&agent) {
                            spawn_agent_probe(&mut probes, &mut probing, &mut probe_agents, agent, probe_sender.clone());
                        }
                    }
                    Err(error) => {
                        ::tracing::warn!(event = "scan_live_discovery_failed", error = %error);
                        overflow = true;
                    }
                }
            }
            result = hints.join_next(), if !hints.is_empty() => {
                match result {
                    Some(Ok(agents)) => for agent in agents {
                        if probing.contains(&agent) { dirty_agents.insert(agent); }
                        else { spawn_agent_probe(&mut probes, &mut probing, &mut probe_agents, agent, probe_sender.clone()); }
                    },
                    Some(Err(error)) => {
                        ::tracing::warn!(event = "scan_live_hint_failed", error = %error);
                        overflow = true;
                    }
                    None => {}
                }
            }
            result = tasks.join_next_with_id(), if !tasks.is_empty() => {
                let Some(result) = result else { continue; };
                let id = match &result { Ok((id, _)) => *id, Err(error) => error.id() };
                let mut owner = owners.remove(&id).expect("each preparation task has an owner");
                match result {
                    Ok((_, PreparedTask::Identity(Some(description), previous))) => {
                        let source_format = crate::analysis::source_format(description.log.agent_type, &description.log.source);
                        match writer.store.session_source_is_allowed(&description.key, source_format) {
                            Ok(true) => {},
                            Ok(false) => { queue.release(&owner.key, owner.generation); continue; },
                            Err(error) => { writer.fail(error); queue.release(&owner.key, owner.generation); continue; },
                        }
                        let is_v2 = fingerprint::uses_bounded_admission(&description.log)
                            && source_format == antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2;
                        let rank = (owner.activity, owner.key.clone(), is_v2);
                        let already_finite = finite_keys.contains(&description.key);
                        if owner.origin == WorkOrigin::Pass { finite_keys.insert(description.key.clone()); }
                        if selected.get(&description.key).is_some_and(|current| current.1 != owner.key &&
                            (current.2 && !is_v2 || current.2 == is_v2 &&
                             (owner.origin == WorkOrigin::Live && already_finite ||
                              already_finite && (current.0 > rank.0 || current.0 == rank.0 && current.1 < rank.1)))) {
                            queue.release(&owner.key, owner.generation);
                            continue;
                        }
                        selected.insert(description.key.clone(), rank);
                        let guard = match session_write_guard(writer.store, &description.log, &description.key, writer.write_fence) {
                            Ok(guard) => guard,
                            Err(error) => {
                                writer.fail(error);
                                queue.release(&owner.key, owner.generation);
                                continue;
                            }
                        };
                        if owner.origin == WorkOrigin::Pass {
                            writer.outcomes.insert(description.key.clone(), AttemptOutcome::Failed);
                        }
                        let home = policy.home.clone();
                        let ignored = policy.ignored.clone();
                        let cutoff = match owner.origin {
                            WorkOrigin::Pass => policy.cutoff,
                            WorkOrigin::Live => Some(unix_now() - CURRENT_WINDOW_SECS),
                            WorkOrigin::Recovery => None,
                        };
                        let include = policy.include_non_repo_folders;
                        let key = description.key.clone();
                        owner.session_key = Some(key.clone());
                        #[cfg(test)]
                        let hooks = policy.hooks.clone();
                        let handle = tasks.spawn(async move {
                            PreparedTask::Ready(prepare_description_with_read(description, home, ignored, previous, cutoff, include,
                                #[cfg(test)] hooks,
                            ).await, guard, key)
                        });
                        owners.insert(handle.id(), owner);
                        continue;
                    }
                    Ok((_, PreparedTask::Ready(prepared, guard, key))) => {
                        if selected.get(&key).is_some_and(|current| current.1 != owner.key) {
                            queue.release(&owner.key, owner.generation);
                            if !finite_keys.contains(&key) && !owners.values().any(|owner| owner.session_key.as_ref() == Some(&key)) {
                                selected.remove(&key);
                            }
                            continue;
                        }
                        debug_assert_eq!(identity(&prepared.log), owner.key);
                        if let Some(previous) = prepared.previous {
                            writer.previous_records.insert(SessionActivityKey::new(previous.key.environment_key.clone(), previous.key.agent.clone(), previous.source_label.clone()), previous);
                        }
                        let parsed_key = match &prepared.outcome {
                            PreparationOutcome::Accepted { record, .. } => Some(&record.key),
                            PreparationOutcome::Rejected(key) => Some(key),
                            _ => None,
                        };
                        if parsed_key.is_some_and(|parsed| parsed != &key) {
                            writer.fail(anyhow::anyhow!("Session identity changed during preparation"));
                        } else {
                            if owner.origin == WorkOrigin::Pass {
                                gate.missing_cwd += prepared.gate.missing_cwd;
                                gate.ignored += prepared.gate.ignored;
                                gate.subagent += prepared.gate.subagent;
                                gate.no_repo += prepared.gate.no_repo;
                                gate.folder += prepared.gate.folder;
                            }
                            match prepared.outcome {
                                PreparationOutcome::Accepted { record, changed } => {
                                    writer.evidence_agents = agents::evidence_cohort().iter().copied().filter(|agent| *agent == record.key.agent).collect();
                                    let started = Instant::now();
                                    let committed = writer.commit(&guard, &record, changed).await;
                                    ::tracing::debug!(event = "scan_metadata_commit_finished", agent = record.key.agent,
                                        elapsed_ms = started.elapsed().as_millis() as u64, committed,
                                        activity_age_secs = record.updated_at_epoch.map(|at| unix_now().saturating_sub(at).max(0)));
                                }
                                PreparationOutcome::Rejected(key) => { writer.delete_rejected(&guard, &key).await; }
                                PreparationOutcome::Deferred(error) => writer.fail(anyhow::anyhow!(error)),
                                PreparationOutcome::OutsideWindow | PreparationOutcome::Skipped => {
                                    if owner.origin == WorkOrigin::Pass { writer.outcomes.remove(&key); }
                                }
                            }
                        }
                        if !finite_keys.contains(&key) {
                            writer.outcomes.remove(&key);
                            if !owners.values().any(|owner| owner.session_key.as_ref() == Some(&key)) { selected.remove(&key); }
                            writer.previous_records.retain(|_, record| record.key != key);
                        }
                    }
                    Ok((_, PreparedTask::Identity(None, _))) => {}
                    Err(error) => writer.fail(anyhow::anyhow!("Session preparation task failed: {error}")),
                }
                queue.release(&owner.key, owner.generation);
                if owner.origin == WorkOrigin::Pass { completed += 1; }
            }
            _ = poll.tick() => {
                if let Some(app) = app {
                    if scheduled_scanning_allowed(app) {
                        let burst = app.state::<ScanController>().peek_burst();
                        let signature = burst.as_ref().map(|burst| (burst.events, burst.paths.clone()));
                        let now = tokio::time::Instant::now();
                        if signature != last_burst && hints.is_empty() {
                            last_burst = signature;
                            spawn_live_hints(&mut hints, app.clone(), burst, probe_sender.clone());
                        }
                        if now >= next_probe {
                            next_probe = now + Duration::from_secs(10);
                            if !probing.contains(&AgentKind::OpenCode) {
                                spawn_agent_probe(&mut probes, &mut probing, &mut probe_agents, AgentKind::OpenCode, probe_sender.clone());
                            }
                        }
                    }
                    if policy.progress && last_progress.elapsed() >= READ_PROGRESS_THROTTLE {
                        let status = app.state::<ScanController>().update(|status| {
                            if can_read { status.phase = ScanPhase::Reading; }
                            status.read = ReadProgress { completed, total: found.max(completed) };
                        });
                        let _ = app.emit(EVENT_PROGRESS, status);
                        last_progress = Instant::now();
                    }
                }
            }
        }
    }
    // The watcher burst remains with the idle scheduler. Poll-only work needs
    // reconciliation if the finite owner stops before it commits.
    if let Some(app) = app
        && (overflow
            || (!queue.is_empty()
                || !probes.is_empty()
                || !hints.is_empty()
                || !probe_receiver.is_empty())
                && app.state::<ScanController>().peek_burst().is_none())
    {
        app.state::<ScanController>()
            .request(ScanTrigger::WatcherOverflow);
    }
    tasks.abort_all();
    fingerprints.abort_all();
    while fingerprints.join_next().await.is_some() {}
    for pending in fingerprint_keys.values() {
        if let Err(error) = writer
            .store
            .retry_source_fingerprint(pending, writer.io.now(), true)
        {
            writer.fail(error);
        }
    }
    for pending in &deferred_fingerprints {
        if let Err(error) = writer
            .store
            .defer_source_fingerprint_retry(pending, writer.io.now())
        {
            writer.fail(error);
        }
    }
    for (_, owner) in owners {
        queue.release(&owner.key, owner.generation);
    }
    let mut records = Vec::new();
    let mut changed = Vec::new();
    let mut rejected = Vec::new();
    for (key, outcome) in &writer.outcomes {
        if !finite_keys.contains(key) {
            continue;
        }
        match outcome {
            AttemptOutcome::Accepted(record, refreshed) => {
                records.push((**record).clone());
                if *refreshed {
                    changed.push(key.clone());
                }
            }
            AttemptOutcome::Rejected => rejected.push(key.clone()),
            AttemptOutcome::Failed => {}
        }
    }
    records.sort_by(|left, right| left.key.cmp(&right.key));
    changed.sort();
    rejected.sort();
    Described {
        records,
        rejected,
        changed,
        list_changed: writer.list_changed,
        gate,
    }
}

fn spawn_live_hints(
    hints: &mut JoinSet<BTreeSet<AgentKind>>,
    app: AppHandle,
    burst: Option<watch::WatchBurst>,
    sender: mpsc::Sender<DiscoveryBatch>,
) {
    hints.spawn(async move {
        let store = app.state::<Store>();
        let home = home_dir().unwrap_or_default();
        let work = burst
            .as_ref()
            .map(|burst| {
                scoped::classify_burst(&burst.paths, &home, &|labels| {
                    store
                        .native_file_session_activity_keys(labels)
                        .unwrap_or_default()
                })
            })
            .unwrap_or_default();
        report_touched(
            &app,
            &store,
            &mut AnonymousLedger::default(),
            &scoped::ScopedWork {
                sessions: work.sessions.clone(),
                ..Default::default()
            },
        )
        .await;
        for key in &work.sessions {
            let record = match store.session_record_by_activity_key(key) {
                Ok(Some(record)) => record,
                Ok(None) => continue,
                Err(error) => {
                    ::tracing::warn!(event = "scan_live_source_lookup_failed", error = %error);
                    continue;
                }
            };
            let Some(agent) = AgentKind::from_slug(&record.key.agent) else {
                continue;
            };
            let log = SessionLog {
                agent_type: agent,
                source: SessionSource::File(PathBuf::from(record.source_label)),
                updated_at: record.updated_at_epoch,
                environment: DiscoveryEnvironment::Native,
            };
            if sender
                .send(DiscoveryBatch {
                    agent,
                    candidates: vec![log],
                    complete: false,
                })
                .await
                .is_err()
            {
                return BTreeSet::new();
            }
        }
        if let Some(burst) = burst {
            for log in scoped::direct_live_logs(&burst.paths, &home, &work.sessions) {
                if sender
                    .send(DiscoveryBatch {
                        agent: log.agent_type,
                        candidates: vec![log],
                        complete: false,
                    })
                    .await
                    .is_err()
                {
                    return BTreeSet::new();
                }
            }
        }
        work.agents
            .into_iter()
            .chain(work.db_agents)
            .chain(work.quiet_agents)
            .chain(work.title_agents)
            .collect()
    });
}

fn spawn_agent_probe(
    probes: &mut JoinSet<AgentKind>,
    probing: &mut BTreeSet<AgentKind>,
    probe_agents: &mut HashMap<Id, AgentKind>,
    agent: AgentKind,
    sender: mpsc::Sender<DiscoveryBatch>,
) {
    probing.insert(agent);
    let handle = probes.spawn(async move {
        Explorers::DISK
            .discover_recent_batches_for_agents(
                &[agent],
                unix_now(),
                CURRENT_WINDOW_SECS,
                BATCH_SIZE,
                sender,
            )
            .await;
        agent
    });
    probe_agents.insert(handle.id(), agent);
}

#[cfg(test)]
#[path = "tests_owner.rs"]
mod tests;

#[cfg(test)]
#[path = "owner_v2_tests.rs"]
mod v2_tests;
