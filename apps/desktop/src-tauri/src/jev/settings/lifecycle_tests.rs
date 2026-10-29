use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::Path;

use super::super as native;
use super::{
    custom_connection, insert_accepted_status_and_source, insert_unfinished_assessment,
    source_and_status_snapshot, table_snapshot, unfinished_assessment_state,
};
use crate::jev::worker::WorkerHandle;
use crate::store::Store;
use native::lifecycle::{
    SaveConnectionIntent, recover_connection_change, remove_connection, remove_connection_recording,
};
use native::{CredentialReference, CredentialVault, SystemOneConnection, SystemOneDraft};

#[path = "recovery_tests.rs"]
mod recovery_tests;

fn save_connection_with_intent(
    store: &Store,
    worker: &WorkerHandle,
    draft: SystemOneDraft,
    intent: SaveConnectionIntent,
    vault: &impl CredentialVault,
) -> Result<(native::SystemOneSettings, bool), &'static str> {
    native::lifecycle::save_connection_with_intent(
        store,
        worker,
        draft,
        intent,
        super::REQUEST_ID,
        vault,
    )
}

fn save_connection_recording(
    store: &Store,
    worker: &WorkerHandle,
    draft: SystemOneDraft,
    intent: SaveConnectionIntent,
    vault: &impl CredentialVault,
    emit: impl FnMut(crate::analytics::event::ModelCheckLifecycle),
) -> Result<native::SystemOneSettings, &'static str> {
    native::lifecycle::save_connection_recording(
        store,
        worker,
        draft,
        intent,
        super::REQUEST_ID,
        vault,
        emit,
    )
}

#[derive(Default)]
struct ProfileVault {
    values: RefCell<BTreeMap<Option<String>, String>>,
    reads: RefCell<Vec<Option<String>>>,
    writes: RefCell<Vec<Option<String>>>,
    deletes: RefCell<Vec<Option<String>>>,
    reject_read: Cell<bool>,
    reject_write: Cell<bool>,
    partial_write: Cell<bool>,
    reject_delete: Cell<bool>,
}

impl CredentialVault for ProfileVault {
    type Error = &'static str;
    fn read(&self, id: Option<&str>) -> Result<Option<String>, Self::Error> {
        self.reads.borrow_mut().push(id.map(str::to_owned));
        if self.reject_read.get() {
            return Err("read");
        }
        Ok(self.values.borrow().get(&id.map(str::to_owned)).cloned())
    }
    fn write(&self, id: Option<&str>, value: &str) -> Result<(), Self::Error> {
        self.writes.borrow_mut().push(id.map(str::to_owned));
        if !self.reject_write.get() || self.partial_write.get() {
            self.values
                .borrow_mut()
                .insert(id.map(str::to_owned), value.to_owned());
        }
        if self.reject_write.get() {
            Err("write")
        } else {
            Ok(())
        }
    }
    fn delete(&self, id: Option<&str>) -> Result<(), Self::Error> {
        self.deletes.borrow_mut().push(id.map(str::to_owned));
        if self.reject_delete.get() {
            return Err("delete");
        }
        self.values.borrow_mut().remove(&id.map(str::to_owned));
        Ok(())
    }
    fn is_missing(_: &Self::Error) -> bool {
        false
    }
    fn write_failure(_: &Self::Error) -> &'static str {
        "Synthetic credential write failed."
    }
}

fn store() -> Store {
    Store::open_in_memory(Path::new("synthetic-profile-lifecycle")).unwrap()
}

fn draft(id: &str, connection: SystemOneConnection, secret: Option<&str>) -> SystemOneDraft {
    SystemOneDraft {
        connection_id: id.to_owned(),
        connection,
        credential: secret.map(str::to_owned),
    }
}

fn create(store: &Store, worker: &WorkerHandle, id: &str, vault: &ProfileVault) {
    save_connection_with_intent(
        store,
        worker,
        draft(id, custom_connection(), None),
        SaveConnectionIntent::Create,
        vault,
    )
    .unwrap();
}

