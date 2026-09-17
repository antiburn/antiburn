use super::*;
use crate::session_lifecycle::{Aggregate, AnonymousClearCause};
use antiburn_local::model::AgentKind;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

fn key(session_id: &str) -> SessionKey {
    SessionKey::new("native", "claude-code", session_id)
}

fn session_ref(session_id: &str) -> SessionRef {
    SessionRef::from(&key(session_id))
}

fn facets(title: bool, analysis: bool) -> UpdateFacets {
    UpdateFacets {
        title,
        analysis,
        ..Default::default()
    }
}

fn updated(seq: u64, session_id: &str, facets: UpdateFacets, at: i64) -> Sequenced {
    Sequenced {
        seq,
        event: SessionEvent::Updated {
            session: session_ref(session_id),
            facets,
            at,
        },
        aggregate: None,
    }
}

fn removed(seq: u64, session_id: Option<&str>, reason: RemovalReason) -> Sequenced {
    Sequenced {
        seq,
        event: SessionEvent::Removed {
            session: session_id.map(session_ref),
            reason,
        },
        aggregate: None,
    }
}

fn index_changed(seq: u64, reason: IndexChangeReason) -> Sequenced {
    Sequenced {
        seq,
        event: SessionEvent::IndexChanged { reason },
        aggregate: None,
    }
}

fn started(seq: u64, session_id: &str, at: i64) -> Sequenced {
    Sequenced {
        seq,
        event: SessionEvent::Started {
            session: session_ref(session_id),
            agent: AgentKind::Claude,
            at,
        },
        aggregate: None,
    }
}

fn anonymous_cleared(seq: u64, at: i64) -> Sequenced {
    Sequenced {
        seq,
        event: SessionEvent::AnonymousCleared {
            agent: AgentKind::Codex,
            at,
            cause: AnonymousClearCause::Expired,
        },
        aggregate: None,
    }
}

/// A synthetic enriched row for one session id.
fn entry(session_id: &str) -> ActivityEntry {
    ActivityEntry {
        agent: "claude-code".into(),
        session_id: session_id.into(),
        repo: "avery/widgets".into(),
        timestamp: "2026-08-01T10:00:00Z".into(),
        is_active: true,
        surface: "cli".into(),
        wsl_distro: None,
        title: None,
        has_fork_parent: false,
        fork_child_count: 0,
        cost: None,
        models: Vec::new(),
        model_runs: Vec::new(),
    }
}

fn loaded(ids: &[&str]) -> LoadResult {
    Ok(ids.iter().map(|id| (key(id), entry(id))).collect())
}

/// The session ids of every `Updated` emission, in order.
fn updated_ids(emissions: &[Emission]) -> Vec<String> {
    emissions
        .iter()
        .filter_map(|emission| match emission {
            Emission::Updated(payload) => Some(payload.session.session_id.clone()),
            _ => None,
        })
        .collect()
}

fn index_causes(emissions: &[Emission]) -> Vec<(u64, IndexChangeCause)> {
    emissions
        .iter()
        .filter_map(|emission| match emission {
            Emission::IndexChanged(payload) => Some((payload.seq, payload.cause)),
            _ => None,
        })
        .collect()
}

fn resync_seqs(emissions: &[Emission]) -> Vec<u64> {
    emissions
        .iter()
        .filter_map(|emission| match emission {
            Emission::Lifecycle(Sequenced {
                seq,
                event: SessionEvent::Resync,
                ..
            }) => Some(*seq),
            _ => None,
        })
        .collect()
}

/// Open the pending batch and return the keys it loads.
fn open(projector: &mut Projector) -> Vec<SessionKey> {
    let (immediates, keys) = projector.open_batch();
    assert!(immediates.is_empty(), "a row batch emits nothing at open");
    keys.expect("the batch has rows")
}

#[test]
fn updated_rows_coalesce_by_session_and_merge_facets() {
    let mut projector = Projector::default();

    assert!(
        projector
            .absorb(updated(1, "row", facets(true, false), 100))
            .is_empty()
    );
    assert!(
        projector
            .absorb(updated(2, "row", facets(false, true), 90))
            .is_empty()
    );
    assert!(
        projector
            .absorb(updated(3, "other", facets(false, true), 95))
            .is_empty()
    );

    let plan = projector.pending.take();
    assert!(!plan.index_refresh);
    assert_eq!(plan.rows.len(), 2);
    // Full identity order: "other" before "row".
    assert_eq!(plan.rows[0].0, key("other"));
    assert_eq!(
        plan.rows[1],
        (
            key("row"),
            PendingRow {
                facets: facets(true, true),
                at: 100,
                seq: 2,
            }
        )
    );
    assert_eq!(plan.seq, 3);
}

#[test]
fn lifecycle_transitions_relay_immediately_and_queue_no_rows() {
    let mut projector = Projector::default();
    let events = [
        SessionEvent::Started {
            session: session_ref("s"),
            agent: AgentKind::Claude,
            at: 1,
        },
        SessionEvent::Activity {
            session: Some(session_ref("s")),
            agent: AgentKind::Claude,
            at: 2,
            resumed: false,
        },
        SessionEvent::Quiet {
            session: session_ref("s"),
            agent: AgentKind::Claude,
            at: 3,
        },
        // The canonical anonymous clear relays at once, like every other
        // lifecycle transition: no reader keeps a timer for it.
        SessionEvent::AnonymousCleared {
            agent: AgentKind::Codex,
            at: 4,
            cause: AnonymousClearCause::Resolved,
        },
        SessionEvent::AnonymousCleared {
            agent: AgentKind::Codex,
            at: 5,
            cause: AnonymousClearCause::Expired,
        },
    ];

    for (index, event) in events.into_iter().enumerate() {
        // The batch's counts travel with the event the registry stamped,
        // untouched: the bridge neither drops nor invents them.
        let aggregate = (index % 2 == 0).then_some(Aggregate {
            working: index,
            total: index + 1,
            anonymous: 1,
            sweep: Vec::new(),
        });
        let sequenced = Sequenced {
            seq: index as u64 + 1,
            event,
            aggregate,
        };
        assert_eq!(
            projector.absorb(sequenced.clone()),
            vec![Immediate::Lifecycle(sequenced)]
        );
    }
    assert!(!projector.has_work());
}

#[test]
fn the_bridge_emits_the_stamped_aggregate_and_fabricates_none_on_lag() {
    let stamped = Sequenced {
        seq: 7,
        event: SessionEvent::Idle {
            session: session_ref("s"),
            agent: AgentKind::Claude,
            at: 9,
        },
        aggregate: Some(Aggregate {
            working: 2,
            total: 5,
            anonymous: 0,
            sweep: Vec::new(),
        }),
    };
    let envelope = LifecycleEnvelope {
        seq: stamped.seq,
        event: &stamped.event,
        aggregate: stamped.aggregate,
    };
    let json = serde_json::to_value(&envelope).unwrap();
    assert_eq!(json["seq"], 7);
    assert_eq!(json["kind"], "idle");
    assert_eq!(json["aggregate"]["working"], 2);
    assert_eq!(json["aggregate"]["total"], 5);

    let mut projector = Projector::default();
    let immediates = projector.absorb_lag(41);
    let Immediate::Lifecycle(resync) = &immediates[0] else {
        panic!("lag relays a lifecycle resync");
    };
    assert_eq!(resync.aggregate, None, "the bridge never invents counts");
    let envelope = LifecycleEnvelope {
        seq: resync.seq,
        event: &resync.event,
        aggregate: resync.aggregate.clone(),
    };
    let json = serde_json::to_value(&envelope).unwrap();
    assert_eq!(json["kind"], "resync");
    assert!(json.get("aggregate").is_none());

    // Emission passes the stamp through untouched.
    let source = include_str!("../session_projection.rs");
    assert!(source.contains("aggregate: sequenced.aggregate,"));
}

