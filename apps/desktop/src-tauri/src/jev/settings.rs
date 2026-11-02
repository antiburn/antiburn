//! Native TypeSafe credentials and authorized window controls.

use antiburn_local::checks::DetectorId;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use crate::dto::BurnCheckDetectorId;
use crate::insights_report::installed_checks::{
    InstalledCheckAvailabilityPayload, InstalledCheckDefinition, availability_rows,
};
use crate::jev::config::{
    CredentialReference, SystemOneConnection, SystemOneEndpoint, SystemOneProvider,
};
use crate::jev::worker::WorkerHandle;
use crate::store::{BurnCheckUsageSummary, Store};

const YAML_DISCOVERY_ENABLED_KEY: &str = "internal:yamlBurnChecksDiscoveryEnabledV1";
const SAVED_KEY_KEY: &str = "internal:typesafeKeySavedV1";
const AUTH_REJECTED_KEY: &str = "internal:typesafeAuthRejectedV1";
const CREDENTIAL_CHANGE_PENDING_KEY: &str = "internal:typesafeCredentialChangePendingV1";
const LEGACY_REMOVAL_PENDING_KEY: &str = "internal:typesafeCredentialRemovalPendingV1";
const ACTIVE_CONNECTION_KEY: &str = "internal:smartChecksActiveConnectionV1";
const CONNECTION_PROFILES_KEY: &str = "internal:smartChecksConnectionsV1";
const PROVIDER_MIGRATION_KEY: &str = "internal:smartChecksProviderMigrationV1";
const AUTOMATIC_LIMITS_MIGRATION_KEY: &str = "internal:smartChecksAutomaticLimitsV1";
#[cfg(debug_assertions)]
const SERVICE: &str = "ai.antiburn.desktop.debug.typesafe";
#[cfg(not(debug_assertions))]
const SERVICE: &str = "ai.antiburn.desktop.typesafe";
const ACCOUNT: &str = "ignored-instructions";
const CONNECTION_PENDING_KEY: &str = "internal:smartChecksConnectionChangePendingV1";
const CONNECTION_REMOVAL_KEY: &str = "internal:smartChecksCredentialRemovalPendingV1";
pub(crate) const AVAILABILITY_EVENT: &str = "checks:availability-changed";
static CREDENTIAL_CHANGE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static CHECK_CHANGE: std::sync::Mutex<()> = std::sync::Mutex::new(());
static STARTUP_ERRORS: StartupErrors = StartupErrors::new();
#[cfg(test)]
static STARTUP_ERROR: &std::sync::Mutex<Option<&'static str>> = &STARTUP_ERRORS.unbound;

struct StartupRecoveryError {
    origin: StartupRecoveryOrigin,
    message: &'static str,
}

enum StartupRecoveryOrigin {
    Operation(String),
    SelectedConnection,
    LegacyCredentialChange,
    SelectedLegacyCredential,
}

#[derive(Debug)]
enum StartupRestoreFailure {
    Unbound(&'static str),
    SelectedConnection(&'static str),
    LegacyCredentialChange(&'static str),
    SelectedLegacyCredential(&'static str),
}

impl StartupRestoreFailure {
    fn message(&self) -> &'static str {
        match self {
            Self::Unbound(message)
            | Self::SelectedConnection(message)
            | Self::LegacyCredentialChange(message)
            | Self::SelectedLegacyCredential(message) => message,
        }
    }
}

impl From<&'static str> for StartupRestoreFailure {
    fn from(message: &'static str) -> Self {
        Self::Unbound(message)
    }
}

struct StartupErrors {
    unbound: std::sync::Mutex<Option<&'static str>>,
    recovery: std::sync::Mutex<Option<StartupRecoveryError>>,
}

impl StartupErrors {
    const fn new() -> Self {
        Self {
            unbound: std::sync::Mutex::new(None),
            recovery: std::sync::Mutex::new(None),
        }
    }

    fn set_unbound(&self, message: Option<&'static str>) {
        let mut unbound = self
            .unbound
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut recovery = self
            .recovery
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *unbound = message;
        *recovery = None;
    }

    fn record_restore_result(&self, store: &Store, result: Result<(), StartupRestoreFailure>) {
        let failure = result.err();
        let origin = failure.as_ref().and_then(|failure| match failure {
            StartupRestoreFailure::LegacyCredentialChange(_) => {
                Some(StartupRecoveryOrigin::LegacyCredentialChange)
            }
            _ => operation::active_pending_operation_id(store)
                .map(StartupRecoveryOrigin::Operation)
                .or(match failure {
                    StartupRestoreFailure::SelectedConnection(_) => {
                        Some(StartupRecoveryOrigin::SelectedConnection)
                    }
                    StartupRestoreFailure::SelectedLegacyCredential(_) => {
                        Some(StartupRecoveryOrigin::SelectedLegacyCredential)
                    }
                    _ => None,
                }),
        });
        let mut unbound = self
            .unbound
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut recovery = self
            .recovery
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(origin) = origin {
            *unbound = None;
            *recovery = failure.map(|failure| StartupRecoveryError {
                origin,
                message: failure.message(),
            });
        } else {
            *unbound = failure.map(|failure| failure.message());
            *recovery = None;
        }
    }

    fn clear_resolved_connection(&self, operation_id: &str, committed: bool) {
        let mut recovery = self
            .recovery
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if recovery.as_ref().is_some_and(|error| match &error.origin {
            StartupRecoveryOrigin::Operation(id) => id == operation_id,
            StartupRecoveryOrigin::SelectedConnection
            | StartupRecoveryOrigin::SelectedLegacyCredential => committed,
            StartupRecoveryOrigin::LegacyCredentialChange => false,
        }) {
            *recovery = None;
        }
    }

    fn record_legacy_removal_result(
        &self,
        active_uses_legacy_key: bool,
        result: Result<(), &'static str>,
    ) {
        let mut recovery = self
            .recovery
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let legacy_error = recovery.as_ref().is_some_and(|error| match &error.origin {
            StartupRecoveryOrigin::LegacyCredentialChange => true,
            StartupRecoveryOrigin::SelectedLegacyCredential => active_uses_legacy_key,
            _ => false,
        });
        match result {
            Ok(()) if legacy_error => *recovery = None,
            Err(_) if active_uses_legacy_key && (recovery.is_none() || legacy_error) => {
                *recovery = Some(StartupRecoveryError {
                    origin: StartupRecoveryOrigin::LegacyCredentialChange,
                    message: "Could not remove the API key from credential storage.",
                });
            }
            _ => {}
        }
    }

    fn message(&self) -> Option<&'static str> {
        let unbound = self
            .unbound
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        unbound.or_else(|| {
            self.recovery
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
                .map(|error| error.message)
        })
    }
}

mod lifecycle;
use lifecycle::SaveConnectionIntent;
mod credential_read;
use credential_read::{
    Category as CredentialReadCategory, Failure as CredentialReadFailure,
    Stage as CredentialReadStage,
};
mod display_numbers;
mod operation;
#[cfg(test)]
use lifecycle::save_connection_with_intent;

pub(crate) fn yaml_discovery_enabled(store: &Store) -> bool {
    match store
        .lock()
        .query_row(
            "SELECT value FROM setting WHERE key = ?1",
            [YAML_DISCOVERY_ENABLED_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()
    {
        Ok(None) => true,
        Ok(Some(value)) => value == "true",
        Err(_) => false,
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckAvailability {
    yaml_enabled: bool,
    configured: bool,
    saved_key: bool,
    error: Option<&'static str>,
    usage: BurnCheckUsageSummary,
    checks: Vec<CheckChoice>,
    installed_checks: Vec<InstalledCheckAvailabilityPayload>,
    revision: u64,
}

impl CheckAvailability {
    pub(crate) fn with_installed_checks(
        mut self,
        definitions: &[InstalledCheckDefinition],
    ) -> Self {
        self.installed_checks = availability_rows(definitions);
        self
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckChoice {
    id: BurnCheckDetectorId,
    enabled: bool,
}

#[derive(Clone, Serialize)]
#[serde(tag = "status", content = "snapshot", rename_all = "snake_case")]
enum CheckAvailabilityEvent {
    Updated(Box<CheckAvailability>),
    Failed,
}

fn entry() -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, ACCOUNT)
}

trait CredentialVault {
    type Error;

    fn read(&self, id: Option<&str>) -> Result<Option<String>, Self::Error>;
    fn write(&self, id: Option<&str>, value: &str) -> Result<(), Self::Error>;
    fn delete(&self, id: Option<&str>) -> Result<(), Self::Error>;
    fn read_legacy_snapshot(&self) -> Result<Option<Vec<u8>>, Self::Error> {
        self.read(None).map(|value| value.map(String::into_bytes))
    }
    fn restore_legacy_snapshot(&self, value: &[u8]) -> Result<(), &'static str> {
        let value = std::str::from_utf8(value)
            .map_err(|_| "The saved credential has an invalid format.")?;
        self.write(None, value)
            .map_err(|error| Self::write_failure(&error))
    }
    fn is_missing(error: &Self::Error) -> bool;
    fn is_bad_encoding(_error: &Self::Error) -> bool {
        false
    }
    fn read_failure(_error: &Self::Error) -> CredentialReadFailure {
        CredentialReadFailure::new(CredentialReadCategory::Unavailable)
    }
    fn write_failure(error: &Self::Error) -> &'static str;
}

struct SystemCredentialVault;

impl CredentialVault for SystemCredentialVault {
    type Error = keyring::Error;

    fn read(&self, id: Option<&str>) -> Result<Option<String>, Self::Error> {
        let entry = match id {
            Some(id) => connection_entry(id)?,
            None => entry()?,
        };
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn write(&self, id: Option<&str>, value: &str) -> Result<(), Self::Error> {
        let entry = match id {
            Some(id) => connection_entry(id)?,
            None => entry()?,
        };
        entry.set_password(value)
    }

    fn delete(&self, id: Option<&str>) -> Result<(), Self::Error> {
        let entry = match id {
            Some(id) => connection_entry(id)?,
            None => entry()?,
        };
        entry.delete_credential()
    }

    fn read_legacy_snapshot(&self) -> Result<Option<Vec<u8>>, Self::Error> {
        entry()?.get_secret().map(Some)
    }

    fn restore_legacy_snapshot(&self, value: &[u8]) -> Result<(), &'static str> {
        entry()
            .and_then(|entry| entry.set_secret(value))
            .map_err(|error| Self::write_failure(&error))
    }

    fn is_missing(error: &Self::Error) -> bool {
        matches!(error, keyring::Error::NoEntry)
    }

    fn is_bad_encoding(error: &Self::Error) -> bool {
        matches!(error, keyring::Error::BadEncoding(_))
    }

    fn read_failure(error: &Self::Error) -> CredentialReadFailure {
        CredentialReadFailure::from_keyring(error)
    }

    fn write_failure(error: &Self::Error) -> &'static str {
        match error {
            keyring::Error::NoStorageAccess(_) => {
                "Could not access secure storage. Unlock the system credential store and retry."
            }
            keyring::Error::PlatformFailure(_) => {
                "Secure storage rejected the credential write. Check app access and retry."
            }
            _ => {
                "Could not save the API token to secure storage. Check credential storage access and retry."
            }
        }
    }
}

fn read_credential<V: CredentialVault>(
    vault: &V,
    id: Option<&str>,
    stage: CredentialReadStage,
) -> Result<Option<String>, &'static str> {
    match vault.read(id) {
        Ok(value) => Ok(value),
        Err(error) if V::is_missing(&error) => Ok(None),
        Err(error) => Err(V::read_failure(&error).report(stage)),
    }
}

fn read_legacy_snapshot<V: CredentialVault>(
    vault: &V,
    stage: CredentialReadStage,
) -> Result<Option<Vec<u8>>, &'static str> {
    match vault.read_legacy_snapshot() {
        Ok(value) => Ok(value),
        Err(error) if V::is_missing(&error) => Ok(None),
        Err(error) => Err(V::read_failure(&error).report(stage)),
    }
}

fn write_credential<V: CredentialVault>(
    vault: &V,
    id: Option<&str>,
    value: &str,
) -> Result<(), &'static str> {
    vault
        .write(id, value)
        .map_err(|error| V::write_failure(&error))
}

