//! Native TypeSafe credentials and authorized window controls.

use antiburn_local::checks::DetectorId;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use crate::dto::BurnCheckDetectorId;
use crate::jev::config::{
    CredentialReference, SystemOneConnection, SystemOneEndpoint, SystemOneProvider,
};
use crate::jev::worker::WorkerHandle;
use crate::store::{BurnCheckHistoryStatus, BurnCheckUsageSummary, Store};

const ENABLED_AT_KEY: &str = "internal:burnChecksEnabledAtEpochV1";
const SAVED_KEY_KEY: &str = "internal:typesafeKeySavedV1";
const HISTORY_DAYS_KEY: &str = "internal:jevBurnCheckHistoryDaysV1";
const AUTH_REJECTED_KEY: &str = "internal:typesafeAuthRejectedV1";
const CREDENTIAL_CHANGE_PENDING_KEY: &str = "internal:typesafeCredentialChangePendingV1";
const ACTIVE_CONNECTION_KEY: &str = "internal:smartChecksActiveConnectionV1";
const CONNECTION_PROFILES_KEY: &str = "internal:smartChecksConnectionsV1";
const PROVIDER_MIGRATION_KEY: &str = "internal:smartChecksProviderMigrationV1";
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
static STARTUP_ERROR: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);

fn enroll_registered_checks(store: &Store) -> Result<(), &'static str> {
    let enabled = store
        .enabled_checks()
        .map_err(|_| "Could not read check preferences.")?;
    let ids = crate::jev::worker::registered_checks()
        .iter()
        .filter(|check| DetectorId::from_key(check.id()).is_some_and(|id| enabled.contains(&id)))
        .map(|check| check.id())
        .collect::<Vec<_>>();
    store
        .capture_burn_check_boundaries(&ids, time::OffsetDateTime::now_utc().unix_timestamp())
        .map(|_| ())
        .map_err(|_| "Could not enroll checks in the local database.")
}

fn registered_history_status(store: &Store) -> anyhow::Result<BurnCheckHistoryStatus> {
    let enabled = store.enabled_checks()?;
    let checks = crate::jev::worker::registered_checks()
        .iter()
        .filter(|check| DetectorId::from_key(check.id()).is_some_and(|id| enabled.contains(&id)))
        .map(|check| {
            (
                check.id(),
                check.policy().idle_secs,
                check.evaluator_revision(),
            )
        })
        .collect::<Vec<_>>();
    store.historical_burn_check_status_for_checks(
        &checks,
        time::OffsetDateTime::now_utc().unix_timestamp(),
    )
}

