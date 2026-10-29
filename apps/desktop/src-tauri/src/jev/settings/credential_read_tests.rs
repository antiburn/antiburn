use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::Path;

use super::super as native;
use super::custom_connection;
use crate::jev::worker::WorkerHandle;
use crate::store::Store;
use crate::trace_capture::LogCapture;
use native::credential_read::{Category, Failure, Stage};
use native::operation::{OperationOutcome, retry_recording, settings_snapshot};
use native::{CredentialReference, CredentialVault, SystemOneConnection};

type ReadResult = Result<Option<String>, keyring::Error>;

#[derive(Default)]
struct ScriptedVault {
    script: RefCell<VecDeque<(Option<String>, ReadResult)>>,
    reads: RefCell<Vec<Option<String>>>,
    delete_script: RefCell<VecDeque<keyring::Result<()>>>,
    deletes: RefCell<Vec<Option<String>>>,
}

impl ScriptedVault {
    fn push(&self, id: Option<&str>, result: ReadResult) {
        self.script
            .borrow_mut()
            .push_back((id.map(str::to_owned), result));
    }
}

impl CredentialVault for ScriptedVault {
    type Error = keyring::Error;

    fn read(&self, id: Option<&str>) -> ReadResult {
        let id = id.map(str::to_owned);
        self.reads.borrow_mut().push(id.clone());
        let (expected, result) = self
            .script
            .borrow_mut()
            .pop_front()
            .expect("Unexpected vault read");
        assert_eq!(id, expected);
        result
    }

    fn write(&self, _: Option<&str>, _: &str) -> Result<(), Self::Error> {
        panic!("A switch must not write credentials")
    }

    fn delete(&self, id: Option<&str>) -> Result<(), Self::Error> {
        self.deletes.borrow_mut().push(id.map(str::to_owned));
        self.delete_script
            .borrow_mut()
            .pop_front()
            .expect("Unexpected vault deletion")
    }

    fn is_missing(error: &Self::Error) -> bool {
        native::SystemCredentialVault::is_missing(error)
    }

    fn read_failure(error: &Self::Error) -> Failure {
        native::SystemCredentialVault::read_failure(error)
    }

    fn write_failure(error: &Self::Error) -> &'static str {
        native::SystemCredentialVault::write_failure(error)
    }
}

fn slot(reference: &CredentialReference) -> Option<&str> {
    match reference {
        CredentialReference::LegacyTypeSafe => None,
        CredentialReference::Connection(id) => Some(id),
    }
}

fn references() -> [CredentialReference; 3] {
    [
        CredentialReference::LegacyTypeSafe,
        CredentialReference::Connection("old-profile-slot".into()),
        CredentialReference::Connection("credential-0123456789abcdef0123456789abcdef".into()),
    ]
}

fn setup(reference: CredentialReference, enabled: bool) -> (Store, WorkerHandle) {
    let target = SystemOneConnection {
        credential: Some(reference),
        ..Default::default()
    };
    setup_with_target(target, enabled)
}

fn setup_with_target(target: SystemOneConnection, enabled: bool) -> (Store, WorkerHandle) {
    let store = Store::open_in_memory(Path::new("synthetic-credential-read")).unwrap();
    let previous = custom_connection();
    native::save_profiles(
        &store,
        &native::ConnectionProfiles {
            active_id: Some("previous".into()),
            profiles: [
                ("previous".into(), previous.clone()),
                ("target".into(), target),
            ]
            .into(),
            retired_ids: Default::default(),
        },
    )
    .unwrap();
    store.set_internal_value(native::PROVIDER_MIGRATION_KEY, "1");
    store.set_internal_value(native::AUTOMATIC_LIMITS_MIGRATION_KEY, "1");
    store.set_internal_value(native::SAVED_KEY_KEY, "true");
    store.set_internal_value(
        native::YAML_DISCOVERY_ENABLED_KEY,
        if enabled { "true" } else { "false" },
    );
    store.set_internal_value(native::AUTH_REJECTED_KEY, "false");
    settings_snapshot(&store).unwrap();
    let worker = WorkerHandle::default();
    worker
        .install_system_one_connection(previous, enabled.then(|| "previous-secret".into()), enabled)
        .unwrap();
    (store, worker)
}

fn platform_error() -> keyring::Error {
    keyring::Error::PlatformFailure(Box::new(std::io::Error::other("raw-error-secret-sentinel")))
}

fn structured_errors() -> Vec<(keyring::Error, Category)> {
    let metadata = keyring::mock::MockCredential::default();
    metadata.set_error(keyring::Error::Invalid(
        "metadata-sentinel".into(),
        "account-sentinel".into(),
    ));
    vec![
        (keyring::Error::NoEntry, Category::Missing),
        (
            keyring::Error::BadEncoding(b"bad-encoding-secret-sentinel\xff".to_vec()),
            Category::Malformed,
        ),
        (
            keyring::Error::NoStorageAccess(Box::new(std::io::Error::other(
                "access-secret-sentinel",
            ))),
            Category::Unavailable,
        ),
        (platform_error(), Category::Platform),
        (
            keyring::Error::Invalid("attribute-sentinel".into(), "reason-sentinel".into()),
            Category::InvalidReference,
        ),
        (
            keyring::Error::TooLong("attribute-sentinel".into(), 1),
            Category::InvalidReference,
        ),
        (
            keyring::Error::Ambiguous(vec![Box::new(metadata)]),
            Category::Ambiguous,
        ),
    ]
}

