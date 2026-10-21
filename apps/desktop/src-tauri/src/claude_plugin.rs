//! Whether the antiburn Claude Code plugin is installed on this machine.
//!
//! Claude Code records each plugin install in `plugins/installed_plugins.json`
//! under its config directory, and records enablement in `settings.json` in
//! the same directory. The config directory is `$CLAUDE_CONFIG_DIR` when that
//! is set, else `~/.claude`.
//!
//! This module reads only those two files. It reads plugin names and version
//! strings, nothing else: no credentials file, no Keychain, no transcripts.
//! Only the state and version leave this module. The install path and
//! marketplace name stay on disk.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// The install keys that count as the antiburn plugin. The public
/// marketplace key is first. The second covers a plugin synced from another
/// machine through Claude Code's own settings sync.
const PLUGIN_KEYS: [&str; 2] = ["antiburn@antiburn", "antiburn@synced"];

/// A guard against reading a file that is not the small registry we expect.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ClaudePluginState {
    /// The registry lists the plugin.
    Installed,
    /// The registry exists and does not list the plugin, or Claude Code has
    /// not written a registry yet.
    Absent,
    /// A file could not be read or parsed. The renderer shows no offer in
    /// this state so it does not nag a reader it cannot see.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudePluginStatus {
    pub state: ClaudePluginState,
    /// The installed version string, when the registry lists one.
    pub version: Option<String>,
    /// False only when `settings.json` lists the plugin key as `false`.
    pub enabled: bool,
}

impl ClaudePluginStatus {
    fn absent() -> Self {
        Self {
            state: ClaudePluginState::Absent,
            version: None,
            enabled: true,
        }
    }

    fn unknown() -> Self {
        Self {
            state: ClaudePluginState::Unknown,
            version: None,
            enabled: true,
        }
    }
}

/// The Claude Code config directory, at the one documented place it lives.
pub fn config_dir() -> Option<PathBuf> {
    antiburn_local::paths::non_empty_env_path("CLAUDE_CONFIG_DIR")
        .or_else(|| antiburn_local::paths::home_dir().map(|home| home.join(".claude")))
}

/// Read the plugin status from the default config directory.
pub fn read_status() -> ClaudePluginStatus {
    match config_dir() {
        Some(dir) => read_status_in(&dir),
        None => ClaudePluginStatus::unknown(),
    }
}

/// Read the plugin status from the registry files under `dir`.
pub fn read_status_in(dir: &Path) -> ClaudePluginStatus {
    let registry = match read_json(&dir.join("plugins").join("installed_plugins.json")) {
        FileRead::Missing => return ClaudePluginStatus::absent(),
        FileRead::Unreadable => return ClaudePluginStatus::unknown(),
        FileRead::Parsed(value) => value,
    };
    let Some(plugins) = registry.get("plugins").and_then(|value| value.as_object()) else {
        return ClaudePluginStatus::unknown();
    };
    let Some((key, entries)) = PLUGIN_KEYS
        .iter()
        .find_map(|key| plugins.get(*key).map(|entries| (*key, entries)))
    else {
        return ClaudePluginStatus::absent();
    };
    let version = entries
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|entry| entry.get("version").and_then(|value| value.as_str()))
        .map(str::to_owned);
    let enabled = match read_json(&dir.join("settings.json")) {
        FileRead::Parsed(settings) => settings
            .get("enabledPlugins")
            .and_then(|value| value.get(key))
            .and_then(|value| value.as_bool())
            .unwrap_or(true),
        // A missing or unreadable settings file does not change the fact that
        // the registry lists the plugin.
        FileRead::Missing | FileRead::Unreadable => true,
    };
    ClaudePluginStatus {
        state: ClaudePluginState::Installed,
        version,
        enabled,
    }
}

enum FileRead {
    Missing,
    Unreadable,
    Parsed(serde_json::Value),
}