#[cfg(feature = "analytics")]
fn history_window_label(days: u8) -> Option<&'static str> {
    match days {
        0 => Some("future"),
        7 => Some("7_days"),
        30 => Some("30_days"),
        _ => None,
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckAvailability {
    configured: bool,
    saved_key: bool,
    error: Option<&'static str>,
    usage: BurnCheckUsageSummary,
    history_days: u8,
    backfill: BurnCheckBackfillSummary,
    checks: Vec<CheckChoice>,
    revision: u64,
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BurnCheckBackfillSummary {
    total: usize,
    waiting_for_data: usize,
    waiting_for_idle: usize,
    ready: usize,
    queued: usize,
    running: usize,
    completed: usize,
    skipped: usize,
    failed: usize,
    reviewed: usize,
    eligible_items: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackfillRunResult {
    queued: usize,
    availability: CheckAvailability,
}

fn entry() -> Result<keyring::Entry, &'static str> {
    keyring::Entry::new(SERVICE, ACCOUNT).map_err(|_| "Credential storage is unavailable.")
}

trait CredentialVault {
    type Error;

    fn read(&self, id: Option<&str>) -> Result<Option<String>, Self::Error>;
    fn write(&self, id: Option<&str>, value: &str) -> Result<(), Self::Error>;
    fn delete(&self, id: Option<&str>) -> Result<(), Self::Error>;
    fn is_missing(error: &Self::Error) -> bool;
    fn write_failure(error: &Self::Error) -> &'static str;
}

struct SystemCredentialVault;

impl CredentialVault for SystemCredentialVault {
    type Error = keyring::Error;

    fn read(&self, id: Option<&str>) -> Result<Option<String>, Self::Error> {
        let entry = match id {
            Some(id) => connection_entry(id).map_err(vault_entry_error)?,
            None => entry().map_err(vault_entry_error)?,
        };
        match entry.get_password() {
            Ok(value) if !value.trim().is_empty() => Ok(Some(value)),
            Ok(_) | Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn write(&self, id: Option<&str>, value: &str) -> Result<(), Self::Error> {
        let entry = match id {
            Some(id) => connection_entry(id).map_err(vault_entry_error)?,
            None => entry().map_err(vault_entry_error)?,
        };
        entry.set_password(value)
    }

    fn delete(&self, id: Option<&str>) -> Result<(), Self::Error> {
        let entry = match id {
            Some(id) => connection_entry(id).map_err(vault_entry_error)?,
            None => entry().map_err(vault_entry_error)?,
        };
        entry.delete_credential()
    }

    fn is_missing(error: &Self::Error) -> bool {
        matches!(error, keyring::Error::NoEntry)
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

fn vault_entry_error(message: &'static str) -> keyring::Error {
    keyring::Error::PlatformFailure(Box::new(std::io::Error::other(message)))
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
    SystemCredentialVault
        .read(None)
        .map_err(|_| "Credential storage is unavailable.")
}

fn saved_key_marker(store: &Store) -> bool {
    store.internal_value(SAVED_KEY_KEY).as_deref() == Some("true")
        || store.internal_value(ENABLED_AT_KEY).is_some()
        || store
            .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
            .as_deref()
            == Some("true")
}

fn preserve_saved_key_marker(store: &Store) -> Result<(), &'static str> {
    if store.internal_value(SAVED_KEY_KEY).as_deref() == Some("true")
        || store.internal_value(ENABLED_AT_KEY).is_none()
    {
        return Ok(());
    }
    store
        .set_internal_value_checked(SAVED_KEY_KEY, "true")
        .map_err(|_| "Could not preserve the saved credential state.")
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ConnectionProfiles {
    active_id: String,
    profiles: std::collections::BTreeMap<String, SystemOneConnection>,
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
    active_id: String,
    profiles: std::collections::BTreeMap<String, SystemOneConnection>,
}

fn connection_entry(id: &str) -> Result<keyring::Entry, &'static str> {
    if id.is_empty()
        || id.len() > 96
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err("The connection credential reference is invalid.");
    }
    keyring::Entry::new(SERVICE, &format!("system-one-{id}"))
        .map_err(|_| "Credential storage is unavailable.")
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
    connection
        .validate()
        .map_err(|_| "Connection settings are invalid.")?;
    let credential = if draft.is_some() {
        draft
    } else {
        match connection.credential.as_ref() {
            Some(CredentialReference::LegacyTypeSafe) => vault.read(None),
            Some(CredentialReference::Connection(id)) => vault.read(Some(id)),
            None => return Ok(None),
        }
        .map_err(|_| "Credential storage is unavailable.")?
    };
    if credential.as_ref().is_some_and(|secret| {
        secret.trim().is_empty() || secret.len() > 4096 || secret.chars().any(char::is_control)
    }) {
        return Err("Enter a valid connection credential.");
    }
    credential
        .map(Some)
        .ok_or("The selected provider needs a credential. Add one and retry.")
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

fn legacy_removal_pending(store: &Store) -> bool {
    store
        .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
        .as_deref()
        == Some("true")
}

fn clear_legacy_removal<V: CredentialVault>(store: &Store, vault: &V) -> Result<(), &'static str> {
    delete_credential(vault, None)?;
    store
        .set_internal_value_checked(SAVED_KEY_KEY, "false")
        .map_err(|_| "Could not save the credential removal state.")?;
    store
        .set_internal_value_checked(CREDENTIAL_CHANGE_PENDING_KEY, "false")
        .map_err(|_| "Could not save the credential removal state.")
}

fn profiles(store: &Store) -> Result<ConnectionProfiles, &'static str> {
    migrate_provider_state(store)?;
    store
        .internal_value(CONNECTION_PROFILES_KEY)
        .and_then(|json| serde_json::from_str(&json).ok())
        .ok_or("Saved Smart Burn Checks provider settings are invalid.")
}

fn save_profiles(store: &Store, value: &ConnectionProfiles) -> Result<(), &'static str> {
    let encoded =
        serde_json::to_string(value).map_err(|_| "Could not save connection settings.")?;
    store
        .set_internal_values_checked(&[
            (CONNECTION_PROFILES_KEY, &encoded),
            (ACTIVE_CONNECTION_KEY, &value.active_id),
        ])
        .map_err(|_| "Could not save connection settings.")
}

fn persist_connection_profile(
    store: &Store,
    saved: &mut ConnectionProfiles,
    id: &str,
    mut connection: SystemOneConnection,
    credential: Option<String>,
    vault: &impl CredentialVault,
    worker: &WorkerHandle,
) -> Result<(SystemOneConnection, Option<String>, bool), &'static str> {
    ensure_no_pending_removal(store)?;
    if legacy_removal_pending(store) {
        clear_legacy_removal(store, vault)?;
    }
    let resolved = resolve_credential(&connection, credential.clone(), vault)?;
    if connection_requires_credential(&connection) && resolved.is_none() {
        return Err("The selected provider needs a credential. Add one and retry.");
    }
    let previous_secret = if credential.is_some() {
        vault
            .read(Some(id))
            .map_err(|_| "Credential storage is unavailable.")?
    } else {
        None
    };
    let previous_connection = saved
        .profiles
        .get(&saved.active_id)
        .cloned()
        .ok_or("The active Smart Burn Checks connection is missing.")?;
    let previous_credential = match previous_connection.credential.as_ref() {
        Some(CredentialReference::LegacyTypeSafe) => vault.read(None),
        Some(CredentialReference::Connection(previous_id)) => vault.read(Some(previous_id)),
        None => Ok(None),
    }
    .map_err(|_| "Credential storage is unavailable.")?;
    let checks_enabled = store.internal_value(ENABLED_AT_KEY).is_some();
    let replacing = credential.is_some();
    let credential_changed = replacing && previous_secret != credential;
    store
        .set_internal_value_checked(CONNECTION_PENDING_KEY, id)
        .map_err(|_| "Could not protect the connection update state.")?;
    worker.suspend_system_one();
    if let Some(secret) = credential {
        if let Err(error) = write_credential(vault, Some(id), &secret) {
            let current_secret = vault
                .read(Some(id))
                .map_err(|_| {
                    "Could not confirm the saved credential. Checks are paused. Retry the connection update in Settings → Checks."
                })?;
            if current_secret != previous_secret {
                let rollback = match previous_secret.as_deref() {
                    Some(previous) => write_credential(vault, Some(id), previous),
                    None => delete_credential(vault, Some(id)),
                };
                if rollback.is_err() {
                    return Err(
                        "Could not restore the previous credential. Checks are paused. Retry the connection update in Settings → Checks.",
                    );
                }
            }
            if worker
                .install_system_one_connection(
                    previous_connection,
                    previous_credential,
                    checks_enabled,
                )
                .is_err()
            {
                return Err(
                    "Could not restore the previous connection. Checks are paused. Retry the connection update in Settings → Checks.",
                );
            }
            store
                .set_internal_value_checked(CONNECTION_PENDING_KEY, "")
                .map_err(|_| {
                    "Could not restore the previous connection state. Retry the connection update in Settings → Checks."
                })?;
            return Err(error);
        }
        connection.credential = Some(CredentialReference::Connection(id.to_owned()));
    }
    let mut updated = saved.clone();
    let changed =
        credential_changed || saved.active_id != id || saved.profiles.get(id) != Some(&connection);
    updated.profiles.insert(id.to_owned(), connection.clone());
    updated.active_id = id.to_owned();
    if let Err(error) = save_profiles(store, &updated) {
        if replacing {
            let rollback = match previous_secret {
                Some(secret) => write_credential(vault, Some(id), &secret),
                None => delete_credential(vault, Some(id)),
            };
            if rollback.is_err() {
                return Err(
                    "Could not restore the connection credential. Retry the connection update in Settings → Checks.",
                );
            }
        }
        return Err(error);
    }
    *saved = updated;
    Ok((connection, resolved, changed))
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
    let mut profiles: ConnectionProfiles = store
        .internal_value(CONNECTION_PROFILES_KEY)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    profiles
        .profiles
        .entry("jev".to_owned())
        .or_insert_with(SystemOneConnection::jev_default);
    if profiles.active_id.is_empty() {
        profiles.active_id = "jev".to_owned();
    }
    let encoded = serde_json::to_string(&profiles)
        .map_err(|_| "Could not migrate Smart Burn Checks provider settings.")?;
    store
        .set_internal_value_checked(CONNECTION_PROFILES_KEY, &encoded)
        .map_err(|_| "Could not migrate Smart Burn Checks provider settings.")?;
    store
        .set_internal_value_checked(ACTIVE_CONNECTION_KEY, &profiles.active_id)
        .map_err(|_| "Could not migrate Smart Burn Checks provider settings.")?;
    store
        .set_internal_value_checked(PROVIDER_MIGRATION_KEY, "1")
        .map_err(|_| "Could not complete Smart Burn Checks provider migration.")
}

fn active_connection(store: &Store) -> Result<SystemOneConnection, &'static str> {
    migrate_provider_state(store)?;
    let profiles: ConnectionProfiles = store
        .internal_value(CONNECTION_PROFILES_KEY)
        .and_then(|json| serde_json::from_str(&json).ok())
        .ok_or("Saved Smart Burn Checks provider settings are invalid.")?;
    let id = store
        .internal_value(ACTIVE_CONNECTION_KEY)
        .unwrap_or(profiles.active_id);
    let connection = profiles
        .profiles
        .get(&id)
        .cloned()
        .ok_or("The active Smart Burn Checks connection is missing.")?;
    connection
        .validate()
        .map_err(|_| "Saved Smart Burn Checks provider settings are invalid.")?;
    Ok(connection)
}

