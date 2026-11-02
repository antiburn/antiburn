use super::*;
use crate::analytics::event::{ModelCheckLifecycle, ProviderSetupOutcome};
use native::operation::{OperationKind, OperationOutcome, retry_recording, settings_snapshot};

const SAVE_REQUEST: &str = "10000000-0000-4000-8000-000000000001";
const OTHER_REQUEST: &str = "20000000-0000-4000-8000-000000000002";

fn selected(store: &Store, worker: &WorkerHandle, vault: &ProfileVault) {
    native::lifecycle::save_connection_recording(
        store,
        worker,
        draft("selected", custom_connection(), Some("old-secret")),
        SaveConnectionIntent::Create,
        super::super::REQUEST_ID,
        vault,
        |_| {},
    )
    .unwrap();
    enable(store, worker, vault);
}

fn protected_work(store: &Store) -> Vec<String> {
    insert_unfinished_assessment(store, "dispatching");
    insert_accepted_status_and_source(store);
    store.lock().execute_batch("UPDATE burn_check_candidate SET preparation_json = '{\"frozen\":true}';
        INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, decision_name, attempt_number, state, outcome, updated_at_epoch)
        VALUES ('unknown-attempt', 'synthetic-candidate', 'review', 1, 'settled', 'unknown', 1);
        INSERT INTO burn_check_decision_usage (attempt_id, usage_known) VALUES ('unknown-attempt', 0);").unwrap();
    protected_snapshot(store)
}

fn protected_snapshot(store: &Store) -> Vec<String> {
    let mut result = source_and_status_snapshot(store);
    result.extend(table_snapshot(
        store,
        &["burn_check_evaluation_attempt", "burn_check_decision_usage"],
    ));
    result.push(store.lock().query_row("SELECT preparation_json FROM burn_check_candidate WHERE candidate_id = 'synthetic-candidate'", [], |row| row.get(0)).unwrap());
    result.push(store.lock().query_row("SELECT json_array(outcome, actions_json, evidence_valid) FROM burn_check_assessment_history WHERE assessment_id = 'synthetic-assessment'", [], |row| row.get(0)).unwrap());
    result
}

fn fault(store: &Store, vault: &ProfileVault, name: &str) {
    match name {
        "write" => vault.reject_write.set(true),
        "partial_write" => { vault.reject_write.set(true); vault.partial_write.set(true); }
        "old_delete" => vault.reject_delete.set(true),
        "runtime_read" => vault.reject_read.set(true),
        "cancel" => store.lock().execute_batch("CREATE TEMP TRIGGER recovery_fault BEFORE UPDATE ON burn_check_candidate WHEN NEW.state = 'cancelled' BEGIN SELECT RAISE(ABORT, 'synthetic cancellation failure'); END;").unwrap(),
        "commit" => store.lock().execute_batch("CREATE TEMP TRIGGER recovery_fault BEFORE UPDATE ON setting WHEN NEW.key = 'internal:smartChecksConnectionsV1' BEGIN SELECT RAISE(ABORT, 'synthetic commit failure'); END;").unwrap(),
        "selection" => store.lock().execute_batch("CREATE TEMP TRIGGER recovery_fault BEFORE UPDATE ON setting WHEN NEW.key = 'internal:smartChecksActiveConnectionV1' BEGIN SELECT RAISE(ABORT, 'synthetic selection failure'); END;").unwrap(),
        "final_clear" => store.lock().execute_batch("CREATE TEMP TRIGGER recovery_fault BEFORE UPDATE ON setting WHEN NEW.key = 'internal:smartChecksProfileOperationV1' AND NEW.value = '' BEGIN SELECT RAISE(ABORT, 'synthetic clear failure'); END;").unwrap(),
        _ => panic!("Unknown synthetic fault"),
    }
}

fn clear_fault(store: &Store, vault: &ProfileVault) {
    vault.reject_write.set(false);
    vault.partial_write.set(false);
    vault.reject_delete.set(false);
    vault.reject_read.set(false);
    store
        .lock()
        .execute_batch("DROP TRIGGER IF EXISTS recovery_fault")
        .unwrap();
}