fn delete_credential<V: CredentialVault>(vault: &V, id: Option<&str>) -> Result<(), &'static str> {
    vault
        .delete(id)
        .or_else(|error| {
            if V::is_missing(&error) {
                Ok(())
            } else {
                Err(error)
            }
        })
        .map_err(|_| "Could not remove the connection credential.")
}

fn read_key() -> Result<Option<String>, &'static str> {
    read_key_with_vault(&SystemCredentialVault)
}

fn read_key_with_vault(vault: &impl CredentialVault) -> Result<Option<String>, &'static str> {
    let credential = read_credential(vault, None, CredentialReadStage::StartupLegacy)?;
    if credential.as_ref().is_some_and(|secret| {
        secret.trim().is_empty() || secret.len() > 4096 || secret.chars().any(char::is_control)
    }) {
        return Err(
            CredentialReadFailure::new(CredentialReadCategory::Malformed)
                .report(CredentialReadStage::StartupLegacy),
        );
    }
    Ok(credential)
}

fn saved_key_marker(store: &Store) -> bool {
    store.internal_value(SAVED_KEY_KEY).as_deref() == Some("true") || legacy_removal_pending(store)
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ConnectionProfiles {
    #[serde(deserialize_with = "Option::<String>::deserialize")]
    active_id: Option<String>,
    profiles: std::collections::BTreeMap<String, SystemOneConnection>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    retired_ids: std::collections::BTreeSet<String>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct SystemOneDraft {
    pub connection_id: String,
    pub connection: SystemOneConnection,
    pub credential: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SystemOneSettings {
    active_id: Option<String>,
    profiles: std::collections::BTreeMap<String, SystemOneConnection>,
    pending_operation: Option<operation::PendingOperation>,
    display_numbers: std::collections::BTreeMap<String, u32>,
}

fn connection_entry(id: &str) -> keyring::Result<keyring::Entry> {
    validate_connection_id(id).map_err(|_| {
        keyring::Error::Invalid("credential reference".into(), "unsupported format".into())
    })?;
    keyring::Entry::new(SERVICE, &format!("system-one-{id}"))
}

fn validate_connection_id(id: &str) -> Result<(), &'static str> {
    if id.is_empty()
        || id.len() > 96
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err("The connection credential reference is invalid.");
    }
    Ok(())
}

fn read_connection_credential(
    connection: &SystemOneConnection,
) -> Result<Option<String>, &'static str> {
    resolve_credential(connection, None, &SystemCredentialVault)
}

fn resolve_credential(
    connection: &SystemOneConnection,
    draft: Option<String>,
    vault: &impl CredentialVault,
) -> Result<Option<String>, &'static str> {
    resolve_credential_at(connection, draft, vault, CredentialReadStage::Resolve)
}

fn resolve_credential_at(
    connection: &SystemOneConnection,
    draft: Option<String>,
    vault: &impl CredentialVault,
    stage: CredentialReadStage,
) -> Result<Option<String>, &'static str> {
    connection.validate().map_err(|error| match error {
        crate::jev::config::ConnectionValidationError::InvalidCredentialReference => {
            CredentialReadFailure::new(CredentialReadCategory::InvalidReference).report(stage)
        }
        _ => "Connection settings are invalid.",
    })?;
    resolve_validated_credential_at(connection, draft, vault, stage)
}

fn resolve_validated_credential(
    connection: &SystemOneConnection,
    draft: Option<String>,
    vault: &impl CredentialVault,
) -> Result<Option<String>, &'static str> {
    resolve_validated_credential_at(connection, draft, vault, CredentialReadStage::Resolve)
}

fn resolve_validated_credential_at(
    connection: &SystemOneConnection,
    draft: Option<String>,
    vault: &impl CredentialVault,
    stage: CredentialReadStage,
) -> Result<Option<String>, &'static str> {
    let stored = draft.is_none();
    let credential = if draft.is_some() {
        draft
    } else {
        match connection.credential.as_ref() {
            Some(CredentialReference::LegacyTypeSafe) => read_credential(vault, None, stage),
            Some(CredentialReference::Connection(id)) => read_credential(vault, Some(id), stage),
            None => return Ok(None),
        }?
    };
    if credential.as_ref().is_some_and(|secret| {
        secret.trim().is_empty() || secret.len() > 4096 || secret.chars().any(char::is_control)
    }) {
        return Err(if stored {
            CredentialReadFailure::new(CredentialReadCategory::Malformed).report(stage)
        } else {
            "Enter a valid connection credential."
        });
    }
    credential
        .map(Some)
        .ok_or_else(|| CredentialReadFailure::new(CredentialReadCategory::Missing).report(stage))
}