#[test]
fn structured_preflight_failures_keep_the_previous_runtime_and_report_only_safe_fields() {
    for (error, category) in structured_errors() {
        let (store, worker) = setup(CredentialReference::Connection("saved-slot".into()), true);
        let before = store.internal_value(native::CONNECTION_PROFILES_KEY);
        let runtime = worker.execution_snapshot().unwrap();
        let vault = ScriptedVault::default();
        vault.push(Some("saved-slot"), Err(error));
        let capture = LogCapture::default();
        let mut events = vec![];
        let result = tracing::subscriber::with_default(capture.clone(), || {
            native::lifecycle::switch_connection_recording(
                &store,
                &worker,
                "target",
                &vault,
                |event| events.push(event),
            )
        });
        assert_eq!(result.unwrap_err(), category.message());
        assert_eq!(
            store.internal_value(native::CONNECTION_PROFILES_KEY),
            before
        );
        assert!(
            settings_snapshot(&store)
                .unwrap()
                .pending_operation
                .is_none()
        );
        assert!(store.internal_value(native::operation::KEY).is_none());
        let after = worker.execution_snapshot().unwrap();
        assert_eq!(after.generation, runtime.generation);
        assert_eq!(after.connection, runtime.connection);
        assert_eq!(after.credential, runtime.credential);
        assert!(events.is_empty());
        let diagnostics = capture.events("system_one_credential_read_failed");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].fields["stage"], "\"switch_preflight\"");
        assert!(
            diagnostics[0]
                .fields
                .keys()
                .all(|key| ["event", "stage", "category", "os_status"].contains(&key.as_str()))
        );
        let text = capture.text();
        for sentinel in [
            "secret-sentinel",
            "metadata-sentinel",
            "account-sentinel",
            "attribute-sentinel",
            "reason-sentinel",
            "saved-slot",
            "target",
        ] {
            assert!(!text.contains(sentinel));
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn numeric_macos_status_comes_only_from_the_native_error_downcast() {
    for (status, category, name) in [
        (-128, Category::Cancelled, "cancelled"),
        (-25293, Category::Access, "access"),
        (-25308, Category::Access, "access"),
        (-25291, Category::Unavailable, "unavailable"),
        (-50, Category::Platform, "platform"),
    ] {
        let source = Box::new(security_framework::base::Error::from_code(status));
        let error = if category == Category::Unavailable {
            keyring::Error::NoStorageAccess(source)
        } else {
            keyring::Error::PlatformFailure(source)
        };
        let capture = LogCapture::default();
        let message = tracing::subscriber::with_default(capture.clone(), || {
            Failure::from_keyring(&error).report(Stage::SwitchPreflight)
        });
        assert_eq!(message, category.message());
        let events = capture.events("system_one_credential_read_failed");
        assert_eq!(events[0].fields["os_status"], status.to_string());
        assert_eq!(events[0].fields["category"], format!("{name:?}"));
    }
    let source = std::io::Error::other("-128 raw-error-secret-sentinel");
    let error = keyring::Error::PlatformFailure(Box::new(source));
    assert_eq!(
        Failure::from_keyring(&error),
        Failure::new(Category::Platform)
    );
}

#[test]
fn stored_missing_blank_and_malformed_values_have_safe_distinct_errors() {
    for value in [
        None,
        Some(String::new()),
        Some(" \t".into()),
        Some("x".repeat(4097)),
        Some("token\n".into()),
    ] {
        let expected = if value.is_none() {
            Category::Missing
        } else {
            Category::Malformed
        };
        let vault = ScriptedVault::default();
        vault.push(Some("saved-slot"), Ok(value));
        let connection = SystemOneConnection {
            credential: Some(CredentialReference::Connection("saved-slot".into())),
            ..Default::default()
        };
        assert_eq!(
            native::resolve_credential(&connection, None, &vault).unwrap_err(),
            expected.message()
        );
    }
}

#[test]
fn native_error_diagnostics_never_format_the_underlying_error() {
    struct SecretError;
    impl std::fmt::Debug for SecretError {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("Do not debug-format a credential error")
        }
    }
    impl std::fmt::Display for SecretError {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("Do not display a credential error")
        }
    }
    impl std::error::Error for SecretError {}
    let capture = LogCapture::default();
    tracing::subscriber::with_default(capture.clone(), || {
        for error in [
            keyring::Error::PlatformFailure(Box::new(SecretError)),
            keyring::Error::NoStorageAccess(Box::new(SecretError)),
        ] {
            Failure::from_keyring(&error).report(Stage::SwitchPreflight);
        }
    });
    assert_eq!(capture.events("system_one_credential_read_failed").len(), 2);
}

