use super::*;
use operation::{OperationKind, PendingChange, PendingOperation};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SaveConnectionIntent {
    Create,
    Update,
}

pub(super) use operation::ensure_no_pending as ensure_no_pending_change;
pub(super) use operation::{pending_affects_active, recover_connection_change};

fn referenced_slot(connection: &SystemOneConnection) -> Option<&str> {
    match &connection.credential {
        Some(CredentialReference::Connection(slot)) => Some(slot),
        _ => None,
    }
}

fn unused_local_slot(
    before: &ConnectionProfiles,
    after: &ConnectionProfiles,
    id: &str,
) -> Option<String> {
    let slot = referenced_slot(before.profiles.get(id)?)?;
    (!after
        .profiles
        .values()
        .any(|profile| referenced_slot(profile) == Some(slot)))
    .then(|| slot.to_owned())
}

fn identity_in_use(saved: &ConnectionProfiles, id: &str) -> bool {
    saved.profiles.contains_key(id)
        || saved.retired_ids.contains(id)
        || saved
            .profiles
            .values()
            .any(|profile| referenced_slot(profile) == Some(id))
}

fn prepare_draft(
    saved: &ConnectionProfiles,
    draft: &mut SystemOneDraft,
    intent: SaveConnectionIntent,
) -> Result<(), &'static str> {
    validate_connection_id(&draft.connection_id)?;
    draft.connection.context_override = None;
    match intent {
        SaveConnectionIntent::Create => {
            if identity_in_use(saved, &draft.connection_id) {
                return Err(
                    "This connection identity was already used. Open Add connection again.",
                );
            }
            draft.connection.revision = 1;
            if draft.connection.credential == Some(CredentialReference::LegacyTypeSafe)
                && draft.connection.provider != SystemOneProvider::Jev
                && draft.credential.is_none()
            {
                return Err("The saved TypeSafe key can be selected only for Jev.");
            }
        }
        SaveConnectionIntent::Update => {
            let previous = saved
                .profiles
                .get(&draft.connection_id)
                .ok_or("This connection was removed. Close the editor and refresh Settings.")?;
            if previous.revision != draft.connection.revision {
                return Err(
                    "This connection changed in another window. Close the editor and refresh Settings.",
                );
            }
            if previous.provider != draft.connection.provider {
                return Err("Add a new connection to use a different provider.");
            }
            if draft.credential.is_none() {
                if draft.connection.credential.is_none() {
                    draft.connection.credential.clone_from(&previous.credential);
                } else if draft.connection.credential != previous.credential {
                    return Err(
                        "The saved credential reference changed. Close the editor and refresh Settings.",
                    );
                }
            }
        }
    }
    draft
        .connection
        .validate()
        .map_err(|_| "Connection settings are invalid.")?;
    if connection_requires_credential(&draft.connection)
        && draft.connection.credential.is_none()
        && draft.credential.is_none()
    {
        return Err("The selected provider needs a credential. Add one and retry.");
    }
    Ok(())
}

fn new_change(
    metadata: operation::Metadata,
    id: &str,
    kind: OperationKind,
    request_id: Option<String>,
    provider: SystemOneProvider,
) -> Result<PendingChange, &'static str> {
    Ok(PendingChange {
        descriptor: PendingOperation {
            id: operation::fresh_id("operation-")?,
            kind,
            connection_id: id.to_owned(),
            request_id,
        },
        revision: metadata
            .profiles
            .profiles
            .get(id)
            .map_or(0, |profile| profile.revision),
        after: metadata.profiles.clone(),
        before: metadata.profiles,
        labels_after: metadata.labels.clone(),
        labels_before: metadata.labels,
        new_slot: None,
        delete_slot: None,
        active_impacted: false,
        pause_checks: false,
        disabled_checks: false,
        provider: Some(provider),
        committed: false,
        legacy_marker: None,
    })
}

fn writable_metadata(store: &Store) -> Result<operation::Metadata, &'static str> {
    let metadata = operation::metadata(store)?;
    ensure_no_pending_credential_change(store)?;
    Ok(metadata)
}

