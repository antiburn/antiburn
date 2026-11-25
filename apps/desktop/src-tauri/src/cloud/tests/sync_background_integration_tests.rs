use super::*;

fn background(status: &Status) -> serde_json::Value {
    serde_json::to_value(status).unwrap()["backgroundProgress"].clone()
}

#[test]
fn cloud_scheduler_background_counts_acceptance_and_retains_transferred_bytes() {
    let mut fixture = Fixture::new(READY, 0);
    write_source(
        &fixture.root,
        &fixture.project,
        "second",
        "second synthetic session",
    );
    let mut observed = Vec::new();
    let mut plans = BTreeMap::new();
    for _ in 0..20 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
        for entry in fixture.host.spool().unwrap().entries().unwrap() {
            let progress = entry.progress().unwrap();
            if progress.negotiated {
                plans.insert(entry.identity.capture_id, progress.required_transfer_bytes);
            }
        }
        if background(observed.last().unwrap())["uploaded"] == 2
            && !observed.last().unwrap().background_sync_in_progress
        {
            break;
        }
    }
    assert!(observed[0].background_sync_in_progress);
    assert!(
        observed
            .iter()
            .all(|status| status.upload_progress.is_none())
    );
    assert_eq!(plans.len(), 2);
    let total: u64 = plans.values().sum();
    let completed = observed.last().unwrap();
    assert_eq!(background(completed)["uploaded"], 2);
    assert_eq!(background(completed)["total"], 2);
    let group_id = &completed.saved_folders.as_ref().unwrap()[0].group_id;
    assert_eq!(background(completed)["folderFound"][group_id], 2);
    assert_eq!(
        background(completed)["transfer"],
        serde_json::json!({ "sentBytes": total, "totalBytes": total })
    );
    assert!(!completed.background_sync_in_progress);
    assert!(!completed.full_sync_in_progress);
    assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
    for _ in 0..3 {
        assert_eq!(
            background(&fixture.published_status()),
            background(completed)
        );
    }
}

#[test]
fn cloud_scheduler_background_saved_verification_needs_no_fresh_scan_tracker() {
    let mut fixture = Fixture::new(VERIFY_PENDING, 0);
    for _ in 0..6 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn();
    }
    fixture.restart_scheduler();
    fixture.turn();
    fixture.runner.state = State::Ready;
    let status = fixture.published_status();
    assert!(status.background_sync_in_progress);
    assert!(status.background_progress.is_none());
    assert!(status.upload_progress.is_none());
    assert!(
        status
            .automatic_recovery_count
            .is_some_and(|count| count > 0)
    );
    assert!(!status.full_sync_in_progress);
}

#[test]
fn cloud_scheduler_background_ready_cursor_has_activity_without_guessed_counts() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.runner.state = State::Ready;
    fixture
        .runner
        .observe_retry_grant(fixture.engine.session().unwrap());
    fixture.runner.binding = Some((
        fixture.policy.account_id.clone(),
        fixture.policy.destination_id.clone(),
        fixture.policy.resource_origin.clone(),
        fixture.policy.revision.clone(),
    ));
    fixture.runner.discovery_cursor = Some(DiscoveryCursor::default());
    let status = fixture.published_status();
    assert!(status.background_sync_in_progress);
    assert!(status.background_progress.is_none());
    assert_eq!(status.queued_count, 0);
    assert!(!status.backfill_in_progress);
    for state in [State::Fenced, State::Interrupted, State::UploadUnavailable] {
        fixture.runner.state = state;
        assert!(fixture.published_status().background_sync_in_progress);
    }
    fixture.runner.binding.as_mut().unwrap().3 = "old-policy".into();
    assert!(!fixture.published_status().background_sync_in_progress);
}

#[test]
fn cloud_scheduler_full_sync_keeps_ownership_until_verification_finishes() {
    let mut fixture = Fixture::new(VERIFY_PENDING, 0);
    fixture.schedule_backfill();
    let mut observed = Vec::new();
    for _ in 0..6 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
    }
    let pending = observed.last().unwrap();
    assert!(!pending.backfill_in_progress);
    assert!(pending.full_sync_in_progress);
    assert!(!pending.background_sync_in_progress);
    assert!(pending.background_progress.is_none());
    assert_eq!(
        serde_json::to_value(pending).unwrap()["uploadProgress"]["uploaded"],
        0
    );
    fixture.service.mode.store(READY, Ordering::Release);
    fixture.runner.next_integrity_poll = None;
    fixture.turn_published(&mut observed);
    let completed = observed.last().unwrap();
    assert!(!completed.full_sync_in_progress);
    assert!(!completed.background_sync_in_progress);
    assert_eq!(
        serde_json::to_value(completed).unwrap()["uploadProgress"]["uploaded"],
        1
    );
}