#[test]
fn legacy_startup_classifies_invalid_stored_text_without_a_system_vault() {
    for value in [String::new(), "token\n".into(), "x".repeat(4097)] {
        let vault = ScriptedVault::default();
        vault.push(None, Ok(Some(value)));
        assert_eq!(
            native::read_key_with_vault(&vault).unwrap_err(),
            Category::Malformed.message()
        );
    }
    let vault = ScriptedVault::default();
    vault.push(None, Err(keyring::Error::NoEntry));
    assert_eq!(native::read_key_with_vault(&vault).unwrap(), None);
}

struct LegacyVault {
    entry: keyring::Entry,
    utf16_password: bool,
    fail_password_read: Cell<bool>,
    fail_write: Cell<bool>,
    fail_restore: Cell<bool>,
    writes: Cell<usize>,
}

impl CredentialVault for LegacyVault {
    type Error = keyring::Error;

    fn read(&self, id: Option<&str>) -> ReadResult {
        assert!(id.is_none());
        if self.fail_password_read.replace(false) {
            return Err(platform_error());
        }
        if !self.utf16_password {
            return self.entry.get_password().map(Some);
        }
        let raw = self.entry.get_secret()?;
        if !raw.len().is_multiple_of(2) {
            return Err(keyring::Error::BadEncoding(raw));
        }
        let units = raw
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16(&units)
            .map(Some)
            .map_err(|_| keyring::Error::BadEncoding(raw))
    }

    fn write(&self, id: Option<&str>, value: &str) -> Result<(), Self::Error> {
        assert!(id.is_none());
        self.writes.set(self.writes.get() + 1);
        if self.utf16_password {
            let raw = value
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>();
            self.entry.set_secret(&raw)?;
        } else {
            self.entry.set_password(value)?;
        }
        if self.fail_write.replace(false) {
            return Err(platform_error());
        }
        Ok(())
    }

    fn delete(&self, id: Option<&str>) -> Result<(), Self::Error> {
        assert!(id.is_none());
        self.entry.delete_credential()
    }

    fn read_legacy_snapshot(&self) -> Result<Option<Vec<u8>>, Self::Error> {
        self.entry.get_secret().map(Some)
    }

    fn restore_legacy_snapshot(&self, value: &[u8]) -> Result<(), &'static str> {
        if self.fail_restore.replace(false) {
            return Err("Synthetic byte restore failure.");
        }
        self.entry
            .set_secret(value)
            .map_err(|error| Self::write_failure(&error))
    }

    fn is_missing(error: &Self::Error) -> bool {
        native::SystemCredentialVault::is_missing(error)
    }

    fn is_bad_encoding(error: &Self::Error) -> bool {
        native::SystemCredentialVault::is_bad_encoding(error)
    }

    fn read_failure(error: &Self::Error) -> Failure {
        native::SystemCredentialVault::read_failure(error)
    }

    fn write_failure(error: &Self::Error) -> &'static str {
        native::SystemCredentialVault::write_failure(error)
    }
}

const UNDECODABLE_CREDENTIAL: &[u8] = b"synthetic-credential\xff";

fn legacy_vault(utf16_password: bool) -> LegacyVault {
    LegacyVault {
        entry: keyring::Entry::new_with_credential(Box::new(
            keyring::mock::MockCredential::default(),
        )),
        utf16_password,
        fail_password_read: Cell::new(false),
        fail_write: Cell::new(false),
        fail_restore: Cell::new(false),
        writes: Cell::new(0),
    }
}

fn legacy_setup() -> (Store, WorkerHandle, LegacyVault) {
    let store = Store::open_in_memory(Path::new("synthetic-legacy-byte-recovery")).unwrap();
    let worker = WorkerHandle::default();
    let vault = legacy_vault(false);
    native::save_legacy_key(&store, &worker, Some("initial-key".into()), &vault).unwrap();
    vault.entry.set_secret(UNDECODABLE_CREDENTIAL).unwrap();
    assert!(matches!(
        vault.entry.get_password(),
        Err(keyring::Error::BadEncoding(_))
    ));
    (store, worker, vault)
}

#[test]
fn legacy_windows_password_reuse_and_same_key_save_keep_decoded_key_and_revision() {
    for key in ["synthetic-ascii-key", "synthetic-\u{100}-\u{1f600}"] {
        let store = Store::open_in_memory(Path::new("synthetic-legacy-windows-password")).unwrap();
        let worker = WorkerHandle::default();
        let vault = legacy_vault(true);
        native::save_legacy_key(&store, &worker, Some(key.into()), &vault).unwrap();
        let raw = key
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(vault.entry.get_secret().unwrap(), raw);
        let revision = native::active_connection(&store).unwrap().revision;
        let writes = vault.writes.get();
        for replacement in [None, Some(key.to_owned())] {
            native::save_legacy_key(&store, &worker, replacement, &vault).unwrap();
            assert_eq!(
                worker.execution_snapshot().unwrap().credential.as_deref(),
                Some(key)
            );
            assert_eq!(
                native::active_connection(&store).unwrap().revision,
                revision
            );
            assert_eq!(vault.writes.get(), writes);
            assert_eq!(vault.entry.get_secret().unwrap(), raw);
        }
    }
}