fn enable(store: &Store, worker: &WorkerHandle, vault: &ProfileVault) {
    let connection = native::active_connection(store).unwrap();
    let credential = native::resolve_credential(&connection, None, vault).unwrap();
    native::enable_model_checks(store, worker, connection, credential).unwrap();
}

#[test]
fn first_add_selects_without_enablement_and_add_with_null_selection_keeps_null() {
    use crate::analytics::event::{ModelCheckLifecycle, ProviderSetupOutcome};
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    native::restore_saved_key_with_vault(
        &store,
        &worker,
        || panic!("fresh setup reads no key"),
        &vault,
    )
    .unwrap();
    let fresh = native::profiles(&store).unwrap();
    assert!(fresh.profiles.is_empty());
    assert!(fresh.active_id.is_none());
    assert!(!native::yaml_discovery_enabled(&store));
    let mut events = vec![];
    let saved = save_connection_recording(
        &store,
        &worker,
        draft(
            "connection-00000000-0000-4000-8000-000000000001",
            custom_connection(),
            None,
        ),
        SaveConnectionIntent::Create,
        &vault,
        |event| events.push(event),
    )
    .unwrap();
    assert_eq!(
        saved.active_id.as_deref(),
        Some("connection-00000000-0000-4000-8000-000000000001")
    );
    assert!(matches!(
        &events[..],
        [ModelCheckLifecycle::ProviderSetup {
            outcome: ProviderSetupOutcome::Saved,
            ..
        }]
    ));
    assert!(!native::yaml_discovery_enabled(&store));
    assert!(!worker.is_available());
    create(&store, &worker, "second", &vault);
    remove_connection(
        &store,
        &worker,
        saved.active_id.as_deref().unwrap(),
        1,
        &vault,
    )
    .unwrap();
    assert!(native::profiles(&store).unwrap().active_id.is_none());
    create(&store, &worker, "third", &vault);
    assert!(native::profiles(&store).unwrap().active_id.is_none());
    native::restore_saved_key_with_vault(
        &store,
        &worker,
        || panic!("null selection reads no key"),
        &vault,
    )
    .unwrap();
    assert!(native::profiles(&store).unwrap().active_id.is_none());
    assert!(!worker.is_available());
}

