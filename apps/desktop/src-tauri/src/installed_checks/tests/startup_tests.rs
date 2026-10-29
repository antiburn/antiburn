use super::*;

fn file_fixture(count: usize) -> Fixture {
    let fixture = Fixture::new();
    fixture
        .store
        .lock()
        .execute("DELETE FROM native_file_delivery", [])
        .unwrap();
    std::fs::write(
        fixture.directory.path().join("main.jsonl"),
        transcript("main", count),
    )
    .unwrap();
    fixture.index("native", "main", count);
    fixture.host().reconcile(&Admission::Ready).unwrap();
    fixture
        .store
        .lock()
        .execute("UPDATE session_evidence SET status = 'failed'", [])
        .unwrap();
    fixture
}

async fn deliver(fixture: &Fixture, now: i64) -> bool {
    crate::insights_worker::file_live::process_next(&fixture.store, &|| now, &|_| {})
        .await
        .unwrap()
}

fn drain(host: &mut InstalledCheckHost<ScriptedEvaluator>) {
    for _ in 0..32 {
        if !host.catch_up().unwrap().1 {
            return;
        }
    }
    panic!("startup publications did not drain");
}

#[tokio::test]
async fn native_file_startup_assesses_latest_once_in_either_attachment_order() {
    for source_first in [false, true] {
        let fixture = file_fixture(12);
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        if source_first {
            assert!(deliver(&fixture, now).await);
            std::fs::write(
                fixture.global.join("failed-retry-loop.yml"),
                CHECK.replace("name: Synthetic retry review", "name: Startup window"),
            )
            .unwrap();
        }
        let mut host = fixture.host();
        host.reconcile(&Admission::Ready).unwrap();
        if !source_first {
            let report = read_installed_checks(&fixture.store, &host.definitions).unwrap();
            assert!(report[0].waiting_for_events.unwrap().catching_up);
            assert_eq!(report[0].waiting_for_results_count, 0);
            assert!(deliver(&fixture, now).await);
        }
        drain(&mut host);
        fixture.evaluator.answer("stuck");
        assert_eq!(
            host.evaluation_jobs(1)
                .pop()
                .unwrap()
                .run()
                .await
                .unwrap()
                .unwrap()
                .outcome,
            RunOutcome::Accepted
        );
        let inputs = fixture.evaluator.inputs.lock().unwrap().clone();
        assert_eq!(inputs.len(), 1);
        let activity = inputs[0]["activity"].as_array().unwrap();
        assert_eq!(activity.len(), 5);
        assert!(
            activity.last().unwrap()["id"]
                .as_str()
                .unwrap()
                .contains("result-11")
        );
        drop(inputs);
        let writes = status_writes(&fixture.store, &host.definitions[0]);
        drop(host);
        let mut restarted = fixture.host();
        restarted.reconcile(&Admission::Ready).unwrap();
        drain(&mut restarted);
        assert!(
            restarted
                .evaluation_jobs(1)
                .pop()
                .unwrap()
                .run()
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(fixture.evaluator.inputs.lock().unwrap().len(), 1);
        assert_eq!(
            status_writes(&fixture.store, &restarted.definitions[0]),
            writes
        );
        let results = fixture
            .store
            .native_tool_results_current(&SessionKey::new("native", "pi", "main"))
            .unwrap();
        assert!(results.iter().all(|record| matches!(
            record.result.position,
            antiburn_local::analysis::ToolResultPosition::FileBytes { record_end, .. }
                if record.result.revision == record_end
        )));
    }
}

#[tokio::test]
async fn admitted_file_append_survives_restart_and_an_old_activity_timestamp() {
    let fixture = file_fixture(0);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    assert!(deliver(&fixture, now).await);
    drain(&mut host);
    let activation = host.projects.values().next().unwrap().active[0].clone();
    std::fs::write(
        fixture.directory.path().join("main.jsonl"),
        transcript("main", 3),
    )
    .unwrap();
    assert!(deliver(&fixture, now + 1).await);
    fixture
        .store
        .lock()
        .execute("UPDATE session SET updated_at_epoch = 1000", [])
        .unwrap();
    drop(host);
    let mut restarted = fixture.host();
    restarted.reconcile(&Admission::Ready).unwrap();
    assert_eq!(
        restarted.projects.values().next().unwrap().active[0],
        activation
    );
    drain(&mut restarted);
    fixture.evaluator.answer("clear");
    assert_eq!(
        restarted
            .evaluation_jobs(1)
            .pop()
            .unwrap()
            .run()
            .await
            .unwrap()
            .unwrap()
            .outcome,
        RunOutcome::Accepted
    );
    assert_eq!(fixture.evaluator.inputs.lock().unwrap().len(), 1);
    assert!(
        restarted
            .evaluation_jobs(1)
            .pop()
            .unwrap()
            .run()
            .await
            .unwrap()
            .is_none()
    );
}

#[test]
fn startup_scope_discovery_excludes_historical_remote_and_child_sessions() {
    let fixture = Fixture::new();
    for index in 0..140 {
        fixture.index_in_cwd(
            "native",
            &format!("old-{index}"),
            0,
            &fixture.directory.path().join(format!("old-{index}")),
        );
    }
    fixture
        .store
        .lock()
        .execute(
            "UPDATE session SET updated_at_epoch = 1000 WHERE session_id LIKE 'old-%'",
            [],
        )
        .unwrap();
    fixture.index("ssh:test", "remote", 0);
    fixture.index("native", "child", 0);
    fixture.store.lock().execute("INSERT INTO session_relation
        (environment_key, agent, session_id, kind, related_id) VALUES ('native', 'pi', 'child', 'forkParent', 'main')", []).unwrap();
    let scopes = live_projects(&fixture.store).unwrap();
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0].environment, "native");
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    assert_eq!(host.projects.len(), 1);
}

