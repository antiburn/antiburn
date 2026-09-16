use super::*;
use crate::session_lifecycle::{IndexChangeReason, Observation, RemovalReason, RemovalScope};
use crate::store::{Incarnation, Revision};

fn persisted_facts(
    store: &Store,
    described: &Described,
    previous: &std::collections::HashMap<SessionActivityKey, SessionRecord>,
) -> Vec<Observation> {
    let evidence =
        persist_changed_records(&described.records, &described.changed, &[], |records| {
            store.upsert_sessions(records, &agents::evidence_cohort())
        })
        .unwrap();
    let Some((incarnations, revision)) = evidence else {
        return Vec::new();
    };
    let now = described
        .records
        .iter()
        .filter_map(|record| record.updated_at_epoch)
        .max()
        .unwrap();
    discovery_report(
        now,
        &described.records,
        &incarnations,
        &described.changed,
        previous,
        revision,
    )
    .collect()
}

#[tokio::test]
async fn discovery_and_membership_reports_follow_real_new_unchanged_and_appended_sources() {
    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let path = write_claude_session(home.path(), "producer");
    let previous = store.session_records().unwrap();
    let first = describe_with_states(
        vec![log(AgentKind::Claude, path.clone(), 1_800_000_000)],
        home.path(),
        &HashSet::new(),
        &previous,
    )
    .await;
    let facts = persisted_facts(&store, &first, &previous);
    let [Observation::Indexed { sessions, revision }] = facts.as_slice() else {
        panic!("one indexed chunk: {facts:?}")
    };
    assert_eq!(*revision, store.revision());
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].is_new);
    let incarnation = sessions[0].incarnation;
    assert!(incarnation > Incarnation(0));
    let membership = membership_reports(&first, &[]);
    assert!(membership.removals.is_empty());
    assert!(matches!(
        membership.index_changed,
        Some(Observation::IndexChanged {
            reason: IndexChangeReason::ScanPass
        })
    ));

    let previous = store.session_records().unwrap();
    let second = describe_with_states(
        vec![log(AgentKind::Claude, path.clone(), 1_800_000_000)],
        home.path(),
        &HashSet::new(),
        &previous,
    )
    .await;
    assert!(persisted_facts(&store, &second, &previous).is_empty());
    let membership = membership_reports(&second, &[]);
    assert!(membership.removals.is_empty());
    assert!(membership.index_changed.is_none());

    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"{\"type\":\"assistant\",\"timestamp\":\"2026-08-01T10:05:00Z\"}\n")
        .unwrap();
    let third = describe_with_states(
        vec![log(AgentKind::Claude, path, 1_800_000_100)],
        home.path(),
        &HashSet::new(),
        &previous,
    )
    .await;
    let facts = persisted_facts(&store, &third, &previous);
    let [
        Observation::Indexed { sessions, revision },
        Observation::RowChanged {
            session,
            facets,
            at,
        },
    ] = facts.as_slice()
    else {
        panic!("indexed then metadata: {facts:?}")
    };
    assert_eq!(*revision, store.revision());
    assert_eq!(sessions[0].incarnation, incarnation);
    assert!(!sessions[0].is_new);
    assert_eq!(*session, sessions[0].key);
    assert_eq!(*at, sessions[0].at);
    assert!(facets.metadata);
    assert!(!facets.title);
    assert!(membership_reports(&third, &[]).index_changed.is_none());
}

