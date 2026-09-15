use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

#[cfg(not(windows))]
use super::json::edit_top_level_string;
use super::json::parse;
use super::{OperationSelector, Target, VendorConfig, VendorPolicy};
use crate::agent_config::filesystem::{path_entry_exists, read_checked};
use crate::agent_config::{ConfigScope, ConfigSetting, ConfigUnavailableReason};

pub(super) struct OpenCode;

pub(super) static OPENCODE: OpenCode = OpenCode;

const RUNTIME_OVERRIDE_NAMES: [&str; 6] = [
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_DIR",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_AUTH_CONTENT",
    "OPENCODE_DB",
    "OPENCODE_DATA_DIR",
];

impl VendorConfig for OpenCode {
    fn policy(&self, setting: ConfigSetting) -> VendorPolicy {
        match setting {
            ConfigSetting::Model => VendorPolicy::AutomaticEdit,
            ConfigSetting::Reasoning => {
                VendorPolicy::Unsupported(ConfigUnavailableReason::UnsupportedSetting)
            }
        }
    }

    fn resolve_target(
        &self,
        setting: ConfigSetting,
        home: &Path,
        workspace_cwd: Option<&Path>,
        trusted_workspace_root: Option<&Path>,
    ) -> Result<Target, ConfigUnavailableReason> {
        if setting != ConfigSetting::Model {
            return Err(ConfigUnavailableReason::UnsupportedSetting);
        }
        reject_runtime_overrides()?;
        reject_remote_inputs(home)?;
        reject_managed_config()?;

        let global_root = global_config_root(home)?;
        reject_directory_overrides(&global_root)?;
        let mut winner = None;
        merge_directory_config(
            &global_root,
            &global_root,
            ConfigScope::Global,
            &mut winner,
            true,
        )?;

        if !project_config_disabled()
            && let Some(cwd) = workspace_cwd
        {
            let safety_root = trusted_workspace_root.ok_or(ConfigUnavailableReason::UnsafePath)?;
            let roots = project_hierarchy(cwd, safety_root)?;
            for root in &roots {
                merge_directory_config(
                    root,
                    safety_root,
                    ConfigScope::Project,
                    &mut winner,
                    false,
                )?;
            }
            for root in roots.iter().rev() {
                let directory = root.join(".opencode");
                if path_entry_exists(&directory)? {
                    reject_directory_overrides(&directory)?;
                    merge_directory_config(
                        &directory,
                        safety_root,
                        ConfigScope::Project,
                        &mut winner,
                        false,
                    )?;
                }
            }
        }

        let home_directory = home.join(".opencode");
        if path_entry_exists(&home_directory)? {
            reject_directory_overrides(&home_directory)?;
            merge_directory_config(
                &home_directory,
                home,
                ConfigScope::Global,
                &mut winner,
                false,
            )?;
        }

        winner.ok_or(ConfigUnavailableReason::MissingTarget)
    }

    fn read_value(
        &self,
        bytes: &[u8],
        operation: &OperationSelector,
    ) -> Result<Option<String>, ConfigUnavailableReason> {
        if operation != &OperationSelector::JsonKey("model") {
            return Err(ConfigUnavailableReason::UnsupportedSetting);
        }
        model(bytes)
    }

    #[cfg(not(windows))]
    fn edit_value(
        &self,
        bytes: &[u8],
        operation: &OperationSelector,
        proposed: &str,
    ) -> Result<Vec<u8>, ConfigUnavailableReason> {
        if operation != &OperationSelector::JsonKey("model") {
            return Err(ConfigUnavailableReason::UnsupportedSetting);
        }
        edit_top_level_string(bytes, "model", proposed)
    }
}

fn merge_directory_config(
    root: &Path,
    safety_root: &Path,
    scope: ConfigScope,
    winner: &mut Option<Target>,
    include_legacy: bool,
) -> Result<(), ConfigUnavailableReason> {
    let names: &[&str] = if include_legacy {
        &["config.json", "opencode.json", "opencode.jsonc"]
    } else {
        &["opencode.json", "opencode.jsonc"]
    };
    for name in names {
        let path = root.join(name);
        if !path_entry_exists(&path)? {
            continue;
        }
        if model(&read_checked(&path, safety_root)?.bytes)?.is_some() {
            *winner = Some(Target {
                path,
                safety_root: safety_root.to_owned(),
                scope,
                operation: OperationSelector::JsonKey("model"),
            });
        }
    }
    Ok(())
}