fn ensure_no_pending_removal(store: &Store) -> Result<(), &'static str> {
    if store
        .internal_value(CONNECTION_REMOVAL_KEY)
        .is_some_and(|id| !id.is_empty())
    {
        return Err("Connection credential removal is incomplete. Retry it in Settings → Checks.");
    }
    Ok(())
}

fn ensure_no_pending_credential_change(store: &Store) -> Result<(), &'static str> {
    lifecycle::ensure_no_pending_change(store)?;
    ensure_no_pending_removal(store)?;
    if legacy_removal_pending(store) {
        return Err("TypeSafe credential removal is incomplete. Retry it in Settings → Checks.");
    }
    if store
        .internal_value(CONNECTION_PENDING_KEY)
        .is_some_and(|pending| !pending.is_empty())
    {
        return Err(
            "Smart Burn Checks connection update is incomplete. Retry it in Settings → Checks.",
        );
    }
    Ok(())
}

fn legacy_removal_pending(store: &Store) -> bool {
    store
        .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
        .as_deref()
        == Some("true")
        || store.internal_value(LEGACY_REMOVAL_PENDING_KEY).as_deref() == Some("true")
}

fn begin_legacy_removal(
    store: &Store,
    worker: &WorkerHandle,
) -> Result<ConnectionProfiles, &'static str> {
    let mut saved = profiles(store)?;
    advance_legacy_connection_revisions(&mut saved)?;
    let active_uses_legacy_key = saved
        .active_id
        .as_ref()
        .and_then(|id| saved.profiles.get(id))
        .is_some_and(|active| {
            matches!(active.credential, Some(CredentialReference::LegacyTypeSafe))
        });
    store
        .set_internal_values_checked(&[
            (LEGACY_REMOVAL_PENDING_KEY, "true"),
            (
                CREDENTIAL_CHANGE_PENDING_KEY,
                if active_uses_legacy_key {
                    "true"
                } else {
                    "false"
                },
            ),
        ])
        .map_err(|_| "Could not protect the credential removal state.")?;
    if active_uses_legacy_key {
        pause_model_checks(store, worker)?;
    }
    Ok(saved)
}

fn finish_legacy_removal<V: CredentialVault>(
    store: &Store,
    vault: &V,
    saved: &ConnectionProfiles,
) -> Result<(), &'static str> {
    let encoded =
        serde_json::to_string(saved).map_err(|_| "Could not save the credential removal state.")?;
    let previous = read_legacy_snapshot(vault, CredentialReadStage::CredentialRemoval)?;
    delete_credential(vault, None)?;
    if store
        .set_internal_values_checked(&[
            (CONNECTION_PROFILES_KEY, &encoded),
            (SAVED_KEY_KEY, "false"),
            (CREDENTIAL_CHANGE_PENDING_KEY, "false"),
            (LEGACY_REMOVAL_PENDING_KEY, "false"),
        ])
        .is_err()
    {
        if let Some(secret) = previous
            && vault.restore_legacy_snapshot(&secret).is_err()
        {
            return Err(
                "Could not restore the API key. Retry credential removal in Settings → Checks.",
            );
        }
        return Err("Could not save the credential removal state.");
    }
    Ok(())
}

fn profiles(store: &Store) -> Result<ConnectionProfiles, &'static str> {
    migrate_provider_state(store)?;
    let saved: ConnectionProfiles = store
        .internal_value(CONNECTION_PROFILES_KEY)
        .and_then(|json| serde_json::from_str(&json).ok())
        .ok_or("Saved Smart Burn Checks provider settings are invalid.")?;
    if saved
        .active_id
        .as_ref()
        .is_some_and(|id| !saved.profiles.contains_key(id))
    {
        return Err("The active Smart Burn Checks connection is missing.");
    }
    Ok(saved)
}

fn save_profiles(store: &Store, value: &ConnectionProfiles) -> Result<(), &'static str> {
    let encoded =
        serde_json::to_string(value).map_err(|_| "Could not save connection settings.")?;
    store
        .set_internal_values_checked(&[
            (CONNECTION_PROFILES_KEY, &encoded),
            (
                ACTIVE_CONNECTION_KEY,
                value.active_id.as_deref().unwrap_or(""),
            ),
        ])
        .map_err(|_| "Could not save connection settings.")
}

fn suspend_for_connection_change(store: &Store, worker: &WorkerHandle) -> Result<(), &'static str> {
    worker.suspend_system_one();
    store
        .cancel_unfinished_burn_check_assessments_for_connection_change()
        .map_err(|_| {
            "Could not cancel unfinished Burn Checks. Checks are paused. Retry the connection change."
        })
}

fn pause_model_checks(store: &Store, worker: &WorkerHandle) -> Result<(), &'static str> {
    store
        .set_internal_value_checked(YAML_DISCOVERY_ENABLED_KEY, "false")
        .map_err(|_| "Could not pause YAML Burn Checks.")?;
    suspend_for_connection_change(store, worker)
}

fn enable_model_checks(
    store: &Store,
    worker: &WorkerHandle,
    connection: SystemOneConnection,
    credential: Option<String>,
) -> Result<(), &'static str> {
    if !yaml_discovery_enabled(store) {
        suspend_for_connection_change(store, worker)?;
    }
    store
        .set_internal_values_checked(&[
            (YAML_DISCOVERY_ENABLED_KEY, "true"),
            (AUTH_REJECTED_KEY, "false"),
        ])
        .map_err(|_| "Could not enable YAML Burn Checks.")?;
    if worker
        .set_system_one_connection(connection, credential)
        .is_err()
    {
        worker.suspend_system_one();
        return Err("Saved Smart Burn Checks provider settings are invalid.");
    }
    Ok(())
}

fn next_connection_revision(revision: u64) -> Result<u64, &'static str> {
    revision
        .checked_add(1)
        .ok_or("The connection revision limit was reached. Save a new connection.")
}

fn migrate_automatic_limits(store: &Store, worker: &WorkerHandle) -> Result<(), &'static str> {
    let first_migration = store
        .internal_value(AUTOMATIC_LIMITS_MIGRATION_KEY)
        .as_deref()
        != Some("1");
    let mut saved = profiles(store)?;
    let mut changed = false;
    let mut active_changed = false;
    for (id, connection) in &mut saved.profiles {
        let old_flash_profile = first_migration
            && connection.model == "clef-flash"
            && (connection.provider == SystemOneProvider::Cloudflare
                || (connection.provider == SystemOneProvider::Custom
                    && connection.response_mode
                        == crate::jev::config::SystemOneResponseMode::CloudflareEnvelope
                    && connection
                        .capabilities()
                        .is_ok_and(|capabilities| capabilities.usable_input_tokens().is_some())));
        if connection.context_override.is_some() || old_flash_profile {
            connection.revision = next_connection_revision(connection.revision)?;
            connection.context_override = None;
            changed = true;
            active_changed |= Some(id.as_str()) == saved.active_id.as_deref();
        }
    }
    if !changed && !first_migration {
        return Ok(());
    }
    if active_changed {
        suspend_for_connection_change(store, worker)?;
    }
    let encoded = serde_json::to_string(&saved)
        .map_err(|_| "Could not save automatic model limits. Retry in Settings.")?;
    store
        .set_internal_values_checked(&[
            (CONNECTION_PROFILES_KEY, &encoded),
            (
                ACTIVE_CONNECTION_KEY,
                saved.active_id.as_deref().unwrap_or(""),
            ),
            (AUTOMATIC_LIMITS_MIGRATION_KEY, "1"),
        ])
        .map_err(|_| "Could not save automatic model limits. Retry in Settings.")
}

fn advance_legacy_connection_revisions(saved: &mut ConnectionProfiles) -> Result<(), &'static str> {
    for connection in saved.profiles.values_mut() {
        if connection.credential.as_ref() == Some(&CredentialReference::LegacyTypeSafe) {
            connection.revision = next_connection_revision(connection.revision)?;
        }
    }
    Ok(())
}

fn connection_requires_credential(connection: &SystemOneConnection) -> bool {
    matches!(
        connection.provider,
        SystemOneProvider::Jev | SystemOneProvider::Cloudflare
    )
}

