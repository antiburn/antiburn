use super::{SourceLimit, source_limit};
use crate::jev::worker::JevCheckDescriptor;
use antiburn_local::analysis::SourceFormat;
use antiburn_local::analysis::jev::JevCheck;
use antiburn_local::checks::sampling::{Candidate, SamplingLimits, SamplingProgress, StableId};
use antiburn_local::checks::skill_opportunities::SkillUseLifecycle;

#[test]
fn invalid_published_skill_use_is_terminal_until_the_source_changes() {
    assert!(super::unavailable_is_terminal("skill_use_invalid"));
    assert!(super::unavailable_is_terminal("unsupported_format"));
    assert!(!super::unavailable_is_terminal(
        "skill_inventory_unavailable"
    ));
    assert!(!super::unavailable_is_terminal("input_query_failed"));
}

#[test]
fn two_64_session_sweeps_reuse_source_inputs_and_discover_inventory_once_per_sweep() {
    use crate::scope_creep_worker::tests::native_sources;
    use crate::smart_check_inputs::inventory_cache::INVENTORY_DISCOVERY_COUNT;
    use std::sync::atomic::Ordering;
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let workspace = directory.path().join("workspace");
    std::fs::create_dir_all(home.join(".claude/skills/review")).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        home.join(".claude/skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review code and tests.\n---\n",
    )
    .unwrap();
    let store = crate::store::Store::open(directory.path()).unwrap();
    for detector in [
        antiburn_local::checks::DetectorId::SkillOpportunities,
        antiburn_local::checks::DetectorId::ScopeCreep,
    ] {
        store.set_check_enabled(detector, true).unwrap();
    }
    store
        .capture_burn_check_boundaries(
            &[
                super::SKILL_OPPORTUNITIES_CHECK_ID,
                crate::scope_creep_worker::CHECK_ID,
            ],
            0,
        )
        .unwrap();
    let (agent, original_session, format, records) = native_sources::sources()
        .into_iter()
        .find(|(agent, _, _, _)| *agent == "claude-code")
        .unwrap();
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
    let config = crate::agent_config::ConfigContext::native(
        antiburn_local::model::AgentKind::Claude,
        &home,
        Some(workspace.clone()),
    );
    let mut candidates = Vec::new();
    for index in 0..64 {
        let session = format!("skill-session-{index:02}");
        let records = records.replace(original_session, &session);
        let candidate =
            native_sources::publish(&store, agent, &session, format, &records, &workspace);
        let mut fence = crate::smart_check_inputs::cache::source_fence(
            &candidate,
            crate::scope_creep_worker::CHECK_ID,
            crate::scope_creep_worker::CHECK.evaluator_revision(),
        );
        fence.input_revision = format!("fixture-scope-{index}");
        assert!(
            store
                .queue_burn_check_assessment(&fence, 1000, 180)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(&fence, 1000, 300, 180)
                .unwrap()
        );
        assert!(
            store
                .complete_burn_check_assessment(&fence, "{}", 1000, 180)
                .unwrap()
        );
        candidates.push(candidate);
    }
    let before = crate::smart_check_inputs::INPUT_LOAD_COUNT.get();
    let discoveries = INVENTORY_DISCOVERY_COUNT.load(Ordering::Relaxed);
    let revisions = candidates
        .iter()
        .map(|candidate| {
            super::current_input_revisions(&store, candidate, &config)
                .unwrap()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        crate::smart_check_inputs::INPUT_LOAD_COUNT.get(),
        before + 64
    );
    assert_eq!(
        INVENTORY_DISCOVERY_COUNT.load(Ordering::Relaxed),
        discoveries + 1
    );
    let started = std::time::Instant::now();
    for (candidate, revision) in candidates.iter().zip(&revisions) {
        assert_eq!(
            super::current_input_revisions(&store, candidate, &config)
                .unwrap()
                .as_ref(),
            Some(revision)
        );
    }
    assert_eq!(
        crate::smart_check_inputs::INPUT_LOAD_COUNT.get(),
        before + 64
    );
    assert_eq!(
        INVENTORY_DISCOVERY_COUNT.load(Ordering::Relaxed),
        discoveries + 2
    );
    let sweep_us = started.elapsed().as_micros();
    let inventory =
        crate::smart_check_inputs::inventory_cache::discover_inventory(&config).unwrap();
    for candidate in &candidates {
        let first = super::load_input(
            &store,
            candidate,
            &config,
            std::sync::Arc::clone(&inventory),
        )
        .unwrap();
        for _ in 0..3 {
            let continuation = super::load_input(
                &store,
                candidate,
                &config,
                std::sync::Arc::clone(&inventory),
            )
            .unwrap();
            assert!(std::sync::Arc::ptr_eq(&first, &continuation));
        }
    }
    assert_eq!(
        crate::smart_check_inputs::INPUT_LOAD_COUNT.get(),
        before + 64
    );
    eprintln!(
        "skill second sweep: sessions=64 source_loads=0 inventory_discoveries=1 elapsed_us={sweep_us}"
    );
}

#[test]
fn checkpoints_reuse_inventory_but_publication_checks_an_external_edit() {
    use crate::smart_check_inputs::inventory_cache::{
        INVENTORY_DISCOVERY_COUNT, discover_inventory,
    };
    use std::sync::atomic::Ordering;
    let (fixture, prepared) = native_skill_fixture(2, 2);
    let candidate = fixture
        .store
        .burn_check_candidates_for_revision(
            super::SKILL_OPPORTUNITIES_CHECK_ID,
            &super::CHECK.evaluator_revision(),
            1000,
            super::POLICY.idle_secs,
            16,
        )
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.session.key == prepared.durable.key)
        .unwrap();
    let config = crate::agent_config::ConfigContext::native(
        antiburn_local::model::AgentKind::OpenCode,
        fixture.directory.path().join("home"),
        Some(fixture.directory.path().join("workspace")),
    );
    let input = super::load_input(
        &fixture.store,
        &candidate,
        &config,
        discover_inventory(&config).unwrap(),
    )
    .unwrap();
    let handle = crate::jev::worker::WorkerHandle::default();
    handle
        .set_system_one_connection(
            crate::jev::config::SystemOneConnection::jev_default(),
            Some("synthetic-key".into()),
        )
        .unwrap();
    let observation = super::observe_skill_input(
        &fixture.store,
        &handle,
        1,
        &candidate,
        Some(input.durable.input_revision.clone()),
    )
    .unwrap()
    .unwrap();
    let fence = super::SkillWriteFence {
        store: &fixture.store,
        handle: &handle,
        provider_generation: 1,
        input_generation: observation.generation,
        input: &input,
        candidate: &candidate,
        config: &config,
    };
    let before = INVENTORY_DISCOVERY_COUNT.load(Ordering::Relaxed);
    for _ in 0..10 {
        assert_eq!(fence.commit(|| Ok(true)).unwrap(), Some(true));
    }
    assert_eq!(INVENTORY_DISCOVERY_COUNT.load(Ordering::Relaxed), before);
    std::fs::write(
        config.home_root.join(".opencode/skills/review-0/SKILL.md"),
        "---\nname: review-0\ndescription: Review changed behavior.\n---\n",
    )
    .unwrap();
    assert!(
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(fence.publish(|| -> anyhow::Result<bool> {
                panic!("changed inventory must not publish")
            }))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        INVENTORY_DISCOVERY_COUNT.load(Ordering::Relaxed),
        before + 1
    );
}

