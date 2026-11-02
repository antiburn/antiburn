use super::*;
use display_numbers::DisplayNumbers;

pub(super) const KEY: &str = "internal:smartChecksProfileOperationV1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationKind {
    Save,
    Switch,
    Remove,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingOperation {
    pub id: String,
    pub kind: OperationKind,
    pub connection_id: String,
    pub request_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct PendingChange {
    pub descriptor: PendingOperation,
    pub revision: u64,
    pub before: ConnectionProfiles,
    pub after: ConnectionProfiles,
    pub labels_before: DisplayNumbers,
    pub labels_after: DisplayNumbers,
    pub new_slot: Option<String>,
    pub delete_slot: Option<String>,
    pub active_impacted: bool,
    pub pause_checks: bool,
    pub disabled_checks: bool,
    pub provider: Option<SystemOneProvider>,
    pub committed: bool,
    pub legacy_marker: Option<String>,
}

#[derive(Deserialize)]
struct PreviousChange {
    id: String,
    revision: u64,
    before: ConnectionProfiles,
    after: ConnectionProfiles,
    new_slot: Option<String>,
    delete_slot: Option<String>,
    active_impacted: bool,
    pause_checks: bool,
    was_enabled: bool,
    removed_provider: Option<SystemOneProvider>,
    committed: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredChange {
    Current(Box<PendingChange>),
    Previous(Box<PreviousChange>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationOutcome {
    Completed,
    RolledBack,
}

#[derive(Debug, Serialize)]
pub(crate) struct OperationResult {
    pub settings: SystemOneSettings,
    pub outcome: OperationOutcome,
}

pub(super) struct Metadata {
    pub profiles: ConnectionProfiles,
    pub labels: DisplayNumbers,
    pub pending: Option<PendingChange>,
}

pub(super) fn fresh_id(prefix: &str) -> Result<String, &'static str> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "Could not create a secure operation identity.")?;
    Ok(format!(
        "{prefix}{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

pub(super) fn validate_request_id(id: &str) -> Result<(), &'static str> {
    if id.len() != 36
        || !id.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
        || !id
            .bytes()
            .any(|byte| byte.is_ascii_hexdigit() && byte != b'0')
    {
        return Err("The Save request identity is invalid. Reopen the connection editor.");
    }
    Ok(())
}

fn stored_change(store: &Store) -> Result<Option<StoredChange>, &'static str> {
    store
        .internal_value(KEY)
        .filter(|raw| !raw.is_empty())
        .map(|raw| {
            serde_json::from_str(&raw)
                .map_err(|_| "The saved connection operation is invalid. Checks cannot resume.")
        })
        .transpose()
}

pub(super) fn ensure_no_pending(store: &Store) -> Result<(), &'static str> {
    if store.internal_value(KEY).is_some_and(|raw| !raw.is_empty()) {
        return Err("A connection operation is pending. Use Retry in Settings → Checks.");
    }
    Ok(())
}

pub(super) fn pending_affects_active(store: &Store) -> bool {
    match stored_change(store) {
        Ok(None) => false,
        Ok(Some(StoredChange::Current(change))) => change.active_impacted,
        Ok(Some(StoredChange::Previous(change))) => change.active_impacted,
        Err(_) => true,
    }
}

pub(super) fn active_pending_operation_id(store: &Store) -> Option<String> {
    match stored_change(store) {
        Ok(Some(StoredChange::Current(change))) if change.active_impacted => {
            Some(change.descriptor.id)
        }
        _ => None,
    }
}

fn expected(change: &PendingChange) -> (&ConnectionProfiles, &DisplayNumbers) {
    if change.committed {
        (&change.after, &change.labels_after)
    } else {
        (&change.before, &change.labels_before)
    }
}

pub(super) fn metadata(store: &Store) -> Result<Metadata, &'static str> {
    let saved = profiles(store)?;
    let stored = stored_change(store)?;
    let legacy = store
        .internal_value(CONNECTION_PENDING_KEY)
        .filter(|value| !value.is_empty());
    let previous_labels = DisplayNumbers::read(store)?;
    if let Some(StoredChange::Current(change)) = stored {
        validate_connection_id(&change.descriptor.id)?;
        validate_connection_id(&change.descriptor.connection_id)?;
        if change.committed && change.legacy_marker.is_none() && change.provider.is_none() {
            return Err("The saved connection operation provider is missing.");
        }
        if legacy.as_ref() != change.legacy_marker.as_ref() {
            return Err("The saved connection recovery markers do not match.");
        }
        let (expected_profiles, expected_labels) = expected(&change);
        if &saved != expected_profiles || previous_labels.as_ref() != Some(expected_labels) {
            return Err("The saved connection operation does not match the saved settings.");
        }
        expected_labels.public(&saved)?;
        return Ok(Metadata {
            profiles: saved,
            labels: expected_labels.clone(),
            pending: Some(*change),
        });
    }
    let mut inventory = saved.profiles.clone();
    if let Some(StoredChange::Previous(change)) = &stored {
        if legacy.is_some() {
            return Err("The saved connection recovery markers conflict.");
        }
        let expected_profiles = if change.committed {
            &change.after
        } else {
            &change.before
        };
        if &saved != expected_profiles {
            return Err("The saved connection operation does not match the saved settings.");
        }
        for (id, connection) in change
            .before
            .profiles
            .iter()
            .chain(change.after.profiles.iter())
        {
            if inventory
                .get(id)
                .is_some_and(|saved| saved.provider != connection.provider)
            {
                return Err("The saved connection operation has conflicting providers.");
            }
            inventory.insert(id.clone(), connection.clone());
        }
    }
    let mut labels = previous_labels.clone().unwrap_or_default();
    labels.migrate(&inventory)?;
    let pending = match stored {
        Some(StoredChange::Previous(change)) => {
            let provider = change.removed_provider.or_else(|| {
                change
                    .after
                    .profiles
                    .get(&change.id)
                    .or_else(|| change.before.profiles.get(&change.id))
                    .map(|profile| profile.provider)
            });
            Some(PendingChange {
                descriptor: PendingOperation {
                    id: fresh_id("operation-")?,
                    kind: if change.removed_provider.is_some() {
                        OperationKind::Remove
                    } else {
                        OperationKind::Save
                    },
                    connection_id: change.id,
                    request_id: None,
                },
                revision: change.revision,
                before: change.before,
                after: change.after,
                labels_before: labels.clone(),
                labels_after: labels.clone(),
                new_slot: change.new_slot,
                delete_slot: change.delete_slot,
                active_impacted: change.active_impacted,
                pause_checks: change.pause_checks,
                disabled_checks: change.removed_provider.is_some()
                    && change.active_impacted
                    && change.was_enabled,
                provider,
                committed: change.committed,
                legacy_marker: None,
            })
        }
        None => legacy
            .map(|marker| -> Result<PendingChange, &'static str> {
                validate_connection_id(&marker)?;
                Ok(PendingChange {
                    descriptor: PendingOperation {
                        id: fresh_id("operation-")?,
                        kind: OperationKind::Switch,
                        connection_id: marker.clone(),
                        request_id: None,
                    },
                    revision: 0,
                    before: saved.clone(),
                    after: saved.clone(),
                    labels_before: labels.clone(),
                    labels_after: labels.clone(),
                    new_slot: None,
                    delete_slot: None,
                    active_impacted: true,
                    pause_checks: true,
                    disabled_checks: false,
                    provider: None,
                    committed: false,
                    legacy_marker: Some(marker),
                })
            })
            .transpose()?,
        Some(StoredChange::Current(_)) => {
            unreachable!("Current operations return before migration")
        }
    };
    let encoded_labels = labels.encode()?;
    let encoded_pending = pending
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| "Could not migrate the connection operation.")?;
    let mut updates = Vec::new();
    if previous_labels.as_ref() != Some(&labels) {
        updates.push((display_numbers::KEY, encoded_labels.as_str()));
    }
    if let Some(encoded) = &encoded_pending {
        updates.push((KEY, encoded.as_str()));
    }
    if !updates.is_empty() {
        store
            .set_internal_values_checked(&updates)
            .map_err(|_| "Could not migrate connection recovery metadata.")?;
    }
    Ok(Metadata {
        profiles: saved,
        labels,
        pending,
    })
}

