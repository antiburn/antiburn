//! The admission schedules S1–S16 and the evidence property, driven
//! against the registry directly: every fact and page is applied in a
//! chosen order with a fixed clock, so each end state is exact.

use super::*;

/// A registry with a fixed clock, a scripted page helper, and readers for
/// the three evidence sets.
struct Sim {
    registry: Registry,
    now: i64,
}

/// A live entry's evidence: incarnation, `exists_at`, `last_activity_at`.
type LiveEvidence = (u64, u64, i64);

impl Sim {
    fn new() -> Self {
        Self {
            registry: Registry::default(),
            now: BASE,
        }
    }

    fn with_guards(forgotten: u64, broad: u64) -> Self {
        let mut sim = Self::new();
        sim.registry.forgotten_through = Revision(forgotten);
        sim.registry.broad_through = Revision(broad);
        sim
    }

    /// Apply one observation with an unlimited fact budget.
    fn apply(&mut self, observation: Observation) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        let mut budget = usize::MAX;
        let progress = self
            .registry
            .observe(observation, self.now, &mut budget, &mut out);
        assert!(
            matches!(progress, Progress::Done),
            "the fact applied in full"
        );
        out
    }

    /// Answer one admission page for every pending key from `rows` at
    /// `revision`, applied against the current state.
    fn admission_page(&mut self, rows: Vec<Presence>, revision: u64) -> Vec<SessionEvent> {
        let request = self
            .registry
            .next_admission_page()
            .expect("pending keys wait for a page");
        self.page(request, rows, revision)
    }

    /// Answer one page of `request` with `rows` at `revision`.
    fn page(
        &mut self,
        request: PageRequest,
        rows: Vec<Presence>,
        revision: u64,
    ) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        self.registry
            .apply_page(&request, rows, Revision(revision), self.now, &mut out);
        out
    }

    /// Issue and answer pages from `store` until nothing wants one.
    fn reconcile_with(&mut self, store: &BTreeMap<SessionKey, (u64, i64)>, revision: u64) {
        for _ in 0..100 {
            let Some(request) = self.registry.next_page_request(self.now) else {
                return;
            };
            let rows = request
                .keys()
                .iter()
                .filter_map(|key| {
                    store.get(key).map(|(inc, epoch)| Presence {
                        key: key.clone(),
                        incarnation: Incarnation(*inc),
                        epoch: *epoch,
                    })
                })
                .collect();
            self.page(request, rows, revision);
        }
        panic!("reconciliation did not reach quiescence");
    }

    fn live(&self, session_id: &str) -> Option<LiveEvidence> {
        self.registry.live.get(&key(session_id)).map(|entry| {
            (
                entry.incarnation.0,
                entry.exists_at.0,
                entry.last_activity_at,
            )
        })
    }

    fn deleted(&self, session_id: &str) -> Option<(u64, u64)> {
        self.registry
            .deleted
            .get(&key(session_id))
            .map(|deletion| (deletion.incarnation.0, deletion.absent_at.0))
    }

    fn pending(&self, session_id: &str) -> Option<(u64, i64, u64)> {
        self.registry
            .pending
            .get(&key(session_id))
            .map(|pending| (pending.incarnation.0, pending.at, pending.revision.0))
    }
}

/// `I(i, at, r)`: incarnation `i` of `k` indexed at revision `r`.
fn i(inc: u64, at: i64, revision: u64) -> Observation {
    index("k", inc, at, false, revision)
}

/// `I(i, at, r, new)`.
fn i_new(inc: u64, at: i64, revision: u64) -> Observation {
    index("k", inc, at, true, revision)
}

/// `T(i, at, seen)`: a watcher touch of incarnation `i` looked up at `seen`.
fn t(inc: u64, at: i64, seen: u64) -> Observation {
    touched("k", inc, at, seen)
}

/// `X(i, d)`: incarnation `i` of `k` deleted at revision `d`.
fn x(inc: u64, revision: u64) -> Observation {
    removed("k", inc, RemovalReason::Deleted, revision)
}

/// `B@b`: a broad purge at revision `b`.
fn b(revision: u64) -> Observation {
    broad(RemovalReason::Purged, revision)
}

fn k_present(inc: u64, epoch: i64) -> Vec<Presence> {
    vec![presence("k", inc, epoch)]
}

fn started(at: i64) -> SessionEvent {
    SessionEvent::Started {
        session: session_ref("k"),
        agent: AgentKind::Claude,
        at,
    }
}

fn activity(at: i64, resumed: bool) -> SessionEvent {
    SessionEvent::Activity {
        session: Some(session_ref("k")),
        agent: AgentKind::Claude,
        at,
        resumed,
    }
}

fn idle(at: i64) -> SessionEvent {
    SessionEvent::Idle {
        session: session_ref("k"),
        agent: AgentKind::Claude,
        at,
    }
}

fn removed_k(reason: RemovalReason) -> SessionEvent {
    SessionEvent::Removed {
        session: Some(session_ref("k")),
        reason,
    }
}

/// Every order of three facts.
fn permutations3<T: Clone>(items: [T; 3]) -> Vec<Vec<T>> {
    let [a, b, c] = items;
    vec![
        vec![a.clone(), b.clone(), c.clone()],
        vec![a.clone(), c.clone(), b.clone()],
        vec![b.clone(), a.clone(), c.clone()],
        vec![b.clone(), c.clone(), a.clone()],
        vec![c.clone(), a.clone(), b.clone()],
        vec![c, b, a],
    ]
}

#[test]
fn s1_index_then_removal_removes_the_entry_and_remembers_the_deletion() {
    let mut sim = Sim::new();
    assert_eq!(sim.apply(i(1, BASE, 5)), vec![activity(BASE, false)]);
    assert_eq!(sim.live("k"), Some((1, 5, BASE)));
    assert_eq!(
        sim.apply(x(1, 6)),
        vec![idle(BASE), removed_k(RemovalReason::Deleted)]
    );
    assert_eq!(sim.live("k"), None);
    assert_eq!(sim.deleted("k"), Some((1, 6)));
}