#[test]
fn unchanged_freshness_reads_reuse_source_preparation_and_inventory_edits_invalidate_it() {
    let (fixture, input) = native_skill_fixture(20, 2);
    let candidate = fixture
        .store
        .burn_check_candidates_for_revision(
            super::SKILL_OPPORTUNITIES_CHECK_ID,
            &super::CHECK.evaluator_revision(),
            1000,
            super::POLICY.idle_secs,
            16,
        )
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.session.key == input.durable.key)
        .unwrap();
    let config = crate::agent_config::ConfigContext::native(
        antiburn_local::model::AgentKind::OpenCode,
        fixture.directory.path().join("home"),
        Some(fixture.directory.path().join("workspace")),
    );
    let first = super::current_input_revisions(&fixture.store, &candidate, &config)
        .unwrap()
        .unwrap();
    let before = crate::smart_check_inputs::INPUT_LOAD_COUNT.get();
    let started = std::time::Instant::now();
    for _ in 0..10 {
        assert_eq!(
            super::current_input_revisions(&fixture.store, &candidate, &config)
                .unwrap()
                .unwrap(),
            first
        );
    }
    assert_eq!(crate::smart_check_inputs::INPUT_LOAD_COUNT.get(), before);
    eprintln!(
        "skill freshness: validations=10 full_source_reloads=0 elapsed_us={}",
        started.elapsed().as_micros()
    );
    std::fs::write(
        config.home_root.join(".opencode/skills/review-0/SKILL.md"),
        "---\nname: review-0\ndescription: Review changed behavior.\n---\n",
    )
    .unwrap();
    let changed = super::current_input_revisions(&fixture.store, &candidate, &config)
        .unwrap()
        .unwrap();
    assert_ne!(changed, first);
    assert_eq!(
        crate::smart_check_inputs::INPUT_LOAD_COUNT.get(),
        before + 1
    );
    fixture
        .store
        .lock()
        .execute("UPDATE session_evidence SET status = 'pending'", [])
        .unwrap();
    assert!(
        super::current_input_revisions(&fixture.store, &candidate, &config)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        crate::smart_check_inputs::INPUT_LOAD_COUNT.get(),
        before + 1
    );
}