#[test]
fn original_save_request_retries_each_precommit_and_committed_fault_after_reopen() {
    for name in [
        "cancel",
        "write",
        "partial_write",
        "commit",
        "old_delete",
        "runtime_read",
        "final_clear",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let worker = WorkerHandle::default();
        let vault = ProfileVault::default();
        selected(&store, &worker, &vault);
        let protected = protected_work(&store);
        let original = settings_snapshot(&store).unwrap().profiles["selected"].clone();
        let generation = worker.execution_snapshot().unwrap().generation;
        let mut edited = original.clone();
        edited.model = "edited-model".into();
        let request = draft("selected", edited, Some("new-secret"));
        fault(&store, &vault, name);
        let mut events = vec![];
        assert!(
            native::lifecycle::save_connection_recording(
                &store,
                &worker,
                request.clone(),
                SaveConnectionIntent::Update,
                SAVE_REQUEST,
                &vault,
                |event| events.push(event)
            )
            .is_err(),
            "{name}"
        );
        assert!(events.is_empty());
        assert!(worker.execution_snapshot().is_none(), "{name}");
        assert!(worker.with_current_generation(generation, || ()).is_none());
        let reads = vault.reads.borrow().len();
        let writes = vault.writes.borrow().len();
        let deletes = vault.deletes.borrow().len();
        let pending = settings_snapshot(&store)
            .unwrap()
            .pending_operation
            .unwrap();
        assert_eq!(pending.kind, OperationKind::Save);
        assert_eq!(pending.request_id.as_deref(), Some(SAVE_REQUEST));
        assert_ne!(pending.id, "selected");
        let second_read = settings_snapshot(&store).unwrap();
        assert_eq!(second_read.pending_operation.unwrap().id, pending.id);
        assert_eq!(
            (
                vault.reads.borrow().len(),
                vault.writes.borrow().len(),
                vault.deletes.borrow().len()
            ),
            (reads, writes, deletes)
        );
        let descriptor = serde_json::to_value(&pending).unwrap();
        assert_eq!(descriptor.as_object().unwrap().len(), 4);
        assert!(!descriptor.to_string().contains("secret"));
        assert!(
            native::lifecycle::save_connection_recording(
                &store,
                &worker,
                request.clone(),
                SaveConnectionIntent::Update,
                OTHER_REQUEST,
                &vault,
                |event| events.push(event)
            )
            .is_err()
        );
        assert!(
            retry_recording(&store, &worker, "operation-stale", &vault, |event| events
                .push(event))
            .is_err()
        );
        assert_eq!(
            settings_snapshot(&store)
                .unwrap()
                .pending_operation
                .unwrap()
                .request_id
                .as_deref(),
            Some(SAVE_REQUEST)
        );
        assert!(events.is_empty());
        clear_fault(&store, &vault);
        drop(store);
        let reopened = Store::open(directory.path()).unwrap();
        let restarted = WorkerHandle::default();
        assert_eq!(
            settings_snapshot(&reopened)
                .unwrap()
                .pending_operation
                .unwrap()
                .id,
            pending.id
        );
        let result = retry_recording(&reopened, &restarted, &pending.id, &vault, |event| {
            events.push(event)
        })
        .unwrap();
        let committed = matches!(name, "old_delete" | "runtime_read" | "final_clear");
        assert_eq!(
            result.outcome,
            if committed {
                OperationOutcome::Completed
            } else {
                OperationOutcome::RolledBack
            }
        );
        assert_eq!(
            result.settings.profiles["selected"].revision,
            if committed { 2 } else { 1 }
        );
        assert!(result.settings.pending_operation.is_none());
        assert_eq!(
            restarted
                .execution_snapshot()
                .unwrap()
                .credential
                .as_deref(),
            Some(if committed {
                "new-secret"
            } else {
                "old-secret"
            })
        );
        assert_eq!(events.len(), usize::from(committed));
        if committed {
            assert!(matches!(
                events[0],
                ModelCheckLifecycle::ProviderSetup {
                    outcome: ProviderSetupOutcome::Saved,
                    ..
                }
            ));
        }
        assert!(
            retry_recording(&reopened, &restarted, &pending.id, &vault, |event| events
                .push(event))
            .is_err()
        );
        assert_eq!(events.len(), usize::from(committed));
        assert_eq!(protected_snapshot(&reopened), protected);
        assert_eq!(unfinished_assessment_state(&reopened), "cancelled");
        assert_eq!(vault.values.borrow().len(), 1);
    }
}

#[test]
fn committed_no_secret_edit_uses_token_retry_not_a_rebased_editor_or_same_id_use() {
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    selected(&store, &worker, &vault);
    let mut connection = settings_snapshot(&store).unwrap().profiles["selected"].clone();
    connection.model = "edited-model".into();
    let original_request = draft("selected", connection, None);
    fault(&store, &vault, "final_clear");
    let mut events = vec![];
    assert!(
        native::lifecycle::save_connection_recording(
            &store,
            &worker,
            original_request.clone(),
            SaveConnectionIntent::Update,
            SAVE_REQUEST,
            &vault,
            |event| events.push(event)
        )
        .is_err()
    );
    let snapshot = settings_snapshot(&store).unwrap();
    assert_eq!(snapshot.profiles["selected"].revision, 2);
    let token = snapshot.pending_operation.unwrap().id;
    assert!(
        native::lifecycle::save_connection_recording(
            &store,
            &worker,
            original_request,
            SaveConnectionIntent::Update,
            SAVE_REQUEST,
            &vault,
            |_| {}
        )
        .is_err()
    );
    assert!(
        native::lifecycle::save_connection_recording(
            &store,
            &worker,
            draft("selected", snapshot.profiles["selected"].clone(), None),
            SaveConnectionIntent::Update,
            OTHER_REQUEST,
            &vault,
            |_| {}
        )
        .is_err()
    );
    assert!(
        native::lifecycle::switch_connection_recording(&store, &worker, "selected", &vault, |_| {})
            .is_err()
    );
    assert!(worker.execution_snapshot().is_none());
    clear_fault(&store, &vault);
    let recovered =
        retry_recording(&store, &worker, &token, &vault, |event| events.push(event)).unwrap();
    assert_eq!(recovered.outcome, OperationOutcome::Completed);
    assert_eq!(worker.execution_snapshot().unwrap().connection.revision, 2);
    assert_eq!(events.len(), 1);
    let generation = worker.execution_snapshot().unwrap().generation;
    worker.reject_authentication(&store, generation).unwrap();
    native::lifecycle::save_connection_recording(
        &store,
        &worker,
        draft(
            "selected",
            recovered.settings.profiles["selected"].clone(),
            None,
        ),
        SaveConnectionIntent::Update,
        OTHER_REQUEST,
        &vault,
        |event| events.push(event),
    )
    .unwrap();
    native::lifecycle::switch_connection_recording(&store, &worker, "selected", &vault, |event| {
        events.push(event)
    })
    .unwrap();
    assert_eq!(events.len(), 1);
    assert!(worker.authentication_rejected());
    assert_eq!(
        store.internal_value(native::AUTH_REJECTED_KEY).as_deref(),
        Some("true")
    );
}