#[tokio::test]
async fn file_page_receipt_and_outbox_roll_back_together() {
    let fixture = file_fixture(3);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let before = fixture
        .store
        .native_tool_result_publications_after(0, 100)
        .unwrap();
    fixture.store.lock().execute_batch("CREATE TEMP TRIGGER reject_file_cursor
        BEFORE UPDATE OF cursor_json ON native_file_delivery BEGIN SELECT RAISE(ABORT, 'reject receipt'); END").unwrap();
    assert!(
        crate::insights_worker::file_live::process_next(&fixture.store, &|| now, &|_| {})
            .await
            .is_err()
    );
    assert_eq!(
        fixture
            .store
            .native_tool_result_publications_after(0, 100)
            .unwrap(),
        before
    );
    assert!(
        fixture
            .store
            .native_tool_results_current(&SessionKey::new("native", "pi", "main"))
            .unwrap()
            .is_empty()
    );
    fixture
        .store
        .lock()
        .execute_batch("DROP TRIGGER reject_file_cursor")
        .unwrap();
    fixture.store.recover_native_file_claims().unwrap();
    assert!(deliver(&fixture, now).await);
    assert_eq!(
        fixture
            .store
            .native_tool_results_current(&SessionKey::new("native", "pi", "main"))
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn dormant_file_delivery_sweeps_for_resume_without_a_new_discovery_generation() {
    let fixture = file_fixture(0);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    assert!(deliver(&fixture, now).await);
    fixture
        .store
        .lock()
        .execute("UPDATE session SET updated_at_epoch = 1000", [])
        .unwrap();
    assert!(deliver(&fixture, now + 181).await);
    assert!(!deliver(&fixture, now + 182).await);
    std::fs::write(
        fixture.directory.path().join("main.jsonl"),
        transcript("main", 3),
    )
    .unwrap();
    assert!(deliver(&fixture, now + 211).await);
    assert_eq!(
        fixture
            .store
            .native_tool_results_current(&SessionKey::new("native", "pi", "main"))
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn startup_without_a_window_coalesces_once_but_live_results_stay_separate() {
    let fixture = Fixture::new();
    let check = "name: Latest result\non:\n  tool_result:\n  cooldown: 0s\ndecisions:\n  review:\n    input: {activity: '${{ message }}'}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {stuck: Stuck., clear: Clear., uncertain: Uncertain.}\nactions:\n  - type: status\n    input: {result: success}\n";
    std::fs::write(fixture.global.join("failed-retry-loop.yml"), check).unwrap();
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    fixture.publish("main", 3);
    drain(&mut host);
    fixture.evaluator.answer("clear");
    for job in host.evaluation_jobs(4) {
        job.run().await.unwrap();
    }
    assert_eq!(fixture.evaluator.inputs.lock().unwrap().len(), 1);
    fixture.publish("main", 6);
    drain(&mut host);
    for _ in 0..3 {
        fixture.evaluator.answer("clear");
    }
    let now_ms =
        u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).unwrap();
    for index in 0..3 {
        run_due_at(&mut host, now_ms + (index + 1) * 2000).await;
    }
    assert_eq!(fixture.evaluator.inputs.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn correction_to_a_seeded_historical_result_invalidates_status_without_a_new_attempt() {
    let fixture = Fixture::new();
    fixture.publish("main", 3);
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    drain(&mut host);
    fixture.evaluator.answer("stuck");
    host.evaluation_jobs(1).pop().unwrap().run().await.unwrap();
    assert_eq!(
        read_installed_checks(&fixture.store, &host.definitions).unwrap()[0].finding,
        1
    );
    let current = fixture
        .store
        .native_tool_results_current(&SessionKey::new("native", "pi", "main"))
        .unwrap();
    let mut correction = current[0].result.clone();
    correction.revision += 1;
    correction.content = Some("Corrected source output.".into());
    let payload = serde_json::to_string(&correction).unwrap();
    {
        let mut connection = fixture.store.lock();
        let tx = connection.transaction().unwrap();
        tx.execute("INSERT INTO native_tool_result_publication
        (outcome, environment_key, agent, session_id, source_format, native_identity, native_position_json,
         native_revision, payload_json, complete, source_generation, claim_fence, live_eligible, project_cwd, session_incarnation)
        SELECT 'update', environment_key, agent, session_id, source_format, native_identity, native_position_json,
            ?2, ?3, complete, source_generation, claim_fence, 0, project_cwd, session_incarnation
          FROM native_tool_result_publication WHERE publication_sequence = ?1",
        rusqlite::params![current[0].publication_sequence, correction.revision, payload]).unwrap();
        tx.execute(
            "UPDATE native_tool_result_current SET native_revision = ?2, payload_json = ?3,
        publication_sequence = last_insert_rowid() WHERE publication_sequence = ?1",
            rusqlite::params![
                current[0].publication_sequence,
                correction.revision,
                payload
            ],
        )
        .unwrap();
        tx.commit().unwrap();
    }
    drain(&mut host);
    assert_eq!(
        read_installed_checks(&fixture.store, &host.definitions).unwrap()[0].finding,
        0
    );
    assert!(
        host.evaluation_jobs(1)
            .pop()
            .unwrap()
            .run()
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(fixture.evaluator.inputs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn productive_file_continuations_rotate_without_a_clock_tick() {
    let fixture = file_fixture(1200);
    std::fs::write(
        fixture.directory.path().join("second.jsonl"),
        transcript("second", 1200),
    )
    .unwrap();
    fixture.index("native", "second", 1200);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    fixture
        .store
        .lock()
        .execute("UPDATE session SET updated_at_epoch = ?1", [now])
        .unwrap();
    let published = Mutex::new(Vec::new());
    for _ in 0..3 {
        assert!(
            crate::insights_worker::file_live::process_next(&fixture.store, &|| now, &|key| {
                published.lock().unwrap().push(key.session_id.clone());
            })
            .await
            .unwrap()
        );
    }
    assert_eq!(*published.lock().unwrap(), ["main", "second", "main"]);
}

#[tokio::test]
async fn idle_file_polls_do_not_rewrite_the_cursor_or_announce_analysis_changes() {
    let fixture = file_fixture(3);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    assert!(deliver(&fixture, now).await);
    let before = fixture
        .store
        .native_tool_result_publications_after(0, 100)
        .unwrap();
    fixture
        .store
        .lock()
        .execute_batch(
            "CREATE TEMP TRIGGER reject_idle_cursor_write
         BEFORE UPDATE OF cursor_json ON native_file_delivery
         BEGIN SELECT RAISE(ABORT, 'idle cursor write'); END",
        )
        .unwrap();
    let announced = Mutex::new(Vec::new());
    assert!(
        crate::insights_worker::file_live::process_next(&fixture.store, &|| now + 1, &|key| {
            announced.lock().unwrap().push(key.clone());
        })
        .await
        .unwrap()
    );
    assert!(announced.lock().unwrap().is_empty());
    assert_eq!(
        fixture
            .store
            .native_tool_result_publications_after(0, 100)
            .unwrap(),
        before
    );
    assert!(!deliver(&fixture, now + 1).await);
    assert!(deliver(&fixture, now + 2).await);
}

#[tokio::test]
async fn exhausted_file_reads_report_source_unavailable_and_recovery_clears_it() {
    let fixture = file_fixture(0);
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    assert!(deliver(&fixture, now).await);
    drain(&mut host);
    let source = fixture.directory.path().join("main.jsonl");
    let unavailable = fixture.directory.path().join("unavailable.jsonl");
    std::fs::rename(&source, &unavailable).unwrap();
    for attempt in 1..=5 {
        assert!(deliver(&fixture, now + attempt * 1000).await);
    }
    assert!(!deliver(&fixture, now + 6000).await);
    let report = read_installed_checks(&fixture.store, &host.definitions).unwrap();
    assert!(
        report[0]
            .diagnostics
            .iter()
            .any(|item| item.code == "source-unavailable")
    );
    assert_eq!(report[0].waiting_for_data_count, 0);
    assert_eq!(report[0].waiting_for_results_count, 0);
    std::fs::rename(&unavailable, &source).unwrap();
    std::fs::write(&source, transcript("main", 3)).unwrap();
    fixture.index("native", "main", 3);
    assert!(deliver(&fixture, now + 6001).await);
    drain(&mut host);
    let report = read_installed_checks(&fixture.store, &host.definitions).unwrap();
    assert!(
        !report[0]
            .diagnostics
            .iter()
            .any(|item| item.code == "source-unavailable")
    );
}

#[tokio::test]
async fn replaced_file_source_does_not_seed_results_from_the_previous_path() {
    let fixture = file_fixture(3);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    assert!(deliver(&fixture, now).await);
    let key = SessionKey::new("native", "pi", "main");
    let mut record = fixture.store.session(&key).unwrap().unwrap();
    let replacement = fixture.directory.path().join("replacement.jsonl");
    std::fs::write(&replacement, transcript("main", 0)).unwrap();
    record.source_label = replacement.to_string_lossy().into_owned();
    record.source_fingerprint = Some("replacement".into());
    fixture
        .store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(deliver(&fixture, now + 1).await);
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    drain(&mut host);
    assert!(
        host.evaluation_jobs(1)
            .pop()
            .unwrap()
            .run()
            .await
            .unwrap()
            .is_none()
    );
    assert!(fixture.evaluator.inputs.lock().unwrap().is_empty());
    let report = read_installed_checks(&fixture.store, &host.definitions).unwrap();
    assert!(
        report[0]
            .diagnostics
            .iter()
            .any(|item| item.code == "source-unavailable")
    );
    assert!(!deliver(&fixture, now + 1000).await);
}

#[tokio::test]
async fn moved_file_session_cannot_dispatch_a_candidate_from_its_previous_scope() {
    let fixture = file_fixture(3);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    assert!(deliver(&fixture, now).await);
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    drain(&mut host);
    let previous_job = host.evaluation_jobs(1).pop().unwrap();
    let key = SessionKey::new("native", "pi", "main");
    let mut record = fixture.store.session(&key).unwrap().unwrap();
    record.cwd = Some(
        fixture
            .directory
            .path()
            .join("other-project")
            .to_string_lossy()
            .into_owned(),
    );
    fixture
        .store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    host.reconcile(&Admission::Ready).unwrap();
    assert!(deliver(&fixture, now + 1).await);
    fixture.evaluator.answer("stuck");
    assert!(previous_job.run().await.unwrap().is_none());
    assert!(fixture.evaluator.inputs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn renamed_file_keeps_its_cursor_and_delivers_new_results_once() {
    let fixture = file_fixture(0);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    assert!(deliver(&fixture, now).await);
    drain(&mut host);
    let source = fixture.directory.path().join("main.jsonl");
    let renamed = fixture.directory.path().join("renamed.jsonl");
    std::fs::rename(&source, &renamed).unwrap();
    let key = SessionKey::new("native", "pi", "main");
    let mut record = fixture.store.session(&key).unwrap().unwrap();
    record.source_label = renamed.to_string_lossy().into_owned();
    fixture
        .store
        .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(deliver(&fixture, now + 1).await);
    std::fs::write(&renamed, transcript("main", 3)).unwrap();
    record.source_fingerprint = Some("renamed-3".into());
    fixture
        .store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(deliver(&fixture, now + 2).await);
    drain(&mut host);
    fixture.evaluator.answer("clear");
    assert_eq!(
        host.evaluation_jobs(1)
            .pop()
            .unwrap()
            .run()
            .await
            .unwrap()
            .unwrap()
            .outcome,
        RunOutcome::Accepted
    );
    assert!(
        host.evaluation_jobs(1)
            .pop()
            .unwrap()
            .run()
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(fixture.evaluator.inputs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn validated_file_move_rebinds_idle_without_another_start_or_result_seed() {
    let fixture = file_fixture(3);
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    assert!(deliver(&fixture, now).await);
    drain(&mut host);
    let before: (String, i64, i64) = fixture
        .store
        .lock()
        .query_row(
            "SELECT observer_epoch, start_publication, (SELECT COUNT(*) FROM burn_check_candidate)
         FROM burn_check_session_attachment",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    fixture
        .store
        .lock()
        .execute(
            "UPDATE burn_check_session_attachment SET last_activity_token = 'old-source',
        last_activity_order_json = '{\"Jsonl\":{\"record_index\":999,\"item_index\":0}}',
        session_selection_json = '{\"model\":\"old/model\",\"fast_mode\":null}'",
            [],
        )
        .unwrap();
    let renamed = fixture.directory.path().join("renamed-observer.jsonl");
    std::fs::rename(fixture.directory.path().join("main.jsonl"), &renamed).unwrap();
    let key = SessionKey::new("native", "pi", "main");
    let mut record = fixture.store.session(&key).unwrap().unwrap();
    record.source_label = renamed.to_string_lossy().into_owned();
    fixture
        .store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(deliver(&fixture, now + 1).await);
    let state: (String, i64, String, i64, Option<String>, Option<String>) = fixture
        .store
        .lock()
        .query_row(
            "SELECT observer_epoch, start_publication, source_label, idle_deadline_ms,
            last_activity_order_json, session_selection_json FROM burn_check_session_attachment",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!((&state.0, state.1), (&before.0, before.1));
    assert_eq!(state.2, renamed.to_string_lossy());
    assert_eq!(state.3, (now + 121) * 1000);
    assert_eq!((state.4, state.5), (None, None));
    assert!(deliver(&fixture, now + 121).await);
    drain(&mut host);
    let counts: (i64, i64, i64) = fixture.store.lock().query_row(
        "SELECT (SELECT COUNT(*) FROM native_tool_result_publication WHERE event_kind = 'session_start'),
            (SELECT COUNT(*) FROM native_tool_result_publication WHERE event_kind = 'session_idle'),
            (SELECT COUNT(*) FROM burn_check_candidate)", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    assert_eq!(counts, (1, 1, before.2));
}

#[test]
fn more_than_four_checks_finish_an_empty_source_sweep() {
    let fixture = Fixture::new();
    for index in 0..6 {
        std::fs::write(fixture.global.join(format!("idle-sweep-{index}.yml")),
            "name: Empty sweep\non: {tool_result: null}\nactions:\n  - type: status\n    input: {result: failure}\n").unwrap();
    }
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    assert!(
        host.projects
            .values()
            .map(|project| project.active.len())
            .sum::<usize>()
            > 4
    );
    drain(&mut host);
    for _ in 0..3 {
        let mut stopped = false;
        for _ in 0..4 {
            let (changed, more) = host.catch_up().unwrap();
            assert!(!changed);
            if !more {
                stopped = true;
                break;
            }
        }
        assert!(
            stopped,
            "an empty sweep must stop after all checks receive a turn"
        );
    }
}

#[tokio::test]
async fn validated_file_move_cancels_unfinished_idle_and_starts_a_full_new_interval() {
    let fixture = file_fixture(0);
    std::fs::write(fixture.global.join("failed-retry-loop.yml"), "name: Idle\non:\n  session_idle:\nactions:\n  - type: status\n    input: {result: failure}\n").unwrap();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let mut host = fixture.host();
    host.reconcile(&Admission::Ready).unwrap();
    assert!(deliver(&fixture, now).await);
    drain(&mut host);
    let first_idle_deadline_ms = fixture
        .store
        .next_session_idle_deadline()
        .unwrap()
        .expect("startup must set an idle deadline");
    assert_eq!(first_idle_deadline_ms % 1000, 0);
    let now = first_idle_deadline_ms / 1000 - 120;
    assert!(deliver(&fixture, now + 119).await);
    drain(&mut host);
    let early_counts: (i64, i64) = fixture.store.lock().query_row(
        "SELECT (SELECT COUNT(*) FROM native_tool_result_publication WHERE event_kind = 'session_idle'),
            (SELECT COUNT(*) FROM burn_check_candidate)", [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(
        early_counts,
        (0, 0),
        "idle publication and candidate must wait for deadline {first_idle_deadline_ms}"
    );
    assert_eq!(
        fixture.store.next_session_idle_deadline().unwrap(),
        Some(first_idle_deadline_ms)
    );
    assert_eq!(
        fixture
            .store
            .request_due_session_idle_catchup(first_idle_deadline_ms)
            .unwrap(),
        1
    );
    assert!(deliver(&fixture, now + 120).await);
    drain(&mut host);
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row("SELECT state FROM burn_check_candidate", [], |row| row
                .get::<_, String>(
                0
            ))
            .expect("delivery at the persisted idle deadline must create a candidate"),
        "pending"
    );
    let renamed = fixture.directory.path().join("moved-idle.jsonl");
    std::fs::rename(fixture.directory.path().join("main.jsonl"), &renamed).unwrap();
    let mut record = fixture
        .store
        .session(&SessionKey::new("native", "pi", "main"))
        .unwrap()
        .unwrap();
    record.source_label = renamed.to_string_lossy().into_owned();
    fixture
        .store
        .upsert_sessions(&[record], &crate::agents::evidence_cohort())
        .unwrap();
    assert!(deliver(&fixture, now + 121).await);
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row("SELECT state FROM burn_check_candidate", [], |row| row
                .get::<_, String>(
                0
            ))
            .unwrap(),
        "stale"
    );
    assert_eq!(
        fixture.store.next_session_idle_deadline().unwrap(),
        Some((now + 241) * 1000)
    );
    assert!(deliver(&fixture, now + 240).await);
    assert_eq!(fixture.store.lock().query_row("SELECT COUNT(*) FROM native_tool_result_publication WHERE event_kind = 'session_idle'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(
        fixture
            .store
            .request_due_session_idle_catchup((now + 241) * 1000)
            .unwrap(),
        1
    );
    assert!(deliver(&fixture, now + 241).await);
    drain(&mut host);
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_candidate WHERE state = 'pending'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(fixture.store.lock().query_row("SELECT COUNT(*) FROM native_tool_result_publication WHERE event_kind = 'session_start'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
}

#[tokio::test]
async fn fresh_file_user_before_attachment_keeps_result_seed_before_mixed_selection() {
    for mixed in [false, true] {
        let fixture = file_fixture(3);
        let definition = format!(
            "name: Event startup\non:\n  tool_result:\n{}  window: {{size: 4, min: 3}}\n  cooldown: 0s\nactions:\n  - type: status\n    input: {{result: failure}}\n",
            if mixed { "  user_message:\n" } else { "" }
        );
        std::fs::write(fixture.global.join("failed-retry-loop.yml"), definition).unwrap();
        let mut host = fixture.host();
        host.reconcile(&Admission::Ready).unwrap();
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        assert!(deliver(&fixture, now).await);
        let fresh = json!({"type":"message","id":"fresh-user","parentId":"result-2",
            "timestamp":"2026-01-01T00:00:04Z","message":{"role":"user","content":[{"type":"text","text":"Fresh user work"}]}});
        std::fs::write(
            fixture.directory.path().join("main.jsonl"),
            format!("{}{fresh}\n", transcript("main", 3)),
        )
        .unwrap();
        fixture.index("native", "main", 4);
        assert!(deliver(&fixture, now + 1).await);
        drain(&mut host);
        super::assert_result_seed_before_fresh_user(&fixture.store, mixed);
    }
}
