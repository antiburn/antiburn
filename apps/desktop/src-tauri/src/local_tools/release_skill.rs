//! Install the canonical Claude Code skill without running an agent or the CLI.

use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager;

use super::Reason;
use super::installation::{
    FileReceipt, SKILL, check_owned, checked_directory, ensure_directory, present, publish,
    read_optional, render_skill,
};
use crate::agent_integrations::transaction;
use crate::cli_install::CliRuntime;

const RELEASE_IDENTIFIER: &str = "ai.antiburn.desktop";
const OWNER: &str = "antiburn-release-skill-v1";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    owner: String,
    profile: PathBuf,
    skill: FileReceipt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    first_publication: Option<FirstPublication>,
}

/// Bind the pending receipt to the staged file. The token rules out inode
/// reuse, and the device and inode rule out a copy that preserves the token.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FirstPublication {
    device: u64,
    inode: u64,
    token: [u8; 16],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stage: Option<String>,
}

impl FirstPublication {
    const ATTRIBUTE: &std::ffi::CStr = c"ai.antiburn.desktop.skill-publication";

    fn mark(file: &File) -> transaction::Result<Self> {
        let mut token = [0; 16];
        getrandom::fill(&mut token).map_err(|_| "random_unavailable")?;
        let metadata = file.metadata().map_err(|_| "config_unreadable")?;
        // Set the marker on the open stage, not on a path that can change.
        if unsafe {
            libc::fsetxattr(
                file.as_raw_fd(),
                Self::ATTRIBUTE.as_ptr(),
                token.as_ptr().cast(),
                token.len(),
                0,
                libc::XATTR_CREATE,
            )
        } != 0
        {
            return Err("stage_marker_unavailable");
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            token,
            stage: None,
        })
    }

    fn stage_path(&self, path: &Path) -> Option<PathBuf> {
        let name = self.stage.as_deref()?;
        let nonce = name.strip_prefix(".antiburn-")?.strip_suffix(".tmp")?;
        if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        Some(path.with_file_name(name))
    }

    fn matches(&self, path: &Path) -> bool {
        let Ok(file) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        else {
            return false;
        };
        let Ok(metadata) = file.metadata() else {
            return false;
        };
        if !metadata.is_file()
            || metadata.nlink() != 1
            || (metadata.dev(), metadata.ino()) != (self.device, self.inode)
        {
            return false;
        }
        let mut token = [0u8; 16];
        let len = unsafe {
            libc::fgetxattr(
                file.as_raw_fd(),
                Self::ATTRIBUTE.as_ptr(),
                token.as_mut_ptr().cast(),
                token.len(),
                0,
                0,
            )
        };
        len == token.len() as isize && token == self.token
    }
}

fn automatic_installation(distribution: bool, debug: bool, identifier: &str) -> bool {
    distribution && !debug && identifier == RELEASE_IDENTIFIER
}

/// Only the macOS release distribution owns the default Claude Code skill.
pub(crate) fn enabled(app: &tauri::AppHandle) -> bool {
    automatic_installation(
        cfg!(feature = "distribution"),
        cfg!(debug_assertions),
        &app.config().identifier,
    )
}

/// Run on the CLI worker after staging. The caller holds the runtime lock.
pub(crate) fn install(app: &tauri::AppHandle, runtime: &CliRuntime) {
    let result = (|| {
        let data = app
            .path()
            .app_data_dir()
            .map_err(|_| Reason::SetupUnavailable)?;
        let home = app
            .path()
            .home_dir()
            .map_err(|_| Reason::ProfileUnavailable)?;
        reconcile_bound(&data, &home, runtime)
    })();
    if let Err(reason) = result {
        tracing::warn!(event = "release_skill_installation_unavailable", ?reason);
    }
}