#[test]
fn a_synthetic_resync_relays_and_requests_an_index_refresh() {
    let mut projector = Projector::default();
    let resync = Sequenced {
        seq: 9,
        event: SessionEvent::Resync,
        aggregate: None,
    };

    let immediates = projector.absorb(resync.clone());

    assert_eq!(immediates, vec![Immediate::Lifecycle(resync)]);
    let plan = projector.pending.take();
    assert!(plan.index_refresh);
    assert!(plan.rows.is_empty());
    assert_eq!(plan.seq, 9);
}

#[test]
fn idle_relays_the_transition_and_queues_a_metadata_row() {
    let mut projector = Projector::default();
    let idle = Sequenced {
        seq: 4,
        event: SessionEvent::Idle {
            session: session_ref("sleepy"),
            agent: AgentKind::Claude,
            at: 200,
        },
        aggregate: None,
    };

    let immediates = projector.absorb(idle.clone());

    assert_eq!(immediates, vec![Immediate::Lifecycle(idle)]);
    let plan = projector.pending.take();
    assert_eq!(
        plan.rows,
        vec![(
            key("sleepy"),
            PendingRow {
                facets: UpdateFacets {
                    metadata: true,
                    ..Default::default()
                },
                at: 200,
                seq: 4,
            }
        )]
    );
}

#[test]
fn removed_drops_the_pending_row_and_names_the_removal() {
    let mut projector = Projector::default();
    let _ = projector.absorb(updated(1, "doomed", facets(true, false), 100));

    let immediates = projector.absorb(removed(2, Some("doomed"), RemovalReason::Deleted));

    assert_eq!(
        immediates,
        vec![Immediate::IndexChanged(IndexChangedPayload {
            seq: 2,
            cause: IndexChangeCause::Removed,
            session: Some(session_ref("doomed")),
            removal: Some(RemovalReason::Deleted),
        })]
    );
    assert!(!projector.has_work(), "the pending patch died with the row");
}

#[test]
fn index_change_reasons_map_to_causes() {
    let mut projector = Projector::default();

    let immediates = projector.absorb(index_changed(1, IndexChangeReason::ScanPass));
    assert_eq!(
        immediates,
        vec![Immediate::IndexChanged(IndexChangedPayload {
            seq: 1,
            cause: IndexChangeCause::ScanPass,
            session: None,
            removal: None,
        })]
    );

    let immediates = projector.absorb(index_changed(2, IndexChangeReason::Invalidated));
    assert_eq!(
        immediates,
        vec![Immediate::IndexChanged(IndexChangedPayload {
            seq: 2,
            cause: IndexChangeCause::Invalidated,
            session: None,
            removal: None,
        })]
    );
}

#[test]
fn the_row_cap_degrades_to_one_index_refresh() {
    let mut projector = Projector::default();
    for index in 0..PENDING_ROW_CAP {
        let _ = projector.absorb(updated(
            index as u64 + 1,
            &format!("s{index}"),
            facets(false, true),
            index as i64,
        ));
    }
    assert!(!projector.pending.index_refresh);

    // One more distinct session crosses the bound.
    let _ = projector.absorb(updated(9_999, "one-too-many", facets(false, true), 0));

    let plan = projector.pending.take();
    assert!(plan.index_refresh);
    assert!(plan.rows.is_empty(), "a refetch supersedes the patches");

    // The next batch starts clean.
    let _ = projector.absorb(updated(10_000, "fresh", facets(true, false), 1));
    let plan = projector.pending.take();
    assert!(!plan.index_refresh);
    assert_eq!(plan.rows.len(), 1);
}

#[test]
fn transport_lag_yields_resync_metadata_and_an_index_refresh() {
    let mut projector = Projector::default();
    let _ = projector.absorb(updated(3, "stale", facets(false, true), 10));

    let immediates = projector.absorb_lag(41);

    assert_eq!(
        immediates,
        vec![Immediate::Lifecycle(Sequenced {
            seq: 41,
            event: SessionEvent::Resync,
            aggregate: None,
        })]
    );
    let plan = projector.pending.take();
    assert!(plan.index_refresh);
    assert!(plan.rows.is_empty());
    assert_eq!(plan.seq, 41);
}

#[test]
fn an_index_refresh_supersedes_later_row_patches_in_the_same_batch() {
    let mut projector = Projector::default();
    let _ = projector.absorb_lag(5);

    let _ = projector.absorb(updated(6, "late", facets(true, false), 10));

    let plan = projector.pending.take();
    assert!(plan.index_refresh);
    assert!(plan.rows.is_empty());
}

#[test]
fn take_resets_the_batch_but_keeps_the_sequence_high_water_mark() {
    let mut projector = Projector::default();
    let _ = projector.absorb(updated(7, "row", facets(true, false), 10));

    let first = projector.pending.take();
    assert_eq!(first.rows.len(), 1);
    assert_eq!(first.seq, 7);

    assert!(!projector.has_work());
    let second = projector.pending.take();
    assert!(second.rows.is_empty());
    assert!(!second.index_refresh);
    assert_eq!(second.seq, 7);
}

#[test]
fn the_index_changed_payload_serializes_with_camel_case_and_snake_case_causes() {
    let payload = IndexChangedPayload {
        seq: 12,
        cause: IndexChangeCause::Removed,
        session: Some(session_ref("gone")),
        removal: Some(RemovalReason::Purged),
    };
    let json = serde_json::to_value(&payload).unwrap();
    assert_eq!(json["seq"], 12);
    assert_eq!(json["cause"], "removed");
    assert_eq!(json["session"]["sessionId"], "gone");
    assert_eq!(json["removal"], "purged");

    let bare = IndexChangedPayload {
        seq: 13,
        cause: IndexChangeCause::ScanPass,
        session: None,
        removal: None,
    };
    let json = serde_json::to_value(&bare).unwrap();
    assert_eq!(json["cause"], "scan_pass");
    assert!(json.get("session").is_none());
    assert!(json.get("removal").is_none());
}