/// Copy legacy TypeSafe state only after its existing secure-storage reference is known.
/// The migration marker is written last, so a failed write can be retried safely.
fn migrate_provider_state(store: &Store) -> Result<(), &'static str> {
    if store.internal_value(PROVIDER_MIGRATION_KEY).as_deref() == Some("1") {
        return Ok(());
    }
    let existing = store.internal_value(CONNECTION_PROFILES_KEY);
    if existing.is_none() && store.internal_value(ACTIVE_CONNECTION_KEY).is_some() {
        return Err("Saved Smart Burn Checks provider settings are missing.");
    }
    let mut profiles: ConnectionProfiles = match existing.as_deref() {
        Some(json) => serde_json::from_str(json)
            .map_err(|_| "Saved Smart Burn Checks provider settings are invalid.")?,
        None => ConnectionProfiles::default(),
    };
    let legacy_configured = saved_key_marker(store)
        || store.internal_value(AUTH_REJECTED_KEY).as_deref() == Some("true");
    if existing.is_none() && legacy_configured {
        profiles
            .profiles
            .insert("jev".to_owned(), SystemOneConnection::jev_default());
        profiles.active_id = Some("jev".to_owned());
    }
    if profiles
        .active_id
        .as_ref()
        .is_some_and(|id| !profiles.profiles.contains_key(id))
    {
        return Err("The active Smart Burn Checks connection is missing.");
    }
    let encoded = serde_json::to_string(&profiles)
        .map_err(|_| "Could not migrate Smart Burn Checks provider settings.")?;
    let mut updates = vec![
        (CONNECTION_PROFILES_KEY, encoded.as_str()),
        (
            ACTIVE_CONNECTION_KEY,
            profiles.active_id.as_deref().unwrap_or(""),
        ),
        (PROVIDER_MIGRATION_KEY, "1"),
    ];
    if existing.is_none() && !legacy_configured {
        updates.push((YAML_DISCOVERY_ENABLED_KEY, "false"));
        updates.push((AUTOMATIC_LIMITS_MIGRATION_KEY, "1"));
    }
    store
        .set_internal_values_checked(&updates)
        .map_err(|_| "Could not migrate Smart Burn Checks provider settings.")
}

fn active_connection(store: &Store) -> Result<SystemOneConnection, &'static str> {
    let profiles = profiles(store)?;
    let connection = profiles
        .profiles
        .get(
            profiles
                .active_id
                .as_deref()
                .ok_or("Select a connection in Settings → Checks.")?,
        )
        .cloned()
        .ok_or("The active Smart Burn Checks connection is missing.")?;
    connection
        .validate()
        .map_err(|_| "Saved Smart Burn Checks provider settings are invalid.")?;
    Ok(connection)
}

#[cfg(test)]
fn restore_saved_key_with_vault(
    store: &Store,
    worker: &WorkerHandle,
    read: impl FnOnce() -> Result<Option<String>, &'static str>,
    vault: &impl CredentialVault,
) -> Result<(), &'static str> {
    restore_saved_key_with_origin(store, worker, read, vault).map_err(|failure| failure.message())
}

fn restore_saved_key_with_origin(
    store: &Store,
    worker: &WorkerHandle,
    read: impl FnOnce() -> Result<Option<String>, &'static str>,
    vault: &impl CredentialVault,
) -> Result<(), StartupRestoreFailure> {
    worker.suspend_system_one();
    operation::metadata(store)?;
    ensure_no_pending_removal(store)?;
    if store
        .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
        .as_deref()
        == Some("true")
    {
        return Err(StartupRestoreFailure::LegacyCredentialChange(
            "TypeSafe credential removal is incomplete. Retry it in Settings → Checks.",
        ));
    }
    let recovered_connection = match lifecycle::recover_connection_change(store, worker, vault) {
        Ok(connection) => {
            migrate_automatic_limits(store, worker)?;
            connection
        }
        Err(error) => {
            if lifecycle::pending_affects_active(store) {
                return Err(error.into());
            }
            None
        }
    };
    let saved = profiles(store)?;
    let Some(id) = &saved.active_id else {
        return Ok(());
    };
    let connection = saved
        .profiles
        .get(id)
        .cloned()
        .ok_or("The active Smart Burn Checks connection is missing.")?;
    if recovered_connection.as_ref() == Some(&connection) {
        return Ok(());
    }
    restore_selected_connection(store, worker, connection, read, vault)
}

fn restore_selected_connection(
    store: &Store,
    worker: &WorkerHandle,
    connection: SystemOneConnection,
    read: impl FnOnce() -> Result<Option<String>, &'static str>,
    vault: &impl CredentialVault,
) -> Result<(), StartupRestoreFailure> {
    connection.validate().map_err(|_| {
        StartupRestoreFailure::SelectedConnection(
            "Saved Smart Burn Checks provider settings are invalid.",
        )
    })?;
    let checks_enabled = yaml_discovery_enabled(store);
    if !checks_enabled {
        return worker
            .install_system_one_connection(connection, None, false)
            .map_err(|_| {
                StartupRestoreFailure::SelectedConnection(
                    "Saved Smart Burn Checks provider settings are invalid.",
                )
            });
    }
    let authentication_rejected =
        store.internal_value(AUTH_REJECTED_KEY).as_deref() == Some("true");
    if authentication_rejected {
        return worker
            .install_system_one_connection(connection, None, false)
            .map_err(|_| {
                StartupRestoreFailure::SelectedConnection(
                    "Saved Smart Burn Checks provider settings are invalid.",
                )
            });
    }
    if matches!(
        connection.credential,
        Some(CredentialReference::LegacyTypeSafe)
    ) {
        let key = if saved_key_marker(store) {
            read().map_err(StartupRestoreFailure::SelectedLegacyCredential)?
        } else {
            None
        };
        if saved_key_marker(store) && key.is_none() {
            return Err(StartupRestoreFailure::SelectedLegacyCredential(
                "The saved TypeSafe API key is missing. Replace it in Settings → Checks.",
            ));
        }
        worker
            .install_system_one_connection(connection, key, true)
            .map_err(|_| {
                StartupRestoreFailure::SelectedConnection(
                    "Saved Smart Burn Checks provider settings are invalid.",
                )
            })?;
    } else {
        let credential = resolve_credential(&connection, None, vault)
            .map_err(StartupRestoreFailure::SelectedConnection)?;
        if connection_requires_credential(&connection) && credential.is_none() {
            return Err(StartupRestoreFailure::SelectedConnection(
                "The active model provider credential is missing. Update it in Settings → Checks.",
            ));
        }
        worker
            .set_system_one_connection(connection, credential)
            .map_err(|_| {
                StartupRestoreFailure::SelectedConnection(
                    "Saved Smart Burn Checks provider settings are invalid.",
                )
            })?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn get_system_one_settings(
    window: WebviewWindow,
    store: tauri::State<'_, Store>,
) -> Result<SystemOneSettings, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
    operation::settings_snapshot(&store).map_err(str::to_owned)
}

fn ollama_model_lookup_client(
    mut draft: SystemOneDraft,
    vault: &impl CredentialVault,
) -> Result<crate::jev_ollama::OllamaClient, &'static str> {
    draft.connection.revision = 1;
    draft
        .connection
        .validate_ollama_model_lookup()
        .map_err(|_| "Enter a valid Ollama base URL to find installed models.")?;
    let credential = resolve_validated_credential(&draft.connection, draft.credential, vault)?;
    let SystemOneEndpoint::BaseUrl(base) = &draft.connection.endpoint else {
        return Err("Enter a valid Ollama base URL to find installed models.");
    };
    crate::jev_ollama::OllamaClient::new(base, credential)
        .map_err(|_| "Enter a valid Ollama base URL to find installed models.")
}

#[tauri::command]
pub(crate) async fn list_ollama_models(
    window: WebviewWindow,
    draft: SystemOneDraft,
) -> Result<Vec<crate::jev_ollama::InstalledOllamaModel>, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let client = {
        let _change = CREDENTIAL_CHANGE.lock().await;
        ensure_no_pending_credential_change(&window.state::<Store>()).map_err(str::to_owned)?;
        ollama_model_lookup_client(draft, &SystemCredentialVault).map_err(str::to_owned)?
    };
    client.list_installed_models().await.map_err(|error| {
        match error {
            crate::jev_ollama::OllamaError::AuthenticationRejected => "Ollama rejected the credential. Update it and find models again.",
            crate::jev_ollama::OllamaError::ResponseTooLarge => "Ollama returned too much model metadata. Reduce the model list and retry.",
            _ => "Could not read installed Ollama models. Check the server address and response, then retry.",
        }.to_owned()
    })
}