#[test]
fn s1_prime_removal_then_index_of_the_same_incarnation_is_stale() {
    let mut sim = Sim::new();
    assert_eq!(sim.apply(x(1, 6)), vec![removed_k(RemovalReason::Deleted)]);
    assert_eq!(sim.deleted("k"), Some((1, 6)));
    assert_eq!(sim.apply(i(1, BASE, 5)), Vec::new());
    assert_eq!(sim.live("k"), None);
    assert_eq!(sim.pending("k"), None);
}

#[test]
fn s2_removal_then_a_higher_incarnation_starts() {
    let mut sim = Sim::new();
    sim.apply(x(1, 6));
    assert_eq!(sim.apply(i_new(2, BASE, 7)), vec![started(BASE)]);
    assert_eq!(sim.live("k"), Some((2, 7, BASE)));
    assert_eq!(sim.deleted("k"), Some((1, 6)));
}

#[test]
fn s2_prime_a_higher_incarnation_then_the_older_removal_keeps_the_entry() {
    let mut sim = Sim::new();
    sim.apply(i_new(2, BASE, 7));
    assert_eq!(
        sim.apply(x(1, 6)),
        Vec::new(),
        "the live row is newer: nothing to say"
    );
    assert_eq!(sim.live("k"), Some((2, 7, BASE)));
    assert_eq!(sim.deleted("k"), Some((1, 6)));
}

#[test]
fn s3_a_forgotten_deletion_defers_the_fact_and_the_page_records_absence() {
    let mut sim = Sim::with_guards(6, 0);
    assert_eq!(sim.apply(i(1, BASE, 5)), Vec::new());
    assert_eq!(sim.live("k"), None);
    assert_eq!(sim.pending("k"), Some((1, BASE, 5)));
    // The page reads current truth: absent at R >= 5, so incarnation 1 is
    // dead and the key never becomes live.
    assert_eq!(sim.admission_page(Vec::new(), 9), Vec::new());
    assert_eq!(sim.pending("k"), None);
    assert_eq!(sim.deleted("k"), Some((1, 9)));
    assert_eq!(sim.live("k"), None);
}

#[test]
fn s4_a_broad_purge_then_the_index_defers_and_the_page_drops_it() {
    let mut sim = Sim::new();
    assert_eq!(
        sim.apply(b(6)),
        vec![SessionEvent::Removed {
            session: None,
            reason: RemovalReason::Purged,
        }]
    );
    assert_eq!(sim.registry.broad_through, Revision(6));
    assert_eq!(sim.apply(i(1, BASE, 5)), Vec::new());
    assert_eq!(sim.pending("k"), Some((1, BASE, 5)));
    sim.admission_page(Vec::new(), 8);
    assert_eq!(sim.pending("k"), None);
    assert_eq!(sim.live("k"), None);
    assert_eq!(sim.deleted("k"), Some((1, 8)));
}

#[test]
fn s4_prime_the_index_then_a_broad_purge_walks_and_removes_the_entry() {
    let mut sim = Sim::new();
    sim.apply(i(1, BASE, 5));
    sim.apply(b(6));
    let request = sim
        .registry
        .next_page_request(BASE)
        .expect("the purge wants a walk");
    assert_eq!(
        request,
        PageRequest::Walk {
            keys: vec![key("k")],
            reason: RemovalReason::Purged,
        }
    );
    assert_eq!(
        sim.page(request, Vec::new(), 8),
        vec![idle(BASE), removed_k(RemovalReason::Purged)]
    );
    assert_eq!(sim.live("k"), None);
    assert_eq!(sim.deleted("k"), Some((1, 8)));
    assert_eq!(
        sim.registry.next_page_request(BASE),
        None,
        "the walk is complete"
    );
}

#[test]
fn s5_a_touch_advances_the_live_entry_and_the_removal_ends_it() {
    let mut sim = Sim::new();
    sim.apply(i(1, BASE, 3));
    assert_eq!(
        sim.apply(t(1, BASE + 100, 5)),
        vec![activity(BASE + 100, false)]
    );
    assert_eq!(sim.live("k"), Some((1, 5, BASE + 100)));
    assert_eq!(
        sim.apply(x(1, 6)),
        vec![idle(BASE), removed_k(RemovalReason::Deleted)]
    );
    assert_eq!(sim.live("k"), None);
}

#[test]
fn s5_prime_a_removal_then_the_touch_of_the_dead_incarnation_is_stale() {
    let mut sim = Sim::new();
    sim.apply(i(1, BASE, 3));
    sim.apply(x(1, 6));
    assert_eq!(sim.apply(t(1, BASE + 100, 5)), Vec::new());
    assert_eq!(sim.live("k"), None);
    assert_eq!(sim.pending("k"), None);
}

#[test]
fn s6_a_removal_then_a_present_page_of_the_dead_incarnation_is_stale() {
    let mut sim = Sim::with_guards(8, 0);
    sim.apply(i(1, BASE, 5));
    assert_eq!(sim.pending("k"), Some((1, BASE, 5)));
    sim.apply(x(1, 11));
    assert_eq!(
        sim.pending("k"),
        None,
        "the removal drops the pending entry"
    );
    assert_eq!(sim.deleted("k"), Some((1, 11)));
    // A page computed before the delete arrives late.
    let request = PageRequest::Admission {
        keys: vec![key("k")],
    };
    assert_eq!(sim.page(request, k_present(1, BASE), 10), Vec::new());
    assert_eq!(sim.live("k"), None);
}