fn model(bytes: &[u8]) -> Result<Option<String>, ConfigUnavailableReason> {
    let document = parse(bytes)?;
    reject_agent_overrides(&document)?;
    document
        .get("model")
        .map(|value| {
            let value = value
                .as_str()
                .ok_or(ConfigUnavailableReason::MalformedConfig)?;
            if value.contains("{env:") || value.contains("{file:") {
                Err(ConfigUnavailableReason::DynamicValue)
            } else {
                Ok(value.to_owned())
            }
        })
        .transpose()
}

fn reject_agent_overrides(document: &serde_json::Value) -> Result<(), ConfigUnavailableReason> {
    if document.get("agent").is_some() || document.get("mode").is_some() {
        Err(ConfigUnavailableReason::InvalidPrecedence)
    } else {
        Ok(())
    }
}

fn reject_runtime_overrides() -> Result<(), ConfigUnavailableReason> {
    if RUNTIME_OVERRIDE_NAMES
        .iter()
        .any(|name| std::env::var_os(name).is_some())
    {
        Err(ConfigUnavailableReason::RuntimeOverride)
    } else {
        Ok(())
    }
}

fn project_config_disabled() -> bool {
    std::env::var("OPENCODE_DISABLE_PROJECT_CONFIG")
        .is_ok_and(|value| project_config_disabled_value(&value))
}

fn project_config_disabled_value(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "1" | "true")
}

fn global_config_root(home: &Path) -> Result<PathBuf, ConfigUnavailableReason> {
    let process_home = std::env::var_os("HOME");
    let xdg_config_home = process_home
        .as_deref()
        .is_some_and(|process_home| Path::new(process_home) == home)
        .then(|| std::env::var_os("XDG_CONFIG_HOME"))
        .flatten();
    global_config_root_for(home, xdg_config_home.as_deref())
}