#[tauri::command]
pub(crate) async fn test_system_one_connection(
    window: WebviewWindow,
    mut draft: SystemOneDraft,
) -> Result<(), String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    draft.connection.revision = 1;
    let provider = draft.connection.provider;
    let result = async {
        {
            let _change = CREDENTIAL_CHANGE.lock().await;
            ensure_no_pending_credential_change(&window.state::<Store>()).map_err(str::to_owned)?;
            draft.credential =
                resolve_credential(&draft.connection, draft.credential, &SystemCredentialVault)
                    .map_err(str::to_owned)?;
        }
        test_connection(draft).await
    }
    .await;
    crate::analytics::record_model_check_lifecycle(
        window.app_handle(),
        crate::analytics::event::ModelCheckLifecycle::ProviderTest {
            provider: provider.into(),
            outcome: if result.is_ok() {
                crate::analytics::event::ProviderTestOutcome::Succeeded
            } else {
                crate::analytics::event::ProviderTestOutcome::Failed
            },
        },
    );
    result
}

async fn test_connection(draft: SystemOneDraft) -> Result<(), String> {
    let request = antiburn_local::analysis::jev::JevRequest {
        model: draft.connection.model.clone(),
        state: serde_json::json!({"text":"synthetic connection validation input"}),
        questions: std::collections::BTreeMap::from([(
            "validation".to_owned(),
            antiburn_local::analysis::jev::JevQuestion::Noul {
                instructions: serde_json::json!("Is this synthetic input valid?"),
                criteria: None,
            },
        )]),
    };
    let limits = crate::jev::config::resolve_execution_capabilities(
        &draft.connection,
        draft.credential.as_deref(),
    )
    .await
    .map_err(capability_error)?;
    let response = match draft.connection.provider {
        SystemOneProvider::Jev => {
            let key = draft
                .credential
                .ok_or_else(|| "Enter a TypeSafe API key.".to_owned())?;
            let client = crate::jev::client::TypeSafeClient::new(key)
                .map_err(|_| "Enter a valid TypeSafe API key.".to_owned())?;
            let endpoint = draft
                .connection
                .inference_endpoint()
                .map_err(|_| "Connection settings are invalid.".to_owned())?;
            client
                .evaluate_at_with_capabilities_and_usage(&request, &endpoint, &limits, |_| Ok(()))
                .await
                .map_err(|_| "TypeSafe rejected the test request. Check the API key.".to_owned())
        }
        SystemOneProvider::Ollama => {
            let SystemOneEndpoint::BaseUrl(base) = &draft.connection.endpoint else {
                return Err("Connection settings are invalid.".to_owned());
            };
            let client = crate::jev_ollama::OllamaClient::new(base, draft.credential)
                .map_err(|error| error.to_string())?;
            client
                .evaluate(&request, &limits)
                .await
                .map_err(|error| match error {
                    crate::jev_ollama::OllamaError::OldVersion
                    | crate::jev_ollama::OllamaError::MissingModel
                    | crate::jev_ollama::OllamaError::UnsupportedRunner
                    | crate::jev_ollama::OllamaError::ContextRejected
                    | crate::jev_ollama::OllamaError::RequestBodyTooLarge
                    | crate::jev_ollama::OllamaError::AuthenticationRejected
                    | crate::jev_ollama::OllamaError::InvalidRequest
                    | crate::jev_ollama::OllamaError::ResponseDecode
                    | crate::jev_ollama::OllamaError::ResponseTooLarge => error.to_string(),
                    crate::jev_ollama::OllamaError::InvalidBaseUrl
                    | crate::jev_ollama::OllamaError::ProviderUnavailable
                    | crate::jev_ollama::OllamaError::RequestOutcomeUnknown => {
                        "Could not reach Ollama. Check the server address and try again.".into()
                    }
                })
        }
        SystemOneProvider::Cloudflare => {
            let SystemOneEndpoint::CloudflareAccount(account_id) = &draft.connection.endpoint
            else {
                return Err("Connection settings are invalid.".to_owned());
            };
            let token = draft
                .credential
                .ok_or_else(|| "Enter a Cloudflare API token.".to_owned())?;
            crate::jev_cloudflare::CloudflareClient::new(
                account_id.clone(),
                token,
                draft.connection.model,
            )
            .map_err(|_| "Enter a valid Cloudflare connection.".to_owned())?
            .evaluate_with_capabilities_and_usage(&request, &limits, |_| Ok(()))
            .await
            .map_err(|_| {
                "Cloudflare rejected the test request. Check the account, model, and token."
                    .to_owned()
            })
        }
        SystemOneProvider::Custom => {
            let endpoint = draft
                .connection
                .inference_endpoint()
                .map_err(|_| "Connection settings are invalid.".to_owned())?;
            crate::jev::client::evaluate_custom(
                &endpoint,
                draft.credential.as_deref(),
                draft.connection.response_mode,
                &request,
                &limits,
            )
            .await
            .map_err(|_| {
                "The custom endpoint rejected the test request. Check the URL and response mode."
                    .to_owned()
            })
        }
    };
    response.map(|_| ())
}

#[tauri::command]
pub(crate) async fn save_system_one_connection(
    app: AppHandle,
    window: WebviewWindow,
    draft: SystemOneDraft,
    intent: SaveConnectionIntent,
    request_id: String,
) -> Result<SystemOneSettings, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
    let result = lifecycle::save_connection_recording(
        &app.state::<Store>(),
        &app.state::<WorkerHandle>(),
        draft,
        intent,
        &request_id,
        &SystemCredentialVault,
        |event| crate::analytics::record_model_check_lifecycle(&app, event),
    );
    changed(&app);
    result.map_err(str::to_owned)
}

#[tauri::command]
pub(crate) async fn switch_system_one_connection(
    app: AppHandle,
    window: WebviewWindow,
    connection_id: String,
) -> Result<SystemOneSettings, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
    let result = lifecycle::switch_connection_recording(
        &app.state::<Store>(),
        &app.state::<WorkerHandle>(),
        &connection_id,
        &SystemCredentialVault,
        |event| crate::analytics::record_model_check_lifecycle(&app, event),
    );
    changed(&app);
    result.map_err(str::to_owned)
}

#[tauri::command]
pub(crate) async fn remove_system_one_connection(
    app: AppHandle,
    window: WebviewWindow,
    connection_id: String,
    revision: u64,
) -> Result<SystemOneSettings, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
    let result = lifecycle::remove_connection_recording(
        &app.state::<Store>(),
        &app.state::<WorkerHandle>(),
        &connection_id,
        revision,
        &SystemCredentialVault,
        |event| crate::analytics::record_model_check_lifecycle(&app, event),
    );
    changed(&app);
    result.map_err(str::to_owned)
}

#[tauri::command]
pub(crate) async fn retry_system_one_connection_operation(
    app: AppHandle,
    window: WebviewWindow,
    operation_id: String,
) -> Result<operation::OperationResult, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
    let result = operation::retry_recording(
        &app.state::<Store>(),
        &app.state::<WorkerHandle>(),
        &operation_id,
        &SystemCredentialVault,
        |event| crate::analytics::record_model_check_lifecycle(&app, event),
    );
    changed(&app);
    result.map_err(str::to_owned)
}

#[cfg(test)]
fn switch_connection(
    store: &Store,
    worker: &WorkerHandle,
    connection_id: &str,
    vault: &impl CredentialVault,
) -> Result<(SystemOneSettings, bool), &'static str> {
    lifecycle::switch_connection(store, worker, connection_id, vault)
}

#[tauri::command]
pub(crate) async fn refresh_system_one_limits(
    window: WebviewWindow,
    mut connection: SystemOneConnection,
    credential: Option<String>,
) -> Result<antiburn_local::analysis::jev::capabilities::ModelCapabilities, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    connection.revision = 1;
    let credential = {
        let _change = CREDENTIAL_CHANGE.lock().await;
        ensure_no_pending_credential_change(&window.state::<Store>()).map_err(str::to_owned)?;
        resolve_credential(&connection, credential, &SystemCredentialVault)
            .map_err(str::to_owned)?
    };
    refresh_limits(connection, credential).await
}