fn snapshot(
    saved: ConnectionProfiles,
    labels: &DisplayNumbers,
    pending: Option<PendingOperation>,
) -> Result<SystemOneSettings, &'static str> {
    let display_numbers = labels.public(&saved)?;
    Ok(SystemOneSettings {
        active_id: saved.active_id,
        profiles: saved.profiles,
        pending_operation: pending,
        display_numbers,
    })
}

pub(super) fn settings_snapshot(store: &Store) -> Result<SystemOneSettings, &'static str> {
    let metadata = metadata(store)?;
    snapshot(
        metadata.profiles,
        &metadata.labels,
        metadata.pending.map(|change| change.descriptor),
    )
}

pub(super) fn begin(
    store: &Store,
    worker: &WorkerHandle,
    change: &PendingChange,
) -> Result<(), &'static str> {
    let encoded =
        serde_json::to_string(change).map_err(|_| "Could not save the connection operation.")?;
    let mut updates = vec![(KEY, encoded.as_str())];
    if change.pause_checks {
        updates.push((YAML_DISCOVERY_ENABLED_KEY, "false"));
    }
    store
        .set_internal_values_checked(&updates)
        .map_err(|_| "Could not protect the connection operation.")?;
    if change.active_impacted {
        suspend_for_connection_change(store, worker)?;
    }
    Ok(())
}

