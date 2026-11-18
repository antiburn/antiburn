//! Receipt-owned debug config and a generated copy of the canonical CLI skill.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::agent_integrations::{Vendor, discovery, transaction};
use crate::cli_install::CliRuntime;

use super::{DEBUG_IDENTIFIER, Reason};

const OWNER: &str = "antiburn-debug-local-tools-v1";
const SKILL: &str = include_str!("../../../../cli/skills/antiburn/SKILL.md");
const COMMAND: &str = "antiburn checks <session_id>";

type Result<T> = std::result::Result<T, Reason>;

/// All destinations derive from the active debug app and selected OpenCode root.
pub(super) struct Installation {
    pub data: PathBuf,
    pub profile: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileReceipt {
    active: Option<Vec<u8>>,
    pending: Option<Vec<u8>>,
}

impl FileReceipt {
    /// Keep installed write-ahead bytes authorized through an interrupted upgrade.
    fn prepare_replacement(&mut self, current: Option<&[u8]>, proposed: Vec<u8>) {
        if current.is_some() && current == self.pending.as_deref() {
            self.active = current.map(<[u8]>::to_vec);
        }
        self.pending = Some(proposed);
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    schema: u32,
    owner: String,
    profile: PathBuf,
    config: FileReceipt,
    skill: FileReceipt,
}

impl Installation {
    /// Resolve the process-selected profile without creating an OpenCode profile.
    pub(super) fn for_app(app: &tauri::AppHandle) -> Result<Self> {
        let data = app
            .path()
            .app_data_dir()
            .map_err(|_| Reason::SetupUnavailable)?;
        let home = app
            .path()
            .home_dir()
            .map_err(|_| Reason::ProfileUnavailable)?
            .canonicalize()
            .map_err(|_| Reason::ProfileUnavailable)?;
        let env = profile_environment(|key| std::env::var_os(key))?;
        let profile = discovery::profile(Vendor::OpenCode, &home, &env, None)
            .map_err(|_| Reason::ProfileUnavailable)?
            .root;
        Ok(Self { data, profile })
    }

    fn tools(&self) -> PathBuf {
        self.data.join("cli/tools")
    }

    fn skill_directory(&self) -> PathBuf {
        self.profile.join("skills/antiburn-debug")
    }

    fn check_binding(&self, runtime: &CliRuntime) -> Result<()> {
        if self.data.file_name().and_then(|name| name.to_str()) != Some(DEBUG_IDENTIFIER)
            || runtime.executable
                != self
                    .data
                    .join("cli/bin")
                    .join(antiburn_cli_runtime::file_name())
        {
            return Err(Reason::SetupUnavailable);
        }
        checked_directory(&self.data).map_err(|_| Reason::SetupUnavailable)?;
        checked_directory(&self.data.join("cli")).map_err(|_| Reason::SetupUnavailable)?;
        checked_directory(&self.profile).map_err(|_| Reason::ProfileUnavailable)
    }

    fn config(&self) -> Result<Vec<u8>> {
        serde_json::to_vec_pretty(&serde_json::json!({
            "data_root": self.data.parent().ok_or(Reason::SetupUnavailable)?,
            "app_identifier": DEBUG_IDENTIFIER,
            "database_filename": "antiburn-debug.sqlite3",
            "state_file": self.tools().join("state.json"),
            "log_file": self.tools().join("calls.log"),
            "pricing_snapshot_path": self.data.join("model-pricing.json"),
        }))
        .map_err(|_| Reason::SetupUnavailable)
    }

    fn skill(&self, runtime: &CliRuntime) -> Result<Vec<u8>> {
        if SKILL.matches(COMMAND).count() != 1 || SKILL.matches("name: antiburn\n").count() != 1 {
            return Err(Reason::SetupUnavailable);
        }
        let command = format!(
            "{} --config {} checks <session_id>",
            quote(&runtime.executable)?,
            quote(&self.tools().join("config.json"))?,
        );
        Ok(SKILL
            .replacen("name: antiburn\n", "name: antiburn-debug\n", 1)
            .replacen(COMMAND, &command, 1)
            .into_bytes())
    }