#[test]
fn migration_and_restart_preserve_empty_null_and_existing_default_shaped_profiles() {
    for marker in [false, true] {
        for nonempty in [false, true] {
            let store = store();
            let profiles = if nonempty {
                serde_json::json!({"jev": SystemOneConnection::jev_default()})
            } else {
                serde_json::json!({})
            };
            let raw = serde_json::json!({"activeId":null,"profiles":profiles}).to_string();
            store.set_internal_value(native::CONNECTION_PROFILES_KEY, &raw);
            if marker {
                store.set_internal_value(native::PROVIDER_MIGRATION_KEY, "1");
            }
            let vault = ProfileVault::default();
            vault.reject_read.set(true);
            let worker = WorkerHandle::default();
            native::restore_saved_key_with_vault(
                &store,
                &worker,
                || panic!("null selection reads no legacy key"),
                &vault,
            )
            .unwrap();
            let saved = native::profiles(&store).unwrap();
            assert!(saved.active_id.is_none());
            assert_eq!(saved.profiles.len(), usize::from(nonempty));
            assert!(vault.reads.borrow().is_empty());
            assert!(!worker.is_available());
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    create(&store, &worker, "last", &vault);
    remove_connection(&store, &worker, "last", 1, &vault).unwrap();
    drop(store);
    let reopened = Store::open(directory.path()).unwrap();
    native::restore_saved_key_with_vault(
        &reopened,
        &worker,
        || panic!("deleted profile reads no key"),
        &vault,
    )
    .unwrap();
    let saved = native::profiles(&reopened).unwrap();
    assert!(saved.profiles.is_empty());
    assert!(saved.active_id.is_none());
    assert!(!native::yaml_discovery_enabled(&reopened));
}

#[test]
fn missing_corrupt_or_dangling_profiles_fail_without_default_repair() {
    for raw in [
        None,
        Some("bad json"),
        Some(r#"{"profiles":{}}"#),
        Some(r#"{"activeId":"missing","profiles":{}}"#),
    ] {
        let store = store();
        store.set_internal_value(native::PROVIDER_MIGRATION_KEY, "1");
        if let Some(raw) = raw {
            store.set_internal_value(native::CONNECTION_PROFILES_KEY, raw);
        }
        let before = store.internal_value(native::CONNECTION_PROFILES_KEY);
        let worker = WorkerHandle::default();
        assert!(
            native::restore_saved_key_with_vault(
                &store,
                &worker,
                || panic!("invalid settings read no key"),
                &ProfileVault::default()
            )
            .is_err()
        );
        assert_eq!(
            store.internal_value(native::CONNECTION_PROFILES_KEY),
            before
        );
        assert!(!worker.is_available());
    }
    let store = store();
    store.set_internal_value(native::ACTIVE_CONNECTION_KEY, "jev");
    assert!(native::migrate_provider_state(&store).is_err());
    assert!(
        store
            .internal_value(native::CONNECTION_PROFILES_KEY)
            .is_none()
    );
}

#[test]
fn inactive_required_credential_and_provider_changes_fail_before_vault_or_runtime_access() {
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    create(&store, &worker, "active", &vault);
    enable(&store, &worker, &vault);
    let generation = worker.execution_snapshot().unwrap().generation;
    let mut profiles = native::profiles(&store).unwrap();
    let missing = SystemOneConnection {
        provider: native::SystemOneProvider::Cloudflare,
        endpoint: native::SystemOneEndpoint::CloudflareAccount("synthetic-account".into()),
        model: "clef".into(),
        credential: None,
        ..custom_connection()
    };
    profiles.profiles.insert("inactive".into(), missing.clone());
    native::save_profiles(&store, &profiles).unwrap();
    let mut edit = missing;
    edit.model = "clef-flash".into();
    assert!(
        save_connection_with_intent(
            &store,
            &worker,
            draft("inactive", edit, None),
            SaveConnectionIntent::Update,
            &vault
        )
        .unwrap_err()
        .contains("needs a credential")
    );
    assert!(
        save_connection_with_intent(
            &store,
            &worker,
            draft(
                "active",
                SystemOneConnection::jev_default(),
                Some("new-secret")
            ),
            SaveConnectionIntent::Update,
            &vault
        )
        .unwrap_err()
        .contains("different provider")
    );
    assert_eq!(native::profiles(&store).unwrap(), profiles);
    assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
    assert!(vault.reads.borrow().is_empty());
    assert!(vault.writes.borrow().is_empty());
    assert!(vault.deletes.borrow().is_empty());
}

#[test]
fn inactive_edit_and_removal_preserve_runtime_authentication_and_unfinished_work() {
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    create(&store, &worker, "active", &vault);
    create(&store, &worker, "inactive", &vault);
    enable(&store, &worker, &vault);
    insert_unfinished_assessment(&store, "dispatching");
    insert_accepted_status_and_source(&store);
    let before = source_and_status_snapshot(&store);
    let generation = worker.execution_snapshot().unwrap().generation;
    worker.reject_authentication(&store, generation).unwrap();
    *native::STARTUP_ERROR.lock().unwrap() = Some("Synthetic startup error.");
    let connection = worker.system_one_connection();
    vault.reject_read.set(true);
    let mut edit = native::profiles(&store).unwrap().profiles["inactive"].clone();
    edit.model = "edited-model".into();
    save_connection_with_intent(
        &store,
        &worker,
        draft("inactive", edit, None),
        SaveConnectionIntent::Update,
        &vault,
    )
    .unwrap();
    remove_connection(&store, &worker, "inactive", 2, &vault).unwrap();
    assert!(worker.authentication_rejected());
    assert_eq!(worker.system_one_connection(), connection);
    assert_eq!(
        store.internal_value(native::AUTH_REJECTED_KEY).as_deref(),
        Some("true")
    );
    assert_eq!(unfinished_assessment_state(&store), "dispatching");
    assert_eq!(source_and_status_snapshot(&store), before);
    assert!(vault.reads.borrow().is_empty());
    assert!(native::yaml_discovery_enabled(&store));
    assert_eq!(
        *native::STARTUP_ERROR.lock().unwrap(),
        Some("Synthetic startup error.")
    );
}

#[test]
fn saved_and_switched_events_remain_separate_and_failed_or_noop_edits_emit_nothing() {
    use crate::analytics::event::{ModelCheckLifecycle, ProviderSetupOutcome};
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    create(&store, &worker, "active", &vault);
    let mut events = vec![];
    save_connection_recording(
        &store,
        &worker,
        draft("inactive", custom_connection(), None),
        SaveConnectionIntent::Create,
        &vault,
        |event| events.push(event),
    )
    .unwrap();
    assert!(matches!(
        &events[..],
        [ModelCheckLifecycle::ProviderSetup {
            outcome: ProviderSetupOutcome::Saved,
            ..
        }]
    ));
    let unchanged = native::profiles(&store).unwrap().profiles["inactive"].clone();
    save_connection_recording(
        &store,
        &worker,
        draft("inactive", unchanged.clone(), None),
        SaveConnectionIntent::Update,
        &vault,
        |event| events.push(event),
    )
    .unwrap();
    assert_eq!(events.len(), 1);
    let mut edit = unchanged.clone();
    edit.model = "edited-model".into();
    save_connection_recording(
        &store,
        &worker,
        draft("inactive", edit, None),
        SaveConnectionIntent::Update,
        &vault,
        |event| events.push(event),
    )
    .unwrap();
    assert!(matches!(
        &events[..],
        [
            ModelCheckLifecycle::ProviderSetup {
                outcome: ProviderSetupOutcome::Saved,
                ..
            },
            ModelCheckLifecycle::ProviderSetup {
                outcome: ProviderSetupOutcome::Saved,
                ..
            }
        ]
    ));
    assert!(
        save_connection_recording(
            &store,
            &worker,
            draft("inactive", unchanged, None),
            SaveConnectionIntent::Update,
            &vault,
            |event| events.push(event)
        )
        .is_err()
    );
    native::lifecycle::switch_connection_recording(&store, &worker, "inactive", &vault, |event| {
        events.push(event)
    })
    .unwrap();
    assert!(matches!(
        events.last(),
        Some(ModelCheckLifecycle::ProviderSetup {
            outcome: ProviderSetupOutcome::Switched,
            ..
        })
    ));
    assert_eq!(events.len(), 3);
    assert!(!native::yaml_discovery_enabled(&store));
    assert!(!worker.is_available());
}

#[test]
fn selected_removal_database_and_fence_failures_do_not_publish_false_success() {
    use crate::analytics::event::ModelCheckLifecycle;
    for failure in [
        "marker",
        "cancellation",
        "profiles",
        "selection",
        "finalize",
    ] {
        let store = store();
        let worker = WorkerHandle::default();
        let vault = ProfileVault::default();
        save_connection_with_intent(
            &store,
            &worker,
            draft("selected", custom_connection(), Some("secret")),
            SaveConnectionIntent::Create,
            &vault,
        )
        .unwrap();
        enable(&store, &worker, &vault);
        insert_unfinished_assessment(&store, "dispatching");
        insert_accepted_status_and_source(&store);
        let before = source_and_status_snapshot(&store);
        let (table, predicate) = match failure {
            "marker" => (
                "setting",
                "NEW.key = 'internal:smartChecksProfileOperationV1' AND NEW.value != '' AND json_extract(NEW.value, '$.committed') = 0",
            ),
            "cancellation" => ("burn_check_candidate", "NEW.state = 'cancelled'"),
            "profiles" => ("setting", "NEW.key = 'internal:smartChecksConnectionsV1'"),
            "selection" => (
                "setting",
                "NEW.key = 'internal:smartChecksActiveConnectionV1' AND NEW.value = ''",
            ),
            _ => (
                "setting",
                "NEW.key = 'internal:smartChecksProfileOperationV1' AND NEW.value = ''",
            ),
        };
        store.lock().execute_batch(&format!("CREATE TEMP TRIGGER reject_profile_removal BEFORE UPDATE ON {table} WHEN {predicate} BEGIN SELECT RAISE(ABORT, 'synthetic removal failure'); END;")).unwrap();
        let mut events = vec![];
        assert!(
            remove_connection_recording(&store, &worker, "selected", 1, &vault, |event| events
                .push(event))
            .is_err(),
            "{failure}"
        );
        assert!(events.is_empty());
        assert_eq!(source_and_status_snapshot(&store), before);
        assert_eq!(
            native::profiles(&store).unwrap().active_id.is_none(),
            failure == "finalize"
        );
        assert_eq!(native::yaml_discovery_enabled(&store), failure == "marker");
        assert_eq!(worker.is_available(), failure == "marker");
        if failure != "finalize" {
            assert!(vault.deletes.borrow().is_empty());
        }
        store
            .lock()
            .execute_batch("DROP TRIGGER reject_profile_removal")
            .unwrap();
        let pending = native::operation::settings_snapshot(&store)
            .unwrap()
            .pending_operation;
        let recovered = pending.map(|pending| {
            native::operation::retry_recording(&store, &worker, &pending.id, &vault, |event| {
                events.push(event)
            })
            .unwrap()
        });
        let saved = match recovered {
            Some(result) if result.outcome == native::operation::OperationOutcome::Completed => {
                result.settings
            }
            _ => remove_connection_recording(&store, &worker, "selected", 1, &vault, |event| {
                events.push(event)
            })
            .unwrap(),
        };
        assert!(saved.active_id.is_none());
        assert!(saved.profiles.is_empty());
        assert_eq!(events.len(), 2, "{failure}");
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, ModelCheckLifecycle::Enablement { enabled: false }))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    ModelCheckLifecycle::ProviderSetup {
                        outcome: crate::analytics::event::ProviderSetupOutcome::ConnectionRemoved,
                        ..
                    }
                ))
                .count(),
            1
        );
        assert_eq!(unfinished_assessment_state(&store), "cancelled");
        assert_eq!(source_and_status_snapshot(&store), before);
    }
}

