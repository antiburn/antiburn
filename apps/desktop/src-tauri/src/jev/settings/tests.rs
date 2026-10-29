use std::cell::Cell;
use std::path::Path;

use super::operation::{OperationOutcome, retry_recording, settings_snapshot};
use super::{
    AUTH_REJECTED_KEY, CONNECTION_PENDING_KEY, CONNECTION_PROFILES_KEY,
    CREDENTIAL_CHANGE_PENDING_KEY, CheckAvailabilityEvent, CredentialVault, PROVIDER_MIGRATION_KEY,
    SAVED_KEY_KEY, YAML_DISCOVERY_ENABLED_KEY, active_connection, delete_credential,
    saved_key_marker, write_credential,
};
use crate::jev::worker::WorkerHandle;
use crate::store::Store;

#[path = "credential_read_tests.rs"]
mod credential_read_tests;
#[path = "lifecycle_tests.rs"]
mod lifecycle_tests;

const REQUEST_ID: &str = "00000000-0000-4000-8000-000000000001";

#[test]
fn yaml_discovery_defaults_enabled_and_preserves_saved_choices() {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-yaml-discovery-setting")).unwrap();
    assert!(super::yaml_discovery_enabled(&store));
    assert!(store.internal_value(YAML_DISCOVERY_ENABLED_KEY).is_none());
    store.set_internal_value("internal:burnChecksEnabledAtEpochV1", "123");
    assert!(super::yaml_discovery_enabled(&store));
    assert!(!saved_key_marker(&store));
    store
        .set_internal_value_checked(YAML_DISCOVERY_ENABLED_KEY, "false")
        .unwrap();
    assert!(!super::yaml_discovery_enabled(&store));
    assert!(
        !super::read_check_availability(&store, false, false, None)
            .unwrap()
            .yaml_enabled
    );
    assert_eq!(
        store.internal_value(YAML_DISCOVERY_ENABLED_KEY).as_deref(),
        Some("false")
    );
    store
        .set_internal_value_checked(YAML_DISCOVERY_ENABLED_KEY, "true")
        .unwrap();
    assert!(super::yaml_discovery_enabled(&store));
}

#[test]
fn yaml_discovery_fails_closed_for_malformed_values() {
    let store = Store::open_in_memory(Path::new("synthetic-yaml-preference")).unwrap();
    for value in ["", "TRUE", "1", "null", " true", "true ", "\"true\""] {
        store
            .set_internal_value_checked(YAML_DISCOVERY_ENABLED_KEY, value)
            .unwrap();
        assert!(!super::yaml_discovery_enabled(&store), "{value:?}");
        assert_eq!(
            store.internal_value(YAML_DISCOVERY_ENABLED_KEY).as_deref(),
            Some(value)
        );
    }
}

#[test]
fn yaml_discovery_fails_closed_for_unreadable_storage() {
    let store = Store::open_in_memory(Path::new("synthetic-yaml-preference")).unwrap();
    store
        .set_internal_value_checked(YAML_DISCOVERY_ENABLED_KEY, "true")
        .unwrap();
    store.lock().execute_batch("DROP TABLE setting").unwrap();
    assert!(!super::yaml_discovery_enabled(&store));
}

#[test]
fn fresh_availability_enables_discovery_without_a_provider_or_discovered_checks() {
    let store = Store::open_in_memory(Path::new("synthetic-fresh-availability")).unwrap();
    let snapshot = super::read_check_availability(&store, false, false, None).unwrap();
    assert!(snapshot.yaml_enabled);
    assert!(!snapshot.configured);
    assert!(!snapshot.saved_key);
    assert!(snapshot.installed_checks.is_empty());
    assert!(store.internal_value(YAML_DISCOVERY_ENABLED_KEY).is_none());
}

#[derive(Default)]
struct OfflineVault {
    fail_read: Cell<bool>,
    fail_write: Cell<bool>,
    write_calls: Cell<usize>,
    fail_delete: Cell<bool>,
    value: std::cell::RefCell<Option<String>>,
    values: std::cell::RefCell<std::collections::BTreeMap<Option<String>, String>>,
    deleted_ids: std::cell::RefCell<Vec<Option<String>>>,
}

impl CredentialVault for OfflineVault {
    type Error = &'static str;

    fn read(&self, id: Option<&str>) -> Result<Option<String>, Self::Error> {
        if self.fail_read.replace(false) {
            return Err("read");
        }
        if self.values.borrow().is_empty() {
            return Ok(self.value.borrow().clone());
        }
        Ok(self.values.borrow().get(&id.map(str::to_owned)).cloned())
    }

    fn write(&self, id: Option<&str>, value: &str) -> Result<(), Self::Error> {
        let call = self.write_calls.get() + 1;
        self.write_calls.set(call);
        if self.fail_write.replace(false) {
            return Err("write");
        }
        *self.value.borrow_mut() = Some(value.to_owned());
        self.values
            .borrow_mut()
            .insert(id.map(str::to_owned), value.to_owned());
        Ok(())
    }

    fn delete(&self, id: Option<&str>) -> Result<(), Self::Error> {
        if self.fail_delete.replace(false) {
            return Err("delete");
        }
        self.values.borrow_mut().remove(&id.map(str::to_owned));
        *self.value.borrow_mut() = self.values.borrow().values().next_back().cloned();
        self.deleted_ids.borrow_mut().push(id.map(str::to_owned));
        Ok(())
    }

    fn is_missing(error: &Self::Error) -> bool {
        *error == "missing"
    }

    fn write_failure(_error: &Self::Error) -> &'static str {
        "Could not save the API token to secure storage. Check credential storage access and retry."
    }
}

fn custom_connection() -> crate::jev::config::SystemOneConnection {
    crate::jev::config::SystemOneConnection {
        provider: crate::jev::config::SystemOneProvider::Custom,
        endpoint: crate::jev::config::SystemOneEndpoint::ExactUrl(
            "https://proxy.example/systemone".into(),
        ),
        model: "proxy-model".into(),
        credential: None,
        context_override: None,
        ..crate::jev::config::SystemOneConnection::default()
    }
}

// These safety fixtures perform separate Save, Use, and enable actions.
pub(super) fn save_and_use(
    store: &Store,
    worker: &WorkerHandle,
    mut draft: super::SystemOneDraft,
    vault: &impl CredentialVault,
) -> Result<(super::SystemOneSettings, bool), &'static str> {
    use super::SaveConnectionIntent;
    let enabled = super::yaml_discovery_enabled(store);
    let saved = super::profiles(store)?;
    let intent = match saved.profiles.get(&draft.connection_id) {
        Some(previous) => {
            draft.connection.revision = previous.revision;
            SaveConnectionIntent::Update
        }
        None => SaveConnectionIntent::Create,
    };
    let id = draft.connection_id.clone();
    let result =
        super::save_connection_with_intent(store, worker, draft, intent, REQUEST_ID, vault)?;
    if intent == SaveConnectionIntent::Create {
        store
            .set_internal_value_checked(
                YAML_DISCOVERY_ENABLED_KEY,
                if enabled { "true" } else { "false" },
            )
            .map_err(|_| "Could not save test enablement.")?;
        super::switch_connection(store, worker, &id, vault)?;
        if enabled {
            let connection = super::active_connection(store)?;
            let credential = super::resolve_credential(&connection, None, vault)?;
            super::enable_model_checks(store, worker, connection, credential)?;
        }
    }
    Ok((super::operation::settings_snapshot(store)?, result.1))
}

fn migrate_provider_state(store: &Store) -> Result<(), &'static str> {
    if store.internal_value(CONNECTION_PROFILES_KEY).is_none() {
        super::save_profiles(
            store,
            &super::ConnectionProfiles {
                active_id: Some("jev".to_owned()),
                profiles: [("jev".to_owned(), super::SystemOneConnection::jev_default())].into(),
                retired_ids: Default::default(),
            },
        )?;
    }
    super::migrate_provider_state(store)
}

fn restore_saved_key(
    store: &Store,
    worker: &WorkerHandle,
    read: impl FnOnce() -> Result<Option<String>, &'static str>,
) -> Result<(), &'static str> {
    let vault = OfflineVault::default();
    vault.fail_delete.set(true);
    super::restore_saved_key_with_vault(store, worker, read, &vault)
}

pub(super) fn persist_connection_profile(
    store: &Store,
    saved: &mut super::ConnectionProfiles,
    id: &str,
    connection: super::SystemOneConnection,
    credential: Option<String>,
    vault: &impl CredentialVault,
    worker: &WorkerHandle,
) -> Result<(super::SystemOneConnection, Option<String>, bool), &'static str> {
    if super::legacy_removal_pending(store) {
        let prepared = super::begin_legacy_removal(store, worker)?;
        super::finish_legacy_removal(store, vault, &prepared)?;
    }
    let (_, changed) = save_and_use(
        store,
        worker,
        super::SystemOneDraft {
            connection_id: id.to_owned(),
            connection,
            credential,
        },
        vault,
    )?;
    *saved = super::profiles(store)?;
    let connection = saved.profiles[id].clone();
    let credential = super::resolve_credential(&connection, None, vault)?;
    Ok((connection, credential, changed))
}

#[derive(Default)]
struct IdentityVault {
    values: std::collections::BTreeMap<Option<String>, String>,
    reads: std::cell::RefCell<Vec<Option<String>>>,
}

impl CredentialVault for IdentityVault {
    type Error = &'static str;

    fn read(&self, id: Option<&str>) -> Result<Option<String>, Self::Error> {
        let id = id.map(str::to_owned);
        self.reads.borrow_mut().push(id.clone());
        Ok(self.values.get(&id).cloned())
    }

    fn write(&self, _: Option<&str>, _: &str) -> Result<(), Self::Error> {
        panic!("Reading or saving an unchanged profile must not write credentials")
    }

    fn delete(&self, _: Option<&str>) -> Result<(), Self::Error> {
        panic!("Reading or saving an unchanged profile must not delete credentials")
    }

    fn is_missing(error: &Self::Error) -> bool {
        *error == "missing"
    }
    fn write_failure(_: &Self::Error) -> &'static str {
        "Unexpected credential write."
    }
}

#[test]
fn raw_native_profiles_use_the_same_vault_id_without_read_migrations_or_noop_saves() {
    use crate::jev::config::{CredentialReference, SystemOneConnection};
    for credential in [
        CredentialReference::LegacyTypeSafe,
        CredentialReference::Connection("work-vault".into()),
    ] {
        for state in [
            "enabled",
            "disabled",
            "rejected",
            "pending",
            "legacy-pending",
            "inactive",
        ] {
            let store = Store::open_in_memory(Path::new("synthetic-native-profile-read")).unwrap();
            let mut native = serde_json::to_value(SystemOneConnection::default()).unwrap();
            native["model"] = serde_json::json!("jev-1.13.0");
            native["modelRevision"] = serde_json::json!("obsolete-pin");
            native["credential"] = serde_json::json!(credential);
            native["revision"] = serde_json::json!(17);
            let active_id = if state == "inactive" { "proxy" } else { "work" };
            let raw = serde_json::json!({"activeId":active_id,"profiles":{"work":native,"proxy":custom_connection()}}).to_string();
            store.set_internal_value(CONNECTION_PROFILES_KEY, &raw);
            store.set_internal_value(super::ACTIVE_CONNECTION_KEY, active_id);
            store.set_internal_value(PROVIDER_MIGRATION_KEY, "1");
            store.set_internal_value(super::AUTOMATIC_LIMITS_MIGRATION_KEY, "1");
            store.set_internal_value(SAVED_KEY_KEY, "true");
            store.set_internal_value(
                YAML_DISCOVERY_ENABLED_KEY,
                if state == "disabled" { "false" } else { "true" },
            );
            store.set_internal_value(
                AUTH_REJECTED_KEY,
                if state == "rejected" { "true" } else { "false" },
            );
            if state == "pending" {
                store.set_internal_value(CONNECTION_PENDING_KEY, "work");
            }
            if state == "legacy-pending" {
                store.set_internal_value(CREDENTIAL_CHANGE_PENDING_KEY, "true");
            }
            insert_unfinished_assessment(&store, "pending");
            insert_accepted_status_and_source(&store);
            super::operation::metadata(&store).unwrap();
            let before = source_and_status_snapshot(&store);
            let changes_before: i64 = store
                .lock()
                .query_row("SELECT total_changes()", [], |row| row.get(0))
                .unwrap();
            let id = match &credential {
                CredentialReference::LegacyTypeSafe => None,
                CredentialReference::Connection(id) => Some(id.clone()),
            };
            let vault = IdentityVault {
                values: [
                    (id.clone(), "correct-secret".into()),
                    (Some("wrong-key".into()), "wrong-secret".into()),
                ]
                .into(),
                ..Default::default()
            };
            let worker = WorkerHandle::default();
            for _ in 0..3 {
                let profiles = super::profiles(&store).unwrap();
                assert_eq!(profiles.active_id.as_deref(), Some(active_id));
                let effective = &profiles.profiles["work"];
                assert_eq!(
                    effective.model,
                    antiburn_local::analysis::jev::NATIVE_MODEL_ALIAS
                );
                assert_eq!(effective.model_revision, None);
                assert_eq!(effective.revision, 17);
                assert_eq!(effective.credential, Some(credential.clone()));
                assert_eq!(effective.context_override, None);
            }
            let result =
                super::restore_saved_key_with_vault(&store, &worker, || vault.read(None), &vault);
            if state == "legacy-pending" {
                assert!(result.is_err());
            } else {
                result.unwrap();
            }
            if state == "enabled" {
                assert_eq!(vault.reads.borrow().as_slice(), std::slice::from_ref(&id));
                assert_eq!(
                    worker.execution_snapshot().unwrap().credential.as_deref(),
                    Some("correct-secret")
                );
                let generation = worker.execution_snapshot().unwrap().generation;
                let connection = active_connection(&store).unwrap();
                let (settings, changed) = super::save_connection(
                    &store,
                    &worker,
                    super::SystemOneDraft {
                        connection_id: "work".into(),
                        connection,
                        credential: None,
                    },
                    &vault,
                )
                .unwrap();
                assert!(!changed);
                assert_eq!(settings.active_id.as_deref(), Some("work"));
                assert_eq!(settings.profiles["work"].revision, 17);
                assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
                assert!(vault.reads.borrow().iter().all(|read| *read == id));
            } else {
                assert!(vault.reads.borrow().is_empty());
            }
            assert_eq!(
                store.internal_value(CONNECTION_PROFILES_KEY).as_deref(),
                Some(raw.as_str())
            );
            assert_eq!(
                store
                    .internal_value(super::ACTIVE_CONNECTION_KEY)
                    .as_deref(),
                Some(active_id)
            );
            assert_eq!(
                store.internal_value(PROVIDER_MIGRATION_KEY).as_deref(),
                Some("1")
            );
            assert_eq!(
                unfinished_assessment_state(&store),
                if state == "pending" {
                    "cancelled"
                } else {
                    "pending"
                }
            );
            assert_eq!(source_and_status_snapshot(&store), before);
            let changes_after: i64 = store
                .lock()
                .query_row("SELECT total_changes()", [], |row| row.get(0))
                .unwrap();
            if state == "pending" {
                assert!(changes_after > changes_before);
            } else {
                assert_eq!(changes_after, changes_before);
            }
        }
    }
}

fn saved_custom(active: bool) -> (Store, WorkerHandle, OfflineVault) {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-credential-lifecycle")).unwrap();
    migrate_provider_state(&store).unwrap();
    let worker = WorkerHandle::default();
    let vault = OfflineVault::default();
    store.set_internal_value(
        YAML_DISCOVERY_ENABLED_KEY,
        if active { "true" } else { "false" },
    );
    super::save_connection(
        &store,
        &worker,
        super::SystemOneDraft {
            connection_id: "proxy".into(),
            connection: custom_connection(),
            credential: Some("synthetic-secret".into()),
        },
        &vault,
    )
    .unwrap();
    (store, worker, vault)
}