#[test]
fn legacy_windows_replacement_rollback_restores_exact_utf16le_bytes() {
    let store = Store::open_in_memory(Path::new("synthetic-legacy-windows-rollback")).unwrap();
    let worker = WorkerHandle::default();
    let vault = legacy_vault(true);
    native::save_legacy_key(&store, &worker, Some("original-key".into()), &vault).unwrap();
    let raw = vault.entry.get_secret().unwrap();
    let revision = native::active_connection(&store).unwrap().revision;
    super::fail_profile_writes(&store);
    assert!(
        native::save_legacy_key(&store, &worker, Some("replacement-key".into()), &vault).is_err()
    );
    assert_eq!(vault.entry.get_secret().unwrap(), raw);
    assert_eq!(vault.read(None).unwrap().as_deref(), Some("original-key"));
    assert_eq!(
        native::active_connection(&store).unwrap().revision,
        revision
    );
    assert!(native::legacy_removal_pending(&store));
    assert!(!worker.is_available());
    store
        .lock()
        .execute_batch("DROP TRIGGER fail_profiles")
        .unwrap();
    native::save_legacy_key(&store, &worker, Some("replacement-key".into()), &vault).unwrap();
    assert_eq!(
        worker.execution_snapshot().unwrap().credential.as_deref(),
        Some("replacement-key")
    );
    assert_eq!(
        native::active_connection(&store).unwrap().revision,
        revision + 1
    );
}

#[test]
fn legacy_windows_undecodable_password_allows_explicit_replacement_or_removal() {
    for replace in [false, true] {
        let store = Store::open_in_memory(Path::new("synthetic-legacy-windows-recovery")).unwrap();
        let worker = WorkerHandle::default();
        let vault = legacy_vault(true);
        native::save_legacy_key(&store, &worker, Some("original-key".into()), &vault).unwrap();
        vault.entry.set_secret(&[0x00, 0xd8]).unwrap();
        assert!(matches!(
            vault.read(None),
            Err(keyring::Error::BadEncoding(_))
        ));
        assert_eq!(
            native::save_legacy_key(&store, &worker, None, &vault).unwrap_err(),
            Category::Malformed.message()
        );
        recover_legacy_credential(&store, &worker, &vault, replace).unwrap();
        assert!(!native::legacy_removal_pending(&store));
        if replace {
            assert_eq!(
                vault.read(None).unwrap().as_deref(),
                Some("replacement-key")
            );
            assert_eq!(
                worker.execution_snapshot().unwrap().credential.as_deref(),
                Some("replacement-key")
            );
        } else {
            assert!(matches!(
                vault.entry.get_secret(),
                Err(keyring::Error::NoEntry)
            ));
            assert!(!worker.is_available());
        }
    }
}

fn recover_legacy_credential(
    store: &Store,
    worker: &WorkerHandle,
    vault: &LegacyVault,
    replace: bool,
) -> Result<(), &'static str> {
    if replace {
        native::save_legacy_key(store, worker, Some("replacement-key".into()), vault)
    } else {
        let prepared = native::begin_legacy_removal(store, worker)?;
        native::finish_legacy_removal(store, vault, &prepared)
    }
}

#[test]
fn legacy_recovery_can_replace_or_remove_undecodable_credentials() {
    for replace in [false, true] {
        let (store, worker, vault) = legacy_setup();
        let revision = native::active_connection(&store).unwrap().revision;
        assert_eq!(
            native::read_key_with_vault(&vault).unwrap_err(),
            Category::Malformed.message()
        );
        recover_legacy_credential(&store, &worker, &vault, replace).unwrap();
        assert_eq!(
            native::active_connection(&store).unwrap().revision,
            revision + 1
        );
        assert!(!native::legacy_removal_pending(&store));
        assert_eq!(native::saved_key_marker(&store), replace);
        assert_eq!(worker.is_available(), replace);
        if replace {
            assert_eq!(vault.entry.get_password().unwrap(), "replacement-key");
            assert_eq!(
                worker.execution_snapshot().unwrap().credential.as_deref(),
                Some("replacement-key")
            );
        } else {
            assert!(matches!(
                vault.entry.get_secret(),
                Err(keyring::Error::NoEntry)
            ));
            assert!(!native::yaml_discovery_enabled(&store));
        }
    }
}

#[test]
fn legacy_recovery_metadata_failure_restores_exact_credential_bytes() {
    for replace in [false, true] {
        let (store, worker, vault) = legacy_setup();
        let before = store.internal_value(native::CONNECTION_PROFILES_KEY);
        let revision = native::active_connection(&store).unwrap().revision;
        super::fail_profile_writes(&store);
        assert!(recover_legacy_credential(&store, &worker, &vault, replace).is_err());
        assert_eq!(vault.entry.get_secret().unwrap(), UNDECODABLE_CREDENTIAL);
        assert_eq!(
            store.internal_value(native::CONNECTION_PROFILES_KEY),
            before
        );
        assert!(native::legacy_removal_pending(&store));
        assert!(!worker.is_available());
        assert!(
            native::restore_saved_key_with_vault(
                &store,
                &worker,
                || panic!("Pending recovery must not read a credential"),
                &vault
            )
            .is_err()
        );
        store
            .lock()
            .execute_batch("DROP TRIGGER fail_profiles")
            .unwrap();
        recover_legacy_credential(&store, &worker, &vault, replace).unwrap();
        assert_eq!(
            native::active_connection(&store).unwrap().revision,
            revision + 1
        );
        assert!(!native::legacy_removal_pending(&store));
    }
}