#[tokio::test]
async fn membership_reports_name_only_successful_rejected_deletions_with_store_evidence() {
    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let row = record("claude-code", "rejected", Some(1_800_000_000));
    let (incarnations, before) = store
        .upsert_sessions(&[row], &agents::evidence_cohort())
        .unwrap();
    let path = write_claude_sidechain(home.path(), "rejected");
    let described = describe(
        vec![log(AgentKind::Claude, path, 1_800_000_050)],
        home.path(),
        &HashSet::new(),
    )
    .await;
    assert_eq!(described.rejected.len(), 1);
    for attempt in 0..2 {
        let mut deleted = Vec::new();
        for key in &described.rejected {
            if let Some((incarnation, revision)) = store.delete_session(key).unwrap() {
                deleted.push((key.clone(), incarnation, revision));
            }
        }
        let reports = membership_reports(&described, &deleted);
        assert!(matches!(
            reports.index_changed,
            Some(Observation::IndexChanged {
                reason: IndexChangeReason::ScanPass
            })
        ));
        if attempt == 0 {
            let [
                Observation::Removed {
                    scope: RemovalScope::One(key, incarnation),
                    reason,
                    revision,
                },
            ] = reports.removals.as_slice()
            else {
                panic!("one rejected deletion")
            };
            assert_eq!(*key, incarnations[0].0);
            assert_eq!(*incarnation, incarnations[0].1);
            assert_eq!(*reason, RemovalReason::Rejected);
            assert!(*revision > before);
            assert_eq!(*revision, store.revision());
        } else {
            assert!(
                reports.removals.is_empty(),
                "a missing row has no deletion evidence"
            );
        }
    }
    assert!(store.recent_sessions(0, 10).unwrap().is_empty());
}

#[tokio::test]
async fn membership_reports_detect_a_new_identity_behind_a_reused_label() {
    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let path = write_claude_session(home.path(), "next");
    let mut old = record_for_facets("previous", None, 1_800_000_000);
    old.source_label = path.to_string_lossy().into_owned();
    store
        .upsert_sessions(&[old], &agents::evidence_cohort())
        .unwrap();
    let previous = store.session_records().unwrap();
    let described = describe_with_states(
        vec![log(AgentKind::Claude, path, 1_800_000_000)],
        home.path(),
        &HashSet::new(),
        &previous,
    )
    .await;
    let facts = persisted_facts(&store, &described, &previous);
    let [Observation::Indexed { sessions, .. }] = facts.as_slice() else {
        panic!("new identity, not a row patch")
    };
    assert_eq!(sessions[0].key.session_id, "next");
    assert!(sessions[0].is_new);
    assert!(matches!(
        membership_reports(&described, &[]).index_changed,
        Some(Observation::IndexChanged {
            reason: IndexChangeReason::ScanPass
        })
    ));
}

#[test]
fn discovery_reports_bound_chunks_and_keep_each_upsert_incarnation_and_revision() {
    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let records: Vec<_> = (0..600)
        .map(|index| record_for_facets(&format!("row{index}"), None, 1000))
        .collect();
    let changed: Vec<_> = records.iter().map(|record| record.key.clone()).collect();
    let (incarnations, revision) = store
        .upsert_sessions(&records, &agents::evidence_cohort())
        .unwrap();
    let mut lengths = Vec::new();
    let mut seen = Vec::new();
    for report in discovery_report(
        1001,
        &records,
        &incarnations,
        &changed,
        &Default::default(),
        revision,
    ) {
        let Observation::Indexed {
            sessions,
            revision: reported,
        } = report
        else {
            panic!("new rows only index")
        };
        assert_eq!(reported, revision);
        lengths.push(sessions.len());
        seen.extend(
            sessions
                .into_iter()
                .map(|session| (session.key, session.incarnation)),
        );
    }
    assert_eq!(lengths, vec![256, 256, 88]);
    assert_eq!(seen, incarnations);
    assert!(
        discovery_report(
            1180,
            &records,
            &incarnations,
            &changed,
            &Default::default(),
            revision
        )
        .next()
        .is_none(),
        "history does not establish liveness"
    );
    assert!(
        discovery_report(
            1001,
            &records,
            &[],
            &changed,
            &Default::default(),
            Revision(0)
        )
        .next()
        .is_none(),
        "no invented existence evidence"
    );
}

#[test]
fn both_scan_paths_use_the_pure_seams_and_never_project_rich_rows() {
    let scan = include_str!("../mod.rs");
    let scoped = include_str!("../scoped.rs");
    for source in [scan, scoped] {
        let production = source.split("#[cfg(test)]").next().unwrap();
        for forbidden in ["ActivityEntry", "activity_entry(", "completion_entry("] {
            assert!(
                !production.contains(forbidden),
                "no producer projection: {forbidden}"
            );
        }
        for reporter in [
            "report_discovery(",
            "report_rejected(",
            "report_membership_changed(",
        ] {
            assert!(
                production.contains(reporter),
                "the producer uses {reporter}"
            );
        }
    }
    assert!(scan.contains("for observation in discovery_report("));
    assert!(
        scan.contains("membership_reports(described, &[(key.clone(), incarnation, revision)])")
    );
    assert!(scan.contains("membership_reports(described, &[]).index_changed"));
}