#[test]
fn unknown_custom_can_stay_saved_without_manual_limits() {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-custom-limits")).unwrap();
    migrate_provider_state(&store).unwrap();
    let worker = WorkerHandle::default();
    let vault = OfflineVault::default();
    for limits in [
        None,
        Some(crate::jev::config::ContextLimitOverride::default()),
        Some(crate::jev::config::ContextLimitOverride {
            state_and_longest_question_tokens: Some(8192),
            ..crate::jev::config::ContextLimitOverride::default()
        }),
    ] {
        let mut connection = custom_connection();
        connection.context_override = limits;
        let (saved, _) = super::save_connection(
            &store,
            &worker,
            super::SystemOneDraft {
                connection_id: "proxy".into(),
                connection,
                credential: Some("synthetic-secret".into()),
            },
            &vault,
        )
        .unwrap();
        assert_eq!(saved.active_id.as_deref(), Some("proxy"));
        assert_eq!(saved.profiles["proxy"].context_override, None);
        assert_eq!(
            saved.profiles["proxy"]
                .capabilities()
                .unwrap()
                .usable_input_tokens(),
            None
        );
        assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
    }
}

#[test]
fn installed_ollama_lookup_validates_provider_url_and_explicit_credentials_before_io() {
    let draft = super::SystemOneDraft {
        connection_id: "local".into(),
        connection: crate::jev::config::SystemOneConnection {
            provider: super::SystemOneProvider::Ollama,
            endpoint: super::SystemOneEndpoint::BaseUrl("http://127.0.0.1:11434".into()),
            model: String::new(),
            credential: None,
            ..Default::default()
        },
        credential: None,
    };
    let vault = OfflineVault::default();
    assert!(super::ollama_model_lookup_client(draft.clone(), &vault).is_ok());
    assert!(draft.connection.validate().is_err());
    for provider in [
        super::SystemOneProvider::Jev,
        super::SystemOneProvider::Cloudflare,
        super::SystemOneProvider::Custom,
    ] {
        let mut invalid = draft.clone();
        invalid.connection.provider = provider;
        assert!(super::ollama_model_lookup_client(invalid, &vault).is_err());
    }
    for base in [
        "file:///model",
        "https://",
        "http://user@example.com",
        "http://127.0.0.1:11434?token=secret",
        "http://127.0.0.1:11434#fragment",
    ] {
        let mut invalid = draft.clone();
        invalid.connection.endpoint = super::SystemOneEndpoint::BaseUrl(base.into());
        assert!(super::ollama_model_lookup_client(invalid, &vault).is_err());
    }
    let mut referenced = draft.clone();
    referenced.connection.credential = Some(crate::jev::config::CredentialReference::Connection(
        "saved-local".into(),
    ));
    assert!(
        super::ollama_model_lookup_client(referenced.clone(), &vault)
            .unwrap_err()
            .contains("saved credential is missing")
    );
    vault.fail_read.set(true);
    assert_eq!(
        super::ollama_model_lookup_client(referenced.clone(), &vault).unwrap_err(),
        "Credential storage is unavailable."
    );
    assert!(super::ollama_model_lookup_client(draft, &vault).is_ok());
    referenced.credential = Some("".into());
    assert_eq!(
        super::ollama_model_lookup_client(referenced, &vault).unwrap_err(),
        "Enter a valid connection credential."
    );
}

#[tokio::test]
async fn installed_ollama_lookup_uses_the_exact_saved_or_draft_credential() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    for mode in ["saved", "draft", "keyless"] {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let expected = match mode { "saved" => Some("saved-secret"), "draft" => Some("draft-secret"), _ => None };
            let draft = super::SystemOneDraft {
                connection_id: "new-name".into(),
                connection: crate::jev::config::SystemOneConnection {
                    provider: super::SystemOneProvider::Ollama,
                    endpoint: super::SystemOneEndpoint::BaseUrl(format!("http://{}/local", listener.local_addr().unwrap())),
                    model: String::new(),
                    credential: (mode != "keyless").then(|| crate::jev::config::CredentialReference::Connection("saved-slot".into())),
                    ..Default::default()
                },
                credential: (mode == "draft").then(|| "draft-secret".into()),
            };
            let vault = IdentityVault {
                values: [(Some("saved-slot".into()), "saved-secret".into()), (Some("new-name".into()), "wrong-secret".into()), (None, "wrong-legacy-secret".into())].into(),
                ..Default::default()
            };
            let client = super::ollama_model_lookup_client(draft, &vault).unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                    assert!(request.len() <= 8192);
                    if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") { break; }
                }
                let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
                assert!(request.starts_with("get /local/api/tags http/1.1\r\n"));
                match expected {
                    Some(secret) => assert!(request.contains(&format!("authorization: bearer {secret}"))),
                    None => assert!(!request.contains("authorization:")),
                }
                let body = r#"{"models":[{"name":"local:9b","digest":"synthetic-digest"}]}"#;
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            });
            assert_eq!(client.list_installed_models().await.unwrap()[0].id, "local:9b");
            server.await.unwrap();
            let expected_reads: Vec<Option<String>> = if mode == "saved" { vec![Some("saved-slot".into())] } else { vec![] };
            assert_eq!(vault.reads.borrow().as_slice(), expected_reads);
        }).await.unwrap();
    }
}

#[test]
fn automatic_limits_migrate_saved_overrides_once_without_rewriting_dispatched_work() {
    use crate::jev::config::{ContextLimitOverride, SystemOneConnection, SystemOneResponseMode};

    for (state, attempt_state, outcome) in [
        ("pending", "prepared", None),
        ("claimed", "prepared", None),
        ("dispatching", "dispatch_intent", None),
        ("dispatching", "settled", Some("unknown")),
        ("answered", "settled", Some("answer")),
    ] {
        let store = Store::open_in_memory(Path::new("synthetic-automatic-limits")).unwrap();
        migrate_provider_state(&store).unwrap();
        let mut saved = super::profiles(&store).unwrap();
        for (id, provider, endpoint, model, response_mode) in [
            (
                "jev",
                super::SystemOneProvider::Jev,
                super::SystemOneEndpoint::ProviderDefault,
                "jev-latest",
                SystemOneResponseMode::Direct,
            ),
            (
                "local",
                super::SystemOneProvider::Ollama,
                super::SystemOneEndpoint::BaseUrl("http://127.0.0.1:11434".into()),
                "selected:9b",
                SystemOneResponseMode::Direct,
            ),
            (
                "cloudflare",
                super::SystemOneProvider::Cloudflare,
                super::SystemOneEndpoint::CloudflareAccount("account".into()),
                "clef-flash",
                SystemOneResponseMode::CloudflareEnvelope,
            ),
            (
                "custom",
                super::SystemOneProvider::Custom,
                super::SystemOneEndpoint::ExactUrl(
                    "https://proxy.example/infer?route=selected".into(),
                ),
                "selected-model",
                SystemOneResponseMode::Direct,
            ),
        ] {
            saved.profiles.insert(
                id.into(),
                SystemOneConnection {
                    provider,
                    endpoint,
                    model: model.into(),
                    response_mode,
                    revision: 17,
                    credential: Some(crate::jev::config::CredentialReference::Connection(
                        id.into(),
                    )),
                    context_override: Some(ContextLimitOverride {
                        total_input_tokens: Some(8192),
                        state_and_longest_question_tokens: Some(6144),
                        runtime_context_tokens: Some(8192),
                    }),
                    ..Default::default()
                },
            );
        }
        saved.active_id = Some("custom".into());
        super::save_profiles(&store, &saved).unwrap();
        store.set_internal_value(YAML_DISCOVERY_ENABLED_KEY, "true");
        store.set_internal_value(AUTH_REJECTED_KEY, "true");
        insert_unfinished_assessment(&store, state);
        insert_accepted_status_and_source(&store);
        let protected = source_and_status_snapshot(&store);
        let preparation = serde_json::json!({"connection": saved.profiles["custom"], "request": {"model": "selected-model", "state": "saved evidence"}}).to_string();
        store
            .lock()
            .execute(
                "UPDATE burn_check_candidate SET preparation_json = ?1",
                [&preparation],
            )
            .unwrap();
        store.lock().execute(
            "INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, decision_name, attempt_number, state, outcome, updated_at_epoch)
             VALUES ('saved-attempt', 'synthetic-candidate', 'review', 1, ?1, ?2, 1)",
            rusqlite::params![attempt_state, outcome],
        ).unwrap();
        store.lock().execute("INSERT INTO burn_check_decision_usage (attempt_id, usage_known) VALUES ('saved-attempt', 0)", []).unwrap();
        let before_attempt: String = store.lock().query_row(
            "SELECT json_array(state, outcome, attempt_number, updated_at_epoch) FROM burn_check_evaluation_attempt", [], |row| row.get(0),
        ).unwrap();
        let worker = WorkerHandle::default();
        for _ in 0..2 {
            super::migrate_automatic_limits(&store, &worker).unwrap();
            let migrated = super::profiles(&store).unwrap();
            assert_eq!(migrated.active_id, saved.active_id);
            for (id, previous) in &saved.profiles {
                let mut expected = previous.clone();
                expected.context_override = None;
                expected.revision = 18;
                assert_eq!(migrated.profiles[id], expected);
            }
            assert_eq!(unfinished_assessment_state(&store), "cancelled");
            assert_eq!(source_and_status_snapshot(&store), protected);
            assert_eq!(store.lock().query_row("SELECT preparation_json FROM burn_check_candidate WHERE candidate_id = 'synthetic-candidate'", [], |row| row.get::<_, String>(0)).unwrap(), preparation);
            assert_eq!(store.lock().query_row("SELECT json_array(state, outcome, attempt_number, updated_at_epoch) FROM burn_check_evaluation_attempt", [], |row| row.get::<_, String>(0)).unwrap(), before_attempt);
            assert_eq!(
                store
                    .lock()
                    .query_row(
                        "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                        [],
                        |row| row.get::<_, u64>(0)
                    )
                    .unwrap(),
                1
            );
            assert_eq!(store.lock().query_row("SELECT usage_known FROM burn_check_decision_usage WHERE attempt_id = 'saved-attempt'", [], |row| row.get::<_, u64>(0)).unwrap(), 0);
            assert_eq!(
                store.internal_value(AUTH_REJECTED_KEY).as_deref(),
                Some("true")
            );
            assert!(super::yaml_discovery_enabled(&store));
            assert!(!worker.is_available());
        }
    }
}

#[test]
fn automatic_limits_correct_old_flash_profiles_without_changing_clef_or_unknown_custom() {
    let store = Store::open_in_memory(Path::new("synthetic-flash-profile-migration")).unwrap();
    migrate_provider_state(&store).unwrap();
    let mut saved = super::profiles(&store).unwrap();
    for model in ["clef", "clef-flash"] {
        saved.profiles.insert(
            model.into(),
            crate::jev::config::SystemOneConnection {
                provider: super::SystemOneProvider::Cloudflare,
                endpoint: super::SystemOneEndpoint::CloudflareAccount("account".into()),
                model: model.into(),
                ..custom_connection()
            },
        );
        saved.profiles.insert(
            format!("custom-{model}"),
            crate::jev::config::SystemOneConnection {
                endpoint: super::SystemOneEndpoint::ExactUrl(
                    crate::jev_cloudflare::account_endpoint("account", model).unwrap(),
                ),
                response_mode: crate::jev::config::SystemOneResponseMode::CloudflareEnvelope,
                model: model.into(),
                ..custom_connection()
            },
        );
    }
    saved.profiles.insert("unknown".into(), custom_connection());
    super::save_profiles(&store, &saved).unwrap();
    insert_unfinished_assessment(&store, "dispatching");
    super::migrate_automatic_limits(&store, &WorkerHandle::default()).unwrap();
    let migrated = super::profiles(&store).unwrap();
    for (id, previous) in saved.profiles {
        let mut expected = previous;
        if id == "clef-flash" || id == "custom-clef-flash" {
            expected.revision += 1;
            assert_eq!(
                migrated.profiles[&id]
                    .capabilities()
                    .unwrap()
                    .total_input_tokens
                    .value,
                Some(24_576)
            );
        }
        assert_eq!(migrated.profiles[&id], expected);
    }
    assert_eq!(unfinished_assessment_state(&store), "dispatching");
}

#[test]
fn automatic_limits_active_flash_correction_cancels_unfinished_work_once() {
    for state in ["pending", "claimed", "dispatching", "answered"] {
        let store = Store::open_in_memory(Path::new("synthetic-active-flash-migration")).unwrap();
        migrate_provider_state(&store).unwrap();
        let mut saved = super::profiles(&store).unwrap();
        let connection = crate::jev::config::SystemOneConnection {
            provider: super::SystemOneProvider::Cloudflare,
            endpoint: super::SystemOneEndpoint::CloudflareAccount("account".into()),
            model: "clef-flash".into(),
            ..custom_connection()
        };
        saved.profiles.insert("flash".into(), connection.clone());
        saved.active_id = Some("flash".into());
        super::save_profiles(&store, &saved).unwrap();
        let worker = WorkerHandle::default();
        worker
            .set_system_one_connection(connection, Some("synthetic-secret".into()))
            .unwrap();
        let generation = worker.execution_snapshot().unwrap().generation;
        insert_unfinished_assessment(&store, state);
        for _ in 0..2 {
            super::migrate_automatic_limits(&store, &worker).unwrap();
            assert_eq!(unfinished_assessment_state(&store), "cancelled");
            assert!(!worker.is_available());
            assert!(!worker.key_is_current(generation));
            assert_eq!(
                super::profiles(&store).unwrap().profiles["flash"].revision,
                2
            );
        }
    }
}

