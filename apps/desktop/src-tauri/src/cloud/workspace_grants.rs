//! Separate browser approvals and secure slots for explicitly selected teams.
//! Membership lists can start approval, but cannot restore proof or admit uploads.

use super::{
    AuthStatus, Controller, DestinationBinding, Workspace, auth, engine::Engine,
    ensure_cloud_setup, now, work::Priority,
};
use crate::store::Store;
use serde::Serialize;
use std::{collections::BTreeMap, sync::atomic::Ordering};
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

type Slot = (String, String);

/// Recovery for one exact team slot. Local reset does not prove remote revocation.
#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Recovery {
    pub local_reset_available: bool,
    pub remote_revocation_unconfirmed: bool,
}

impl Recovery {
    pub(super) fn read(engine: &Engine) -> Self {
        engine.workspace_recovery().unwrap_or(Self::UNKNOWN)
    }

    const UNKNOWN: Self = Self {
        local_reset_available: false,
        remote_revocation_unconfirmed: true,
    };
}

#[derive(Default)]
pub(super) struct WorkspaceGrants {
    engines: BTreeMap<Slot, Engine>,
    pending: Option<PendingApproval>,
}

struct PendingApproval {
    slot: Slot,
    generation: u64,
    expires_at: i64,
}

impl WorkspaceGrants {
    /// Read cached team authority only. This does not open a vault or refresh a grant.
    pub(super) fn cached_session(&self, account: &str, workspace: &str) -> Option<&auth::Session> {
        self.engines
            .get(&(account.to_owned(), workspace.to_owned()))?
            .session()
    }

    /// Read a registered slot without restoring access or changing its lifecycle.
    pub(super) fn recovery(&self, anchor: &Engine, identifier: &str, key: &Slot) -> Recovery {
        let cached = self.engines.get(key).filter(|engine| {
            engine
                .config
                .as_ref()
                .zip(anchor.config.as_ref())
                .is_some_and(|(team, personal)| {
                    team.issuer == personal.issuer && team.resource == personal.resource
                })
        });
        cached.map_or_else(
            || {
                Self::open_slot(identifier, anchor, key)
                    .map_or(Recovery::UNKNOWN, |slot| Recovery::read(&slot))
            },
            Recovery::read,
        )
    }

    fn clear_stale_approval(&mut self, generation: u64) {
        if self.pending.as_ref().is_some_and(|approval| {
            approval.generation != generation || approval.expires_at <= now()
        }) {
            self.cancel_pending(generation, |_, _| {});
        }
    }

    /// Restore only a registered slot for the exact requested workspace.
    pub(super) fn lookup(
        &mut self,
        store: &Store,
        identifier: &str,
        anchor: &Engine,
        account: &str,
        workspace: &str,
    ) -> Result<Option<&mut Engine>, String> {
        let config = anchor.config.as_ref().ok_or("Cloud is unavailable.")?;
        let key = (account.to_owned(), workspace.to_owned());
        let Some(session) = anchor
            .session()
            .filter(|session| session.user.id == account)
        else {
            return Ok(None);
        };
        if !store
            .active_cloud_workspace_slots(
                &config.issuer,
                &config.resource,
                account,
                &session.grant_id,
            )
            .map_err(|_| "Could not read saved workspace connections.")?
            .contains(&key.1)
        {
            return Ok(None);
        }
        self.slot(identifier, anchor, key).map(Some)
    }

