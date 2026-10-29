use super::*;
use crate::context::{CurrentInventories, InventorySnapshot, SkillRecord};

fn inventory(revision: &str, has_skill: bool) -> CurrentInventories {
    CurrentInventories {
        global: InventorySnapshot {
            skills: Some(if has_skill {
                vec![SkillRecord {
                    name: revision.into(),
                    description: String::new(),
                    path: "/synthetic/SKILL.md".into(),
                }]
            } else {
                vec![]
            }),
            skills_limits: vec!["partial-supported-sources".into()],
            snapshot_revision: Some(revision.into()),
            ..InventorySnapshot::default()
        },
        repo: InventorySnapshot {
            skills_limits: vec!["repository-root-unavailable".into()],
            ..InventorySnapshot::default()
        },
    }
}

fn inventory_activation(condition: &str, minimum: u8) -> EffectiveActivation {
    EffectiveActivation {
        compiled: Arc::new(compile(&format!("name: Inventory\nif: {condition}\non:\n  tool_result:\n  window: {{size: 3, min: {minimum}}}\nactions:\n  - type: status\n    input:\n      result: failure\n      message: \"${{{{ global.skills[0].name }}}}\"\n")).unwrap()),
        ..activation()
    }
}

#[test]
fn inventory_freeze_condition_runs_before_window_and_reuses_capture_for_candidate() {
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let activation = inventory_activation("global.skills.length > 0", 2);
    let mut calls = 0;
    let mut capture = |_: PublicationCursor, _: &Observation| {
        calls += 1;
        Ok(Some(inventory("current", calls > 1)))
    };
    for (cursor, id) in [(1, "a"), (2, "b"), (3, "c")] {
        engine
            .apply_source_page_with_inventories(&activation, &source_page(cursor, id), &mut capture)
            .unwrap();
    }
    assert_eq!(calls, 3);
    let candidates = store.stored_candidates.lock().unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0]
            .messages
            .iter()
            .map(|value| value.id.as_str())
            .collect::<Vec<_>>(),
        ["b", "c"]
    );
    assert_eq!(
        candidates[0].context.value["global"]["skills_complete"],
        false
    );
    assert!(candidates[0].context.value["repo"].get("skills").is_none());
}

#[test]
fn inventory_freeze_action_demand_waits_for_candidate_and_keeps_native_observations() {
    let activation = inventory_activation("true", 2);
    let mut fingerprints = Vec::new();
    for revision in ["first", "second"] {
        let store = Arc::new(FakeStore::default());
        let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
        let mut calls = 0;
        for (cursor, id) in [(1, "a"), (2, "b")] {
            let mut page = source_page(cursor, id);
            if let SourceEvent::Observation { observation, .. } = &mut page.events[0] {
                observation.context.value["repo"] = json!({"root": "/synthetic/repo"});
            }
            engine
                .apply_source_page_with_inventories(&activation, &page, &mut |_, _| {
                    calls += 1;
                    Ok(Some(inventory(revision, true)))
                })
                .unwrap();
            assert_eq!(
                store.applied_events.lock().unwrap().last(),
                page.events.last()
            );
        }
        assert_eq!(calls, 1);
        let candidates = store.stored_candidates.lock().unwrap();
        let candidate = &candidates[0];
        assert_eq!(candidate.context.value["repo"]["root"], "/synthetic/repo");
        assert_eq!(
            candidate.context.value["global"]["snapshot_revision"],
            revision
        );
        assert_eq!(
            candidate
                .dependencies
                .iter()
                .filter(|value| matches!(value, EvidenceDependency::Message { .. }))
                .count(),
            2
        );
        for observation in store.committed.lock().unwrap().iter() {
            assert!(
                observation
                    .observation
                    .context
                    .value
                    .get("global")
                    .is_none()
            );
            assert!(observation.observation.version.0.starts_with("version-"));
        }
        let restored: FrozenCandidate =
            serde_json::from_str(&serde_json::to_string(candidate).unwrap()).unwrap();
        assert_eq!(&restored, candidate);
        fingerprints.push(candidate.fingerprint);
    }
    assert_ne!(fingerprints[0], fingerprints[1]);
}