#[test]
fn an_update_during_a_load_retires_the_loading_row_and_merges_its_facets() {
    let mut projector = Projector::default();
    let _ = projector.absorb(updated(1, "a", facets(true, false), 100));
    let _ = projector.absorb(updated(2, "b", facets(false, true), 100));
    assert_eq!(open(&mut projector), vec![key("a"), key("b")]);

    // A newer change for `a` arrives while its row loads.
    assert!(
        projector
            .absorb(updated(3, "a", facets(false, true), 120))
            .is_empty()
    );
    assert_eq!(
        projector
            .in_flight
            .as_ref()
            .unwrap()
            .rows
            .keys()
            .collect::<Vec<_>>(),
        vec![&key("b")],
        "the loading row for `a` is retired"
    );
    assert_eq!(
        projector.pending.rows[&key("a")],
        PendingRow {
            facets: facets(true, true),
            at: 120,
            seq: 3,
        },
        "the retired facets merge into the later projection"
    );

    // The load emits only `b`; `a` waits for the follow-up batch.
    let emissions = projector.complete(loaded(&["a", "b"]));
    assert_eq!(updated_ids(&emissions), vec!["b"]);
    assert!(index_causes(&emissions).is_empty(), "nothing is missing");
    assert!(!projector.loading());
    assert_eq!(open(&mut projector), vec![key("a")]);
    let emissions = projector.complete(loaded(&["a"]));
    assert_eq!(updated_ids(&emissions), vec!["a"]);
    let Emission::Updated(payload) = &emissions[0] else {
        panic!("a row emission");
    };
    assert_eq!(payload.seq, 3);
    assert_eq!(payload.facets, facets(true, true));
}

#[test]
fn an_idle_during_a_load_retires_the_loading_row_too() {
    let mut projector = Projector::default();
    let _ = projector.absorb(updated(1, "a", facets(true, false), 100));
    let _ = open(&mut projector);

    let idle = Sequenced {
        seq: 2,
        event: SessionEvent::Idle {
            session: session_ref("a"),
            agent: AgentKind::Claude,
            at: 300,
        },
        aggregate: None,
    };
    assert_eq!(
        projector.absorb(idle.clone()),
        vec![Immediate::Lifecycle(idle)]
    );
    assert!(projector.in_flight.as_ref().unwrap().rows.is_empty());
    assert_eq!(
        projector.pending.rows[&key("a")],
        PendingRow {
            facets: UpdateFacets {
                title: true,
                metadata: true,
                ..Default::default()
            },
            at: 300,
            seq: 2,
        }
    );
    assert!(updated_ids(&projector.complete(loaded(&["a"]))).is_empty());
}

#[test]
fn a_keyed_removal_during_a_load_suppresses_that_row_only() {
    let mut projector = Projector::default();
    let _ = projector.absorb(updated(1, "gone", facets(true, false), 100));
    let _ = projector.absorb(updated(2, "kept", facets(true, false), 100));
    let _ = open(&mut projector);

    let immediates = projector.absorb(removed(3, Some("gone"), RemovalReason::Deleted));
    assert_eq!(
        immediates,
        vec![Immediate::IndexChanged(IndexChangedPayload {
            seq: 3,
            cause: IndexChangeCause::Removed,
            session: Some(session_ref("gone")),
            removal: Some(RemovalReason::Deleted),
        })]
    );
    assert!(
        !projector.pending.has_work(),
        "a removed row is not re-queued"
    );

    // The loader still returns the row; it is not emitted, and its absence
    // is not "missing" either.
    let emissions = projector.complete(loaded(&["gone", "kept"]));
    assert_eq!(updated_ids(&emissions), vec!["kept"]);
    assert!(index_causes(&emissions).is_empty());
}

/// One way the bus can tell the loop that a refetch supersedes its rows.
type Invalidation = Box<dyn Fn(&mut Projector) -> Vec<Immediate>>;

#[test]
fn a_broad_removal_an_invalidation_or_lag_invalidates_the_batch_in_flight() {
    let cases: [(&str, Invalidation); 4] = [
        (
            "broad removal",
            Box::new(|projector| projector.absorb(removed(5, None, RemovalReason::Purged))),
        ),
        (
            "index invalidation",
            Box::new(|projector| {
                projector.absorb(index_changed(5, IndexChangeReason::Invalidated))
            }),
        ),
        ("lag", Box::new(|projector| projector.absorb_lag(5))),
        (
            "synthetic resync",
            Box::new(|projector| {
                projector.absorb(Sequenced {
                    seq: 5,
                    event: SessionEvent::Resync,
                    aggregate: None,
                })
            }),
        ),
    ];
    for (name, event) in cases {
        let mut projector = Projector::default();
        let _ = projector.absorb(updated(1, "a", facets(true, false), 100));
        let _ = projector.absorb(updated(2, "b", facets(true, false), 100));
        let _ = open(&mut projector);

        let immediates = event(&mut projector);
        assert_eq!(immediates.len(), 1, "{name}: one immediate");
        assert!(
            projector.in_flight.as_ref().unwrap().invalidated,
            "{name}: the batch in flight is invalidated"
        );

        // The load's rows are never applied after the refetch.
        let emissions = projector.complete(loaded(&["a", "b"]));
        assert!(emissions.is_empty(), "{name}: no stale patch");
        assert!(!projector.loading());
        match name {
            "broad removal" | "index invalidation" => {
                assert!(
                    !projector.pending.has_work(),
                    "{name}: the readers' own refetch covers the rows"
                );
            }
            _ => {
                let (immediates, keys) = projector.open_batch();
                assert!(keys.is_none());
                assert_eq!(
                    immediates,
                    vec![Immediate::IndexChanged(IndexChangedPayload {
                        seq: 5,
                        cause: IndexChangeCause::Resync,
                        session: None,
                        removal: None,
                    })],
                    "{name}: one index refresh follows"
                );
            }
        }
    }
}

#[test]
fn a_scan_pass_membership_change_keeps_the_batch_in_flight() {
    let mut projector = Projector::default();
    let _ = projector.absorb(updated(1, "a", facets(true, false), 100));
    let _ = open(&mut projector);

    let _ = projector.absorb(index_changed(2, IndexChangeReason::ScanPass));

    assert!(!projector.in_flight.as_ref().unwrap().invalidated);
    assert_eq!(updated_ids(&projector.complete(loaded(&["a"]))), vec!["a"]);
}

#[test]
fn a_missing_row_emits_the_found_rows_then_one_invalidation() {
    let mut projector = Projector::default();
    let _ = projector.absorb(updated(1, "a", facets(true, false), 100));
    let _ = projector.absorb(updated(2, "vanished", facets(true, false), 100));
    let _ = projector.absorb(updated(3, "c", facets(true, false), 100));
    let _ = open(&mut projector);

    let emissions = projector.complete(loaded(&["a", "c"]));

    assert_eq!(updated_ids(&emissions), vec!["a", "c"]);
    assert_eq!(
        index_causes(&emissions),
        vec![(3, IndexChangeCause::Invalidated)],
        "one refetch at the batch's sequence"
    );
    assert!(matches!(emissions.last(), Some(Emission::IndexChanged(_))));
    assert!(!projector.has_work(), "the missing row is not retried");
}