async fn refresh_limits(
    connection: SystemOneConnection,
    credential: Option<String>,
) -> Result<antiburn_local::analysis::jev::capabilities::ModelCapabilities, String> {
    crate::jev::config::resolve_discovery_capabilities(&connection, credential.as_deref())
        .await
        .map_err(capability_error)
}

fn capability_error(error: antiburn_local::analysis::jev::JevError) -> String {
    use antiburn_local::analysis::jev::JevError;
    match error {
        JevError::AuthenticationRejected => {
            "The selected provider rejected its credential. Update it in Settings → Checks."
        }
        JevError::UnsupportedModel => "This connection has no supported model profile or runner. Choose a supported provider and model.",
        JevError::ContextRejected => "The selected provider did not report usable model limits.",
        JevError::InvalidRequestSchema => "Connection settings are invalid.",
        _ => "Could not read model limits from the selected provider. Check the connection and try again.",
    }
    .to_owned()
}

#[tauri::command]
pub(crate) async fn remove_system_one_credential(
    app: AppHandle,
    window: WebviewWindow,
    connection_id: String,
) -> Result<(), String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
    let store = app.state::<Store>();
    let active_impacted = profiles(&store)
        .map_err(str::to_owned)?
        .active_id
        .as_deref()
        == Some(&connection_id);
    let result = remove_connection_credential(
        &store,
        &app.state::<WorkerHandle>(),
        &connection_id,
        &SystemCredentialVault,
    );
    if result.is_ok() && active_impacted {
        STARTUP_ERRORS.set_unbound(None);
    }
    if let Ok(Some(provider)) = &result {
        crate::analytics::record_model_check_lifecycle(
            &app,
            crate::analytics::event::ModelCheckLifecycle::ProviderSetup {
                provider: (*provider).into(),
                outcome: crate::analytics::event::ProviderSetupOutcome::CredentialRemoved,
            },
        );
    }
    changed(&app);
    result.map(|_| ()).map_err(str::to_owned)
}

fn remove_connection_credential(
    store: &Store,
    worker: &WorkerHandle,
    id: &str,
    vault: &impl CredentialVault,
) -> Result<Option<SystemOneProvider>, &'static str> {
    lifecycle::ensure_no_pending_change(store)?;
    if legacy_removal_pending(store)
        || store
            .internal_value(CONNECTION_PENDING_KEY)
            .is_some_and(|pending| !pending.is_empty())
    {
        return Err(
            "A connection update is incomplete. Retry it in Settings → Checks before removing a credential.",
        );
    }
    if let Some(pending) = store.internal_value(CONNECTION_REMOVAL_KEY)
        && !pending.is_empty()
        && pending != id
    {
        return Err("Connection credential removal is incomplete. Retry it in Settings → Checks.");
    }
    let mut saved = profiles(store)?;
    let profile = saved
        .profiles
        .get_mut(id)
        .ok_or("The saved connection is missing.")?;
    let reference = profile.credential.clone();
    let provider = profile.provider;
    let Some(CredentialReference::Connection(key_id)) = reference else {
        if profile.credential.is_some() {
            return Err("Remove the TypeSafe API key in Settings → Checks.");
        }
        return Ok(None);
    };
    profile.credential = None;
    profile.revision = next_connection_revision(profile.revision)?;
    let shared = saved.profiles.values().any(|profile| {
        profile.credential.as_ref() == Some(&CredentialReference::Connection(key_id.clone()))
    });
    let previous = if shared {
        None
    } else {
        read_credential(vault, Some(&key_id), CredentialReadStage::CredentialRemoval)?
            .filter(|secret| !secret.trim().is_empty())
    };
    store
        .set_internal_value_checked(CONNECTION_REMOVAL_KEY, id)
        .map_err(|_| "Could not protect the credential removal state.")?;
    if saved.active_id.as_deref() == Some(id) {
        pause_model_checks(store, worker)?;
    }
    if !shared {
        delete_credential(vault, Some(&key_id))?;
    }
    let encoded =
        serde_json::to_string(&saved).map_err(|_| "Could not save connection settings.")?;
    let mut updates = vec![
        (CONNECTION_PROFILES_KEY, encoded.as_str()),
        (
            ACTIVE_CONNECTION_KEY,
            saved.active_id.as_deref().unwrap_or(""),
        ),
        (CONNECTION_REMOVAL_KEY, ""),
    ];
    if saved.active_id.as_deref() == Some(id) {
        updates.push((AUTH_REJECTED_KEY, "false"));
    }
    if store.set_internal_values_checked(&updates).is_err() {
        if let Some(secret) = previous
            && write_credential(vault, Some(&key_id), &secret).is_err()
        {
            return Err(
                "Could not restore the connection credential. Retry credential removal in Settings → Checks.",
            );
        }
        return Err("Could not save the credential removal state.");
    }
    Ok(Some(provider))
}

/// Restore only a previously enabled key. Serialize startup with Settings changes.
pub(crate) fn restore_at_launch(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _change = CREDENTIAL_CHANGE.lock().await;
        let store = app.state::<Store>().inner().clone();
        if let Err(error) = migrate_provider_state(&store) {
            STARTUP_ERRORS.set_unbound(Some(error));
            crate::installed_checks::credentials_restored(&app);
            changed(&app);
            return;
        }
        if let Err(error) = profiles(&store) {
            STARTUP_ERRORS.set_unbound(Some(error));
            crate::installed_checks::credentials_restored(&app);
            changed(&app);
            return;
        }
        let worker_app = app.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let worker = worker_app.state::<WorkerHandle>();
            restore_saved_key_with_origin(&store, &worker, read_key, &SystemCredentialVault)
        })
        .await;
        let result = match result {
            Ok(result) => result,
            Err(_) => Err(StartupRestoreFailure::Unbound(
                "Credential storage is unavailable.",
            )),
        };
        STARTUP_ERRORS.record_restore_result(&app.state::<Store>(), result);
        crate::installed_checks::credentials_restored(&app);
        changed(&app);
    });
}

fn checks_settings_window(window: &WebviewWindow) -> Result<(), &'static str> {
    checks_settings_label(window.label())
}

fn checks_settings_label(label: &str) -> Result<(), &'static str> {
    if matches!(label, crate::settings::LABEL | crate::main_window::LABEL) {
        Ok(())
    } else {
        Err("This action is available only in the main window or Settings.")
    }
}

pub(crate) fn read_check_availability(
    store: &Store,
    configured: bool,
    saved_key: bool,
    error: Option<&'static str>,
) -> Result<CheckAvailability, &'static str> {
    let (enabled_checks, revision) = store
        .check_preferences_snapshot()
        .map_err(|_| "Could not read check preferences from the local database.")?;
    Ok(CheckAvailability {
        yaml_enabled: yaml_discovery_enabled(store),
        configured,
        saved_key,
        error,
        usage: store
            .burn_check_usage_summary()
            .map_err(|_| "Could not read check usage from the local database.")?,
        checks: DetectorId::ALL
            .into_iter()
            .map(|detector| CheckChoice {
                id: detector.into(),
                enabled: enabled_checks.contains(&detector),
            })
            .collect(),
        installed_checks: Vec::new(),
        revision,
    })
}

fn availability(app: &AppHandle) -> Result<CheckAvailability, &'static str> {
    let store = app.state::<Store>();
    let removal_pending = store
        .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
        .as_deref()
        == Some("true");
    let authentication_rejected = app.state::<WorkerHandle>().authentication_rejected()
        || store.internal_value(AUTH_REJECTED_KEY).as_deref() == Some("true");
    let error = if lifecycle::pending_affects_active(&store) {
        Some("A connection operation is incomplete. Retry it in Settings → Checks.")
    } else if removal_pending {
        Some("TypeSafe credential removal is incomplete. Retry it in Settings → Checks.")
    } else if store
        .internal_value(CONNECTION_REMOVAL_KEY)
        .is_some_and(|id| !id.is_empty())
    {
        Some("Connection credential removal is incomplete. Retry it in Settings → Checks.")
    } else if store
        .internal_value(CONNECTION_PENDING_KEY)
        .is_some_and(|id| !id.is_empty())
    {
        Some("Smart Burn Checks connection update is incomplete. Retry it in Settings → Checks.")
    } else if authentication_rejected {
        Some("The selected provider rejected its credential. Update it in Settings → Checks.")
    } else {
        STARTUP_ERRORS.message()
    };
    read_check_availability(
        &store,
        app.state::<WorkerHandle>().is_available(),
        saved_key_marker(&store),
        error,
    )
    .map(|availability| availability.with_installed_checks(&crate::installed_checks::metadata(app)))
}