#[test]
fn restart_rolls_back_an_uncommitted_fresh_slot_without_replaying_unknown_delivery() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    save_connection_with_intent(
        &store,
        &worker,
        draft("selected", custom_connection(), Some("old-secret")),
        SaveConnectionIntent::Create,
        &vault,
    )
    .unwrap();
    enable(&store, &worker, &vault);
    insert_unfinished_assessment(&store, "dispatching");
    insert_accepted_status_and_source(&store);
    store.lock().execute_batch("INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, decision_name, attempt_number, state, outcome, updated_at_epoch)
        VALUES ('unknown-attempt', 'synthetic-candidate', 'review', 1, 'settled', 'unknown', 1);
        INSERT INTO burn_check_decision_usage (attempt_id, usage_known) VALUES ('unknown-attempt', 0);
        CREATE TEMP TRIGGER reject_profile_save BEFORE UPDATE ON setting WHEN NEW.key = 'internal:smartChecksConnectionsV1' BEGIN SELECT RAISE(ABORT, 'synthetic save failure'); END;").unwrap();
    let before = source_and_status_snapshot(&store);
    let attempts = table_snapshot(
        &store,
        &["burn_check_evaluation_attempt", "burn_check_decision_usage"],
    );
    let profile = native::profiles(&store).unwrap().profiles["selected"].clone();
    vault.reject_delete.set(true);
    assert!(
        save_connection_with_intent(
            &store,
            &worker,
            draft("selected", profile.clone(), Some("new-secret")),
            SaveConnectionIntent::Update,
            &vault
        )
        .is_err()
    );
    assert_eq!(
        native::profiles(&store).unwrap().profiles["selected"],
        profile
    );
    assert_eq!(vault.values.borrow().len(), 2);
    drop(store);
    let reopened = Store::open(directory.path()).unwrap();
    let restarted = WorkerHandle::default();
    vault.reject_delete.set(false);
    native::restore_saved_key_with_vault(
        &reopened,
        &restarted,
        || panic!("profile restart reads no legacy key"),
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
    assert_eq!(vault.values.borrow().len(), 1);
    assert_eq!(source_and_status_snapshot(&reopened), before);
    assert_eq!(
        table_snapshot(
            &reopened,
            &["burn_check_evaluation_attempt", "burn_check_decision_usage"]
        ),
        attempts
    );
    assert_eq!(unfinished_assessment_state(&reopened), "cancelled");
}

#[test]
fn removal_preserves_history_evidence_usage_attempts_and_frozen_preparation() {
    use crate::analytics::event::{ModelCheckLifecycle, ProviderSetupOutcome};
    for selected in [false, true] {
        for enabled in [false, true] {
            let store = store();
            let worker = WorkerHandle::default();
            let vault = ProfileVault::default();
            create(&store, &worker, "active", &vault);
            create(&store, &worker, "inactive", &vault);
            if enabled {
                enable(&store, &worker, &vault);
            }
            insert_unfinished_assessment(&store, "dispatching");
            insert_accepted_status_and_source(&store);
            store.lock().execute_batch("UPDATE burn_check_candidate SET preparation_json = '{\"frozen\":true}';
                INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, decision_name, attempt_number, state, outcome, updated_at_epoch)
                VALUES ('unknown-attempt', 'synthetic-candidate', 'review', 1, 'settled', 'unknown', 1);
                INSERT INTO burn_check_decision_usage (attempt_id, usage_known) VALUES ('unknown-attempt', 0);").unwrap();
            let protected = source_and_status_snapshot(&store);
            let attempts = table_snapshot(
                &store,
                &["burn_check_evaluation_attempt", "burn_check_decision_usage"],
            );
            let accepted: String = store.lock().query_row("SELECT json_array(outcome, actions_json, evidence_valid) FROM burn_check_assessment_history WHERE assessment_id = 'synthetic-assessment'", [], |row| row.get(0)).unwrap();
            let mut events = vec![];
            let id = if selected { "active" } else { "inactive" };
            let saved = remove_connection_recording(&store, &worker, id, 1, &vault, |event| {
                events.push(event)
            })
            .unwrap();
            assert_eq!(
                saved.active_id.as_deref(),
                if selected { None } else { Some("active") }
            );
            assert!(!saved.profiles.contains_key(id));
            assert_eq!(events.len(), if selected && enabled { 2 } else { 1 });
            assert!(matches!(
                events[0],
                ModelCheckLifecycle::ProviderSetup {
                    outcome: ProviderSetupOutcome::ConnectionRemoved,
                    ..
                }
            ));
            if selected && enabled {
                assert!(matches!(
                    events[1],
                    ModelCheckLifecycle::Enablement { enabled: false }
                ));
            }
            assert_eq!(
                unfinished_assessment_state(&store),
                if selected { "cancelled" } else { "dispatching" }
            );
            assert_eq!(source_and_status_snapshot(&store), protected);
            assert_eq!(
                table_snapshot(
                    &store,
                    &["burn_check_evaluation_attempt", "burn_check_decision_usage"]
                ),
                attempts
            );
            assert_eq!(store.lock().query_row("SELECT json_array(outcome, actions_json, evidence_valid) FROM burn_check_assessment_history WHERE assessment_id = 'synthetic-assessment'", [], |row| row.get::<_, String>(0)).unwrap(), accepted);
            assert_eq!(store.lock().query_row("SELECT preparation_json FROM burn_check_candidate WHERE candidate_id = 'synthetic-candidate'", [], |row| row.get::<_, String>(0)).unwrap(), "{\"frozen\":true}");
            assert_eq!(native::yaml_discovery_enabled(&store), enabled && !selected);
        }
    }
}

#[tokio::test]
async fn stale_edit_delete_and_identity_collisions_cannot_recreate_or_replace_profiles() {
    let _lock = native::CREDENTIAL_CHANGE.lock().await;
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    create(&store, &worker, "profile", &vault);
    let initial = native::profiles(&store).unwrap().profiles["profile"].clone();
    let mut edit = initial.clone();
    edit.model = "new-model".into();
    save_connection_with_intent(
        &store,
        &worker,
        draft("profile", edit, None),
        SaveConnectionIntent::Update,
        &vault,
    )
    .unwrap();
    assert!(
        save_connection_with_intent(
            &store,
            &worker,
            draft("profile", initial.clone(), None),
            SaveConnectionIntent::Update,
            &vault
        )
        .unwrap_err()
        .contains("another window")
    );
    assert!(remove_connection(&store, &worker, "profile", 1, &vault).is_err());
    assert!(
        save_connection_with_intent(
            &store,
            &worker,
            draft("profile", initial.clone(), None),
            SaveConnectionIntent::Create,
            &vault
        )
        .is_err()
    );
    remove_connection(&store, &worker, "profile", 2, &vault).unwrap();
    assert!(
        save_connection_with_intent(
            &store,
            &worker,
            draft("profile", initial.clone(), None),
            SaveConnectionIntent::Update,
            &vault
        )
        .is_err()
    );
    assert!(
        save_connection_with_intent(
            &store,
            &worker,
            draft("profile", initial, None),
            SaveConnectionIntent::Create,
            &vault
        )
        .is_err()
    );
    let mut profiles = native::profiles(&store).unwrap();
    let mut alias = custom_connection();
    alias.credential = Some(CredentialReference::Connection("surviving-slot".into()));
    profiles.profiles.insert("alias".into(), alias);
    native::save_profiles(&store, &profiles).unwrap();
    assert!(
        save_connection_with_intent(
            &store,
            &worker,
            draft("surviving-slot", custom_connection(), None),
            SaveConnectionIntent::Create,
            &vault
        )
        .is_err()
    );
}

#[test]
fn shared_credential_replacement_and_profile_removal_use_isolated_slots() {
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    save_connection_with_intent(
        &store,
        &worker,
        draft("active", custom_connection(), Some("original-secret")),
        SaveConnectionIntent::Create,
        &vault,
    )
    .unwrap();
    let mut profiles = native::profiles(&store).unwrap();
    let original = profiles.profiles["active"].clone();
    profiles
        .profiles
        .insert("inactive".into(), original.clone());
    native::save_profiles(&store, &profiles).unwrap();
    enable(&store, &worker, &vault);
    let generation = worker.execution_snapshot().unwrap().generation;
    vault.reads.borrow_mut().clear();
    vault.reject_read.set(true);
    let (saved, _) = save_connection_with_intent(
        &store,
        &worker,
        draft("inactive", original.clone(), Some("new-secret")),
        SaveConnectionIntent::Update,
        &vault,
    )
    .unwrap();
    assert_eq!(saved.profiles["active"], original);
    assert_eq!(saved.profiles["inactive"].revision, 2);
    assert_ne!(
        saved.profiles["active"].credential,
        saved.profiles["inactive"].credential
    );
    assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
    assert_eq!(
        worker.execution_snapshot().unwrap().credential.as_deref(),
        Some("original-secret")
    );
    assert!(vault.reads.borrow().is_empty());
    remove_connection(&store, &worker, "inactive", 2, &vault).unwrap();
    vault.reject_read.set(false);
    assert_eq!(
        native::resolve_credential(&original, None, &vault)
            .unwrap()
            .as_deref(),
        Some("original-secret")
    );
    assert_eq!(vault.values.borrow().len(), 1);
}

#[test]
fn retained_legacy_key_is_not_reused_or_deleted_by_profile_operations_and_can_be_forgotten() {
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    vault
        .values
        .borrow_mut()
        .insert(None, "retained-secret".into());
    store.set_internal_value(native::SAVED_KEY_KEY, "true");
    native::migrate_provider_state(&store).unwrap();
    remove_connection(&store, &worker, "jev", 1, &vault).unwrap();
    assert!(vault.values.borrow().contains_key(&None));
    assert!(native::saved_key_marker(&store));
    let mut null_reference = SystemOneConnection::jev_default();
    null_reference.credential = None;
    assert!(
        save_connection_with_intent(
            &store,
            &worker,
            draft("new-jev", null_reference, None),
            SaveConnectionIntent::Create,
            &vault
        )
        .is_err()
    );
    save_connection_with_intent(
        &store,
        &worker,
        draft("explicit-jev", SystemOneConnection::jev_default(), None),
        SaveConnectionIntent::Create,
        &vault,
    )
    .unwrap();
    remove_connection(&store, &worker, "explicit-jev", 1, &vault).unwrap();
    let prepared = native::begin_legacy_removal(&store, &worker).unwrap();
    native::finish_legacy_removal(&store, &vault, &prepared).unwrap();
    assert!(!vault.values.borrow().contains_key(&None));
    assert!(!native::saved_key_marker(&store));
    assert!(native::profiles(&store).unwrap().profiles.is_empty());
}

#[test]
fn selected_removal_cleanup_failure_is_durable_and_emits_only_after_retry_settles() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    save_connection_with_intent(
        &store,
        &worker,
        draft("selected", custom_connection(), Some("secret")),
        SaveConnectionIntent::Create,
        &vault,
    )
    .unwrap();
    enable(&store, &worker, &vault);
    vault.reject_delete.set(true);
    let mut events = vec![];
    assert!(
        remove_connection_recording(&store, &worker, "selected", 1, &vault, |event| events
            .push(event))
        .is_err()
    );
    assert!(events.is_empty());
    assert!(native::profiles(&store).unwrap().active_id.is_none());
    assert!(native::profiles(&store).unwrap().profiles.is_empty());
    assert!(!native::yaml_discovery_enabled(&store));
    assert!(!worker.is_available());
    drop(store);
    let reopened = Store::open(directory.path()).unwrap();
    assert!(
        native::restore_saved_key_with_vault(
            &reopened,
            &worker,
            || panic!("pending removal reads no key"),
            &vault
        )
        .is_err()
    );
    vault.reject_delete.set(false);
    let pending = native::operation::settings_snapshot(&reopened)
        .unwrap()
        .pending_operation
        .unwrap();
    native::operation::retry_recording(&reopened, &worker, &pending.id, &vault, |event| {
        events.push(event)
    })
    .unwrap();
    assert_eq!(events.len(), 2);
    native::restore_saved_key_with_vault(
        &reopened,
        &worker,
        || panic!("empty restore reads no key"),
        &vault,
    )
    .unwrap();
    assert!(!worker.is_available());
}