fn read_json(path: &Path) -> FileRead {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return FileRead::Missing,
        Err(_) => return FileRead::Unreadable,
    };
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return FileRead::Unreadable;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return FileRead::Unreadable;
    };
    match serde_json::from_str(&text) {
        Ok(value) => FileRead::Parsed(value),
        Err(_) => FileRead::Unreadable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, relative: &str, text: &str) {
        let path = dir.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    }

    const REGISTRY_WITH_PLUGIN: &str = r#"{
        "version": 2,
        "plugins": {
            "other@market": [{"scope": "user", "version": "9.9.9"}],
            "antiburn@antiburn": [
                {"scope": "user", "installPath": "/x", "version": "0.2.1"},
                {"scope": "project", "version": "0.2.0"}
            ]
        }
    }"#;

    #[test]
    fn missing_registry_is_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(read_status_in(dir.path()), ClaudePluginStatus::absent());
    }

    #[test]
    fn registry_without_the_key_is_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "plugins/installed_plugins.json",
            r#"{"version": 2, "plugins": {"other@market": [{"version": "1.0.0"}]}}"#,
        );
        assert_eq!(read_status_in(dir.path()), ClaudePluginStatus::absent());
    }

    #[test]
    fn registry_with_the_key_is_installed_with_its_first_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "plugins/installed_plugins.json",
            REGISTRY_WITH_PLUGIN,
        );
        assert_eq!(
            read_status_in(dir.path()),
            ClaudePluginStatus {
                state: ClaudePluginState::Installed,
                version: Some("0.2.1".to_owned()),
                enabled: true,
            }
        );
    }

    #[test]
    fn synced_key_counts_as_installed() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "plugins/installed_plugins.json",
            r#"{"version": 2, "plugins": {"antiburn@synced": [{"version": "0.3.0"}]}}"#,
        );
        let status = read_status_in(dir.path());
        assert_eq!(status.state, ClaudePluginState::Installed);
        assert_eq!(status.version.as_deref(), Some("0.3.0"));
    }

    #[test]
    fn settings_can_disable_the_plugin() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "plugins/installed_plugins.json",
            REGISTRY_WITH_PLUGIN,
        );
        write(
            dir.path(),
            "settings.json",
            r#"{"enabledPlugins": {"antiburn@antiburn": false, "other@market": true}}"#,
        );
        let status = read_status_in(dir.path());
        assert_eq!(status.state, ClaudePluginState::Installed);
        assert!(!status.enabled);
    }

    #[test]
    fn malformed_settings_leave_the_plugin_enabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "plugins/installed_plugins.json",
            REGISTRY_WITH_PLUGIN,
        );
        write(dir.path(), "settings.json", "{not json");
        assert!(read_status_in(dir.path()).enabled);
    }

    #[test]
    fn malformed_registry_is_unknown() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(dir.path(), "plugins/installed_plugins.json", "{not json");
        assert_eq!(read_status_in(dir.path()), ClaudePluginStatus::unknown());
    }

    #[test]
    fn registry_without_a_plugins_map_is_unknown() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "plugins/installed_plugins.json",
            r#"{"version": 2}"#,
        );
        assert_eq!(read_status_in(dir.path()), ClaudePluginStatus::unknown());
    }

    #[test]
    fn entry_without_a_version_is_installed_without_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            dir.path(),
            "plugins/installed_plugins.json",
            r#"{"version": 2, "plugins": {"antiburn@antiburn": [{"scope": "user"}]}}"#,
        );
        let status = read_status_in(dir.path());
        assert_eq!(status.state, ClaudePluginState::Installed);
        assert_eq!(status.version, None);
    }

    #[test]
    fn status_serializes_in_camel_case() {
        let json = serde_json::to_value(ClaudePluginStatus {
            state: ClaudePluginState::Installed,
            version: Some("0.2.1".to_owned()),
            enabled: false,
        })
        .expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({"state": "installed", "version": "0.2.1", "enabled": false})
        );
        assert_eq!(
            serde_json::to_value(ClaudePluginStatus::absent()).expect("serialize")["state"],
            "absent"
        );
    }
}
