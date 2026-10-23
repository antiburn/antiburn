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
const UNAVAILABLE_FINGERPRINT: &str = "unavailable";

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StateFile {
    schema_version: u32,
    sessions: Vec<SessionState>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionState {
    environment_key: String,
    agent: String,
    session_id: String,
    last_checked_at_epoch: u64,
    checks: BTreeMap<String, CheckState>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckState {
    status: String,
    fingerprint: String,
    last_triggered: Option<TriggeredCheck>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TriggeredCheck {
    status: String,
    fingerprint: String,
    at_epoch: u64,
}

pub(crate) fn return_new_findings(
    response: &mut ChecksResponse,
    state_path: &Path,
) -> Result<(), String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before the Unix epoch: {error}"))?
        .as_secs();
    let parent = usable_parent(state_path);
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "could not create Antiburn CLI state directory {}: {error}",
            parent.display()
        )
    })?;

    let lock_path = state_lock_path(state_path);
    let _lock = OpenOptions::new()
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
    _lock.lock().map_err(|error| {
        format!(
            "could not lock Antiburn CLI state {}: {error}",
            lock_path.display()
        )
    })?;

    let mut state = read_state(state_path)?;
    let session_index = state.sessions.iter().position(|session| {
        session.environment_key == response.identity.environment_key
            && session.agent == response.identity.agent
            && session.session_id == response.identity.session_id
    });
    if let Some(index) = session_index {
        let mut session = state.sessions.remove(index);
        update_session(&mut session, response, now)?;
        state.sessions.push(session);
    } else {
        let mut session = SessionState {
            environment_key: response.identity.environment_key.clone(),
            agent: response.identity.agent.clone(),
            session_id: response.identity.session_id.clone(),
            last_checked_at_epoch: now,
            checks: BTreeMap::new(),
        };
        update_session(&mut session, response, now)?;
        state.sessions.push(session);
    }
    state.schema_version = STATE_SCHEMA_VERSION;
    state.sessions.sort_by(|left, right| {
        (&left.environment_key, &left.agent, &left.session_id).cmp(&(
            &right.environment_key,
            &right.agent,
            &right.session_id,
        ))
    });
    write_state(state_path, &state)?;
    Ok(())
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
        seen.insert(key);
    }

    for (key, previous) in &mut session.checks {
        if !seen.contains(key) {
            previous.status = "unavailable".to_owned();
            previous.fingerprint = UNAVAILABLE_FINGERPRINT.to_owned();
        }
    }
    session.last_checked_at_epoch = now;
    response.checks = new_findings;
    Ok(())
}

fn is_problematic(check: &CheckAssessment) -> bool {
    match check.source {
        CheckAssessmentSource::SessionEvidenceBadge | CheckAssessmentSource::PredefinedYaml => {
            check.status == "finding"
        }
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
    };
    format!("{source}:{}", check.check_id)
}