fn reconcile_bound(data: &Path, home: &Path, runtime: &CliRuntime) -> Result<(), Reason> {
    if runtime.executable != data.join("cli/bin").join(antiburn_cli_runtime::file_name()) {
        return Err(Reason::SetupUnavailable);
    }
    let proposed = render_skill(SKILL, &runtime.executable, None)?;
    reconcile(data, home, &proposed)
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
            first_publication: None,
        }
    };
    let mut current = read_optional(&skill_path)?;
    if receipt.skill.active.is_none() {
        if current.is_none()
            && let Some(proof) = &receipt.first_publication
        {
            // If neither path exists, the user may have removed the published skill.
            let stage = proof
                .stage_path(&skill_path)
                .ok_or(Reason::OwnedFileChanged)?;
            current = Some(publish_first(
                &stage,
                &skill_path,
                receipt
                    .skill
                    .pending
                    .as_deref()
                    .ok_or(Reason::OwnershipConflict)?,
                proof,
            )?);
        }
        if let Some(installed) = current.as_deref() {
            finish_first_publication(&skill_path, installed, &mut receipt)?;
            if installed == proposed {
                return save(&receipt_path, &receipt, &mut receipt_bytes);
            }
        }
    }
    check_owned(current.as_deref(), &receipt.skill)?;
    if current.as_deref() == Some(proposed) && receipt.skill.pending.is_none() {
        return Ok(());
    }
    receipt
        .skill
        .prepare_replacement(current.as_deref(), proposed.to_vec());
    receipt.first_publication = None;
    save(&receipt_path, &receipt, &mut receipt_bytes)?;
    for path in [&receipt.profile, &skills, &directory] {
        ensure_directory(path)?;
    }
    if current.is_none() {
        let (stage, proof) = stage_first(&skill_path, proposed)?;
        receipt.first_publication = Some(proof);
        save(&receipt_path, &receipt, &mut receipt_bytes)?;
        let installed = publish_first(
            &stage,
            &skill_path,
            proposed,
            receipt
                .first_publication
                .as_ref()
                .ok_or(Reason::OwnershipConflict)?,
        )?;
        finish_first_publication(&skill_path, &installed, &mut receipt)?;
    } else {
        if current.as_deref() != Some(proposed) {
            publish(&skill_path, current.as_deref(), proposed)?;
        }
        receipt.skill.active = receipt.skill.pending.take();
    }
    save(&receipt_path, &receipt, &mut receipt_bytes)
}

/// Create an initial receipt without leaving a second link after a crash.
fn save(path: &Path, receipt: &Receipt, previous: &mut Option<Vec<u8>>) -> Result<(), Reason> {
    if previous.is_some() {
        return super::installation::save(path, receipt, previous);
    }
    let bytes = serde_json::to_vec(receipt).map_err(|_| Reason::SetupUnavailable)?;
    let (stage, ()) =
        transaction::stage_new(path, &bytes, |_| Ok(())).map_err(|_| Reason::SetupUnavailable)?;
    transaction::publish_staged_new(&stage, path, |current| current == bytes)
        .map_err(|_| Reason::SetupUnavailable)?;
    *previous = Some(bytes);
    Ok(())
}

/// Record the stage name and identity before the skill becomes visible.
fn stage_first(path: &Path, proposed: &[u8]) -> Result<(PathBuf, FirstPublication), Reason> {
    let (stage, mut proof) = transaction::stage_new(path, proposed, FirstPublication::mark)
        .map_err(|_| Reason::SetupUnavailable)?;
    proof.stage = Some(
        stage
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(Reason::SetupUnavailable)?
            .into(),
    );
    Ok((stage, proof))
}

/// A retry can publish only the unchanged staged file from the receipt.
fn publish_first(
    stage: &Path,
    path: &Path,
    pending: &[u8],
    proof: &FirstPublication,
) -> Result<Vec<u8>, Reason> {
    if !present(stage)? {
        return Err(Reason::OwnedFileChanged);
    }
    transaction::publish_staged_new(stage, path, |current| {
        current == pending && proof.matches(stage)
    })
    .map_err(|error| match error {
        "unsafe_config_file" => Reason::OwnedFileChanged,
        "concurrent_config_change" => Reason::OwnershipConflict,
        _ => Reason::SetupUnavailable,
    })?;
    read_optional(path)?.ok_or(Reason::OwnedFileChanged)
}

/// Promote pending bytes only when the current file is the exact staged file.
fn finish_first_publication(
    path: &Path,
    current: &[u8],
    receipt: &mut Receipt,
) -> Result<(), Reason> {
    if receipt.skill.pending.as_deref() != Some(current)
        || !receipt
            .first_publication
            .as_ref()
            .is_some_and(|proof| proof.matches(path))
    {
        return Err(Reason::OwnershipConflict);
    }
    receipt.skill.active = receipt.skill.pending.take();
    receipt.first_publication = None;
    Ok(())
}

#[cfg(test)]
#[path = "tests/release_skill_tests.rs"]
mod tests;