#[test]
fn retry_honors_current_off_and_rollback_authentication_instead_of_saved_enablement() {
    for committed in [false, true] {
        for off in [false, true] {
            let store = store();
            let worker = WorkerHandle::default();
            let vault = ProfileVault::default();
            selected(&store, &worker, &vault);
            if !committed {
                let generation = worker.execution_snapshot().unwrap().generation;
                worker.reject_authentication(&store, generation).unwrap();
            }
            let connection = settings_snapshot(&store).unwrap().profiles["selected"].clone();
            fault(
                &store,
                &vault,
                if committed {
                    "final_clear"
                } else {
                    "partial_write"
                },
            );
            assert!(
                native::lifecycle::save_connection_recording(
                    &store,
                    &worker,
                    draft("selected", connection, Some("new-secret")),
                    SaveConnectionIntent::Update,
                    SAVE_REQUEST,
                    &vault,
                    |_| {}
                )
                .is_err()
            );
            let token = settings_snapshot(&store)
                .unwrap()
                .pending_operation
                .unwrap()
                .id;
            if off {
                native::pause_model_checks(&store, &worker).unwrap();
            }
            clear_fault(&store, &vault);
            let reads = vault.reads.borrow().len();
            let result = retry_recording(&store, &worker, &token, &vault, |_| {}).unwrap();
            assert_eq!(
                result.outcome,
                if committed {
                    OperationOutcome::Completed
                } else {
                    OperationOutcome::RolledBack
                }
            );
            assert_eq!(native::yaml_discovery_enabled(&store), !off);
            assert_eq!(worker.is_available(), committed && !off);
            if off || !committed {
                assert_eq!(vault.reads.borrow().len(), reads);
            }
            assert_eq!(
                store.internal_value(native::AUTH_REJECTED_KEY).as_deref(),
                Some(if committed { "false" } else { "true" })
            );
        }
    }
}

#[test]
fn use_faults_have_live_token_recovery_and_no_duplicate_switch_event() {
    for name in ["cancel", "commit", "selection", "final_clear"] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let worker = WorkerHandle::default();
        let vault = ProfileVault::default();
        selected(&store, &worker, &vault);
        create(&store, &worker, "target", &vault);
        let protected = protected_work(&store);
        fault(&store, &vault, name);
        let mut events = vec![];
        assert!(
            native::lifecycle::switch_connection_recording(
                &store,
                &worker,
                "target",
                &vault,
                |event| events.push(event)
            )
            .is_err()
        );
        let pending = settings_snapshot(&store)
            .unwrap()
            .pending_operation
            .unwrap();
        assert_eq!(pending.kind, OperationKind::Switch);
        assert!(pending.request_id.is_none());
        assert!(worker.execution_snapshot().is_none());
        clear_fault(&store, &vault);
        drop(store);
        let reopened = Store::open(directory.path()).unwrap();
        let restarted = WorkerHandle::default();
        let result = retry_recording(&reopened, &restarted, &pending.id, &vault, |event| {
            events.push(event)
        })
        .unwrap();
        let completed = name == "final_clear";
        assert_eq!(
            result.outcome,
            if completed {
                OperationOutcome::Completed
            } else {
                OperationOutcome::RolledBack
            }
        );
        assert_eq!(
            result.settings.active_id.as_deref(),
            Some(if completed { "target" } else { "selected" })
        );
        assert!(restarted.is_available());
        assert_eq!(events.len(), usize::from(completed));
        if completed {
            assert!(matches!(
                events[0],
                ModelCheckLifecycle::ProviderSetup {
                    outcome: ProviderSetupOutcome::Switched,
                    ..
                }
            ));
        }
        assert!(
            retry_recording(&reopened, &restarted, &pending.id, &vault, |event| events
                .push(event))
            .is_err()
        );
        assert_eq!(protected_snapshot(&reopened), protected);
    }
}

#[test]
fn removed_row_retry_survives_snapshot_remount_and_stale_tokens_do_not_finish_next_operation() {
    for active in [false, true] {
        let store = store();
        let worker = WorkerHandle::default();
        let vault = ProfileVault::default();
        selected(&store, &worker, &vault);
        native::lifecycle::save_connection_recording(
            &store,
            &worker,
            draft("inactive", custom_connection(), Some("inactive-secret")),
            SaveConnectionIntent::Create,
            OTHER_REQUEST,
            &vault,
            |_| {},
        )
        .unwrap();
        let generation = worker.execution_snapshot().unwrap().generation;
        let id = if active { "selected" } else { "inactive" };
        vault.reject_delete.set(true);
        let mut events = vec![];
        assert!(
            native::lifecycle::remove_connection_recording(
                &store,
                &worker,
                id,
                1,
                &vault,
                |event| events.push(event)
            )
            .is_err()
        );
        let snapshot = settings_snapshot(&store).unwrap();
        assert!(!snapshot.profiles.contains_key(id));
        let pending = snapshot.pending_operation.unwrap();
        assert_eq!(pending.kind, OperationKind::Remove);
        assert_eq!(pending.connection_id, id);
        assert!(pending.request_id.is_none());
        if !active {
            assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
        }
        let remounted = settings_snapshot(&store).unwrap();
        assert_eq!(remounted.pending_operation.unwrap().id, pending.id);
        vault.reject_delete.set(false);
        let result = retry_recording(&store, &worker, &pending.id, &vault, |event| {
            events.push(event)
        })
        .unwrap();
        assert_eq!(result.outcome, OperationOutcome::Completed);
        assert_eq!(events.len(), if active { 2 } else { 1 });
        assert!(matches!(
            events[0],
            ModelCheckLifecycle::ProviderSetup {
                outcome: ProviderSetupOutcome::ConnectionRemoved,
                ..
            }
        ));
        if active {
            assert!(matches!(
                events[1],
                ModelCheckLifecycle::Enablement { enabled: false }
            ));
        }
        let survivor = if active { "inactive" } else { "selected" };
        fault(&store, &vault, "final_clear");
        let connection = settings_snapshot(&store).unwrap().profiles[survivor].clone();
        assert!(
            native::lifecycle::save_connection_recording(
                &store,
                &worker,
                draft(survivor, connection, Some("next-secret")),
                SaveConnectionIntent::Update,
                SAVE_REQUEST,
                &vault,
                |_| {}
            )
            .is_err()
        );
        let next = settings_snapshot(&store)
            .unwrap()
            .pending_operation
            .unwrap();
        assert_ne!(next.id, pending.id);
        assert!(
            retry_recording(&store, &worker, &pending.id, &vault, |event| events
                .push(event))
            .is_err()
        );
        assert_eq!(
            settings_snapshot(&store)
                .unwrap()
                .pending_operation
                .unwrap()
                .id,
            next.id
        );
        assert_eq!(events.len(), if active { 2 } else { 1 });
    }
}