#[test]
fn legacy_recovery_failed_byte_rollback_keeps_the_retry_gate() {
    for replace in [false, true] {
        let (store, worker, vault) = legacy_setup();
        let before = store.internal_value(native::CONNECTION_PROFILES_KEY);
        super::fail_profile_writes(&store);
        vault.fail_restore.set(true);
        assert!(
            recover_legacy_credential(&store, &worker, &vault, replace)
                .unwrap_err()
                .contains("restore the API key")
        );
        assert_eq!(
            store.internal_value(native::CONNECTION_PROFILES_KEY),
            before
        );
        assert!(native::legacy_removal_pending(&store));
        assert!(!worker.is_available());
        store
            .lock()
            .execute_batch("DROP TRIGGER fail_profiles")
            .unwrap();
        recover_legacy_credential(&store, &worker, &vault, replace).unwrap();
        assert!(!native::legacy_removal_pending(&store));
    }
}

#[test]
fn legacy_replacement_write_failure_restores_undecodable_bytes() {
    let (store, worker, vault) = legacy_setup();
    let before = store.internal_value(native::CONNECTION_PROFILES_KEY);
    vault.fail_write.set(true);
    assert!(recover_legacy_credential(&store, &worker, &vault, true).is_err());
    assert_eq!(vault.entry.get_secret().unwrap(), UNDECODABLE_CREDENTIAL);
    assert_eq!(
        store.internal_value(native::CONNECTION_PROFILES_KEY),
        before
    );
    assert!(native::legacy_removal_pending(&store));
    assert!(!worker.is_available());
}

#[test]
fn legacy_snapshot_access_failure_keeps_the_credential_and_reports_safe_fields() {
    for replace in [false, true] {
        let (store, worker, vault) = legacy_setup();
        let before = store.internal_value(native::CONNECTION_PROFILES_KEY);
        let generation = worker.execution_snapshot().unwrap().generation;
        vault
            .entry
            .get_credential()
            .downcast_ref::<keyring::mock::MockCredential>()
            .unwrap()
            .set_error(platform_error());
        let capture = LogCapture::default();
        let result = tracing::subscriber::with_default(capture.clone(), || {
            recover_legacy_credential(&store, &worker, &vault, replace)
        });
        assert_eq!(result.unwrap_err(), Category::Platform.message());
        assert_eq!(vault.entry.get_secret().unwrap(), UNDECODABLE_CREDENTIAL);
        assert_eq!(
            store.internal_value(native::CONNECTION_PROFILES_KEY),
            before
        );
        if replace {
            assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
            assert!(!native::legacy_removal_pending(&store));
        } else {
            assert!(!worker.is_available());
            assert!(native::legacy_removal_pending(&store));
        }
        let diagnostics = capture.events("system_one_credential_read_failed");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].fields["stage"],
            if replace {
                "\"legacy_update\""
            } else {
                "\"credential_removal\""
            }
        );
        assert_eq!(diagnostics[0].fields["category"], "\"platform\"");
        assert!(!capture.text().contains("secret-sentinel"));
        assert!(!capture.text().contains("synthetic-credential"));
    }
}

#[test]
fn legacy_replacement_does_not_bypass_a_password_read_access_failure() {
    let (store, worker, vault) = legacy_setup();
    let profiles = store.internal_value(native::CONNECTION_PROFILES_KEY);
    let generation = worker.execution_snapshot().unwrap().generation;
    vault.fail_password_read.set(true);
    assert_eq!(
        recover_legacy_credential(&store, &worker, &vault, true).unwrap_err(),
        Category::Platform.message()
    );
    assert_eq!(vault.entry.get_secret().unwrap(), UNDECODABLE_CREDENTIAL);
    assert_eq!(
        store.internal_value(native::CONNECTION_PROFILES_KEY),
        profiles
    );
    assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
    assert!(!native::legacy_removal_pending(&store));
}

#[test]
fn legacy_key_reuse_rejects_undecodable_bytes_without_mutation() {
    let (store, worker, vault) = legacy_setup();
    let before = store.internal_value(native::CONNECTION_PROFILES_KEY);
    let generation = worker.execution_snapshot().unwrap().generation;
    assert_eq!(
        native::save_legacy_key(&store, &worker, None, &vault).unwrap_err(),
        Category::Malformed.message()
    );
    assert_eq!(vault.entry.get_secret().unwrap(), UNDECODABLE_CREDENTIAL);
    assert_eq!(
        store.internal_value(native::CONNECTION_PROFILES_KEY),
        before
    );
    assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
    assert!(!native::legacy_removal_pending(&store));
}