fn restore_saved_key(
    store: &Store,
    worker: &WorkerHandle,
    read: impl FnOnce() -> Result<Option<String>, &'static str>,
) -> Result<(), &'static str> {
    worker.suspend_system_one();
    ensure_no_pending_removal(store)?;
    if store
        .internal_value(CONNECTION_PENDING_KEY)
        .is_some_and(|value| !value.is_empty())
    {
        return Err(
            "Smart Burn Checks connection update is incomplete. Retry it in Settings → Checks.",
        );
    }
    if store
        .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
        .as_deref()
        == Some("true")
    {
        return Err("TypeSafe credential removal is incomplete. Retry it in Settings → Checks.");
    }
    let connection = active_connection(store)?;
    if store.internal_value(ENABLED_AT_KEY).is_none() {
        return worker
            .install_system_one_connection(connection, None, false)
            .map_err(|_| "Saved Smart Burn Checks provider settings are invalid.");
    }
    enroll_registered_checks(store)?;
    let authentication_rejected =
        store.internal_value(AUTH_REJECTED_KEY).as_deref() == Some("true");
    if matches!(
        connection.credential,
        Some(CredentialReference::LegacyTypeSafe)
    ) {
        let key = if saved_key_marker(store) && !authentication_rejected {
            read()?
        } else {
            None
        };
        if saved_key_marker(store) && key.is_none() && !authentication_rejected {
            return Err("The saved TypeSafe API key is missing. Replace it in Settings → Checks.");
        }
        worker
            .install_system_one_connection(connection, key, !authentication_rejected)
            .map_err(|_| "Saved Smart Burn Checks provider settings are invalid.")?;
    } else if !authentication_rejected {
        let credential = read_connection_credential(&connection)?;
        worker
            .set_system_one_connection(connection, credential)
            .map_err(|_| "Saved Smart Burn Checks provider settings are invalid.")?;
        if saved_key_marker(store) && !authentication_rejected && !worker.is_available() {
            return Err(
                "The active model provider credential is missing. Update it in Settings → Checks.",
            );
        }
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn get_system_one_settings(
    window: WebviewWindow,
    store: tauri::State<'_, Store>,
) -> Result<SystemOneSettings, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let saved = profiles(&store).map_err(str::to_owned)?;
    Ok(SystemOneSettings {
        active_id: saved.active_id,
        profiles: saved.profiles,
    })
}

#[tauri::command]
pub(crate) async fn test_system_one_connection(
    window: WebviewWindow,
    mut draft: SystemOneDraft,
) -> Result<(), String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let provider = draft.connection.provider;
    let result = async {
        {
            let _change = CREDENTIAL_CHANGE.lock().await;
            ensure_no_pending_removal(&window.state::<Store>()).map_err(str::to_owned)?;
            draft.credential =
                resolve_credential(&draft.connection, draft.credential, &SystemCredentialVault)
                    .map_err(str::to_owned)?;
        }
        test_connection(draft).await
    }
    .await;
    crate::analytics::record_smart_check_lifecycle(
        window.app_handle(),
        crate::analytics::event::SmartCheckLifecycle::ProviderTest {
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
    let limits = draft
        .connection
        .capabilities()
        .map_err(|_| "Connection settings are invalid.".to_owned())?;
    let response = match draft.connection.provider {
        SystemOneProvider::Jev => {
            let key = draft
                .credential
                .ok_or_else(|| "Enter a TypeSafe API key.".to_owned())?;
            let client = crate::jev::client::TypeSafeClient::new(key)
                .map_err(|_| "Enter a valid TypeSafe API key.".to_owned())?;
            client
                .evaluate_async(&request)
                .await
                .map_err(|_| "TypeSafe rejected the test request. Check the API key.".to_owned())
        }
        SystemOneProvider::Ollama => {
            let SystemOneEndpoint::BaseUrl(base) = &draft.connection.endpoint else {
                return Err("Connection settings are invalid.".to_owned());
            };
            let client = crate::jev_ollama::OllamaClient::new(base, draft.credential)
                .map_err(|error| error.to_string())?;
            let discovered = client
                .discover(&draft.connection.model)
                .await
                .map_err(|error| error.to_string())?;
            let limits = draft
                .connection
                .apply_capability_overrides(discovered.capabilities);
            client
                .evaluate(&request, &limits)
                .await
                .map_err(|error| match error {
                    crate::jev_ollama::OllamaError::OldVersion
                    | crate::jev_ollama::OllamaError::MissingModel
                    | crate::jev_ollama::OllamaError::UnsupportedRunner
                    | crate::jev_ollama::OllamaError::ColdLoad
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
            .evaluate(&request)
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
) -> Result<SystemOneSettings, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
    let provider = draft.connection.provider;
    let result = save_connection(
        &app.state::<Store>(),
        &app.state::<WorkerHandle>(),
        draft,
        &SystemCredentialVault,
    );
    if let Ok((_, true)) = &result {
        crate::analytics::record_smart_check_lifecycle(
            &app,
            crate::analytics::event::SmartCheckLifecycle::ProviderSetup {
                provider: provider.into(),
                outcome: crate::analytics::event::ProviderSetupOutcome::Saved,
            },
        );
    }
    changed(&app);
    result.map(|(settings, _)| settings).map_err(str::to_owned)
}

fn save_connection(
    store: &Store,
    worker: &WorkerHandle,
    mut draft: SystemOneDraft,
    vault: &impl CredentialVault,
) -> Result<(SystemOneSettings, bool), &'static str> {
    draft
        .connection
        .validate()
        .map_err(|_| "Connection settings are invalid.")?;
    let id = draft.connection_id.clone();
    connection_entry(&id)?;
    let mut saved = profiles(store)?;
    ensure_no_pending_removal(store)?;
    let checks_enabled = store.internal_value(ENABLED_AT_KEY).is_some();
    let (connection, credential, changed) = persist_connection_profile(
        store,
        &mut saved,
        &id,
        draft.connection,
        draft.credential.take(),
        vault,
        worker,
    )?;
    if checks_enabled {
        enroll_registered_checks(store)?;
    }
    store
        .set_internal_values_checked(&[(CONNECTION_PENDING_KEY, ""), (AUTH_REJECTED_KEY, "false")])
        .map_err(|_| "Could not complete the connection update.")?;
    worker
        .install_system_one_connection(
            connection,
            credential.filter(|_| checks_enabled),
            checks_enabled,
        )
        .map_err(|_| "Connection settings are invalid.")?;
    *STARTUP_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    Ok((
        SystemOneSettings {
            active_id: saved.active_id,
            profiles: saved.profiles,
        },
        changed,
    ))
}

#[tauri::command]
pub(crate) async fn switch_system_one_connection(
    app: AppHandle,
    window: WebviewWindow,
    connection_id: String,
) -> Result<SystemOneSettings, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
    let result = switch_connection(
        &app.state::<Store>(),
        &app.state::<WorkerHandle>(),
        &connection_id,
        &SystemCredentialVault,
    );
    if let Ok((settings, true)) = &result {
        crate::analytics::record_smart_check_lifecycle(
            &app,
            crate::analytics::event::SmartCheckLifecycle::ProviderSetup {
                provider: settings.profiles[&settings.active_id].provider.into(),
                outcome: crate::analytics::event::ProviderSetupOutcome::Switched,
            },
        );
    }
    changed(&app);
    result.map(|(settings, _)| settings).map_err(str::to_owned)
}

fn switch_connection(
    store: &Store,
    worker: &WorkerHandle,
    connection_id: &str,
    vault: &impl CredentialVault,
) -> Result<(SystemOneSettings, bool), &'static str> {
    let mut saved = profiles(store)?;
    let changed = saved.active_id != connection_id;
    ensure_no_pending_removal(store)?;
    let connection = saved
        .profiles
        .get(connection_id)
        .cloned()
        .ok_or("The saved connection is missing.")?;
    connection
        .validate()
        .map_err(|_| "Saved connection settings are invalid.")?;
    let checks_enabled = store.internal_value(ENABLED_AT_KEY).is_some();
    if checks_enabled {
        enroll_registered_checks(store)?;
    }
    let credential = resolve_credential(&connection, None, vault)?;
    if connection_requires_credential(&connection) && credential.is_none() {
        return Err("The selected provider needs a credential. Add one and retry.");
    }
    store
        .set_internal_value_checked(CONNECTION_PENDING_KEY, connection_id)
        .map_err(|_| "Could not protect the connection update state.")?;
    worker.suspend_system_one();
    saved.active_id = connection_id.to_owned();
    save_profiles(store, &saved)?;
    store
        .set_internal_values_checked(&[(CONNECTION_PENDING_KEY, ""), (AUTH_REJECTED_KEY, "false")])
        .map_err(|_| "Could not complete the connection update.")?;
    worker
        .install_system_one_connection(
            connection,
            credential.filter(|_| checks_enabled),
            checks_enabled,
        )
        .map_err(|_| "Saved connection settings are invalid.")?;
    *STARTUP_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    Ok((
        SystemOneSettings {
            active_id: saved.active_id,
            profiles: saved.profiles,
        },
        changed,
    ))
}

#[tauri::command]
pub(crate) async fn refresh_system_one_limits(
    window: WebviewWindow,
    connection: SystemOneConnection,
    credential: Option<String>,
) -> Result<antiburn_local::analysis::jev::capabilities::ModelCapabilities, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let credential = {
        let _change = CREDENTIAL_CHANGE.lock().await;
        ensure_no_pending_removal(&window.state::<Store>()).map_err(str::to_owned)?;
        resolve_credential(&connection, credential, &SystemCredentialVault)
            .map_err(str::to_owned)?
    };
    refresh_limits(connection, credential).await
}