#[test]
fn s6_prime_a_present_page_admits_and_the_later_removal_ends_it() {
    let mut sim = Sim::with_guards(8, 0);
    sim.apply(i(1, BASE, 5));
    assert_eq!(
        sim.admission_page(k_present(1, BASE), 10),
        vec![activity(BASE, false)]
    );
    assert_eq!(sim.live("k"), Some((1, 10, BASE)));
    assert_eq!(
        sim.apply(x(1, 11)),
        vec![idle(BASE), removed_k(RemovalReason::Deleted)]
    );
    assert_eq!(sim.live("k"), None);
}

#[test]
fn s7_an_absent_page_records_the_pending_incarnation_dead_then_a_newer_one_starts() {
    let mut sim = Sim::with_guards(8, 0);
    sim.apply(i(1, BASE, 5));
    sim.admission_page(Vec::new(), 10);
    assert_eq!(sim.pending("k"), None);
    assert_eq!(sim.deleted("k"), Some((1, 10)));
    assert_eq!(sim.apply(i_new(2, BASE, 11)), vec![started(BASE)]);
    assert_eq!(sim.live("k"), Some((2, 11, BASE)));
}

#[test]
fn s7_prime_an_absent_page_older_than_the_live_entry_keeps_it() {
    let mut sim = Sim::with_guards(8, 0);
    sim.apply(i_new(2, BASE, 11));
    assert_eq!(sim.live("k"), Some((2, 11, BASE)));
    let request = PageRequest::Admission {
        keys: vec![key("k")],
    };
    assert_eq!(sim.page(request, Vec::new(), 10), Vec::new());
    assert_eq!(sim.live("k"), Some((2, 11, BASE)));
}

#[test]
fn s9_a_spill_cell_folds_two_removals_into_the_highest_of_each() {
    let mut spill = Spill::default();
    spill.fold(sync_removed("k", 1, RemovalReason::Deleted, 6));
    spill.fold(sync_removed("k", 2, RemovalReason::Purged, 9));
    let cell = spill.removed[&key("k")];
    assert_eq!(
        cell,
        RemovedCell {
            incarnation: Incarnation(2),
            reason: RemovalReason::Purged,
            revision: Revision(9),
        }
    );

    // Applying the cell equals applying X(1,6) then X(2,9).
    let mut folded = Sim::new();
    folded.apply(i(2, BASE, 7));
    let mut out = Vec::new();
    folded.registry.remove(
        &key("k"),
        cell.incarnation,
        cell.reason,
        cell.revision,
        BASE,
        &mut out,
    );
    let mut sequential = Sim::new();
    sequential.apply(i(2, BASE, 7));
    sequential.apply(x(1, 6));
    sequential.apply(removed("k", 2, RemovalReason::Purged, 9));
    assert_eq!(folded.live("k"), sequential.live("k"));
    assert_eq!(folded.deleted("k"), sequential.deleted("k"));
    assert_eq!(folded.deleted("k"), Some((2, 9)));
    assert_eq!(out, vec![idle(BASE), removed_k(RemovalReason::Purged)]);
}

#[test]
fn s10_every_order_of_delete_create_delete_ends_not_live_with_the_highest_deletion() {
    for order in permutations3([x(1, 6), i_new(2, BASE, 7), x(2, 8)]) {
        let mut sim = Sim::new();
        for fact in order.clone() {
            sim.apply(fact);
        }
        assert_eq!(sim.live("k"), None, "{order:?}");
        assert_eq!(sim.pending("k"), None, "{order:?}");
        assert_eq!(sim.deleted("k"), Some((2, 8)), "{order:?}");
    }
}

#[test]
fn s12_a_broad_purge_during_a_walk_starts_a_second_run_above_the_first() {
    let mut sim = Sim::new();
    for index in 0..3 {
        sim.apply(index_fact(&format!("s{index}"), 1, BASE, 5));
    }
    sim.apply(b(10));
    // The first walk page is out. A newer purge arrives mid-walk.
    let first = sim.registry.next_page_request(BASE).expect("a walk page");
    assert!(matches!(first, PageRequest::Walk { .. }));
    sim.apply(b(12));
    assert_eq!(
        sim.registry.reconcile.needs,
        Some((RemovalReason::Purged, Revision(12)))
    );
    // Every row is still present: the page confirms them at 11.
    let rows = first
        .keys()
        .iter()
        .map(|key| Presence {
            key: key.clone(),
            incarnation: Incarnation(1),
            epoch: BASE,
        })
        .collect();
    sim.page(first, rows, 11);
    for index in 0..3 {
        assert_eq!(
            sim.registry.live[&key(&format!("s{index}"))].exists_at,
            Revision(11)
        );
    }
    // The first run is complete; entries confirmed at 11 still predate 12,
    // so a second run covers them.
    let second = sim.registry.next_page_request(BASE).expect("a second run");
    assert_eq!(
        second,
        PageRequest::Walk {
            keys: (0..3).map(|index| key(&format!("s{index}"))).collect(),
            reason: RemovalReason::Purged,
        }
    );
    assert_eq!(sim.registry.reconcile.needs, None);
    sim.page(second, Vec::new(), 13);
    assert!(
        sim.registry.live.is_empty(),
        "absent at 13: every entry removed"
    );
    assert_eq!(sim.registry.next_page_request(BASE), None);
}

#[test]
fn s13_a_guard_advance_during_a_page_defers_again_and_the_next_page_admits() {
    let mut sim = Sim::with_guards(8, 0);
    sim.apply(i(1, BASE, 5));
    let request = sim.registry.next_admission_page().expect("pending");
    // A purge lands while the page (computed at 10) is in flight.
    sim.apply(b(12));
    assert_eq!(sim.page(request, k_present(1, BASE), 10), Vec::new());
    assert_eq!(sim.live("k"), None, "10 < 12: deferred again");
    assert_eq!(sim.pending("k"), Some((1, BASE, 10)));
    // The next page reads at or above the guard and admits.
    assert_eq!(
        sim.admission_page(k_present(1, BASE), 12),
        vec![activity(BASE, false)]
    );
    assert_eq!(sim.live("k"), Some((1, 12, BASE)));
}