#[test]
fn automatic_limits_inactive_updates_preserve_active_work_and_recovery() {
    use std::sync::Arc;

    use crate::jev::config::{ContextLimitOverride, SystemOneConnection};
    use crate::store::burn_checks::StoreCheckStore;
    use antiburn_checks::check_store::{CheckStore, RecoveryWork};
    use antiburn_checks::decision::{DecisionAttempt, attempt_id};
    use antiburn_checks::definition_lifecycle::{EffectiveActivation, LifecycleTransition};
    use antiburn_checks::domain::{
        ActivationId, ActivationScope, BranchId, FrozenCandidate, FrozenContext, Generation,
        ObservationPosition, RunScope,
    };
    use antiburn_checks::expression_eval::EvaluationBudget;
    use antiburn_checks::fit::PreparationRecord;
    use antiburn_checks::prepared_assessment::{PreparedAssessment, PreparedBlock};
    use antiburn_checks::results::{
        ChoiceResult, DecisionResult, EvaluatedAssessment, NamedAnswer, QuestionResult,
    };
    use antiburn_checks::source::PublicationCursor;
    use serde_json::json;

    const SOURCE: &str = "name: Migration\non:\n  tool_result:\ndecisions:\n  review:\n    input: {activity: \"${{ message.tool.result.status }}\"}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\nactions:\n  - type: status\n    if: ${{ decisions.review.details.verdict.probabilities.yes > 0.9 }}\n    input: {result: failure}\n";

    for inactive_change in ["override", "flash"] {
        for (state, attempt_state, outcome) in [
            ("pending", "settled", Some("safe_transient")),
            ("claimed", "settled", Some("safe_transient")),
            ("dispatching", "settled", Some("safe_transient")),
            ("dispatching", "dispatch_intent", None),
            ("dispatching", "settled", Some("unknown")),
            ("answered", "settled", Some("answer")),
        ] {
            let store = Arc::new(
                Store::open_in_memory(Path::new("synthetic-inactive-policy-migration")).unwrap(),
            );
            migrate_provider_state(&store).unwrap();
            let mut saved = super::profiles(&store).unwrap();
            let inactive = SystemOneConnection {
                provider: super::SystemOneProvider::Cloudflare,
                endpoint: super::SystemOneEndpoint::CloudflareAccount("account".into()),
                model: if inactive_change == "flash" {
                    "clef-flash"
                } else {
                    "clef"
                }
                .into(),
                revision: 7,
                context_override: (inactive_change == "override").then_some(ContextLimitOverride {
                    total_input_tokens: Some(8192),
                    ..Default::default()
                }),
                ..Default::default()
            };
            saved.profiles.insert("inactive".into(), inactive.clone());
            super::save_profiles(&store, &saved).unwrap();
            store.set_internal_value(YAML_DISCOVERY_ENABLED_KEY, "true");
            store.set_internal_value(SAVED_KEY_KEY, "true");
            let worker = WorkerHandle::default();
            worker
                .set_system_one_connection(
                    saved.profiles["jev"].clone(),
                    Some("synthetic-key".into()),
                )
                .unwrap();
            let before_worker = worker.execution_snapshot().unwrap();
            let scope = ActivationScope {
                environment: "native".into(),
                session_cwd: "/repo".into(),
                run: RunScope::Live,
            };
            let adapter =
                StoreCheckStore::new(store.clone(), "owner".into(), scope.clone()).unwrap();
            let active = EffectiveActivation {
                id: ActivationId {
                    scope,
                    definition_id: "migration".into(),
                    generation: Generation(1),
                },
                revision: [1; 32],
                definition_scope: antiburn_checks::discovery::DefinitionScope::Global,
                source_paths: vec![],
                project_root: Some("/repo".into()),
                compiled: Arc::new(antiburn_checks::compiler::compile(SOURCE).unwrap()),
                bootstrap_observation_position: ObservationPosition(11),
                bootstrap_publication: PublicationCursor(12),
            };
            adapter
                .activate(
                    "migration",
                    &LifecycleTransition::Activated(active.clone()),
                    Some(&active),
                )
                .unwrap();
            let candidate = FrozenCandidate {
                activation: active.id.clone(),
                branch: BranchId("branch".into()),
                trigger_position: ObservationPosition(13),
                messages: vec![],
                context: FrozenContext {
                    value: json!({}),
                    dependencies: vec![],
                },
                dependencies: vec![],
                fingerprint: [7; 32],
            };
            let candidate_id = "07".repeat(32);
            let preparation = PreparationRecord::Ready(
                PreparedAssessment::new(
                    "synthetic-profile".into(),
                    EvaluationBudget::default(),
                    0,
                    vec![PreparedBlock {
                        decision: "review".into(),
                        original_input: json!({}),
                        input: json!({}),
                        request: json!({"exact": "saved"}),
                        report: Default::default(),
                    }],
                )
                .unwrap(),
            );
            let block = &active.compiled.decisions[0];
            let requirements = active.compiled.question_requirements();
            let result = DecisionResult::new(
                block,
                &requirements,
                vec![
                    NamedAnswer::new(
                        "verdict",
                        QuestionResult::Choice(
                            ChoiceResult::new(
                                &block.outputs[0].question,
                                "yes",
                                Some([("yes".into(), 0.99), ("no".into(), 0.01)].into()),
                                None,
                            )
                            .unwrap(),
                        ),
                    )
                    .unwrap(),
                ],
                1,
            )
            .unwrap();
            let evaluated = EvaluatedAssessment::new(
                &active.compiled.decisions,
                &requirements,
                vec![result.clone()],
            )
            .unwrap();
            let saved_attempt_id = attempt_id(
                &candidate.fingerprint,
                &DecisionAttempt::new("review", 1).unwrap(),
            );
            store.lock().execute(
                "INSERT INTO burn_check_candidate (candidate_id, check_id, activation_scope_key, generation, branch_key,
                 session_key_json, trigger_position, selected_publication_head, dependencies_json, frozen_input_json,
                 input_fingerprint, state, claim_owner, claim_fence, preparation_json, answer_json, created_at_epoch, updated_at_epoch)
                 VALUES (?1, 'migration', ?2, 1, 'branch', '{}', 13, 12, '[]', ?3, 'synthetic-input', ?4, 'owner', 1, ?5, ?6, 1, 1)",
                rusqlite::params![candidate_id, StoreCheckStore::activation_scope_key(&active.id.scope), serde_json::to_string(&candidate).unwrap(), state,
                    serde_json::to_string(&preparation).unwrap(), (state == "answered").then(|| serde_json::to_string(&evaluated).unwrap())],
            ).unwrap();
            store.lock().execute(
                "INSERT INTO burn_check_evaluation_attempt (attempt_id, candidate_id, decision_name, attempt_number, state, outcome, response_json, updated_at_epoch)
                 VALUES (?1, ?2, 'review', 1, ?3, ?4, ?5, 1)",
                rusqlite::params![saved_attempt_id, candidate_id, attempt_state, outcome,
                    (state == "answered").then(|| serde_json::to_string(&result).unwrap())],
            ).unwrap();
            store.lock().execute("INSERT INTO burn_check_decision_usage (attempt_id, usage_known) VALUES (?1, 0)", [&saved_attempt_id]).unwrap();
            let work_tables = [
                "burn_check_candidate",
                "burn_check_evaluation_attempt",
                "burn_check_decision_usage",
                "burn_check_assessment_history",
            ];
            let before_work = table_snapshot(&store, &work_tables);
            let before_source = source_and_status_snapshot(&store);
            let before_progress = adapter.load_source_progress(&active).unwrap();
            for _ in 0..2 {
                super::migrate_automatic_limits(&store, &worker).unwrap();
                let migrated = super::profiles(&store).unwrap();
                let mut expected_inactive = inactive.clone();
                expected_inactive.revision = 8;
                expected_inactive.context_override = None;
                assert_eq!(migrated.profiles["inactive"], expected_inactive);
                assert_eq!(migrated.profiles["jev"], saved.profiles["jev"]);
                assert_eq!(migrated.active_id, saved.active_id);
                assert_eq!(table_snapshot(&store, &work_tables), before_work);
                assert_eq!(source_and_status_snapshot(&store), before_source);
                assert_eq!(
                    adapter.load_source_progress(&active).unwrap(),
                    before_progress
                );
                let current_worker = worker.execution_snapshot().unwrap();
                assert_eq!(current_worker.connection, before_worker.connection);
                assert_eq!(current_worker.credential, before_worker.credential);
                assert_eq!(current_worker.generation, before_worker.generation);
                assert!(worker.key_is_current(before_worker.generation));
            }
            super::restore_saved_key_with_vault(
                &store,
                &worker,
                || Ok(Some("synthetic-key".into())),
                &OfflineVault::default(),
            )
            .unwrap();
            assert_eq!(table_snapshot(&store, &work_tables), before_work);
            assert_eq!(source_and_status_snapshot(&store), before_source);
            assert_eq!(
                worker.execution_snapshot().unwrap().connection,
                before_worker.connection
            );
            let recovered =
                StoreCheckStore::new(store.clone(), "restarted".into(), active.id.scope.clone())
                    .unwrap();
            recovered.register_activation(&active).unwrap();
            let records = recovered.load_recovery(None, 10).unwrap();
            match outcome {
                Some("safe_transient") => assert!(
                    matches!(&records[..], [record] if matches!(&record.work, RecoveryWork::Pending { candidate: restored, resumed: true } if restored == &candidate))
                ),
                Some("answer") => assert!(
                    matches!(&records[..], [record] if matches!(&record.work, RecoveryWork::Answered { claimed, evaluated: restored } if claimed.candidate == candidate && restored == &evaluated))
                ),
                _ => assert!(records.is_empty()),
            }
            assert_eq!(
                recovered.load_source_progress(&active).unwrap(),
                before_progress
            );
            assert_eq!(
                store
                    .lock()
                    .query_row(
                        "SELECT COUNT(*) FROM burn_check_evaluation_attempt",
                        [],
                        |row| row.get::<_, u64>(0)
                    )
                    .unwrap(),
                1
            );
            assert_eq!(
                store
                    .lock()
                    .query_row(
                        "SELECT usage_known FROM burn_check_decision_usage WHERE attempt_id = ?1",
                        [&saved_attempt_id],
                        |row| row.get::<_, u64>(0)
                    )
                    .unwrap(),
                0
            );
        }
    }
}

#[test]
fn automatic_limits_migration_failures_stay_paused_and_retry_without_partial_metadata() {
    for failure in ["revision", "cancellation", "save"] {
        let (store, worker, vault) = saved_custom(true);
        let mut saved = super::profiles(&store).unwrap();
        let previous = saved.profiles.get_mut("proxy").unwrap();
        previous.context_override = Some(crate::jev::config::ContextLimitOverride {
            total_input_tokens: Some(8192),
            ..Default::default()
        });
        if failure == "revision" {
            previous.revision = u64::MAX;
        }
        super::save_profiles(&store, &saved).unwrap();
        insert_unfinished_assessment(&store, "pending");
        let before = store.internal_value(CONNECTION_PROFILES_KEY);
        if failure == "cancellation" {
            store.lock().execute_batch("CREATE TEMP TRIGGER fail_migration BEFORE UPDATE ON burn_check_candidate WHEN NEW.state = 'cancelled' BEGIN SELECT RAISE(ABORT, 'injected cancellation failure'); END;").unwrap();
        } else if failure == "save" {
            fail_profile_writes(&store);
        }
        assert!(
            super::restore_saved_key_with_vault(
                &store,
                &worker,
                || panic!("migration must not read a credential"),
                &vault
            )
            .is_err()
        );
        assert!(!worker.is_available());
        assert_eq!(store.internal_value(CONNECTION_PROFILES_KEY), before);
        assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
        assert_eq!(
            unfinished_assessment_state(&store),
            if failure == "save" {
                "cancelled"
            } else {
                "pending"
            }
        );
        if failure == "cancellation" {
            store
                .lock()
                .execute_batch("DROP TRIGGER fail_migration")
                .unwrap();
        } else if failure == "save" {
            store
                .lock()
                .execute_batch("DROP TRIGGER fail_profiles")
                .unwrap();
        } else {
            saved.profiles.get_mut("proxy").unwrap().revision = 1;
            super::save_profiles(&store, &saved).unwrap();
        }
        super::migrate_automatic_limits(&store, &worker).unwrap();
        assert_eq!(
            super::profiles(&store).unwrap().profiles["proxy"].context_override,
            None
        );
        assert_eq!(unfinished_assessment_state(&store), "cancelled");
    }
}

#[test]
fn provider_operation_outcomes_require_real_saved_transitions() {
    let (store, worker, vault) = saved_custom(true);
    let draft = super::SystemOneDraft {
        connection_id: "proxy".into(),
        connection: custom_connection(),
        credential: None,
    };
    assert!(
        !super::save_connection(&store, &worker, draft.clone(), &vault)
            .unwrap()
            .1
    );
    let mut replacement = draft;
    replacement.credential = Some("replacement-secret".into());
    assert!(
        super::save_connection(&store, &worker, replacement, &vault)
            .unwrap()
            .1
    );
    assert!(
        !super::switch_connection(&store, &worker, "proxy", &vault)
            .unwrap()
            .1
    );
    let saved = super::profiles(&store).unwrap();
    super::save_connection(
        &store,
        &worker,
        super::SystemOneDraft {
            connection_id: "alias".into(),
            connection: saved.profiles["proxy"].clone(),
            credential: None,
        },
        &vault,
    )
    .unwrap();
    assert!(
        super::switch_connection(&store, &worker, "proxy", &vault)
            .unwrap()
            .1
    );
    assert_eq!(
        super::remove_connection_credential(&store, &worker, "proxy", &vault).unwrap(),
        Some(super::SystemOneProvider::Custom)
    );
    assert_eq!(
        super::remove_connection_credential(&store, &worker, "proxy", &vault).unwrap(),
        None
    );
}

#[test]
fn saved_revisions_require_matching_edits_and_advance_only_for_profile_changes() {
    let (store, worker, vault) = saved_custom(true);
    let initial_generation = worker.execution_snapshot().unwrap().generation;
    for revision in [0, 1, u64::MAX] {
        let mut connection = active_connection(&store).unwrap();
        connection.revision = revision;
        connection.context_override = Some(crate::jev::config::ContextLimitOverride {
            total_input_tokens: Some(8192),
            ..Default::default()
        });
        let result = super::save_connection_with_intent(
            &store,
            &worker,
            super::SystemOneDraft {
                connection_id: "proxy".into(),
                connection,
                credential: None,
            },
            super::SaveConnectionIntent::Update,
            REQUEST_ID,
            &vault,
        );
        if revision == 1 {
            let (settings, changed) = result.unwrap();
            assert!(!changed);
            assert_eq!(settings.profiles["proxy"].revision, 1);
        } else {
            assert!(result.unwrap_err().contains("another window"));
        }
        assert_eq!(
            worker.execution_snapshot().unwrap().generation,
            initial_generation
        );
    }
    for index in 0..5 {
        let mut connection = active_connection(&store).unwrap();
        let mut credential = None;
        match index {
            0 => {
                connection.endpoint = super::SystemOneEndpoint::ExactUrl(
                    "https://other-proxy.example/systemone".into(),
                )
            }
            1 => connection.model = "updated-model".into(),
            2 => connection.model_revision = Some("updated-model-revision".into()),
            3 => {
                connection.response_mode =
                    crate::jev::config::SystemOneResponseMode::CloudflareEnvelope
            }
            _ => credential = Some("updated-credential".into()),
        }
        let (settings, changed) = super::save_connection_with_intent(
            &store,
            &worker,
            super::SystemOneDraft {
                connection_id: "proxy".into(),
                connection,
                credential,
            },
            super::SaveConnectionIntent::Update,
            REQUEST_ID,
            &vault,
        )
        .unwrap();
        assert!(changed);
        assert_eq!(settings.profiles["proxy"].revision, index + 2);
    }
}

#[test]
fn revision_overflow_fails_before_credential_or_runtime_changes() {
    let (store, worker, vault) = saved_custom(true);
    let mut saved = super::profiles(&store).unwrap();
    saved.profiles.get_mut("proxy").unwrap().revision = u64::MAX;
    super::save_profiles(&store, &saved).unwrap();
    let generation = worker.execution_snapshot().unwrap().generation;
    assert!(
        super::save_connection_with_intent(
            &store,
            &worker,
            super::SystemOneDraft {
                connection_id: "proxy".into(),
                connection: active_connection(&store).unwrap(),
                credential: Some("replacement".into()),
            },
            super::SaveConnectionIntent::Update,
            REQUEST_ID,
            &vault,
        )
        .is_err()
    );
    assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
    assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
    assert_eq!(
        super::profiles(&store).unwrap().profiles["proxy"].revision,
        u64::MAX
    );
}

#[test]
fn legacy_removal_preflights_every_revision_before_marking_or_pausing() {
    for exhausted_id in ["jev", "legacy-alias"] {
        let store = Store::open_in_memory(Path::new("synthetic-legacy-removal-overflow")).unwrap();
        let worker = WorkerHandle::default();
        let vault = OfflineVault::default();
        super::save_legacy_key(&store, &worker, Some("synthetic-secret".into()), &vault).unwrap();
        let mut saved = super::profiles(&store).unwrap();
        saved
            .profiles
            .insert("legacy-alias".into(), saved.profiles["jev"].clone());
        saved.profiles.get_mut(exhausted_id).unwrap().revision = u64::MAX;
        super::save_profiles(&store, &saved).unwrap();
        let before = store.internal_value(CONNECTION_PROFILES_KEY);
        let before_runtime = worker.execution_snapshot().unwrap();
        let pending_before = [
            store.internal_value(CREDENTIAL_CHANGE_PENDING_KEY),
            store.internal_value(super::LEGACY_REMOVAL_PENDING_KEY),
            store.internal_value(CONNECTION_PENDING_KEY),
            store.internal_value(super::CONNECTION_REMOVAL_KEY),
        ];
        let writes_before = vault.write_calls.get();
        assert!(
            super::begin_legacy_removal(&store, &worker)
                .unwrap_err()
                .contains("revision limit")
        );
        let after_runtime = worker.execution_snapshot().unwrap();
        assert_eq!(after_runtime.generation, before_runtime.generation);
        assert_eq!(after_runtime.connection, before_runtime.connection);
        assert_eq!(after_runtime.credential, before_runtime.credential);
        assert!(super::yaml_discovery_enabled(&store));
        assert_eq!(store.internal_value(CONNECTION_PROFILES_KEY), before);
        assert_eq!(
            [
                store.internal_value(CREDENTIAL_CHANGE_PENDING_KEY),
                store.internal_value(super::LEGACY_REMOVAL_PENDING_KEY),
                store.internal_value(CONNECTION_PENDING_KEY),
                store.internal_value(super::CONNECTION_REMOVAL_KEY),
            ],
            pending_before
        );
        assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
        assert_eq!(vault.write_calls.get(), writes_before);
        assert!(vault.deleted_ids.borrow().is_empty());
        let (settings, changed) = super::save_connection(
            &store,
            &worker,
            super::SystemOneDraft {
                connection_id: "other-profile".into(),
                connection: custom_connection(),
                credential: None,
            },
            &vault,
        )
        .unwrap();
        assert!(changed);
        assert_eq!(settings.active_id.as_deref(), Some("other-profile"));
        assert!(worker.is_available());
    }
}