pub(super) fn commit(store: &Store, change: &mut PendingChange) -> Result<(), &'static str> {
    let encoded =
        serde_json::to_string(&change.after).map_err(|_| "Could not save connection settings.")?;
    let labels = change.labels_after.encode()?;
    change.committed = true;
    let pending =
        serde_json::to_string(change).map_err(|_| "Could not save the connection operation.")?;
    let mut updates = vec![
        (CONNECTION_PROFILES_KEY, encoded.as_str()),
        (
            ACTIVE_CONNECTION_KEY,
            change.after.active_id.as_deref().unwrap_or(""),
        ),
        (display_numbers::KEY, labels.as_str()),
        (KEY, pending.as_str()),
    ];
    if change.pause_checks {
        updates.push((YAML_DISCOVERY_ENABLED_KEY, "false"));
    }
    if change.active_impacted {
        updates.push((AUTH_REJECTED_KEY, "false"));
    }
    if change.descriptor.kind == OperationKind::Save
        && change.revision == 0
        && change
            .after
            .profiles
            .get(&change.descriptor.connection_id)
            .is_some_and(|profile| profile.credential == Some(CredentialReference::LegacyTypeSafe))
    {
        updates.push((SAVED_KEY_KEY, "true"));
    }
    if store.set_internal_values_checked(&updates).is_err() {
        change.committed = false;
        return Err("Could not save connection settings. Use Retry in Settings → Checks.");
    }
    Ok(())
}

pub(super) struct ValidatedSwitchCredential {
    connection: SystemOneConnection,
    credential: Option<String>,
}

impl ValidatedSwitchCredential {
    pub(super) fn resolve(
        connection: &SystemOneConnection,
        vault: &impl CredentialVault,
    ) -> Result<Self, &'static str> {
        let credential = resolve_credential_at(
            connection,
            None,
            vault,
            CredentialReadStage::SwitchPreflight,
        )?;
        if connection_requires_credential(connection) && credential.is_none() {
            return Err(CredentialReadFailure::new(CredentialReadCategory::Missing)
                .report(CredentialReadStage::SwitchPreflight));
        }
        Ok(Self {
            connection: connection.clone(),
            credential,
        })
    }

    fn into_credential(
        self,
        connection: &SystemOneConnection,
    ) -> Result<Option<String>, &'static str> {
        if &self.connection != connection {
            return Err("The saved connection operation does not match the saved settings.");
        }
        Ok(self.credential)
    }
}