/// S14, the adversarial schedule: a lookup of incarnation 1 at revision 5
/// with watcher time 100, then the delete of incarnation 1 at 6, then the
/// re-create of incarnation 2 (epoch 90) at 7. In every receipt order the
/// end state is incarnation 2 at 90, and the stale touch never imports 100.
#[test]
fn s14_every_receipt_order_ends_with_incarnation_two_at_its_own_epoch() {
    let facts = [t(1, BASE + 100, 5), x(1, 6), i_new(2, BASE + 90, 7)];
    for order in permutations3(facts) {
        let mut sim = Sim::new();
        for fact in order.clone() {
            sim.apply(fact);
        }
        assert_eq!(sim.live("k"), Some((2, 7, BASE + 90)), "{order:?}");
        let deleted = sim.deleted("k").expect("incarnation 1 is remembered dead");
        assert_eq!(deleted.0, 1, "{order:?}");
        assert!(deleted.1 >= 6, "{order:?}");
        assert_eq!(sim.pending("k"), None, "{order:?}");
    }
}

#[test]
fn s14_detail_index_removal_touch() {
    let mut sim = Sim::new();
    assert_eq!(sim.apply(i_new(2, BASE + 90, 7)), vec![started(BASE + 90)]);
    assert_eq!(sim.apply(x(1, 6)), Vec::new());
    assert_eq!(sim.deleted("k"), Some((1, 6)));
    assert_eq!(sim.apply(t(1, BASE + 100, 5)), Vec::new(), "1 < 2: stale");
    assert_eq!(sim.live("k"), Some((2, 7, BASE + 90)));
}

#[test]
fn s14_detail_touch_index_removal() {
    let mut sim = Sim::new();
    assert_eq!(
        sim.apply(t(1, BASE + 100, 5)),
        vec![activity(BASE + 100, false)]
    );
    assert_eq!(
        sim.apply(i_new(2, BASE + 90, 7)),
        vec![
            idle(BASE),
            removed_k(RemovalReason::Reconciled),
            started(BASE + 90),
        ]
    );
    assert_eq!(sim.deleted("k"), Some((1, 7)));
    assert_eq!(sim.apply(x(1, 6)), Vec::new());
    assert_eq!(
        sim.deleted("k"),
        Some((1, 7)),
        "a removal never lowers a revision"
    );
    assert_eq!(sim.live("k"), Some((2, 7, BASE + 90)));
}

#[test]
fn s14_detail_removal_touch_index() {
    let mut sim = Sim::new();
    sim.apply(x(1, 6));
    assert_eq!(sim.apply(t(1, BASE + 100, 5)), Vec::new());
    assert_eq!(sim.apply(i_new(2, BASE + 90, 7)), vec![started(BASE + 90)]);
    assert_eq!(sim.live("k"), Some((2, 7, BASE + 90)));
}

#[test]
fn s14_detail_touch_removal_index() {
    let mut sim = Sim::new();
    sim.apply(t(1, BASE + 100, 5));
    assert_eq!(
        sim.apply(x(1, 6)),
        vec![idle(BASE), removed_k(RemovalReason::Deleted)]
    );
    assert_eq!(sim.apply(i_new(2, BASE + 90, 7)), vec![started(BASE + 90)]);
    assert_eq!(sim.live("k"), Some((2, 7, BASE + 90)));
}

#[test]
fn s14_detail_index_touch_removal_and_removal_index_touch() {
    let mut sim = Sim::new();
    sim.apply(i_new(2, BASE + 90, 7));
    assert_eq!(sim.apply(t(1, BASE + 100, 5)), Vec::new());
    assert_eq!(sim.apply(x(1, 6)), Vec::new());
    assert_eq!(sim.live("k"), Some((2, 7, BASE + 90)));

    let mut sim = Sim::new();
    sim.apply(x(1, 6));
    sim.apply(i_new(2, BASE + 90, 7));
    assert_eq!(sim.apply(t(1, BASE + 100, 5)), Vec::new());
    assert_eq!(sim.live("k"), Some((2, 7, BASE + 90)));
}

/// S14a: a genuine touch of the current incarnation advances, whatever its
/// `seen`; the same touch attributed to incarnation 1 is stale.
#[test]
fn a_same_incarnation_touch_with_an_older_seen_advances() {
    let mut sim = Sim::new();
    sim.apply(i(2, BASE + 90, 7));
    assert_eq!(
        sim.apply(t(2, BASE + 100, 4)),
        vec![activity(BASE + 100, false)]
    );
    assert_eq!(sim.live("k"), Some((2, 7, BASE + 100)));

    let mut sim = Sim::new();
    assert_eq!(
        sim.apply(t(2, BASE + 100, 8)),
        vec![activity(BASE + 100, false)]
    );
    assert_eq!(
        sim.apply(i(2, BASE + 90, 7)),
        Vec::new(),
        "an older epoch is unchanged"
    );
    assert_eq!(sim.live("k"), Some((2, 8, BASE + 100)));

    let mut sim = Sim::new();
    sim.apply(i(2, BASE + 90, 7));
    assert_eq!(sim.apply(t(1, BASE + 100, 5)), Vec::new());
    assert_eq!(sim.live("k"), Some((2, 7, BASE + 90)));
}