#[test]
fn legacy_marker_retry_uses_only_valid_current_selection_and_stays_paused() {
    for saved_selection in [Some("selected"), Some("target"), None] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let worker = WorkerHandle::default();
        let vault = ProfileVault::default();
        selected(&store, &worker, &vault);
        create(&store, &worker, "target", &vault);
        let mut saved = native::profiles(&store).unwrap();
        saved.active_id = saved_selection.map(str::to_owned);
        native::save_profiles(&store, &saved).unwrap();
        store.set_internal_value(native::CONNECTION_PENDING_KEY, "target-may-be-missing");
        let generation = worker.execution_snapshot().unwrap().generation;
        let reads = vault.reads.borrow().len();
        let snapshot = settings_snapshot(&store).unwrap();
        let pending = snapshot.pending_operation.unwrap();
        assert_eq!(
            settings_snapshot(&store)
                .unwrap()
                .pending_operation
                .unwrap()
                .id,
            pending.id
        );
        assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
        assert_eq!(vault.reads.borrow().len(), reads);
        assert_eq!(
            native::profiles(&store).unwrap().active_id.as_deref(),
            saved_selection
        );
        drop(store);
        let reopened = Store::open(directory.path()).unwrap();
        let restarted = WorkerHandle::default();
        vault.reject_read.set(true);
        let mut events = vec![];
        let result = retry_recording(&reopened, &restarted, &pending.id, &vault, |event| {
            events.push(event)
        })
        .unwrap();
        assert_eq!(result.outcome, OperationOutcome::RolledBack);
        assert_eq!(result.settings.active_id.as_deref(), saved_selection);
        assert!(!native::yaml_discovery_enabled(&reopened));
        assert!(!restarted.is_available());
        assert!(events.is_empty());
        assert_eq!(vault.reads.borrow().len(), reads);
        assert_eq!(
            reopened
                .internal_value(native::CONNECTION_PENDING_KEY)
                .as_deref(),
            Some("")
        );
    }
}

#[test]
fn explicit_jev_legacy_reference_is_validated_but_null_draft_never_reads_it() {
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    vault
        .values
        .borrow_mut()
        .insert(None, "retained-secret".into());
    store.set_internal_value(native::SAVED_KEY_KEY, "true");
    native::migrate_provider_state(&store).unwrap();
    native::lifecycle::remove_connection_recording(&store, &worker, "jev", 1, &vault, |_| {})
        .unwrap();
    let mut null_reference = SystemOneConnection::jev_default();
    null_reference.credential = None;
    assert!(
        native::lifecycle::save_connection_recording(
            &store,
            &worker,
            draft("null-key", null_reference, None),
            SaveConnectionIntent::Create,
            SAVE_REQUEST,
            &vault,
            |_| {}
        )
        .is_err()
    );
    assert!(vault.reads.borrow().is_empty());
    let saved = native::lifecycle::save_connection_recording(
        &store,
        &worker,
        draft("explicit-key", SystemOneConnection::jev_default(), None),
        SaveConnectionIntent::Create,
        SAVE_REQUEST,
        &vault,
        |_| {},
    )
    .unwrap();
    assert_eq!(
        saved.profiles["explicit-key"].credential,
        Some(CredentialReference::LegacyTypeSafe)
    );
    assert_eq!(vault.reads.borrow().as_slice(), &[None]);
    assert!(!worker.is_available());
    assert!(!native::yaml_discovery_enabled(&store));
    assert!(vault.deletes.borrow().is_empty());
}

#[test]
fn native_display_numbers_ignore_stale_client_ordinals_and_keep_removed_counters() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    let save = |id: &str, request: &str| {
        native::lifecycle::save_connection_recording(
            &store,
            &worker,
            draft(
                id,
                SystemOneConnection::jev_default(),
                Some("synthetic-key"),
            ),
            SaveConnectionIntent::Create,
            request,
            &vault,
            |_| {},
        )
        .unwrap()
    };
    let first = save("jev-1-00000000-0000-4000-8000-000000000001", SAVE_REQUEST);
    let second_id = "jev-2-00000000-0000-4000-8000-000000000002";
    let third_id = "jev-2-00000000-0000-4000-8000-000000000003";
    let second = save(second_id, SAVE_REQUEST);
    let third = save(third_id, OTHER_REQUEST);
    assert_eq!(
        first.display_numbers[first.active_id.as_deref().unwrap()],
        1
    );
    assert_eq!(second.display_numbers[second_id], 2);
    assert_eq!(third.display_numbers[third_id], 3);
    native::lifecycle::remove_connection_recording(&store, &worker, second_id, 1, &vault, |_| {})
        .unwrap();
    drop(store);
    let reopened = Store::open(directory.path()).unwrap();
    let snapshot = settings_snapshot(&reopened).unwrap();
    assert_eq!(snapshot.display_numbers[third_id], 3);
    let fourth = native::lifecycle::save_connection_recording(
        &reopened,
        &worker,
        draft(
            "jev-2-another-uuid",
            SystemOneConnection::jev_default(),
            Some("synthetic-key"),
        ),
        SaveConnectionIntent::Create,
        OTHER_REQUEST,
        &vault,
        |_| {},
    )
    .unwrap();
    assert_eq!(fourth.display_numbers["jev-2-another-uuid"], 4);
    assert!(
        !serde_json::to_string(&fourth.profiles)
            .unwrap()
            .contains("display")
    );
}