#[test]
fn legacy_removal_commits_its_preflight_revisions_once() {
    let store = Store::open_in_memory(Path::new("synthetic-legacy-removal-preflight")).unwrap();
    let worker = WorkerHandle::default();
    let vault = OfflineVault::default();
    super::save_legacy_key(&store, &worker, Some("synthetic-secret".into()), &vault).unwrap();
    let mut saved = super::profiles(&store).unwrap();
    let mut alias = saved.profiles["jev"].clone();
    alias.revision = 7;
    saved.profiles.insert("legacy-alias".into(), alias);
    super::save_profiles(&store, &saved).unwrap();
    let prepared = super::begin_legacy_removal(&store, &worker).unwrap();
    assert!(!worker.is_available());
    assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
    assert_eq!(
        store
            .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
            .as_deref(),
        Some("true")
    );
    super::finish_legacy_removal(&store, &vault, &prepared).unwrap();
    let saved = super::profiles(&store).unwrap();
    assert_eq!(saved.profiles["jev"].revision, 3);
    assert_eq!(saved.profiles["legacy-alias"].revision, 8);
    assert_eq!(
        store
            .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
            .as_deref(),
        Some("false")
    );
    assert!(vault.value.borrow().is_none());
}

#[test]
fn legacy_removal_marker_failure_keeps_both_markers_and_runtime_unchanged() {
    let store =
        Store::open_in_memory(Path::new("synthetic-legacy-removal-marker-failure")).unwrap();
    let worker = WorkerHandle::default();
    let vault = OfflineVault::default();
    super::save_legacy_key(&store, &worker, Some("synthetic-secret".into()), &vault).unwrap();
    let before = worker.execution_snapshot().unwrap();
    let profiles_before = store.internal_value(CONNECTION_PROFILES_KEY);
    let removal_before = store.internal_value(super::LEGACY_REMOVAL_PENDING_KEY);
    store.lock().execute_batch("CREATE TEMP TRIGGER fail_legacy_marker BEFORE UPDATE ON setting WHEN NEW.key = 'internal:typesafeCredentialChangePendingV1' BEGIN SELECT RAISE(ABORT, 'synthetic marker failure'); END;").unwrap();
    assert!(super::begin_legacy_removal(&store, &worker).is_err());
    assert_eq!(
        store
            .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
            .as_deref(),
        Some("false")
    );
    assert_eq!(
        store.internal_value(super::LEGACY_REMOVAL_PENDING_KEY),
        removal_before
    );
    assert_eq!(
        store.internal_value(CONNECTION_PROFILES_KEY),
        profiles_before
    );
    assert!(super::yaml_discovery_enabled(&store));
    assert_eq!(
        worker.execution_snapshot().unwrap().generation,
        before.generation
    );
    assert_eq!(
        worker.execution_snapshot().unwrap().credential,
        before.credential
    );
    assert_eq!(
        vault.read(None).unwrap().as_deref(),
        Some("synthetic-secret")
    );
}

#[test]
fn inactive_legacy_removal_failure_preserves_active_work_and_startup_restore() {
    for keyless in [false, true] {
        for fail_delete in [false, true] {
            let (store, worker, vault) = saved_custom(true);
            let mut saved = super::profiles(&store).unwrap();
            if keyless {
                saved.profiles.get_mut("proxy").unwrap().credential = None;
                worker
                    .set_system_one_connection(saved.profiles["proxy"].clone(), None)
                    .unwrap();
            }
            let legacy = super::SystemOneConnection::jev_default();
            let legacy_revision = legacy.revision;
            saved.profiles.insert("legacy".into(), legacy);
            super::save_profiles(&store, &saved).unwrap();
            store.set_internal_value(super::AUTOMATIC_LIMITS_MIGRATION_KEY, "1");
            vault.write(None, "shared-key").unwrap();
            store.set_internal_value(SAVED_KEY_KEY, "true");
            insert_unfinished_assessment(&store, "pending");
            let before = worker.execution_snapshot().unwrap();
            let profiles_before = store.internal_value(CONNECTION_PROFILES_KEY);
            if fail_delete {
                vault.fail_delete.set(true);
            } else {
                fail_profile_writes(&store);
            }
            let prepared = super::begin_legacy_removal(&store, &worker).unwrap();
            assert!(super::finish_legacy_removal(&store, &vault, &prepared).is_err());
            assert!(super::legacy_removal_pending(&store));
            assert_eq!(
                store
                    .internal_value(super::LEGACY_REMOVAL_PENDING_KEY)
                    .as_deref(),
                Some("true")
            );
            assert_eq!(
                store
                    .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
                    .as_deref(),
                Some("false")
            );
            assert_eq!(
                store.internal_value(CONNECTION_PROFILES_KEY),
                profiles_before
            );
            assert_eq!(vault.read(None).unwrap().as_deref(), Some("shared-key"));
            assert!(super::yaml_discovery_enabled(&store));
            assert_eq!(unfinished_assessment_state(&store), "pending");
            let current = worker.execution_snapshot().unwrap();
            assert_eq!(current.generation, before.generation);
            assert_eq!(current.connection, before.connection);
            assert_eq!(current.credential, before.credential);
            let restored = WorkerHandle::default();
            super::restore_saved_key_with_vault(
                &store,
                &restored,
                || panic!("An unrelated connection must not read the legacy credential"),
                &vault,
            )
            .unwrap();
            assert!(restored.is_available());
            assert_eq!(
                restored.execution_snapshot().unwrap().connection,
                before.connection
            );
            assert_eq!(
                restored.execution_snapshot().unwrap().credential,
                before.credential
            );
            assert!(super::legacy_removal_pending(&store));
            if !fail_delete {
                store
                    .lock()
                    .execute_batch("DROP TRIGGER fail_profiles")
                    .unwrap();
            }
            let retry = super::begin_legacy_removal(&store, &worker).unwrap();
            super::finish_legacy_removal(&store, &vault, &retry).unwrap();
            assert!(!super::legacy_removal_pending(&store));
            assert_eq!(
                super::profiles(&store).unwrap().profiles["legacy"].revision,
                legacy_revision + 1
            );
            assert_eq!(
                worker.execution_snapshot().unwrap().generation,
                before.generation
            );
            assert!(vault.read(None).unwrap().is_none());
            assert_eq!(unfinished_assessment_state(&store), "pending");
        }
    }
}

#[test]
fn inactive_legacy_removal_retry_clears_its_restart_error() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let worker = WorkerHandle::default();
    let vault = OfflineVault::default();
    super::save_legacy_key(&store, &worker, Some("shared-key".into()), &vault).unwrap();
    let mut saved = super::profiles(&store).unwrap();
    let selected = custom_connection();
    saved.profiles.insert("proxy".into(), selected.clone());
    saved.active_id = Some("proxy".into());
    super::save_profiles(&store, &saved).unwrap();
    worker
        .set_system_one_connection(selected.clone(), None)
        .unwrap();
    fail_profile_writes(&store);
    assert!(
        super::save_legacy_key(&store, &worker, Some("replacement-key".into()), &vault).is_err()
    );
    assert!(!worker.is_available());
    assert!(super::legacy_removal_pending(&store));
    drop(store);
    drop(worker);

    let reopened = Store::open(directory.path()).unwrap();
    let restarted = WorkerHandle::default();
    let startup_errors = super::StartupErrors::new();
    let result = super::restore_saved_key_with_origin(
        &reopened,
        &restarted,
        || panic!("Pending recovery must not read a legacy credential"),
        &vault,
    );
    assert!(matches!(
        result,
        Err(super::StartupRestoreFailure::LegacyCredentialChange(_))
    ));
    startup_errors.record_restore_result(&reopened, result);
    let startup_error = startup_errors.message();
    assert!(startup_error.is_some());
    assert!(startup_errors.unbound.lock().unwrap().is_none());

    vault.fail_delete.set(true);
    let prepared = super::begin_legacy_removal(&reopened, &restarted).unwrap();
    let result = super::finish_legacy_removal(&reopened, &vault, &prepared);
    assert!(result.is_err());
    startup_errors.record_legacy_removal_result(false, result);
    assert_eq!(startup_errors.message(), startup_error);
    super::restore_saved_key_with_vault(
        &reopened,
        &restarted,
        || panic!("An unrelated connection must not read a legacy credential"),
        &vault,
    )
    .unwrap();
    assert!(restarted.is_available());

    let retry = super::begin_legacy_removal(&reopened, &restarted).unwrap();
    let result = super::finish_legacy_removal(&reopened, &vault, &retry);
    assert!(result.is_ok());
    startup_errors.record_legacy_removal_result(false, result);
    assert!(!super::legacy_removal_pending(&reopened));
    assert!(startup_errors.message().is_none());
    assert_eq!(restarted.execution_snapshot().unwrap().connection, selected);
    assert!(
        super::read_check_availability(
            &reopened,
            restarted.is_available(),
            saved_key_marker(&reopened),
            startup_errors.message()
        )
        .unwrap()
        .error
        .is_none()
    );
}

#[test]
fn legacy_removal_preserves_unrelated_startup_errors() {
    for active_uses_legacy_key in [false, true] {
        for origin in [
            None,
            Some(super::StartupRecoveryOrigin::SelectedConnection),
            Some(super::StartupRecoveryOrigin::Operation(
                "unrelated-operation".into(),
            )),
        ] {
            let startup_errors = super::StartupErrors::new();
            let message = "Synthetic unrelated startup error.";
            match origin {
                None => startup_errors.set_unbound(Some(message)),
                Some(origin) => {
                    *startup_errors.recovery.lock().unwrap() =
                        Some(super::StartupRecoveryError { origin, message })
                }
            }
            startup_errors.record_legacy_removal_result(
                active_uses_legacy_key,
                Err("Synthetic removal failure."),
            );
            assert_eq!(startup_errors.message(), Some(message));
            startup_errors.record_legacy_removal_result(active_uses_legacy_key, Ok(()));
            assert_eq!(startup_errors.message(), Some(message));
        }
    }
}

#[test]
fn legacy_removal_clears_the_selected_legacy_credential_error() {
    let store = Store::open_in_memory(Path::new("synthetic-legacy-startup-error-removal")).unwrap();
    let worker = WorkerHandle::default();
    let vault = OfflineVault::default();
    super::save_legacy_key(&store, &worker, Some("shared-key".into()), &vault).unwrap();
    vault.fail_read.set(true);
    let startup_errors = super::StartupErrors::new();
    let result = super::restore_saved_key_with_origin(
        &store,
        &worker,
        || super::read_key_with_vault(&vault),
        &vault,
    );
    assert!(matches!(
        result,
        Err(super::StartupRestoreFailure::SelectedLegacyCredential(_))
    ));
    startup_errors.record_restore_result(&store, result);
    assert!(startup_errors.message().is_some());
    let prepared = super::begin_legacy_removal(&store, &worker).unwrap();
    let result = super::finish_legacy_removal(&store, &vault, &prepared);
    assert!(result.is_ok());
    startup_errors.record_legacy_removal_result(true, result);
    assert!(startup_errors.message().is_none());
    assert!(!super::legacy_removal_pending(&store));
    assert!(!worker.is_available());
}

#[test]
fn shared_credential_replacement_advances_only_the_edited_profile_revision() {
    let (store, worker, vault) = saved_custom(true);
    let mut saved = super::profiles(&store).unwrap();
    saved
        .profiles
        .insert("alias".into(), saved.profiles["proxy"].clone());
    super::save_profiles(&store, &saved).unwrap();
    super::save_connection(
        &store,
        &worker,
        super::SystemOneDraft {
            connection_id: "proxy".into(),
            connection: saved.profiles["proxy"].clone(),
            credential: Some("replacement".into()),
        },
        &vault,
    )
    .unwrap();
    let saved = super::profiles(&store).unwrap();
    assert_eq!(saved.profiles["proxy"].revision, 2);
    assert_eq!(saved.profiles["alias"].revision, 1);
    assert_ne!(
        saved.profiles["proxy"].credential,
        saved.profiles["alias"].credential
    );
}

#[test]
fn save_switch_restore_and_remove_cover_each_provider_and_custom_response_mode() {
    for (provider, response_mode, optional) in [
        (
            super::SystemOneProvider::Jev,
            crate::jev::config::SystemOneResponseMode::Direct,
            false,
        ),
        (
            super::SystemOneProvider::Cloudflare,
            crate::jev::config::SystemOneResponseMode::Direct,
            false,
        ),
        (
            super::SystemOneProvider::Ollama,
            crate::jev::config::SystemOneResponseMode::Direct,
            true,
        ),
        (
            super::SystemOneProvider::Custom,
            crate::jev::config::SystemOneResponseMode::Direct,
            true,
        ),
        (
            super::SystemOneProvider::Custom,
            crate::jev::config::SystemOneResponseMode::CloudflareEnvelope,
            true,
        ),
    ] {
        for with_credential in [false, true] {
            let store = Store::open_in_memory(Path::new("synthetic-provider-settings")).unwrap();
            store.set_internal_value(YAML_DISCOVERY_ENABLED_KEY, "true");
            let worker = WorkerHandle::default();
            let vault = OfflineVault::default();
            let mut connection = match provider {
                super::SystemOneProvider::Jev => {
                    crate::jev::config::SystemOneConnection::jev_default()
                }
                super::SystemOneProvider::Ollama => crate::jev::config::SystemOneConnection {
                    provider,
                    endpoint: super::SystemOneEndpoint::BaseUrl("http://127.0.0.1:11434".into()),
                    model: "clef-flash".into(),
                    ..custom_connection()
                },
                super::SystemOneProvider::Cloudflare => crate::jev::config::SystemOneConnection {
                    provider,
                    endpoint: super::SystemOneEndpoint::CloudflareAccount(
                        "synthetic-account".into(),
                    ),
                    model: "clef-flash".into(),
                    ..custom_connection()
                },
                super::SystemOneProvider::Custom => custom_connection(),
            };
            connection.response_mode = response_mode;
            connection.credential = None;
            connection.revision = u64::MAX;
            let result = super::save_connection(
                &store,
                &worker,
                super::SystemOneDraft {
                    connection_id: "selected".into(),
                    connection,
                    credential: with_credential.then(|| "synthetic-token".into()),
                },
                &vault,
            );
            if !optional && !with_credential {
                assert!(result.is_err());
                assert!(!worker.is_available());
                continue;
            }
            let (settings, changed) = result.unwrap();
            assert!(changed);
            assert_eq!(settings.active_id.as_deref(), Some("selected"));
            assert_eq!(settings.profiles["selected"].revision, 1);
            let snapshot = worker.execution_snapshot().unwrap();
            assert_eq!(snapshot.connection.provider, provider);
            assert_eq!(snapshot.connection.response_mode, response_mode);
            assert_eq!(snapshot.credential.is_some(), with_credential);
            super::switch_connection(&store, &worker, "selected", &vault).unwrap();
            assert_eq!(
                worker.execution_snapshot().unwrap().generation,
                snapshot.generation
            );
            let mut saved = super::profiles(&store).unwrap();
            saved.profiles.insert("other".into(), custom_connection());
            super::save_profiles(&store, &saved).unwrap();
            assert!(
                super::switch_connection(&store, &worker, "other", &vault)
                    .unwrap()
                    .1
            );
            assert!(
                super::switch_connection(&store, &worker, "selected", &vault)
                    .unwrap()
                    .1
            );
            assert_eq!(
                worker.system_one_connection(),
                settings.profiles["selected"]
            );
            let restored = WorkerHandle::default();
            super::restore_saved_key_with_vault(
                &store,
                &restored,
                || panic!("a saved connection slot must not read the legacy key"),
                &vault,
            )
            .unwrap();
            let restored_snapshot = restored.execution_snapshot().unwrap();
            assert_eq!(restored_snapshot.connection, snapshot.connection);
            assert_eq!(restored_snapshot.credential, snapshot.credential);
            assert!(
                !store
                    .internal_value(CONNECTION_PROFILES_KEY)
                    .unwrap()
                    .contains("synthetic-token")
            );
            assert_eq!(
                super::remove_connection_credential(&store, &worker, "selected", &vault).unwrap(),
                with_credential.then_some(provider),
            );
            assert_eq!(worker.is_available(), !with_credential);
            assert!(
                super::profiles(&store).unwrap().profiles["selected"]
                    .credential
                    .is_none()
            );
        }
    }
}