#[test]
fn partial_fresh_slot_write_and_failed_cleanup_never_change_the_old_shared_secret() {
    let store = store();
    let worker = WorkerHandle::default();
    let vault = ProfileVault::default();
    save_connection_with_intent(
        &store,
        &worker,
        draft("active", custom_connection(), Some("old-secret")),
        SaveConnectionIntent::Create,
        &vault,
    )
    .unwrap();
    let mut profiles = native::profiles(&store).unwrap();
    let old = profiles.profiles["active"].clone();
    profiles.profiles.insert("inactive".into(), old.clone());
    native::save_profiles(&store, &profiles).unwrap();
    enable(&store, &worker, &vault);
    let generation = worker.execution_snapshot().unwrap().generation;
    vault.reject_write.set(true);
    vault.partial_write.set(true);
    vault.reject_delete.set(true);
    assert!(
        save_connection_with_intent(
            &store,
            &worker,
            draft("inactive", old.clone(), Some("new-secret")),
            SaveConnectionIntent::Update,
            &vault
        )
        .is_err()
    );
    assert_eq!(native::profiles(&store).unwrap(), profiles);
    assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
    assert_eq!(
        worker.execution_snapshot().unwrap().credential.as_deref(),
        Some("old-secret")
    );
    assert_eq!(
        native::resolve_credential(&old, None, &vault)
            .unwrap()
            .as_deref(),
        Some("old-secret")
    );
    assert_eq!(vault.values.borrow().len(), 2);
    vault.reject_delete.set(false);
    recover_connection_change(&store, &worker, &vault).unwrap();
    assert_eq!(vault.values.borrow().len(), 1);
    assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
}