#[test]
fn invalid_reference_fails_before_the_vault_or_any_switch_mutation() {
    for id in [String::new(), "x".repeat(97), "invalid/reference".into()] {
        let (store, worker) = setup(CredentialReference::Connection(id), true);
        let vault = ScriptedVault::default();
        let before = store.internal_value(native::CONNECTION_PROFILES_KEY);
        let generation = worker.execution_snapshot().unwrap().generation;
        assert_eq!(
            native::lifecycle::switch_connection_recording(
                &store,
                &worker,
                "target",
                &vault,
                |_| {}
            )
            .unwrap_err(),
            Category::InvalidReference.message()
        );
        assert!(vault.reads.borrow().is_empty());
        assert_eq!(
            store.internal_value(native::CONNECTION_PROFILES_KEY),
            before
        );
        assert!(store.internal_value(native::operation::KEY).is_none());
        assert_eq!(worker.execution_snapshot().unwrap().generation, generation);
    }
}

#[test]
fn normal_use_reads_each_exact_reference_once_even_if_a_second_read_would_fail() {
    for reference in references() {
        let (store, worker) = setup(reference.clone(), true);
        let vault = ScriptedVault::default();
        vault.push(slot(&reference), Ok(Some("selected-secret".into())));
        vault.push(slot(&reference), Err(platform_error()));
        let mut events = vec![];
        let settings = native::lifecycle::switch_connection_recording(
            &store,
            &worker,
            "target",
            &vault,
            |event| events.push(event),
        )
        .unwrap();
        assert_eq!(settings.active_id.as_deref(), Some("target"));
        assert!(settings.pending_operation.is_none());
        assert_eq!(
            worker.execution_snapshot().unwrap().credential.as_deref(),
            Some("selected-secret")
        );
        assert_eq!(
            vault.reads.borrow().as_slice(),
            &[slot(&reference).map(str::to_owned)]
        );
        assert_eq!(vault.script.borrow().len(), 1);
        assert_eq!(events.len(), 1);
    }
}

#[test]
fn use_while_off_still_validates_a_required_credential_once() {
    for succeeds in [false, true] {
        let (store, worker) = setup(CredentialReference::Connection("saved-slot".into()), false);
        let vault = ScriptedVault::default();
        vault.push(
            Some("saved-slot"),
            if succeeds {
                Ok(Some("selected-secret".into()))
            } else {
                Err(platform_error())
            },
        );
        let result = native::lifecycle::switch_connection_recording(
            &store,
            &worker,
            "target",
            &vault,
            |_| {},
        );
        assert_eq!(result.is_ok(), succeeds);
        assert_eq!(vault.reads.borrow().len(), 1);
        assert!(!worker.is_available());
        assert!(!native::yaml_discovery_enabled(&store));
        let saved = settings_snapshot(&store).unwrap();
        assert_eq!(
            saved.active_id.as_deref(),
            Some(if succeeds { "target" } else { "previous" })
        );
        assert!(saved.pending_operation.is_none());
    }
}

#[test]
fn keyless_custom_use_skips_the_vault() {
    let (store, worker) = setup(CredentialReference::LegacyTypeSafe, true);
    let mut saved = native::profiles(&store).unwrap();
    saved.profiles.insert("keyless".into(), custom_connection());
    native::save_profiles(&store, &saved).unwrap();
    let vault = ScriptedVault::default();
    native::lifecycle::switch_connection_recording(&store, &worker, "keyless", &vault, |_| {})
        .unwrap();
    assert!(vault.reads.borrow().is_empty());
    assert!(worker.execution_snapshot().unwrap().credential.is_none());
}

fn fail_final_clear(store: &Store) {
    store.lock().execute_batch("CREATE TEMP TRIGGER credential_read_fault BEFORE UPDATE ON setting WHEN NEW.key = 'internal:smartChecksProfileOperationV1' AND NEW.value = '' BEGIN SELECT RAISE(ABORT, 'synthetic final clear failure'); END;").unwrap();
}

fn clear_fault(store: &Store) {
    store
        .lock()
        .execute_batch("DROP TRIGGER credential_read_fault")
        .unwrap();
}

