use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::model::{CheckAssessment, CheckAssessmentSource, ChecksResponse};

const STATE_SCHEMA_VERSION: u32 = 1;
const DELIVERY_LEASE_SECONDS: u64 = 60;

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StateFile {
    schema_version: u32,
    sessions: Vec<SessionState>,
    #[serde(default)]
    pending: Vec<PendingDelivery>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionState {
    environment_key: String,
    agent: String,
    session_id: String,
    last_checked_at_epoch: u64,
    checks: BTreeMap<String, CheckState>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckState {
    status: String,
    fingerprint: String,
    last_triggered: Option<TriggeredCheck>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TriggeredCheck {
    status: String,
    fingerprint: String,
    at_epoch: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PendingDelivery {
    token: String,
    expires_at_epoch: u64,
    session: SessionState,
}

#[derive(Debug)]
pub struct Delivery {
    state_path: PathBuf,
    token: String,
}

impl Delivery {
    pub async fn finish(self, delivered: bool) -> Result<(), String> {
        tokio::task::spawn_blocking(move || self.finish_at(delivered, epoch_seconds()?))
            .await
            .map_err(|error| format!("Antiburn delivery task failed: {error}"))?
    }

    fn finish_at(self, delivered: bool, now: u64) -> Result<(), String> {
        let _lock = lock_state(&self.state_path)?;
        let mut state = read_state(&self.state_path)?;
        let index = state
            .pending
            .iter()
            .position(|pending| pending.token == self.token)
            .ok_or_else(|| "Antiburn delivery reservation expired; retry the command".to_owned())?;
        let pending = state.pending.remove(index);
        if delivered {
            if pending.expires_at_epoch <= now {
                return Err("Antiburn delivery reservation expired; retry the command".to_owned());
            }
            state
                .sessions
                .retain(|session| !same_session(session, &pending.session));
            state.sessions.push(pending.session);
        }
        write_state(&self.state_path, &state)
    }
}

fn epoch_seconds() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .map_err(|error| format!("system clock is before the Unix epoch: {error}"))
}

fn same_session(left: &SessionState, right: &SessionState) -> bool {
    left.environment_key == right.environment_key
        && left.agent == right.agent
        && left.session_id == right.session_id
}

fn lock_state(state_path: &Path) -> Result<File, String> {
    let parent = usable_parent(state_path);
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "could not create Antiburn CLI state directory {}: {error}",
            parent.display()
        )
    })?;
    let lock_path = state_lock_path(state_path);
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| {
            format!(
                "could not open Antiburn CLI state lock {}: {error}",
                lock_path.display()
            )
        })?;
    lock.try_lock()
        .map_err(|error| format!("Antiburn CLI state is busy; retry the command: {error}"))?;
    Ok(lock)
}

pub(crate) fn prepare_delivery(
    response: &mut ChecksResponse,
    state_path: &Path,
) -> Result<Delivery, String> {
    prepare_delivery_at(response, state_path, epoch_seconds()?)
}

