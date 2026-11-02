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

fn yaml(status: &str) -> CheckAssessment {
    CheckAssessment {
        source: CheckAssessmentSource::PredefinedYaml,
        check_id: "retry-loop".to_owned(),
        status: status.to_owned(),
        persisted: None,
        result: Some(
            json!({"id":"retry-loop:r1", "name":"Failed retry loop", "message":"Review recent attempts."}),
        ),
    }
}

#[test]
fn yaml_unknown_availability_preserves_deduplication_and_clean_rearms() {
    let temp = tempdir().unwrap();
    let path = temp.path().join("state.json");
    let mut first = response(vec![yaml("finding")], "session-1");
    return_new_findings(&mut first, &path).unwrap();
    assert_eq!(first.checks.len(), 1);
    for checks in [vec![], vec![yaml("not_assessed")]] {
        return_new_findings(&mut response(checks, "session-1"), &path).unwrap();
        let mut same = response(vec![yaml("finding")], "session-1");
        return_new_findings(&mut same, &path).unwrap();
        assert!(same.checks.is_empty());
    }
    return_new_findings(&mut response(vec![yaml("clean")], "session-1"), &path).unwrap();
    let mut recurrence = response(vec![yaml("finding")], "session-1");
    return_new_findings(&mut recurrence, &path).unwrap();
    assert_eq!(recurrence.checks.len(), 1);
}