fn skill_advisory_answers(
    plan: &antiburn_local::analysis::jev::JevCheckPlan<
        antiburn_local::checks::skill_opportunities::PreparedSkillOpportunities,
    >,
) -> Vec<antiburn_local::analysis::jev::JevWorkItemResult> {
    use antiburn_local::analysis::jev::{JevAnswer, JevQuestion, JevUsage, JevWorkItemResult};
    plan.work_items
        .iter()
        .map(|item| {
            let comparison = plan
                .prepared
                .comparisons
                .iter()
                .find(|comparison| comparison.id == item.id)
                .unwrap();
            let choice = if comparison.skill.name == "unused-review" {
                "useful_opportunity"
            } else {
                "uncertain"
            };
            JevWorkItemResult {
                request_id: item.id.clone(),
                work_item_id: item.id.clone(),
                model: plan.capabilities.model.clone(),
                answers: item
                    .questions
                    .iter()
                    .map(|(key, question)| {
                        let JevQuestion::Choice { criteria, .. } = question else {
                            panic!("expected a skill choice")
                        };
                        assert!(criteria.contains_key(choice));
                        (
                            key.clone(),
                            JevAnswer::Choice {
                                choice: choice.into(),
                                confidence: 1.0,
                                probabilities: criteria
                                    .keys()
                                    .map(|key| (key.clone(), if key == choice { 1.0 } else { 0.0 }))
                                    .collect(),
                            },
                        )
                    })
                    .collect(),
                evidence: plan
                    .shared_context
                    .iter()
                    .flat_map(|context| context.evidence.iter())
                    .chain(&item.window.evidence)
                    .cloned()
                    .collect(),
                usage: JevUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            }
        })
        .collect()
}

