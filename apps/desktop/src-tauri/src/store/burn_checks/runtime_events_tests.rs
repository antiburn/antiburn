use super::*;
use crate::published_evidence_reader::StorePublishedEvidenceReader;
use crate::store::SessionKey;
use crate::store::native_tool_results::publications::PublicationPolicy;
use crate::store::native_tool_results::{evidence, observers, request_origin};
use antiburn_checks::source::PublishedEvidenceSource;
use antiburn_local::analysis::file_live::{NativeFileReadError, read_native_file_page};
use antiburn_local::analysis::*;

fn native_session(store: &Store) -> (tempfile::TempDir, SessionKey) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.jsonl");
    std::fs::write(&path, format!("{}\n", json!({"type":"user", "uuid":"root", "parentUuid":null,
        "sessionId":"session", "cwd":"/repo", "isSidechain":false, "version":"2.1.278", "entrypoint":"cli",
        "message":{"role":"user", "content":"Baseline request."}}))).unwrap();
    let page = read_native_file_page(
        &SessionInput {
            agent: "claude".into(),
            session_id: "session".into(),
            source: RawSource::File(path.clone()),
            source_format: SourceFormat::ClaudeJsonl,
            fork_parent_session_id: None,
        },
        None,
        &|| false,
    )
    .unwrap();
    let key = SessionKey::new("native", "claude-code", "session");
    let mut connection = store.lock();
    let tx = connection.transaction().unwrap();
    tx.execute(
        "INSERT INTO session(environment_key, agent, session_id, source_kind, source_label,
        first_seen_at, last_seen_at, cwd) VALUES ('native', 'claude-code', 'session', 'file', ?1,
        'now', 'now', '/repo')",
        [path.to_string_lossy().as_ref()],
    )
    .unwrap();
    tx.execute("INSERT INTO native_file_delivery(environment_key, agent, session_id, incarnation,
        source_label, cwd, parser_revision, cursor_json, pending, active_until_epoch)
        SELECT environment_key, agent, session_id, incarnation, source_label, cwd, ?1, ?2, 0, 0 FROM session",
        params![PARSER_REVISION, serde_json::to_string(&page.cursor).unwrap()]).unwrap();
    assert!(request_origin::ensure_boundary_in(&tx, &key, 1, 1).unwrap());
    tx.commit().unwrap();
    (directory, key)
}

fn native_user() -> NativeEvidenceEvent {
    NativeEvidenceEvent {
        live_eligible: true,
        fact: NativeEvidenceFact::Message(NativeMessageObservation::User(UserMessageObservation {
            revision: 1,
            message: UserMessage {
                source_format: SourceFormat::ClaudeJsonl,
                session_id: "session".into(),
                message_id: "user-one".into(),
                position: UserMessagePosition::Jsonl { record_index: 1 },
                origin: UserMessageOrigin::ClaudeUserContent,
                scope: UserMessageScope::MainSession,
                change: UserMessageChange::Present,
                completeness: UserMessageCompleteness::Complete,
                parts: vec![UserMessagePart {
                    item_index: 0,
                    native_part_id: None,
                    text: "Public user-only work.".into(),
                }],
                proof_revision: 1,
            },
            context: NativeEventContext {
                first_eligible_position: ToolResultPosition::Jsonl {
                    record_index: 1,
                    item_index: 0,
                },
                main_session: true,
                complete_through_event: true,
                user_messages: Vec::new(),
            },
        })),
    }
}

fn source_page(
    store: &Store,
    activation: &EffectiveActivation,
    after: PublicationCursor,
) -> SourceBatch {
    PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_project(store, "native", "/repo")
            .with_activation(&activation.id),
        40,
    )
    .unwrap()
    .read_batch(after)
    .unwrap()
}

fn attach(store: &Store, key: &SessionKey, activation: &EffectiveActivation) {
    let mut connection = store.lock();
    let tx = connection.transaction().unwrap();
    let incarnation: i64 = tx
        .query_row("SELECT incarnation FROM session", [], |r| r.get(0))
        .unwrap();
    tx.execute("INSERT INTO burn_check_session_attachment(check_id, activation_scope_key, generation, branch_key, incarnation)
        VALUES (?1, ?2, 1, ?3, ?4)", params![activation.id.definition_id,
            StoreCheckStore::activation_scope_key(&activation.id.scope), key.to_identity_json(), incarnation]).unwrap();
    observers::attach_in(&tx, &activation.id, key, incarnation, "receipt", 1000).unwrap();
    tx.commit().unwrap();
}