fn price_outlier(status: &str, cost_usd: f64) -> CheckAssessment {
    CheckAssessment {
        source: CheckAssessmentSource::SessionPriceOutlier,
        check_id: "outlier_priced_session".to_owned(),
        status: status.to_owned(),
        persisted: None,
        result: Some(json!({
            "outcome": status,
            "estimated_cost_usd": cost_usd,
            "threshold_usd": 3.0,
            "priced_session_count": 8
        })),
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
fn price_outlier_ignores_metrics_and_retriggers_after_resolution() {
    let temp = tempdir().unwrap();
    let path = temp.path().join("state.json");
    let mut first = response(vec![price_outlier("finding", 4.0)], "session-1");
    return_new_findings(&mut first, &path).unwrap();
    assert_eq!(first.checks.len(), 1);
    assert_eq!(
        first.checks[0].source,
        CheckAssessmentSource::SessionPriceOutlier
    );

    let mut repeated = response(vec![price_outlier("finding", 4.0)], "session-1");
    return_new_findings(&mut repeated, &path).unwrap();
    assert!(repeated.checks.is_empty());

    let mut changed = response(vec![price_outlier("finding", 5.0)], "session-1");
    return_new_findings(&mut changed, &path).unwrap();
    assert!(changed.checks.is_empty());

    let mut resolved = response(vec![price_outlier("clean", 1.0)], "session-1");
    return_new_findings(&mut resolved, &path).unwrap();
    assert!(resolved.checks.is_empty());

    let mut retriggered = response(vec![price_outlier("finding", 5.0)], "session-1");
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
    metadata.input_revision = Some("new-input".to_owned());
    metadata.evaluator_revision = Some("new-evaluator".to_owned());
    metadata.result_revision = Some("new-result".to_owned());
    metadata.result_matches_input_revision = Some(false);
    metadata.source_generation = Some(99);
    metadata.source_fingerprint = Some("new-source".to_owned());
    metadata.published_fence = Some(99);
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

#[test]
fn unknown_availability_preserves_findings_until_confirmed_resolution() {
    for source in ["badge", "price", "persisted"] {
        let temp = tempdir().unwrap();
        let path = temp.path().join("state.json");
        let finding = match source {
            "badge" => badge("finding", json!({"outcome": "finding"})),
            "price" => price_outlier("finding", 4.0),
            _ => persisted("completed", Some(json!({"findings": ["a"]}))),
        };
        let mut first = response(vec![finding.clone()], "session-1");
        return_new_findings(&mut first, &path).unwrap();
        let mut unknowns = vec![Vec::new()];
        for status in [
            "not_assessed",
            "running",
            "failed",
            "pending",
            "unsupported",
        ] {
            let mut unknown = finding.clone();
            unknown.status = status.to_owned();
            unknowns.push(vec![unknown]);
        }
        for checks in unknowns {
            return_new_findings(&mut response(checks, "session-1"), &path).unwrap();
            let mut repeated = response(vec![finding.clone()], "session-1");
            return_new_findings(&mut repeated, &path).unwrap();
            assert!(repeated.checks.is_empty(), "{source}");
        }
        let mut clean = finding.clone();
        if source == "persisted" {
            clean.result = Some(json!({"findings": []}));
        } else {
            clean.status = "clean".to_owned();
        }
        return_new_findings(&mut response(vec![clean], "session-1"), &path).unwrap();
        return_new_findings(&mut response(Vec::new(), "session-1"), &path).unwrap();
        let mut recurrence = response(vec![finding], "session-1");
        return_new_findings(&mut recurrence, &path).unwrap();
        assert_eq!(recurrence.checks.len(), 1, "{source}");
    }
}

#[test]
fn persisted_fingerprint_tracks_finding_content_without_result_bookkeeping() {
    let temp = tempdir().unwrap();
    let path = temp.path().join("state.json");
    let finding =
        json!({"id": "a", "message": "one", "source_generation": 1, "publication_fence": 2});
    let mut first = response(
        vec![persisted("completed", Some(json!({"findings": [finding]})))],
        "session-1",
    );
    return_new_findings(&mut first, &path).unwrap();
    let mut changed = response(
        vec![persisted(
            "completed",
            Some(json!({
                "input_revision": "new", "coverage": {"count": 90}, "request_count": 5,
                "findings": [{"id": "a", "message": "one", "source_generation": 8, "publication_fence": 9}]
            })),
        )],
        "session-1",
    );
    return_new_findings(&mut changed, &path).unwrap();
    assert!(changed.checks.is_empty());
    let mut meaningful = response(
        vec![persisted(
            "completed",
            Some(json!({"findings": [{"id": "a", "message": "two"}]})),
        )],
        "session-1",
    );
    return_new_findings(&mut meaningful, &path).unwrap();
    assert_eq!(meaningful.checks.len(), 1);
}

#[test]
fn abandoned_delivery_expires_and_old_receipts_cannot_acknowledge_a_retry() {
    use super::{prepare_delivery_at, read_state};
    let temp = tempdir().unwrap();
    let path = temp.path().join("state.json");
    let make = || response(vec![price_outlier("finding", 4.0)], "session-1");
    let first = prepare_delivery_at(&mut make(), &path, 100).unwrap();
    assert!(read_state(&path).unwrap().sessions.is_empty());
    assert!(prepare_delivery_at(&mut make(), &path, 101).is_err());
    let mut retry = make();
    let second = prepare_delivery_at(&mut retry, &path, 160).unwrap();
    assert_eq!(retry.checks.len(), 1);
    assert!(first.finish_at(true, 161).is_err());
    second.finish_at(true, 161).unwrap();
    let mut repeated = make();
    prepare_delivery_at(&mut repeated, &path, 162)
        .unwrap()
        .finish_at(true, 162)
        .unwrap();
    assert!(repeated.checks.is_empty());
}

#[test]
fn concurrent_sessions_keep_both_acknowledgements_and_locks_fail_fast() {
    use super::{lock_state, prepare_delivery_at, read_state};
    let temp = tempdir().unwrap();
    let path = temp.path().join("state.json");
    let lock = lock_state(&path).unwrap();
    assert!(prepare_delivery_at(&mut response(Vec::new(), "session-1"), &path, 100).is_err());
    drop(lock);
    let first = prepare_delivery_at(&mut response(Vec::new(), "session-1"), &path, 100).unwrap();
    let second = prepare_delivery_at(&mut response(Vec::new(), "session-2"), &path, 100).unwrap();
    second.finish_at(true, 101).unwrap();
    first.finish_at(true, 102).unwrap();
    assert_eq!(read_state(&path).unwrap().sessions.len(), 2);
}

#[test]
fn atomic_state_replacement_succeeds_and_failure_preserves_the_destination() {
    use super::{read_state, write_state};
    let temp = tempdir().unwrap();
    let path = temp.path().join("state.json");
    let mut state = read_state(&path).unwrap();
    write_state(&path, &state).unwrap();
    state.schema_version = 2;
    write_state(&path, &state).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap()["schema_version"],
        2
    );
    let directory = temp.path().join("destination");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("valid-state.json"), fs::read(&path).unwrap()).unwrap();
    assert!(write_state(&directory, &state).is_err());
    assert_eq!(
        fs::read(directory.join("valid-state.json")).unwrap(),
        fs::read(&path).unwrap()
    );
}

struct FailingOutput {
    fail_flush: bool,
}

impl tokio::io::AsyncWrite for FailingOutput {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Ready(if self.fail_flush {
            Ok(bytes.len())
        } else {
            Err(std::io::ErrorKind::BrokenPipe.into())
        })
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn failed_write_or_flush_retries_and_successful_output_suppresses() {
    use super::prepare_delivery;
    use crate::checks::PendingChecks;
    let temp = tempdir().unwrap();
    let path = temp.path().join("state.json");
    for fail_flush in [false, true] {
        let mut response = response(vec![price_outlier("finding", 4.0)], "session-1");
        let delivery = prepare_delivery(&mut response, &path).unwrap();
        assert_eq!(response.checks.len(), 1);
        assert!(
            PendingChecks { response, delivery }
                .write_json(&mut FailingOutput { fail_flush })
                .await
                .is_err()
        );
    }
    let mut first = response(vec![price_outlier("finding", 4.0)], "session-1");
    let delivery = prepare_delivery(&mut first, &path).unwrap();
    let mut output = Vec::new();
    PendingChecks {
        response: first,
        delivery,
    }
    .write_json(&mut output)
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&output).unwrap()["checks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let mut repeated = response(vec![price_outlier("finding", 4.0)], "session-1");
    return_new_findings(&mut repeated, &path).unwrap();
    assert!(repeated.checks.is_empty());
}

#[test]
fn replacement_failure_keeps_existing_valid_state_and_removes_temporary_file() {
    use super::{read_state, write_state, write_state_with_rename};
    let temp = tempdir().unwrap();
    let path = temp.path().join("state.json");
    let state = read_state(&path).unwrap();
    write_state(&path, &state).unwrap();
    let original = fs::read(&path).unwrap();
    let error = write_state_with_rename(&path, &state, |temporary, destination| {
        assert!(temporary.is_file());
        assert_eq!(destination, path);
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "replacement denied",
        ))
    })
    .unwrap_err();
    assert!(error.contains("replacement denied"));
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(read_state(&path).is_ok());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[test]
fn persisted_source_types_ignore_provenance_and_preserve_meaningful_finding_changes() {
    use super::check_fingerprint;
    for (check_id, finding) in [
        (
            "ignored_instructions",
            json!({"id": "a", "reference": {"rule_id": "r", "action_id": "action"}, "decision": {"source_generation": 1, "source_fingerprint": "one", "publication_fence": 1, "context_revision": "one", "evaluator_revision": "one", "model": "one", "citations": ["proof"]}}),
        ),
        (
            "scope_creep",
            json!({"id": "a", "work": ["work"], "task_scope": ["task"], "source_generation": 1, "publication_fence": 1}),
        ),
        (
            "over_exploring",
            json!({"episode_id": "a", "reads": ["read"], "reason": "repeated_read", "model": "one", "revisions": {"reducer": 1}}),
        ),
        (
            "skill_opportunities",
            json!({"comparison": {"id": "a", "skill": "skill"}, "message": "Use the skill", "model": "one", "revisions": {"reducer": 1}}),
        ),
    ] {
        let mut check = persisted("completed", Some(json!({"findings": [finding]})));
        check.check_id = check_id.to_owned();
        let fingerprint = check_fingerprint(&check).unwrap();
        let item = &mut check.result.as_mut().unwrap()["findings"][0];
        item["model"] = json!("two");
        item["revisions"] = json!({"reducer": 2});
        item["source_generation"] = json!(2);
        item["publication_fence"] = json!(2);
        if check_id == "ignored_instructions" {
            for field in [
                "source_generation",
                "source_fingerprint",
                "publication_fence",
                "context_revision",
                "evaluator_revision",
                "model",
            ] {
                item["decision"][field] = json!("two");
            }
        }
        assert_eq!(
            fingerprint,
            check_fingerprint(&check).unwrap(),
            "{check_id}"
        );
        check.result.as_mut().unwrap()["findings"][0]["new_evidence"] = json!("new action");
        assert_ne!(
            fingerprint,
            check_fingerprint(&check).unwrap(),
            "{check_id}"
        );
    }
}

#[test]
fn finding_order_does_not_repeat_but_added_findings_do() {
    use super::check_fingerprint;
    let mut check = persisted("completed", Some(json!({"findings": ["a", "b"]})));
    let fingerprint = check_fingerprint(&check).unwrap();
    check.result = Some(json!({"findings": ["b", "a"]}));
    assert_eq!(fingerprint, check_fingerprint(&check).unwrap());
    check.result = Some(json!({"findings": ["b", "a", "c"]}));
    assert_ne!(fingerprint, check_fingerprint(&check).unwrap());
}

#[cfg(windows)]
#[test]
fn windows_replacement_denial_preserves_an_existing_state_file() {
    use super::{read_state, write_state};
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempdir().unwrap();
    let path = temp.path().join("state.json");
    let state = read_state(&path).unwrap();
    write_state(&path, &state).unwrap();
    let original = fs::read(&path).unwrap();
    let guard = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    assert!(write_state(&path, &state).is_err());
    drop(guard);
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[tokio::test]
async fn failed_output_preserves_resolution_and_retries_undelivered_findings() {
    use super::prepare_delivery;
    use crate::checks::PendingChecks;

    for finding in [
        badge("finding", json!({"outcome": "finding"})),
        price_outlier("finding", 4.0),
        yaml("finding"),
        persisted("completed", Some(json!({"findings": ["a"]}))),
    ] {
        for fail_flush in [false, true] {
            let temp = tempdir().unwrap();
            let path = temp.path().join("state.json");
            return_new_findings(&mut response(vec![finding.clone()], "session-1"), &path).unwrap();
            let mut clean = finding.clone();
            if clean.source == CheckAssessmentSource::BurnCheckAssessment {
                clean.result = Some(json!({"findings": []}));
            } else {
                clean.status = "clean".to_owned();
                clean.result = Some(json!({"outcome": "clean"}));
            }
            let mut undelivered = finding.clone();
            undelivered.check_id = "another_check".to_owned();
            let mut assessment = response(vec![clean, undelivered.clone()], "session-1");
            let delivery = prepare_delivery(&mut assessment, &path).unwrap();
            assert_eq!(assessment.checks.len(), 1);
            assert!(
                PendingChecks {
                    response: assessment,
                    delivery
                }
                .write_json(&mut FailingOutput { fail_flush })
                .await
                .is_err()
            );
            let mut recurrence = response(vec![finding.clone(), undelivered], "session-1");
            return_new_findings(&mut recurrence, &path).unwrap();
            assert_eq!(
                recurrence.checks.len(),
                2,
                "{:?}, flush={fail_flush}",
                finding.source
            );
        }
    }
}

#[test]
fn abandoned_clean_delivery_preserves_resolution_after_the_lease_expires() {
    use super::prepare_delivery_at;

    let temp = tempdir().unwrap();
    let path = temp.path().join("state.json");
    let finding = price_outlier("finding", 4.0);
    prepare_delivery_at(
        &mut response(vec![finding.clone()], "session-1"),
        &path,
        100,
    )
    .unwrap()
    .finish_at(true, 101)
    .unwrap();
    let abandoned = prepare_delivery_at(
        &mut response(vec![price_outlier("clean", 1.0)], "session-1"),
        &path,
        102,
    )
    .unwrap();
    let mut recurrence = response(vec![finding], "session-1");
    let retry = prepare_delivery_at(&mut recurrence, &path, 162).unwrap();
    assert_eq!(recurrence.checks.len(), 1);
    assert!(abandoned.finish_at(true, 163).is_err());
    retry.finish_at(true, 163).unwrap();
}