#[test]
fn token_retry_reads_fresh_and_keeps_dispatch_suspended_until_final_clear() {
    for reference in references() {
        let (store, worker) = setup(reference.clone(), true);
        let vault = ScriptedVault::default();
        vault.push(slot(&reference), Ok(Some("preflight-secret".into())));
        fail_final_clear(&store);
        assert!(
            native::lifecycle::switch_connection_recording(
                &store,
                &worker,
                "target",
                &vault,
                |_| {}
            )
            .is_err()
        );
        assert_eq!(vault.reads.borrow().len(), 1);
        assert!(worker.execution_snapshot().is_none());
        let pending = settings_snapshot(&store)
            .unwrap()
            .pending_operation
            .unwrap();
        assert_eq!(
            settings_snapshot(&store).unwrap().active_id.as_deref(),
            Some("target")
        );
        clear_fault(&store);
        vault.push(slot(&reference), Err(platform_error()));
        assert!(retry_recording(&store, &worker, &pending.id, &vault, |_| {}).is_err());
        assert_eq!(
            settings_snapshot(&store)
                .unwrap()
                .pending_operation
                .unwrap()
                .id,
            pending.id
        );
        assert!(worker.execution_snapshot().is_none());
        vault.push(slot(&reference), Ok(Some("fresh-retry-secret".into())));
        let mut events = vec![];
        let result = retry_recording(&store, &worker, &pending.id, &vault, |event| {
            events.push(event)
        })
        .unwrap();
        assert_eq!(result.outcome, OperationOutcome::Completed);
        assert!(result.settings.pending_operation.is_none());
        assert_eq!(
            worker.execution_snapshot().unwrap().credential.as_deref(),
            Some("fresh-retry-secret")
        );
        assert_eq!(vault.reads.borrow().len(), 3);
        assert_eq!(events.len(), 1);
        assert!(retry_recording(&store, &worker, &pending.id, &vault, |_| {}).is_err());
        assert_eq!(vault.reads.borrow().len(), 3);
    }
}

#[test]
fn startup_recovery_never_reuses_the_failed_switch_memory() {
    for reference in references() {
        let (store, worker) = setup(reference.clone(), true);
        let vault = ScriptedVault::default();
        vault.push(slot(&reference), Ok(Some("preflight-secret".into())));
        fail_final_clear(&store);
        assert!(
            native::lifecycle::switch_connection_recording(
                &store,
                &worker,
                "target",
                &vault,
                |_| {}
            )
            .is_err()
        );
        clear_fault(&store);
        store.set_internal_value(native::AUTOMATIC_LIMITS_MIGRATION_KEY, "0");
        let token = settings_snapshot(&store)
            .unwrap()
            .pending_operation
            .unwrap()
            .id;
        let restarted = WorkerHandle::default();
        vault.push(slot(&reference), Err(platform_error()));
        assert!(
            native::restore_saved_key_with_vault(
                &store,
                &restarted,
                || native::read_key_with_vault(&vault),
                &vault
            )
            .is_err()
        );
        assert_eq!(
            settings_snapshot(&store)
                .unwrap()
                .pending_operation
                .unwrap()
                .id,
            token
        );
        assert!(restarted.execution_snapshot().is_none());
        vault.push(slot(&reference), Ok(Some("fresh-startup-secret".into())));
        vault.push(slot(&reference), Err(platform_error()));
        native::restore_saved_key_with_vault(
            &store,
            &restarted,
            || native::read_key_with_vault(&vault),
            &vault,
        )
        .unwrap();
        assert!(
            settings_snapshot(&store)
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
            Some("fresh-startup-secret")
        );
        assert_eq!(
            vault.reads.borrow().as_slice(),
            &[
                slot(&reference).map(str::to_owned),
                slot(&reference).map(str::to_owned),
                slot(&reference).map(str::to_owned)
            ]
        );
        assert_eq!(vault.script.borrow().len(), 1);
        assert_eq!(
            store
                .internal_value(native::AUTOMATIC_LIMITS_MIGRATION_KEY)
                .as_deref(),
            Some("1")
        );
    }
}

#[test]
fn normal_startup_without_a_pending_operation_reads_the_selected_credential_once() {
    for reference in references() {
        let (store, _) = setup(reference.clone(), true);
        let mut saved = native::profiles(&store).unwrap();
        saved.active_id = Some("target".into());
        native::save_profiles(&store, &saved).unwrap();
        let vault = ScriptedVault::default();
        vault.push(slot(&reference), Ok(Some("fresh-startup-secret".into())));
        vault.push(slot(&reference), Err(platform_error()));
        let restarted = WorkerHandle::default();
        native::restore_saved_key_with_vault(
            &store,
            &restarted,
            || native::read_key_with_vault(&vault),
            &vault,
        )
        .unwrap();
        assert_eq!(
            vault.reads.borrow().as_slice(),
            &[slot(&reference).map(str::to_owned)]
        );
        assert_eq!(vault.script.borrow().len(), 1);
        assert_eq!(
            restarted
                .execution_snapshot()
                .unwrap()
                .credential
                .as_deref(),
            Some("fresh-startup-secret")
        );
    }
}