#[test]
fn startup_uses_saved_active_id_instead_of_the_legacy_mirror() {
    let (store, worker, vault) = saved_custom(true);
    super::save_connection(
        &store,
        &worker,
        super::SystemOneDraft {
            connection_id: "keyless".into(),
            connection: custom_connection(),
            credential: None,
        },
        &vault,
    )
    .unwrap();
    store.set_internal_value(super::ACTIVE_CONNECTION_KEY, "jev");
    restore_saved_key(&store, &worker, || {
        panic!("saved activeId must control startup")
    })
    .unwrap();
    assert_eq!(
        worker.system_one_connection().provider,
        super::SystemOneProvider::Custom
    );
    assert!(worker.execution_snapshot().unwrap().credential.is_none());
    assert_eq!(
        super::profiles(&store).unwrap().active_id.as_deref(),
        Some("keyless")
    );
}

#[test]
fn saving_without_a_new_secret_keeps_the_saved_reference_until_explicit_removal() {
    let (store, worker, vault) = saved_custom(true);
    let (_, changed) = super::save_connection(
        &store,
        &worker,
        super::SystemOneDraft {
            connection_id: "proxy".into(),
            connection: custom_connection(),
            credential: None,
        },
        &vault,
    )
    .unwrap();
    assert!(!changed);
    assert_eq!(
        worker.execution_snapshot().unwrap().credential.as_deref(),
        Some("synthetic-secret")
    );
    super::remove_connection_credential(&store, &worker, "proxy", &vault).unwrap();
    assert_eq!(
        super::profiles(&store).unwrap().profiles["proxy"].revision,
        2
    );
}

#[test]
fn legacy_key_save_advances_revision_and_failed_metadata_rolls_back_the_secret() {
    let store = Store::open_in_memory(Path::new("synthetic-legacy-key-update")).unwrap();
    let worker = WorkerHandle::default();
    let vault = OfflineVault::default();
    super::save_legacy_key(&store, &worker, Some("first-key".into()), &vault).unwrap();
    assert_eq!(active_connection(&store).unwrap().revision, 2);
    assert!(worker.is_available());
    fail_profile_writes(&store);
    assert!(super::save_legacy_key(&store, &worker, Some("replacement".into()), &vault).is_err());
    assert!(!worker.is_available());
    assert_eq!(vault.value.borrow().as_deref(), Some("first-key"));
    assert_eq!(active_connection(&store).unwrap().revision, 2);
    assert_eq!(
        store
            .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
            .as_deref(),
        Some("true")
    );
    store
        .lock()
        .execute_batch("DROP TRIGGER fail_profiles")
        .unwrap();
    super::save_legacy_key(&store, &worker, Some("replacement".into()), &vault).unwrap();
    assert_eq!(active_connection(&store).unwrap().revision, 3);
    assert!(worker.is_available());
}

fn insert_unfinished_assessment(store: &Store, state: &str) {
    store
        .lock()
        .execute(
            "INSERT INTO burn_check_candidate (
            candidate_id, check_id, activation_scope_key, generation, branch_key,
            session_key_json, trigger_position, selected_publication_head,
            dependencies_json, frozen_input_json, input_fingerprint, state,
            created_at_epoch, updated_at_epoch)
         VALUES ('synthetic-candidate', 'synthetic-check', 'synthetic-scope', 1,
            'synthetic-branch', '{}', 1, 1, '[]', '{}', 'synthetic-input', ?1, 1, 1)",
            [state],
        )
        .unwrap();
}

fn unfinished_assessment_state(store: &Store) -> String {
    store
        .lock()
        .query_row(
            "SELECT state FROM burn_check_candidate WHERE candidate_id = 'synthetic-candidate'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn insert_accepted_status_and_source(store: &Store) {
    store
        .lock()
        .execute_batch(
            "INSERT INTO burn_check_activation (
            check_id, activation_scope_key, definition_revision, enabled,
            generation, bootstrap_sequence, applied_sequence, updated_at_epoch,
            checkpoint_json, lifecycle_json)
         VALUES ('synthetic-check', 'synthetic-scope', 'synthetic-revision', 1,
            1, 0, 1, 1, '{\"sequence\":1}', '{}');
         INSERT INTO burn_check_observation (
            check_id, activation_scope_key, observation_id, session_key_json,
            publication_sequence, source_format, native_identity, native_revision,
            evidence_version, removed, payload_json)
         VALUES ('synthetic-check', 'synthetic-scope', 'synthetic-observation', '{}',
            1, 'synthetic-format', 'synthetic-native-id', 1, 'synthetic-evidence',
            0, '{\"text\":\"synthetic source\"}');
         INSERT INTO burn_check_candidate (
            candidate_id, check_id, activation_scope_key, generation, branch_key,
            session_key_json, trigger_position, selected_publication_head,
            dependencies_json, frozen_input_json, input_fingerprint, state,
            created_at_epoch, updated_at_epoch)
         SELECT 'synthetic-accepted', check_id, activation_scope_key, generation,
            branch_key, session_key_json, 0, selected_publication_head,
            dependencies_json, frozen_input_json, 'synthetic-accepted-input',
            'accepted', 1, 1
         FROM burn_check_candidate WHERE candidate_id = 'synthetic-candidate';
         INSERT INTO burn_check_assessment_history (
            assessment_id, candidate_id, check_id, activation_scope_key, generation,
            branch_key, trigger_position, outcome, actions_json, evidence_valid,
            accepted_at_epoch)
         VALUES ('synthetic-assessment', 'synthetic-accepted', 'synthetic-check',
            'synthetic-scope', 1, 'synthetic-branch', 0, 'assessed', '{}', 1, 1);
         INSERT INTO burn_check_current_status (
            check_id, activation_scope_key, branch_key, generation, status,
            assessment_id, position, evidence_valid, updated_at_epoch)
         VALUES ('synthetic-check', 'synthetic-scope', 'synthetic-branch', 1,
            'success', 'synthetic-assessment', 0, 1, 1);",
        )
        .unwrap();
}

fn source_and_status_snapshot(store: &Store) -> Vec<String> {
    table_snapshot(
        store,
        &[
            "burn_check_activation",
            "burn_check_observation",
            "burn_check_current_status",
        ],
    )
}

fn table_snapshot(store: &Store, tables: &[&str]) -> Vec<String> {
    let connection = store.lock();
    tables
        .iter()
        .map(|table| {
            let mut statement = connection
                .prepare(&format!("SELECT * FROM {table}"))
                .unwrap();
            let columns = statement.column_count();
            let rows = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| row.get::<_, rusqlite::types::Value>(column))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            format!("{table}: {rows:?}")
        })
        .collect()
}

fn assert_paused_selection_noops(store: &Store, worker: &WorkerHandle, vault: &OfflineVault) {
    let draft = super::SystemOneDraft {
        connection_id: "proxy".into(),
        connection: active_connection(store).unwrap(),
        credential: None,
    };
    assert!(
        !super::save_connection(store, worker, draft, vault)
            .unwrap()
            .1
    );
    assert!(!worker.is_available());
    assert!(
        !super::switch_connection(store, worker, "proxy", vault)
            .unwrap()
            .1
    );
    assert!(!worker.is_available());
    assert_eq!(unfinished_assessment_state(store), "pending");
}

fn run_connection_transition(
    operation: &str,
    store: &Store,
    worker: &WorkerHandle,
    vault: &OfflineVault,
) -> Result<(), &'static str> {
    match operation {
        "save" => {
            let mut connection = custom_connection();
            connection.model = "replacement-model".into();
            super::save_connection(
                store,
                worker,
                super::SystemOneDraft {
                    connection_id: "proxy".into(),
                    connection,
                    credential: Some("replacement-secret".into()),
                },
                vault,
            )
            .map(|_| ())
        }
        "switch" => super::switch_connection(store, worker, "keyless", vault).map(|_| ()),
        "remove" => super::remove_connection_credential(store, worker, "proxy", vault).map(|_| ()),
        "pause" => super::pause_model_checks(store, worker),
        _ => unreachable!(),
    }
}

#[test]
fn meaningful_transitions_cancel_unfinished_assessments_before_installation() {
    for operation in ["save", "switch", "remove", "pause"] {
        for state in ["pending", "claimed", "dispatching", "answered"] {
            let (store, worker, vault) = saved_custom(true);
            let mut saved = super::profiles(&store).unwrap();
            saved.profiles.insert("keyless".into(), custom_connection());
            super::save_profiles(&store, &saved).unwrap();
            insert_unfinished_assessment(&store, state);
            let generation = worker.execution_snapshot().unwrap().generation;
            run_connection_transition(operation, &store, &worker, &vault).unwrap();
            assert_eq!(unfinished_assessment_state(&store), "cancelled");
            assert!(!worker.key_is_current(generation));
            if operation == "switch" {
                super::switch_connection(&store, &worker, "proxy", &vault).unwrap();
                assert_eq!(unfinished_assessment_state(&store), "cancelled");
            }
        }
    }
}

#[test]
fn cancellation_failures_keep_each_transition_paused_without_replacing_the_secret() {
    for operation in ["save", "switch", "remove", "pause"] {
        let (store, worker, vault) = saved_custom(true);
        let mut saved = super::profiles(&store).unwrap();
        saved.profiles.insert("keyless".into(), custom_connection());
        super::save_profiles(&store, &saved).unwrap();
        let before = store.internal_value(CONNECTION_PROFILES_KEY);
        insert_unfinished_assessment(&store, "pending");
        store
            .lock()
            .execute_batch(
                "CREATE TEMP TRIGGER reject_settings_cancel BEFORE UPDATE ON burn_check_candidate
             WHEN NEW.state = 'cancelled'
             BEGIN SELECT RAISE(ABORT, 'injected cancellation failure'); END;",
            )
            .unwrap();
        let error = run_connection_transition(operation, &store, &worker, &vault).unwrap_err();
        assert!(error.contains("cancel unfinished"));
        assert!(!worker.is_available());
        assert!(worker.execution_snapshot().is_none());
        assert_eq!(unfinished_assessment_state(&store), "pending");
        assert_eq!(store.internal_value(CONNECTION_PROFILES_KEY), before);
        assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
        assert!(vault.deleted_ids.borrow().is_empty());
    }
}

#[test]
fn failed_master_pause_stays_paused_through_noops_restart_and_failed_enable_until_retry() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let worker = WorkerHandle::default();
    let vault = OfflineVault::default();
    store.set_internal_value(YAML_DISCOVERY_ENABLED_KEY, "true");
    super::save_connection(
        &store,
        &worker,
        super::SystemOneDraft {
            connection_id: "proxy".into(),
            connection: custom_connection(),
            credential: Some("synthetic-secret".into()),
        },
        &vault,
    )
    .unwrap();
    insert_unfinished_assessment(&store, "pending");
    insert_accepted_status_and_source(&store);
    let source_and_status = source_and_status_snapshot(&store);
    let profiles_before = store.internal_value(CONNECTION_PROFILES_KEY);
    let pending_before = store.internal_value(CONNECTION_PENDING_KEY);
    store
        .lock()
        .execute_batch(
            "CREATE TRIGGER reject_master_pause_cancel BEFORE UPDATE ON burn_check_candidate
         WHEN NEW.state = 'cancelled'
         BEGIN SELECT RAISE(ABORT, 'injected cancellation failure'); END;",
        )
        .unwrap();
    assert!(super::pause_model_checks(&store, &worker).is_err());
    assert!(!super::yaml_discovery_enabled(&store));
    assert_paused_selection_noops(&store, &worker, &vault);
    assert!(
        super::enable_model_checks(
            &store,
            &worker,
            active_connection(&store).unwrap(),
            Some("synthetic-secret".into()),
        )
        .is_err()
    );
    assert!(!worker.is_available());
    assert!(!super::yaml_discovery_enabled(&store));
    assert_eq!(source_and_status_snapshot(&store), source_and_status);
    assert_eq!(store.internal_value(CONNECTION_PENDING_KEY), pending_before);
    drop(worker);
    drop(store);

    let store = Store::open(directory.path()).unwrap();
    let worker = WorkerHandle::default();
    vault.fail_read.set(true);
    super::restore_saved_key_with_vault(
        &store,
        &worker,
        || panic!("paused restart must not read the legacy key"),
        &vault,
    )
    .unwrap();
    assert!(vault.fail_read.get());
    vault.fail_read.set(false);
    assert_paused_selection_noops(&store, &worker, &vault);
    assert!(
        super::enable_model_checks(
            &store,
            &worker,
            active_connection(&store).unwrap(),
            Some("synthetic-secret".into()),
        )
        .is_err()
    );
    assert!(!worker.is_available());
    assert!(!super::yaml_discovery_enabled(&store));
    assert_eq!(unfinished_assessment_state(&store), "pending");
    assert_eq!(source_and_status_snapshot(&store), source_and_status);

    store
        .lock()
        .execute_batch("DROP TRIGGER reject_master_pause_cancel")
        .unwrap();
    super::pause_model_checks(&store, &worker).unwrap();
    assert!(!worker.is_available());
    assert!(!super::yaml_discovery_enabled(&store));
    assert_eq!(unfinished_assessment_state(&store), "cancelled");
    assert_eq!(source_and_status_snapshot(&store), source_and_status);
    super::enable_model_checks(
        &store,
        &worker,
        active_connection(&store).unwrap(),
        Some("synthetic-secret".into()),
    )
    .unwrap();
    assert!(worker.is_available());
    assert_eq!(unfinished_assessment_state(&store), "cancelled");
    assert_eq!(source_and_status_snapshot(&store), source_and_status);
    assert_eq!(
        store.internal_value(CONNECTION_PROFILES_KEY),
        profiles_before
    );
    assert_eq!(store.internal_value(CONNECTION_PENDING_KEY), pending_before);
    let cancellations: u64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM burn_check_assessment_history
         WHERE candidate_id = 'synthetic-candidate' AND outcome = 'cancelled'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cancellations, 1);
    assert!(
        !super::switch_connection(&store, &worker, "proxy", &vault)
            .unwrap()
            .1
    );
    assert_eq!(unfinished_assessment_state(&store), "cancelled");
}

#[test]
fn master_pause_failure_keeps_existing_profile_pending_markers() {
    let (store, worker, _) = saved_custom(true);
    store.set_internal_value(CONNECTION_PENDING_KEY, "other-profile");
    store.set_internal_value(super::CONNECTION_REMOVAL_KEY, "other-removal");
    insert_unfinished_assessment(&store, "pending");
    store
        .lock()
        .execute_batch(
            "CREATE TEMP TRIGGER reject_master_pause_cancel BEFORE UPDATE ON burn_check_candidate
         WHEN NEW.state = 'cancelled'
         BEGIN SELECT RAISE(ABORT, 'injected cancellation failure'); END;",
        )
        .unwrap();
    assert!(super::pause_model_checks(&store, &worker).is_err());
    assert!(!worker.is_available());
    assert!(!super::yaml_discovery_enabled(&store));
    assert_eq!(
        store.internal_value(CONNECTION_PENDING_KEY).as_deref(),
        Some("other-profile")
    );
    assert_eq!(
        store
            .internal_value(super::CONNECTION_REMOVAL_KEY)
            .as_deref(),
        Some("other-removal")
    );
}

