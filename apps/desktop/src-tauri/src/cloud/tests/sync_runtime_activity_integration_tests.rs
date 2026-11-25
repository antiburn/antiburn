use super::*;

#[test]
fn cloud_scheduler_offline_transfer_retry_stays_active_but_offline_account_does_not() {
    for full_sync in [false, true] {
        let mut fixture = Fixture::new(PART_OFFLINE, 0);
        if full_sync {
            fixture.schedule_backfill();
        }
        let mut observed = Vec::new();
        fixture.turn_published(&mut observed);
        fixture.turn_published(&mut observed);
        let delayed = observed.last().unwrap();
        assert_eq!(delayed.status, State::Offline);
        assert_eq!(delayed.automatic_recovery_count, Some(1));
        assert_eq!(delayed.full_sync_in_progress, full_sync);
        assert_eq!(delayed.background_sync_in_progress, !full_sync);
        assert!(
            fixture
                .runner
                .retry_deadlines
                .values()
                .any(|(deadline, reason)| {
                    *deadline > Instant::now() && *reason == upload::Failure::Offline
                })
        );
        fixture.engine.status = AuthStatus::Offline;
        let account_unavailable = fixture.published_status();
        assert!(!account_unavailable.full_sync_in_progress);
        assert!(!account_unavailable.background_sync_in_progress);
    }
}

#[test]
fn cloud_scheduler_unsupported_saved_format_keeps_other_uploads_active_until_a_service_outage() {
    let mut fixture = Fixture::new(CODEX_AND_PI, 0);
    let root = fixture
        ._directory
        .path()
        .canonicalize()
        .unwrap()
        .join("codex");
    fs::create_dir(&root).unwrap();
    let id = "11111111-1111-4111-8111-111111111111";
    fs::write(
        root.join(format!("rollout-2026-11-19T12-00-00-{id}.jsonl")),
        format!(
            "{}\n",
            serde_json::json!({
                "type": "session_meta",
                "payload": {"id": id, "cwd": fixture.project, "cli_version": "1.2.3"}
            })
        ),
    )
    .unwrap();
    let codex = SourceSelection {
        source_kind: SourceKind::CodexJsonl,
        path: root.to_str().unwrap().into(),
    };
    let project = fixture.project.to_str().unwrap().to_owned();
    let selection = |source: &SourceSelection| ProjectSelection {
        source_kind: source.source_kind,
        source_path: source.path.clone(),
        project_path: project.clone(),
    };
    fixture.policy = fixture
        .store
        .save_cloud_sync_policy(
            "account",
            "personal",
            &fixture.service.origin,
            std::slice::from_ref(&codex),
            &[selection(&codex)],
            &BTreeMap::new(),
        )
        .unwrap();
    fixture.turn();
    let saved = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    let unsupported = fixture
        .store
        .cloud_sync_job(&saved.identity.capture_id)
        .unwrap()
        .unwrap();
    fixture
        .store
        .set_cloud_sync_job_state(&unsupported, JobState::Interrupted)
        .unwrap();
    let claude = SourceSelection {
        source_kind: SourceKind::ClaudeJsonl,
        path: fixture.root.to_str().unwrap().into(),
    };
    fixture.policy = fixture
        .store
        .save_cloud_sync_policy(
            "account",
            "personal",
            &fixture.service.origin,
            &[claude.clone(), codex.clone()],
            &[selection(&claude), selection(&codex)],
            &BTreeMap::new(),
        )
        .unwrap();
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.turn();
    let unsupported = fixture
        .store
        .cloud_sync_job(&saved.identity.capture_id)
        .unwrap()
        .unwrap();
    fixture
        .store
        .set_cloud_sync_job_state(&unsupported, JobState::Queued)
        .unwrap();
    let entries = fixture.host.spool().unwrap().entries().unwrap();
    assert_eq!(entries.len(), 2);
    let healthy = entries
        .iter()
        .find(|entry| entry.identity != saved.identity)
        .unwrap();
    fixture.runner.cursor = Some(healthy.identity.capture_id.clone());
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    let format_unavailable = observed.last().unwrap();
    assert_eq!(format_unavailable.status, State::UploadUnavailable);
    assert!(format_unavailable.background_sync_in_progress);
    assert!(!fixture.runner.discovery_unavailable);
    assert!(
        fixture
            .runner
            .unsupported_saved_uploads
            .contains(&saved.identity.capture_id)
    );
    fixture.runner.cursor = Some(healthy.identity.capture_id.clone());
    fixture.runner.retry_deadlines.clear();
    fixture.service.mode.store(CAPS_MISSING, Ordering::Release);
    fixture.turn_published(&mut observed);
    let service_unavailable = observed.last().unwrap();
    assert_eq!(service_unavailable.status, State::UploadUnavailable);
    assert!(fixture.runner.discovery_unavailable);
    assert!(!service_unavailable.background_sync_in_progress);
    assert!(!service_unavailable.full_sync_in_progress);
    assert_eq!(fixture.host.spool().unwrap().entries().unwrap().len(), 2);
}