async fn refresh_limits(
    connection: SystemOneConnection,
    credential: Option<String>,
) -> Result<antiburn_local::analysis::jev::capabilities::ModelCapabilities, String> {
    let mut capabilities = connection
        .capabilities()
        .map_err(|_| "Connection settings are invalid.".to_owned())?;
    if connection.provider == SystemOneProvider::Ollama {
        let SystemOneEndpoint::BaseUrl(base) = &connection.endpoint else {
            return Err("Connection settings are invalid.".to_owned());
        };
        capabilities = crate::jev_ollama::OllamaClient::new(base, credential)
            .map_err(|error| error.to_string())?
            .discover(&connection.model)
            .await
            .map_err(|error| error.to_string())?
            .capabilities;
        capabilities = connection.apply_capability_overrides(capabilities);
    }
    Ok(capabilities)
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
    let result = remove_connection_credential(
        &store,
        &app.state::<WorkerHandle>(),
        &connection_id,
        &SystemCredentialVault,
    );
    if result.is_ok() {
        *STARTUP_ERROR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
    if let Ok(Some(provider)) = &result {
        crate::analytics::record_smart_check_lifecycle(
            &app,
            crate::analytics::event::SmartCheckLifecycle::ProviderSetup {
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
    profile.revision = profile
        .revision
        .checked_add(1)
        .ok_or("Connection settings are invalid.")?;
    let shared = saved.profiles.values().any(|profile| {
        profile.credential.as_ref() == Some(&CredentialReference::Connection(key_id.clone()))
    });
    let previous = if shared {
        None
    } else {
        vault
            .read(Some(&key_id))
            .map_err(|_| "Credential storage is unavailable.")?
    };
    store
        .set_internal_value_checked(CONNECTION_REMOVAL_KEY, id)
        .map_err(|_| "Could not protect the credential removal state.")?;
    if saved.active_id == id {
        worker.suspend_system_one();
        store
            .disable_burn_checks()
            .map_err(|_| "Could not disable checks.")?;
    }
    if !shared {
        delete_credential(vault, Some(&key_id))?;
    }
    let encoded =
        serde_json::to_string(&saved).map_err(|_| "Could not save connection settings.")?;
    let mut updates = vec![
        (CONNECTION_PROFILES_KEY, encoded.as_str()),
        (ACTIVE_CONNECTION_KEY, saved.active_id.as_str()),
        (CONNECTION_REMOVAL_KEY, ""),
    ];
    if saved.active_id == id {
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
            *STARTUP_ERROR
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
            changed(&app);
            return;
        }
        if let Err(error) = active_connection(&store) {
            *STARTUP_ERROR
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
            changed(&app);
            return;
        }
        let worker_app = app.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let worker = worker_app.state::<WorkerHandle>();
            restore_saved_key(&store, &worker, read_key)
        })
        .await;
        let error = match result {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error),
            Err(_) => Some("Credential storage is unavailable."),
        };
        *STARTUP_ERROR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = error;
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
    let status = registered_history_status(store)
        .map_err(|_| "Could not read check history from the local database.")?;
    let backfill = BurnCheckBackfillSummary::from(status);
    Ok(CheckAvailability {
        configured,
        saved_key,
        error,
        usage: store
            .burn_check_usage_summary()
            .map_err(|_| "Could not read check usage from the local database.")?,
        history_days: store
            .internal_value(HISTORY_DAYS_KEY)
            .and_then(|value| value.parse::<u8>().ok())
            .filter(|days| matches!(days, 0 | 7 | 30))
            .unwrap_or(0),
        backfill,
        checks: DetectorId::ALL
            .into_iter()
            .map(|detector| CheckChoice {
                id: detector.into(),
                enabled: enabled_checks.contains(&detector),
            })
            .collect(),
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
        || (saved_key_marker(&store)
            && store.internal_value(AUTH_REJECTED_KEY).as_deref() == Some("true"));
    let error = if removal_pending {
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
        *STARTUP_ERROR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    };
    read_check_availability(
        &store,
        app.state::<WorkerHandle>().is_available(),
        saved_key_marker(&store),
        error,
    )
}

impl From<BurnCheckHistoryStatus> for BurnCheckBackfillSummary {
    fn from(status: BurnCheckHistoryStatus) -> Self {
        Self {
            total: status.total,
            waiting_for_data: status.waiting_for_data,
            waiting_for_idle: status.waiting_for_idle,
            ready: status.ready,
            queued: status.queued,
            running: status.running,
            completed: status.completed,
            skipped: status.skipped,
            failed: status.failed,
            reviewed: status.reviewed,
            eligible_items: status.eligible_items,
        }
    }
}

pub(crate) fn changed(app: &AppHandle) {
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
    let connection = active_connection(&store).map_err(str::to_owned)?;
    let was_enabled = store.internal_value(ENABLED_AT_KEY).is_some();
    if !enabled {
        preserve_saved_key_marker(&store).map_err(str::to_owned)?;
        app.state::<WorkerHandle>().suspend_system_one();
        store
            .disable_burn_checks()
            .map_err(|_| "Could not pause Smart Burn Checks.".to_owned())?;
    } else {
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
        enroll_registered_checks(&store).map_err(str::to_owned)?;
        app.state::<WorkerHandle>()
            .set_system_one_connection(connection, credential)
            .map_err(|_| "Saved Smart Burn Checks provider settings are invalid.".to_owned())?;
        if !app.state::<WorkerHandle>().is_available() {
            return Err("The selected model provider needs a valid credential.".to_owned());
        }
        store.set_internal_value(AUTH_REJECTED_KEY, "false");
    }
    if was_enabled != enabled {
        crate::analytics::record_smart_check_lifecycle(
            &app,
            crate::analytics::event::SmartCheckLifecycle::Enablement { enabled },
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
    let smart = crate::jev::worker::registered_check_ids().contains(&detector);
    let save = || {
        store.set_check_enabled_with_smart_transition(
            detector,
            enabled,
            smart,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
    };
    let changed_preference = if smart {
        app.state::<WorkerHandle>()
            .persist_check_transition(detector.key(), save)
    } else {
        save()
    }
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
    crate::jev::worker::wake(&app);
    changed(&app);
    crate::analytics::record_check_enablement_saved(&app, detector, enabled);
    availability(&app).map_err(str::to_owned)
}

fn resume_after_key_save(was_enabled: bool, replacing: bool, was_configured: bool) -> bool {
    was_enabled || (replacing && !was_configured)
}

#[tauri::command]
pub(crate) async fn set_typesafe_api_key(
    app: AppHandle,
    window: WebviewWindow,
    key: Option<String>,
) -> Result<CheckAvailability, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    let _change = CREDENTIAL_CHANGE.lock().await;
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
    if key.is_none()
        && app
            .state::<Store>()
            .internal_value(CREDENTIAL_CHANGE_PENDING_KEY)
            .as_deref()
            == Some("true")
    {
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
    let replacing = key.is_some();
    let was_configured = saved_key_marker(&app.state::<Store>());
    let was_enabled = app
        .state::<Store>()
        .internal_value(ENABLED_AT_KEY)
        .is_some();
    if key
        .as_ref()
        .is_some_and(|key| key.is_empty() || key.len() > 4096 || key.chars().any(char::is_control))
    {
        return Err("Enter a valid TypeSafe API key.".to_owned());
    }
    // Stop work before replacing or loading a credential.
    app.state::<WorkerHandle>().suspend_system_one();
    let store = app.state::<Store>().inner().clone();
    #[cfg(feature = "analytics")]
    let first_enablement = replacing && !was_configured;
    if replacing && was_enabled {
        store
            .set_internal_value_checked(CREDENTIAL_CHANGE_PENDING_KEY, "true")
            .map_err(|_| "Could not protect the credential update state.".to_owned())?;
    }
    changed(&app);
    let key = tauri::async_runtime::spawn_blocking(move || {
        if let Some(key) = key {
            entry()?
                .set_password(&key)
                .map_err(|_| "Could not save the API key in credential storage.")?;
            Ok(key)
        } else {
            read_key()?.ok_or("The saved TypeSafe API key is missing. Enter a new key.")
        }
    })
    .await
    .map_err(|_| "Credential storage is unavailable.".to_owned())?
    .map_err(str::to_owned)?;
    let resume_after_save = resume_after_key_save(was_enabled, replacing, was_configured);
    if resume_after_save {
        enroll_registered_checks(&store).map_err(str::to_owned)?;
    }
    if replacing {
        store
            .set_internal_value_checked(SAVED_KEY_KEY, "true")
            .map_err(|_| "Could not save the credential state.".to_owned())?;
    }
    store
        .set_internal_value_checked(CREDENTIAL_CHANGE_PENDING_KEY, "false")
        .map_err(|_| "Could not save the credential update state.".to_owned())?;
    if replacing {
        store
            .retry_rejected_burn_checks()
            .map_err(|_| "Could not schedule checks after replacing the API key.".to_owned())?;
    }
    store.set_internal_value(AUTH_REJECTED_KEY, "false");
    let connection = active_connection(&store).map_err(str::to_owned)?;
    let credential = if matches!(
        connection.credential,
        Some(CredentialReference::LegacyTypeSafe)
    ) {
        Some(key)
    } else {
        read_connection_credential(&connection).map_err(str::to_owned)?
    };
    app.state::<WorkerHandle>()
        .install_system_one_connection(connection, credential, resume_after_save)
        .map_err(|_| "Saved Smart Burn Checks provider settings are invalid.".to_owned())?;
    #[cfg(feature = "analytics")]
    if first_enablement {
        crate::analytics::record_smart_check_lifecycle(
            &app,
            crate::analytics::event::SmartCheckLifecycle::Enablement { enabled: true },
        );
    }
    *STARTUP_ERROR
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
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
    let had_saved_key = saved_key_marker(&app.state::<Store>());
    if !had_saved_key && !app.state::<WorkerHandle>().is_available() {
        return availability(&app).map_err(str::to_owned);
    }
    let store = app.state::<Store>();
    let active = active_connection(&store).map_err(str::to_owned)?;
    #[cfg(feature = "analytics")]
    let was_enabled = store.internal_value(ENABLED_AT_KEY).is_some();
    let active_uses_legacy_key =
        matches!(active.credential, Some(CredentialReference::LegacyTypeSafe));
    let retrying_legacy_removal = legacy_removal_pending(&store);
    if active_uses_legacy_key {
        app.state::<WorkerHandle>().suspend_system_one();
        store
            .set_internal_value_checked(CREDENTIAL_CHANGE_PENDING_KEY, "true")
            .map_err(|_| "Could not protect the credential removal state.".to_owned())?;
        store
            .disable_burn_checks()
            .map_err(|_| "Could not disable checks.".to_owned())?;
    }
    changed(&app);
    #[cfg(feature = "analytics")]
    if active_uses_legacy_key && was_enabled {
        crate::analytics::record_smart_check_lifecycle(
            &app,
            crate::analytics::event::SmartCheckLifecycle::Enablement { enabled: false },
        );
    }
    let removal = tauri::async_runtime::spawn_blocking({
        let store = store.inner().clone();
        move || clear_legacy_removal(&store, &SystemCredentialVault)
    })
    .await
    .map_err(|_| "Credential storage is unavailable.".to_owned())?
    .map_err(str::to_owned);
    if let Err(error) = removal {
        if active_uses_legacy_key || retrying_legacy_removal {
            *STARTUP_ERROR
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some("Could not remove the API key from credential storage.");
        }
        changed(&app);
        return Err(error);
    }
    if active_uses_legacy_key || retrying_legacy_removal {
        *STARTUP_ERROR
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
    if had_saved_key {
        crate::analytics::record_smart_check_lifecycle(
            &app,
            crate::analytics::event::SmartCheckLifecycle::ProviderSetup {
                provider: crate::analytics::event::SmartCheckProvider::Jev,
                outcome: crate::analytics::event::ProviderSetupOutcome::CredentialRemoved,
            },
        );
    }
    changed(&app);
    availability(&app).map_err(str::to_owned)
}

#[tauri::command]
pub(crate) fn set_check_history_days(
    app: AppHandle,
    window: WebviewWindow,
    days: u8,
) -> Result<CheckAvailability, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    if !matches!(days, 0 | 7 | 30) {
        return Err("Choose future checks, 7 days, or 30 days.".to_owned());
    }
    let store = app.state::<Store>();
    #[cfg(feature = "analytics")]
    let previous = store
        .internal_value(HISTORY_DAYS_KEY)
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| matches!(value, 0 | 7 | 30))
        .unwrap_or(0);
    store
        .set_internal_value_checked(HISTORY_DAYS_KEY, &days.to_string())
        .map_err(|_| "Could not save the check history window.".to_owned())?;
    #[cfg(feature = "analytics")]
    if previous != days
        && let Some(detail) = history_window_label(days)
    {
        crate::analytics::record(
            &app,
            crate::analytics::event::EventName::IgnoredInstructionLifecycle,
            crate::analytics::event::Facts {
                label: Some("history_window"),
                detail: Some(detail),
                ..Default::default()
            },
        );
    }
    changed(&app);
    availability(&app).map_err(str::to_owned)
}

#[tauri::command]
pub(crate) fn run_check_backfill(
    app: AppHandle,
    window: WebviewWindow,
) -> Result<BackfillRunResult, String> {
    checks_settings_window(&window).map_err(str::to_owned)?;
    if !app.state::<WorkerHandle>().is_available()
        || app.state::<WorkerHandle>().authentication_rejected()
        || app
            .state::<Store>()
            .internal_value(AUTH_REJECTED_KEY)
            .as_deref()
            == Some("true")
    {
        return Err("Add a working TypeSafe API key before running checks.".to_owned());
    }
    let days = app
        .state::<Store>()
        .internal_value(HISTORY_DAYS_KEY)
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|days| matches!(days, 7 | 30))
        .ok_or_else(|| "Choose the last 7 or 30 days first.".to_owned())?;
    let store = app.state::<Store>();
    store
        .reconcile_evidence_revisions(
            &crate::agents::evidence_cohort(),
            crate::analysis::projection_revisions(),
        )
        .map_err(|_| "Could not refresh session evidence for this check.".to_owned())?;
    let enabled = store
        .enabled_checks()
        .map_err(|_| "Could not read check preferences.".to_owned())?;
    let checks = crate::jev::worker::registered_checks()
        .iter()
        .filter(|check| DetectorId::from_key(check.id()).is_some_and(|id| enabled.contains(&id)))
        .map(|check| (check.id(), check.evaluator_revision()))
        .collect::<Vec<_>>();
    if checks.is_empty() {
        return Err("Turn on a Smart Burn Check before checking past sessions.".to_owned());
    }
    let queued = app
        .state::<Store>()
        .enqueue_burn_checks_for_revisions(
            &checks,
            time::OffsetDateTime::now_utc().unix_timestamp(),
            days,
        )
        .map_err(|_| "Could not queue checks for this period.".to_owned())?;
    let progress = registered_history_status(&store)
        .map_err(|_| "Could not read check progress.".to_owned())?;
    ::tracing::info!(
        event = "ignored_instruction_history_requested",
        days,
        queued,
        total = progress.total,
        waiting_for_data = progress.waiting_for_data,
        waiting_for_idle = progress.waiting_for_idle,
        ready = progress.ready,
        running = progress.running,
        completed = progress.completed,
        skipped = progress.skipped,
        failed = progress.failed,
    );
    crate::insights_worker::wake(&app);
    crate::jev::worker::wake(&app);
    #[cfg(feature = "analytics")]
    crate::analytics::record(
        &app,
        crate::analytics::event::EventName::IgnoredInstructionLifecycle,
        crate::analytics::event::Facts {
            label: Some("backfill"),
            detail: Some("requested"),
            ..Default::default()
        },
    );
    progress_changed(&app);
    Ok(BackfillRunResult {
        queued,
        availability: availability(&app).map_err(str::to_owned)?,
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::Path;

    use super::{
        AUTH_REJECTED_KEY, CONNECTION_PENDING_KEY, CONNECTION_PROFILES_KEY,
        CREDENTIAL_CHANGE_PENDING_KEY, CheckAvailabilityEvent, CredentialVault, ENABLED_AT_KEY,
        PROVIDER_MIGRATION_KEY, active_connection, delete_credential, migrate_provider_state,
        persist_connection_profile, preserve_saved_key_marker, restore_saved_key, saved_key_marker,
        write_credential,
    };
    use crate::jev::worker::WorkerHandle;
    use crate::store::Store;

    #[test]
    fn restore_and_save_enroll_the_current_registry_without_resetting_legacy_state() {
        for restore in [true, false] {
            let store =
                Store::open_in_memory(Path::new("/tmp/antiburn-registry-settings-test")).unwrap();
            for detector in crate::jev::worker::registered_check_ids() {
                store.set_check_enabled(detector, true).unwrap();
            }
            store.set_internal_value(ENABLED_AT_KEY, "123");
            let worker = WorkerHandle::default();
            if restore {
                restore_saved_key(&store, &worker, || Ok(Some("synthetic-key".into()))).unwrap();
            } else {
                super::save_connection(
                    &store,
                    &worker,
                    super::SystemOneDraft {
                        connection_id: "keyless".into(),
                        connection: custom_connection(),
                        credential: None,
                    },
                    &OfflineVault::default(),
                )
                .unwrap();
            }
            let epochs = crate::jev::worker::registered_checks()
                .iter()
                .map(|check| {
                    let key = format!("internal:burnCheckEnabledAtEpochV1:{}", check.id());
                    let epoch = store
                        .internal_value(&key)
                        .expect("every registered check has an epoch with zero sessions");
                    if check.id() == "ignored_instructions" {
                        assert_eq!(epoch, "123");
                    } else {
                        assert!(epoch.parse::<i64>().unwrap() > 123);
                    }
                    (key, epoch)
                })
                .collect::<Vec<_>>();
            super::enroll_registered_checks(&store).unwrap();
            for (key, epoch) in epochs {
                assert_eq!(store.internal_value(&key), Some(epoch));
            }
            assert_eq!(store.internal_value(ENABLED_AT_KEY).as_deref(), Some("123"));
            assert!(worker.is_available());
        }
    }

    #[derive(Default)]
    struct OfflineVault {
        fail_read: Cell<bool>,
        fail_write: Cell<bool>,
        write_calls: Cell<usize>,
        fail_write_at: Cell<Option<usize>>,
        fail_delete: Cell<bool>,
        value: std::cell::RefCell<Option<String>>,
        deleted_ids: std::cell::RefCell<Vec<Option<String>>>,
    }

    impl CredentialVault for OfflineVault {
        type Error = &'static str;

        fn read(&self, _id: Option<&str>) -> Result<Option<String>, Self::Error> {
            if self.fail_read.replace(false) {
                return Err("read");
            }
            Ok(self.value.borrow().clone())
        }

        fn write(&self, _id: Option<&str>, value: &str) -> Result<(), Self::Error> {
            let call = self.write_calls.get() + 1;
            self.write_calls.set(call);
            if self.fail_write.replace(false) || self.fail_write_at.get() == Some(call) {
                return Err("write");
            }
            *self.value.borrow_mut() = Some(value.to_owned());
            Ok(())
        }

        fn delete(&self, id: Option<&str>) -> Result<(), Self::Error> {
            if self.fail_delete.replace(false) {
                return Err("delete");
            }
            self.value.borrow_mut().take();
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
            context_override: Some(crate::jev::config::ContextLimitOverride {
                total_input_tokens: Some(8192),
                ..crate::jev::config::ContextLimitOverride::default()
            }),
            ..crate::jev::config::SystemOneConnection::default()
        }
    }

    fn saved_custom(active: bool) -> (Store, WorkerHandle, OfflineVault) {
        let store = Store::open_in_memory(Path::new("/tmp/antiburn-credential-lifecycle")).unwrap();
        migrate_provider_state(&store).unwrap();
        let worker = WorkerHandle::default();
        let vault = OfflineVault::default();
        if active {
            store.set_internal_value(ENABLED_AT_KEY, "123");
        }
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
    fn custom_without_input_bound_cannot_save_or_activate() {
        let store = Store::open_in_memory(Path::new("/tmp/antiburn-custom-limits")).unwrap();
        migrate_provider_state(&store).unwrap();
        let worker = WorkerHandle::default();
        let vault = OfflineVault::default();
        let original = super::profiles(&store).unwrap();
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
            assert!(
                super::save_connection(
                    &store,
                    &worker,
                    super::SystemOneDraft {
                        connection_id: "proxy".into(),
                        connection,
                        credential: Some("synthetic-secret".into()),
                    },
                    &vault,
                )
                .is_err()
            );
            let saved = super::profiles(&store).unwrap();
            assert_eq!(saved.active_id, original.active_id);
            assert_eq!(saved.profiles, original.profiles);
            assert!(vault.value.borrow().is_none());
        }
    }

    #[test]
    fn provider_operation_outcomes_require_real_saved_transitions() {
        let (store, worker, vault) = saved_custom(true);
        let draft = super::SystemOneDraft {
            connection_id: "proxy".into(),
            connection: custom_connection(),
            credential: Some("synthetic-secret".into()),
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
        assert!(worker.is_available());
        store.set_internal_value(AUTH_REJECTED_KEY, "true");
        super::remove_connection_credential(&store, &worker, "proxy", &vault).unwrap();
        assert!(!worker.is_available());
        assert!(store.internal_value(ENABLED_AT_KEY).is_none());
        assert!(active_connection(&store).unwrap().credential.is_none());
        assert!(vault.value.borrow().is_none());
        assert_eq!(
            store.internal_value(AUTH_REJECTED_KEY).as_deref(),
            Some("false")
        );
        super::remove_connection_credential(&store, &worker, "proxy", &vault).unwrap();
        assert_eq!(*vault.deleted_ids.borrow(), vec![Some("proxy".into())]);
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
        assert_eq!(super::profiles(&store).unwrap().active_id, "keyless");
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
        assert!(store.internal_value(ENABLED_AT_KEY).is_some());
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
        assert!(
            restore_saved_key(&store, &worker, || panic!("pending removal read a key")).is_err()
        );
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
                Some("")
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
        assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
        assert!(!worker.is_available());
        assert!(
            restore_saved_key(&store, &worker, || panic!("pending update reads no keys")).is_err()
        );
        store
            .lock()
            .execute_batch("DROP TRIGGER fail_profiles")
            .unwrap();
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
    fn failed_save_rollback_reports_failure_and_keeps_launch_blocked_until_retry() {
        let (store, worker, vault) = saved_custom(true);
        fail_profile_writes(&store);
        vault.fail_write_at.set(Some(vault.write_calls.get() + 2));
        let draft = super::SystemOneDraft {
            connection_id: "proxy".into(),
            connection: custom_connection(),
            credential: Some("replacement".into()),
        };
        let error = super::save_connection(&store, &worker, draft.clone(), &vault).unwrap_err();
        assert!(error.contains("restore"));
        assert_eq!(vault.value.borrow().as_deref(), Some("replacement"));
        assert!(!worker.is_available());
        assert!(
            restore_saved_key(&store, &worker, || panic!("failed rollback reads no keys")).is_err()
        );
        store
            .lock()
            .execute_batch("DROP TRIGGER fail_profiles")
            .unwrap();
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
             WHEN NEW.key = 'internal:smartChecksConnectionChangePendingV1' AND NEW.value = ''
             BEGIN SELECT RAISE(ABORT, 'injected finalization failure'); END;",
            )
            .unwrap();
        let draft = super::SystemOneDraft {
            connection_id: "keyless".into(),
            connection: custom_connection(),
            credential: None,
        };
        assert!(super::save_connection(&store, &worker, draft.clone(), &vault).is_err());
        assert_eq!(super::profiles(&store).unwrap().active_id, "keyless");
        assert!(!worker.is_available());
        assert!(
            restore_saved_key(&store, &worker, || panic!("staged update reads no keys")).is_err()
        );
        store
            .lock()
            .execute_batch("DROP TRIGGER fail_finalize")
            .unwrap();
        super::save_connection(&store, &worker, draft, &vault).unwrap();
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
        store.set_internal_value(ENABLED_AT_KEY, "123");
        super::switch_connection(&store, &worker, "keyless", &vault).unwrap();
        assert!(worker.is_available());
        worker.suspend_system_one();
        assert!(!worker.is_available());
        restore_saved_key(&store, &worker, || panic!("keyless launch reads no keys")).unwrap();
        assert!(worker.is_available());
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
                    ("/api/version", serde_json::json!({"version":"0.35.0"})),
                    (
                        "/api/tags",
                        serde_json::json!({"models":[{"name":"clef-flash:latest", "digest":"synthetic-digest"}]}),
                    ),
                    (
                        "/api/show",
                        serde_json::json!({"capabilities":["decision"], "model_info":{"general.architecture":"clef","clef.context_length":16384}}),
                    ),
                    (
                        "/api/ps",
                        serde_json::json!({"models":[{"name":"clef-flash:latest","digest":"synthetic-digest","context_length":8192}]}),
                    ),
                ];
                for (route, body) in discovery.clone().into_iter().chain([(
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
            Some("")
        );
        assert_eq!(
            active_connection(&store).unwrap().provider,
            crate::jev::config::SystemOneProvider::Jev
        );
        assert!(!worker.is_available());

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
        assert_eq!(
            active.credential,
            Some(crate::jev::config::CredentialReference::Connection(
                "proxy".to_owned()
            ))
        );
        assert_eq!(vault.value.borrow().as_deref(), Some("synthetic-secret"));
    }

    #[test]
    fn migration_retries_after_profile_state_was_written_without_its_marker() {
        let store = Store::open_in_memory(Path::new("/tmp/antiburn-provider-migrate-retry"))
            .expect("store");
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
        saved.active_id = "local".to_owned();
        super::save_profiles(&store, &saved).expect("persist before migration marker");
        store.set_internal_value(ENABLED_AT_KEY, "123");
        store.set_internal_value(super::HISTORY_DAYS_KEY, "30");

        migrate_provider_state(&store).expect("retry interrupted migration");

        assert_eq!(active_connection(&store).unwrap(), saved.profiles["local"]);
        assert_eq!(
            store.internal_value(PROVIDER_MIGRATION_KEY).as_deref(),
            Some("1")
        );
        assert_eq!(store.internal_value(ENABLED_AT_KEY).as_deref(), Some("123"));
        assert_eq!(
            store.internal_value(super::HISTORY_DAYS_KEY).as_deref(),
            Some("30")
        );
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
    fn availability_lists_all_checks_and_excludes_disabled_checks_from_history() {
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
            .set_check_enabled(
                antiburn_local::checks::DetectorId::IgnoredInstructions,
                false,
            )
            .unwrap();
        assert_eq!(super::registered_history_status(&store).unwrap().total, 0);
        assert_eq!(
            super::read_check_availability(&store, false, false, None)
                .unwrap()
                .revision,
            1
        );
    }

    #[test]
    fn startup_skips_credential_storage_without_the_enabled_marker() {
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
        let store =
            Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-paused")).expect("store");
        store.set_internal_value(ENABLED_AT_KEY, "123");

        preserve_saved_key_marker(&store).expect("migrate saved key marker");
        store.disable_burn_checks().expect("pause checks");

        assert!(saved_key_marker(&store));
        assert!(store.internal_value(ENABLED_AT_KEY).is_none());
    }

    #[test]
    fn startup_restores_only_a_saved_valid_key() {
        let store =
            Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-restored")).expect("store");
        let worker = WorkerHandle::default();
        store.set_internal_value(ENABLED_AT_KEY, "123");
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
    fn removal_before_startup_restore_prevents_a_keychain_read() {
        let store =
            Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-removed")).expect("store");
        let worker = WorkerHandle::default();
        store.set_internal_value(ENABLED_AT_KEY, "123");
        store.disable_burn_checks().expect("marker removed");
        restore_saved_key(&store, &worker, || panic!("removed key read from keychain"))
            .expect("restore skipped");
        assert!(!worker.is_available());
    }

    #[test]
    fn missing_saved_key_does_not_start_checks() {
        let store =
            Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-missing")).expect("store");
        let worker = WorkerHandle::default();
        store.set_internal_value(ENABLED_AT_KEY, "123");
        assert!(restore_saved_key(&store, &worker, || Ok(None)).is_err());
        assert!(!worker.is_available());
    }

    #[test]
    fn pending_credential_removal_blocks_startup_restore() {
        let store = Store::open_in_memory(Path::new("/tmp/antiburn-typesafe-pending-remove"))
            .expect("store");
        let worker = WorkerHandle::default();
        store.set_internal_value(ENABLED_AT_KEY, "123");
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
        store.set_internal_value(ENABLED_AT_KEY, "123");
        store.set_internal_value(CREDENTIAL_CHANGE_PENDING_KEY, "true");
        let vault = OfflineVault::default();
        let mut profiles = super::profiles(&store).expect("read profiles");
        let replacement = crate::jev::config::SystemOneConnection {
            provider: crate::jev::config::SystemOneProvider::Ollama,
            endpoint: crate::jev::config::SystemOneEndpoint::BaseUrl(
                "http://127.0.0.1:11434".into(),
            ),
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
        assert!(worker.is_available());
    }

    #[test]
    fn provider_migration_is_idempotent_and_preserves_legacy_enablement_and_history() {
        let store =
            Store::open_in_memory(Path::new("/tmp/antiburn-provider-migration")).expect("store");
        store.set_internal_value(ENABLED_AT_KEY, "123");
        store.set_internal_value(super::HISTORY_DAYS_KEY, "30");
        store.set_internal_value(super::SAVED_KEY_KEY, "true");

        migrate_provider_state(&store).expect("migrate");
        let first = store.internal_value(CONNECTION_PROFILES_KEY).unwrap();
        migrate_provider_state(&store).expect("repeat migration");

        assert_eq!(store.internal_value(CONNECTION_PROFILES_KEY), Some(first));
        assert_eq!(
            store.internal_value(PROVIDER_MIGRATION_KEY).as_deref(),
            Some("1")
        );
        assert_eq!(store.internal_value(ENABLED_AT_KEY).as_deref(), Some("123"));
        assert_eq!(
            store.internal_value(super::HISTORY_DAYS_KEY).as_deref(),
            Some("30")
        );
        assert!(saved_key_marker(&store));
        assert!(active_connection(&store).is_ok());
    }

    #[test]
    fn keyless_provider_migration_does_not_read_or_create_a_credential() {
        let store =
            Store::open_in_memory(Path::new("/tmp/antiburn-keyless-migration")).expect("store");
        migrate_provider_state(&store).expect("migrate keyless settings");
        assert!(!saved_key_marker(&store));
        assert!(active_connection(&store).is_ok());
    }

    #[test]
    fn keyless_ollama_connection_restores_without_touching_the_keyring() {
        let store =
            Store::open_in_memory(Path::new("/tmp/antiburn-keyless-ollama")).expect("store");
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
        profiles.active_id = "local".to_owned();
        super::save_profiles(&store, &profiles).unwrap();
        store.set_internal_value(ENABLED_AT_KEY, "123");

        let worker = WorkerHandle::default();
        super::restore_saved_key(&store, &worker, || {
            panic!("Ollama must not read TypeSafe keyring")
        })
        .unwrap();

        assert_eq!(worker.system_one_connection(), connection);
        assert!(worker.is_available());
        store.disable_burn_checks().unwrap();
        super::restore_saved_key(&store, &worker, || panic!("paused startup reads no keys"))
            .unwrap();
        assert_eq!(worker.system_one_connection(), connection);
        assert!(!worker.is_available());
    }

    #[test]
    fn saved_connection_profiles_switch_and_restore_to_jev_without_serializing_secrets() {
        let store =
            Store::open_in_memory(Path::new("/tmp/antiburn-provider-profiles")).expect("store");
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
        saved.active_id = "local".to_owned();
        super::save_profiles(&store, &saved).unwrap();
        assert_eq!(active_connection(&store).unwrap(), ollama);

        saved.active_id = "jev".to_owned();
        super::save_profiles(&store, &saved).unwrap();
        assert_eq!(
            active_connection(&store).unwrap(),
            crate::jev::config::SystemOneConnection::jev_default()
        );
        let encoded = store.internal_value(CONNECTION_PROFILES_KEY).unwrap();
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("api_key"));
    }
}