fn prepare_delivery_at(
    response: &mut ChecksResponse,
    state_path: &Path,
    now: u64,
) -> Result<Delivery, String> {
    let _lock = lock_state(state_path)?;
    let mut state = read_state(state_path)?;
    state
        .pending
        .retain(|pending| pending.expires_at_epoch > now);
    let mut session = SessionState {
        environment_key: response.identity.environment_key.clone(),
        agent: response.identity.agent.clone(),
        session_id: response.identity.session_id.clone(),
        last_checked_at_epoch: now,
        checks: BTreeMap::new(),
    };
    if state
        .pending
        .iter()
        .any(|pending| same_session(&pending.session, &session))
    {
        return Err("Antiburn session delivery is pending; retry the command".to_owned());
    }
    if let Some(index) = state
        .sessions
        .iter()
        .position(|previous| same_session(previous, &session))
    {
        session = state.sessions.remove(index);
        state.sessions.push(session.clone());
    }
    let resolved_keys: Vec<_> = response
        .checks
        .iter()
        .filter(|check| is_assessed(check) && !is_problematic(check))
        .map(check_key)
        .collect();
    let mut confirmed = session.clone();
    update_session(&mut session, response, now)?;
    if !resolved_keys.is_empty() {
        // Persist confirmed resolutions so a failed delivery cannot hide a recurrence.
        for key in resolved_keys {
            if let Some(resolved) = session.checks.get(&key) {
                confirmed.checks.insert(key, resolved.clone());
            }
        }
        state
            .sessions
            .retain(|previous| !same_session(previous, &confirmed));
        state.sessions.push(confirmed);
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before the Unix epoch: {error}"))?
        .as_nanos();
    let token = format!("{}-{nonce}", std::process::id());
    state.pending.push(PendingDelivery {
        token: token.clone(),
        expires_at_epoch: now.saturating_add(DELIVERY_LEASE_SECONDS),
        session,
    });
    write_state(state_path, &state)?;
    Ok(Delivery {
        state_path: state_path.to_owned(),
        token,
    })
}

#[cfg(test)]
fn return_new_findings(response: &mut ChecksResponse, state_path: &Path) -> Result<(), String> {
    prepare_delivery(response, state_path)?.finish_at(true, epoch_seconds()?)
}

fn update_session(
    session: &mut SessionState,
    response: &mut ChecksResponse,
    now: u64,
) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    let mut new_findings = Vec::new();
    for check in response.checks.drain(..) {
        let key = check_key(&check);
        seen.insert(key.clone());
        if !is_assessed(&check) {
            if let Some(previous) = session.checks.get_mut(&key) {
                previous.status = check.status.clone();
            }
            continue;
        }
        let fingerprint = check_fingerprint(&check)?;
        let changed = session
            .checks
            .get(&key)
            .is_none_or(|previous| previous.fingerprint != fingerprint);
        let is_problem = is_problematic(&check);
        let last_triggered = if is_problem && changed {
            new_findings.push(check.clone());
            Some(TriggeredCheck {
                status: check.status.clone(),
                fingerprint: fingerprint.clone(),
                at_epoch: now,
            })
        } else {
            session
                .checks
                .get(&key)
                .and_then(|previous| previous.last_triggered.as_ref())
                .map(|previous| TriggeredCheck {
                    status: previous.status.clone(),
                    fingerprint: previous.fingerprint.clone(),
                    at_epoch: previous.at_epoch,
                })
        };
        session.checks.insert(
            key.clone(),
            CheckState {
                status: check.status.clone(),
                fingerprint,
                last_triggered,
            },
        );
    }

    for (key, previous) in &mut session.checks {
        if !seen.contains(key) {
            previous.status = "unavailable".to_owned();
        }
    }
    session.last_checked_at_epoch = now;
    response.checks = new_findings;
    Ok(())
}

fn is_assessed(check: &CheckAssessment) -> bool {
    match check.source {
        CheckAssessmentSource::SessionEvidenceBadge
        | CheckAssessmentSource::PredefinedYaml
        | CheckAssessmentSource::SessionPriceOutlier => {
            matches!(check.status.as_str(), "finding" | "clean")
        }
        CheckAssessmentSource::BurnCheckAssessment => {
            check.status == "completed"
                && check
                    .result
                    .as_ref()
                    .and_then(|result| result.get("findings"))
                    .and_then(Value::as_array)
                    .is_some()
        }
    }
}

fn is_problematic(check: &CheckAssessment) -> bool {
    match check.source {
        CheckAssessmentSource::PredefinedYaml
        | CheckAssessmentSource::SessionEvidenceBadge
        | CheckAssessmentSource::SessionPriceOutlier => check.status == "finding",
        CheckAssessmentSource::BurnCheckAssessment => {
            check.status == "completed"
                && check
                    .result
                    .as_ref()
                    .and_then(|result| result.get("findings"))
                    .and_then(Value::as_array)
                    .is_some_and(|findings| !findings.is_empty())
        }
    }
}