    fn slot(
        &mut self,
        identifier: &str,
        anchor: &Engine,
        key: Slot,
    ) -> Result<&mut Engine, String> {
        use std::collections::btree_map::Entry;
        match self.engines.entry(key) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => {
                let engine = Self::open_slot(identifier, anchor, entry.key())?;
                Ok(entry.insert(engine))
            }
        }
    }

    fn open_slot(
        identifier: &str,
        anchor: &Engine,
        (account, id): &Slot,
    ) -> Result<Engine, String> {
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
            .map_err(|_| "Could not open the workspace connection.".into())
    }

    /// Reset the registered slot selected by the caller and publish its new state.
    pub(super) fn reset<R: tauri::Runtime>(
        &mut self,
        app: &tauri::AppHandle<R>,
        anchor: &Engine,
        key: Slot,
        current: impl Fn() -> bool,
    ) -> Result<(), String> {
        if !current() {
            return Err("The Cloud connection changed.".into());
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|approval| approval.slot == key)
        {
            return Err("Cancel this team's browser approval before resetting it.".into());
        }
        let slot = self.slot(&app.config().identifier, anchor, key.clone())?;
        slot.reset_local()
            .map_err(|_| "This team still has saved sign-out proof or is not ready to reset.")?;
        emit(app, &key, slot, &current);
        if !current() {
            return Err("The Cloud connection changed.".into());
        }
        Ok(())
    }

    /// Inject synthetic secure storage for a team slot test.
    #[cfg(test)]
    pub(super) fn install_test_slot(&mut self, key: Slot, engine: Engine, pending: bool) {
        if pending {
            let attempt = engine.attempt.as_ref().unwrap();
            self.pending = Some(PendingApproval {
                slot: key.clone(),
                generation: attempt.generation,
                expires_at: attempt.expires_at,
            });
        }
        self.engines.insert(key, engine);
    }

    /// Cancel the pending workspace only; established grants remain independent.
    pub(super) fn cancel_pending(&mut self, generation: u64, publish: impl FnOnce(&Slot, &Engine)) {
        let Some(approval) = self.pending.take() else {
            return;
        };
        let Some(engine) = self.engines.get_mut(&approval.slot) else {
            return;
        };
        if let Err(error) = engine.cancel(generation) {
            engine.status = error.into();
        }
        publish(&approval.slot, engine);
    }

    /// Explicit device sign-out closes all its registered team slots for this service.
    /// Leave uncertain slots indexed so another sign-out can retry cleanup.
    pub(super) fn close(
        &mut self,
        store: &Store,
        identifier: &str,
        anchor: &Engine,
    ) -> Result<(), String> {
        let Some(config) = &anchor.config else {
            return Ok(());
        };
        self.pending = None;
        let mut unconfirmed = false;
        for (account, workspace) in store
            .cloud_workspace_slots(&config.issuer, &config.resource)
            .map_err(|_| "Could not read saved workspace connections.")?
        {
            let key = (account, workspace);
            let slot = self.slot(identifier, anchor, key.clone())?;
            if slot.sign_out().is_err()
                || slot.cleanup.blocks_connection()
                || slot.cleanup.prior_unconfirmed()
            {
                unconfirmed = true;
                continue;
            }
            store
                .remove_cloud_workspace_slot(&config.issuer, &config.resource, &key.0, &key.1)
                .map_err(|_| "Could not close the saved workspace connection.")?;
            self.engines.remove(&key);
        }
        if unconfirmed {
            Err("Cloud could not confirm all workspace sign-outs.".into())
        } else {
            Ok(())
        }
    }
}

/// Send recovery only while the caller's Personal connection is still current.
pub(super) fn emit<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    slot: &Slot,
    engine: &Engine,
    current: impl Fn() -> bool,
) {
    let recovery = Recovery::read(engine);
    if !current() {
        return;
    }
    let payload = serde_json::json!({
        "accountId": slot.0, "workspaceId": slot.1, "status": engine.status,
        "localResetAvailable": recovery.local_reset_available,
        "remoteRevocationUnconfirmed": recovery.remote_revocation_unconfirmed,
    });
    for window in ["onboarding", "main", "settings"] {
        let _ = app.emit_to(window, "cloud:workspace-changed", &payload);
    }
}

/// Reset one saved team slot only after explicit user action. No membership or
/// grant is created, and a missing refresh proof never confirms remote sign-out.
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
        let mut grants = controller
            .workspace_grants
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        grants.clear_stale_approval(controller.attempt_generation.load(Ordering::Acquire));
        grants.reset(&app, &anchor, key, || {
            current()
                && anchor
                    .session()
                    .is_some_and(|active| active.same_grant(session))
        })?;
        super::emit_snapshot(&app, super::snapshot(&app, &anchor));
        Ok(AuthStatus::SignedOut)
    })
    .await
}

