use super::*;
use antiburn_cloud_capture::CaptureIdentity;

fn tracker() -> BackfillProgress {
    BackfillProgress {
        account: "account".into(),
        destination: "personal".into(),
        resource: Some("https://example.test".into()),
        revision: "revision".into(),
        grant_id: "grant".into(),
        sources: BTreeMap::new(),
        transfers: BTreeMap::new(),
        decisions: BTreeMap::new(),
        last_report: None,
        reported_complete: false,
        incomplete: false,
        finished: false,
        background: false,
    }
}

fn job(source: &str, capture: &str, observation: &str) -> Job {
    Job {
        identity: CaptureIdentity {
            account_id: "account".into(),
            destination_id: "personal".into(),
            source_id: source.into(),
            capture_id: capture.into(),
        },
        resource_origin: Some("https://example.test".into()),
        issuer_origin: Some("https://example.test".into()),
        team_grant_id: None,
        membership_generation: None,
        source_scope_id: "scope".into(),
        project_path: "/synthetic/project".into(),
        project_folders: Vec::new(),
        repository_origin: None,
        recorded_project_paths: None,
        observation_key: observation.into(),
        state: JobState::Queued,
        reservation_retry_at: None,
    }
}

fn source(progress: &mut BackfillProgress, id: &str, observation: &str, uploaded: bool) {
    progress.sources.insert(
        id.into(),
        Source {
            observation: observation.into(),
            folder_group_id: None,
            uploaded,
            problem: None,
            refresh_needed: false,
        },
    );
}

fn bytes(sent: u64, total: u64) -> TransferProgress {
    TransferProgress {
        negotiated: true,
        total_compressed_bytes: total + 100,
        required_transfer_bytes: total,
        reused_bytes: 100,
        confirmed_upload_bytes: sent,
    }
}

fn assert_transfer(progress: &BackfillProgress, sent: u64, total: Option<u64>) {
    assert_eq!(
        progress.transfer_status(false),
        Some(ScanTransfer {
            sent_bytes: sent,
            total_bytes: total,
        })
    );
}

#[test]
fn backfill_transfer_empty_and_previously_accepted_sources_need_zero_bytes() {
    let mut progress = tracker();
    assert_transfer(&progress, 0, None);
    progress.finish();
    assert_transfer(&progress, 0, Some(0));
    source(&mut progress, "scope/accepted", "version", true);
    let mut accepted = job("scope/accepted", "old-capture", "version");
    accepted.state = JobState::Accepted;
    progress.observe_transfer(&accepted, &bytes(100, 100), true);
    assert_transfer(&progress, 0, Some(0));
    source(&mut progress, "scope/reused", "version", false);
    progress.observe_transfer(
        &job("scope/reused", "reuse-capture", "version"),
        &bytes(0, 0),
        false,
    );
    assert_transfer(&progress, 0, Some(0));
}

#[test]
fn backfill_transfer_waits_for_discovery_and_every_required_plan() {
    let mut progress = tracker();
    source(&mut progress, "scope/first", "first-version", false);
    source(&mut progress, "scope/second", "second-version", false);
    let first = job("scope/first", "capture-first", "first-version");
    let second = job("scope/second", "capture-second", "second-version");
    progress.observe_transfer(&first, &bytes(20, 100), false);
    assert_transfer(&progress, 20, None);
    progress.finish();
    assert_transfer(&progress, 20, None);
    progress.observe_transfer(&second, &TransferProgress::default(), false);
    assert_transfer(&progress, 20, None);
    progress.observe_transfer(&second, &bytes(50, 200), false);
    assert_transfer(&progress, 70, Some(300));
    assert_eq!(progress.transfer_status(true).unwrap().total_bytes, None);
}

#[test]
fn backfill_transfer_repeated_snapshots_replace_expired_receipts() {
    let mut progress = tracker();
    source(&mut progress, "scope/session", "version", false);
    let upload = job("scope/session", "capture", "version");
    progress.finish();
    for _ in 0..3 {
        progress.observe_transfer(&upload, &bytes(80, 100), false);
    }
    assert_transfer(&progress, 80, Some(100));
    assert_eq!(progress.transfers.len(), 1);
    progress.observe_transfer(&upload, &bytes(20, 100), false);
    assert_transfer(&progress, 20, Some(100));
}

#[test]
fn backfill_transfer_retains_acceptance_before_source_discovery() {
    let mut progress = tracker();
    let upload = job("scope/session", "capture", "version");
    progress.observe_transfer(&upload, &bytes(100, 100), true);
    assert_transfer(&progress, 0, None);
    source(&mut progress, "scope/session", "version", true);
    progress.finish();
    progress.observe_transfer(&upload, &bytes(100, 100), true);
    assert_transfer(&progress, 100, Some(100));
    assert_eq!(progress.transfers.len(), 1);
}