    fn receipt(&self) -> Result<Option<(Receipt, Vec<u8>)>> {
        let Some(bytes) = read_optional(&self.tools().join("receipt.json"))? else {
            return Ok(None);
        };
        let receipt: Receipt =
            serde_json::from_slice(&bytes).map_err(|_| Reason::OwnershipConflict)?;
        if receipt.schema != 1 || receipt.owner != OWNER {
            return Err(Reason::OwnershipConflict);
        }
        if receipt.profile != self.profile {
            return Err(Reason::ProfileChanged);
        }
        Ok(Some((receipt, bytes)))
    }

    /// Verify exact owned files without repairing removed or modified files.
    pub(super) fn inspect(&self, runtime: &CliRuntime) -> Result<()> {
        self.check_binding(runtime)?;
        let (receipt, _) = self.receipt()?.ok_or(Reason::OwnershipConflict)?;
        let config = self.config()?;
        let skill = self.skill(runtime)?;
        for (path, file, expected) in [
            (
                self.tools().join("config.json"),
                &receipt.config,
                config.as_slice(),
            ),
            (
                self.skill_directory().join("SKILL.md"),
                &receipt.skill,
                skill.as_slice(),
            ),
        ] {
            if file.pending.is_some()
                || file.active.as_deref() != Some(expected)
                || read_optional(&path)?.as_deref() != Some(expected)
            {
                return Err(Reason::OwnedFileChanged);
            }
        }
        Ok(())
    }