#[test]
fn request_identity_validation_precedes_all_mutation_and_never_grants_retry_authority() {
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    for request in [
        "",
        "not-a-uuid",
        "00000000-0000-0000-0000-000000000000",
        "10000000_0000-4000-8000-000000000001",
    ] {
        assert!(
            native::lifecycle::save_connection_recording(
                &store,
                &worker,
                draft("new", custom_connection(), Some("secret")),
                SaveConnectionIntent::Create,
                request,
                &vault,
                |_| {}
            )
            .is_err()
        );
        assert!(
            store
                .internal_value(native::CONNECTION_PROFILES_KEY)
                .is_none()
        );
    }
    assert!(vault.writes.borrow().is_empty());
    selected(&store, &worker, &vault);
    fault(&store, &vault, "final_clear");
    let connection = settings_snapshot(&store).unwrap().profiles["selected"].clone();
    assert!(
        native::lifecycle::save_connection_recording(
            &store,
            &worker,
            draft("selected", connection, Some("replacement")),
            SaveConnectionIntent::Update,
            SAVE_REQUEST,
            &vault,
            |_| {}
        )
        .is_err()
    );
    let token = settings_snapshot(&store)
        .unwrap()
        .pending_operation
        .unwrap()
        .id;
    clear_fault(&store, &vault);
    assert!(retry_recording(&store, &worker, SAVE_REQUEST, &vault, |_| {}).is_err());
    assert_eq!(
        settings_snapshot(&store)
            .unwrap()
            .pending_operation
            .unwrap()
            .id,
        token
    );
}

#[test]
fn rollback_keeps_its_token_when_old_runtime_credential_cannot_be_restored() {
    for name in ["write", "partial_write", "commit"] {
        let store = store();
        let worker = WorkerHandle::default();
        let vault = ProfileVault::default();
        selected(&store, &worker, &vault);
        let protected = protected_work(&store);
        let connection = settings_snapshot(&store).unwrap().profiles["selected"].clone();
        fault(&store, &vault, name);
        assert!(
            native::lifecycle::save_connection_recording(
                &store,
                &worker,
                draft("selected", connection, Some("new-secret")),
                SaveConnectionIntent::Update,
                SAVE_REQUEST,
                &vault,
                |_| {}
            )
            .is_err()
        );
        let token = settings_snapshot(&store)
            .unwrap()
            .pending_operation
            .unwrap()
            .id;
        clear_fault(&store, &vault);
        vault.reject_read.set(true);
        let mut events = vec![];
        assert!(
            retry_recording(&store, &worker, &token, &vault, |event| events.push(event)).is_err()
        );
        assert_eq!(
            settings_snapshot(&store)
                .unwrap()
                .pending_operation
                .unwrap()
                .id,
            token
        );
        assert!(worker.execution_snapshot().is_none());
        assert!(events.is_empty());
        assert_eq!(vault.values.borrow().len(), 1);
        vault.reject_read.set(false);
        let result =
            retry_recording(&store, &worker, &token, &vault, |event| events.push(event)).unwrap();
        assert_eq!(result.outcome, OperationOutcome::RolledBack);
        assert_eq!(
            worker.execution_snapshot().unwrap().credential.as_deref(),
            Some("old-secret")
        );
        assert!(events.is_empty());
        assert_eq!(protected_snapshot(&store), protected);
    }
}

#[test]
fn startup_settles_saved_metadata_without_success_events_and_inactive_failure_keeps_active_work() {
    for committed in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let worker = WorkerHandle::default();
        let vault = ProfileVault::default();
        selected(&store, &worker, &vault);
        let protected = protected_work(&store);
        let connection = settings_snapshot(&store).unwrap().profiles["selected"].clone();
        fault(
            &store,
            &vault,
            if committed {
                "final_clear"
            } else {
                "partial_write"
            },
        );
        let mut events = vec![];
        assert!(
            native::lifecycle::save_connection_recording(
                &store,
                &worker,
                draft("selected", connection, Some("new-secret")),
                SaveConnectionIntent::Update,
                SAVE_REQUEST,
                &vault,
                |event| events.push(event)
            )
            .is_err()
        );
        clear_fault(&store, &vault);
        drop(store);
        let reopened = Store::open(directory.path()).unwrap();
        let restarted = WorkerHandle::default();
        native::restore_saved_key_with_vault(
            &reopened,
            &restarted,
            || panic!("local profile restore must not read a legacy key"),
            &vault,
        )
        .unwrap();
        assert!(
            settings_snapshot(&reopened)
                .unwrap()
                .pending_operation
                .is_none()
        );
        assert_eq!(
            restarted
                .execution_snapshot()
                .unwrap()
                .credential
                .as_deref(),
            Some(if committed {
                "new-secret"
            } else {
                "old-secret"
            })
        );
        assert!(events.is_empty());
        assert_eq!(protected_snapshot(&reopened), protected);
    }
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    selected(&store, &worker, &vault);
    native::lifecycle::save_connection_recording(
        &store,
        &worker,
        draft("inactive", custom_connection(), Some("inactive-secret")),
        SaveConnectionIntent::Create,
        OTHER_REQUEST,
        &vault,
        |_| {},
    )
    .unwrap();
    vault.reject_delete.set(true);
    assert!(
        native::lifecycle::remove_connection_recording(
            &store,
            &worker,
            "inactive",
            1,
            &vault,
            |_| {}
        )
        .is_err()
    );
    let token = settings_snapshot(&store)
        .unwrap()
        .pending_operation
        .unwrap()
        .id;
    let restarted = WorkerHandle::default();
    native::restore_saved_key_with_vault(
        &store,
        &restarted,
        || panic!("local profile startup reads no legacy key"),
        &vault,
    )
    .unwrap();
    assert_eq!(
        restarted
            .execution_snapshot()
            .unwrap()
            .credential
            .as_deref(),
        Some("old-secret")
    );
    assert_eq!(
        settings_snapshot(&store)
            .unwrap()
            .pending_operation
            .unwrap()
            .id,
        token
    );
}