fn local_assessment(
    adapter: &StoreCheckStore,
    activation: &EffectiveActivation,
    candidate: &FrozenCandidate,
    now_ms: u64,
) -> AssessmentAcceptance {
    let CandidateClaim::Claimed(claimed) = adapter.claim_due(candidate, now_ms, true).unwrap()
    else {
        panic!("local work must be claimable");
    };
    assert_eq!(
        adapter
            .record_local_start(
                &claimed,
                now_ms,
                Duration::ZERO,
                &LocalPreparation {
                    version: 1,
                    started_at_unix_ms: now_ms,
                    evaluation_budget: EvaluationBudget::default(),
                }
            )
            .unwrap(),
        ClaimOutcome::Current
    );
    let evaluated = EvaluatedAssessment::local();
    let actions = plan_status_actions(
        &activation.compiled.actions,
        &candidate.context.value,
        &evaluated,
        claimed.prior_status,
    )
    .unwrap();
    AssessmentAcceptance {
        claimed: *claimed,
        evaluated,
        actions,
    }
}

fn fail_native_read(store: &Store, key: &SessionKey, failure: NativeFileReadError, now: i64) {
    let source_state = || {
        store.lock().query_row("SELECT json_array(cursor_json,
        (SELECT value FROM setting WHERE key LIKE 'internal:claudeRequestOriginParser65:%'),
        (SELECT json_group_array(json_array(publication_sequence, payload_json, dependency_binding_json))
           FROM native_tool_result_publication)) FROM native_file_delivery", [], |row| row.get::<_, String>(0)).unwrap()
    };
    let before = source_state();
    let claim = store.claim_next_native_file(now, 60).unwrap().unwrap();
    assert_eq!(&claim.key, key);
    assert!(
        store
            .fail_native_file(&claim, failure, now, now + 1)
            .unwrap()
    );
    assert_eq!(source_state(), before);
    let ready = request_origin::source_ready_sql("s");
    assert!(
        !store
            .lock()
            .query_row(&format!("SELECT {ready} FROM session s"), [], |row| row
                .get::<_, bool>(0))
            .unwrap()
    );
    assert!(
        store
            .lock()
            .query_row(
                "SELECT continuity_eligible = 0 FROM burn_check_session_attachment",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
}

#[test]
fn attached_start_accepts_local_work_after_a_real_native_read_outage() {
    for failure in [
        NativeFileReadError::ReadFailed,
        NativeFileReadError::Unavailable,
    ] {
        let (store, adapter, activation) = fixture_with_definition(
            Arc::new(Store::open_in_memory(Path::new("/synthetic/start-read-outage")).unwrap()),
            "name: Start\non:\n  session_start:\nactions:\n  - type: status\n    input: {result: failure}\n",
        );
        let (_source, key) = native_session(&store);
        attach(&store, &key, &activation);
        let candidate = apply(
            &adapter,
            &activation,
            &source_page(&store, &activation, PublicationCursor(0)),
        );
        let assessment = local_assessment(&adapter, &activation, &candidate, 2000);
        fail_native_read(&store, &key, failure, 3);
        let id = hex(&assessment.claimed.id.0);
        assert!(
            request_origin::new_candidate_request_origin_current_in(&store.lock(), &id).unwrap()
        );
        assert!(observers::typed_dependencies_current_in(&store.lock(), &id).unwrap());
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Accepted
        );
    }
}

#[test]
fn idle_local_work_stays_stale_after_a_real_native_read_outage() {
    for failure in [
        NativeFileReadError::ReadFailed,
        NativeFileReadError::Unavailable,
    ] {
        let (store, adapter, activation) = fixture_with_definition(
            Arc::new(Store::open_in_memory(Path::new("/synthetic/idle-read-outage")).unwrap()),
            "name: Idle\non:\n  session_idle:\nactions:\n  - type: status\n    input: {result: failure}\n",
        );
        let (_source, key) = native_session(&store);
        attach(&store, &key, &activation);
        {
            let mut connection = store.lock();
            let tx = connection.transaction().unwrap();
            observers::frontier_in(&tx, &key, "receipt", true, 121_000).unwrap();
            tx.commit().unwrap();
        }
        let candidate = apply(
            &adapter,
            &activation,
            &source_page(&store, &activation, PublicationCursor(0)),
        );
        let assessment = local_assessment(&adapter, &activation, &candidate, 121_000);
        fail_native_read(&store, &key, failure, 122);
        let id = hex(&assessment.claimed.id.0);
        assert!(
            request_origin::new_candidate_request_origin_current_in(&store.lock(), &id).unwrap()
        );
        assert!(!observers::typed_dependencies_current_in(&store.lock(), &id).unwrap());
        assert_eq!(
            adapter.accept(&assessment).unwrap(),
            AcceptanceOutcome::Stale
        );
    }
}

#[test]
fn attached_start_fences_claim_local_start_and_acceptance_to_its_observer() {
    for stage in ["claim", "local_start", "acceptance"] {
        for mutation in [
            "UPDATE session SET source_label = '/synthetic/replaced.jsonl'",
            "UPDATE session SET cwd = '/other'",
            "UPDATE session SET incarnation = incarnation + 1",
            "UPDATE burn_check_session_attachment SET observer_epoch = 'replaced'",
            "UPDATE burn_check_session_attachment SET generation = generation + 1",
            "DELETE FROM burn_check_session_attachment",
        ] {
            let (store, adapter, activation) = fixture_with_definition(
                Arc::new(Store::open_in_memory(Path::new("/synthetic/start-fence")).unwrap()),
                "name: Start\non:\n  session_start:\nactions:\n  - type: status\n    input: {result: failure}\n",
            );
            let (_source, key) = native_session(&store);
            attach(&store, &key, &activation);
            let candidate = apply(
                &adapter,
                &activation,
                &source_page(&store, &activation, PublicationCursor(0)),
            );
            assert_eq!(store.lock().query_row(
                "SELECT COUNT(*) FROM burn_check_candidate_dependency WHERE native_revision IS NULL
                   AND json_type(dependency_binding_json, '$.HostObservation') = 'object'",
                [], |row| row.get::<_, i64>(0),
            ).unwrap(), 1);
            if stage == "claim" {
                store.lock().execute(mutation, []).unwrap();
                assert!(
                    matches!(
                        adapter.claim_due(&candidate, 2000, false).unwrap(),
                        CandidateClaim::Stale
                    ),
                    "{stage}: {mutation}"
                );
                continue;
            }
            let claimed = claim(&adapter, &candidate);
            let preparation = LocalPreparation {
                version: 1,
                started_at_unix_ms: 2000,
                evaluation_budget: EvaluationBudget::default(),
            };
            if stage == "local_start" {
                store.lock().execute(mutation, []).unwrap();
                assert_eq!(
                    adapter
                        .record_local_start(&claimed, 2000, Duration::ZERO, &preparation)
                        .unwrap(),
                    ClaimOutcome::Stale,
                    "{stage}: {mutation}"
                );
                continue;
            }
            assert_eq!(
                adapter
                    .record_local_start(&claimed, 2000, Duration::ZERO, &preparation)
                    .unwrap(),
                ClaimOutcome::Current
            );
            let evaluated = EvaluatedAssessment::local();
            let actions = plan_status_actions(
                &activation.compiled.actions,
                &candidate.context.value,
                &evaluated,
                claimed.prior_status,
            )
            .unwrap();
            store.lock().execute(mutation, []).unwrap();
            assert_eq!(
                adapter
                    .accept(&AssessmentAcceptance {
                        claimed,
                        evaluated,
                        actions
                    })
                    .unwrap(),
                AcceptanceOutcome::Stale,
                "{stage}: {mutation}"
            );
            assert_eq!(
                store
                    .lock()
                    .query_row(
                        "SELECT COUNT(*) FROM burn_check_current_status",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                0
            );
        }
    }
}

#[test]
fn attached_start_survives_later_activity_and_continuity_loss() {
    let (store, adapter, activation) = fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/start-boundary")).unwrap()),
        "name: Start\non:\n  session_start:\nactions:\n  - type: status\n    input: {result: failure}\n",
    );
    let (_source, key) = native_session(&store);
    attach(&store, &key, &activation);
    let engine = CheckEngine::new(adapter, Arc::new(NoEvaluator));
    engine
        .apply_source_page(
            &activation,
            &source_page(&store, &activation, PublicationCursor(0)),
        )
        .unwrap();
    {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        observers::activity_in(
            &tx,
            &key,
            &NativeActivityObservation {
                source_format: SourceFormat::ClaudeJsonl,
                session_id: key.session_id.clone(),
                position: ToolResultPosition::Jsonl {
                    record_index: 1,
                    item_index: 0,
                },
                token: "later-work".into(),
                native_at_ms: None,
            },
            2000,
        )
        .unwrap();
        observers::break_continuity_in(&tx, &key).unwrap();
        tx.commit().unwrap();
    }
    let StartOutcome::Started(task) = engine.start_one_due(&activation, 3000).unwrap() else {
        panic!("the attachment boundary remains valid")
    };
    assert_eq!(poll_settlement(task).outcome, RunOutcome::Accepted);
}

#[test]
fn user_only_publication_reaches_selection_and_local_acceptance() {
    let (store, adapter, activation) = fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/public-user")).unwrap()),
        "name: User\non:\n  user_message:\nactions:\n  - type: status\n    input: {result: failure}\n",
    );
    let (_source, key) = native_session(&store);
    {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        evidence::reconcile_evidence_in(
            &tx,
            &key,
            1,
            1,
            &NativeEvidenceBatch {
                source_format: SourceFormat::ClaudeJsonl,
                events: vec![native_user()],
                history_complete: true,
            },
            PublicationPolicy {
                suppress_coverage: true,
                ..Default::default()
            },
            2000,
        )
        .unwrap();
        tx.commit().unwrap();
    }
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    let page = source_page(&store, &activation, PublicationCursor(0));
    assert_eq!(
        engine
            .apply_source_page(&activation, &page)
            .unwrap()
            .candidates,
        1
    );
    let StartOutcome::Started(task) = engine.start_one_due(&activation, 2000).unwrap() else {
        panic!("user candidate did not start")
    };
    assert_eq!(poll_settlement(task).outcome, RunOutcome::Accepted);
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn idle_publication_reaches_acceptance_and_later_activity_keeps_accepted_history() {
    let (store, adapter, activation) = fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/public-idle")).unwrap()),
        "name: Idle\non:\n  session_idle:\nactions:\n  - type: status\n    input: {result: failure}\n",
    );
    let (_source, key) = native_session(&store);
    attach(&store, &key, &activation);
    {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        observers::frontier_in(&tx, &key, "receipt", true, 121_000).unwrap();
        tx.commit().unwrap();
    }
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    let page = source_page(&store, &activation, PublicationCursor(0));
    assert_eq!(
        engine
            .apply_source_page(&activation, &page)
            .unwrap()
            .candidates,
        1
    );
    let StartOutcome::Started(task) = engine.start_one_due(&activation, 121_000).unwrap() else {
        panic!("idle candidate did not start")
    };
    assert_eq!(poll_settlement(task).outcome, RunOutcome::Accepted);
    {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        observers::activity_in(
            &tx,
            &key,
            &NativeActivityObservation {
                source_format: SourceFormat::ClaudeJsonl,
                session_id: "session".into(),
                position: ToolResultPosition::Jsonl {
                    record_index: 1,
                    item_index: 0,
                },
                token: "later-work".into(),
                native_at_ms: None,
            },
            122_000,
        )
        .unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(
        store
            .lock()
            .query_row("SELECT state FROM burn_check_candidate", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "accepted"
    );
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT evidence_valid FROM burn_check_assessment_history",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn activity_cancels_idle_local_work_before_local_start() {
    let (store, adapter, activation) = fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/public-idle-cancel")).unwrap()),
        "name: Idle\non:\n  session_idle:\nactions:\n  - type: status\n    input: {result: failure}\n",
    );
    let (_source, key) = native_session(&store);
    attach(&store, &key, &activation);
    {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        observers::frontier_in(&tx, &key, "receipt", true, 121_000).unwrap();
        tx.commit().unwrap();
    }
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    engine
        .apply_source_page(
            &activation,
            &source_page(&store, &activation, PublicationCursor(0)),
        )
        .unwrap();
    let pending: String = store
        .lock()
        .query_row(
            "SELECT frozen_input_json FROM burn_check_candidate",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let pending: FrozenCandidate = serde_json::from_str(&pending).unwrap();
    let CandidateClaim::Claimed(claim) = adapter.claim_due(&pending, 121_000, false).unwrap()
    else {
        panic!("idle claim failed")
    };
    {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        observers::activity_in(
            &tx,
            &key,
            &NativeActivityObservation {
                source_format: SourceFormat::ClaudeJsonl,
                session_id: "session".into(),
                position: ToolResultPosition::Jsonl {
                    record_index: 1,
                    item_index: 0,
                },
                token: "new-work".into(),
                native_at_ms: None,
            },
            121_001,
        )
        .unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(
        adapter
            .record_local_start(
                &claim,
                121_002,
                Duration::ZERO,
                &LocalPreparation {
                    version: 1,
                    started_at_unix_ms: 121_002,
                    evaluation_budget: EvaluationBudget::default()
                }
            )
            .unwrap(),
        ClaimOutcome::Stale
    );
}

#[test]
fn cycle_members_bind_native_current_without_selected_member_observations() {
    let (store, adapter, activation) = fixture_with_definition(
        Arc::new(Store::open_in_memory(Path::new("/synthetic/public-cycle")).unwrap()),
        "name: Cycle\non:\n  turn_end:\nactions:\n  - type: status\n    input: {result: failure}\n",
    );
    let key = SessionKey::new("native", "codex", "session");
    store.lock().execute("INSERT INTO session(environment_key, agent, session_id, source_kind, source_label,
        first_seen_at, last_seen_at, cwd) VALUES ('native', 'codex', 'session', 'file', '/synthetic/session.jsonl', 'now', 'now', '/repo')", []).unwrap();
    let NativeEvidenceFact::Message(NativeMessageObservation::User(mut member)) =
        native_user().fact
    else {
        unreachable!()
    };
    member.message.source_format = SourceFormat::CodexRolloutJsonl;
    member.message.origin = UserMessageOrigin::CodexRetainedUserText;
    member.message.parts[0].text = "Retained public task input. ".repeat(32);
    let cycle = ResponseCycleObservation {
        source_format: SourceFormat::CodexRolloutJsonl,
        session_id: key.session_id.clone(),
        key: NativeCycleKey::CodexTurn {
            turn_id: "task-one".into(),
        },
        start_position: ToolResultPosition::Jsonl {
            record_index: 0,
            item_index: 0,
        },
        position: ToolResultPosition::Jsonl {
            record_index: 3,
            item_index: 0,
        },
        revision: 3,
        change: ToolResultChange::Update,
        status: antiburn_local::analysis::ResponseCycleStatus::Completed,
        native_reason: None,
        native_error_code: None,
        started_at_ms: Some(1000),
        ended_at_ms: Some(2000),
        requester: NativeRequestAttribution::default(),
        context: NativeEventContext {
            first_eligible_position: ToolResultPosition::Jsonl {
                record_index: 3,
                item_index: 0,
            },
            main_session: true,
            complete_through_event: true,
            user_messages: vec![member.message.clone()],
        },
        messages: vec![NativeMessageObservation::User(member.clone())],
    };
    let mut oversized = cycle.clone();
    oversized.key = NativeCycleKey::CodexTurn {
        turn_id: "oversized-task".into(),
    };
    let member_bytes = serde_json::to_vec(&cycle.messages[0]).unwrap().len();
    oversized.messages =
        vec![cycle.messages[0].clone(); (4 * 1024 * 1024 - 16 * 1024) / (member_bytes + 1)];
    assert!(oversized.messages.len() <= 4096);
    assert!(serde_json::to_vec(&oversized).unwrap().len() < 4 * 1024 * 1024);
    {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        evidence::reconcile_evidence_in(
            &tx,
            &key,
            1,
            1,
            &NativeEvidenceBatch {
                source_format: SourceFormat::CodexRolloutJsonl,
                events: vec![
                    NativeEvidenceEvent {
                        live_eligible: false,
                        fact: NativeEvidenceFact::Message(NativeMessageObservation::User(
                            member.clone(),
                        )),
                    },
                    NativeEvidenceEvent {
                        live_eligible: true,
                        fact: NativeEvidenceFact::ResponseCycle(oversized),
                    },
                    NativeEvidenceEvent {
                        live_eligible: true,
                        fact: NativeEvidenceFact::ResponseCycle(cycle),
                    },
                ],
                history_complete: true,
            },
            PublicationPolicy {
                suppress_coverage: true,
                ..Default::default()
            },
            2000,
        )
        .unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(store.lock().query_row(
        "SELECT COUNT(*) FROM native_tool_result_publication WHERE event_kind = 'response_cycle'", [],
        |row| row.get::<_, i64>(0),
    ).unwrap(), 1, "cycle payload and member bindings share one byte budget");
    let page = source_page(&store, &activation, PublicationCursor(0));
    let (member_cursor, member_version, normalized_version) = page
        .events
        .iter()
        .find_map(|event| {
            if let antiburn_checks::source::SourceEvent::Observation {
                cursor,
                source_evidence_version,
                observation,
                ..
            } = event
                && matches!(
                    observation.kind,
                    antiburn_checks::domain::ObservationKind::UserMessage(_)
                )
            {
                Some((
                    *cursor,
                    source_evidence_version.clone().unwrap(),
                    observation.version.clone(),
                ))
            } else {
                None
            }
        })
        .unwrap();
    assert_ne!(member_version, normalized_version);
    let binding = evidence::current_binding_in(
        &store.lock(),
        &key,
        &evidence::message_payload(&NativeMessageObservation::User(member.clone()))
            .identity()
            .unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(member_version, binding.version());
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    assert_eq!(
        engine
            .apply_source_page(&activation, &page)
            .unwrap()
            .candidates,
        1
    );
    assert_eq!(store.lock().query_row("SELECT COUNT(*) FROM burn_check_candidate_dependency WHERE dependency_binding_json IS NOT NULL", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        assert!(
            evidence::reconcile_evidence_in(
                &tx,
                &key,
                1,
                1,
                &NativeEvidenceBatch {
                    source_format: SourceFormat::CodexRolloutJsonl,
                    events: vec![NativeEvidenceEvent {
                        live_eligible: true,
                        fact: NativeEvidenceFact::Message(NativeMessageObservation::User(
                            member.clone()
                        ))
                    }],
                    history_complete: true,
                },
                PublicationPolicy {
                    suppress_coverage: true,
                    ..Default::default()
                },
                2500
            )
            .unwrap()
        );
        tx.commit().unwrap();
    }
    let unchanged = source_page(&store, &activation, page.next);
    assert!(unchanged.events.is_empty());
    assert_eq!(
        evidence::current_binding_in(&store.lock(), &key, &binding.identity)
            .unwrap()
            .unwrap(),
        binding
    );
    assert_eq!(
        store
            .lock()
            .query_row("SELECT state FROM burn_check_candidate", [], |row| row
                .get::<_, String>(
                0
            ))
            .unwrap(),
        "pending"
    );
    {
        member.revision = 4;
        member.message.parts[0].text = "Corrected task input.".into();
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        evidence::reconcile_evidence_in(
            &tx,
            &key,
            1,
            1,
            &NativeEvidenceBatch {
                source_format: SourceFormat::CodexRolloutJsonl,
                events: vec![NativeEvidenceEvent {
                    live_eligible: true,
                    fact: NativeEvidenceFact::Message(NativeMessageObservation::User(member)),
                }],
                history_complete: true,
            },
            PublicationPolicy {
                suppress_coverage: true,
                ..Default::default()
            },
            3000,
        )
        .unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(
        store
            .lock()
            .query_row("SELECT state FROM burn_check_candidate", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "stale"
    );
    let reread = source_page(&store, &activation, PublicationCursor(0));
    let original_version = reread
        .events
        .iter()
        .find_map(|event| match event {
            antiburn_checks::source::SourceEvent::Observation {
                cursor,
                source_evidence_version,
                observation,
                ..
            } if *cursor == member_cursor => {
                assert_eq!(observation.version, normalized_version);
                source_evidence_version.clone()
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(original_version, member_version);
    assert_ne!(
        evidence::current_binding_in(&store.lock(), &key, &binding.identity)
            .unwrap()
            .unwrap()
            .version(),
        member_version
    );
}