fn global_config_root_for(
    home: &Path,
    value: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, ConfigUnavailableReason> {
    let Some(value) = value else {
        return Ok(home.join(".config/opencode"));
    };
    if value.is_empty() {
        return Ok(home.join(".config/opencode"));
    }
    let root = PathBuf::from(value);
    if !root.is_absolute() {
        return Err(ConfigUnavailableReason::RuntimeOverride);
    }
    Ok(root.join("opencode"))
}

fn project_hierarchy(
    cwd: &Path,
    trusted_root: &Path,
) -> Result<Vec<PathBuf>, ConfigUnavailableReason> {
    if !cwd.starts_with(trusted_root) {
        return Err(ConfigUnavailableReason::UnsafePath);
    }
    let mut roots = vec![cwd.to_owned()];
    let mut current = cwd;
    while current != trusted_root {
        let Some(parent) = current.parent() else {
            return Err(ConfigUnavailableReason::UnsafePath);
        };
        if !parent.starts_with(trusted_root) {
            return Err(ConfigUnavailableReason::UnsafePath);
        }
        current = parent;
        roots.push(current.to_owned());
    }
    roots.reverse();
    Ok(roots)
}

fn reject_directory_overrides(root: &Path) -> Result<(), ConfigUnavailableReason> {
    for name in ["agent", "agents", "mode", "modes"] {
        if path_entry_exists(&root.join(name))? {
            return Err(ConfigUnavailableReason::InvalidPrecedence);
        }
    }
    Ok(())
}

fn reject_managed_config() -> Result<(), ConfigUnavailableReason> {
    #[cfg(windows)]
    {
        Ok(())
    }

    #[cfg(not(windows))]
    {
        #[cfg(target_os = "macos")]
        let root = Path::new("/Library/Application Support/opencode");
        #[cfg(all(unix, not(target_os = "macos")))]
        let root = Path::new("/etc/opencode");
        reject_managed_config_at(root)?;
        #[cfg(target_os = "macos")]
        reject_managed_preferences()?;
        Ok(())
    }
}

#[cfg(not(windows))]
fn reject_managed_config_at(root: &Path) -> Result<(), ConfigUnavailableReason> {
    for path in [root.join("opencode.json"), root.join("opencode.jsonc")] {
        if path_entry_exists(&path)? {
            return Err(ConfigUnavailableReason::ManagedConfiguration);
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn reject_managed_preferences() -> Result<(), ConfigUnavailableReason> {
    let preferences = Path::new("/Library/Managed Preferences");
    if path_entry_exists(&preferences.join("ai.opencode.managed.plist"))? {
        return Err(ConfigUnavailableReason::ManagedConfiguration);
    }
    if path_entry_exists(preferences)? {
        let entries = std::fs::read_dir(preferences)
            .map_err(|_| ConfigUnavailableReason::PermissionDenied)?;
        for entry in entries {
            let entry = entry.map_err(|_| ConfigUnavailableReason::UnsafePath)?;
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(true)
                && path_entry_exists(&entry.path().join("ai.opencode.managed.plist"))?
            {
                return Err(ConfigUnavailableReason::ManagedConfiguration);
            }
        }
    }
    Ok(())
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn environment_inputs_are_table_driven() {
        let home = Path::new("/home/test");
        let config_cases = [
            (None, Ok(home.join(".config/opencode"))),
            (Some(""), Ok(home.join(".config/opencode"))),
            (Some("/tmp/xdg"), Ok(PathBuf::from("/tmp/xdg/opencode"))),
            (
                Some("relative"),
                Err(ConfigUnavailableReason::RuntimeOverride),
            ),
        ];
        for (value, expected) in config_cases {
            assert_eq!(
                global_config_root_for(home, value.map(std::ffi::OsStr::new)),
                expected
            );
        }
        for (value, expected) in [
            ("1", true),
            ("true", true),
            ("TRUE", true),
            ("0", false),
            ("false", false),
            ("", false),
        ] {
            assert_eq!(project_config_disabled_value(value), expected);
        }
        assert_eq!(
            RUNTIME_OVERRIDE_NAMES,
            [
                "OPENCODE_CONFIG",
                "OPENCODE_CONFIG_DIR",
                "OPENCODE_CONFIG_CONTENT",
                "OPENCODE_AUTH_CONTENT",
                "OPENCODE_DB",
                "OPENCODE_DATA_DIR",
            ]
        );
    }

    #[test]
    fn managed_config_files_fail_closed() {
        let temporary = tempfile::tempdir().unwrap();
        for name in ["opencode.json", "opencode.jsonc"] {
            let root = temporary.path().join(name.replace('.', "-"));
            std::fs::create_dir(&root).unwrap();
            std::fs::write(root.join(name), "{}").unwrap();
            assert_eq!(
                reject_managed_config_at(&root),
                Err(ConfigUnavailableReason::ManagedConfiguration)
            );
        }
    }
}

fn reject_remote_inputs(home: &Path) -> Result<(), ConfigUnavailableReason> {
    let data_root = match std::env::var_os("XDG_DATA_HOME") {
        Some(value) if !value.is_empty() => {
            let root = PathBuf::from(value);
            if !root.is_absolute() {
                return Err(ConfigUnavailableReason::RuntimeOverride);
            }
            root.join("opencode")
        }
        _ => home.join(".local/share/opencode"),
    };
    let auth = data_root.join("auth.json");
    if path_entry_exists(&auth)? {
        let root = data_root
            .canonicalize()
            .map_err(|_| ConfigUnavailableReason::UnsafePath)?;
        let document = parse(&read_checked(&auth, &root)?.bytes)?;
        let entries = document
            .as_object()
            .ok_or(ConfigUnavailableReason::MalformedConfig)?;
        if entries
            .values()
            .any(|entry| entry.get("type").and_then(serde_json::Value::as_str) == Some("wellknown"))
        {
            return Err(ConfigUnavailableReason::RuntimeOverride);
        }
    }
    if !path_entry_exists(&data_root)? {
        return Ok(());
    }
    let entries =
        std::fs::read_dir(&data_root).map_err(|_| ConfigUnavailableReason::PermissionDenied)?;
    for entry in entries {
        let entry = entry.map_err(|_| ConfigUnavailableReason::UnsafePath)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name == "opencode.db" || name.starts_with("opencode-") && name.ends_with(".db")) {
            continue;
        }
        let connection = Connection::open_with_flags(
            entry.path(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| ConfigUnavailableReason::RuntimeOverride)?;
        let active = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM account_state WHERE active_org_id IS NOT NULL LIMIT 1)",
            [],
            |row| row.get::<_, bool>(0),
        );
        match active {
            Ok(true) => return Err(ConfigUnavailableReason::RuntimeOverride),
            Ok(false) => {}
            Err(error) if error.to_string().contains("no such table") => {}
            Err(_) => return Err(ConfigUnavailableReason::RuntimeOverride),
        }
    }
    Ok(())
}