fn native_skill_fixture(
    groups: usize,
    skills_count: usize,
) -> (
    crate::scope_creep_worker::tests::NativeFixture,
    super::PreparedSkillOpportunityInput,
) {
    let fixture = crate::scope_creep_worker::tests::NativeFixture::paged_groups(groups);
    fixture
        .store
        .set_check_enabled(antiburn_local::checks::DetectorId::SkillOpportunities, true)
        .unwrap();
    fixture
        .store
        .capture_burn_check_boundaries(&[super::SKILL_OPPORTUNITIES_CHECK_ID], 0)
        .unwrap();
    let home = fixture.directory.path().join("home");
    let workspace = fixture.directory.path().join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    for index in 0..skills_count {
        let path = home.join(format!(".opencode/skills/review-{index}/SKILL.md"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!("---\nname: review-{index}\ndescription: Review code and tests.\n---\n"),
        )
        .unwrap();
    }
    let mut candidate = fixture.publish();
    candidate.session.cwd = Some(workspace.to_str().unwrap().to_owned());
    fixture
        .store
        .lock()
        .execute(
            "UPDATE session SET cwd = ?1 WHERE session_id = ?2",
            rusqlite::params![candidate.session.cwd, candidate.session.key.session_id],
        )
        .unwrap();
    let config = crate::agent_config::ConfigContext::native(
        antiburn_local::model::AgentKind::OpenCode,
        &home,
        Some(workspace),
    );
    let snapshot = fixture
        .store
        .load_smart_check_inputs(
            &candidate.session.key,
            candidate.published_fence,
            candidate.source_generation,
            crate::smart_check_inputs::DetectorInput::SkillOpportunities,
        )
        .unwrap();
    let skills = fixture
        .store
        .load_smart_check_skill_inputs(snapshot, &config)
        .unwrap();
    let input = super::prepare(&candidate, skills, super::CHECK.evaluator_revision()).unwrap();
    (fixture, input)
}

#[test]
fn judged_skill_reassessment_retracts_stale_findings_but_unassessed_keeps_them() {
    let (_fixture, input) = native_skill_fixture(1, 1);
    let plan = input
        .check
        .prepare_with_capabilities(
            &input.check.session_context(),
            &super::ModelCapabilities::jev_default(),
        )
        .unwrap();
    let mut answers = skill_advisory_answers(&plan);
    for answer in &mut answers {
        for value in answer.answers.values_mut() {
            let antiburn_local::analysis::jev::JevAnswer::Choice {
                choice,
                probabilities,
                ..
            } = value
            else {
                panic!("expected choice")
            };
            *choice = "useful_opportunity".into();
            for (key, probability) in probabilities {
                *probability = if key == "useful_opportunity" {
                    1.0
                } else {
                    0.0
                };
            }
        }
    }
    let positive = input.check.reduce(&plan, &answers, true).unwrap();
    assert!(!positive.findings.is_empty());
    let mut saved = positive.clone();
    super::merge_skill_result(&mut saved, input.check.reduce(&plan, &[], false).unwrap());
    assert_eq!(saved.findings, positive.findings);
    for answer in &mut answers {
        for value in answer.answers.values_mut() {
            let antiburn_local::analysis::jev::JevAnswer::Choice {
                choice,
                probabilities,
                ..
            } = value
            else {
                panic!("expected choice")
            };
            *choice = "no_opportunity".into();
            for (key, probability) in probabilities {
                *probability = if key == "no_opportunity" { 1.0 } else { 0.0 };
            }
        }
    }
    let negative = input.check.reduce(&plan, &answers, true).unwrap();
    super::merge_skill_result(&mut saved, negative.clone());
    assert!(saved.findings.is_empty());
    assert_eq!(saved.decisions, negative.decisions);
}

#[test]
fn descriptor_continuation_reopens_beyond_4096_without_retrying_terminal_page() {
    let (fixture, input) = native_skill_fixture(260, 64);
    assert!(
        input.check.descriptor_count() > super::MAX_SAMPLE_CANDIDATES,
        "count={}",
        input.check.descriptor_count()
    );
    let mut cursor = super::SkillCursor {
        input_revision: input.durable.input_revision.clone(),
        sampling: Some(super::new_sampling_progress().unwrap()),
        ..Default::default()
    };
    while cursor.inventory.descriptors.len() < super::MAX_SAMPLE_CANDIDATES {
        super::enumerate_skill_turn(&input, &mut cursor).unwrap();
    }
    let first_page: std::collections::BTreeSet<_> = input
        .check
        .descriptor_candidates(&cursor.inventory)
        .unwrap()
        .into_iter()
        .map(|candidate| candidate.id)
        .collect();
    let sampling = cursor.sampling.as_mut().unwrap();
    while sampling.runnable_count(input.check.sampling_identity()) > 0 {
        sampling.begin_run();
        while let Some(job) = sampling.choose_job() {
            sampling.terminate_candidate(&job).unwrap();
        }
    }
    cursor.context_blocked_ids.insert("first-page-fit".into());
    assert_eq!(
        cursor
            .sampling
            .as_ref()
            .unwrap()
            .coverage(input.check.sampling_identity())
            .unwrap()
            .remaining,
        4096
    );
    assert_eq!(
        cursor
            .sampling
            .as_ref()
            .unwrap()
            .runnable_count(input.check.sampling_identity()),
        0
    );
    assert!(
        fixture
            .store
            .queue_burn_check_assessment(&input.durable, super::unix_now(), super::POLICY.idle_secs)
            .unwrap()
    );
    assert!(
        fixture
            .store
            .claim_burn_check_assessment(
                &input.durable,
                super::unix_now(),
                super::POLICY.lease_secs,
                super::POLICY.idle_secs
            )
            .unwrap()
    );
    super::save_scheduling(&fixture.store, &input, &cursor).unwrap();
    fixture
        .store
        .save_burn_check_checkpoint(
            &input.durable,
            &serde_json::to_string(&cursor).unwrap(),
            None,
            super::unix_now(),
            super::POLICY.lease_secs,
            super::POLICY.idle_secs,
        )
        .unwrap();
    let reopened = crate::store::Store::open(fixture.directory.path()).unwrap();
    let saved = reopened
        .burn_check_assessment(&input.durable.key, super::SKILL_OPPORTUNITIES_CHECK_ID)
        .unwrap()
        .unwrap();
    let mut restored = super::restore_cursor(Some(&saved), &input.durable.input_revision, 99);
    super::enumerate_skill_turn(&input, &mut restored).unwrap();
    assert_eq!(restored.inventory.page_start, 4096);
    assert!(restored.inventory.next_comparison > 4096);
    assert_eq!(restored.skipped_before_page, 4096);
    assert_eq!(restored.context_blocked_before_page, 1);
    assert!(restored.context_blocked_ids.is_empty());
    assert!(
        input
            .check
            .descriptor_candidates(&restored.inventory)
            .unwrap()
            .iter()
            .all(|candidate| !first_page.contains(&candidate.id))
    );
    super::save_scheduling(&reopened, &input, &restored).unwrap();
    let (total, reviewed, runnable): (usize, usize, usize) = reopened.lock().query_row("SELECT eligible_targets, reviewed_targets, runnable_targets FROM burn_check_assessment WHERE check_id = ?1", [super::SKILL_OPPORTUNITIES_CHECK_ID], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
    assert_eq!(total, input.check.descriptor_count());
    assert_eq!(reviewed, 0);
    assert_eq!(runnable, total - 4096);
}

#[test]
fn empty_skill_inputs_are_terminal_without_model_review() {
    for (groups, skills) in [(2, 0), (0, 2)] {
        let (fixture, input) = native_skill_fixture(groups, skills);
        let mut cursor = super::SkillCursor {
            input_revision: input.durable.input_revision.clone(),
            sampling: Some(super::new_sampling_progress().unwrap()),
            ..Default::default()
        };
        super::enumerate_skill_turn(&input, &mut cursor).unwrap();
        assert!(cursor.inventory.complete);
        assert_eq!(
            cursor
                .sampling
                .as_ref()
                .unwrap()
                .coverage(input.check.sampling_identity())
                .unwrap()
                .eligible,
            0
        );
        assert!(super::record_no_candidates(&fixture.store, &input, &cursor).unwrap());
        let (category, retry): (Option<String>, Option<i64>) = fixture.store.lock().query_row(
            "SELECT last_error_category, next_attempt_at_epoch FROM burn_check_assessment WHERE check_id = ?1",
            [super::SKILL_OPPORTUNITIES_CHECK_ID], |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(category.as_deref(), Some("no_candidates"));
        assert_eq!(retry, None);
        assert!(
            !fixture
                .store
                .queue_burn_check_assessment(
                    &input.durable,
                    super::unix_now() + 1000,
                    super::POLICY.idle_secs
                )
                .unwrap()
        );
        let mut changed = input.durable.clone();
        changed.input_revision.push_str("-changed");
        assert!(
            fixture
                .store
                .queue_burn_check_assessment(
                    &changed,
                    super::unix_now() + 1000,
                    super::POLICY.idle_secs
                )
                .unwrap()
        );
    }
}

#[test]
fn skill_descriptor_pages_persist_chronology_and_terminal_jobs_without_review() {
    let (fixture, input) = native_skill_fixture(20, 16);
    assert!(input.check.descriptor_count() > 256);
    let mut cursor = super::restore_cursor(None, &input.durable.input_revision, 7);
    cursor.sampling = Some(super::new_sampling_progress().unwrap());
    super::enumerate_skill_turn(&input, &mut cursor).unwrap();
    assert!(!cursor.inventory.complete);
    assert!(!cursor.inventory.descriptors.is_empty());
    assert!(cursor.inventory.descriptors.len() <= 256);
    let chronology = input
        .check
        .descriptor_chronology(&cursor.inventory)
        .unwrap();
    assert_eq!(chronology.len(), cursor.inventory.descriptors.len());
    cursor.sampling.as_mut().unwrap().begin_run();
    let jobs: Vec<_> =
        std::iter::from_fn(|| cursor.sampling.as_mut().unwrap().choose_job()).collect();
    assert_eq!(jobs.len(), 4);
    let mut capabilities =
        antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default();
    capabilities.request_body_bytes.value = Some(1);
    let selected = input
        .check
        .prepare_inventory_sampled(
            &cursor.inventory,
            &input.check.session_context(),
            &capabilities,
            &jobs,
        )
        .unwrap();
    assert_eq!(selected.prepared.comparisons.len(), 4);
    assert!(selected.work_items.is_empty());
    let gap = input.check.reduce(&selected, &[], false).unwrap();
    assert!(
        gap.decisions
            .iter()
            .all(|decision| decision.judgments.is_none())
    );
    cursor.result = Some(gap);
    for job in &jobs {
        cursor
            .sampling
            .as_mut()
            .unwrap()
            .terminate_candidate(job)
            .unwrap();
    }
    let now = crate::jev::worker::unix_now();
    fixture
        .store
        .queue_burn_check_assessment(&input.durable, now, super::POLICY.idle_secs)
        .unwrap();
    fixture
        .store
        .claim_burn_check_assessment(
            &input.durable,
            now,
            super::POLICY.lease_secs,
            super::POLICY.idle_secs,
        )
        .unwrap();
    super::save_scheduling(&fixture.store, &input, &cursor).unwrap();
    fixture
        .store
        .save_burn_check_checkpoint(
            &input.durable,
            &serde_json::to_string(&cursor).unwrap(),
            None,
            now,
            super::POLICY.lease_secs,
            super::POLICY.idle_secs,
        )
        .unwrap();
    fixture
        .store
        .release_failed_burn_check_lease(&input.durable, "continuing", now + 1)
        .unwrap();
    let reopened = crate::store::Store::open(fixture.directory.path()).unwrap();
    let counts: (Option<usize>, usize, usize) = reopened.lock().query_row(
        "SELECT eligible_targets, reviewed_targets, runnable_targets FROM burn_check_assessment WHERE check_id = ?1",
        [super::SKILL_OPPORTUNITIES_CHECK_ID], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
    assert_eq!(
        counts,
        (
            Some(input.check.descriptor_count()),
            0,
            input.check.descriptor_count() - 4
        )
    );
    let saved = reopened
        .burn_check_assessment(&input.durable.key, super::SKILL_OPPORTUNITIES_CHECK_ID)
        .unwrap()
        .unwrap();
    let mut restored = super::restore_cursor(Some(&saved), &input.durable.input_revision, 99);
    assert_eq!(restored.inventory, cursor.inventory);
    let first_descriptors = restored.inventory.descriptors.clone();
    super::enumerate_skill_turn(&input, &mut restored).unwrap();
    assert!(
        restored
            .inventory
            .descriptors
            .starts_with(&first_descriptors)
    );
    assert!(restored.inventory.descriptors.len() - first_descriptors.len() <= 256);
    while !restored.inventory.complete {
        super::enumerate_skill_turn(&input, &mut restored).unwrap();
    }
    assert_eq!(
        restored.inventory.descriptors.len(),
        input.check.descriptor_count()
    );
    assert_eq!(
        restored
            .sampling
            .as_ref()
            .unwrap()
            .coverage(input.check.sampling_identity())
            .unwrap()
            .completed,
        0
    );
    assert_eq!(
        restored
            .sampling
            .as_ref()
            .unwrap()
            .runnable_count(input.check.sampling_identity()),
        input.check.descriptor_count() - 4
    );
    super::save_scheduling(&reopened, &input, &restored).unwrap();
    let counts: (Option<usize>, usize, usize) = reopened.lock().query_row(
        "SELECT eligible_targets, reviewed_targets, runnable_targets FROM burn_check_assessment WHERE check_id = ?1",
        [super::SKILL_OPPORTUNITIES_CHECK_ID], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
    assert_eq!(
        counts,
        (
            Some(input.check.descriptor_count()),
            0,
            input.check.descriptor_count() - 4
        )
    );
    let mut empty_partial = super::SkillCursor {
        input_revision: input.durable.input_revision.clone(),
        sampling: Some(super::new_sampling_progress().unwrap()),
        ..Default::default()
    };
    empty_partial
        .sampling
        .as_mut()
        .unwrap()
        .synchronize_ordered(
            input.check.sampling_identity(),
            input.check.sampling_epoch(),
            &[],
            &[],
        )
        .unwrap();
    super::save_scheduling(&reopened, &input, &empty_partial).unwrap();
    let counts: (Option<usize>, usize, usize) = reopened.lock().query_row(
        "SELECT eligible_targets, reviewed_targets, runnable_targets FROM burn_check_assessment WHERE check_id = ?1",
        [super::SKILL_OPPORTUNITIES_CHECK_ID], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
    assert_eq!(
        counts,
        (
            Some(input.check.descriptor_count()),
            0,
            input.check.descriptor_count()
        )
    );
}

#[test]
fn mocked_authentication_rejection_preserves_skill_batch_and_replacement_generation() {
    use antiburn_local::analysis::jev::{
        JevError, JevRunProgress, admit_jev_orchestration, run_jev_check_prepared,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    for replace_generation in [false, true] {
        let (fixture, input) = native_skill_fixture(2, 3);
        let capabilities =
            antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default();
        let (handle, generation) = active_handle();
        let mut cursor = super::restore_cursor(None, &input.durable.input_revision, generation);
        cursor.sampling = Some(super::new_sampling_progress().unwrap());
        super::enumerate_skill_turn(&input, &mut cursor).unwrap();
        assert!(cursor.inventory.complete);
        assert!(cursor.inventory.descriptors.len() > 4);
        cursor.sampling.as_mut().unwrap().begin_run();
        let jobs: Vec<_> =
            std::iter::from_fn(|| cursor.sampling.as_mut().unwrap().choose_job()).collect();
        assert_eq!(jobs.len(), 4);
        cursor.active_job = jobs.first().cloned();
        cursor.batch_jobs = jobs.iter().skip(1).cloned().collect();
        let mut plan = input
            .check
            .prepare_inventory_sampled(
                &cursor.inventory,
                &input.check.session_context(),
                &capabilities,
                &jobs,
            )
            .unwrap();
        assert_eq!(plan.work_items.len(), 4);
        cursor.active_plan = Some(plan.clone());
        let now = crate::jev::worker::unix_now();
        fixture
            .store
            .queue_burn_check_assessment(&input.durable, now, super::POLICY.idle_secs)
            .unwrap();
        fixture
            .store
            .claim_burn_check_assessment(
                &input.durable,
                now,
                super::POLICY.lease_secs,
                super::POLICY.idle_secs,
            )
            .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let dispatches = AtomicUsize::new(0);
        let terminal_probes = AtomicUsize::new(0);
        let mut connection = handle.system_one_connection();
        connection
            .model_revision
            .clone_from(&capabilities.model_revision);
        let outcome = runtime.block_on(async {
            run_jev_check_prepared(
                input.check.as_ref(),
                &input.check.session_context(),
                &mut plan,
                JevRunProgress::default(),
                admit_jev_orchestration().await.unwrap(),
                |batch| {
                    dispatches.fetch_add(1, Ordering::SeqCst);
                    crate::scope_creep_worker::tests::mark_authentication_rejected_batch(
                        &fixture.store,
                        &input.durable,
                        &connection,
                        &batch,
                    );
                    async { Err(JevError::AuthenticationRejected) }
                },
                |_| Ok(()),
            )
            .await
            .unwrap()
        });
        assert_eq!(outcome.failure, Some(JevError::AuthenticationRejected));
        cursor.run_progress = outcome.progress;
        cursor.result = Some(outcome.result);
        super::save_scheduling(&fixture.store, &input, &cursor).unwrap();
        fixture
            .store
            .save_burn_check_checkpoint(
                &input.durable,
                &serde_json::to_string(&cursor).unwrap(),
                Some(&cursor.run_progress),
                now,
                super::POLICY.lease_secs,
                super::POLICY.idle_secs,
            )
            .unwrap();
        for job in &jobs {
            let mut target = plan.clone();
            target.work_items.retain(|item| {
                StableId::new("skill-opportunities", &[item.id.as_bytes()]) == job.candidate
            });
            assert_eq!(
                crate::scope_creep_worker::dispatch_readiness(
                    &fixture.store,
                    &handle,
                    &input.durable,
                    &capabilities,
                    &target,
                    &cursor.run_progress
                )
                .unwrap(),
                crate::store::BurnCheckRequestAdmission::Exhausted
            );
            assert!(
                !crate::scope_creep_worker::target_failure_is_terminal(
                    outcome.failure.as_ref().unwrap(),
                    || {
                        terminal_probes.fetch_add(1, Ordering::SeqCst);
                        crate::scope_creep_worker::dispatch_readiness(
                            &fixture.store,
                            &handle,
                            &input.durable,
                            &capabilities,
                            &target,
                            &cursor.run_progress,
                        )
                    }
                )
                .unwrap()
            );
        }
        if replace_generation {
            handle
                .set_system_one_connection(
                    crate::jev::config::SystemOneConnection::jev_default(),
                    Some("replacement-key".into()),
                )
                .unwrap();
        }
        let result_json = super::publication_json(&input, cursor.result.as_ref().unwrap()).unwrap();
        handle
            .with_current_generation(generation, || {
                fixture.store.fail_burn_check_assessment_with_result(
                    &input.durable,
                    &crate::store::BurnCheckFailure {
                        error_category: "authentication_rejected",
                        result_json: &result_json,
                        progress_json: &serde_json::to_string(&cursor).unwrap(),
                        retry_at_epoch: None,
                    },
                    now,
                    super::POLICY.idle_secs,
                )
            })
            .transpose()
            .unwrap();
        assert_eq!(
            handle
                .reject_authentication(&fixture.store, generation)
                .unwrap(),
            !replace_generation
        );
        assert_eq!(dispatches.load(Ordering::SeqCst), 1);
        assert_eq!(terminal_probes.load(Ordering::SeqCst), 0);
        assert_eq!(handle.authentication_rejected(), !replace_generation);
        assert!(!handle.key_is_current(generation));
        assert_eq!(handle.is_available(), replace_generation);
        assert_eq!(
            fixture
                .store
                .internal_value("internal:typesafeAuthRejectedV1")
                .as_deref(),
            (!replace_generation).then_some("true")
        );
        let reopened = crate::store::Store::open(fixture.directory.path()).unwrap();
        let saved = reopened
            .burn_check_assessment(&input.durable.key, super::SKILL_OPPORTUNITIES_CHECK_ID)
            .unwrap()
            .unwrap();
        let restored =
            super::restore_cursor(Some(&saved), &input.durable.input_revision, generation + 1);
        assert_eq!(restored.active_job, cursor.active_job);
        assert_eq!(restored.batch_jobs, cursor.batch_jobs);
        assert_eq!(restored.active_plan, cursor.active_plan);
        assert_eq!(restored.run_progress, cursor.run_progress);
        assert_eq!(
            restored
                .sampling
                .as_ref()
                .unwrap()
                .runnable_count(input.check.sampling_identity()),
            input.check.descriptor_count()
        );
        assert_eq!(
            restored
                .sampling
                .as_ref()
                .unwrap()
                .coverage(input.check.sampling_identity())
                .unwrap()
                .completed,
            0
        );
    }
}

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
        for detector in [
            antiburn_local::checks::DetectorId::SkillOpportunities,
            antiburn_local::checks::DetectorId::ScopeCreep,
        ] {
            store.set_check_enabled(detector, true).unwrap();
        }
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
        let mut bound_candidate = candidate.clone();
        bound_candidate
            .boundary_positions
            .insert(inputs.input().boundary().source_key.clone(), u64::MAX);
        let bound = inputs.clone().for_candidate(&bound_candidate).unwrap();
        let bound_usage = antiburn_local::analysis::jev_evidence::select_session_content(
            bound.input().content(),
            antiburn_local::checks::skill_opportunities::SKILL_USE_SELECTION,
        );
        assert_eq!(
            bound.usage().selected_input_revision(),
            Some(bound_usage.selected_input_digest.as_str())
        );
        assert_eq!(bound.check().unwrap().descriptor_count(), 0);
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
                .any(|comparison| comparison.skill.name == skill),
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
                        assert_eq!(question, "opportunity");
                        let choice = "no_opportunity";
                        (
                            question.clone(),
                            antiburn_local::analysis::jev::JevAnswer::Choice {
                                choice: choice.into(),
                                confidence: 1.0,
                                probabilities: std::collections::BTreeMap::from([
                                    ("useful_opportunity".into(), 0.0),
                                    ("specialist_check".into(), 0.0),
                                    ("already_covered".into(), 0.0),
                                    ("no_opportunity".into(), 1.0),
                                    ("uncertain".into(), 0.0),
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
        let mut sampling = super::new_sampling_progress().unwrap();
        super::synchronize_sampling(&first, &mut sampling).unwrap();
        sampling.begin_run();
        let jobs: Vec<_> = std::iter::from_fn(|| sampling.choose_job()).collect();
        let sampled = super::prepare_sampled(&first, &plan.capabilities, &jobs).unwrap();
        assert_eq!(sampled.work_items.len(), jobs.len());
        let sampled_answers = skill_advisory_answers(&sampled);
        let partial = first
            .check
            .reduce(&sampled, &sampled_answers, true)
            .unwrap();
        assert!(!partial.complete);
        assert!(!partial.findings.is_empty());
        assert!(super::publication_has_assessed_coverage(&partial));
        for job in &jobs {
            first
                .check
                .record_sampling_result(&mut sampling, job, &partial)
                .unwrap();
            let pairs = super::sampled_pairs_for_job(
                &sampled,
                &partial,
                job,
                candidate.incarnation,
                &first.durable.input_revision,
            );
            assert_eq!(pairs.len(), 1);
            assert!(pairs[0].pair.assessed);
        }
        assert_eq!(
            sampling
                .coverage(first.check.sampling_identity())
                .unwrap()
                .completed,
            jobs.len()
        );
        sampling.begin_run();
        let remaining = sampling
            .coverage(first.check.sampling_identity())
            .unwrap()
            .remaining;
        match sampling.choose_job() {
            Some(next) => {
                assert!(remaining > 0);
                assert!(jobs.iter().all(|job| job.candidate != next.candidate));
            }
            None => assert_eq!(remaining, 0),
        }
        let mut finding = partial.findings[0].clone();
        finding.comparison.absence_assessable = false;
        finding.comparison.work_context_assessable = false;
        finding.comparison.use_eligibility.absence =
            antiburn_local::checks::skill_opportunities::SkillAbsenceEvidence::Unassessable;
        assert!(super::publishable_finding(&finding));
        let mut merged = first.check.reduce(&plan, &[], false).unwrap();
        super::merge_skill_result(&mut merged, partial.clone());
        assert_eq!(
            merged
                .decisions
                .iter()
                .filter(|decision| decision.judgments.is_some())
                .count(),
            jobs.len(),
        );
        assert_eq!(merged.findings, partial.findings);
        let mut unassessed = result.clone();
        unassessed.coverage.selected_items = 0;
        assert!(
            !super::publication_has_assessed_coverage(&unassessed),
            "{agent}"
        );
        let mut incomplete = result.clone();
        incomplete.complete = true;
        incomplete.coverage.not_selected_items = 1;
        assert!(
            !super::publication_has_assessed_coverage(&incomplete),
            "{agent}"
        );
        let mut limited = result.clone();
        limited.complete = true;
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
        let mut interrupted_sampling = super::new_sampling_progress().unwrap();
        super::synchronize_sampling(&first, &mut interrupted_sampling).unwrap();
        interrupted_sampling.begin_run();
        let interrupted_jobs: Vec<_> =
            std::iter::from_fn(|| interrupted_sampling.choose_job()).collect();
        let interrupted_plan =
            super::prepare_sampled(&first, &plan.capabilities, &interrupted_jobs).unwrap();
        let interrupted_answers = skill_advisory_answers(&interrupted_plan);
        let selected_answer = interrupted_answers
            .iter()
            .find(|answer| {
                StableId::new("skill-opportunities", &[answer.work_item_id.as_bytes()])
                    == interrupted_jobs[0].candidate
            })
            .unwrap()
            .clone();
        let accepted = first
            .check
            .reduce(
                &interrupted_plan,
                std::slice::from_ref(&selected_answer),
                false,
            )
            .unwrap();
        first
            .check
            .record_sampling_result(&mut interrupted_sampling, &interrupted_jobs[0], &accepted)
            .unwrap();
        let mut inventory =
            antiburn_local::checks::skill_opportunities::SkillDescriptorInventory::default();
        first.check.enumerate_descriptors(&mut inventory).unwrap();
        assert!(inventory.complete);
        let cursor = super::SkillCursor {
            input_revision: first.durable.input_revision.clone(),
            provider_generation: 7,
            sampling: Some(interrupted_sampling),
            active_job: interrupted_jobs.first().cloned(),
            batch_jobs: interrupted_jobs.iter().skip(1).cloned().collect(),
            active_plan: Some(interrupted_plan.clone()),
            result: Some(accepted),
            inventory,
            run_progress: antiburn_local::analysis::jev::JevRunProgress {
                results: std::collections::BTreeMap::from([(
                    selected_answer.work_item_id.clone(),
                    selected_answer,
                )]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(super::save_scheduling(&store, &first, &cursor).unwrap());
        assert!(
            store
                .save_burn_check_checkpoint(
                    &first.durable,
                    &serde_json::to_string(&cursor).unwrap(),
                    Some(&cursor.run_progress),
                    1001,
                    300,
                    180
                )
                .unwrap()
        );
        store
            .release_failed_burn_check_lease(&first.durable, "continuing", 1002)
            .unwrap();
        let reopened = crate::store::Store::open(directory.path()).unwrap();
        let saved = reopened
            .burn_check_assessment(&first.durable.key, super::SKILL_OPPORTUNITIES_CHECK_ID)
            .unwrap()
            .unwrap();
        let restored = super::restore_cursor(Some(&saved), &first.durable.input_revision, 99);
        assert_eq!(restored.active_job, cursor.active_job);
        assert_eq!(restored.batch_jobs, cursor.batch_jobs);
        assert_eq!(restored.active_plan, Some(interrupted_plan));
        assert_eq!(restored.run_progress, cursor.run_progress);
        assert_eq!(
            restored
                .sampling
                .as_ref()
                .unwrap()
                .coverage(first.check.sampling_identity())
                .unwrap()
                .completed,
            1
        );
        assert_eq!(
            restored
                .sampling
                .as_ref()
                .unwrap()
                .runnable_count(first.check.sampling_identity()),
            first.check.descriptor_count() - 1
        );
        let counts = reopened.lock().query_row(
            "SELECT eligible_targets, reviewed_targets, runnable_targets FROM burn_check_assessment WHERE check_id = ?1",
            [super::SKILL_OPPORTUNITIES_CHECK_ID], |row| Ok((row.get::<_, usize>(0)?, row.get::<_, usize>(1)?, row.get::<_, usize>(2)?))).unwrap();
        assert_eq!(
            counts,
            (
                first.check.descriptor_count(),
                1,
                first.check.descriptor_count() - 1
            )
        );
        assert!(
            store
                .queue_burn_check_assessment(&first.durable, 1002, 180)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(&first.durable, 1002, 300, 180)
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
        .set_check_enabled(antiburn_local::checks::DetectorId::SkillOpportunities, true)
        .unwrap();
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
            .sampling
            .is_none()
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
fn durable_cursor_survives_process_generation_and_reopens_changed_inputs() {
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
    assert_eq!(
        super::restore_cursor(Some(&saved), &input.input_revision, 8).active_job,
        Some(job.clone())
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