#[test]
fn inactive_startup_cleanup_still_reads_the_selected_credential_once() {
    let (store, worker) = setup(CredentialReference::Connection("active-slot".into()), true);
    let mut saved = native::profiles(&store).unwrap();
    saved.active_id = Some("target".into());
    saved.profiles.get_mut("previous").unwrap().credential =
        Some(CredentialReference::Connection("inactive-slot".into()));
    native::save_profiles(&store, &saved).unwrap();
    worker
        .set_system_one_connection(
            saved.profiles["target"].clone(),
            Some("previous-active-secret".into()),
        )
        .unwrap();
    let vault = ScriptedVault::default();
    vault
        .delete_script
        .borrow_mut()
        .push_back(Err(platform_error()));
    assert!(
        native::lifecycle::remove_connection_recording(
            &store,
            &worker,
            "previous",
            1,
            &vault,
            |_| {}
        )
        .is_err()
    );
    assert!(!native::lifecycle::pending_affects_active(&store));
    vault.delete_script.borrow_mut().push_back(Ok(()));
    vault.push(Some("active-slot"), Ok(Some("fresh-active-secret".into())));
    vault.push(Some("active-slot"), Err(platform_error()));
    let restarted = WorkerHandle::default();
    native::restore_saved_key_with_vault(
        &store,
        &restarted,
        || panic!("A connection slot must not read the legacy key"),
        &vault,
    )
    .unwrap();
    assert_eq!(
        vault.deletes.borrow().as_slice(),
        &[Some("inactive-slot".into()), Some("inactive-slot".into())]
    );
    assert_eq!(
        vault.reads.borrow().as_slice(),
        &[Some("active-slot".into())]
    );
    assert_eq!(vault.script.borrow().len(), 1);
    assert!(
        settings_snapshot(&store)
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
        Some("fresh-active-secret")
    );
}

#[test]
fn startup_migration_does_not_keep_a_runtime_for_the_old_profile_revision() {
    for succeeds in [false, true] {
        let target = SystemOneConnection {
            provider: native::SystemOneProvider::Cloudflare,
            endpoint: native::SystemOneEndpoint::CloudflareAccount("synthetic-account".into()),
            model: "clef-flash".into(),
            response_mode: crate::jev::config::SystemOneResponseMode::CloudflareEnvelope,
            credential: Some(CredentialReference::Connection("saved-slot".into())),
            context_override: Some(crate::jev::config::ContextLimitOverride {
                total_input_tokens: Some(8192),
                state_and_longest_question_tokens: None,
                runtime_context_tokens: None,
            }),
            ..Default::default()
        };
        let original_revision = target.revision;
        let (store, worker) = setup_with_target(target, true);
        let vault = ScriptedVault::default();
        vault.push(Some("saved-slot"), Ok(Some("preflight-secret".into())));
        fail_final_clear(&store);
        assert!(
            native::lifecycle::switch_connection_recording(
                &store,
                &worker,
                "target",
                &vault,
                |_| {}
            )
            .is_err()
        );
        clear_fault(&store);
        store.set_internal_value(native::AUTOMATIC_LIMITS_MIGRATION_KEY, "0");
        vault.push(Some("saved-slot"), Ok(Some("fresh-recovery-secret".into())));
        vault.push(
            Some("saved-slot"),
            if succeeds {
                Ok(Some("fresh-migrated-secret".into()))
            } else {
                Err(platform_error())
            },
        );
        let restarted = WorkerHandle::default();
        let result = native::restore_saved_key_with_vault(
            &store,
            &restarted,
            || panic!("A connection slot must not read the legacy key"),
            &vault,
        );
        assert_eq!(result.is_ok(), succeeds);
        let settings = settings_snapshot(&store).unwrap();
        let migrated = &settings.profiles["target"];
        assert!(settings.pending_operation.is_none());
        assert_eq!(migrated.revision, original_revision + 1);
        assert!(migrated.context_override.is_none());
        assert_eq!(
            vault.reads.borrow().as_slice(),
            &[
                Some("saved-slot".into()),
                Some("saved-slot".into()),
                Some("saved-slot".into())
            ]
        );
        if succeeds {
            let runtime = restarted.execution_snapshot().unwrap();
            assert_eq!(&runtime.connection, migrated);
            assert_eq!(runtime.credential.as_deref(), Some("fresh-migrated-secret"));
        } else {
            assert!(restarted.execution_snapshot().is_none());
        }
    }
}

#[test]
fn a_prepared_switch_credential_cannot_bind_to_another_reference_or_revision() {
    for change_reference in [false, true] {
        let (store, worker) = setup(CredentialReference::Connection("saved-slot".into()), true);
        let vault = ScriptedVault::default();
        vault.push(Some("saved-slot"), Ok(Some("preflight-secret".into())));
        fail_final_clear(&store);
        assert!(
            native::lifecycle::switch_connection_recording(
                &store,
                &worker,
                "target",
                &vault,
                |_| {}
            )
            .is_err()
        );
        clear_fault(&store);
        let pending = native::operation::metadata(&store)
            .unwrap()
            .pending
            .unwrap();
        let mut connection = pending.after.profiles["target"].clone();
        if change_reference {
            connection.credential = Some(CredentialReference::Connection("wrong-slot".into()));
        } else {
            connection.revision += 1;
        }
        let id = if change_reference {
            "wrong-slot"
        } else {
            "saved-slot"
        };
        vault.push(Some(id), Ok(Some("wrong-binding-secret".into())));
        let prepared =
            native::operation::ValidatedSwitchCredential::resolve(&connection, &vault).unwrap();
        assert!(
            native::operation::settle_switch(&store, &worker, pending, &vault, prepared).is_err()
        );
        assert!(worker.execution_snapshot().is_none());
        assert!(
            settings_snapshot(&store)
                .unwrap()
                .pending_operation
                .is_some()
        );
        assert_eq!(vault.reads.borrow().len(), 2);
    }
}