#[test]
fn a_failed_load_gives_its_rows_back_once_then_becomes_one_refetch() {
    let mut projector = Projector::default();
    let _ = projector.absorb(updated(1, "a", facets(true, false), 100));
    let _ = projector.absorb(updated(2, "b", facets(false, true), 100));
    let _ = open(&mut projector);
    // A newer change for `b` lands while the failing load runs.
    let _ = projector.absorb(updated(3, "b", facets(true, false), 110));

    let emissions = projector.complete(Err(anyhow::anyhow!("scripted load failure")));

    assert!(emissions.is_empty(), "the first failure emits nothing");
    assert!(!projector.loading());
    assert_eq!(projector.pending.rows.len(), 2, "both rows are back");
    assert_eq!(
        projector.pending.rows[&key("b")],
        PendingRow {
            facets: facets(true, true),
            at: 110,
            seq: 3,
        },
        "the given-back facets merge with the newer change"
    );
    assert!(projector.pending.retry);

    // The retry loads the same keys; a second failure is one refetch, not
    // an unbounded loop.
    assert_eq!(open(&mut projector), vec![key("a"), key("b")]);
    assert!(projector.in_flight.as_ref().unwrap().retried);
    let emissions = projector.complete(Err(anyhow::anyhow!("scripted load failure")));
    assert_eq!(
        index_causes(&emissions),
        vec![(3, IndexChangeCause::Invalidated)]
    );
    assert!(!projector.has_work());
    assert!(!projector.pending.retry, "the retry flag is consumed");

    // A retry that succeeds emits the rows and clears the flag for the
    // batch after it.
    let _ = projector.absorb(updated(4, "c", facets(true, false), 100));
    let _ = open(&mut projector);
    let _ = projector.complete(Err(anyhow::anyhow!("scripted load failure")));
    let _ = open(&mut projector);
    assert_eq!(updated_ids(&projector.complete(loaded(&["c"]))), vec!["c"]);
    let _ = projector.absorb(updated(5, "d", facets(true, false), 100));
    let _ = open(&mut projector);
    assert!(
        !projector.in_flight.as_ref().unwrap().retried,
        "a fresh batch gets its own retry"
    );
}

#[test]
fn a_failed_load_under_a_refetch_needs_no_retry() {
    let mut projector = Projector::default();
    let _ = projector.absorb(updated(1, "a", facets(true, false), 100));
    let _ = open(&mut projector);
    // The cap overflows while the load runs: one refetch is already due.
    for index in 0..=PENDING_ROW_CAP {
        let _ = projector.absorb(updated(
            index as u64 + 2,
            &format!("s{index}"),
            facets(false, true),
            index as i64,
        ));
    }
    assert!(projector.pending.index_refresh);

    let emissions = projector.complete(Err(anyhow::anyhow!("scripted load failure")));

    assert!(emissions.is_empty());
    assert!(
        projector.pending.rows.is_empty(),
        "the refetch supersedes the rows"
    );
    let (immediates, keys) = projector.open_batch();
    assert!(keys.is_none());
    assert_eq!(
        index_causes(
            &immediates
                .into_iter()
                .map(Emission::from)
                .collect::<Vec<_>>()
        )
        .len(),
        1
    );
}

#[test]
fn repeated_lag_in_one_cycle_coalesces_and_a_newer_gap_gets_a_follow_up() {
    let mut projector = Projector::default();

    // The first lag opens the cycle at the registry's sequence.
    assert_eq!(
        resync_seqs(
            &projector
                .absorb_lag(41)
                .into_iter()
                .map(Emission::from)
                .collect::<Vec<_>>()
        ),
        vec![41]
    );
    assert_eq!(
        projector.recovery,
        Some(Recovery {
            watermark: 41,
            follow_up: None
        })
    );
    // Lag at or below the watermark is covered by the snapshot the reader
    // re-reads: coalesced, no follow-up.
    assert!(projector.absorb_lag(41).is_empty());
    assert_eq!(projector.recovery.unwrap().follow_up, None);
    // Lag above the watermark may hide a newer gap: a follow-up is kept,
    // at the highest sequence seen.
    assert!(projector.absorb_lag(50).is_empty());
    assert!(projector.absorb_lag(60).is_empty());
    assert!(projector.absorb_lag(55).is_empty());
    assert_eq!(projector.recovery.unwrap().follow_up, Some(60));

    // The index refresh closes the cycle and the follow-up opens the next
    // one at its own sequence; no sequence here is invented.
    let (immediates, keys) = projector.open_batch();
    assert!(keys.is_none());
    let emissions = immediates
        .into_iter()
        .map(Emission::from)
        .collect::<Vec<_>>();
    assert_eq!(
        index_causes(&emissions),
        vec![(60, IndexChangeCause::Resync)]
    );
    assert_eq!(resync_seqs(&emissions), vec![60]);
    assert_eq!(
        projector.recovery,
        Some(Recovery {
            watermark: 60,
            follow_up: None
        })
    );
    assert!(
        projector.pending.index_refresh,
        "the follow-up refetches too"
    );

    // The follow-up's own refresh closes the cycle without another resync.
    let (immediates, keys) = projector.open_batch();
    assert!(keys.is_none());
    let emissions = immediates
        .into_iter()
        .map(Emission::from)
        .collect::<Vec<_>>();
    assert_eq!(
        index_causes(&emissions),
        vec![(60, IndexChangeCause::Resync)]
    );
    assert!(resync_seqs(&emissions).is_empty());
    assert_eq!(projector.recovery, None);
    assert!(!projector.has_work());

    // Lag after the cycle is a new cycle.
    assert_eq!(
        resync_seqs(
            &projector
                .absorb_lag(70)
                .into_iter()
                .map(Emission::from)
                .collect::<Vec<_>>()
        ),
        vec![70]
    );
}

#[test]
fn a_synthetic_resync_inside_a_cycle_raises_the_watermark_and_drops_a_covered_follow_up() {
    let mut projector = Projector::default();
    let _ = projector.absorb_lag(10);
    let _ = projector.absorb_lag(20);
    assert_eq!(projector.recovery.unwrap().follow_up, Some(20));

    // The synthetic marker at 25 covers the retained gap at 20.
    let resync = Sequenced {
        seq: 25,
        event: SessionEvent::Resync,
        aggregate: None,
    };
    assert_eq!(
        projector.absorb(resync.clone()),
        vec![Immediate::Lifecycle(resync)]
    );
    assert_eq!(
        projector.recovery,
        Some(Recovery {
            watermark: 25,
            follow_up: None
        })
    );
    let (immediates, _) = projector.open_batch();
    assert_eq!(immediates.len(), 1, "one index refresh, no follow-up");
}

/// A row loader that records every request, answers from a scripted row
/// set, fails on demand, and holds each answer until the test lets it go.
#[derive(Default)]
struct GatedLoader {
    rows: Mutex<HashMap<SessionKey, ActivityEntry>>,
    failures: Mutex<usize>,
    requests: Mutex<Vec<Vec<SessionKey>>>,
    /// `None`: loads return at once. `Some(n)`: `n` loads may return, then
    /// loads wait for `allow` or `release`.
    gate: (Mutex<Option<usize>>, Condvar),
    completed: AtomicUsize,
}