    /// Publish only absent files or bytes covered by the active/pending receipt.
    /// Write-ahead bytes permit retry after interruption without taking ownership
    /// of user edits. Existing unknown skill directories remain untouched.
    /// Success requires the published receipt and files to pass status inspection.
    pub(super) fn reconcile(&self, runtime: &CliRuntime) -> Result<()> {
        self.check_binding(runtime)?;
        let tools = self.tools();
        let _lock = transaction::Lock::acquire(&tools).map_err(|_| Reason::SetupUnavailable)?;
        let config_path = tools.join("config.json");
        let skill_path = self.skill_directory().join("SKILL.md");
        let config = self.config()?;
        let skill = self.skill(runtime)?;
        let current_config = read_optional(&config_path)?;
        let current_skill = read_optional(&skill_path)?;
        if present(&self.profile.join("skills/antiburn-debug.md"))? {
            return Err(Reason::OwnershipConflict);
        }
        let saved = self.receipt()?;
        let (mut receipt, mut receipt_bytes) = if let Some((receipt, bytes)) = saved {
            (receipt, Some(bytes))
        } else {
            if current_config.is_some() || present(&self.skill_directory())? {
                return Err(Reason::OwnershipConflict);
            }
            (
                Receipt {
                    schema: 1,
                    owner: OWNER.into(),
                    profile: self.profile.clone(),
                    config: FileReceipt {
                        active: None,
                        pending: None,
                    },
                    skill: FileReceipt {
                        active: None,
                        pending: None,
                    },
                },
                None,
            )
        };
        check_owned(current_config.as_deref(), &receipt.config)?;
        check_owned(current_skill.as_deref(), &receipt.skill)?;
        if receipt.config.active.as_deref() == Some(config.as_slice())
            && receipt.skill.active.as_deref() == Some(skill.as_slice())
            && receipt.config.pending.is_none()
            && receipt.skill.pending.is_none()
        {
            return self.inspect(runtime);
        }
        receipt
            .config
            .prepare_replacement(current_config.as_deref(), config.clone());
        receipt
            .skill
            .prepare_replacement(current_skill.as_deref(), skill.clone());
        save(&tools.join("receipt.json"), &receipt, &mut receipt_bytes)?;
        ensure_directory(&self.profile.join("skills"))?;
        ensure_directory(&self.skill_directory())?;
        for (path, current, proposed) in [
            (&config_path, current_config.as_deref(), config.as_slice()),
            (&skill_path, current_skill.as_deref(), skill.as_slice()),
        ] {
            if current != Some(proposed) {
                publish(path, current, proposed)?;
            }
        }
        receipt.config = FileReceipt {
            active: Some(config),
            pending: None,
        };
        receipt.skill = FileReceipt {
            active: Some(skill),
            pending: None,
        };
        save(&tools.join("receipt.json"), &receipt, &mut receipt_bytes)?;
        self.inspect(runtime)
    }
}

/// Reject an unreadable explicit selection instead of using a different profile.
fn profile_environment(
    mut read: impl FnMut(&str) -> Option<OsString>,
) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    for key in ["OPENCODE_CONFIG_DIR", "XDG_CONFIG_HOME"] {
        if let Some(value) = read(key) {
            env.insert(
                key.to_owned(),
                value
                    .into_string()
                    .map_err(|_| Reason::ProfileUnavailable)?,
            );
        }
    }
    Ok(env)
}

fn check_owned(current: Option<&[u8]>, receipt: &FileReceipt) -> Result<()> {
    if current == receipt.active.as_deref()
        || receipt
            .pending
            .as_deref()
            .is_some_and(|pending| current == Some(pending))
    {
        Ok(())
    } else {
        Err(Reason::OwnedFileChanged)
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    let parent = path.parent().ok_or(Reason::SetupUnavailable)?;
    match fs::symlink_metadata(parent) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(Reason::SetupUnavailable),
        Ok(_) => {
            checked_directory(parent)?;
            if present(path)? {
                antiburn_cli_runtime::validate_owned_file(path)
                    .map_err(|_| Reason::OwnedFileChanged)?;
            }
            transaction::read(path).map_err(|_| Reason::OwnedFileChanged)
        }
    }
}

fn ensure_directory(path: &Path) -> Result<()> {
    if present(path)? {
        checked_directory(path)
    } else {
        checked_directory(path.parent().ok_or(Reason::ProfileUnavailable)?)?;
        transaction::private_directory(path).map_err(|_| Reason::ProfileUnavailable)?;
        checked_directory(path)
    }
}

fn checked_directory(path: &Path) -> Result<()> {
    transaction::checked_directory(path).map_err(|_| Reason::ProfileUnavailable)?;
    antiburn_cli_runtime::validate_directory_ancestors(path).map_err(|_| Reason::ProfileUnavailable)
}

fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(Reason::SetupUnavailable),
    }
}

fn save(path: &Path, receipt: &Receipt, previous: &mut Option<Vec<u8>>) -> Result<()> {
    let bytes = serde_json::to_vec(receipt).map_err(|_| Reason::SetupUnavailable)?;
    if previous.as_deref() != Some(bytes.as_slice()) {
        publish(path, previous.as_deref(), &bytes)?;
        *previous = Some(bytes);
    }
    Ok(())
}

/// Recheck directory and existing-file policy immediately before atomic publication.
fn publish(path: &Path, expected: Option<&[u8]>, proposed: &[u8]) -> Result<()> {
    checked_directory(path.parent().ok_or(Reason::SetupUnavailable)?)?;
    if present(path)? {
        antiburn_cli_runtime::validate_owned_file(path).map_err(|_| Reason::OwnedFileChanged)?;
    }
    transaction::publish(path, expected, proposed).map_err(|_| Reason::SetupUnavailable)
}

fn quote(path: &Path) -> Result<String> {
    let value = path.to_str().ok_or(Reason::SetupUnavailable)?;
    if !path.is_absolute()
        || value.len() > 8192
        || value.chars().any(char::is_control)
        || value.contains('`')
    {
        return Err(Reason::SetupUnavailable);
    }
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

#[cfg(all(test, target_os = "macos"))]
#[path = "tests/installation_tests.rs"]
mod tests;