/// S14b: the deletion of incarnation 1 was evicted (`forgotten_through=6`).
#[test]
fn s14b_a_forgotten_deletion_defers_the_stale_touch_and_the_re_create_admits_directly() {
    let mut sim = Sim::with_guards(6, 0);
    assert_eq!(sim.apply(t(1, BASE + 100, 5)), Vec::new());
    assert_eq!(sim.pending("k"), Some((1, BASE + 100, 5)));
    // 7 >= 6: the re-create admits directly and proves the pending
    // incarnation dead. No page is needed for this key.
    assert_eq!(sim.apply(i_new(2, BASE + 90, 7)), vec![started(BASE + 90)]);
    assert_eq!(sim.live("k"), Some((2, 7, BASE + 90)));
    assert_eq!(sim.pending("k"), None);
    assert_eq!(sim.deleted("k"), Some((1, 7)));

    // Reverse order: live first, then the stale touch is rejected at once.
    let mut sim = Sim::with_guards(6, 0);
    sim.apply(i_new(2, BASE + 90, 7));
    assert_eq!(sim.apply(t(1, BASE + 100, 5)), Vec::new());
    assert_eq!(sim.pending("k"), None);
    assert_eq!(sim.live("k"), Some((2, 7, BASE + 90)));
}

/// S14b with the page: the stale touch is pending when the page answers
/// with a higher incarnation.
#[test]
fn a_page_row_with_a_higher_incarnation_replaces_pending_without_importing_its_at() {
    let mut sim = Sim::with_guards(6, 0);
    sim.apply(t(1, BASE + 100, 5));
    assert_eq!(
        sim.admission_page(k_present(2, BASE + 90), 9),
        vec![activity(BASE + 90, false)],
        "the page admits incarnation 2 from its own epoch"
    );
    assert_eq!(
        sim.live("k"),
        Some((2, 9, BASE + 90)),
        "100 was never merged"
    );
    assert_eq!(sim.pending("k"), None);
    assert_eq!(sim.deleted("k"), Some((1, 9)));
}

/// S15: a newer fact arrives while a page is in flight.
#[test]
fn an_absent_page_older_than_pending_evidence_keeps_the_entry() {
    // Guards above both revisions: the newer fact is deferred too.
    let mut sim = Sim::with_guards(11, 0);
    sim.apply(t(1, BASE + 100, 5));
    let request = sim.registry.next_admission_page().expect("pending");
    sim.apply(i(2, BASE + 90, 10));
    assert_eq!(
        sim.pending("k"),
        Some((2, BASE + 90, 10)),
        "replaced, not merged"
    );
    // The page (absent at 9) is older than the pending evidence.
    assert_eq!(sim.page(request, Vec::new(), 9), Vec::new());
    assert_eq!(
        sim.pending("k"),
        Some((2, BASE + 90, 10)),
        "kept for the next page"
    );
    assert_eq!(
        sim.deleted("k"),
        None,
        "the page cannot say which incarnation it missed"
    );
    assert_eq!(
        sim.admission_page(k_present(2, BASE + 90), 12),
        vec![activity(BASE + 90, false)]
    );
    assert_eq!(sim.live("k"), Some((2, 12, BASE + 90)));

    // Guards below the newer fact: it admits directly, and the absent page
    // is older than the live entry.
    let mut sim = Sim::with_guards(6, 0);
    sim.apply(t(1, BASE + 100, 5));
    let request = sim.registry.next_admission_page().expect("pending");
    assert_eq!(sim.apply(i_new(2, BASE + 90, 10)), vec![started(BASE + 90)]);
    assert_eq!(sim.page(request, Vec::new(), 9), Vec::new());
    assert_eq!(sim.live("k"), Some((2, 10, BASE + 90)));
}

/// S15': the page names a lower incarnation than the pending evidence.
#[test]
fn a_page_row_with_a_lower_incarnation_than_pending_is_stale() {
    let mut sim = Sim::with_guards(11, 0);
    sim.apply(t(1, BASE + 100, 5));
    let request = sim.registry.next_admission_page().expect("pending");
    sim.apply(i(2, BASE + 90, 10));
    assert_eq!(sim.page(request, k_present(1, BASE + 100), 9), Vec::new());
    assert_eq!(sim.live("k"), None);
    assert_eq!(sim.pending("k"), Some((2, BASE + 90, 10)));
    assert_eq!(
        sim.deleted("k"),
        Some((1, 10)),
        "incarnation 2 at 10 proves 1 dead"
    );
}

/// S16: a remembered deletion of an older incarnation does not bypass the
/// guards for a newer one.
#[test]
fn a_remembered_deletion_does_not_bypass_the_guards() {
    let mut sim = Sim::new();
    sim.apply(x(1, 6));
    sim.apply(b(9));
    assert_eq!(sim.deleted("k"), Some((1, 6)));
    assert_eq!(sim.registry.broad_through, Revision(9));
    assert_eq!(sim.apply(i_new(2, BASE, 7)), Vec::new(), "7 < 9: deferred");
    assert_eq!(sim.pending("k"), Some((2, BASE, 7)));
    sim.admission_page(Vec::new(), 10);
    assert_eq!(sim.live("k"), None);
    assert_eq!(sim.pending("k"), None);
    assert_eq!(sim.deleted("k"), Some((2, 10)));
}

/// I2: a fact whose revision is below either guard never admits alone.
/// Equality is safe.
#[test]
fn a_deletion_below_the_guards_is_never_admitted_from_a_fact_alone() {
    for (forgotten, broad_guard) in [(10, 0), (0, 10), (10, 10)] {
        let mut sim = Sim::with_guards(forgotten, broad_guard);
        for fact in [i_new(1, BASE, 9), t(1, BASE + 1, 9), i(3, BASE + 2, 4)] {
            assert_eq!(sim.apply(fact.clone()), Vec::new(), "{fact:?}");
            assert_eq!(sim.live("k"), None, "{fact:?}");
        }
        assert_eq!(sim.pending("k"), Some((3, BASE + 2, 4)));

        let mut sim = Sim::with_guards(forgotten, broad_guard);
        assert_eq!(
            sim.apply(i_new(1, BASE, 10)),
            vec![started(BASE)],
            "equality admits"
        );
    }
}