fn save_operation(
    store: &Store,
    worker: &WorkerHandle,
    mut draft: SystemOneDraft,
    intent: SaveConnectionIntent,
    request_id: &str,
    vault: &impl CredentialVault,
) -> Result<Option<operation::Settled>, &'static str> {
    operation::validate_request_id(request_id)?;
    let metadata = writable_metadata(store)?;
    prepare_draft(&metadata.profiles, &mut draft, intent)?;
    let previous = metadata.profiles.profiles.get(&draft.connection_id);
    if previous == Some(&draft.connection) && draft.credential.is_none() {
        return Ok(None);
    }
    let selected = metadata.profiles.active_id.as_deref() == Some(&draft.connection_id);
    if selected || intent == SaveConnectionIntent::Create || draft.credential.is_some() {
        let credential = resolve_credential(&draft.connection, draft.credential.clone(), vault)?;
        if connection_requires_credential(&draft.connection) && credential.is_none() {
            return Err("The selected provider needs a credential. Add one and retry.");
        }
    }
    if intent == SaveConnectionIntent::Update {
        draft.connection.revision = next_connection_revision(draft.connection.revision)?;
    }
    let first = metadata.profiles.profiles.is_empty();
    let mut change = new_change(
        metadata,
        &draft.connection_id,
        OperationKind::Save,
        Some(request_id.to_owned()),
        draft.connection.provider,
    )?;
    if intent == SaveConnectionIntent::Create {
        change
            .labels_after
            .allocate(&draft.connection_id, draft.connection.provider)?;
    }
    if let Some(slot) = change
        .before
        .profiles
        .get(&draft.connection_id)
        .and_then(referenced_slot)
    {
        change.after.retired_ids.insert(slot.to_owned());
    }
    if draft.credential.is_some() {
        let slot = loop {
            let slot = operation::fresh_id("credential-")?;
            if !identity_in_use(&change.before, &slot) {
                break slot;
            }
        };
        change.after.retired_ids.insert(slot.clone());
        draft.connection.credential = Some(CredentialReference::Connection(slot.clone()));
        change.new_slot = Some(slot);
    }
    if first {
        change.after.active_id = Some(draft.connection_id.clone());
    }
    change
        .after
        .profiles
        .insert(draft.connection_id.clone(), draft.connection);
    change.delete_slot = unused_local_slot(&change.before, &change.after, &draft.connection_id);
    change.active_impacted = selected || first;
    change.pause_checks = first;
    operation::begin(store, worker, &change)?;
    if let (Some(slot), Some(secret)) = (&change.new_slot, &draft.credential) {
        write_credential(vault, Some(slot), secret)?;
    }
    operation::commit(store, &mut change)?;
    operation::settle(store, worker, change, vault).map(Some)
}

#[cfg(test)]
pub(super) fn save_connection_with_intent(
    store: &Store,
    worker: &WorkerHandle,
    draft: SystemOneDraft,
    intent: SaveConnectionIntent,
    request_id: &str,
    vault: &impl CredentialVault,
) -> Result<(SystemOneSettings, bool), &'static str> {
    match save_operation(store, worker, draft, intent, request_id, vault)? {
        Some(settled) => Ok((settled.result.settings, true)),
        None => Ok((operation::settings_snapshot(store)?, false)),
    }
}

pub(super) fn save_connection_recording(
    store: &Store,
    worker: &WorkerHandle,
    draft: SystemOneDraft,
    intent: SaveConnectionIntent,
    request_id: &str,
    vault: &impl CredentialVault,
    emit: impl FnMut(crate::analytics::event::ModelCheckLifecycle),
) -> Result<SystemOneSettings, &'static str> {
    match save_operation(store, worker, draft, intent, request_id, vault)? {
        Some(settled) => {
            operation::emit_settled(&settled, emit);
            Ok(settled.result.settings)
        }
        None => operation::settings_snapshot(store),
    }
}

