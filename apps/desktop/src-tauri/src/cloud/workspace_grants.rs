//! Recovery and revocation for old Team slots. Never restore their access.

use super::{
    AuthStatus, Controller, DestinationBinding, Workspace, auth, engine::Engine,
    ensure_cloud_setup, now, work::Priority,
};
use crate::store::Store;
use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager};

type Slot = (String, String);

/// Local reset does not prove remote revocation.
#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Recovery {
    pub local_reset_available: bool,
    pub remote_revocation_unconfirmed: bool,
}

impl Recovery {
    fn read(engine: &Engine) -> Self {
        engine.workspace_recovery().unwrap_or(Self::UNKNOWN)
    }

    const UNKNOWN: Self = Self {
        local_reset_available: false,
        remote_revocation_unconfirmed: true,
    };
}

fn open_slot(identifier: &str, anchor: &Engine, (account, id): &Slot) -> Result<Engine, String> {
    anchor
        .workspace_engine(
            identifier.into(),
            account.clone(),
            Workspace {
                id: id.clone(),
                name: "Team workspace".into(),
                kind: auth::WorkspaceKind::Team,
            },
        )
        .map_err(|_| "Could not open the saved workspace connection.".into())
}

/// Read only a registered slot's cleanup state for the configured origins.
pub(super) fn recovery(anchor: &Engine, identifier: &str, key: &Slot) -> Recovery {
    open_slot(identifier, anchor, key).map_or(Recovery::UNKNOWN, |slot| Recovery::read(&slot))
}

/// Close old registered slots for this service without waiting for Cloud.
/// Keep uncertain slots indexed so Settings can report them.
pub(super) fn close(store: &Store, identifier: &str, anchor: &Engine) -> Result<(), String> {
    let Some(config) = &anchor.config else {
        return Ok(());
    };
    let mut local_cleanup_failed = false;
    for key in store
        .cloud_workspace_slots(&config.issuer, &config.resource)
        .map_err(|_| "Could not read saved workspace connections.")?
    {
        let mut slot = open_slot(identifier, anchor, &key)?;
        match slot.sign_out() {
            Ok(Some(proof)) => {
                slot.try_remote_logout(proof);
            }
            Ok(None) => {}
            Err(_) => {
                local_cleanup_failed = true;
                continue;
            }
        }
        if slot.cleanup.blocks_connection() {
            local_cleanup_failed = true;
            continue;
        }
        if slot.cleanup.prior_unconfirmed() {
            continue;
        }
        store
            .remove_cloud_workspace_slot(&config.issuer, &config.resource, &key.0, &key.1)
            .map_err(|_| "Could not close the saved workspace connection.")?;
    }
    if local_cleanup_failed {
        Err("Desktop could not close all saved workspace connections.".into())
    } else {
        Ok(())
    }
}

/// Reset old local state only after explicit action and for an exact registered
/// slot. This does not change a folder policy or create any access.
#[tauri::command]
pub(crate) async fn cloud_reset_workspace(
    app: tauri::AppHandle,
    binding: DestinationBinding,
    workspace_id: String,
) -> Result<AuthStatus, String> {
    let controller = app.state::<Controller>();
    let generation = controller.account_generation.load(Ordering::Acquire);
    let work = controller.blocking.clone();
    work.run(Priority::Cleanup, move || {
        let controller = app.state::<Controller>();
        let current = || controller.account_generation.load(Ordering::Acquire) == generation;
        let mut anchor = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if anchor.generation != generation || !current() {
            return Err("The Cloud connection changed.".into());
        }
        anchor
            .refresh(now(), current)
            .map_err(|_| "Connect to Cloud before resetting a team.")?;
        let session = anchor
            .session()
            .filter(|session| binding.matches(session))
            .ok_or("The Cloud connection changed.")?;
        let key = (binding.account_id, workspace_id);
        let store = app.state::<Store>();
        ensure_cloud_setup(&controller.setup_ready, &store)?;
        if !store
            .cloud_workspace_slots(&session.iss, &session.resource)
            .map_err(|_| "Could not read saved workspace connections.")?
            .contains(&key)
        {
            return Err("This team has no saved connection on this device.".into());
        }
        let mut slot = open_slot(&app.config().identifier, &anchor, &key)?;
        if !current() {
            return Err("The Cloud connection changed.".into());
        }
        slot.reset_local()
            .map_err(|_| "This team still has saved sign-out proof or is not ready to reset.")?;
        let recovery = Recovery::read(&slot);
        if !current() {
            return Err("The Cloud connection changed.".into());
        }
        let payload = serde_json::json!({
            "accountId": key.0, "workspaceId": key.1, "status": slot.status,
            "localResetAvailable": recovery.local_reset_available,
            "remoteRevocationUnconfirmed": recovery.remote_revocation_unconfirmed,
        });
        for window in ["onboarding", "main", "settings"] {
            let _ = app.emit_to(window, "cloud:workspace-changed", &payload);
        }
        super::emit_snapshot(&app, super::snapshot(&app, &anchor));
        Ok(AuthStatus::SignedOut)
    })
    .await
}