fn check_key(check: &CheckAssessment) -> String {
    let source = match check.source {
        CheckAssessmentSource::BurnCheckAssessment => "burn_check_assessment",
        CheckAssessmentSource::SessionEvidenceBadge => "session_evidence_badge",
        CheckAssessmentSource::PredefinedYaml => "predefined_yaml",
        CheckAssessmentSource::SessionPriceOutlier => "session_price_outlier",
    };
    format!("{source}:{}", check.check_id)
}

fn check_fingerprint(check: &CheckAssessment) -> Result<String, String> {
    let result = match check.source {
        CheckAssessmentSource::SessionEvidenceBadge | CheckAssessmentSource::PredefinedYaml => {
            check.result.clone()
        }
        CheckAssessmentSource::SessionPriceOutlier => None,
        CheckAssessmentSource::BurnCheckAssessment => check
            .result
            .as_ref()
            .and_then(|result| result.get("findings"))
            .cloned()
            .map(|mut findings| {
                if let Some(items) = findings.as_array_mut() {
                    for item in items.iter_mut() {
                        if let Some(object) = item.as_object_mut() {
                            for key in [
                                "source_generation",
                                "publication_fence",
                                "model",
                                "model_revision",
                                "revisions",
                            ] {
                                object.remove(key);
                            }
                            if check.check_id == "ignored_instructions"
                                && let Some(decision) =
                                    object.get_mut("decision").and_then(Value::as_object_mut)
                            {
                                for key in [
                                    "source_generation",
                                    "source_fingerprint",
                                    "publication_fence",
                                    "context_revision",
                                    "evaluator_revision",
                                    "model",
                                ] {
                                    decision.remove(key);
                                }
                            }
                        }
                    }
                    items.sort_by_cached_key(Value::to_string);
                }
                findings
            }),
    };
    let payload = serde_json::to_vec(&(check.source, &check.check_id, &check.status, result))
        .map_err(|error| format!("could not fingerprint an Antiburn check: {error}"))?;
    let digest = Sha256::digest(payload);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn read_state(path: &Path) -> Result<StateFile, String> {
    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StateFile {
                schema_version: STATE_SCHEMA_VERSION,
                sessions: Vec::new(),
                pending: Vec::new(),
            });
        }
        Err(error) => {
            return Err(format!(
                "could not read Antiburn CLI state {}: {error}",
                path.display()
            ));
        }
    };
    let state: StateFile = serde_json::from_slice(&contents)
        .map_err(|error| format!("invalid Antiburn CLI state {}: {error}", path.display()))?;
    if state.schema_version != STATE_SCHEMA_VERSION {
        return Err(format!(
            "unsupported Antiburn CLI state schema version {} in {}",
            state.schema_version,
            path.display()
        ));
    }
    Ok(state)
}

fn write_state(path: &Path, state: &StateFile) -> Result<(), String> {
    write_state_with_rename(path, state, |from, to| fs::rename(from, to))
}

fn write_state_with_rename(
    path: &Path,
    state: &StateFile,
    rename: impl FnOnce(&Path, &Path) -> std::io::Result<()>,
) -> Result<(), String> {
    let contents = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("could not serialize Antiburn CLI state: {error}"))?;
    let parent = usable_parent(path);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state.json");
    let temporary_path = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let mut file = File::create(&temporary_path).map_err(|error| {
        format!(
            "could not create temporary Antiburn CLI state {}: {error}",
            temporary_path.display()
        )
    })?;
    file.write_all(&contents).map_err(|error| {
        format!(
            "could not write temporary Antiburn CLI state {}: {error}",
            temporary_path.display()
        )
    })?;
    file.sync_all().map_err(|error| {
        format!(
            "could not sync temporary Antiburn CLI state {}: {error}",
            temporary_path.display()
        )
    })?;
    drop(file);

    if let Err(error) = rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(format!(
            "could not install Antiburn CLI state {}: {error}",
            path.display()
        ));
    }
    Ok(())
}

fn state_lock_path(path: &Path) -> PathBuf {
    let mut lock_name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    lock_name.push(".lock");
    usable_parent(path).join(lock_name)
}

fn usable_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
