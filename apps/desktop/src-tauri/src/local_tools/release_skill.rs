//! Install the canonical Claude Code skill without running an agent or the CLI.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager;

use super::Reason;
use super::installation::{
    FileReceipt, SKILL, check_owned, checked_directory, ensure_directory, present, publish,
    read_optional, save,
};
use crate::agent_integrations::transaction;

const RELEASE_IDENTIFIER: &str = "ai.antiburn.desktop";
const OWNER: &str = "antiburn-release-skill-v1";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    owner: String,
    profile: PathBuf,
    skill: FileReceipt,
}

fn automatic_installation(distribution: bool, debug: bool, identifier: &str) -> bool {
    distribution && !debug && identifier == RELEASE_IDENTIFIER
}

/// Run once per release launch, separate from CLI setup and debug tool status.
pub(super) fn start(app: &tauri::AppHandle) {
    if !automatic_installation(
        cfg!(feature = "distribution"),
        cfg!(debug_assertions),
        &app.config().identifier,
    ) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| {
            let data = app
                .path()
                .app_data_dir()
                .map_err(|_| Reason::SetupUnavailable)?;
            let home = app
                .path()
                .home_dir()
                .map_err(|_| Reason::ProfileUnavailable)?;
            reconcile(&data, &home, SKILL.as_bytes())
        })();
        if let Err(reason) = result {
            tracing::warn!(event = "release_skill_installation_unavailable", ?reason);
        }
    });
}

/// Replace only the exact bytes in the receipt. A pending write can resume on
/// the next launch. A removed or changed managed file stays untouched.
fn reconcile(data: &Path, home: &Path, proposed: &[u8]) -> Result<(), Reason> {
    if data.file_name().and_then(|name| name.to_str()) != Some(RELEASE_IDENTIFIER) {
        return Err(Reason::SetupUnavailable);
    }
    checked_directory(data).map_err(|_| Reason::SetupUnavailable)?;
    let state = data.join("skill-install");
    ensure_directory(&state).map_err(|_| Reason::SetupUnavailable)?;
    let _lock = transaction::Lock::acquire(&state).map_err(|_| Reason::SetupUnavailable)?;

    checked_directory(home)?;
    let profile = home.join(".claude");
    let skills = profile.join("skills");
    let directory = skills.join("antiburn");
    for path in [&profile, &skills, &directory] {
        if present(path)? {
            checked_directory(path)?;
        }
    }
    let skill_path = directory.join("SKILL.md");
    let receipt_path = state.join("receipt.json");
    let mut receipt_bytes = read_optional(&receipt_path)?;
    let mut receipt = if let Some(bytes) = &receipt_bytes {
        let receipt: Receipt =
            serde_json::from_slice(bytes).map_err(|_| Reason::OwnershipConflict)?;
        if receipt.owner != OWNER {
            return Err(Reason::OwnershipConflict);
        }
        if receipt.profile != profile {
            return Err(Reason::ProfileChanged);
        }
        receipt
    } else {
        if present(&directory)? {
            return Err(Reason::OwnershipConflict);
        }
        Receipt {
            owner: OWNER.into(),
            profile,
            skill: FileReceipt {
                active: None,
                pending: None,
            },
        }
    };
    let current = read_optional(&skill_path)?;
    if receipt.skill.active.is_none() && current.is_some() {
        // Pending bytes do not prove who created a file during first install.
        return Err(Reason::OwnershipConflict);
    }
    check_owned(current.as_deref(), &receipt.skill)?;
    if current.as_deref() == Some(proposed) && receipt.skill.pending.is_none() {
        return Ok(());
    }
    receipt
        .skill
        .prepare_replacement(current.as_deref(), proposed.to_vec());
    save(&receipt_path, &receipt, &mut receipt_bytes)?;
    for path in [&receipt.profile, &skills, &directory] {
        ensure_directory(path)?;
    }
    if current.as_deref() != Some(proposed) {
        publish(&skill_path, current.as_deref(), proposed)?;
    }
    receipt.skill.active = Some(proposed.to_vec());
    receipt.skill.pending = None;
    save(&receipt_path, &receipt, &mut receipt_bytes)
}

#[cfg(test)]
#[path = "tests/release_skill_tests.rs"]
mod tests;
