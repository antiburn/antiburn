use super::{SourceLimit, source_limit};
use crate::jev::worker::JevCheckDescriptor;
use antiburn_local::analysis::SourceFormat;
use antiburn_local::analysis::jev::JevCheck;
use antiburn_local::checks::sampling::{Candidate, SamplingLimits, SamplingProgress, StableId};
use antiburn_local::checks::skill_opportunities::{SkillUseLifecycle, SkillUseStatus};

#[test]
fn native_skill_requests_and_document_selections_prepare_without_asserting_success() {
    use crate::scope_creep_worker::tests::native_sources;
    for (agent, session, format, records) in native_sources::sources() {
        let records = native_sources::records_with_work(agent, records);
        let records = records
            .lines()
            .enumerate()
            .map(|(index, line)| {
                let mut row: serde_json::Value = serde_json::from_str(line).unwrap();
                row["timestamp"] = serde_json::json!(format!("2099-01-01T00:00:{index:02}Z"));
                row.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let workspace = directory.path().join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        let (skill, relative) = match agent {
            "codex" => ("verify", ".codex/skills/verify/SKILL.md"),
            "claude-code" => ("api-review", ".claude/skills/api-review/SKILL.md"),
            "pi" => (
                "boundary-review",
                ".pi/agent/skills/boundary-review/SKILL.md",
            ),
            _ => unreachable!(),
        };
        let path = home.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!("---\nname: {skill}\ndescription: Review code and run tests.\n---\n"),
        )
        .unwrap();
        let unused_path = path
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("unused-review/SKILL.md");
        std::fs::create_dir_all(unused_path.parent().unwrap()).unwrap();
        std::fs::write(
            &unused_path,
            "---\nname: unused-review\ndescription: Review API boundaries and tests.\n---\n",
        )
        .unwrap();
        if agent == "codex" {
            std::fs::write(
                home.join(".codex/config.toml"),
                format!(
                    "[projects.{}]\ntrust_level = \"trusted\"\n",
                    serde_json::to_string(workspace.canonicalize().unwrap().to_str().unwrap())
                        .unwrap()
                ),
            )
            .unwrap();
        }
        let store = crate::store::Store::open(directory.path()).unwrap();
        store
            .capture_burn_check_boundaries(
                &[
                    super::SKILL_OPPORTUNITIES_CHECK_ID,
                    crate::scope_creep_worker::CHECK_ID,
                ],
                0,
            )
            .unwrap();
        let candidate =
            native_sources::publish(&store, agent, session, format, &records, &workspace);
        let config = crate::agent_config::ConfigContext::native(
            antiburn_local::model::AgentKind::from_slug(agent).unwrap(),
            &home,
            Some(workspace),
        );
        let load = || {
            let snapshot = store
                .load_smart_check_inputs(
                    &candidate.session.key,
                    candidate.published_fence,
                    candidate.source_generation,
                    crate::smart_check_inputs::DetectorInput::SkillOpportunities,
                )
                .unwrap();
            store
                .load_smart_check_skill_inputs(snapshot, &config)
                .unwrap()
        };
        let inputs = load();
        assert!(
            inputs
                .inventory()
                .skills()
                .iter()
                .any(|definition| definition.name == skill),
            "{agent}"
        );
        let events = inputs.usage().events();
        assert!(!events.is_empty(), "{agent}");
        assert!(
            events.iter().any(|event| matches!(
                event.lifecycle,
                SkillUseLifecycle::Requested | SkillUseLifecycle::DocumentSelected
            )),
            "{agent}: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| event.lifecycle == SkillUseLifecycle::Succeeded),
            "{agent}: {events:?}"
        );
        let first = super::prepare(&candidate, inputs, super::CHECK.evaluator_revision())
            .unwrap_or_else(|error| panic!("{agent}: {error:?}"));
        let plan = first.check.prepare(&first.check.session_context()).unwrap();
        assert!(
            plan.prepared
                .comparisons
                .iter()
                .any(|comparison| comparison.skill.name == "unused-review"),
            "{agent}"
        );
        assert!(
            plan.prepared
                .comparisons
                .iter()
                .all(|comparison| comparison.skill.name != skill),
            "{agent}"
        );
        let answers = plan
            .work_items
            .iter()
            .map(|item| antiburn_local::analysis::jev::JevWorkItemResult {
                request_id: item.id.clone(),
                work_item_id: item.id.clone(),
                model: plan.capabilities.model.clone(),
                answers: item
                    .questions
                    .keys()
                    .map(|question| {
                        let choice = if question == "sufficiency" {
                            "yes"
                        } else {
                            "no"
                        };
                        (
                            question.clone(),
                            antiburn_local::analysis::jev::JevAnswer::Choice {
                                choice: choice.into(),
                                confidence: 1.0,
                                probabilities: std::collections::BTreeMap::from([
                                    ("yes".into(), if choice == "yes" { 1.0 } else { 0.0 }),
                                    ("no".into(), if choice == "no" { 1.0 } else { 0.0 }),
                                    ("unknown".into(), 0.0),
                                ]),
                            },
                        )
                    })
                    .collect(),
                evidence: plan
                    .shared_context
                    .as_ref()
                    .unwrap()
                    .evidence
                    .iter()
                    .chain(&item.window.evidence)
                    .cloned()
                    .collect(),
                usage: antiburn_local::analysis::jev::JevUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            })
            .collect::<Vec<_>>();
        let result = first.check.reduce(&plan, &answers, true).unwrap();
        assert_eq!(
            result.decisions.len(),
            plan.prepared.comparisons.len(),
            "{agent}"
        );
        assert!(result.findings.is_empty(), "{agent}");
        assert!(super::publication_has_assessed_coverage(&result), "{agent}");
        let mut unassessed = result.clone();
        unassessed.coverage.selected_items = 0;
        assert!(
            !super::publication_has_assessed_coverage(&unassessed),
            "{agent}"
        );
        let mut incomplete = result.clone();
        incomplete.coverage.not_selected_items = 1;
        assert!(
            !super::publication_has_assessed_coverage(&incomplete),
            "{agent}"
        );
        let mut limited = result.clone();
        limited.coverage.limitations.push("synthetic_limit".into());
        assert!(
            !super::publication_has_assessed_coverage(&limited),
            "{agent}"
        );
        assert!(
            store
                .queue_burn_check_assessment(&first.durable, 1000, 180)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(&first.durable, 1000, 300, 180)
                .unwrap()
        );
        assert!(
            store
                .complete_burn_check_assessment(
                    &first.durable,
                    &super::publication_json(&first, &result).unwrap(),
                    1001,
                    180
                )
                .unwrap()
        );
        let (handle, provider_generation) = active_handle();
        let input_generation = super::INPUT_OBSERVATIONS
            .lock()
            .unwrap()
            .observe_input(
                &first.durable.key,
                Some(first.durable.input_revision.clone()),
            )
            .unwrap();
        std::fs::write(
            &path,
            format!("---\nname: {skill}\ndescription: Review API boundaries.\n---\n"),
        )
        .unwrap();
        let changed =
            super::prepare(&candidate, load(), super::CHECK.evaluator_revision()).unwrap();
        assert_ne!(
            first.inventory_revision, changed.inventory_revision,
            "{agent}"
        );
        assert_ne!(
            first.durable.input_revision, changed.durable.input_revision,
            "{agent}"
        );
        let fence = super::SkillWriteFence {
            store: &store,
            handle: &handle,
            provider_generation,
            input_generation,
            input: &first,
            candidate: &candidate,
            config: &config,
        };
        assert!(
            fence
                .commit(|| -> anyhow::Result<()> { panic!("changed inventory cannot publish") })
                .unwrap()
                .is_none(),
            "{agent}"
        );
        let saved = store
            .burn_check_assessment(&candidate.session.key, super::SKILL_OPPORTUNITIES_CHECK_ID)
            .unwrap()
            .unwrap();
        assert!(saved.result_json.is_none(), "{agent}");
        assert!(saved.result_revision.is_none(), "{agent}");
    }
}

#[test]
fn reconciliation_pages_equal_session_ids_across_agents_once() {
    let directory = tempfile::tempdir().unwrap();
    let store = crate::store::Store::open(directory.path()).unwrap();
    for agent in ["claude-code", "codex", "opencode", "pi"] {
        let key = crate::store::SessionKey::new("native", agent, "same-session");
        let input = durable_input_for_key(&store, key);
        assert!(
            store
                .queue_burn_check_assessment(&input, 1000, 180)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(&input, 1000, 300, 180)
                .unwrap()
        );
        assert!(
            store
                .complete_burn_check_assessment(&input, "{}", 1001, 180)
                .unwrap()
        );
    }
    let mut cursor: Option<(String, String)> = None;
    let mut seen = Vec::new();
    loop {
        let page = store
            .skill_observation_candidates(
                cursor
                    .as_ref()
                    .map(|(agent, session)| (agent.as_str(), session.as_str())),
                1,
            )
            .unwrap();
        let Some(candidate) = page.into_iter().next() else {
            break;
        };
        let identity = (
            candidate.session.key.agent,
            candidate.session.key.session_id,
        );
        cursor = Some(identity.clone());
        seen.push(identity);
        assert!(seen.len() <= 4);
    }
    assert_eq!(
        seen,
        ["claude-code", "codex", "opencode", "pi"]
            .map(|agent| (agent.to_owned(), "same-session".to_owned()))
    );
}

#[test]
fn accepted_agent_source_pairs_have_skill_source_support() {
    for (agent, format) in [
        ("opencode", SourceFormat::OpenCodeSqliteV2),
        ("codex", SourceFormat::CodexRolloutJsonl),
        ("claude", SourceFormat::ClaudeJsonl),
        ("pi", SourceFormat::PiV3Jsonl),
    ] {
        assert_eq!(source_limit(agent, format), SourceLimit::Supported);
        assert_eq!(
            source_limit("cursor", format),
            SourceLimit::ProducerContractNotAdmitted
        );
    }
    for format in [
        SourceFormat::OpenCodeJsonl,
        SourceFormat::CursorCliAgentJsonl,
        SourceFormat::AntigravityBrainJsonl,
        SourceFormat::Uncharacterized,
    ] {
        assert_eq!(
            source_limit("opencode", format),
            SourceLimit::ProducerContractNotAdmitted
        );
    }
}

#[test]
fn cursor_round_trip_keeps_the_active_sampling_run_and_candidate() {
    let check = StableId::new("skill-check", &[b"v1"]);
    let candidate = Candidate {
        id: StableId::new("skill-candidate", &[b"first"]),
        required_answers: vec![StableId::new("skill-answer", &[b"answer"])],
    };
    let mut progress = SamplingProgress::new(SamplingLimits {
        checks: 1,
        candidates_per_check: 8,
        answers_per_candidate: 8,
        judgments_per_run: 2,
    })
    .unwrap();
    progress
        .synchronize(check, StableId::new("epoch", &[b"one"]), &[candidate])
        .unwrap();
    progress.begin_run();
    let active = progress.choose_job().unwrap();
    let cursor = serde_json::json!({"revision": super::CURSOR_REVISION, "input_revision": "input", "sampling": progress,
        "active_job": active, "run_progress": antiburn_local::analysis::jev::JevRunProgress::default()});
    let restored: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&cursor).unwrap()).unwrap();
    assert_eq!(restored["sampling"]["run"], cursor["sampling"]["run"]);
    assert_eq!(
        restored["active_job"]["candidate"],
        cursor["active_job"]["candidate"]
    );
}