#[test]
fn cloud_scheduler_background_new_sweep_resets_bytes_and_preserves_explicit_result() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.schedule_backfill();
    let mut observed = Vec::new();
    for _ in 0..10 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
        if serde_json::to_value(observed.last().unwrap()).unwrap()["uploadProgress"]["uploaded"]
            == 1
        {
            break;
        }
    }
    let explicit =
        serde_json::to_value(observed.last().unwrap()).unwrap()["uploadProgress"].clone();
    let group_id = saved_team_folder_id(fixture.project.to_str().unwrap());
    assert_eq!(explicit["folderFound"][&group_id], 1);
    write_source(
        &fixture.root,
        &fixture.project,
        "second",
        "second synthetic session",
    );
    fixture.runner.next_reconcile = None;
    fixture.turn_published(&mut observed);
    let next = observed.last().unwrap();
    assert!(next.background_sync_in_progress);
    assert!(!next.full_sync_in_progress);
    assert_eq!(background(next)["uploaded"], 1);
    assert_eq!(background(next)["folderFound"][&group_id], 2);
    assert_eq!(background(next)["transfer"]["sentBytes"], 0);
    assert_eq!(
        serde_json::to_value(next).unwrap()["uploadProgress"],
        explicit
    );
    for _ in 0..10 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
        if background(observed.last().unwrap())["uploaded"] == 2
            && !observed.last().unwrap().background_sync_in_progress
        {
            break;
        }
    }
    let completed = observed.last().unwrap();
    assert_eq!(background(completed)["uploaded"], 2);
    assert_eq!(background(completed)["folderFound"][&group_id], 2);
    assert!(!completed.background_sync_in_progress);
    assert_eq!(
        serde_json::to_value(completed).unwrap()["uploadProgress"],
        explicit
    );
    fixture.runner.next_reconcile = None;
    fixture.turn_published(&mut observed);
    assert_eq!(
        background(observed.last().unwrap())["transfer"],
        serde_json::json!({ "sentBytes": 0, "totalBytes": 0 })
    );
}

#[test]
fn cloud_scheduler_background_errors_and_pause_do_not_claim_active_uploads() {
    let mut fixture = Fixture::new(CAPS_MISSING, 0);
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    let blocked = observed.last().unwrap();
    assert_eq!(background(blocked)["unsupported"], 0);
    assert_eq!(background(blocked)["total"], serde_json::Value::Null);
    assert_eq!(background(blocked)["scanIncomplete"], true);
    assert!(!blocked.background_sync_in_progress);
    assert!(!blocked.full_sync_in_progress);

    let mut fixture = Fixture::new(RESERVE_CAPACITY, 0);
    fixture.turn_published(&mut observed);
    let active = observed.last().unwrap().clone();
    assert!(active.background_sync_in_progress);
    let paused = active.with_device_upload_enabled(false);
    assert!(!paused.background_sync_in_progress);
    assert!(!paused.full_sync_in_progress);
    assert!(paused.background_progress.is_none());
}

#[test]
fn cloud_scheduler_background_hides_counters_after_grant_change_or_storage_failure() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.accept_first();
    assert!(fixture.published_status().background_progress.is_some());
    fixture.sign_in_again();
    assert!(fixture.published_status().background_progress.is_none());
    let unavailable = fixture.runner.report_status(
        &fixture.store,
        &mut fixture.host,
        &fixture.engine,
        true,
        &|| true,
        |_| {},
    );
    assert!(!unavailable.storage_known);
    assert!(!unavailable.background_sync_in_progress);
    assert!(!unavailable.full_sync_in_progress);
    assert!(unavailable.background_progress.is_none());
}

#[test]
fn cloud_scheduler_background_keeps_totals_unknown_when_discovery_has_gaps() {
    let mut fixture = Fixture::new(READY, 0);
    write_source(
        &fixture.root,
        &fixture.root,
        "partial",
        "outside the saved folder",
    );
    fs::OpenOptions::new()
        .append(true)
        .open(fixture.root.join("partial.jsonl"))
        .unwrap()
        .write_all(b"{\"type\":\n")
        .unwrap();
    let mut observed = Vec::new();
    for _ in 0..10 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
    }
    let completed = observed.last().unwrap();
    assert_eq!(background(completed)["uploaded"], 1);
    assert_eq!(background(completed)["total"], serde_json::Value::Null);
    assert_eq!(background(completed)["scanIncomplete"], true);
    assert_eq!(
        background(completed)["transfer"]["totalBytes"],
        serde_json::Value::Null
    );
    assert!(!completed.background_sync_in_progress);
}

#[test]
fn cloud_scheduler_background_counts_a_changed_session_once() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.accept_first();
    write_source(&fixture.root, &fixture.project, "session", "changed");
    fixture.runner.next_reconcile = None;
    fixture.turn();
    let progress = background(&fixture.published_status());
    let group_id = saved_team_folder_id(fixture.project.to_str().unwrap());
    assert_eq!(progress["folderFound"][&group_id], 1);
    assert_eq!(progress["uploaded"], 0);
}