pub(crate) fn changed(app: &AppHandle) {
    crate::installed_checks::wake(app);
    let event = match availability(app) {
        Ok(snapshot) => CheckAvailabilityEvent::Updated(Box::new(snapshot)),
        Err(_) => CheckAvailabilityEvent::Failed,
    };
    let _ = app.emit(AVAILABILITY_EVENT, event);
    let _ = app.emit(crate::commands::CHECKS_REPORT_CHANGED_EVENT, ());
}

pub(crate) fn progress_changed(app: &AppHandle) {
    let event = match availability(app) {
        Ok(snapshot) => CheckAvailabilityEvent::Updated(Box::new(snapshot)),
        Err(_) => CheckAvailabilityEvent::Failed,
    };
    let _ = app.emit(AVAILABILITY_EVENT, event);
}

#[tauri::command]
pub(crate) async fn set_smart_burn_checks_enabled(
    app: AppHandle,
    window: WebviewWindow,
    enabled: bool,
) -> Result<CheckAvailability, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
    let store = app.state::<Store>().inner().clone();
    let was_enabled = yaml_discovery_enabled(&store);
    if !enabled {
        if let Err(error) = pause_model_checks(&store, &app.state::<WorkerHandle>()) {
            changed(&app);
            return Err(error.to_owned());
        }
        if let Err(error) = crate::installed_checks::settle_disable(&app).await {
            changed(&app);
            return Err(error.to_string());
        }
    } else {
        ensure_no_pending_credential_change(&store).map_err(str::to_owned)?;
        migrate_automatic_limits(&store, &app.state::<WorkerHandle>()).map_err(str::to_owned)?;
        let connection = active_connection(&store).map_err(str::to_owned)?;
        ensure_no_pending_removal(&store).map_err(str::to_owned)?;
        if store
            .internal_value(CONNECTION_PENDING_KEY)
            .is_some_and(|id| !id.is_empty())
        {
            return Err(
                "Smart Burn Checks connection update is incomplete. Retry it in Settings → Checks."
                    .to_owned(),
            );
        }
        if store
            .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
            .as_deref()
            == Some("true")
        {
            return Err(
                "TypeSafe credential removal is incomplete. Retry it in Settings → Checks."
                    .to_owned(),
            );
        }
        if store.internal_value(AUTH_REJECTED_KEY).as_deref() == Some("true") {
            return Err(
                "The selected provider rejected its credential. Update it in Settings → Checks."
                    .to_owned(),
            );
        }
        let connection_for_worker = connection.clone();
        let credential = tauri::async_runtime::spawn_blocking(move || {
            read_connection_credential(&connection_for_worker)
        })
        .await
        .map_err(|_| "Credential storage is unavailable.".to_owned())?
        .map_err(str::to_owned)?;
        if connection_requires_credential(&connection) && credential.is_none() {
            return Err("The selected provider needs a valid credential.".to_owned());
        }
        enable_model_checks(&store, &app.state::<WorkerHandle>(), connection, credential)
            .map_err(str::to_owned)?;
    }
    if was_enabled != enabled {
        crate::analytics::record_model_check_lifecycle(
            &app,
            crate::analytics::event::ModelCheckLifecycle::Enablement { enabled },
        );
    }
    changed(&app);
    availability(&app).map_err(str::to_owned)
}

#[tauri::command]
pub(crate) fn get_check_availability(app: AppHandle) -> Result<CheckAvailability, String> {
    availability(&app).map_err(str::to_owned)
}

#[tauri::command]
pub(crate) fn set_check_enabled(
    app: AppHandle,
    window: WebviewWindow,
    detector: BurnCheckDetectorId,
    enabled: bool,
) -> Result<CheckAvailability, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CHECK_CHANGE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let detector = DetectorId::from(detector);
    let store = app.state::<Store>();
    if store
        .check_enabled(detector)
        .map_err(|_| "Could not read the check preference.".to_owned())?
        == enabled
    {
        return availability(&app).map_err(str::to_owned);
    }
    let changed_preference = store
        .set_check_enabled(detector, enabled)
        .map_err(|_| "Could not save the check preference.".to_owned())?;
    if !changed_preference {
        return availability(&app).map_err(str::to_owned);
    }
    app.state::<crate::insights_ipc::InsightsController>()
        .cancel();
    crate::session_lifecycle::report(
        &app,
        crate::session_lifecycle::SyncObservation::IndexChanged {
            reason: crate::session_lifecycle::IndexChangeReason::Invalidated,
        },
    );
    changed(&app);
    crate::analytics::record_check_enablement_saved(&app, detector, enabled);
    availability(&app).map_err(str::to_owned)
}

fn resume_after_key_save(was_enabled: bool, replacing: bool, was_configured: bool) -> bool {
    was_enabled || (replacing && !was_configured)
}

fn save_legacy_key<V: CredentialVault>(
    store: &Store,
    worker: &WorkerHandle,
    key: Option<String>,
    vault: &V,
) -> Result<(), &'static str> {
    if key.is_some()
        && store.internal_value(CONNECTION_PROFILES_KEY).is_none()
        && store.internal_value(PROVIDER_MIGRATION_KEY).is_none()
    {
        let saved = ConnectionProfiles {
            active_id: Some("jev".to_owned()),
            profiles: [("jev".to_owned(), SystemOneConnection::jev_default())].into(),
            retired_ids: Default::default(),
        };
        save_profiles(store, &saved)?;
    }
    migrate_automatic_limits(store, worker)?;
    let replacing = key.is_some();
    let resume = resume_after_key_save(
        yaml_discovery_enabled(store),
        replacing,
        saved_key_marker(store),
    );
    let previous_snapshot = read_legacy_snapshot(vault, CredentialReadStage::LegacyUpdate)?;
    let previous = match vault.read(None) {
        Ok(value) => value,
        Err(error) if V::is_missing(&error) || (replacing && V::is_bad_encoding(&error)) => None,
        Err(error) => return Err(V::read_failure(&error).report(CredentialReadStage::LegacyUpdate)),
    }
    .filter(|secret| !secret.trim().is_empty());
    let key = key
        .or_else(|| previous.clone())
        .ok_or("The saved TypeSafe API key is missing. Enter a new key.")?;
    let mut saved = profiles(store)?;
    let credential_changed = previous.as_ref() != Some(&key);
    if credential_changed || legacy_removal_pending(store) {
        advance_legacy_connection_revisions(&mut saved)?;
    }
    let connection = saved
        .profiles
        .get(
            saved
                .active_id
                .as_deref()
                .ok_or("Select a connection in Settings → Checks.")?,
        )
        .cloned()
        .ok_or("The active Smart Burn Checks connection is missing.")?;
    connection
        .validate()
        .map_err(|_| "Saved Smart Burn Checks provider settings are invalid.")?;
    let credential = if connection.credential.as_ref() == Some(&CredentialReference::LegacyTypeSafe)
    {
        Some(key.clone())
    } else {
        resolve_credential(&connection, None, vault)?
    };
    let encoded =
        serde_json::to_string(&saved).map_err(|_| "Could not save the credential update state.")?;
    store
        .set_internal_value_checked(CREDENTIAL_CHANGE_PENDING_KEY, "true")
        .map_err(|_| "Could not protect the credential update state.")?;
    suspend_for_connection_change(store, worker)?;
    if credential_changed && let Err(error) = write_credential(vault, None, &key) {
        restore_legacy_key(vault, previous_snapshot.as_deref())?;
        return Err(error);
    }
    if store
        .set_internal_values_checked(&[
            (CONNECTION_PROFILES_KEY, &encoded),
            (SAVED_KEY_KEY, "true"),
            (
                YAML_DISCOVERY_ENABLED_KEY,
                if resume { "true" } else { "false" },
            ),
            (CREDENTIAL_CHANGE_PENDING_KEY, "false"),
            (LEGACY_REMOVAL_PENDING_KEY, "false"),
            (AUTH_REJECTED_KEY, "false"),
        ])
        .is_err()
    {
        if credential_changed {
            restore_legacy_key(vault, previous_snapshot.as_deref())?;
        }
        return Err("Could not save the credential update state.");
    }
    worker
        .install_system_one_connection(connection, credential.filter(|_| resume), resume)
        .map_err(|_| "Saved Smart Burn Checks provider settings are invalid.")
}