#[test]
fn backfill_transfer_changed_versions_need_a_current_plan_or_receipt() {
    let mut progress = tracker();
    source(&mut progress, "scope/session", "old", true);
    let old = job("scope/session", "old-capture", "old");
    progress.observe_transfer(&old, &bytes(100, 100), true);
    progress.finish();
    source(&mut progress, "scope/session", "new", false);
    assert_transfer(&progress, 100, None);
    let new = job("scope/session", "new-capture", "new");
    progress.observe_transfer(&new, &bytes(20, 200), false);
    assert_transfer(&progress, 120, Some(300));
    progress.refresh_changed_source("scope/session");
    assert_transfer(&progress, 120, None);
    progress.finish_refresh();
    assert_transfer(&progress, 120, None);
}

#[test]
fn backfill_transfer_completed_scan_excludes_later_background_versions() {
    let mut progress = tracker();
    source(&mut progress, "scope/session", "old", true);
    progress.observe_transfer(
        &job("scope/session", "old-capture", "old"),
        &bytes(100, 100),
        true,
    );
    progress.finish();
    progress.observe_transfer(
        &job("scope/session", "background-capture", "new"),
        &bytes(200, 200),
        true,
    );
    assert_transfer(&progress, 100, Some(100));
    assert_eq!(progress.transfers.len(), 1);
}

#[test]
fn backfill_transfer_exclusion_and_suppression_remove_source_contributions() {
    let mut progress = tracker();
    source(&mut progress, "scope/excluded", "version", true);
    source(&mut progress, "scope/deleted", "version", true);
    let excluded = job("scope/excluded", "first-capture", "version");
    let deleted = job("scope/deleted", "second-capture", "version");
    progress.observe_transfer(&excluded, &bytes(100, 100), true);
    progress.observe_transfer(&deleted, &bytes(200, 200), true);
    progress.finish();
    assert_transfer(&progress, 300, Some(300));
    progress.observe_exclusion("scope/excluded");
    assert_transfer(&progress, 200, Some(200));
    progress.observe_suppression(&deleted);
    assert_transfer(&progress, 0, Some(0));
    assert!(progress.transfers.is_empty());
}

#[test]
fn backfill_transfer_bounds_acceptance_before_discovery_and_marks_gaps() {
    let mut progress = tracker();
    for index in 0..=MAX_TRACKED_SOURCES {
        let upload = job(
            &format!("scope/source-{index}"),
            &format!("capture-{index}"),
            "version",
        );
        progress.observe_transfer(&upload, &bytes(100, 100), true);
    }
    assert_eq!(progress.transfers.len(), MAX_TRACKED_SOURCES);
    assert!(progress.incomplete);
    source(&mut progress, "scope/source-0", "version", true);
    progress.finish();
    assert_eq!(progress.transfers.len(), 1);
    assert_transfer(&progress, 100, None);
}

#[test]
fn backfill_transfer_rejects_other_bindings_and_hides_unknown_storage() {
    let mut progress = tracker();
    source(&mut progress, "scope/session", "version", true);
    let upload = job("scope/session", "capture", "version");
    let mut other = upload.clone();
    other.identity.account_id = "other-account".into();
    progress.observe_transfer(&other, &bytes(100, 100), true);
    other = upload.clone();
    other.identity.destination_id = "other-workspace".into();
    progress.observe_transfer(&other, &bytes(100, 100), true);
    other = upload.clone();
    other.resource_origin = Some("https://other.example.test".into());
    progress.observe_transfer(&other, &bytes(100, 100), true);
    assert!(progress.transfers.is_empty());
    progress.observe_transfer(&upload, &bytes(100, 100), true);
    progress.finish();
    let status = Status {
        storage_known: true,
        account_id: Some("account".into()),
        destination_id: Some("personal".into()),
        policy_revision: Some("revision".into()),
        ..Status::default()
    };
    assert_eq!(
        serde_json::to_value(progress.status(&status, false)).unwrap()["transfer"],
        serde_json::json!({ "sentBytes": 100, "totalBytes": 100 })
    );
    for unknown in [
        Status {
            storage_known: false,
            ..status.clone()
        },
        Status {
            policy_revision: Some("other-revision".into()),
            ..status.clone()
        },
        Status {
            paused: true,
            ..status.clone()
        },
        Status {
            status: State::Offline,
            ..status
        },
    ] {
        assert!(progress.status(&unknown, false).is_none());
    }
}