#[test]
fn headless_settings_switch_and_restore_use_keyless_profiles_in_a_file_store() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let worker = WorkerHandle::default();
    let first = custom_connection();
    let mut second = first.clone();
    second.model = "other-model".into();
    super::headless::setup_keyless_profiles(
        &store,
        std::collections::BTreeMap::from([
            ("first".into(), first.clone()),
            ("second".into(), second.clone()),
        ]),
        "first",
        true,
    )
    .unwrap();
    super::headless::restore_keyless_profile(&store, &worker).unwrap();
    let snapshot = worker.execution_snapshot().unwrap();
    assert_eq!(snapshot.connection, first);
    assert!(snapshot.credential.is_none());
    assert!(super::headless::switch_keyless_profile(&store, &worker, "second").unwrap());
    assert_eq!(worker.execution_snapshot().unwrap().connection, second);
    super::headless::pause_model_checks(&store, &worker).unwrap();
    assert!(!worker.is_available());
    drop(worker);
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    let worker = WorkerHandle::default();
    super::headless::restore_keyless_profile(&store, &worker).unwrap();
    assert_eq!(worker.system_one_connection(), second);
    assert!(!worker.is_available());
    assert!(!super::yaml_discovery_enabled(&store));
}

#[test]
fn repeated_saved_selection_does_not_cancel_current_work_or_clear_auth_rejection() {
    let (store, worker, vault) = saved_custom(true);
    insert_unfinished_assessment(&store, "pending");
    let generation = worker.execution_snapshot().unwrap().generation;
    let draft = super::SystemOneDraft {
        connection_id: "proxy".into(),
        connection: active_connection(&store).unwrap(),
        credential: None,
    };
    assert!(
        !super::save_connection(&store, &worker, draft.clone(), &vault)
            .unwrap()
            .1
    );
    assert!(
        !super::switch_connection(&store, &worker, "proxy", &vault)
            .unwrap()
            .1
    );
    assert_eq!(unfinished_assessment_state(&store), "pending");
    assert!(worker.key_is_current(generation));
    worker.reject_authentication(&store, generation).unwrap();
    assert!(
        !super::save_connection(&store, &worker, draft, &vault)
            .unwrap()
            .1
    );
    assert!(
        !super::switch_connection(&store, &worker, "proxy", &vault)
            .unwrap()
            .1
    );
    assert!(!worker.is_available());
    assert_eq!(
        store.internal_value(AUTH_REJECTED_KEY).as_deref(),
        Some("true")
    );
}

#[test]
fn failed_fresh_slot_cleanup_preserves_the_old_secret_and_requires_cleanup_before_retry() {
    let (store, worker, vault) = saved_custom(true);
    fail_profile_writes(&store);
    vault.fail_delete.set(true);
    let draft = super::SystemOneDraft {
        connection_id: "proxy".into(),
        connection: custom_connection(),
        credential: Some("replacement".into()),
    };
    assert!(super::save_connection(&store, &worker, draft, &vault).is_err());
    let previous = active_connection(&store).unwrap();
    assert_eq!(
        super::resolve_credential(&previous, None, &vault)
            .unwrap()
            .as_deref(),
        Some("synthetic-secret")
    );
    assert!(!worker.is_available());
    assert!(super::switch_connection(&store, &worker, "proxy", &vault).is_err());
    assert!(super::remove_connection_credential(&store, &worker, "proxy", &vault).is_err());
    assert!(
        super::save_connection(
            &store,
            &worker,
            super::SystemOneDraft {
                connection_id: "other".into(),
                connection: custom_connection(),
                credential: None,
            },
            &vault,
        )
        .is_err()
    );
    store
        .lock()
        .execute_batch("DROP TRIGGER fail_profiles")
        .unwrap();
    vault.fail_delete.set(false);
    let token = settings_snapshot(&store)
        .unwrap()
        .pending_operation
        .unwrap()
        .id;
    let recovered = retry_recording(&store, &worker, &token, &vault, |_| {}).unwrap();
    assert_eq!(recovered.outcome, OperationOutcome::RolledBack);
    assert_eq!(recovered.settings.profiles["proxy"].revision, 1);
    assert_eq!(
        worker.execution_snapshot().unwrap().credential.as_deref(),
        Some("synthetic-secret")
    );
    let (settings, changed) = super::save_connection(
        &store,
        &worker,
        super::SystemOneDraft {
            connection_id: "proxy".into(),
            connection: active_connection(&store).unwrap(),
            credential: Some("replacement".into()),
        },
        &vault,
    )
    .unwrap();
    assert!(changed);
    assert_eq!(settings.profiles["proxy"].revision, 2);
    assert_eq!(
        worker.execution_snapshot().unwrap().credential.as_deref(),
        Some("replacement")
    );
}

#[test]
fn failed_credential_write_cannot_resume_a_rejected_connection() {
    let (store, worker, vault) = saved_custom(true);
    let generation = worker.execution_snapshot().unwrap().generation;
    worker.reject_authentication(&store, generation).unwrap();
    vault.fail_write.set(true);
    assert!(
        super::save_connection(
            &store,
            &worker,
            super::SystemOneDraft {
                connection_id: "proxy".into(),
                connection: custom_connection(),
                credential: Some("replacement".into()),
            },
            &vault,
        )
        .is_err()
    );
    assert!(!worker.is_available());
    assert_eq!(
        store.internal_value(AUTH_REJECTED_KEY).as_deref(),
        Some("true")
    );
    assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
}

#[test]
fn rejected_startup_restores_the_selected_profile_without_reading_its_secret() {
    let (store, worker, vault) = saved_custom(true);
    let connection = active_connection(&store).unwrap();
    store.set_internal_value(AUTH_REJECTED_KEY, "true");
    vault.fail_read.set(true);
    let restored = WorkerHandle::default();
    super::restore_saved_key_with_vault(
        &store,
        &restored,
        || panic!("a rejected connection must not read the legacy key"),
        &vault,
    )
    .unwrap();
    assert_eq!(restored.system_one_connection(), connection);
    assert!(restored.execution_snapshot().is_none());
    assert!(vault.fail_read.get());
    assert!(worker.is_available());
}

fn fail_profile_writes(store: &Store) {
    store
        .lock()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_profiles BEFORE UPDATE ON setting
         WHEN NEW.key = 'internal:smartChecksConnectionsV1'
         BEGIN SELECT RAISE(ABORT, 'injected profile failure'); END;",
        )
        .unwrap();
}

#[test]
fn active_credential_removal_pauses_checks_detaches_metadata_and_is_idempotent() {
    let (store, worker, vault) = saved_custom(true);
    let Some(crate::jev::config::CredentialReference::Connection(slot)) =
        active_connection(&store).unwrap().credential
    else {
        panic!("saved credential slot");
    };
    assert!(worker.is_available());
    store.set_internal_value(AUTH_REJECTED_KEY, "true");
    super::remove_connection_credential(&store, &worker, "proxy", &vault).unwrap();
    assert!(!worker.is_available());
    assert!(!super::yaml_discovery_enabled(&store));
    assert!(active_connection(&store).unwrap().credential.is_none());
    assert!(vault.value.borrow().is_none());
    assert_eq!(
        store.internal_value(AUTH_REJECTED_KEY).as_deref(),
        Some("false")
    );
    super::remove_connection_credential(&store, &worker, "proxy", &vault).unwrap();
    assert_eq!(*vault.deleted_ids.borrow(), vec![Some(slot)]);
    restore_saved_key(&store, &worker, || {
        panic!("removed secret must not be read")
    })
    .unwrap();
    assert!(!worker.is_available());
}

#[test]
fn removing_an_inactive_shared_reference_preserves_the_active_credential() {
    let (store, worker, vault) = saved_custom(true);
    let mut saved = super::profiles(&store).unwrap();
    saved
        .profiles
        .insert("alias".into(), saved.profiles["proxy"].clone());
    super::save_profiles(&store, &saved).unwrap();
    super::remove_connection_credential(&store, &worker, "alias", &vault).unwrap();
    assert!(worker.is_available());
    assert!(active_connection(&store).unwrap().credential.is_some());
    assert!(
        super::profiles(&store).unwrap().profiles["alias"]
            .credential
            .is_none()
    );
    assert!(vault.deleted_ids.borrow().is_empty());
    assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
}

#[test]
fn inactive_removal_deletes_the_referenced_key_slot_without_changing_active_runtime() {
    let (store, worker, vault) = saved_custom(true);
    super::save_connection(
        &store,
        &worker,
        super::SystemOneDraft {
            connection_id: "keyless".into(),
            connection: custom_connection(),
            credential: None,
        },
        &vault,
    )
    .unwrap();
    let mut saved = super::profiles(&store).unwrap();
    saved.profiles.get_mut("proxy").unwrap().credential = Some(
        crate::jev::config::CredentialReference::Connection("key-slot".into()),
    );
    super::save_profiles(&store, &saved).unwrap();
    let active = worker.system_one_connection();
    super::remove_connection_credential(&store, &worker, "proxy", &vault).unwrap();
    assert!(worker.is_available());
    assert_eq!(worker.system_one_connection(), active);
    assert_eq!(
        super::profiles(&store).unwrap().active_id.as_deref(),
        Some("keyless")
    );
    assert!(
        super::profiles(&store).unwrap().profiles["proxy"]
            .credential
            .is_none()
    );
    assert_eq!(*vault.deleted_ids.borrow(), vec![Some("key-slot".into())]);
}

#[test]
fn removal_preflight_and_pending_marker_failures_do_not_remove_keys_or_suspend_runtime() {
    let (store, worker, vault) = saved_custom(true);
    let before = store.internal_value(CONNECTION_PROFILES_KEY);
    vault.fail_read.set(true);
    assert!(super::remove_connection_credential(&store, &worker, "proxy", &vault).is_err());
    assert!(worker.is_available());
    store
        .lock()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_removal_marker BEFORE INSERT ON setting
         WHEN NEW.key = 'internal:smartChecksCredentialRemovalPendingV1'
         BEGIN SELECT RAISE(ABORT, 'injected marker failure'); END;",
        )
        .unwrap();
    assert!(super::remove_connection_credential(&store, &worker, "proxy", &vault).is_err());
    assert!(worker.is_available());
    assert!(super::yaml_discovery_enabled(&store));
    assert_eq!(store.internal_value(CONNECTION_PROFILES_KEY), before);
    assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
    assert!(vault.deleted_ids.borrow().is_empty());
}

#[test]
fn removal_finalization_failure_rolls_back_all_metadata_and_restores_the_key() {
    let (store, worker, vault) = saved_custom(true);
    let before = store.internal_value(CONNECTION_PROFILES_KEY);
    store
        .lock()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_removal_finalize BEFORE UPDATE ON setting
         WHEN NEW.key = 'internal:smartChecksCredentialRemovalPendingV1' AND NEW.value = ''
         BEGIN SELECT RAISE(ABORT, 'injected finalization failure'); END;",
        )
        .unwrap();
    assert!(super::remove_connection_credential(&store, &worker, "proxy", &vault).is_err());
    assert_eq!(store.internal_value(CONNECTION_PROFILES_KEY), before);
    assert_eq!(
        store
            .internal_value(super::CONNECTION_REMOVAL_KEY)
            .as_deref(),
        Some("proxy")
    );
    assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
    assert!(!worker.is_available());
    store
        .lock()
        .execute_batch("DROP TRIGGER fail_removal_finalize")
        .unwrap();
    super::remove_connection_credential(&store, &worker, "proxy", &vault).unwrap();
    assert!(vault.value.borrow().is_none());
    assert!(active_connection(&store).unwrap().credential.is_none());
}

#[test]
fn pending_removal_blocks_replacement_and_restore_until_the_same_removal_retries() {
    let (store, worker, vault) = saved_custom(true);
    vault.fail_delete.set(true);
    assert!(super::remove_connection_credential(&store, &worker, "proxy", &vault).is_err());
    assert!(!worker.is_available());
    assert!(active_connection(&store).unwrap().credential.is_some());
    assert!(restore_saved_key(&store, &worker, || panic!("pending removal read a key")).is_err());
    assert!(
        super::save_connection(
            &store,
            &worker,
            super::SystemOneDraft {
                connection_id: "proxy".into(),
                connection: custom_connection(),
                credential: Some("replacement".into()),
            },
            &vault
        )
        .is_err()
    );
    assert!(super::switch_connection(&store, &worker, "proxy", &vault).is_err());
    assert!(super::remove_connection_credential(&store, &worker, "other", &vault).is_err());
    assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
    super::remove_connection_credential(&store, &worker, "proxy", &vault).unwrap();
    super::save_connection(
        &store,
        &worker,
        super::SystemOneDraft {
            connection_id: "proxy".into(),
            connection: custom_connection(),
            credential: Some("replacement".into()),
        },
        &vault,
    )
    .unwrap();
    super::remove_connection_credential(&store, &worker, "alias", &vault).unwrap_err();
    assert_eq!(vault.value.borrow().as_deref(), Some("replacement"));
}

#[test]
fn removal_metadata_failure_restores_the_secret_and_rollback_failure_stays_retryable() {
    for rollback_fails in [false, true] {
        let (store, worker, vault) = saved_custom(true);
        fail_profile_writes(&store);
        vault.fail_write.set(rollback_fails);
        let error =
            super::remove_connection_credential(&store, &worker, "proxy", &vault).unwrap_err();
        assert_eq!(error.contains("restore"), rollback_fails);
        assert!(active_connection(&store).unwrap().credential.is_some());
        assert!(!worker.is_available());
        assert_eq!(vault.value.borrow().is_none(), rollback_fails);
        assert!(
            restore_saved_key(&store, &worker, || panic!(
                "incomplete removal reads no keys"
            ))
            .is_err()
        );
        store
            .lock()
            .execute_batch("DROP TRIGGER fail_profiles")
            .unwrap();
        super::remove_connection_credential(&store, &worker, "proxy", &vault).unwrap();
        assert!(active_connection(&store).unwrap().credential.is_none());
        assert!(vault.value.borrow().is_none());
    }
}

#[test]
fn missing_or_unreadable_saved_credentials_do_not_mutate_save_or_switch_state() {
    for missing in [true, false] {
        let (store, worker, vault) = saved_custom(true);
        let mut saved = super::profiles(&store).unwrap();
        saved
            .profiles
            .insert("target".into(), saved.profiles["proxy"].clone());
        super::save_profiles(&store, &saved).unwrap();
        let before = store.internal_value(CONNECTION_PROFILES_KEY);
        let connection = active_connection(&store).unwrap();
        if missing {
            vault.value.borrow_mut().take();
            vault.values.borrow_mut().clear();
        } else {
            vault.fail_read.set(true);
        }
        assert!(
            super::save_connection(
                &store,
                &worker,
                super::SystemOneDraft {
                    connection_id: "new".into(),
                    connection: connection.clone(),
                    credential: None,
                },
                &vault
            )
            .is_err()
        );
        assert_eq!(store.internal_value(CONNECTION_PROFILES_KEY), before);
        assert!(worker.is_available());
        if !missing {
            vault.fail_read.set(true);
        }
        assert!(super::switch_connection(&store, &worker, "target", &vault).is_err());
        assert_eq!(store.internal_value(CONNECTION_PROFILES_KEY), before);
        assert_eq!(
            store
                .internal_value(super::CONNECTION_PENDING_KEY)
                .as_deref(),
            None
        );
        assert_eq!(worker.system_one_connection(), connection);
        assert!(worker.is_available());
    }
}

