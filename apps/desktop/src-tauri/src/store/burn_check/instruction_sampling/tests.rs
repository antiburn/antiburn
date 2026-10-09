use super::*;

fn fixture() -> (Store, BurnCheckInput) {
    let store =
        Store::open_in_memory(std::path::Path::new("/synthetic/instruction-sampling")).unwrap();
    let record = SessionRecord {
        key: SessionKey::new("native", "claude-code", "instruction-sampling"),
        source_kind: "file".into(),
        source_label: "/synthetic/session.jsonl".into(),
        wsl_distro: None,
        title: None,
        title_source: None,
        cwd: None,
        surface: "cli".into(),
        updated_at_epoch: Some(100),
        activity_cursor: "activity".into(),
        activity_source: "mtime".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: None,
    };
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    store
        .lock()
        .execute("UPDATE session SET source_generation = 2", [])
        .unwrap();
    store.lock().execute("UPDATE session_evidence SET status = 'ready', analyzed_generation = 2, published_fence = 3", []).unwrap();
    store.lock().execute("UPDATE session_evidence SET parser_revision = ?1, analyzer_revision = ?2, evidence_schema_revision = ?3", rusqlite::params![antiburn_local::analysis::PARSER_REVISION, antiburn_local::analysis::ANALYZER_REVISION, antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION]).unwrap();
    store.lock().execute(
        "INSERT INTO burn_check_assessment (environment_key, agent, session_id, check_id, incarnation, source_generation, published_fence, input_revision, result_revision, result_json, status, created_at_epoch, updated_at_epoch)
         VALUES (?1, ?2, ?3, 'ignored_instructions', 1, 2, 3, 'revision', 'revision', '{}', 'completed', 10, 10)",
        rusqlite::params![record.key.environment_key, record.key.agent, record.key.session_id],
    ).unwrap();
    (
        store,
        BurnCheckInput {
            key: record.key,
            check_id: "ignored_instructions".into(),
            incarnation: 1,
            source_generation: 2,
            source_fingerprint: None,
            activity_cursor: "activity".into(),
            published_fence: 3,
            input_revision: "revision".into(),
            evaluator_revision: "current".into(),
            boundary_at_epoch: 0,
        },
    )
}

fn pair(id: usize, variant: usize, incarnation: u64) -> BurnCheckSampledPair {
    BurnCheckSampledPair {
        comparison_id: format!("pair-{id}"),
        dependency_digest: format!("dependency-{variant}-{incarnation}"),
        incarnation,
        action_id: format!("action-{id}"),
        action_digest: "action-digest".into(),
        instruction_digest: "instruction-digest".into(),
        selector_revision: 2,
        round: (variant % 5) as u32,
        assessed: variant.is_multiple_of(2),
    }
}