#[test]
fn cloud_scheduler_background_retains_bytes_when_an_accepted_entry_needs_cleanup() {
    let mut fixture = Fixture::new(VERIFY_PENDING, 0);
    let mut observed = Vec::new();
    for _ in 0..6 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
    }
    let entry = fixture.host.spool().unwrap().entries().unwrap().remove(0);
    let total = entry.progress().unwrap().required_transfer_bytes;
    fixture.service.mode.store(READY, Ordering::Release);
    let current = || true;
    let mut transport = upload::HttpTransport::new(
        &mut fixture.engine,
        &current,
        &current,
        &fixture.service.origin,
        None,
    );
    let accepted = fixture
        .host
        .spool()
        .unwrap()
        .resume_step(&entry.identity, &mut transport)
        .unwrap();
    assert_eq!(accepted.entry.state, CaptureState::Accepted);
    fixture.turn_published(&mut observed);
    let completed = observed.last().unwrap();
    assert_eq!(background(completed)["uploaded"], 1);
    assert_eq!(
        background(completed)["transfer"],
        serde_json::json!({ "sentBytes": total, "totalBytes": total })
    );
    assert!(fixture.host.spool().unwrap().entries().unwrap().is_empty());
}

#[test]
fn cloud_scheduler_full_sync_replaces_background_context_without_losing_saved_work() {
    let mut fixture = Fixture::new(READY, 0);
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    let capture = fixture
        .host
        .spool()
        .unwrap()
        .entries()
        .unwrap()
        .remove(0)
        .identity;
    assert!(observed.last().unwrap().background_sync_in_progress);
    fixture.schedule_backfill();
    let started = fixture.published_status();
    assert!(started.full_sync_in_progress);
    assert!(!started.background_sync_in_progress);
    assert!(started.background_progress.is_none());
    assert_eq!(
        fixture.host.spool().unwrap().entries().unwrap()[0].identity,
        capture
    );
    for _ in 0..10 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
    }
    let completed = observed.last().unwrap();
    assert!(!completed.full_sync_in_progress);
    assert!(!completed.background_sync_in_progress);
    assert_eq!(
        serde_json::to_value(completed).unwrap()["uploadProgress"]["uploaded"],
        1
    );
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
}

#[test]
fn cloud_scheduler_background_inactive_flags_override_a_previous_syncing_state() {
    let mut fixture = Fixture::new(READY, 0);
    fixture.accept_first();
    let completed = fixture.published_status();
    assert_eq!(completed.status, State::Syncing);
    let wire = serde_json::to_value(completed).unwrap();
    assert_eq!(
        wire.get("backgroundSyncInProgress"),
        Some(&serde_json::json!(false))
    );
    assert_eq!(
        wire.get("fullSyncInProgress"),
        Some(&serde_json::json!(false))
    );
}

#[test]
fn cloud_scheduler_background_does_not_add_bytes_to_a_settled_blocked_explicit_source() {
    let mut fixture = Fixture::new(CAPS_SMALL, 0);
    write_source(
        &fixture.root,
        &fixture.project,
        "session",
        &"x".repeat(256 * 1024),
    );
    fixture.schedule_backfill();
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    let settled = serde_json::to_value(observed.last().unwrap()).unwrap()["uploadProgress"].clone();
    assert_eq!(settled["blocked"], 1);
    assert!(!observed.last().unwrap().full_sync_in_progress);
    write_source(
        &fixture.root,
        &fixture.project,
        "session",
        "changed synthetic source",
    );
    fixture.runner.next_reconcile = None;
    for _ in 0..10 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
        assert_eq!(
            serde_json::to_value(observed.last().unwrap()).unwrap()["uploadProgress"],
            settled
        );
        if background(observed.last().unwrap())["uploaded"] == 1 {
            break;
        }
    }
    assert_eq!(background(observed.last().unwrap())["uploaded"], 1);
}

#[test]
fn cloud_scheduler_background_keeps_activity_after_a_blocked_page_until_a_healthy_later_page() {
    let mut fixture = Fixture::new(READY, 0);
    let database = fixture.select_opencode(513);
    let connection = rusqlite::Connection::open(database).unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory=?1",
            [fixture.project.to_str().unwrap()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO session_message
             SELECT 'broken-' || id, id, 'user', 1, ?1
             FROM session_v2 WHERE id <> 'session-0512'",
            [r#"{"files":[{"uri":"media:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#],
        )
        .unwrap();
    drop(connection);
    let mut observed = Vec::new();
    fixture.turn_published(&mut observed);
    assert_eq!(observed[0].status, State::Fenced);
    assert!(fixture.runner.discovery_cursor.is_some());
    assert!(observed[0].background_sync_in_progress);
    assert_eq!(background(&observed[0])["total"], serde_json::Value::Null);
    for _ in 0..80 {
        fixture.runner.next_integrity_poll = None;
        fixture.turn_published(&mut observed);
        if background(observed.last().unwrap())["uploaded"] == 1 {
            break;
        }
    }
    assert_eq!(background(observed.last().unwrap())["uploaded"], 1);
    assert_eq!(fixture.service.reserves.load(Ordering::Acquire), 1);
}