fn restore_legacy_key(
    vault: &impl CredentialVault,
    previous: Option<&[u8]>,
) -> Result<(), &'static str> {
    match previous {
        Some(secret) => vault.restore_legacy_snapshot(secret),
        None => delete_credential(vault, None),
    }
    .map_err(|_| "Could not restore the API key. Checks are paused. Retry the key update.")
}

#[tauri::command]
pub(crate) async fn set_typesafe_api_key(
    app: AppHandle,
    window: WebviewWindow,
    key: Option<String>,
) -> Result<CheckAvailability, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
    lifecycle::ensure_no_pending_change(&app.state::<Store>()).map_err(str::to_owned)?;
    ensure_no_pending_removal(&app.state::<Store>()).map_err(str::to_owned)?;
    if app
        .state::<Store>()
        .internal_value(CONNECTION_PENDING_KEY)
        .is_some_and(|id| !id.is_empty())
    {
        return Err(
            "Smart Burn Checks connection update is incomplete. Retry it in Settings → Checks."
                .to_owned(),
        );
    }
    if key.is_none() && legacy_removal_pending(&app.state::<Store>()) {
        return Err(
            "TypeSafe credential removal is incomplete. Retry it in Settings → Checks.".to_owned(),
        );
    }
    if key.is_none() && !saved_key_marker(&app.state::<Store>()) {
        return Err("Enter a TypeSafe API key first.".to_owned());
    }
    if key.is_none()
        && app
            .state::<Store>()
            .internal_value(AUTH_REJECTED_KEY)
            .as_deref()
            == Some("true")
    {
        return Err("TypeSafe rejected this API key. Enter a replacement key.".to_owned());
    }
    let key = key.map(|key| key.trim().to_owned());
    #[cfg(feature = "analytics")]
    let first_enablement = key.is_some() && !saved_key_marker(&app.state::<Store>());
    if key
        .as_ref()
        .is_some_and(|key| key.is_empty() || key.len() > 4096 || key.chars().any(char::is_control))
    {
        return Err("Enter a valid TypeSafe API key.".to_owned());
    }
    let store = app.state::<Store>().inner().clone();
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        save_legacy_key(
            &store,
            &worker_app.state::<WorkerHandle>(),
            key,
            &SystemCredentialVault,
        )
    })
    .await
    .map_err(|_| "Credential storage is unavailable.".to_owned())?
    .map_err(str::to_owned);
    if let Err(error) = result {
        changed(&app);
        return Err(error);
    }
    #[cfg(feature = "analytics")]
    if first_enablement {
        crate::analytics::record_model_check_lifecycle(
            &app,
            crate::analytics::event::ModelCheckLifecycle::Enablement { enabled: true },
        );
    }
    STARTUP_ERRORS.set_unbound(None);
    changed(&app);
    availability(&app).map_err(str::to_owned)
}

#[tauri::command]
pub(crate) async fn remove_typesafe_api_key(
    app: AppHandle,
    window: WebviewWindow,
) -> Result<CheckAvailability, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
    lifecycle::ensure_no_pending_change(&app.state::<Store>()).map_err(str::to_owned)?;
    let had_saved_key = saved_key_marker(&app.state::<Store>());
    if !had_saved_key && !app.state::<WorkerHandle>().is_available() {
        return availability(&app).map_err(str::to_owned);
    }
    let store = app.state::<Store>();
    let saved = profiles(&store).map_err(str::to_owned)?;
    #[cfg(feature = "analytics")]
    let was_enabled = yaml_discovery_enabled(&store);
    let active_uses_legacy_key = saved
        .active_id
        .as_ref()
        .and_then(|id| saved.profiles.get(id))
        .is_some_and(|active| {
            matches!(active.credential, Some(CredentialReference::LegacyTypeSafe))
        });
    let saved =
        begin_legacy_removal(&store, &app.state::<WorkerHandle>()).map_err(str::to_owned)?;
    changed(&app);
    let removal = tauri::async_runtime::spawn_blocking({
        let store = store.inner().clone();
        move || finish_legacy_removal(&store, &SystemCredentialVault, &saved)
    })
    .await
    .map_err(|_| "Credential storage is unavailable.".to_owned())?;
    STARTUP_ERRORS.record_legacy_removal_result(active_uses_legacy_key, removal);
    if let Err(error) = removal {
        changed(&app);
        return Err(error.to_owned());
    }
    #[cfg(feature = "analytics")]
    if active_uses_legacy_key && was_enabled {
        crate::analytics::record_model_check_lifecycle(
            &app,
            crate::analytics::event::ModelCheckLifecycle::Enablement { enabled: false },
        );
    }
    if had_saved_key {
        crate::analytics::record_model_check_lifecycle(
            &app,
            crate::analytics::event::ModelCheckLifecycle::ProviderSetup {
                provider: crate::analytics::event::DecisionProvider::Jev,
                outcome: crate::analytics::event::ProviderSetupOutcome::CredentialRemoved,
            },
        );
    }
    changed(&app);
    availability(&app).map_err(str::to_owned)
}

#[cfg(test)]
pub(crate) mod headless {
    use super::*;

    struct OfflineKeylessVault;

    impl CredentialVault for OfflineKeylessVault {
        type Error = &'static str;

        fn read(&self, _: Option<&str>) -> Result<Option<String>, Self::Error> {
            Ok(None)
        }

        fn write(&self, _: Option<&str>, _: &str) -> Result<(), Self::Error> {
            Err("The keyless test vault does not accept writes.")
        }

        fn delete(&self, _: Option<&str>) -> Result<(), Self::Error> {
            Err("The keyless test vault does not accept removals.")
        }

        fn is_missing(_: &Self::Error) -> bool {
            false
        }

        fn write_failure(_: &Self::Error) -> &'static str {
            "The keyless test vault does not accept writes."
        }
    }

    pub(crate) fn setup_keyless_profiles(
        store: &Store,
        profiles: std::collections::BTreeMap<String, SystemOneConnection>,
        active_id: &str,
        enabled: bool,
    ) -> Result<(), &'static str> {
        if !profiles.contains_key(active_id) {
            return Err("The test active profile is missing.");
        }
        for connection in profiles.values() {
            if !matches!(
                connection.provider,
                SystemOneProvider::Custom | SystemOneProvider::Ollama
            ) || connection.credential.is_some()
            {
                return Err(
                    "The test profiles must use keyless Custom or local Ollama connections.",
                );
            }
            connection
                .validate()
                .map_err(|_| "The test connection is invalid.")?;
            if connection.provider == SystemOneProvider::Ollama {
                let SystemOneEndpoint::BaseUrl(base) = &connection.endpoint else {
                    return Err("The test Ollama endpoint is invalid.");
                };
                let endpoint =
                    url::Url::parse(base).map_err(|_| "The test Ollama endpoint is invalid.")?;
                if !endpoint.host().is_some_and(|host| match host {
                    url::Host::Ipv4(address) => address.is_loopback(),
                    url::Host::Ipv6(address) => address.is_loopback(),
                    url::Host::Domain(_) => false,
                }) {
                    return Err("The test Ollama endpoint must use a loopback IP address.");
                }
            }
        }
        migrate_provider_state(store)?;
        save_profiles(
            store,
            &ConnectionProfiles {
                active_id: Some(active_id.to_owned()),
                profiles,
                retired_ids: Default::default(),
            },
        )?;
        store
            .set_internal_value_checked(
                YAML_DISCOVERY_ENABLED_KEY,
                if enabled { "true" } else { "false" },
            )
            .map_err(|_| "Could not save test enablement.")
    }

    pub(crate) fn switch_keyless_profile(
        store: &Store,
        worker: &WorkerHandle,
        connection_id: &str,
    ) -> Result<bool, &'static str> {
        switch_connection(store, worker, connection_id, &OfflineKeylessVault)
            .map(|(_, changed)| changed)
    }

    pub(crate) fn restore_keyless_profile(
        store: &Store,
        worker: &WorkerHandle,
    ) -> Result<(), &'static str> {
        restore_saved_key_with_vault(store, worker, || Ok(None), &OfflineKeylessVault)
    }

    pub(crate) fn pause_model_checks(
        store: &Store,
        worker: &WorkerHandle,
    ) -> Result<(), &'static str> {
        super::pause_model_checks(store, worker)
    }
}

#[cfg(test)]
#[path = "settings/tests.rs"]
mod tests;

#[cfg(test)]
use tests::save_and_use as save_connection;