#[test]
fn compact_reads_keep_exact_older_variants_without_loading_each_variant() {
    let (store, input) = fixture();
    let variants = (0..2000)
        .map(|variant| pair(variant % 20, variant, u64::from(variant < 1000)))
        .collect::<Vec<_>>();
    store
        .save_burn_check_sampled_pairs(&input, &variants)
        .unwrap();
    let (round, rows) = store.instruction_sampled_pairs(&input.key, 1).unwrap();
    assert_eq!(round, 4);
    assert_eq!(rows.len(), 20);
    assert!(rows.iter().all(|pair| pair.incarnation == 1));
    let older = pair(0, 0, 1);
    let dependencies = vec![(
        older.comparison_id.clone(),
        older.dependency_digest.clone(),
        "encoded-current".into(),
    )];
    assert_eq!(
        store
            .matching_instruction_sampled_pairs(&input.key, 1, 5, false, &dependencies)
            .unwrap(),
        vec![older]
    );
    assert!(
        store
            .matching_instruction_sampled_pairs(&input.key, 2, 5, false, &dependencies)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .matching_instruction_sampled_pairs(&input.key, 1, 0, false, &dependencies)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .matching_instruction_sampled_pairs(&input.key, 1, 0, true, &dependencies)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn replacement_prunes_only_obsolete_variants_after_a_fenced_publication() {
    let (store, input) = fixture();
    let variants = (0..200)
        .flat_map(|variant| {
            [
                pair(0, variant, 1),
                pair(1, variant, 1),
                pair(0, variant, 0),
            ]
        })
        .collect::<Vec<_>>();
    store
        .save_burn_check_sampled_pairs(&input, &variants)
        .unwrap();
    let before = store
        .burn_check_sampled_pairs(&input.key, &input.check_id)
        .unwrap();
    let invalid = BurnCheckSampledPair {
        dependency_digest: "{invalid".into(),
        ..pair(0, 300, 1)
    };
    assert!(
        store
            .save_instruction_sampled_pairs(&input, &[invalid])
            .is_err()
    );
    assert_eq!(
        store
            .burn_check_sampled_pairs(&input.key, &input.check_id)
            .unwrap()
            .len(),
        before.len()
    );
    let mut current = pair(0, 300, 1);
    current.assessed = true;
    let mut stale = input.clone();
    stale.published_fence += 1;
    assert!(
        !store
            .save_instruction_sampled_pairs(&stale, std::slice::from_ref(&current))
            .unwrap()
    );
    assert_eq!(
        store
            .burn_check_sampled_pairs(&input.key, &input.check_id)
            .unwrap()
            .len(),
        before.len()
    );
    assert!(
        store
            .save_instruction_sampled_pairs(&input, std::slice::from_ref(&current))
            .unwrap()
    );
    let after = store
        .burn_check_sampled_pairs(&input.key, &input.check_id)
        .unwrap();
    assert_eq!(after.len(), 201);
    assert_eq!(
        after
            .iter()
            .filter(|pair| pair.comparison_id == current.comparison_id)
            .collect::<Vec<_>>(),
        vec![&current]
    );
    assert!(after.iter().all(|pair| pair.incarnation == 1));
    for variant in 301..401 {
        store
            .save_instruction_sampled_pairs(&input, &[pair(0, variant, 1)])
            .unwrap();
    }
    assert_eq!(
        store
            .burn_check_sampled_pairs(&input.key, &input.check_id)
            .unwrap()
            .len(),
        201
    );
    let mut wrong_incarnation = current;
    wrong_incarnation.incarnation = 2;
    assert!(
        store
            .save_instruction_sampled_pairs(&input, &[wrong_incarnation])
            .is_err()
    );
}

#[test]
fn source_changes_keep_paid_answers_without_pruning_another_context() {
    let (store, input) = fixture();
    let older = pair(0, 0, 1);
    store
        .save_burn_check_sampled_pairs(&input, std::slice::from_ref(&older))
        .unwrap();
    store
        .lock()
        .execute("UPDATE session SET source_generation = 3", [])
        .unwrap();
    let current = pair(0, 1, 1);
    assert!(
        store
            .save_instruction_sampled_pairs(&input, &[current])
            .unwrap()
    );
    let rows = store
        .burn_check_sampled_pairs(&input.key, &input.check_id)
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.contains(&older));
}

#[test]
fn coordinate_upgrade_keeps_the_exact_legacy_answer_and_its_review_round() {
    let (store, input) = fixture();
    let old = pair(0, 0, 1);
    store
        .save_burn_check_sampled_pairs(&input, std::slice::from_ref(&old))
        .unwrap();
    let encoded =
        serde_json::json!({"digest": old.dependency_digest, "coordinate": {"rule_id": "rule"}})
            .to_string();
    let current = BurnCheckSampledPair {
        dependency_digest: encoded.clone(),
        round: 4,
        assessed: false,
        ..old.clone()
    };
    store
        .save_instruction_sampled_pairs(&input, &[current])
        .unwrap();
    let rows = store
        .burn_check_sampled_pairs(&input.key, &input.check_id)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].assessed);
    assert_eq!(rows[0].round, old.round);
    assert_eq!(
        store
            .matching_instruction_sampled_pairs(
                &input.key,
                1,
                4,
                false,
                &[(old.comparison_id, old.dependency_digest, encoded)]
            )
            .unwrap(),
        rows
    );
}

#[test]
fn validated_reuse_prunes_variants_without_another_provider_result() {
    let (store, input) = fixture();
    let variants = (0..200)
        .flat_map(|variant| {
            [
                pair(0, variant, 1),
                pair(1, variant, 1),
                pair(0, variant, 0),
            ]
        })
        .collect::<Vec<_>>();
    store
        .save_burn_check_sampled_pairs(&input, &variants)
        .unwrap();
    let current = pair(0, 0, 1);
    let dependencies = vec![(
        current.comparison_id.clone(),
        current.dependency_digest.clone(),
        "encoded-current".into(),
    )];
    assert!(
        store
            .prune_verified_instruction_pairs(&input, std::slice::from_ref(&current), &dependencies)
            .unwrap()
    );
    let remaining = store
        .burn_check_sampled_pairs(&input.key, &input.check_id)
        .unwrap();
    assert_eq!(remaining.len(), 201);
    assert!(remaining.contains(&current));
    assert!(remaining.iter().all(|pair| pair.incarnation == 1));
    store
        .lock()
        .execute("UPDATE session SET source_generation = 3", [])
        .unwrap();
    let another = pair(1, 0, 1);
    let dependencies = vec![(
        another.comparison_id.clone(),
        another.dependency_digest.clone(),
        "encoded-another".into(),
    )];
    assert!(
        !store
            .prune_verified_instruction_pairs(&input, &[another], &dependencies)
            .unwrap()
    );
    assert_eq!(
        store
            .burn_check_sampled_pairs(&input.key, &input.check_id)
            .unwrap()
            .len(),
        201
    );
}