struct ValidatedRuntime {
    connection: SystemOneConnection,
    credential: Option<String>,
    enabled: bool,
}

impl ValidatedRuntime {
    fn resolve(
        store: &Store,
        saved: &ConnectionProfiles,
        vault: &impl CredentialVault,
        validated_switch: Option<ValidatedSwitchCredential>,
    ) -> Result<Option<Self>, &'static str> {
        let Some(id) = &saved.active_id else {
            if validated_switch.is_some() {
                return Err("The saved connection operation does not match the saved settings.");
            }
            return Ok(None);
        };
        let connection = saved
            .profiles
            .get(id)
            .cloned()
            .ok_or("The selected connection is missing.")?;
        connection
            .validate()
            .map_err(|_| "Saved connection settings are invalid.")?;
        let unconfigured_legacy = connection.credential
            == Some(CredentialReference::LegacyTypeSafe)
            && !saved_key_marker(store);
        let enabled = yaml_discovery_enabled(store)
            && store.internal_value(AUTH_REJECTED_KEY).as_deref() != Some("true")
            && !unconfigured_legacy;
        let prepared = validated_switch
            .map(|validated| validated.into_credential(&connection))
            .transpose()?;
        let credential = if enabled {
            let credential = match prepared {
                Some(credential) => credential,
                None => resolve_credential_at(
                    &connection,
                    None,
                    vault,
                    CredentialReadStage::RuntimeRestore,
                )?,
            };
            if connection_requires_credential(&connection) && credential.is_none() {
                return Err(
                    "The selected provider needs a credential. Retry after restoring secure storage.",
                );
            }
            credential
        } else {
            None
        };
        Ok(Some(Self {
            connection,
            credential,
            enabled,
        }))
    }

    fn install(self, worker: &WorkerHandle) {
        // The installer can fail only when validation of this immutable value fails.
        worker
            .install_system_one_connection(self.connection, self.credential, self.enabled)
            .expect("The runtime snapshot was validated before journal finalization");
    }
}

pub(super) struct Settled {
    pub result: OperationResult,
    change: PendingChange,
}

pub(super) fn settle(
    store: &Store,
    worker: &WorkerHandle,
    change: PendingChange,
    vault: &impl CredentialVault,
) -> Result<Settled, &'static str> {
    settle_with_startup_errors(store, worker, change, vault, &STARTUP_ERRORS, None)
}

pub(super) fn settle_switch(
    store: &Store,
    worker: &WorkerHandle,
    change: PendingChange,
    vault: &impl CredentialVault,
    validated: ValidatedSwitchCredential,
) -> Result<Settled, &'static str> {
    if change.descriptor.kind != OperationKind::Switch || !change.committed {
        return Err("The saved connection operation does not match the saved settings.");
    }
    settle_with_startup_errors(
        store,
        worker,
        change,
        vault,
        &STARTUP_ERRORS,
        Some(validated),
    )
}