impl GatedLoader {
    fn with_rows(ids: &[&str]) -> Arc<Self> {
        let loader = Self::default();
        *loader.rows.lock().unwrap() = ids.iter().map(|id| (key(id), entry(id))).collect();
        Arc::new(loader)
    }

    fn fail_next(&self, loads: usize) {
        *self.failures.lock().unwrap() = loads;
    }

    fn requests(&self) -> Vec<Vec<SessionKey>> {
        self.requests.lock().unwrap().clone()
    }

    /// Hold every load after it is computed, until `allow` or `release`.
    fn hold(&self) {
        *self.gate.0.lock().unwrap() = Some(0);
    }

    fn allow(&self, loads: usize) {
        let mut gate = self.gate.0.lock().unwrap();
        *gate = Some(gate.unwrap_or(0) + loads);
        self.gate.1.notify_all();
    }

    fn release(&self) {
        *self.gate.0.lock().unwrap() = None;
        self.gate.1.notify_all();
    }

    /// Spin, without yielding to the loop, until `loads` loads have
    /// returned, then a short margin so the blocking task can finish.
    fn spin_until_completed(&self, loads: usize) {
        for _ in 0..5_000 {
            if self.completed.load(Ordering::Relaxed) >= loads {
                std::thread::sleep(Duration::from_millis(20));
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("the load did not complete in time");
    }
}

impl RowLoader for GatedLoader {
    fn load(&self, keys: &[SessionKey], _now: i64) -> LoadResult {
        assert!(
            keys.len() <= PENDING_ROW_CAP,
            "a load never exceeds the cap"
        );
        self.requests.lock().unwrap().push(keys.to_vec());
        let answer = {
            let mut failures = self.failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                Err(anyhow::anyhow!("scripted load failure"))
            } else {
                let rows = self.rows.lock().unwrap();
                Ok(keys
                    .iter()
                    .filter_map(|key| rows.get(key).map(|entry| (key.clone(), entry.clone())))
                    .collect())
            }
        };
        {
            // A held load waits for a permit. The wait is bounded so a
            // failing test cannot hang the runtime's shutdown on this
            // thread.
            let mut gate = self.gate.0.lock().unwrap();
            let opened = std::time::Instant::now();
            loop {
                match *gate {
                    None => break,
                    Some(0) => {
                        if opened.elapsed() > Duration::from_secs(15) {
                            break;
                        }
                        gate = self
                            .gate
                            .1
                            .wait_timeout(gate, Duration::from_millis(50))
                            .unwrap()
                            .0;
                    }
                    Some(permits) => {
                        *gate = Some(permits - 1);
                        break;
                    }
                }
            }
        }
        self.completed.fetch_add(1, Ordering::Relaxed);
        answer
    }
}

/// An emitter that keeps everything the loop hands it.
#[derive(Default)]
struct Recorder {
    emissions: Mutex<Vec<Emission>>,
}

impl Recorder {
    fn take(&self) -> Vec<Emission> {
        std::mem::take(&mut *self.emissions.lock().unwrap())
    }