#[test]
fn failed_save_metadata_rolls_back_the_key_and_does_not_publish_draft_profiles() {
    let (store, worker, vault) = saved_custom(true);
    let before = store.internal_value(CONNECTION_PROFILES_KEY);
    fail_profile_writes(&store);
    let draft = super::SystemOneDraft {
        connection_id: "proxy".into(),
        connection: custom_connection(),
        credential: Some("replacement".into()),
    };
    assert!(super::save_connection(&store, &worker, draft.clone(), &vault).is_err());
    assert_eq!(store.internal_value(CONNECTION_PROFILES_KEY), before);
    assert_eq!(
        super::resolve_credential(&active_connection(&store).unwrap(), None, &vault)
            .unwrap()
            .as_deref(),
        Some("synthetic-secret")
    );
    assert_eq!(vault.values.borrow().len(), 2);
    assert!(!worker.is_available());
    assert!(restore_saved_key(&store, &worker, || panic!("pending update reads no keys")).is_err());
    store
        .lock()
        .execute_batch("DROP TRIGGER fail_profiles")
        .unwrap();
    let token = settings_snapshot(&store)
        .unwrap()
        .pending_operation
        .unwrap()
        .id;
    let recovered = retry_recording(&store, &worker, &token, &vault, |_| {}).unwrap();
    assert_eq!(recovered.outcome, OperationOutcome::RolledBack);
    assert_eq!(
        worker.execution_snapshot().unwrap().credential.as_deref(),
        Some("synthetic-secret")
    );
    assert_eq!(vault.values.borrow().len(), 1);
    super::save_connection(&store, &worker, draft, &vault).unwrap();
    assert!(worker.is_available());
    assert_eq!(vault.value.borrow().as_deref(), Some("replacement"));
    assert!(
        !store
            .internal_value(CONNECTION_PROFILES_KEY)
            .unwrap()
            .contains("replacement")
    );
}

#[test]
fn null_draft_uses_only_its_explicit_reference_and_keyless_drafts_skip_the_vault() {
    let vault = OfflineVault::default();
    let keyless = custom_connection();
    vault.fail_read.set(true);
    assert_eq!(
        super::resolve_credential(&keyless, None, &vault).unwrap(),
        None
    );
    let mut referenced = keyless.clone();
    referenced.credential = Some(crate::jev::config::CredentialReference::Connection(
        "proxy".into(),
    ));
    assert!(super::resolve_credential(&referenced, None, &vault).is_err());
    assert!(super::resolve_credential(&referenced, None, &vault).is_err());
    *vault.value.borrow_mut() = Some("saved-secret".into());
    assert_eq!(
        super::resolve_credential(&referenced, None, &vault)
            .unwrap()
            .as_deref(),
        Some("saved-secret")
    );
    assert_eq!(
        super::resolve_credential(&referenced, Some("draft-secret".into()), &vault)
            .unwrap()
            .as_deref(),
        Some("draft-secret")
    );
    assert!(super::resolve_credential(&keyless, Some("".into()), &vault).is_err());
    assert!(
        !serde_json::to_string(&referenced)
            .unwrap()
            .contains("saved-secret")
    );
}

#[test]
fn failed_save_cleanup_reports_failure_and_keeps_launch_blocked_until_retry() {
    let (store, worker, vault) = saved_custom(true);
    fail_profile_writes(&store);
    vault.fail_delete.set(true);
    let draft = super::SystemOneDraft {
        connection_id: "proxy".into(),
        connection: custom_connection(),
        credential: Some("replacement".into()),
    };
    let error = super::save_connection(&store, &worker, draft.clone(), &vault).unwrap_err();
    assert!(error.contains("Retry"));
    let token = settings_snapshot(&store)
        .unwrap()
        .pending_operation
        .unwrap()
        .id;
    assert!(
        retry_recording(&store, &worker, &token, &vault, |_| {})
            .unwrap_err()
            .contains("cleanup")
    );
    assert_eq!(
        super::resolve_credential(&active_connection(&store).unwrap(), None, &vault)
            .unwrap()
            .as_deref(),
        Some("synthetic-secret")
    );
    assert!(!worker.is_available());
    assert!(
        restore_saved_key(&store, &worker, || panic!("failed rollback reads no keys")).is_err()
    );
    store
        .lock()
        .execute_batch("DROP TRIGGER fail_profiles")
        .unwrap();
    vault.fail_delete.set(false);
    let recovered = retry_recording(&store, &worker, &token, &vault, |_| {}).unwrap();
    assert_eq!(recovered.outcome, OperationOutcome::RolledBack);
    assert_eq!(
        worker.execution_snapshot().unwrap().credential.as_deref(),
        Some("synthetic-secret")
    );
    super::save_connection(&store, &worker, draft, &vault).unwrap();
    assert!(worker.is_available());
}

#[test]
fn finalization_failure_keeps_the_staged_profile_disabled_and_can_be_retried() {
    let (store, worker, vault) = saved_custom(true);
    store
        .lock()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_finalize BEFORE UPDATE ON setting
         WHEN NEW.key = 'internal:smartChecksProfileOperationV1' AND NEW.value = ''
         BEGIN SELECT RAISE(ABORT, 'injected finalization failure'); END;",
        )
        .unwrap();
    let draft = super::SystemOneDraft {
        connection_id: "proxy".into(),
        connection: crate::jev::config::SystemOneConnection {
            model: "updated-model".into(),
            ..custom_connection()
        },
        credential: None,
    };
    assert!(super::save_connection(&store, &worker, draft.clone(), &vault).is_err());
    assert_eq!(
        super::profiles(&store).unwrap().active_id.as_deref(),
        Some("proxy")
    );
    assert!(!worker.is_available());
    assert!(
        super::restore_saved_key_with_vault(
            &store,
            &worker,
            || panic!("staged update reads no keys"),
            &vault
        )
        .is_err()
    );
    store
        .lock()
        .execute_batch("DROP TRIGGER fail_finalize")
        .unwrap();
    let token = settings_snapshot(&store)
        .unwrap()
        .pending_operation
        .unwrap()
        .id;
    let recovered = retry_recording(&store, &worker, &token, &vault, |_| {}).unwrap();
    assert_eq!(recovered.outcome, OperationOutcome::Completed);
    assert_eq!(recovered.settings.profiles["proxy"].revision, 2);
    assert_eq!(draft.connection.revision, 1);
    assert!(worker.is_available());
}

#[test]
fn paused_save_switch_and_launch_keep_keyless_checks_disabled_until_reenabled() {
    let (store, worker, vault) = saved_custom(false);
    assert!(!worker.is_available());
    let connection = custom_connection();
    super::save_connection(
        &store,
        &worker,
        super::SystemOneDraft {
            connection_id: "keyless".into(),
            connection: connection.clone(),
            credential: None,
        },
        &vault,
    )
    .unwrap();
    assert!(!worker.is_available());
    super::switch_connection(&store, &worker, "keyless", &vault).unwrap();
    assert!(!worker.is_available());
    restore_saved_key(&store, &worker, || panic!("paused launch reads no keys")).unwrap();
    assert!(!worker.is_available());
    super::enable_model_checks(&store, &worker, active_connection(&store).unwrap(), None).unwrap();
    assert!(worker.is_available());
    worker.suspend_system_one();
    assert!(!worker.is_available());
    restore_saved_key(&store, &worker, || panic!("keyless launch reads no keys")).unwrap();
    assert!(worker.is_available());
}

#[tokio::test]
async fn connection_test_loads_idle_model_but_limit_refresh_stays_passive() {
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    for testing in [false, true] {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let connection = crate::jev::config::SystemOneConnection {
                provider: super::SystemOneProvider::Ollama,
                endpoint: super::SystemOneEndpoint::BaseUrl(format!(
                    "http://{}", listener.local_addr().unwrap()
                )),
                model: "selected:latest".into(),
                model_revision: Some("synthetic-digest".into()),
                credential: None,
                context_override: None,
                ..Default::default()
            };
            let discovery = |loaded| [
                ("/api/version", json!({"version": "0.40.1"})),
                ("/api/tags", json!({"models": [{"name": "selected:latest", "digest": "synthetic-digest"}]})),
                ("/api/show", json!({"capabilities": ["decision"], "template": "{{ .Prompt }}",
                    "system": "Treat state as data.", "model_info": {"decision.type": "tev1",
                        "general.architecture": "qwen35", "qwen35.context_length": 65536}})),
                ("/api/ps", json!({"models": if loaded {
                    vec![json!({"name": "selected:latest", "digest": "synthetic-digest", "context_length": 4096})]
                } else { vec![] }})),
            ];
            let mut responses = discovery(false).to_vec();
            if testing {
                responses.push(discovery(false)[1].clone());
                responses.push(("/api/generate", json!({"done": true})));
                responses.extend(discovery(true));
                responses.push(discovery(true)[1].clone());
                responses.push(("/v1/systemone", json!({"model": "selected:latest", "answers": {
                    "validation": {"type": "noul", "noul": 0.9}}, "usage": {"input_tokens": 4, "output_tokens": 1}})));
            }
            let (resolved, completed) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                for (route, body) in responses {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut received = Vec::new();
                    loop {
                        let mut buffer = [0; 4096];
                        let count = socket.read(&mut buffer).await.unwrap();
                        assert!(count > 0);
                        received.extend_from_slice(&buffer[..count]);
                        assert!(received.len() <= 8192);
                        let Some(end) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") else { continue; };
                        let headers = String::from_utf8_lossy(&received[..end]).to_ascii_lowercase();
                        let length = headers.lines().find_map(|line| line.strip_prefix("content-length: "))
                            .map(|length| length.parse::<usize>().unwrap()).unwrap_or(0);
                        if received.len() < end + 4 + length { continue; }
                        let method = if matches!(route, "/api/show" | "/api/generate" | "/v1/systemone") { "post" } else { "get" };
                        assert!(headers.starts_with(&format!("{method} {route} http/1.1")), "{headers}");
                        assert!(!headers.contains("authorization:"));
                        if method == "post" {
                            let request: serde_json::Value = serde_json::from_slice(&received[end + 4..end + 4 + length]).unwrap();
                            match route {
                                "/api/show" => assert_eq!(request, json!({"model": "selected:latest"})),
                                "/api/generate" => assert_eq!(request, json!({"model": "selected:latest", "prompt": "", "stream": false})),
                                _ => {
                                    assert_eq!(request["model"], "selected:latest");
                                    assert_eq!(request["state"], json!({"text": "synthetic connection validation input"}));
                                    assert_eq!(request["questions"].as_object().unwrap().len(), 1);
                                    assert_eq!(request["questions"]["validation"]["type"], "noul");
                                }
                            }
                        }
                        break;
                    }
                    let body = body.to_string();
                    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
                tokio::select! {
                    extra = listener.accept() => panic!("Unexpected Ollama request: {extra:?}"),
                    _ = completed => {}
                }
            });
            if testing {
                super::test_connection(super::SystemOneDraft {
                    connection_id: "cold-local".into(),
                    connection,
                    credential: None,
                }).await.unwrap();
            } else {
                let limits = super::refresh_limits(connection, None).await.unwrap();
                assert_eq!(limits.usable_input_tokens(), None);
                assert_eq!(limits.usable_state_tokens(), None);
                assert_eq!(limits.runtime_context_tokens.value, None);
                assert_eq!(limits.model_revision.as_deref(), Some("synthetic-digest"));
                assert!(limits.output.noul);
                assert_eq!(limits.output.choice.unwrap().maximum_criteria, 24);
                assert!(limits.uses_ollama_tev1_accounting());
            }
            resolved.send(()).unwrap();
            server.await.unwrap();
        }).await.unwrap();
    }
}

#[tokio::test]
async fn test_and_refresh_send_saved_authentication_but_keep_keyless_requests_keyless() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    for saved_secret in [None, Some("synthetic-local-token")] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let connection = crate::jev::config::SystemOneConnection {
            provider: crate::jev::config::SystemOneProvider::Ollama,
            endpoint: crate::jev::config::SystemOneEndpoint::BaseUrl(format!(
                "http://{}",
                listener.local_addr().unwrap()
            )),
            model: "clef-flash".into(),
            credential: saved_secret
                .map(|_| crate::jev::config::CredentialReference::Connection("local".into())),
            ..custom_connection()
        };
        let server = tokio::spawn(async move {
            let discovery = [
                ("/api/version", serde_json::json!({"version":"0.40.1"})),
                (
                    "/api/tags",
                    serde_json::json!({"models":[{"name":"clef-flash:latest", "digest":"synthetic-digest"}]}),
                ),
                (
                    "/api/show",
                    serde_json::json!({"capabilities":["decision"], "model_info":{"decision.type":"clef","general.architecture":"clef","clef.context_length":16384}}),
                ),
                (
                    "/api/ps",
                    serde_json::json!({"models":[{"name":"clef-flash:latest","digest":"synthetic-digest","context_length":8192}]}),
                ),
            ];
            for (route, body) in discovery.clone().into_iter().chain([discovery[1].clone(), (
                "/v1/systemone",
                serde_json::json!({"model":"clef-flash", "answers":{"validation":{"type":"noul","noul":0.9}}, "usage":{"input_tokens":4,"output_tokens":1}}),
            )]).chain(discovery) {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                    assert!(request.len() < 65536);
                    if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                    {
                        let headers =
                            String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                        let body_bytes = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .map(|length| length.parse::<usize>().unwrap())
                            .unwrap_or(0);
                        if request.len() >= end + 4 + body_bytes {
                            assert!(headers.contains(route));
                            match saved_secret {
                                Some(secret) => assert!(
                                    headers
                                        .contains(&format!("authorization: bearer {secret}"))
                                ),
                                None => assert!(!headers.contains("authorization:")),
                            }
                            break;
                        }
                    }
                }
                let body = body.to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        let vault = OfflineVault::default();
        *vault.value.borrow_mut() = saved_secret.map(str::to_owned);
        let credential = super::resolve_credential(&connection, None, &vault).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            super::test_connection(super::SystemOneDraft {
                connection_id: "local".into(),
                connection: connection.clone(),
                credential: credential.clone(),
            })
            .await
            .unwrap();
            let capabilities = super::refresh_limits(connection.clone(), credential)
                .await
                .unwrap();
            assert_eq!(
                capabilities.model_revision.as_deref(),
                Some("synthetic-digest")
            );
            assert_eq!(capabilities.runtime_context_tokens.value, Some(8192));
            server.await.unwrap();
        })
        .await
        .unwrap();
        assert!(
            !serde_json::to_string(&connection)
                .unwrap()
                .contains("synthetic-local-token")
        );
    }
}

#[tokio::test]
async fn discovery_failures_cannot_use_manual_limits_or_change_the_selected_profile() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    for testing in [false, true] {
        for authentication_rejected in [false, true] {
            let (store, worker, vault) = saved_custom(true);
            let before = store.internal_value(CONNECTION_PROFILES_KEY);
            let generation = worker.execution_snapshot().unwrap().generation;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let connection = crate::jev::config::SystemOneConnection {
                provider: super::SystemOneProvider::Ollama,
                endpoint: super::SystemOneEndpoint::BaseUrl(format!(
                    "http://{}",
                    listener.local_addr().unwrap()
                )),
                model: "clef-flash".into(),
                context_override: Some(crate::jev::config::ContextLimitOverride {
                    total_input_tokens: Some(8192),
                    state_and_longest_question_tokens: Some(8192),
                    runtime_context_tokens: Some(8192),
                }),
                ..custom_connection()
            };
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                    assert!(request.len() < 65536);
                }
                assert!(String::from_utf8_lossy(&request).contains("/api/version"));
                let (status, body) = if authentication_rejected {
                    ("401 Unauthorized", r#"{"error":"authentication required"}"#)
                } else {
                    ("200 OK", r#"{"version":"0.1.0"}"#)
                };
                socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            });
            let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                if testing {
                    super::test_connection(super::SystemOneDraft {
                        connection_id: "draft-only".into(),
                        connection,
                        credential: Some("draft-secret".into()),
                    })
                    .await
                    .map(|_| ())
                } else {
                    super::refresh_limits(connection, Some("draft-secret".into()))
                        .await
                        .map(|_| ())
                }
            })
            .await
            .unwrap();
            let error = result.unwrap_err();
            if authentication_rejected {
                assert!(error.contains("rejected its credential"));
            } else {
                assert!(error.contains("no supported model profile or runner"));
            }
            server.await.unwrap();
            assert_eq!(store.internal_value(CONNECTION_PROFILES_KEY), before);
            assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
            assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
        }
    }
}