fn settle_with_startup_errors(
    store: &Store,
    worker: &WorkerHandle,
    change: PendingChange,
    vault: &impl CredentialVault,
    startup_errors: &StartupErrors,
    validated_switch: Option<ValidatedSwitchCredential>,
) -> Result<Settled, &'static str> {
    let (saved, labels) = expected(&change);
    if &profiles(store)? != saved || DisplayNumbers::read(store)?.as_ref() != Some(labels) {
        return Err("The saved connection operation does not match the saved settings.");
    }
    if change.legacy_marker.is_some() {
        store
            .set_internal_value_checked(YAML_DISCOVERY_ENABLED_KEY, "false")
            .map_err(|_| "Could not pause legacy connection recovery.")?;
    }
    if change.active_impacted {
        suspend_for_connection_change(store, worker)?;
    }
    let cleanup = if change.committed {
        &change.delete_slot
    } else {
        &change.new_slot
    };
    if let Some(slot) = cleanup {
        delete_credential(vault, Some(slot))
            .map_err(|_| "Could not finish credential cleanup. Use Retry in Settings → Checks.")?;
    }
    let runtime = if change.active_impacted {
        ValidatedRuntime::resolve(store, saved, vault, validated_switch)?
    } else {
        None
    };
    let settings = snapshot(saved.clone(), labels, None)?;
    let mut updates = vec![(KEY, "")];
    if change.legacy_marker.is_some() {
        updates.push((CONNECTION_PENDING_KEY, ""));
    }
    store.set_internal_values_checked(&updates).map_err(
        |_| "Could not complete the connection operation. Use Retry in Settings → Checks.",
    )?;
    // Dispatch stays suspended until the checked final clear succeeds.
    if let Some(runtime) = runtime {
        runtime.install(worker);
    }
    if change.active_impacted {
        startup_errors.clear_resolved_connection(&change.descriptor.id, change.committed);
    }
    let outcome = if change.committed && change.legacy_marker.is_none() {
        OperationOutcome::Completed
    } else {
        OperationOutcome::RolledBack
    };
    Ok(Settled {
        result: OperationResult { settings, outcome },
        change,
    })
}

pub(super) fn emit_settled(
    settled: &Settled,
    mut emit: impl FnMut(crate::analytics::event::ModelCheckLifecycle),
) {
    use crate::analytics::event::{ModelCheckLifecycle, ProviderSetupOutcome};
    if settled.result.outcome == OperationOutcome::Completed {
        let provider = settled
            .change
            .provider
            .expect("Committed native operations retain their provider");
        let outcome = match settled.change.descriptor.kind {
            OperationKind::Save => ProviderSetupOutcome::Saved,
            OperationKind::Switch => ProviderSetupOutcome::Switched,
            OperationKind::Remove => ProviderSetupOutcome::ConnectionRemoved,
        };
        emit(ModelCheckLifecycle::ProviderSetup {
            provider: provider.into(),
            outcome,
        });
    }
    if settled.change.descriptor.kind == OperationKind::Remove && settled.change.disabled_checks {
        emit(ModelCheckLifecycle::Enablement { enabled: false });
    }
}

pub(super) fn retry_recording(
    store: &Store,
    worker: &WorkerHandle,
    id: &str,
    vault: &impl CredentialVault,
    emit: impl FnMut(crate::analytics::event::ModelCheckLifecycle),
) -> Result<OperationResult, &'static str> {
    retry_recording_with_startup_errors(store, worker, id, vault, emit, &STARTUP_ERRORS)
}

pub(super) fn retry_recording_with_startup_errors(
    store: &Store,
    worker: &WorkerHandle,
    id: &str,
    vault: &impl CredentialVault,
    emit: impl FnMut(crate::analytics::event::ModelCheckLifecycle),
    startup_errors: &StartupErrors,
) -> Result<OperationResult, &'static str> {
    let metadata = metadata(store)?;
    let change = metadata
        .pending
        .filter(|change| change.descriptor.id == id)
        .ok_or("This recovery operation is no longer pending. Refresh Settings.")?;
    let settled = settle_with_startup_errors(store, worker, change, vault, startup_errors, None)?;
    emit_settled(&settled, emit);
    Ok(settled.result)
}

pub(super) fn recover_connection_change(
    store: &Store,
    worker: &WorkerHandle,
    vault: &impl CredentialVault,
) -> Result<Option<SystemOneConnection>, &'static str> {
    let Some(change) = metadata(store)?.pending else {
        return Ok(None);
    };
    let settled = settle(store, worker, change, vault)?;
    if settled.change.active_impacted {
        return Ok(settled
            .result
            .settings
            .active_id
            .as_ref()
            .and_then(|id| settled.result.settings.profiles.get(id))
            .cloned());
    }
    Ok(None)
}