/// Start browser consent for a team chosen from the current bearer-only list.
/// The Personal grant is never overwritten, even when explicit approval fails.
#[tauri::command]
pub(crate) async fn cloud_connect_workspace(
    app: tauri::AppHandle,
    binding: DestinationBinding,
    workspace_id: String,
) -> Result<AuthStatus, String> {
    let controller = app.state::<Controller>();
    let account_generation = controller.account_generation.load(Ordering::Acquire);
    let expected = controller.attempt_generation.load(Ordering::Acquire);
    let work = controller.blocking.clone();
    work.run(Priority::Normal, move || {
        let controller = app.state::<Controller>();
        let generation = expected + 1;
        let current =
            || controller.account_generation.load(Ordering::Acquire) == account_generation;
        let mut anchor = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if anchor.generation != account_generation || !current() || anchor.attempt.is_some() {
            return Err("The Cloud connection changed. Try again.".into());
        }
        let verified = anchor
            .workspaces(&binding, &current)
            .map_err(|_| "Could not verify Cloud workspaces.")?
            .and_then(|workspaces| {
                workspaces.into_iter().find(|workspace| {
                    workspace.id == workspace_id && workspace.kind == auth::WorkspaceKind::Team
                })
            })
            .ok_or("This team workspace is not available.")?;
        let mut grants = controller
            .workspace_grants
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        let active_generation = controller.attempt_generation.load(Ordering::Acquire);
        if active_generation != expected {
            return Err("Cloud approval was cancelled.".into());
        }
        grants.clear_stale_approval(active_generation);
        if grants.pending.is_some() {
            return Err("Finish the current workspace approval first.".into());
        }
        controller
            .attempt_generation
            .compare_exchange(expected, generation, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "Cloud approval was cancelled.")?;
        let config = anchor.config.clone().ok_or("Cloud is unavailable.")?;
        let anchor_grant = anchor
            .session()
            .ok_or("The Cloud connection changed.")?
            .grant_id
            .clone();
        let store = app.state::<Store>();
        ensure_cloud_setup(&controller.setup_ready, &store)?;
        let key = (binding.account_id, verified.id);
        store
            .register_cloud_workspace_slot(&config.issuer, &config.resource, &key.0, &key.1)
            .map_err(|_| "Could not save the workspace connection request.")?;
        let engine = grants.slot(&app.config().identifier, &anchor, key.clone())?;
        engine
            .refresh(now(), current)
            .map_err(|_| "Could not restore the workspace connection.")?;
        if engine.session().is_some() {
            store
                .set_cloud_workspace_anchor(
                    &config.issuer,
                    &config.resource,
                    &key.0,
                    &key.1,
                    &anchor_grant,
                )
                .map_err(|_| "Could not activate the team workspace.")?;
            emit(&app, &key, engine, current);
            return Ok(AuthStatus::SignedIn);
        }
        let (url, listener, attempt) = engine.begin(now(), generation).map_err(
            |_| "Could not start workspace approval. Close a prior connection before retrying.",
        )?;
        if let Err(error) = app.opener().open_url(url, None::<&str>) {
            if super::browser_launch_did_not_start(&error) {
                engine.abort_browser_launch(&attempt)
            } else {
                engine.cancel(generation)
            }
            .map_err(|_| "Could not cancel workspace approval.")?;
            emit(&app, &key, engine, current);
            return Err("Could not open workspace approval.".into());
        }
        engine.status = AuthStatus::Waiting;
        grants.pending = Some(PendingApproval {
            slot: key.clone(),
            generation,
            expires_at: attempt.expires_at,
        });
        let callback_app = app.clone();
        tauri::async_runtime::spawn(async move {
            let active = || {
                let controller = callback_app.state::<Controller>();
                controller.account_generation.load(Ordering::Acquire) == account_generation
                    && controller.attempt_generation.load(Ordering::Acquire) == generation
            };
            let callback = listener.wait(&config, &attempt, active).await;
            let work = callback_app.state::<Controller>().blocking.clone();
            let _ = work
                .run(Priority::Cleanup, move || {
                    let controller = callback_app.state::<Controller>();
                    let current = || {
                        controller.account_generation.load(Ordering::Acquire) == account_generation
                            && controller.attempt_generation.load(Ordering::Acquire) == generation
                    };
                    let anchor = controller
                        .engine
                        .lock()
                        .map_err(|_| "Cloud is unavailable.")?;
                    let mut grants = controller
                        .workspace_grants
                        .lock()
                        .map_err(|_| "Cloud is unavailable.")?;
                    if !grants.pending.as_ref().is_some_and(|approval| {
                        approval.slot == key && approval.generation == generation
                    }) {
                        return Ok(());
                    }
                    grants.pending = None;
                    let Some(engine) = grants.engines.get_mut(&key) else {
                        return Ok(());
                    };
                    if !current()
                        || !anchor.session().is_some_and(|session| {
                            session.user.id == key.0 && session.grant_id == anchor_grant
                        })
                        || callback.is_none()
                    {
                        if let Err(error) = engine.cancel(generation) {
                            engine.status = error.into();
                        }
                    } else if let Some(callback) = callback
                        && let Err(error) = engine.callback(&attempt, callback, now(), current)
                    {
                        engine.status = error.into();
                    }
                    if current() && engine.session().is_some() {
                        let store = callback_app.state::<Store>();
                        if store
                            .set_cloud_workspace_anchor(
                                &config.issuer,
                                &config.resource,
                                &key.0,
                                &key.1,
                                &anchor_grant,
                            )
                            .is_err()
                        {
                            engine.status = AuthStatus::Unavailable;
                        }
                    }
                    emit(&callback_app, &key, engine, || {
                        controller.account_generation.load(Ordering::Acquire) == account_generation
                            && anchor.session().is_some_and(|session| {
                                session.user.id == key.0 && session.grant_id == anchor_grant
                            })
                    });
                    Ok(())
                })
                .await;
        });
        Ok(AuthStatus::Waiting)
    })
    .await
}
