//! Give the same-user CLI a read-only token for the current Desktop connection.
//! Full device proof and refresh stay in Desktop.

mod socket;

use super::{Controller, emit_snapshot, snapshot, work::Priority};
use socket::DeviceSession;
use std::{path::PathBuf, sync::atomic::Ordering};
use tauri::Manager;

/// Start the broker without delaying Desktop startup. If the socket is not safe
/// or cannot bind, the CLI stays local and Desktop Cloud still works.
pub(super) fn start(app: tauri::AppHandle, directory: PathBuf) {
    tauri::async_runtime::spawn(async move {
        let Ok(Ok((listener, owner))) =
            tokio::task::spawn_blocking(move || socket::bind(&directory)).await
        else {
            return;
        };
        let Ok(listener) = tokio::net::UnixListener::from_std(listener) else {
            return;
        };
        let _owner = owner;
        socket::serve(
            listener,
            {
                let app = app.clone();
                move || load_session(app.clone())
            },
            move |generation| account_generation(&app) == generation,
        )
        .await;
    });
}

fn account_generation(app: &tauri::AppHandle) -> u64 {
    app.state::<Controller>()
        .account_generation
        .load(Ordering::Acquire)
}

/// Fetch the read token in the existing Cloud worker. Carry the account
/// generation out so the socket can reject a late response.
async fn load_session(app: tauri::AppHandle) -> Option<(u64, DeviceSession)> {
    let generation = account_generation(&app);
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Normal, move || {
        let controller = app.state::<Controller>();
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        let current = || account_generation(&app) == generation;
        if !current() || engine.generation != generation {
            return Ok(None);
        }

        let result = engine.cli_device_session(&current);
        emit_snapshot(&app, snapshot(&app, &engine));
        if !current() || engine.session().is_none() {
            return Ok(None);
        }
        Ok(result
            .ok()
            .map(|(origin, read_token, personal_workspace_id)| {
                (
                    generation,
                    DeviceSession {
                        origin,
                        read_token,
                        personal_workspace_id,
                    },
                )
            }))
    })
    .await
    .ok()
    .flatten()
}