fn switch_operation(
    store: &Store,
    worker: &WorkerHandle,
    id: &str,
    vault: &impl CredentialVault,
) -> Result<Option<operation::Settled>, &'static str> {
    let metadata = writable_metadata(store)?;
    if metadata.profiles.active_id.as_deref() == Some(id) {
        return Ok(None);
    }
    let connection = metadata
        .profiles
        .profiles
        .get(id)
        .ok_or("The saved connection is missing.")?;
    let credential = operation::ValidatedSwitchCredential::resolve(connection, vault)?;
    let provider = connection.provider;
    let mut change = new_change(metadata, id, OperationKind::Switch, None, provider)?;
    change.after.active_id = Some(id.to_owned());
    change.active_impacted = true;
    operation::begin(store, worker, &change)?;
    operation::commit(store, &mut change)?;
    operation::settle_switch(store, worker, change, vault, credential).map(Some)
}

#[cfg(test)]
pub(super) fn switch_connection(
    store: &Store,
    worker: &WorkerHandle,
    id: &str,
    vault: &impl CredentialVault,
) -> Result<(SystemOneSettings, bool), &'static str> {
    match switch_operation(store, worker, id, vault)? {
        Some(settled) => Ok((settled.result.settings, true)),
        None => Ok((operation::settings_snapshot(store)?, false)),
    }
}

pub(super) fn switch_connection_recording(
    store: &Store,
    worker: &WorkerHandle,
    id: &str,
    vault: &impl CredentialVault,
    emit: impl FnMut(crate::analytics::event::ModelCheckLifecycle),
) -> Result<SystemOneSettings, &'static str> {
    match switch_operation(store, worker, id, vault)? {
        Some(settled) => {
            operation::emit_settled(&settled, emit);
            Ok(settled.result.settings)
        }
        None => operation::settings_snapshot(store),
    }
}

fn remove_operation(
    store: &Store,
    worker: &WorkerHandle,
    id: &str,
    revision: u64,
    vault: &impl CredentialVault,
) -> Result<operation::Settled, &'static str> {
    let metadata = writable_metadata(store)?;
    let profile = metadata
        .profiles
        .profiles
        .get(id)
        .ok_or("The saved connection is missing.")?;
    if profile.revision != revision {
        return Err(
            "This connection changed in another window. Refresh Settings before removing it.",
        );
    }
    let provider = profile.provider;
    let mut change = new_change(metadata, id, OperationKind::Remove, None, provider)?;
    change.after.profiles.remove(id);
    change.after.retired_ids.insert(id.to_owned());
    if let Some(slot) = change.before.profiles.get(id).and_then(referenced_slot) {
        change.after.retired_ids.insert(slot.to_owned());
    }
    change.active_impacted = change.before.active_id.as_deref() == Some(id);
    change.pause_checks = change.active_impacted;
    change.disabled_checks = change.active_impacted && yaml_discovery_enabled(store);
    if change.active_impacted {
        change.after.active_id = None;
    }
    change.delete_slot = unused_local_slot(&change.before, &change.after, id);
    operation::begin(store, worker, &change)?;
    operation::commit(store, &mut change)?;
    operation::settle(store, worker, change, vault)
}

#[cfg(test)]
pub(super) fn remove_connection(
    store: &Store,
    worker: &WorkerHandle,
    id: &str,
    revision: u64,
    vault: &impl CredentialVault,
) -> Result<SystemOneSettings, &'static str> {
    remove_operation(store, worker, id, revision, vault).map(|settled| settled.result.settings)
}

pub(super) fn remove_connection_recording(
    store: &Store,
    worker: &WorkerHandle,
    id: &str,
    revision: u64,
    vault: &impl CredentialVault,
    emit: impl FnMut(crate::analytics::event::ModelCheckLifecycle),
) -> Result<SystemOneSettings, &'static str> {
    let settled = remove_operation(store, worker, id, revision, vault)?;
    operation::emit_settled(&settled, emit);
    Ok(settled.result.settings)
}