#[test]
fn previous_full_journal_and_label_migration_keep_bound_snapshots_and_credential_references() {
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    selected(&store, &worker, &vault);
    let before = native::profiles(&store).unwrap();
    let mut after = before.clone();
    let mut added = custom_connection();
    added.model = "future-model".into();
    after.profiles.insert("custom-2-future".into(), added);
    let raw =
        serde_json::json!({"id":"custom-2-future", "revision":0, "before":before, "after":after,
        "new_slot":null, "delete_slot":null, "active_impacted":false, "pause_checks":false,
        "was_enabled":true, "removed_provider":null, "committed":false})
        .to_string();
    store.set_internal_value(native::operation::KEY, &raw);
    let snapshot = settings_snapshot(&store).unwrap();
    let token = snapshot.pending_operation.unwrap().id;
    assert_eq!(snapshot.display_numbers["selected"], 1);
    assert!(!snapshot.profiles.contains_key("custom-2-future"));
    let migrated: serde_json::Value =
        serde_json::from_str(&store.internal_value(native::operation::KEY).unwrap()).unwrap();
    assert_eq!(migrated["before"], serde_json::to_value(&before).unwrap());
    assert_eq!(migrated["after"], serde_json::to_value(&after).unwrap());
    assert_eq!(
        settings_snapshot(&store)
            .unwrap()
            .pending_operation
            .unwrap()
            .id,
        token
    );
    let generation = worker.execution_snapshot().unwrap().generation;
    let mut events = vec![];
    let result =
        retry_recording(&store, &worker, &token, &vault, |event| events.push(event)).unwrap();
    assert_eq!(result.outcome, OperationOutcome::RolledBack);
    assert_eq!(native::profiles(&store).unwrap(), before);
    assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
    assert!(events.is_empty());
}

#[test]
fn legacy_display_migration_preserves_unique_hints_and_fails_number_exhaustion_before_begin() {
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    let mut saved = native::ConnectionProfiles::default();
    for id in [
        "custom",
        "custom-2-first",
        "custom-2-second",
        "custom-8-stable",
    ] {
        saved.profiles.insert(id.into(), custom_connection());
    }
    native::save_profiles(&store, &saved).unwrap();
    let snapshot = settings_snapshot(&store).unwrap();
    assert_eq!(snapshot.display_numbers["custom"], 1);
    assert_eq!(snapshot.display_numbers["custom-2-first"], 2);
    assert_eq!(snapshot.display_numbers["custom-2-second"], 3);
    assert_eq!(snapshot.display_numbers["custom-8-stable"], 8);
    native::lifecycle::remove_connection_recording(&store, &worker, "custom", 1, &vault, |_| {})
        .unwrap();
    assert_eq!(
        settings_snapshot(&store).unwrap().display_numbers["custom-2-second"],
        3
    );
    let mut metadata: serde_json::Value =
        serde_json::from_str(&store.internal_value(native::display_numbers::KEY).unwrap()).unwrap();
    metadata["counters"]["custom"] = serde_json::json!(u32::MAX);
    store.set_internal_value(native::display_numbers::KEY, &metadata.to_string());
    assert!(
        native::lifecycle::save_connection_recording(
            &store,
            &worker,
            draft("new", custom_connection(), None),
            SaveConnectionIntent::Create,
            SAVE_REQUEST,
            &vault,
            |_| {}
        )
        .unwrap_err()
        .contains("number limit")
    );
    assert!(
        settings_snapshot(&store)
            .unwrap()
            .pending_operation
            .is_none()
    );
    assert!(
        !settings_snapshot(&store)
            .unwrap()
            .profiles
            .contains_key("new")
    );
    assert!(vault.writes.borrow().is_empty());
}

#[test]
fn prepared_selected_removal_rollback_records_only_the_saved_pause_once() {
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    selected(&store, &worker, &vault);
    fault(&store, &vault, "commit");
    let mut events = vec![];
    assert!(
        native::lifecycle::remove_connection_recording(
            &store,
            &worker,
            "selected",
            1,
            &vault,
            |event| events.push(event)
        )
        .is_err()
    );
    let token = settings_snapshot(&store)
        .unwrap()
        .pending_operation
        .unwrap()
        .id;
    assert!(events.is_empty());
    clear_fault(&store, &vault);
    let result =
        retry_recording(&store, &worker, &token, &vault, |event| events.push(event)).unwrap();
    assert_eq!(result.outcome, OperationOutcome::RolledBack);
    assert!(result.settings.profiles.contains_key("selected"));
    assert!(matches!(
        &events[..],
        [ModelCheckLifecycle::Enablement { enabled: false }]
    ));
    assert!(!native::yaml_discovery_enabled(&store));
    assert!(!worker.is_available());
    assert!(retry_recording(&store, &worker, &token, &vault, |event| events.push(event)).is_err());
    native::lifecycle::remove_connection_recording(
        &store,
        &worker,
        "selected",
        1,
        &vault,
        |event| events.push(event),
    )
    .unwrap();
    assert!(matches!(
        &events[..],
        [
            ModelCheckLifecycle::Enablement { enabled: false },
            ModelCheckLifecycle::ProviderSetup {
                outcome: ProviderSetupOutcome::ConnectionRemoved,
                ..
            }
        ]
    ));
}