fn durable_input(store: &crate::store::Store) -> crate::store::BurnCheckInput {
    durable_input_for_key(
        store,
        crate::store::SessionKey::new("native", "opencode", "skill-restart"),
    )
}

fn durable_input_for_key(
    store: &crate::store::Store,
    key: crate::store::SessionKey,
) -> crate::store::BurnCheckInput {
    use crate::store::SessionRecord;
    store
        .capture_burn_check_boundaries(&[super::SKILL_OPPORTUNITIES_CHECK_ID], 0)
        .unwrap();
    store
        .upsert_sessions(
            &[SessionRecord {
                key: key.clone(),
                source_kind: "providerDb".into(),
                source_label: "synthetic-db".into(),
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: None,
                surface: "cli".into(),
                updated_at_epoch: Some(100),
                activity_cursor: "activity".into(),
                activity_source: "event".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: None,
            }],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let (incarnation, generation): (u64, i64) = store
        .lock()
        .query_row(
            "SELECT incarnation, source_generation FROM session WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            rusqlite::params![key.environment_key, key.agent, key.session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET status = 'ready', analyzed_generation = ?4,
         parser_revision = ?1, analyzer_revision = ?2, evidence_schema_revision = ?3,
          published_fence = 1, processed_fingerprint = NULL WHERE environment_key = ?5 AND agent = ?6 AND session_id = ?7",
            rusqlite::params![
                antiburn_local::analysis::PARSER_REVISION,
                antiburn_local::analysis::ANALYZER_REVISION,
                antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                generation, key.environment_key, key.agent, key.session_id
            ],
        )
        .unwrap();
    crate::store::BurnCheckInput {
        key,
        check_id: super::SKILL_OPPORTUNITIES_CHECK_ID.into(),
        incarnation,
        source_generation: generation,
        source_fingerprint: None,
        activity_cursor: "activity".into(),
        published_fence: 1,
        input_revision: "inventory-use-context".into(),
        evaluator_revision: super::CHECK.evaluator_revision(),
        boundary_at_epoch: 0,
    }
}

#[test]
fn evaluator_and_restored_cursor_bind_every_engine_revision() {
    use crate::jev::worker::JevCheckDescriptor;
    let revisions = super::SKILL_OPPORTUNITIES_REVISIONS;
    let evaluator = super::CHECK.evaluator_revision();
    assert!(evaluator.contains(&format!(
        ":p{}:c{}:q{}:r{}",
        revisions.projection, revisions.chunking, revisions.questions, revisions.reducer
    )));
    let mut cursor = super::SkillCursor {
        input_revision: "input".into(),
        provider_generation: 1,
        ..Default::default()
    };
    cursor.engine_revisions.questions += 1;
    let saved = crate::store::BurnCheckAssessment {
        key: crate::store::SessionKey::new("native", "opencode", "revision-test"),
        check_id: super::SKILL_OPPORTUNITIES_CHECK_ID.into(),
        input_revision: Some("input".into()),
        status: "running".into(),
        progress_json: serde_json::to_string(&cursor).unwrap(),
        result_json: None,
        result_revision: None,
        request_count: 0,
    };
    assert!(
        super::restore_cursor(Some(&saved), "input", 1)
            .input_revision
            .is_empty()
    );
}

fn active_handle() -> (crate::jev::worker::WorkerHandle, u64) {
    let handle = crate::jev::worker::WorkerHandle::default();
    handle
        .set_system_one_connection(
            crate::jev::config::SystemOneConnection::jev_default(),
            Some("synthetic-key".into()),
        )
        .unwrap();
    assert!(handle.key_is_current(1));
    (handle, 1)
}

#[test]
fn reconciliation_invalidates_completed_results_when_inventory_context_is_unavailable() {
    let store =
        crate::store::Store::open_in_memory(std::path::Path::new("/tmp/antiburn-skill-reconcile"))
            .unwrap();
    let input = durable_input(&store);
    assert!(
        store
            .queue_burn_check_assessment(&input, 1000, 180)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input, 1000, 300, 180)
            .unwrap()
    );
    assert!(
        store
            .complete_burn_check_assessment(&input, "{}", 1001, 180)
            .unwrap()
    );
    let candidate = crate::store::BurnCheckCandidate {
        session: store.session(&input.key).unwrap().unwrap(),
        incarnation: input.incarnation,
        source_generation: input.source_generation,
        source_fingerprint: input.source_fingerprint.clone(),
        activity_cursor: input.activity_cursor.clone(),
        published_fence: input.published_fence,
        boundary_at_epoch: input.boundary_at_epoch,
        boundary_positions: Default::default(),
        historical: false,
    };
    assert!(candidate.session.cwd.is_none());
    let (handle, provider_generation) = active_handle();
    let mut stale_candidate = candidate.clone();
    stale_candidate.published_fence += 1;
    assert!(
        super::reconcile_skill_inputs(&store, &handle, provider_generation, &stale_candidate)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .burn_check_assessment(&input.key, &input.check_id)
            .unwrap()
            .unwrap()
            .result_json
            .as_deref(),
        Some("{}")
    );
    let observation =
        super::reconcile_skill_inputs(&store, &handle, provider_generation, &candidate)
            .unwrap()
            .unwrap();
    assert!(observation.invalidated);
    assert!(observation.input_revision.is_none());
    let saved = store
        .burn_check_assessment(&input.key, &input.check_id)
        .unwrap()
        .unwrap();
    assert!(saved.result_json.is_none());
    assert!(saved.result_revision.is_none());
    assert!(saved.input_revision.is_none());
    assert_eq!(saved.progress_json, "{}");
    let repeated = super::reconcile_skill_inputs(&store, &handle, provider_generation, &candidate)
        .unwrap()
        .unwrap();
    assert!(!repeated.invalidated);
    assert_eq!(repeated.generation, observation.generation);
}

