use super::*;

#[tokio::test]
async fn watcher_recovers_a_restored_historical_source_after_a_current_window_scan() {
    let now = unix_now();
    for (age_days, should_recover) in [(45, true), (91, false)] {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(temp.path()).unwrap();
        store
            .update_settings(|settings| settings.session_data_retention_days = 90)
            .unwrap();
        let id = "11111111-1111-4111-8111-111111111111";
        let mut log = fixture_log(temp.path(), id);
        let agent = log.agent_type.slug();
        let SessionSource::File(path) = &log.source else {
            unreachable!()
        };
        let activity = now - age_days * 86_400;
        log.updated_at = Some(activity);
        let mut event: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        event["timestamp"] = time::OffsetDateTime::from_unix_timestamp(activity)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap()
            .into();
        let contents = format!("{event}\n");
        std::fs::write(path, &contents).unwrap();

        let DescribeOutcome::Session(record) = describe_one(log.clone(), temp.path(), None).await
        else {
            panic!("historical source was not described");
        };
        assert_eq!(record.updated_at_epoch, Some(activity));
        store
            .upsert_sessions(std::slice::from_ref(record.as_ref()), &[agent])
            .unwrap();
        let claim = store
            .claim_next_evidence(&[agent], now, 60)
            .unwrap()
            .unwrap();
        std::fs::remove_file(path).unwrap();
        store
            .fail_evidence(
                &claim,
                crate::store::EvidenceFailure::Failed {
                    revisions: crate::analysis::projection_revisions(),
                },
                crate::insights_worker::EVIDENCE_ERROR_SOURCE_MISSING,
            )
            .unwrap();
        std::fs::write(path, &contents).unwrap();

        let restored = SessionActivityKey::new("native", agent, log.source_label());
        let other = SessionActivityKey::new("native", agent, "other.jsonl");
        let mut pending = scoped::ScopedWork {
            sessions: BTreeSet::from([restored.clone(), other]),
            ..Default::default()
        };
        let uncovered = scoped::uncovered_historical_recoveries(&store, &pending.sessions, now)
            .await
            .unwrap();
        let mut floors = scoped::Floors::default();
        let scanned_at = tokio::time::Instant::now();
        floors.settle_full_pass(&mut pending, &uncovered, scanned_at);
        let (ready, deferred, _) = floors.admit(pending, scanned_at);
        assert!(deferred.is_empty());
        assert_eq!(
            ready.sessions,
            should_recover
                .then_some(restored.clone())
                .into_iter()
                .collect()
        );

        let mut admission = writer(&store);
        admission.previous_records.insert(restored, *record);
        let mut watch_policy = policy(&store, temp.path(), None);
        watch_policy.cutoff = Some(now - CURRENT_WINDOW_SECS);
        watch_policy.historical_missing =
            historical_missing_sources(&store, &admission.previous_records, now, 90)
                .await
                .unwrap();
        let (sender, finite) = mpsc::channel(1);
        let (_live, live) = mpsc::channel(1);
        sender.send(batch(vec![log])).await.unwrap();
        drop(sender);
        run(finite, live, None, &watch_policy, &mut admission).await;
        admission.finish().unwrap();

        let retry = store.claim_next_evidence(&[agent], now, 60).unwrap();
        assert_eq!(
            retry.map(|retry| retry.key),
            should_recover.then_some(claim.key)
        );
    }
}

/// Feed only finite discovery results so the owner receives no watcher hints.
async fn scan_without_watcher(
    store: &Store,
    home: &std::path::Path,
    cutoff: Option<i64>,
    logs: Vec<SessionLog>,
) -> Described {
    let mut scan_policy = policy(store, home, None);
    scan_policy.cutoff = cutoff;
    let mut admission = writer(store);
    let (sender, finite) = mpsc::channel(1);
    let (_live_sender, live) = mpsc::channel(1);
    sender.send(batch(logs)).await.unwrap();
    drop(sender);
    let result = run(finite, live, None, &scan_policy, &mut admission).await;
    admission.finish().unwrap();
    result
}

#[tokio::test]
async fn history_indexes_older_sessions_and_reconciliation_indexes_missed_sources() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let store = Store::open_in_memory(home).unwrap();
    let now = unix_now();
    let cutoff = Some(now - CURRENT_WINDOW_SECS);
    let recent = fixture_log(home, "11111111-1111-4111-8111-111111111111");
    let mut older = fixture_log(home, "22222222-2222-4222-8222-222222222222");
    let older_activity = now - 45 * 86_400;
    older.updated_at = Some(older_activity);
    let SessionSource::File(path) = &older.source else {
        unreachable!()
    };
    let mut event: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    event["timestamp"] = time::OffsetDateTime::from_unix_timestamp(older_activity)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
        .into();
    std::fs::write(path, format!("{event}\n")).unwrap();

    let current =
        scan_without_watcher(&store, home, cutoff, vec![recent.clone(), older.clone()]).await;
    assert_eq!(current.records.len(), 1);
    assert_eq!(current.records[0].source_label, recent.source_label());

    let historical =
        scan_without_watcher(&store, home, None, vec![recent.clone(), older.clone()]).await;
    assert_eq!(historical.records.len(), 2);
    assert!(historical.records.iter().any(|row| {
        row.source_label == older.source_label() && row.updated_at_epoch == Some(older_activity)
    }));

    let agent = AgentKind::Claude.slug();
    for _ in 0..2 {
        let claim = store
            .claim_next_evidence(&[agent], now, 60)
            .unwrap()
            .unwrap();
        store
            .fail_evidence(
                &claim,
                crate::store::EvidenceFailure::Failed {
                    revisions: crate::analysis::projection_revisions(),
                },
                "synthetic_terminal_error",
            )
            .unwrap();
    }

    let missed = fixture_log(home, "33333333-3333-4333-8333-333333333333");
    let periodic =
        scan_without_watcher(&store, home, cutoff, vec![recent, older, missed.clone()]).await;
    assert_eq!(periodic.records.len(), 2);
    let missed_row = periodic
        .records
        .iter()
        .find(|row| row.source_label == missed.source_label())
        .unwrap();
    assert_eq!(periodic.changed, vec![missed_row.key.clone()]);
    let claim = store
        .claim_next_evidence(&[agent], now + 1, 60)
        .unwrap()
        .unwrap();
    assert_eq!(claim.key, missed_row.key);
    assert!(
        store
            .claim_next_evidence(&[agent], now + 1, 60)
            .unwrap()
            .is_none()
    );
}