fn check_fingerprint(check: &CheckAssessment) -> Result<String, String> {
    let payload = match check.source {
        CheckAssessmentSource::SessionEvidenceBadge | CheckAssessmentSource::PredefinedYaml => {
            serde_json::to_vec(&(check.source, &check.check_id, &check.status, &check.result))
        }
        CheckAssessmentSource::BurnCheckAssessment => {
            let persisted = check.persisted.as_deref();
            serde_json::to_vec(&(
                check.source,
                &check.check_id,
                &check.status,
                persisted.map(|assessment| {
                    (
                        &assessment.input_revision,
                        &assessment.evaluator_revision,
                        &assessment.result_revision,
                        assessment.result_matches_input_revision,
                        assessment.source_generation,
                        &assessment.source_fingerprint,
                        assessment.published_fence,
                    )
                }),
                &check.result,
            ))
        }
    }
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

    if let Err(error) = fs::rename(&temporary_path, path) {
        if path.exists() {
            fs::remove_file(path).map_err(|remove_error| {
                format!(
                    "could not replace Antiburn CLI state {}: {remove_error}",
                    path.display()
                )
            })?;
            fs::rename(&temporary_path, path).map_err(|rename_error| {
                format!(
                    "could not install Antiburn CLI state {} after rename error ({error}): {rename_error}",
                    path.display()
                )
            })?;
        } else {
            return Err(format!(
                "could not install Antiburn CLI state {}: {error}",
                path.display()
            ));
        }
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
mod tests {
    use std::fs;

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use super::return_new_findings;
    use crate::checks::model::{
        CheckAssessment, CheckAssessmentSource, ChecksResponse, EvidenceRecord, SessionIdentity,
    };

    fn response(checks: Vec<CheckAssessment>, session_id: &str) -> ChecksResponse {
        ChecksResponse {
            session_id: session_id.to_owned(),
            identity: SessionIdentity {
                environment_key: "native".to_owned(),
                agent: "pi".to_owned(),
                session_id: session_id.to_owned(),
            },
            freshness_evaluated: false,
            evidence: EvidenceRecord {
                status: "ready".to_owned(),
                analyzed_generation: Some(4),
                processed_fingerprint: Some("fingerprint".to_owned()),
                parser_revision: Some(1),
                analyzer_revision: Some(2),
                evidence_schema_revision: Some(3),
                published_fence: Some(5),
                data: Some(json!({"private": "evidence payload"})),
                diagnostics: None,
            },
            checks,
        }
    }

    fn badge(status: &str, result: Value) -> CheckAssessment {
        CheckAssessment {
            source: CheckAssessmentSource::SessionEvidenceBadge,
            check_id: "sessions_over_depth".to_owned(),
            status: status.to_owned(),
            persisted: None,
            result: Some(result),
        }
    }

    fn persisted(status: &str, result: Option<Value>) -> CheckAssessment {
        use crate::checks::model::PersistedAssessmentMetadata;

        CheckAssessment {
            source: CheckAssessmentSource::BurnCheckAssessment,
            check_id: "ignored_instructions".to_owned(),
            status: status.to_owned(),
            persisted: Some(Box::new(PersistedAssessmentMetadata {
                input_revision: Some("input".to_owned()),
                evaluator_revision: Some("evaluator".to_owned()),
                result_revision: Some("input".to_owned()),
                result_matches_input_revision: Some(true),
                incarnation: 1,
                boundary_generation: Some(4),
                boundary_activity_cursor: Some("cursor".to_owned()),
                boundary_at_epoch: Some(10),
                source_generation: Some(4),
                source_fingerprint: Some("fingerprint".to_owned()),
                published_fence: Some(5),
                request_count: 1,
                created_at_epoch: 10,
                updated_at_epoch: 11,
                last_error_category: None,
                progress: json!({}),
            })),
            result,
        }
    }

    #[test]
    fn returns_a_finding_once_and_tracks_the_last_check_time_and_trigger() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("cli-state.json");
        let finding = badge("finding", json!({"outcome": "finding"}));
        let mut first = response(vec![finding], "session-1");
        return_new_findings(&mut first, &path).unwrap();
        assert_eq!(first.checks.len(), 1);

        let mut second = response(
            vec![badge("finding", json!({"outcome": "finding"}))],
            "session-1",
        );
        return_new_findings(&mut second, &path).unwrap();
        assert!(second.checks.is_empty());

        let contents = fs::read_to_string(path).unwrap();
        let state: Value = serde_json::from_str(&contents).unwrap();
        assert!(
            state["sessions"][0]["last_checked_at_epoch"]
                .as_u64()
                .is_some()
        );
        assert_eq!(
            state["sessions"][0]["checks"]["session_evidence_badge:sessions_over_depth"]["last_triggered"]
                ["status"],
            "finding"
        );
        assert!(!contents.contains("private evidence payload"));
    }

    #[test]
    fn changed_findings_and_resolved_then_retriggered_findings_are_new() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("state.json");
        let mut first = response(
            vec![badge("finding", json!({"outcome": "finding", "detail": 1}))],
            "session-1",
        );
        return_new_findings(&mut first, &path).unwrap();

        let mut changed = response(
            vec![badge("finding", json!({"outcome": "finding", "detail": 2}))],
            "session-1",
        );
        return_new_findings(&mut changed, &path).unwrap();
        assert_eq!(changed.checks.len(), 1);

        let mut resolved = response(
            vec![badge("clean", json!({"outcome": "clean"}))],
            "session-1",
        );
        return_new_findings(&mut resolved, &path).unwrap();
        assert!(resolved.checks.is_empty());

        let mut retriggered = response(
            vec![badge("finding", json!({"outcome": "finding", "detail": 2}))],
            "session-1",
        );
        return_new_findings(&mut retriggered, &path).unwrap();
        assert_eq!(retriggered.checks.len(), 1);
    }

    #[test]
    fn evidence_snapshot_changes_do_not_repeat_an_unchanged_badge_result() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("state.json");
        let mut first = response(
            vec![badge("finding", json!({"outcome": "finding"}))],
            "session-1",
        );
        return_new_findings(&mut first, &path).unwrap();

        let mut changed_evidence = response(
            vec![badge("finding", json!({"outcome": "finding"}))],
            "session-1",
        );
        changed_evidence.evidence.processed_fingerprint = Some("new-evidence".to_owned());
        return_new_findings(&mut changed_evidence, &path).unwrap();
        assert!(changed_evidence.checks.is_empty());
    }

    #[test]
    fn persisted_metadata_refresh_does_not_retrigger_an_unchanged_finding() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("state.json");
        let mut first = response(
            vec![persisted("completed", Some(json!({"findings": ["a"]})))],
            "session-1",
        );
        return_new_findings(&mut first, &path).unwrap();

        let mut refreshed = response(
            vec![persisted("completed", Some(json!({"findings": ["a"]})))],
            "session-1",
        );
        let metadata = refreshed.checks[0].persisted.as_mut().unwrap();
        metadata.request_count += 1;
        metadata.updated_at_epoch += 10;
        metadata.progress = json!({"phase": "still completed"});
        return_new_findings(&mut refreshed, &path).unwrap();
        assert!(refreshed.checks.is_empty());
    }

    #[test]
    fn only_completed_persisted_findings_are_returned() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("state.json");
        let mut response = response(
            vec![
                persisted("completed", Some(json!({"findings": ["a"]}))),
                persisted("completed", Some(json!({"findings": []}))),
                persisted("running", Some(json!({"findings": ["b"]}))),
            ],
            "session-1",
        );
        return_new_findings(&mut response, &path).unwrap();
        assert_eq!(response.checks.len(), 1);
        assert_eq!(response.checks[0].check_id, "ignored_instructions");
    }

    #[test]
    fn state_is_scoped_to_the_composite_session_identity_and_check_source() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("state.json");
        let mut first = response(
            vec![badge("finding", json!({"outcome": "finding"}))],
            "session-1",
        );
        return_new_findings(&mut first, &path).unwrap();

        let mut second_identity = response(
            vec![badge("finding", json!({"outcome": "finding"}))],
            "session-2",
        );
        return_new_findings(&mut second_identity, &path).unwrap();
        assert_eq!(second_identity.checks.len(), 1);

        let mut duplicate_source = response(
            vec![
                badge("finding", json!({"outcome": "finding"})),
                persisted("completed", Some(json!({"findings": ["a"]}))),
            ],
            "session-1",
        );
        return_new_findings(&mut duplicate_source, &path).unwrap();
        assert_eq!(duplicate_source.checks.len(), 1);
        assert_eq!(
            duplicate_source.checks[0].source,
            CheckAssessmentSource::BurnCheckAssessment
        );
    }

    #[test]
    fn invalid_state_is_not_overwritten() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("state.json");
        fs::write(&path, "not json").unwrap();
        let mut response = response(
            vec![badge("finding", json!({"outcome": "finding"}))],
            "session-1",
        );
        assert!(return_new_findings(&mut response, &path).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "not json");
    }
}