#[test]
fn a_presence_page_records_absence_so_a_second_delayed_fact_needs_no_page() {
    let mut sim = Sim::with_guards(8, 0);
    sim.apply(i(1, BASE, 5));
    sim.admission_page(Vec::new(), 9);
    assert_eq!(sim.deleted("k"), Some((1, 9)));
    assert_eq!(sim.apply(t(1, BASE + 3, 5)), Vec::new());
    assert_eq!(
        sim.pending("k"),
        None,
        "step 3 rejects it before the guards"
    );
    assert_eq!(sim.registry.next_page_request(BASE), None);
}

#[test]
fn a_higher_incarnation_fact_replaces_a_live_entry_with_idle_and_reconciled() {
    let mut sim = Sim::new();
    sim.apply(i(1, BASE, 3));
    assert_eq!(
        sim.apply(t(2, BASE + 4, 8)),
        vec![
            idle(BASE),
            removed_k(RemovalReason::Reconciled),
            activity(BASE + 4, false),
        ]
    );
    assert_eq!(sim.live("k"), Some((2, 8, BASE + 4)));
    assert_eq!(sim.deleted("k"), Some((1, 8)));
}

#[test]
fn an_older_incarnation_touch_never_advances_a_live_entry() {
    let mut sim = Sim::new();
    sim.apply(i(3, BASE, 9));
    for inc in [0, 1, 2] {
        assert_eq!(sim.apply(t(inc, BASE + 150, 9)), Vec::new());
        assert_eq!(sim.apply(index("k", inc, BASE + 150, true, 9)), Vec::new());
    }
    assert_eq!(sim.live("k"), Some((3, 9, BASE)));
}

/// G5: a deleted row rediscovered by a pass is a new incarnation and starts.
#[test]
fn a_re_indexed_deleted_row_starts_again_with_a_higher_incarnation() {
    let mut sim = Sim::new();
    assert_eq!(sim.apply(i_new(1, BASE, 3)), vec![started(BASE)]);
    assert_eq!(
        sim.apply(x(1, 4)),
        vec![idle(BASE), removed_k(RemovalReason::Deleted)]
    );
    // The same epoch, the same key: without incarnations this was refused.
    assert_eq!(sim.apply(i_new(2, BASE, 5)), vec![started(BASE)]);
    assert_eq!(sim.live("k"), Some((2, 5, BASE)));
}

#[test]
fn a_removal_of_a_live_entry_publishes_idle_then_removed() {
    let mut sim = Sim::new();
    sim.apply(i(1, BASE, 3));
    assert_eq!(
        sim.apply(removed("k", 1, RemovalReason::Purged, 4)),
        vec![idle(BASE), removed_k(RemovalReason::Purged)]
    );
    // A removal of a pending entry publishes only Removed.
    let mut sim = Sim::with_guards(8, 0);
    sim.apply(i(1, BASE, 3));
    assert_eq!(
        sim.apply(removed("k", 1, RemovalReason::Purged, 4)),
        vec![removed_k(RemovalReason::Purged)]
    );
    assert_eq!(sim.pending("k"), None);
}

#[test]
fn out_of_window_pending_entries_are_pruned_before_a_page() {
    let mut sim = Sim::with_guards(8, 0);
    sim.apply(index_fact("fresh", 1, BASE, 5));
    sim.apply(index_fact(
        "aging",
        1,
        BASE - ACTIVE_SESSION_WINDOW_SECS + 2,
        5,
    ));
    assert_eq!(sim.registry.pending.len(), 2);
    sim.now = BASE + 2;
    assert_eq!(
        sim.registry.next_page_request(sim.now),
        Some(PageRequest::Admission {
            keys: vec![key("fresh")],
        })
    );
    assert!(!sim.registry.pending.contains_key(&key("aging")));
}

#[test]
fn a_broad_removal_keeps_unrelated_sessions() {
    let mut sim = Sim::new();
    sim.apply(index_fact("purged", 1, BASE, 5));
    sim.apply(index_fact("unrelated", 1, BASE, 5));
    sim.apply(index_fact("newer", 1, BASE, 12));
    sim.apply(b(10));
    let request = sim.registry.next_page_request(BASE).expect("a walk");
    assert_eq!(
        request.keys(),
        &[key("purged"), key("unrelated")],
        "entries at or above the purge revision are not walked"
    );
    let events = sim.page(request, vec![presence("unrelated", 1, BASE)], 11);
    assert_eq!(
        events,
        vec![
            SessionEvent::Idle {
                session: session_ref("purged"),
                agent: AgentKind::Claude,
                at: BASE,
            },
            SessionEvent::Removed {
                session: Some(session_ref("purged")),
                reason: RemovalReason::Purged,
            },
        ]
    );
    assert_eq!(
        sim.registry.live.keys().cloned().collect::<Vec<_>>(),
        vec![key("newer"), key("unrelated")]
    );
    assert_eq!(sim.registry.live[&key("unrelated")].exists_at, Revision(11));
}

#[test]
fn evicting_a_deletion_raises_forgotten_through_and_guards_later_facts() {
    let mut sim = Sim::new();
    for index in 0..DELETION_MEMORY_CAP {
        sim.apply(removed(
            &format!("d{index:04}"),
            1,
            RemovalReason::Deleted,
            100 + index as u64,
        ));
    }
    assert_eq!(sim.registry.deleted.len(), DELETION_MEMORY_CAP);
    assert_eq!(sim.registry.forgotten_through, Revision(0));
    // One more: the lowest `absent_at` leaves and becomes the guard.
    sim.apply(removed("late", 1, RemovalReason::Deleted, 5_000));
    assert_eq!(sim.registry.deleted.len(), DELETION_MEMORY_CAP);
    assert!(!sim.registry.deleted.contains_key(&key("d0000")));
    assert_eq!(sim.registry.forgotten_through, Revision(100));
    assert_eq!(sim.registry.deleted_by_revision.len(), DELETION_MEMORY_CAP);
    // A delayed fact for the forgotten key at its old revision is deferred,
    // not admitted, though nothing remembers the key.
    assert_eq!(sim.apply(index_fact("d0000", 1, BASE, 99)), Vec::new());
    assert!(sim.registry.pending.contains_key(&key("d0000")));
    // A fact at the guard is safe.
    assert_eq!(sim.apply(index_fact("other", 1, BASE, 100)).len(), 1);
}