#[test]
fn inventory_freeze_skips_inventory_free_corrections_filtered_and_duplicate_results() {
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let activation = activation();
    for page in [
        source_page(1, "a"),
        revised_source_page(2, "a", 2),
        revised_source_page(3, "a", 2),
        source_page(4, "b"),
    ] {
        engine
            .apply_source_page_with_inventories(&activation, &page, &mut |_, _| {
                panic!("inventory-free checks must not collect")
            })
            .unwrap();
    }
    let mut rejected = inventory_activation("false", 1);
    rejected.id.definition_id = "rejected".into();
    let engine = CheckEngine::new(Arc::new(FakeStore::default()), Arc::new(FakeEvaluator));
    engine
        .apply_source_page_with_inventories(&rejected, &source_page(1, "a"), &mut |_, _| {
            panic!("rejected triggers must not collect")
        })
        .unwrap();
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store, Arc::new(FakeEvaluator));
    let mut filtered = inventory_activation("global.skills.length > 0", 1);
    Arc::make_mut(&mut filtered.compiled).trigger.statuses =
        Some(vec![crate::compiler::ToolResultStatus::Completed]);
    engine
        .apply_source_page_with_inventories(&filtered, &source_page(1, "a"), &mut |_, _| {
            panic!("filtered triggers must not collect")
        })
        .unwrap();
}

#[test]
fn inventory_freeze_capture_and_commit_errors_leave_no_durable_snapshot() {
    let store = Arc::new(FakeStore::default());
    let activation = inventory_activation("global.skills.length > 0", 1);
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let page = source_page(1, "a");
    let mut calls = 0;
    assert!(
        engine
            .apply_source_page_with_inventories(&activation, &page, &mut |_, _| {
                calls += 1;
                Err(StoreError {
                    message: "capture failed".into(),
                })
            })
            .is_err()
    );
    assert_eq!(*store.progress.lock().unwrap(), SourceProgress::default());
    *store.fail_next_page.lock().unwrap() = true;
    assert!(
        engine
            .apply_source_page_with_inventories(&activation, &page, &mut |_, _| {
                calls += 1;
                Ok(Some(inventory("uncommitted", true)))
            })
            .is_err()
    );
    assert_eq!(*store.progress.lock().unwrap(), SourceProgress::default());
    assert!(store.stored_candidates.lock().unwrap().is_empty());
    drop(engine);
    let restarted = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    restarted
        .apply_source_page_with_inventories(&activation, &page, &mut |_, _| {
            calls += 1;
            Ok(Some(inventory("committed", true)))
        })
        .unwrap();
    assert_eq!(calls, 3);
    assert_eq!(
        store.stored_candidates.lock().unwrap()[0].context.value["global"]["snapshot_revision"],
        "committed"
    );
    assert_eq!(store.applied_events.lock().unwrap().as_slice(), page.events);
}

#[test]
fn inventory_freeze_native_correction_reads_only_condition_demand() {
    for condition in ["true", "global.skills.length > 0"] {
        let activation = inventory_activation(condition, 1);
        let store = Arc::new(FakeStore::default());
        let engine = CheckEngine::new(store, Arc::new(FakeEvaluator));
        let mut calls = 0;
        let mut capture = |_: PublicationCursor, _: &Observation| {
            calls += 1;
            Ok(Some(inventory("current", true)))
        };
        engine
            .apply_source_page_with_inventories(&activation, &source_page(1, "a"), &mut capture)
            .unwrap();
        engine
            .apply_source_page_with_inventories(
                &activation,
                &revised_source_page(2, "a", 2),
                &mut capture,
            )
            .unwrap();
        assert_eq!(calls, if condition == "true" { 1 } else { 2 });
    }
}