#[tokio::test(start_paused = true)]
async fn unchanged_launch_discovery_does_not_own_failed_seed_recovery() {
    use crate::session_lifecycle::{ReconcileSource, SessionEvent, SessionEvents, SessionRef};
    use crate::store::{ActiveCursor, Presence};
    use std::sync::{Arc, Mutex};

    struct FailingSeed {
        store: Store,
        requests: Mutex<Vec<Option<ActiveCursor>>>,
    }
    impl ReconcileSource for FailingSeed {
        fn presence(&self, keys: &[SessionKey]) -> anyhow::Result<(Vec<Presence>, Revision)> {
            self.store.session_presence_for_keys(keys)
        }
        fn active(
            &self,
            since: i64,
            after: Option<&ActiveCursor>,
            limit: usize,
        ) -> anyhow::Result<(Vec<Presence>, Revision)> {
            let mut requests = self.requests.lock().unwrap();
            requests.push(after.cloned());
            if requests.len() == 2 {
                anyhow::bail!("the second seed page fails");
            }
            self.store.sessions_active_since_page(since, after, limit)
        }
    }

    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open_in_memory(home.path()).unwrap();
    let paths: Vec<_> = (0..257)
        .map(|index| write_claude_session(home.path(), &format!("seed-{index:03}")))
        .collect();
    let previous = store.session_records().unwrap();
    let logs = || {
        paths
            .iter()
            .map(|path| log(AgentKind::Claude, path.clone(), 1_800_000_000))
            .collect()
    };
    let first = describe_with_states(logs(), home.path(), &HashSet::new(), &previous).await;
    assert_eq!(first.records.len(), 257);
    assert!(!persisted_facts(&store, &first, &previous).is_empty());
    let now = first
        .records
        .iter()
        .filter_map(|record| record.updated_at_epoch)
        .max()
        .unwrap();
    let previous = store.session_records().unwrap();
    let source = Arc::new(FailingSeed {
        store: store.clone(),
        requests: Mutex::new(Vec::new()),
    });
    let events = Arc::new(SessionEvents::default());
    let mut bus = events.subscribe();
    let actor = crate::session_lifecycle::tests::start_seeded(events.clone(), source.clone(), now);
    let seeded = events.snapshot(500);
    assert_eq!(seeded.total, 256);
    assert_eq!(seeded.seq, 0);
    let omitted = first
        .records
        .iter()
        .find(|record| {
            !seeded
                .sessions
                .iter()
                .any(|live| live.session == SessionRef::from(&record.key))
        })
        .unwrap()
        .key
        .clone();
    let awake = tokio::spawn(async {
        loop {
            tokio::task::yield_now().await;
        }
    });
    let unchanged = describe_with_states(logs(), home.path(), &HashSet::new(), &previous).await;
    let revision = store.revision();
    assert!(persisted_facts(&store, &unchanged, &previous).is_empty());
    assert_eq!(store.revision(), revision);
    assert!(membership_reports(&unchanged, &[]).index_changed.is_none());
    assert_eq!(events.snapshot(500).total, 256);
    assert_eq!(events.current_seq(), 0);
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    for _ in 0..5_000 {
        if events.snapshot(500).total == 257 {
            break;
        }
        tokio::task::yield_now().await;
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(events.snapshot(500).total, 257);
    let answer = events.presence(&[SessionRef::from(&omitted)]);
    assert_eq!(answer.seq, 1);
    assert_eq!(answer.present.len(), 1);
    assert!(answer.absent.is_empty());
    let event = bus.try_recv().unwrap();
    assert_eq!(event.seq, 1);
    assert!(
        matches!(event.event, SessionEvent::Activity { session: Some(session), .. } if session == SessionRef::from(&omitted))
    );
    assert_eq!(event.aggregate.unwrap().total, 257);
    assert!(
        bus.try_recv().is_err(),
        "seeded rows have no duplicate narration"
    );
    {
        let requests = source.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1], requests[2]);
    }
    awake.abort();
    actor.abort();
    assert!(actor.await.unwrap_err().is_cancelled());
}