fn index_fact(session_id: &str, inc: u64, at: i64, revision: u64) -> Observation {
    index(session_id, inc, at, false, revision)
}

/// A small deterministic generator for the property tests.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for index in (1..items.len()).rev() {
            let other = self.below(index as u64 + 1) as usize;
            items.swap(index, other);
        }
    }
}

/// One generated store history: the facts it produced, the rows it ended
/// with, its final revision, and the touch times per incarnation.
struct History {
    facts: Vec<Observation>,
    store: BTreeMap<SessionKey, (u64, i64)>,
    revision: u64,
    touches: BTreeMap<(SessionKey, u64), i64>,
}

fn generate(rng: &mut Rng) -> History {
    let keys = [key("a"), key("b")];
    let mut next_incarnation = 0;
    let mut revision = 10;
    let mut store: BTreeMap<SessionKey, (u64, i64)> = BTreeMap::new();
    let mut facts = Vec::new();
    let mut touches: BTreeMap<(SessionKey, u64), i64> = BTreeMap::new();
    let steps = 4 + rng.below(8);
    for _ in 0..steps {
        let target = keys[rng.below(2) as usize].clone();
        let agent = AgentKind::Claude;
        match rng.below(5) {
            0 | 1 => {
                revision += 1;
                match store.get(&target).copied() {
                    Some((inc, epoch)) => {
                        let at = epoch.max(BASE + rng.below(60) as i64);
                        store.insert(target.clone(), (inc, at));
                        facts.push(Observation::Indexed {
                            sessions: vec![IndexedSession {
                                key: target,
                                agent,
                                incarnation: Incarnation(inc),
                                at,
                                is_new: false,
                            }],
                            revision: Revision(revision),
                        });
                    }
                    None => {
                        next_incarnation += 1;
                        let at = BASE + rng.below(60) as i64;
                        store.insert(target.clone(), (next_incarnation, at));
                        facts.push(Observation::Indexed {
                            sessions: vec![IndexedSession {
                                key: target,
                                agent,
                                incarnation: Incarnation(next_incarnation),
                                at,
                                is_new: true,
                            }],
                            revision: Revision(revision),
                        });
                    }
                }
            }
            2 => {
                if let Some((inc, _)) = store.get(&target).copied() {
                    let at = BASE + rng.below(120) as i64;
                    let slot = touches.entry((target.clone(), inc)).or_insert(i64::MIN);
                    *slot = (*slot).max(at);
                    facts.push(Observation::Touched {
                        session: TouchedSession {
                            key: target,
                            incarnation: Incarnation(inc),
                            seen: Revision(revision),
                        },
                        agent,
                        at,
                    });
                }
            }
            3 => {
                if let Some((inc, _)) = store.remove(&target) {
                    revision += 1;
                    facts.push(Observation::Removed {
                        scope: RemovalScope::One(target, Incarnation(inc)),
                        reason: RemovalReason::Deleted,
                        revision: Revision(revision),
                    });
                }
            }
            _ => {
                if !store.is_empty() {
                    revision += 1;
                    let victims = store
                        .keys()
                        .filter(|_| rng.below(2) == 0)
                        .cloned()
                        .collect::<Vec<_>>();
                    for victim in victims {
                        store.remove(&victim);
                    }
                    facts.push(broad(RemovalReason::Purged, revision));
                }
            }
        }
    }
    History {
        facts,
        store,
        revision,
        touches,
    }
}

/// I4 as stated after the addendum: for a fixed multiset of facts, every
/// receipt order (and spill folding of its sync part) admits no
/// cross-incarnation activity, keeps every valid transient touch, admits
/// only through the guards, and converges to the store once pages settle.
#[test]
fn the_evidence_is_independent_of_receipt_order() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..300 {
        let history = generate(&mut rng);
        if history.facts.is_empty() {
            continue;
        }
        // A guard somewhere inside the history's revision range, the same in
        // every order, so some facts defer and some admit directly.
        let forgotten = 10 + rng.below(history.revision - 9);
        let mut ends = Vec::new();
        for _ in 0..6 {
            let mut order = history.facts.clone();
            rng.shuffle(&mut order);
            let mut sim = Sim::with_guards(forgotten, 0);
            for fact in order.clone() {
                sim.apply(fact);
                // A live entry never sits below a remembered deletion of
                // its key, and never carries another incarnation's time.
                for (live_key, entry) in &sim.registry.live {
                    if let Some(deletion) = sim.registry.deleted.get(live_key) {
                        assert!(entry.incarnation > deletion.incarnation, "{order:?}");
                    }
                    let own_time = history
                        .facts
                        .iter()
                        .filter_map(|fact| activity_time_for(fact, live_key, entry.incarnation))
                        .any(|at| at == entry.last_activity_at);
                    assert!(
                        own_time,
                        "{order:?}: {live_key:?} carries a time no fact of its incarnation has"
                    );
                }
            }
            sim.reconcile_with(&history.store, history.revision + 1);
            assert!(sim.registry.pending.is_empty(), "{order:?}");
            for target in [key("a"), key("b")] {
                match history.store.get(&target) {
                    Some((inc, epoch)) => {
                        let entry = sim
                            .registry
                            .live
                            .get(&target)
                            .unwrap_or_else(|| panic!("{order:?}: {target:?} should be live"));
                        assert_eq!(entry.incarnation, Incarnation(*inc), "{order:?}");
                        let expected_at = history
                            .touches
                            .get(&(target.clone(), *inc))
                            .copied()
                            .map_or(*epoch, |touch| touch.max(*epoch));
                        assert_eq!(
                            entry.last_activity_at, expected_at,
                            "{order:?}: valid transient activity is kept, older incarnations' is not"
                        );
                    }
                    None => {
                        assert!(
                            !sim.registry.live.contains_key(&target),
                            "{order:?}: {target:?} is gone from the store"
                        );
                    }
                }
            }
            ends.push(
                sim.registry
                    .live
                    .iter()
                    .map(|(key, entry)| (key.clone(), entry.incarnation, entry.last_activity_at))
                    .collect::<Vec<_>>(),
            );
        }
        assert!(
            ends.windows(2).all(|pair| pair[0] == pair[1]),
            "every order converges to the same live view"
        );
    }
}

