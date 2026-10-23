use std::{
    io::ErrorKind,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;

const DEFAULT_APP_IDENTIFIER: &str = "ai.antiburn.desktop";
const DEFAULT_DATABASE_FILENAME: &str = "antiburn.sqlite3";
const DEFAULT_BUSY_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Clone)]
pub struct Config {
    pub data_root: Option<PathBuf>,
    pub app_identifier: String,
    pub database_filename: String,
    pub busy_timeout: Duration,
    pub state_file: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigOverrides {
    data_root: Option<PathBuf>,
    app_identifier: Option<String>,
    database_filename: Option<String>,
    busy_timeout_ms: Option<u64>,
    state_file: Option<PathBuf>,
}

impl Config {
    pub async fn load(config_path: Option<PathBuf>) -> Result<Self, String> {
        let explicit_path = config_path.is_some();
        let config_path = config_path.or_else(default_config_path);
        let Some(config_path) = config_path else {
            return Ok(Self::defaults());
        };

        let contents = match tokio::fs::read_to_string(&config_path).await {
            Ok(contents) => contents,
            Err(error) if !explicit_path && error.kind() == ErrorKind::NotFound => {
                return Ok(Self::defaults());
            }
            Err(error) => {
                return Err(format!(
                    "could not read CLI config file {}: {error}",
                    config_path.display()
                ));
            }
        };

        let overrides: ConfigOverrides = serde_json::from_str(&contents).map_err(|error| {
            format!("invalid CLI config file {}: {error}", config_path.display())
        })?;
        Self::from_overrides(overrides)
    }

    pub(crate) fn defaults() -> Self {
        Self {
            data_root: None,
            app_identifier: DEFAULT_APP_IDENTIFIER.to_owned(),
            database_filename: DEFAULT_DATABASE_FILENAME.to_owned(),
            busy_timeout: Duration::from_millis(DEFAULT_BUSY_TIMEOUT_MS),
            state_file: None,
        }
    }

    fn from_overrides(overrides: ConfigOverrides) -> Result<Self, String> {
        let mut config = Self::defaults();
        config.data_root = overrides.data_root;
        if let Some(app_identifier) = overrides.app_identifier {
            validate_path_component("app_identifier", &app_identifier)?;
            config.app_identifier = app_identifier;
        }
        if let Some(database_filename) = overrides.database_filename {
            validate_path_component("database_filename", &database_filename)?;
            config.database_filename = database_filename;
        }
        if let Some(busy_timeout_ms) = overrides.busy_timeout_ms {
            config.busy_timeout = Duration::from_millis(busy_timeout_ms);
        }
        config.state_file = overrides.state_file;
        Ok(config)
    }

    pub(crate) fn state_path(&self) -> Result<PathBuf, String> {
        self.state_file
            .clone()
            .or_else(default_state_path)
            .ok_or_else(|| {
                "could not locate the platform config directory for CLI state".to_owned()
            })
    }
}

fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|directory| directory.join("antiburn").join("cli.json"))
}

fn default_state_path() -> Option<PathBuf> {
    dirs::config_dir().map(|directory| directory.join("antiburn").join("cli-state.json"))
}

fn validate_path_component(field: &str, value: &str) -> Result<(), String> {
    let mut components = Path::new(value).components();
    let is_one_normal_component =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    if !is_one_normal_component || value.contains('/') || value.contains('\\') {
        return Err(format!("{field} must be one path component"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::tempdir;

    use super::Config;

    #[test]
    fn release_database_values_are_defaults() {
        let config = Config::defaults();
        assert_eq!(config.app_identifier, "ai.antiburn.desktop");
        assert_eq!(config.database_filename, "antiburn.sqlite3");
        assert_eq!(config.busy_timeout.as_millis(), 5_000);
        assert!(config.data_root.is_none());
        assert!(config.state_file.is_none());
    }

    #[tokio::test]
    async fn missing_explicit_config_is_an_error() {
        let temp = tempdir().unwrap();
        let config = Config::load(Some(temp.path().join("missing.json"))).await;
        assert!(config.is_err());
    }

    #[tokio::test]
    async fn config_file_overrides_release_values() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("cli.json");
        fs::write(
            &path,
            r#"{
                "data_root": "/tmp/antiburn-data",
                "app_identifier": "ai.antiburn.desktop.debug",
                "database_filename": "antiburn-debug.sqlite3",
                "busy_timeout_ms": 12000,
                "state_file": "/tmp/antiburn-state.json"
            }"#,
        )
        .unwrap();

        let config = Config::load(Some(path)).await.unwrap();
        assert_eq!(
            config.data_root.as_ref().unwrap().clone(),
            PathBuf::from("/tmp/antiburn-data")
        );
        assert_eq!(config.app_identifier, "ai.antiburn.desktop.debug");
        assert_eq!(config.database_filename, "antiburn-debug.sqlite3");
        assert_eq!(config.busy_timeout.as_millis(), 12_000);
        assert_eq!(
            config.state_path().unwrap(),
            PathBuf::from("/tmp/antiburn-state.json")
        );
    }

    #[tokio::test]
    async fn config_rejects_path_traversal_and_unknown_fields() {
        for contents in [
            r#"{"app_identifier":"../other"}"#,
            r#"{"database_filename":"folder/file.sqlite3"}"#,
            r#"{"unexpected":true}"#,
        ] {
            let temp = tempdir().unwrap();
            let path = temp.path().join("cli.json");
            fs::write(&path, contents).unwrap();
            assert!(Config::load(Some(path)).await.is_err());
        }
    }
}