    fn count(&self) -> usize {
        self.emissions.lock().unwrap().len()
    }
}

impl ProjectionEmitter for Recorder {
    fn emit(&self, emission: Emission) {
        self.emissions.lock().unwrap().push(emission);
    }
}

/// A running test loop: the bus sender, the loader, the recorder, and the
/// registry sequence the loop reads on lag.
struct Harness {
    bus: broadcast::Sender<Sequenced>,
    loader: Arc<GatedLoader>,
    recorder: Arc<Recorder>,
    watermark: Arc<AtomicU64>,
    task: JoinHandle<()>,
}

impl Harness {
    /// Put an event on the bus without yielding.
    fn send(&self, sequenced: Sequenced) {
        self.watermark.fetch_max(sequenced.seq, Ordering::Relaxed);
        let _ = self.bus.send(sequenced);
    }
}

fn start(loader: Arc<GatedLoader>, capacity: usize) -> Harness {
    let (bus, receiver) = broadcast::channel(capacity);
    let recorder = Arc::new(Recorder::default());
    let watermark = Arc::new(AtomicU64::new(0));
    let task = {
        let loader: Arc<dyn RowLoader> = loader.clone();
        let emitter: Arc<dyn ProjectionEmitter> = recorder.clone();
        let watermark = watermark.clone();
        tokio::spawn(async move {
            let watermark = move || watermark.load(Ordering::Relaxed);
            let now = || 1_800_000_000;
            run(receiver, loader, emitter, &watermark, &now).await;
        })
    };
    Harness {
        bus,
        loader,
        recorder,
        watermark,
        task,
    }
}

/// Let the loop receive without advancing the clock.
async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

/// Yield until `done` holds, sleeping a real millisecond between checks so
/// a blocking thread can make progress.
async fn wait_until(mut done: impl FnMut() -> bool) {
    for _ in 0..5_000 {
        if done() {
            return;
        }
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("the condition did not hold in time");
}

#[tokio::test(start_paused = true)]
async fn the_first_row_fixes_the_flush_deadline_and_later_rows_do_not_move_it() {
    let harness = start(GatedLoader::with_rows(&["a", "b"]), 16);

    harness.send(updated(1, "a", facets(true, false), 100));
    settle().await;
    tokio::time::advance(Duration::from_millis(200)).await;
    settle().await;
    assert!(harness.loader.requests().is_empty(), "nothing loads early");

    // A second row 200 ms in does not push the deadline out.
    harness.send(updated(2, "b", facets(true, false), 100));
    settle().await;
    tokio::time::advance(Duration::from_millis(49)).await;
    settle().await;
    assert!(harness.loader.requests().is_empty());
    tokio::time::advance(Duration::from_millis(1)).await;
    wait_until(|| harness.loader.requests().len() == 1).await;
    harness.loader.spin_until_completed(1);
    settle().await;

    assert_eq!(
        harness.loader.requests(),
        vec![vec![key("a"), key("b")]],
        "one load carries both rows, in identity order"
    );
    let emissions = harness.recorder.take();
    assert_eq!(updated_ids(&emissions), vec!["a", "b"]);
    let Emission::Updated(payload) = &emissions[1] else {
        panic!("a row emission");
    };
    assert_eq!(payload.seq, 2);

    // With nothing pending, no deadline is open: nothing else happens.
    tokio::time::advance(Duration::from_secs(5)).await;
    settle().await;
    assert_eq!(harness.loader.requests().len(), 1);
    assert_eq!(harness.recorder.count(), 0);
}

#[tokio::test(start_paused = true)]
async fn the_bus_is_received_while_a_load_runs_and_lifecycle_relays_at_once() {
    let harness = start(GatedLoader::with_rows(&["a", "b"]), 16);
    harness.loader.hold();

    harness.send(updated(1, "a", facets(true, false), 100));
    settle().await;
    tokio::time::advance(FLUSH_DELAY).await;
    wait_until(|| harness.loader.requests().len() == 1).await;
    assert_eq!(harness.loader.completed.load(Ordering::Relaxed), 0);

    // While the load is held: lifecycle events relay immediately, the
    // anonymous clear included, and a keyed removal is emitted at once.
    harness.send(started(2, "s", 5));
    harness.send(anonymous_cleared(3, 6));
    harness.send(removed(4, Some("other"), RemovalReason::Deleted));
    settle().await;
    let emissions = harness.recorder.take();
    assert_eq!(
        emissions.len(),
        3,
        "received during the load: {emissions:?}"
    );
    assert!(matches!(
        &emissions[0],
        Emission::Lifecycle(Sequenced {
            seq: 2,
            event: SessionEvent::Started { .. },
            ..
        })
    ));
    assert!(matches!(
        &emissions[1],
        Emission::Lifecycle(Sequenced {
            seq: 3,
            event: SessionEvent::AnonymousCleared { .. },
            ..
        })
    ));
    assert_eq!(
        index_causes(&emissions),
        vec![(4, IndexChangeCause::Removed)]
    );

    // A newer change for the loading row wins: the loaded row is not
    // emitted, and the follow-up batch carries the merged facets.
    harness.send(updated(5, "a", facets(false, true), 120));
    settle().await;
    harness.loader.release();
    harness.loader.spin_until_completed(1);
    settle().await;
    assert!(
        updated_ids(&harness.recorder.take()).is_empty(),
        "the retired row is not emitted from the first load"
    );
    tokio::time::advance(FLUSH_DELAY).await;
    wait_until(|| harness.loader.requests().len() == 2).await;
    harness.loader.spin_until_completed(2);
    settle().await;
    let emissions = harness.recorder.take();
    assert_eq!(updated_ids(&emissions), vec!["a"]);
    let Emission::Updated(payload) = &emissions[0] else {
        panic!("a row emission");
    };
    assert_eq!(payload.seq, 5);
    assert_eq!(payload.facets, facets(true, true));
}

#[tokio::test(start_paused = true)]
async fn a_deadline_that_passes_during_a_load_starts_the_next_batch_when_it_ends() {
    let harness = start(GatedLoader::with_rows(&["a", "b", "c"]), 16);
    harness.loader.hold();

    harness.send(updated(1, "a", facets(true, false), 100));
    settle().await;
    tokio::time::advance(FLUSH_DELAY).await;
    wait_until(|| harness.loader.requests().len() == 1).await;

    // The second batch opens during the load and its deadline passes
    // while the load is still held: no second load starts yet.
    harness.send(updated(2, "b", facets(true, false), 100));
    settle().await;
    tokio::time::advance(FLUSH_DELAY * 3).await;
    settle().await;
    assert_eq!(harness.loader.requests().len(), 1, "one load at a time");
    // A third row after the deadline joins the due batch; it does not
    // reopen the deadline.
    harness.send(updated(3, "c", facets(true, false), 100));
    settle().await;

    // The load ends: the due batch starts at once, without a new wait.
    harness.loader.allow(1);
    harness.loader.spin_until_completed(1);
    settle().await;
    wait_until(|| harness.loader.requests().len() == 2).await;
    assert_eq!(harness.loader.requests()[1], vec![key("b"), key("c")]);
    assert_eq!(updated_ids(&harness.recorder.take()), vec!["a"]);
    harness.loader.release();
    harness.loader.spin_until_completed(2);
    settle().await;
    assert_eq!(updated_ids(&harness.recorder.take()), vec!["b", "c"]);
}

#[tokio::test(start_paused = true)]
async fn a_broad_removal_during_a_load_discards_it_and_the_refetch_follows() {
    let harness = start(GatedLoader::with_rows(&["a", "b"]), 16);
    harness.loader.hold();

    harness.send(updated(1, "a", facets(true, false), 100));
    harness.send(updated(2, "b", facets(true, false), 100));
    settle().await;
    tokio::time::advance(FLUSH_DELAY).await;
    wait_until(|| harness.loader.requests().len() == 1).await;

    harness.send(removed(3, None, RemovalReason::Purged));
    settle().await;
    assert_eq!(
        index_causes(&harness.recorder.take()),
        vec![(3, IndexChangeCause::Removed)]
    );
    harness.loader.release();
    harness.loader.spin_until_completed(1);
    settle().await;
    tokio::time::advance(FLUSH_DELAY).await;
    settle().await;

    assert!(
        harness.recorder.take().is_empty(),
        "no stale patch after the refetch, and no second load"
    );
    assert_eq!(harness.loader.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_missing_row_and_a_failed_load_each_end_in_a_bounded_refetch() {
    let harness = start(GatedLoader::with_rows(&["a"]), 16);

    // Missing: the found row, then one invalidation.
    harness.send(updated(1, "a", facets(true, false), 100));
    harness.send(updated(2, "vanished", facets(true, false), 100));
    settle().await;
    tokio::time::advance(FLUSH_DELAY).await;
    wait_until(|| harness.loader.requests().len() == 1).await;
    harness.loader.spin_until_completed(1);
    settle().await;
    let emissions = harness.recorder.take();
    assert_eq!(updated_ids(&emissions), vec!["a"]);
    assert_eq!(
        index_causes(&emissions),
        vec![(2, IndexChangeCause::Invalidated)]
    );

    // Failed: one retry after a fresh deadline, then one invalidation.
    harness.loader.fail_next(2);
    harness.send(updated(3, "a", facets(false, true), 110));
    settle().await;
    tokio::time::advance(FLUSH_DELAY).await;
    wait_until(|| harness.loader.requests().len() == 2).await;
    harness.loader.spin_until_completed(2);
    settle().await;
    assert!(
        harness.recorder.take().is_empty(),
        "the first failure retries"
    );
    assert_eq!(
        harness.loader.requests().len(),
        2,
        "the retry waits its deadline"
    );
    tokio::time::advance(FLUSH_DELAY).await;
    wait_until(|| harness.loader.requests().len() == 3).await;
    harness.loader.spin_until_completed(3);
    settle().await;
    assert_eq!(harness.loader.requests()[2], vec![key("a")]);
    assert_eq!(
        index_causes(&harness.recorder.take()),
        vec![(3, IndexChangeCause::Invalidated)]
    );
    tokio::time::advance(FLUSH_DELAY * 4).await;
    settle().await;
    assert_eq!(harness.loader.requests().len(), 3, "no third attempt");
}

#[tokio::test(start_paused = true)]
async fn lag_relays_one_resync_and_keeps_a_follow_up_for_a_newer_gap() {
    let harness = start(GatedLoader::with_rows(&["a"]), 2);

    // Four events into a two-slot bus before the loop receives: lag.
    for seq in 1..=4 {
        harness.send(updated(seq, "a", facets(true, false), 100));
    }
    settle().await;
    let emissions = harness.recorder.take();
    assert_eq!(
        resync_seqs(&emissions),
        vec![4],
        "the registry's sequence, not an invented one"
    );
    assert!(
        index_causes(&emissions).is_empty(),
        "the refetch waits for the deadline"
    );

    // More lag in the same cycle, above the watermark: coalesced now, a
    // follow-up later.
    for seq in 5..=8 {
        harness.send(updated(seq, "a", facets(true, false), 100));
    }
    settle().await;
    assert!(harness.recorder.take().is_empty(), "coalesced");

    tokio::time::advance(FLUSH_DELAY).await;
    settle().await;
    let emissions = harness.recorder.take();
    assert_eq!(
        index_causes(&emissions),
        vec![(8, IndexChangeCause::Resync)]
    );
    assert_eq!(
        resync_seqs(&emissions),
        vec![8],
        "the follow-up names the newer gap"
    );
    assert!(
        harness.loader.requests().is_empty(),
        "rows lost to lag are not loaded"
    );

    // The follow-up's own refresh, then quiet.
    tokio::time::advance(FLUSH_DELAY).await;
    settle().await;
    let emissions = harness.recorder.take();
    assert_eq!(
        index_causes(&emissions),
        vec![(8, IndexChangeCause::Resync)]
    );
    assert!(resync_seqs(&emissions).is_empty());
    tokio::time::advance(FLUSH_DELAY * 4).await;
    settle().await;
    assert_eq!(harness.recorder.count(), 0);
}

#[tokio::test(start_paused = true)]
async fn shutdown_discards_the_load_in_flight() {
    let harness = start(GatedLoader::with_rows(&["a"]), 16);
    harness.loader.hold();

    harness.send(updated(1, "a", facets(true, false), 100));
    settle().await;
    tokio::time::advance(FLUSH_DELAY).await;
    wait_until(|| harness.loader.requests().len() == 1).await;

    let Harness {
        bus,
        loader,
        recorder,
        task,
        ..
    } = harness;
    drop(bus);
    task.await.expect("the loop ends when the bus closes");
    loader.release();
    loader.spin_until_completed(1);
    settle().await;
    assert_eq!(recorder.count(), 0, "the discarded load emits nothing");
}

/// Producers report observations; only this module emits session events.
/// A producer source that names a frontend session event constant is a
/// regression toward direct emission.
#[test]
fn no_producer_source_references_the_frontend_session_events() {
    let sources = [
        ("scan/mod.rs", include_str!("../scan/mod.rs")),
        ("scan/scoped.rs", include_str!("../scan/scoped.rs")),
        ("insights_worker.rs", include_str!("../insights_worker.rs")),
        ("retention.rs", include_str!("../retention.rs")),
        ("runtime_pricing.rs", include_str!("../runtime_pricing.rs")),
        (
            "session_lifecycle.rs",
            include_str!("../session_lifecycle.rs"),
        ),
    ];
    let forbidden = [
        "SESSION_UPDATED_EVENT",
        "SESSION_INDEX_CHANGED_EVENT",
        "SESSION_LIFECYCLE_EVENT",
    ];
    for (name, source) in sources {
        for needle in forbidden {
            assert!(
                !source.contains(needle),
                "{name} references {needle}: producers must report observations instead"
            );
        }
    }
}

/// The legacy direct-emission events are gone. No backend source may name
/// them again: rows travel as `session:updated`, membership as
/// `session:index-changed`, both from this module only.
#[test]
fn the_legacy_session_events_are_not_reintroduced() {
    let sources = [
        ("commands.rs", include_str!("../commands/mod.rs")),
        ("scan/mod.rs", include_str!("../scan/mod.rs")),
        ("scan/scoped.rs", include_str!("../scan/scoped.rs")),
        ("insights_worker.rs", include_str!("../insights_worker.rs")),
        ("retention.rs", include_str!("../retention.rs")),
        ("runtime_pricing.rs", include_str!("../runtime_pricing.rs")),
        (
            "session_lifecycle.rs",
            include_str!("../session_lifecycle.rs"),
        ),
        (
            "session_projection.rs",
            include_str!("../session_projection.rs"),
        ),
        ("lib.rs", include_str!("../lib.rs")),
    ];
    for (name, source) in sources {
        for needle in ["sessions:entry-changed", "sessions:invalidated"] {
            assert!(
                !source.contains(needle),
                "{name} names the removed legacy event {needle}"
            );
        }
    }
}

/// Only the projection worker may hand a session event to Tauri. A new
/// `emit` of a session event constant in `commands.rs` is a producer
/// bypassing the bridge.
#[test]
fn commands_do_not_emit_session_events_directly() {
    let commands = include_str!("../commands/mod.rs");
    for needle in [
        "emit(SESSION_LIFECYCLE_EVENT",
        "emit(SESSION_UPDATED_EVENT",
        "emit(SESSION_INDEX_CHANGED_EVENT",
    ] {
        assert!(
            !commands.contains(needle),
            "commands.rs emits a session event directly: {needle}"
        );
    }
}

/// The bridge is the one Tauri emitter of the three session scopes, and
/// every emission goes through [`ProjectionEmitter`], so the loop's
/// output is what the tests record.
#[test]
fn the_bridge_is_the_only_tauri_emitter_of_session_scopes() {
    let this = include_str!("../session_projection.rs");
    for needle in [
        "SESSION_LIFECYCLE_EVENT",
        "SESSION_UPDATED_EVENT",
        "SESSION_INDEX_CHANGED_EVENT",
    ] {
        assert_eq!(
            this.matches(needle).count(),
            1,
            "{needle} is emitted from exactly one place in the bridge"
        );
    }
    let bridge = this
        .split("impl ProjectionEmitter for AppHandle")
        .nth(1)
        .expect("the Tauri emitter exists");
    let bridge = bridge.split("\n}\n").next().unwrap();
    assert_eq!(
        bridge.matches("Emitter::emit(").count(),
        3,
        "the three scopes leave through the emitter impl"
    );
    let rest = this.replace(bridge, "");
    assert!(
        !rest.contains("Emitter::emit(") && !rest.contains(".emit(crate::commands::"),
        "no Tauri emit outside the emitter impl"
    );
}

#[tokio::test(start_paused = true)]
async fn queued_removal_wins_when_the_load_and_bus_are_ready_together() {
    let harness = start(GatedLoader::with_rows(&["a", "b"]), 16);
    harness.loader.hold();
    harness.send(updated(1, "a", facets(true, false), 100));
    harness.send(updated(2, "b", facets(true, false), 100));
    settle().await;
    tokio::time::advance(FLUSH_DELAY).await;
    wait_until(|| harness.loader.requests().len() == 1).await;
    harness.send(started(3, "other", 100));
    harness.send(removed(4, Some("a"), RemovalReason::Deleted));
    harness.send(updated(5, "b", facets(false, true), 101));
    harness.loader.release();
    harness.loader.spin_until_completed(1);
    settle().await;
    let emissions = harness.recorder.take();
    assert!(updated_ids(&emissions).is_empty());
    assert_eq!(
        index_causes(&emissions),
        vec![(4, IndexChangeCause::Removed)]
    );
    tokio::time::advance(FLUSH_DELAY).await;
    wait_until(|| harness.loader.requests().len() == 2).await;
    harness.loader.spin_until_completed(2);
    settle().await;
    let emissions = harness.recorder.take();
    assert_eq!(updated_ids(&emissions), vec!["b"]);
    let Emission::Updated(row) = &emissions[0] else {
        panic!("expected row")
    };
    assert_eq!(row.seq, 5);
    assert_eq!(row.facets, facets(true, true));
}

#[test]
fn overflow_suppresses_a_successful_inflight_load_and_bounds_both_batches() {
    let mut projector = Projector::default();
    for seq in 1..=PENDING_ROW_CAP {
        projector.absorb(updated(
            seq as u64,
            &format!("a{seq}"),
            facets(true, false),
            100,
        ));
    }
    assert_eq!(open(&mut projector).len(), PENDING_ROW_CAP);
    for seq in 1..=PENDING_ROW_CAP {
        projector.absorb(updated(
            (seq + PENDING_ROW_CAP) as u64,
            &format!("b{seq}"),
            facets(true, false),
            100,
        ));
    }
    assert_eq!(projector.pending.rows.len(), PENDING_ROW_CAP);
    assert_eq!(
        projector.in_flight.as_ref().unwrap().rows.len(),
        PENDING_ROW_CAP
    );
    projector.absorb(updated(2000, "overflow", facets(true, false), 100));
    assert!(projector.in_flight.as_ref().unwrap().invalidated);
    assert!(projector.pending.rows.is_empty());
    assert!(projector.complete(loaded(&["a1"])).is_empty());
    let (events, keys) = projector.open_batch();
    assert!(keys.is_none());
    assert_eq!(events.len(), 1);
}

#[test]
fn broad_invalidation_also_discards_older_pending_patches() {
    for event in [
        removed(3, None, RemovalReason::Purged),
        index_changed(3, IndexChangeReason::Invalidated),
    ] {
        let mut projector = Projector::default();
        projector.absorb(updated(1, "a", facets(true, false), 100));
        open(&mut projector);
        projector.absorb(updated(2, "b", facets(true, false), 100));
        assert_eq!(projector.absorb(event).len(), 1);
        assert!(!projector.has_work());
        assert!(projector.complete(loaded(&["a"])).is_empty());
        projector.absorb(updated(4, "b", facets(false, true), 101));
        assert_eq!(open(&mut projector), vec![key("b")]);
        let emissions = projector.complete(loaded(&["b"]));
        let Emission::Updated(row) = &emissions[0] else {
            panic!("expected row")
        };
        assert_eq!(row.seq, 4);
        assert_eq!(row.facets, facets(false, true));
    }
}

#[tokio::test(start_paused = true)]
async fn abort_discards_a_completed_but_unobserved_load() {
    let harness = start(GatedLoader::with_rows(&["a"]), 16);
    harness.loader.hold();
    harness.send(updated(1, "a", facets(true, false), 100));
    settle().await;
    tokio::time::advance(FLUSH_DELAY).await;
    wait_until(|| harness.loader.requests().len() == 1).await;
    harness.loader.release();
    harness.loader.spin_until_completed(1);
    harness.task.abort();
    assert!(harness.task.await.unwrap_err().is_cancelled());
    assert_eq!(harness.recorder.count(), 0);
}

#[tokio::test(start_paused = true)]
async fn lag_during_a_gated_load_suppresses_it_and_recovers_the_newer_gap() {
    let harness = start(GatedLoader::with_rows(&["a"]), 2);
    harness.loader.hold();
    harness.send(updated(1, "a", facets(true, false), 100));
    settle().await;
    tokio::time::advance(FLUSH_DELAY).await;
    wait_until(|| harness.loader.requests().len() == 1).await;
    for seq in 2..=5 {
        harness.send(anonymous_cleared(seq, 100));
    }
    settle().await;
    assert_eq!(resync_seqs(&harness.recorder.take()), vec![5]);
    for seq in 6..=9 {
        harness.send(anonymous_cleared(seq, 100));
    }
    settle().await;
    assert!(resync_seqs(&harness.recorder.take()).is_empty());
    tokio::time::advance(FLUSH_DELAY).await;
    harness.loader.release();
    harness.loader.spin_until_completed(1);
    settle().await;
    let emissions = harness.recorder.take();
    assert!(updated_ids(&emissions).is_empty());
    assert_eq!(
        index_causes(&emissions),
        vec![(9, IndexChangeCause::Resync)]
    );
    assert_eq!(resync_seqs(&emissions), vec![9]);
    tokio::time::advance(FLUSH_DELAY).await;
    settle().await;
    assert_eq!(
        index_causes(&harness.recorder.take()),
        vec![(9, IndexChangeCause::Resync)]
    );
    assert_eq!(harness.loader.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_failed_load_retries_successfully_without_losing_merged_facets() {
    let harness = start(GatedLoader::with_rows(&["a"]), 16);
    harness.loader.fail_next(1);
    harness.send(updated(1, "a", facets(true, false), 100));
    settle().await;
    tokio::time::advance(FLUSH_DELAY).await;
    wait_until(|| harness.loader.requests().len() == 1).await;
    harness.loader.spin_until_completed(1);
    settle().await;
    assert_eq!(harness.recorder.count(), 0);
    harness.send(updated(2, "a", facets(false, true), 101));
    settle().await;
    tokio::time::advance(FLUSH_DELAY).await;
    wait_until(|| harness.loader.requests().len() == 2).await;
    harness.loader.spin_until_completed(2);
    settle().await;
    let emissions = harness.recorder.take();
    assert_eq!(emissions.len(), 1);
    let Emission::Updated(row) = &emissions[0] else {
        panic!("expected retry row")
    };
    assert_eq!(row.seq, 2);
    assert_eq!(row.facets, facets(true, true));
}

#[tokio::test(start_paused = true)]
async fn a_panicked_loader_is_an_error_and_keeps_one_bounded_retry() {
    let mut projector = Projector::default();
    projector.absorb(updated(1, "a", facets(true, false), 100));
    open(&mut projector);
    let mut task = Some(tokio::task::spawn_blocking(|| -> LoadResult {
        panic!("scripted panic")
    }));
    let outcome = join_load(&mut task).await;
    assert!(outcome.is_err());
    assert!(projector.complete(outcome).is_empty());
    assert_eq!(open(&mut projector), vec![key("a")]);
    assert!(projector.in_flight.as_ref().unwrap().retried);
    let emissions = projector.complete(loaded(&["a"]));
    assert_eq!(updated_ids(&emissions), vec!["a"]);
}

#[tokio::test(start_paused = true)]
async fn removing_the_last_pending_row_gives_the_next_row_its_own_deadline() {
    let harness = start(GatedLoader::with_rows(&["a", "b"]), 16);
    harness.send(updated(1, "a", facets(true, false), 100));
    settle().await;
    tokio::time::advance(Duration::from_millis(200)).await;
    harness.send(removed(2, Some("a"), RemovalReason::Deleted));
    settle().await;
    harness.send(updated(3, "b", facets(true, false), 101));
    settle().await;
    tokio::time::advance(Duration::from_millis(249)).await;
    settle().await;
    assert!(harness.loader.requests().is_empty());
    tokio::time::advance(Duration::from_millis(1)).await;
    wait_until(|| harness.loader.requests().len() == 1).await;
    assert_eq!(harness.loader.requests(), vec![vec![key("b")]]);
    harness.loader.spin_until_completed(1);
    settle().await;
    assert_eq!(updated_ids(&harness.recorder.take()), vec!["b"]);
}