#[test]
fn observed_input_change_rejects_a_restored_checkpoint_before_reload() {
    let (handle, provider_generation) = active_handle();
    let observations =
        std::sync::Mutex::new(crate::smart_check_inputs::InventoryRevisionObserver::default());
    let key = crate::store::SessionKey::new("native", "opencode", "restored-race");
    let generation = observations
        .lock()
        .unwrap()
        .observe_input(&key, Some("old-input".into()))
        .unwrap();
    observations
        .lock()
        .unwrap()
        .observe_input(&key, Some("new-input".into()))
        .unwrap();
    assert!(
        super::with_skill_input_generation(
            &handle,
            provider_generation,
            &observations,
            super::InputWritePermit {
                key: &key,
                revision: "old-input",
                generation
            },
            || panic!("stale observation cannot reload"),
            || -> anyhow::Result<()> { panic!("stale observation cannot store restored answers") },
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn stale_provider_generation_rejects_reload_and_direct_writes() {
    let (handle, provider_generation) = active_handle();
    let observations =
        std::sync::Mutex::new(crate::smart_check_inputs::InventoryRevisionObserver::default());
    let key = crate::store::SessionKey::new("native", "opencode", "provider-race");
    let generation = observations
        .lock()
        .unwrap()
        .observe_input(&key, Some("input".into()))
        .unwrap();
    handle
        .set_system_one_connection(
            crate::jev::config::SystemOneConnection::jev_default(),
            Some("replacement-key".into()),
        )
        .unwrap();
    assert!(
        super::with_skill_input_generation(
            &handle,
            provider_generation,
            &observations,
            super::InputWritePermit {
                key: &key,
                revision: "input",
                generation
            },
            || panic!("stale provider cannot reload"),
            || -> anyhow::Result<()> { panic!("stale provider cannot write") },
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn final_reload_rejects_inventory_use_and_context_changes_and_unavailable_inputs() {
    let (handle, provider_generation) = active_handle();
    for revision in [
        Some("changed-inventory"),
        Some("changed-use"),
        Some("changed-context"),
        None,
    ] {
        let observations =
            std::sync::Mutex::new(crate::smart_check_inputs::InventoryRevisionObserver::default());
        let key = crate::store::SessionKey::new("native", "opencode", "input-race");
        let generation = observations
            .lock()
            .unwrap()
            .observe_input(&key, Some("input".into()))
            .unwrap();
        assert!(
            super::with_skill_input_generation(
                &handle,
                provider_generation,
                &observations,
                super::InputWritePermit {
                    key: &key,
                    revision: "input",
                    generation
                },
                || Ok(revision.map(str::to_owned)),
                || -> anyhow::Result<()> { panic!("changed input cannot publish") },
            )
            .unwrap()
            .is_none()
        );
        assert!(
            !observations
                .lock()
                .unwrap()
                .input_is_current(&key, "input", generation)
        );
    }
}

#[test]
fn observation_eviction_is_bounded_and_rejects_old_permits_after_reentry() {
    use crate::smart_check_inputs::InventoryRevisionObserver;
    let mut observations = InventoryRevisionObserver::default();
    let key = crate::store::SessionKey::new("native", "opencode", "first");
    let generation = observations
        .observe_input(&key, Some("input".into()))
        .unwrap();
    for index in 0..InventoryRevisionObserver::MAX_INPUTS {
        observations
            .observe_input(
                &crate::store::SessionKey::new("native", "opencode", format!("key-{index}")),
                Some("input".into()),
            )
            .unwrap();
    }
    assert!(!observations.input_is_current(&key, "input", generation));
    let next = observations
        .observe_input(&key, Some("input".into()))
        .unwrap();
    assert!(next > generation);
    assert!(!observations.input_is_current(&key, "input", generation));
    assert!(observations.input_is_current(&key, "input", next));
}

#[test]
fn checkpoint_commit_holds_provider_then_inventory_guards_through_store_write() {
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;
    let store = crate::store::Store::open_in_memory(std::path::Path::new(
        "/tmp/antiburn-skill-guarded-checkpoint",
    ))
    .unwrap();
    let input = durable_input(&store);
    assert!(
        store
            .queue_burn_check_assessment(&input, 1000, 180)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input, 1000, 300, 180)
            .unwrap()
    );
    let (handle, provider_generation) = active_handle();
    let observations = Mutex::new(crate::smart_check_inputs::InventoryRevisionObserver::default());
    let generation = observations
        .lock()
        .unwrap()
        .observe_input(&input.key, Some(input.input_revision.clone()))
        .unwrap();
    std::thread::scope(|scope| {
        let store = &store;
        let input = &input;
        let handle = &handle;
        let observations = &observations;
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer = scope.spawn(move || {
            super::with_skill_input_generation(
                handle,
                provider_generation,
                observations,
                super::InputWritePermit {
                    key: &input.key,
                    revision: &input.input_revision,
                    generation,
                },
                || Ok(Some(input.input_revision.clone())),
                || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    store.save_burn_check_checkpoint(
                        input,
                        "{\"guarded\":true}",
                        None,
                        1001,
                        300,
                        180,
                    )
                },
            )
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (provider_tx, provider_rx) = mpsc::channel();
        let provider_change = scope.spawn(move || {
            handle
                .set_system_one_connection(
                    crate::jev::config::SystemOneConnection::jev_default(),
                    Some("replacement-key".into()),
                )
                .unwrap();
            provider_tx.send(()).unwrap();
        });
        let (inventory_tx, inventory_rx) = mpsc::channel();
        let inventory_change = scope.spawn(move || {
            observations
                .lock()
                .unwrap()
                .observe_input(&input.key, Some("changed".into()))
                .unwrap();
            inventory_tx.send(()).unwrap();
        });
        assert!(matches!(
            provider_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(matches!(
            inventory_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        release_tx.send(()).unwrap();
        assert_eq!(writer.join().unwrap().unwrap(), Some(true));
        provider_change.join().unwrap();
        inventory_change.join().unwrap();
    });
    assert_eq!(
        store
            .burn_check_assessment(&input.key, &input.check_id)
            .unwrap()
            .unwrap()
            .progress_json,
        "{\"guarded\":true}"
    );
    assert!(!handle.key_is_current(provider_generation));
    assert!(!observations.lock().unwrap().input_is_current(
        &input.key,
        &input.input_revision,
        generation
    ));
}

#[test]
fn durable_cursor_survives_reopen_and_rejects_provider_or_semantic_changes() {
    let directory = tempfile::tempdir().unwrap();
    let store = crate::store::Store::open(directory.path()).unwrap();
    let input = durable_input(&store);
    assert!(
        store
            .queue_burn_check_assessment(&input, 1000, 180)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input, 1000, 300, 180)
            .unwrap()
    );
    let mut sampling = super::new_sampling_progress().unwrap();
    let check = StableId::new("skill-check", &[b"v1"]);
    sampling
        .synchronize(
            check,
            StableId::new("epoch", &[b"one"]),
            &[Candidate {
                id: StableId::new("candidate", &[b"work"]),
                required_answers: vec![StableId::new("answer", &[b"fit"])],
            }],
        )
        .unwrap();
    sampling.begin_run();
    let job = sampling.choose_job().unwrap();
    let cursor = super::SkillCursor {
        input_revision: input.input_revision.clone(),
        provider_generation: 7,
        sampling: Some(sampling),
        active_job: Some(job.clone()),
        ..Default::default()
    };
    assert!(
        store
            .save_burn_check_checkpoint(
                &input,
                &serde_json::to_string(&cursor).unwrap(),
                None,
                1001,
                300,
                180
            )
            .unwrap()
    );
    drop(store);
    let store = crate::store::Store::open(directory.path()).unwrap();
    let saved = store
        .burn_check_assessment(&input.key, &input.check_id)
        .unwrap()
        .unwrap();
    let restored = super::restore_cursor(Some(&saved), &input.input_revision, 7);
    assert_eq!(restored.active_job.unwrap(), job);
    assert_eq!(
        restored
            .sampling
            .unwrap()
            .coverage(check)
            .unwrap()
            .completed,
        0
    );
    assert!(
        super::restore_cursor(Some(&saved), &input.input_revision, 8)
            .active_job
            .is_none()
    );
    for changed in ["inventory-changed", "use-changed", "context-changed"] {
        assert!(
            super::restore_cursor(Some(&saved), changed, 7)
                .active_job
                .is_none()
        );
    }
    let mut completed = super::restore_cursor(Some(&saved), &input.input_revision, 7);
    let sampling = completed.sampling.as_mut().unwrap();
    sampling
        .record_reduced_answer(&job, StableId::new("answer", &[b"fit"]))
        .unwrap();
    sampling.complete_candidate(&job).unwrap();
    completed.active_job = None;
    assert!(
        store
            .save_burn_check_checkpoint(
                &input,
                &serde_json::to_string(&completed).unwrap(),
                None,
                1002,
                300,
                180,
            )
            .unwrap()
    );
    drop(store);
    let reopened = crate::store::Store::open(directory.path()).unwrap();
    let saved = reopened
        .burn_check_assessment(&input.key, &input.check_id)
        .unwrap()
        .unwrap();
    let mut sampling = super::restore_cursor(Some(&saved), &input.input_revision, 7)
        .sampling
        .unwrap();
    assert_eq!(sampling.coverage(check).unwrap().completed, 1);
    sampling.begin_run();
    assert!(sampling.choose_job().is_none());
}

#[test]
fn deletion_and_clear_local_data_fence_skill_checkpoints_and_results() {
    for clear in [false, true] {
        let store =
            crate::store::Store::open_in_memory(std::path::Path::new("/tmp/antiburn-skill-fences"))
                .unwrap();
        let input = durable_input(&store);
        assert!(
            store
                .queue_burn_check_assessment(&input, 1000, 180)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(&input, 1000, 300, 180)
                .unwrap()
        );
        if clear {
            store.clear_local_session_data().unwrap();
        } else {
            store.delete_session(&input.key).unwrap();
        }
        assert!(
            !store
                .save_burn_check_checkpoint(&input, "{}", None, 1001, 300, 180)
                .unwrap()
        );
        assert!(
            !store
                .complete_burn_check_assessment(&input, "{}", 1001, 180)
                .unwrap()
        );
        assert!(
            store
                .burn_check_assessment(&input.key, &input.check_id)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn new_publication_or_source_generation_fences_old_skill_work() {
    for change in [
        "UPDATE session_evidence SET published_fence = 2 WHERE session_id = 'skill-restart'",
        "UPDATE session SET source_generation = source_generation + 1 WHERE session_id = 'skill-restart'",
    ] {
        let store = crate::store::Store::open_in_memory(std::path::Path::new(
            "/tmp/antiburn-skill-source-fences",
        ))
        .unwrap();
        let input = durable_input(&store);
        assert!(
            store
                .queue_burn_check_assessment(&input, 1000, 180)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(&input, 1000, 300, 180)
                .unwrap()
        );
        store.lock().execute(change, []).unwrap();
        assert!(
            !store
                .save_burn_check_checkpoint(&input, "{}", None, 1001, 300, 180)
                .unwrap()
        );
        assert!(
            !store
                .complete_burn_check_assessment(&input, "{}", 1001, 180)
                .unwrap()
        );
    }
}

#[test]
fn incomplete_native_lifecycle_never_proves_absence() {
    assert!(super::typed_lifecycle_is_complete(
        SkillUseStatus::Complete,
        false,
        &[
            SkillUseLifecycle::Requested,
            SkillUseLifecycle::DocumentSelected
        ]
    ));
    assert!(!super::typed_lifecycle_is_complete(
        SkillUseStatus::Complete,
        true,
        &[SkillUseLifecycle::Succeeded]
    ));
    assert!(!super::typed_lifecycle_is_complete(
        SkillUseStatus::Partial,
        false,
        &[SkillUseLifecycle::Succeeded]
    ));
    assert!(super::typed_lifecycle_is_complete(
        SkillUseStatus::Complete,
        false,
        &[SkillUseLifecycle::Succeeded, SkillUseLifecycle::Failed]
    ));
}

#[test]
fn publication_requires_current_input_and_inventory_revisions() {
    let current = ("input-2".to_owned(), "inventory-3".to_owned());
    assert!(super::current_revisions_match(
        Some(&current),
        "input-2",
        "inventory-3"
    ));
    assert!(!super::current_revisions_match(
        Some(&current),
        "input-1",
        "inventory-3"
    ));
    assert!(!super::current_revisions_match(
        Some(&current),
        "input-2",
        "inventory-2"
    ));
    assert!(!super::current_revisions_match(
        None,
        "input-2",
        "inventory-3"
    ));
}