#[test]
fn prepared_save_startup_failure_clears_only_after_matching_live_rollback() {
    for startup_fault in ["old_read", "fresh_cleanup"] {
        for pause_before_retry in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let store = Store::open(directory.path()).unwrap();
            let worker = WorkerHandle::default();
            let vault = ProfileVault::default();
            selected(&store, &worker, &vault);
            let protected = protected_work(&store);
            let original = settings_snapshot(&store).unwrap().profiles["selected"].clone();
            fault(&store, &vault, "partial_write");
            let mut events = vec![];
            assert!(
                native::lifecycle::save_connection_recording(
                    &store,
                    &worker,
                    draft("selected", original.clone(), Some("new-secret")),
                    SaveConnectionIntent::Update,
                    SAVE_REQUEST,
                    &vault,
                    |event| events.push(event)
                )
                .is_err()
            );
            let token = settings_snapshot(&store)
                .unwrap()
                .pending_operation
                .unwrap()
                .id;
            clear_fault(&store, &vault);
            drop(store);
            let reopened = Store::open(directory.path()).unwrap();
            let restarted = WorkerHandle::default();
            if startup_fault == "old_read" {
                vault.reject_read.set(true);
            } else {
                vault.reject_delete.set(true);
            }
            let startup_errors = native::StartupErrors::new();
            let result = native::restore_saved_key_with_origin(
                &reopened,
                &restarted,
                || panic!("local startup recovery reads no legacy key"),
                &vault,
            );
            assert!(result.is_err());
            startup_errors.record_restore_result(&reopened, result);
            assert!(startup_errors.message().is_some());
            assert_eq!(
                settings_snapshot(&reopened)
                    .unwrap()
                    .pending_operation
                    .unwrap()
                    .id,
                token
            );
            if pause_before_retry {
                native::pause_model_checks(&reopened, &restarted).unwrap();
            }
            clear_fault(&reopened, &vault);
            let recovered = native::operation::retry_recording_with_startup_errors(
                &reopened,
                &restarted,
                &token,
                &vault,
                |event| events.push(event),
                &startup_errors,
            )
            .unwrap();
            assert_eq!(recovered.outcome, OperationOutcome::RolledBack);
            assert!(recovered.settings.pending_operation.is_none());
            assert_eq!(recovered.settings.profiles["selected"], original);
            assert_eq!(restarted.is_available(), !pause_before_retry);
            assert_eq!(
                native::yaml_discovery_enabled(&reopened),
                !pause_before_retry
            );
            assert!(startup_errors.message().is_none());
            assert!(
                native::read_check_availability(
                    &reopened,
                    restarted.is_available(),
                    false,
                    startup_errors.message()
                )
                .unwrap()
                .error
                .is_none()
            );
            assert!(events.is_empty());
            assert_eq!(protected_snapshot(&reopened), protected);
        }
    }
}

#[test]
fn legacy_marker_startup_failure_clears_after_its_matching_paused_rollback() {
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    selected(&store, &worker, &vault);
    let protected = protected_work(&store);
    store.set_internal_value(native::CONNECTION_PENDING_KEY, "missing-target");
    fault(&store, &vault, "final_clear");
    let startup_errors = native::StartupErrors::new();
    let result = native::restore_saved_key_with_origin(
        &store,
        &worker,
        || panic!("legacy rollback must not read the key"),
        &vault,
    );
    assert!(result.is_err());
    startup_errors.record_restore_result(&store, result);
    assert!(startup_errors.message().is_some());
    let pending = settings_snapshot(&store)
        .unwrap()
        .pending_operation
        .unwrap();
    assert_eq!(pending.kind, OperationKind::Switch);
    assert!(!native::yaml_discovery_enabled(&store));
    clear_fault(&store, &vault);
    let mut events = vec![];
    let result = native::operation::retry_recording_with_startup_errors(
        &store,
        &worker,
        &pending.id,
        &vault,
        |event| events.push(event),
        &startup_errors,
    )
    .unwrap();
    assert_eq!(result.outcome, OperationOutcome::RolledBack);
    assert!(result.settings.pending_operation.is_none());
    assert_eq!(result.settings.active_id.as_deref(), Some("selected"));
    assert!(!worker.is_available());
    assert!(!native::yaml_discovery_enabled(&store));
    assert!(startup_errors.message().is_none());
    assert!(events.is_empty());
    assert_eq!(protected_snapshot(&store), protected);
}