/// The activity time a fact carries for `(key, incarnation)`, if any.
fn activity_time_for(
    fact: &Observation,
    target: &SessionKey,
    incarnation: Incarnation,
) -> Option<i64> {
    match fact {
        Observation::Touched {
            session: touched,
            at,
            ..
        } if touched.key == *target && touched.incarnation == incarnation => Some(*at),
        Observation::Indexed { sessions, .. } => sessions
            .iter()
            .find(|session| session.key == *target && session.incarnation == incarnation)
            .map(|session| session.at),
        _ => None,
    }
}

/// Spill folding of a random sync sequence for one key equals sequential
/// application: same live entry, same deletion memory, same broad guard,
/// same walk requirement, and the same set of removal events.
#[test]
fn spill_folding_matches_sequential_application() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for _ in 0..300 {
        let start_inc = 1 + rng.below(3);
        let mut sequential = Sim::new();
        let mut folded = Sim::new();
        for sim in [&mut sequential, &mut folded] {
            sim.apply(i(start_inc, BASE, 5));
        }
        let mut spill = Spill::default();
        let mut sequential_events = Vec::new();
        let count = 1 + rng.below(8);
        for _ in 0..count {
            let observation = match rng.below(4) {
                0 => sync_removed(
                    "k",
                    rng.below(5),
                    if rng.below(2) == 0 {
                        RemovalReason::Deleted
                    } else {
                        RemovalReason::Rejected
                    },
                    6 + rng.below(10),
                ),
                1 => SyncObservation::Removed {
                    scope: RemovalScope::Broad,
                    reason: RemovalReason::Purged,
                    revision: Revision(6 + rng.below(10)),
                },
                2 => SyncObservation::RowChanged {
                    session: key("k"),
                    facets: UpdateFacets {
                        analysis: rng.below(2) == 0,
                        title: rng.below(2) == 0,
                        ..UpdateFacets::default()
                    },
                    at: BASE + rng.below(9) as i64,
                },
                _ => SyncObservation::IndexChanged {
                    reason: if rng.below(2) == 0 {
                        IndexChangeReason::ScanPass
                    } else {
                        IndexChangeReason::Invalidated
                    },
                },
            };
            sequential_events.extend(sequential.apply(Observation::from(observation.clone())));
            spill.fold(observation);
        }
        let mut folded_events = Vec::new();
        for (session, (facets, at)) in spill.rows {
            folded_events.push(SessionEvent::Updated {
                session: SessionRef::from(&session),
                facets,
                at,
            });
        }
        for (removed_key, cell) in spill.removed {
            folded.registry.remove(
                &removed_key,
                cell.incarnation,
                cell.reason,
                cell.revision,
                BASE,
                &mut folded_events,
            );
        }
        if let Some((reason, revision)) = spill.broad {
            folded.registry.broad(reason, revision, &mut folded_events);
        }
        for reason in spill.index_changed {
            folded_events.push(SessionEvent::IndexChanged { reason });
        }

        assert_eq!(folded.live("k"), sequential.live("k"));
        assert_eq!(folded.deleted("k"), sequential.deleted("k"));
        assert_eq!(
            folded.registry.broad_through,
            sequential.registry.broad_through
        );
        assert_eq!(
            folded
                .registry
                .reconcile
                .needs
                .map(|(_, revision)| revision),
            sequential
                .registry
                .reconcile
                .needs
                .map(|(_, revision)| revision)
        );
        // The row cell carries the union of facets and the latest epoch.
        let merged_facets = sequential_events.iter().fold(
            (UpdateFacets::default(), i64::MIN),
            |(mut facets, latest), event| match event {
                SessionEvent::Updated {
                    facets: more, at, ..
                } => {
                    facets.merge(*more);
                    (facets, latest.max(*at))
                }
                _ => (facets, latest),
            },
        );
        let folded_row = folded_events.iter().find_map(|event| match event {
            SessionEvent::Updated { facets, at, .. } => Some((*facets, *at)),
            _ => None,
        });
        assert_eq!(
            folded_row,
            (merged_facets.1 != i64::MIN).then_some(merged_facets)
        );
        // Whether the live entry left is the same; the spill may compress
        // the Idle/Removed narration into one pair.
        let left = |events: &[SessionEvent]| {
            events
                .iter()
                .any(|event| matches!(event, SessionEvent::Idle { .. }))
        };
        assert_eq!(left(&folded_events), left(&sequential_events));
        // Every index reason the sequence named is named once by the spill.
        let reasons = |events: &[SessionEvent]| {
            events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::IndexChanged { reason } => Some(*reason),
                    _ => None,
                })
                .collect::<BTreeSet<_>>()
        };
        assert_eq!(reasons(&folded_events), reasons(&sequential_events));
    }
}