#[test]
fn vault_read_write_and_delete_failures_are_injectable_offline() {
    let vault = OfflineVault::default();
    vault.fail_read.set(true);
    assert!(vault.read(None).is_err());
    vault.fail_write.set(true);
    assert!(write_credential(&vault, Some("jev"), "synthetic").is_err());
    assert!(vault.value.borrow().is_none());
    write_credential(&vault, Some("jev"), "synthetic").unwrap();
    vault.fail_delete.set(true);
    assert!(delete_credential(&vault, Some("jev")).is_err());
    assert_eq!(vault.value.borrow().as_deref(), Some("synthetic"));
    delete_credential(&vault, Some("jev")).unwrap();
    assert!(vault.value.borrow().is_none());
}

#[test]
fn failed_connection_secret_write_restores_the_previous_provider_and_allows_retry() {
    let store =
        Store::open_in_memory(Path::new("/tmp/antiburn-provider-save-retry")).expect("store");
    migrate_provider_state(&store).expect("migrate");
    let mut saved = super::profiles(&store).expect("read profiles");
    let connection = crate::jev::config::SystemOneConnection {
        provider: crate::jev::config::SystemOneProvider::Custom,
        endpoint: crate::jev::config::SystemOneEndpoint::ExactUrl(
            "https://proxy.example/v1/systemone".to_owned(),
        ),
        model: "proxy-model".to_owned(),
        model_revision: None,
        response_mode: crate::jev::config::SystemOneResponseMode::Direct,
        credential: None,
        revision: 1,
        context_override: Some(crate::jev::config::ContextLimitOverride {
            total_input_tokens: Some(8192),
            ..crate::jev::config::ContextLimitOverride::default()
        }),
    };
    let vault = OfflineVault::default();
    vault.fail_write.set(true);
    let worker = WorkerHandle::default();

    assert_eq!(
        persist_connection_profile(
            &store,
            &mut saved,
            "proxy",
            connection.clone(),
            Some("synthetic-secret".to_owned()),
            &vault,
            &worker,
        )
        .unwrap_err(),
        "Could not save the API token to secure storage. Check credential storage access and retry."
    );
    assert_eq!(
        store
            .internal_value(super::CONNECTION_PENDING_KEY)
            .as_deref(),
        None
    );
    assert_eq!(
        active_connection(&store).unwrap().provider,
        crate::jev::config::SystemOneProvider::Jev
    );
    assert!(!worker.is_available());

    let token = settings_snapshot(&store)
        .unwrap()
        .pending_operation
        .unwrap()
        .id;
    let recovered = retry_recording(&store, &worker, &token, &vault, |_| {}).unwrap();
    assert_eq!(recovered.outcome, OperationOutcome::RolledBack);
    persist_connection_profile(
        &store,
        &mut saved,
        "proxy",
        connection.clone(),
        Some("synthetic-secret".to_owned()),
        &vault,
        &worker,
    )
    .expect("retry save");
    let active = active_connection(&store).unwrap();
    assert_eq!(active.provider, connection.provider);
    assert_eq!(active.endpoint, connection.endpoint);
    assert!(
        matches!(active.credential, Some(crate::jev::config::CredentialReference::Connection(ref slot)) if slot.starts_with("credential-") && slot != "proxy")
    );
    assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
}

#[test]
fn migration_retries_after_profile_state_was_written_without_its_marker() {
    let store =
        Store::open_in_memory(Path::new("/tmp/antiburn-provider-migrate-retry")).expect("store");
    let mut saved = super::ConnectionProfiles::default();
    saved.profiles.insert(
        "local".to_owned(),
        crate::jev::config::SystemOneConnection {
            provider: crate::jev::config::SystemOneProvider::Ollama,
            endpoint: crate::jev::config::SystemOneEndpoint::BaseUrl(
                "http://127.0.0.1:11434".to_owned(),
            ),
            model: "clef-flash".to_owned(),
            model_revision: None,
            response_mode: crate::jev::config::SystemOneResponseMode::Direct,
            credential: None,
            revision: 1,
            context_override: None,
        },
    );
    saved.active_id = Some("local".to_owned());
    super::save_profiles(&store, &saved).expect("persist before migration marker");
    store.set_internal_value(YAML_DISCOVERY_ENABLED_KEY, "true");

    migrate_provider_state(&store).expect("retry interrupted migration");

    assert_eq!(active_connection(&store).unwrap(), saved.profiles["local"]);
    assert_eq!(
        store.internal_value(PROVIDER_MIGRATION_KEY).as_deref(),
        Some("1")
    );
    assert!(super::yaml_discovery_enabled(&store));
}

#[test]
fn availability_failure_event_has_a_typed_failure_status() {
    assert_eq!(
        serde_json::to_value(CheckAvailabilityEvent::Failed).unwrap(),
        serde_json::json!({"status":"failed"})
    );
}

#[test]
fn check_mutations_accept_only_main_and_settings_windows() {
    assert!(super::checks_settings_label(crate::main_window::LABEL).is_ok());
    assert!(super::checks_settings_label(crate::settings::LABEL).is_ok());
    assert!(super::checks_settings_label("popover").is_err());
}

#[test]
fn replacing_a_saved_key_preserves_a_paused_master_switch() {
    assert!(!super::resume_after_key_save(false, true, true));
    assert!(super::resume_after_key_save(true, true, true));
    assert!(super::resume_after_key_save(false, true, false));
}

#[test]
fn availability_lists_deterministic_preferences_without_retired_history_fields() {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-check-availability")).unwrap();
    let snapshot = super::read_check_availability(&store, false, false, None).unwrap();
    assert_eq!(
        snapshot.checks.len(),
        antiburn_local::checks::DetectorId::COUNT
    );
    for choice in &snapshot.checks {
        let detector = antiburn_local::checks::DetectorId::from(choice.id);
        assert_eq!(choice.enabled, store.check_enabled(detector).unwrap());
    }
    store
        .set_check_enabled(antiburn_local::checks::DetectorId::ALL[0], false)
        .unwrap();
    let encoded = serde_json::to_value(&snapshot).unwrap();
    assert!(encoded.get("historyDays").is_none());
    assert!(encoded.get("backfill").is_none());
    assert_eq!(
        super::read_check_availability(&store, false, false, None)
            .unwrap()
            .revision,
        1
    );
}

#[test]
fn startup_skips_credential_storage_without_a_saved_key() {
    let store =
        Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-unconfigured")).expect("store");
    let worker = WorkerHandle::default();
    restore_saved_key(&store, &worker, || {
        panic!("unconfigured startup read the keychain")
    })
    .expect("no key needed");
    assert!(!worker.is_available());
}

#[test]
fn pausing_keeps_the_saved_key_marker_separate_from_enablement() {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-paused")).expect("store");
    store.set_internal_value(YAML_DISCOVERY_ENABLED_KEY, "true");
    store.set_internal_value(SAVED_KEY_KEY, "true");
    store
        .set_internal_value_checked(YAML_DISCOVERY_ENABLED_KEY, "false")
        .expect("pause checks");

    assert!(saved_key_marker(&store));
    assert!(!super::yaml_discovery_enabled(&store));
}

#[test]
fn startup_restores_only_a_saved_valid_key() {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-restored")).expect("store");
    let worker = WorkerHandle::default();
    store.set_internal_value(YAML_DISCOVERY_ENABLED_KEY, "true");
    store.set_internal_value(SAVED_KEY_KEY, "true");
    restore_saved_key(&store, &worker, || Ok(Some("synthetic-key".to_owned())))
        .expect("key restored");
    assert!(worker.is_available());

    worker.suspend_system_one();
    store.set_internal_value(AUTH_REJECTED_KEY, "true");
    restore_saved_key(&store, &worker, || Ok(Some("rejected-key".to_owned())))
        .expect("rejected key read");
    assert!(!worker.is_available());
}

#[test]
fn startup_restores_jev_for_yaml_checks_without_legacy_enablement() {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-yaml-jev-restored")).expect("store");
    let worker = WorkerHandle::default();
    store.set_internal_value(YAML_DISCOVERY_ENABLED_KEY, "true");
    store.set_internal_value(SAVED_KEY_KEY, "true");

    restore_saved_key(&store, &worker, || Ok(Some("synthetic-key".to_owned())))
        .expect("yaml setting restores the Jev connection");

    assert!(worker.is_available());
    assert!(super::yaml_discovery_enabled(&store));
}

#[test]
fn removal_before_startup_restore_prevents_a_keychain_read() {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-removed")).expect("store");
    let worker = WorkerHandle::default();
    store.set_internal_value(YAML_DISCOVERY_ENABLED_KEY, "true");
    store
        .set_internal_value_checked(YAML_DISCOVERY_ENABLED_KEY, "false")
        .expect("pause checks");
    restore_saved_key(&store, &worker, || panic!("removed key read from keychain"))
        .expect("restore skipped");
    assert!(!worker.is_available());
}

#[test]
fn missing_saved_key_does_not_start_checks() {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-missing")).expect("store");
    let worker = WorkerHandle::default();
    store.set_internal_value(YAML_DISCOVERY_ENABLED_KEY, "true");
    store.set_internal_value(SAVED_KEY_KEY, "true");
    assert!(restore_saved_key(&store, &worker, || Ok(None)).is_err());
    assert!(!worker.is_available());
}

#[test]
fn pending_credential_removal_blocks_startup_restore() {
    let store =
        Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-pending-remove")).expect("store");
    let worker = WorkerHandle::default();
    store.set_internal_value(YAML_DISCOVERY_ENABLED_KEY, "true");
    store.set_internal_value(CREDENTIAL_CHANGE_PENDING_KEY, "true");
    assert!(
        restore_saved_key(&store, &worker, || {
            panic!("pending removal read the old credential")
        })
        .is_err()
    );
    assert!(!worker.is_available());
}

#[test]
fn failed_legacy_removal_can_retry_after_replacement_and_reenable_checks() {
    let store =
        Store::open_in_memory(Path::new("/tmp/antiburn-legacy-removal-retry")).expect("store");
    migrate_provider_state(&store).expect("migrate");
    store.set_internal_value(YAML_DISCOVERY_ENABLED_KEY, "true");
    store.set_internal_value(CREDENTIAL_CHANGE_PENDING_KEY, "true");
    let vault = OfflineVault::default();
    let mut profiles = super::profiles(&store).expect("read profiles");
    let replacement = crate::jev::config::SystemOneConnection {
        provider: crate::jev::config::SystemOneProvider::Ollama,
        endpoint: crate::jev::config::SystemOneEndpoint::BaseUrl("http://127.0.0.1:11434".into()),
        model: "clef-flash".into(),
        credential: None,
        ..Default::default()
    };
    let worker = WorkerHandle::default();
    vault.fail_delete.set(true);
    assert!(
        persist_connection_profile(
            &store,
            &mut profiles,
            "local",
            replacement.clone(),
            None,
            &vault,
            &worker,
        )
        .is_err()
    );
    assert_eq!(
        active_connection(&store).unwrap().provider,
        crate::jev::config::SystemOneProvider::Jev
    );
    vault.fail_delete.set(false);
    persist_connection_profile(
        &store,
        &mut profiles,
        "local",
        replacement,
        None,
        &vault,
        &worker,
    )
    .expect("retry cleanup and provider replacement");
    assert_eq!(
        store
            .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
            .as_deref(),
        Some("false")
    );
    store.set_internal_value(CONNECTION_PENDING_KEY, "");
    restore_saved_key(&store, &worker, || {
        panic!("replacement must not read legacy key")
    })
    .expect("restore replacement");
    assert!(!worker.is_available());
    super::enable_model_checks(&store, &worker, active_connection(&store).unwrap(), None).unwrap();
    assert!(worker.is_available());
}

#[test]
fn provider_migration_is_idempotent_and_preserves_yaml_enablement_and_credentials() {
    let store =
        Store::open_in_memory(Path::new("/tmp/antiburn-provider-migration")).expect("store");
    store.set_internal_value(YAML_DISCOVERY_ENABLED_KEY, "true");
    store.set_internal_value(super::SAVED_KEY_KEY, "true");

    migrate_provider_state(&store).expect("migrate");
    let first = store.internal_value(CONNECTION_PROFILES_KEY).unwrap();
    migrate_provider_state(&store).expect("repeat migration");

    assert_eq!(store.internal_value(CONNECTION_PROFILES_KEY), Some(first));
    assert_eq!(
        store.internal_value(PROVIDER_MIGRATION_KEY).as_deref(),
        Some("1")
    );
    assert!(super::yaml_discovery_enabled(&store));
    assert!(saved_key_marker(&store));
    assert!(active_connection(&store).is_ok());
}

#[test]
fn keyless_provider_migration_does_not_read_or_create_a_credential() {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-keyless-migration")).expect("store");
    migrate_provider_state(&store).expect("migrate keyless settings");
    assert!(!saved_key_marker(&store));
    assert!(active_connection(&store).is_ok());
}

#[test]
fn keyless_ollama_connection_restores_without_touching_the_keyring() {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-keyless-ollama")).expect("store");
    migrate_provider_state(&store).expect("migrate default settings");
    let mut profiles: super::ConnectionProfiles =
        serde_json::from_str(&store.internal_value(CONNECTION_PROFILES_KEY).unwrap()).unwrap();
    let connection = crate::jev::config::SystemOneConnection {
        provider: crate::jev::config::SystemOneProvider::Ollama,
        endpoint: crate::jev::config::SystemOneEndpoint::BaseUrl(
            "http://127.0.0.1:11434".to_owned(),
        ),
        model: "clef-flash".to_owned(),
        model_revision: None,
        response_mode: crate::jev::config::SystemOneResponseMode::Direct,
        credential: None,
        revision: 1,
        context_override: None,
    };
    profiles
        .profiles
        .insert("local".to_owned(), connection.clone());
    profiles.active_id = Some("local".to_owned());
    super::save_profiles(&store, &profiles).unwrap();
    store.set_internal_value(YAML_DISCOVERY_ENABLED_KEY, "true");

    let worker = WorkerHandle::default();
    restore_saved_key(&store, &worker, || {
        panic!("Ollama must not read TypeSafe keyring")
    })
    .unwrap();

    assert_eq!(worker.system_one_connection(), connection);
    assert!(worker.is_available());
    store
        .set_internal_value_checked(YAML_DISCOVERY_ENABLED_KEY, "false")
        .unwrap();
    restore_saved_key(&store, &worker, || panic!("paused startup reads no keys")).unwrap();
    assert_eq!(worker.system_one_connection(), connection);
    assert!(!worker.is_available());
}

#[test]
fn saved_connection_profiles_switch_and_restore_to_jev_without_serializing_secrets() {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-provider-profiles")).expect("store");
    migrate_provider_state(&store).expect("migrate");
    let mut saved = super::profiles(&store).expect("read default profile");
    let ollama = crate::jev::config::SystemOneConnection {
        provider: crate::jev::config::SystemOneProvider::Ollama,
        endpoint: crate::jev::config::SystemOneEndpoint::BaseUrl(
            "http://127.0.0.1:11434".to_owned(),
        ),
        model: "clef-flash".to_owned(),
        model_revision: None,
        response_mode: crate::jev::config::SystemOneResponseMode::Direct,
        credential: None,
        revision: 1,
        context_override: None,
    };
    saved.profiles.insert("local".to_owned(), ollama.clone());
    saved.active_id = Some("local".to_owned());
    super::save_profiles(&store, &saved).unwrap();
    assert_eq!(active_connection(&store).unwrap(), ollama);

    saved.active_id = Some("jev".to_owned());
    super::save_profiles(&store, &saved).unwrap();
    assert_eq!(
        active_connection(&store).unwrap(),
        crate::jev::config::SystemOneConnection::jev_default()
    );
    let encoded = store.internal_value(CONNECTION_PROFILES_KEY).unwrap();
    assert!(!encoded.contains("secret"));
    assert!(!encoded.contains("api_key"));
}