#[test]
fn other_operation_and_inactive_rollback_preserve_the_original_bound_startup_error() {
    let first = store();
    let first_worker = WorkerHandle::default();
    let first_vault = ProfileVault::default();
    selected(&first, &first_worker, &first_vault);
    let original = settings_snapshot(&first).unwrap().profiles["selected"].clone();
    fault(&first, &first_vault, "partial_write");
    assert!(
        native::lifecycle::save_connection_recording(
            &first,
            &first_worker,
            draft("selected", original, Some("replacement")),
            SaveConnectionIntent::Update,
            SAVE_REQUEST,
            &first_vault,
            |_| {}
        )
        .is_err()
    );
    clear_fault(&first, &first_vault);
    first_vault.reject_delete.set(true);
    let startup_errors = native::StartupErrors::new();
    let result = native::restore_saved_key_with_origin(
        &first,
        &first_worker,
        || panic!("local recovery reads no legacy key"),
        &first_vault,
    );
    assert!(result.is_err());
    startup_errors.record_restore_result(&first, result);
    let original_error = startup_errors.message();
    assert!(original_error.is_some());
    let original_token = settings_snapshot(&first)
        .unwrap()
        .pending_operation
        .unwrap()
        .id;
    for inactive in [false, true] {
        let other = store();
        let worker = WorkerHandle::default();
        let vault = ProfileVault::default();
        selected(&other, &worker, &vault);
        create(&other, &worker, "inactive", &vault);
        let id = if inactive { "inactive" } else { "selected" };
        let connection = settings_snapshot(&other).unwrap().profiles[id].clone();
        let generation = worker.execution_snapshot().unwrap().generation;
        fault(&other, &vault, "partial_write");
        assert!(
            native::lifecycle::save_connection_recording(
                &other,
                &worker,
                draft(id, connection, Some("other-secret")),
                SaveConnectionIntent::Update,
                OTHER_REQUEST,
                &vault,
                |_| {}
            )
            .is_err()
        );
        let token = settings_snapshot(&other)
            .unwrap()
            .pending_operation
            .unwrap()
            .id;
        assert_ne!(token, original_token);
        clear_fault(&other, &vault);
        let mut events = vec![];
        let result = native::operation::retry_recording_with_startup_errors(
            &other,
            &worker,
            &token,
            &vault,
            |event| events.push(event),
            &startup_errors,
        )
        .unwrap();
        assert_eq!(result.outcome, OperationOutcome::RolledBack);
        assert_eq!(startup_errors.message(), original_error);
        assert!(events.is_empty());
        if inactive {
            assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
        }
    }
}

#[test]
fn unrelated_configuration_error_survives_completed_recovery_and_normal_noops() {
    let invalid = store();
    invalid.set_internal_value(native::PROVIDER_MIGRATION_KEY, "1");
    invalid.set_internal_value(
        native::CONNECTION_PROFILES_KEY,
        "invalid synthetic configuration",
    );
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    let startup_errors = native::StartupErrors::new();
    let result = native::restore_saved_key_with_origin(
        &invalid,
        &worker,
        || panic!("invalid metadata reads no key"),
        &vault,
    );
    assert!(result.is_err());
    startup_errors.record_restore_result(&invalid, result);
    let unrelated_error = startup_errors.message();
    assert!(unrelated_error.is_some());
    let healthy = store();
    selected(&healthy, &worker, &vault);
    let connection = settings_snapshot(&healthy).unwrap().profiles["selected"].clone();
    fault(&healthy, &vault, "final_clear");
    assert!(
        native::lifecycle::save_connection_recording(
            &healthy,
            &worker,
            draft("selected", connection, Some("replacement")),
            SaveConnectionIntent::Update,
            SAVE_REQUEST,
            &vault,
            |_| {}
        )
        .is_err()
    );
    let token = settings_snapshot(&healthy)
        .unwrap()
        .pending_operation
        .unwrap()
        .id;
    clear_fault(&healthy, &vault);
    let mut events = vec![];
    let recovered = native::operation::retry_recording_with_startup_errors(
        &healthy,
        &worker,
        &token,
        &vault,
        |event| events.push(event),
        &startup_errors,
    )
    .unwrap();
    assert_eq!(recovered.outcome, OperationOutcome::Completed);
    assert_eq!(startup_errors.message(), unrelated_error);
    let generation = worker.execution_snapshot().unwrap().generation;
    native::lifecycle::save_connection_recording(
        &healthy,
        &worker,
        draft(
            "selected",
            recovered.settings.profiles["selected"].clone(),
            None,
        ),
        SaveConnectionIntent::Update,
        OTHER_REQUEST,
        &vault,
        |event| events.push(event),
    )
    .unwrap();
    native::lifecycle::switch_connection_recording(
        &healthy,
        &worker,
        "selected",
        &vault,
        |event| events.push(event),
    )
    .unwrap();
    assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
    assert_eq!(startup_errors.message(), unrelated_error);
    assert_eq!(events.len(), 1);
}

#[test]
fn selected_connection_startup_error_still_clears_after_a_proven_new_commit() {
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    selected(&store, &worker, &vault);
    vault.reject_read.set(true);
    let startup_errors = native::StartupErrors::new();
    let result = native::restore_saved_key_with_origin(
        &store,
        &worker,
        || panic!("local startup reads no legacy key"),
        &vault,
    );
    assert!(result.is_err());
    startup_errors.record_restore_result(&store, result);
    assert!(startup_errors.message().is_some());
    clear_fault(&store, &vault);
    let connection = settings_snapshot(&store).unwrap().profiles["selected"].clone();
    fault(&store, &vault, "final_clear");
    assert!(
        native::lifecycle::save_connection_recording(
            &store,
            &worker,
            draft("selected", connection, Some("new-secret")),
            SaveConnectionIntent::Update,
            SAVE_REQUEST,
            &vault,
            |_| {}
        )
        .is_err()
    );
    let token = settings_snapshot(&store)
        .unwrap()
        .pending_operation
        .unwrap()
        .id;
    clear_fault(&store, &vault);
    let mut events = vec![];
    let result = native::operation::retry_recording_with_startup_errors(
        &store,
        &worker,
        &token,
        &vault,
        |event| events.push(event),
        &startup_errors,
    )
    .unwrap();
    assert_eq!(result.outcome, OperationOutcome::Completed);
    assert!(startup_errors.message().is_none());
    assert_eq!(
        worker.execution_snapshot().unwrap().credential.as_deref(),
        Some("new-secret")
    );
    assert!(matches!(
        &events[..],
        [ModelCheckLifecycle::ProviderSetup {
            outcome: ProviderSetupOutcome::Saved,
            ..
        }]
    ));
}
