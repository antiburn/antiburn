use std::{
    io::ErrorKind,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;

const DEFAULT_APP_IDENTIFIER: &str = "ai.antiburn.desktop";
const DEFAULT_DATABASE_FILENAME: &str = "antiburn.sqlite3";
const DEFAULT_BUSY_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_PRICE_OUTLIER_MAX_SESSIONS: usize = 16;
const DEFAULT_PRICE_OUTLIER_MAX_AGE_HOURS: u64 = 72;
const DEFAULT_PRICE_OUTLIER_MINIMUM_PRICED_SESSIONS: usize = 8;
const DEFAULT_PRICE_OUTLIER_FLOOR_USD: f64 = 2.0;
const DEFAULT_PRICE_OUTLIER_MEDIAN_MULTIPLE: f64 = 3.0;

#[derive(Debug, Clone, Copy)]
pub struct PriceOutlierConfig {
    pub max_sessions: usize,
    pub max_age_hours: u64,
    pub minimum_priced_sessions: usize,
    pub floor_usd: f64,
    pub median_multiple: f64,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub data_root: Option<PathBuf>,
    pub app_identifier: String,
    pub database_filename: String,
    pub busy_timeout: Duration,
    pub state_file: Option<PathBuf>,
    pub pricing_snapshot_path: Option<PathBuf>,
    pub price_outlier: PriceOutlierConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigOverrides {
    data_root: Option<PathBuf>,
    app_identifier: Option<String>,
    database_filename: Option<String>,
    busy_timeout_ms: Option<u64>,
    state_file: Option<PathBuf>,
    pricing_snapshot_path: Option<PathBuf>,
    price_outlier: Option<PriceOutlierOverrides>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PriceOutlierOverrides {
    max_sessions: Option<usize>,
    max_age_hours: Option<u64>,
    minimum_priced_sessions: Option<usize>,
    floor_usd: Option<f64>,
    median_multiple: Option<f64>,
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
            pricing_snapshot_path: None,
            price_outlier: PriceOutlierConfig {
                max_sessions: DEFAULT_PRICE_OUTLIER_MAX_SESSIONS,
                max_age_hours: DEFAULT_PRICE_OUTLIER_MAX_AGE_HOURS,
                minimum_priced_sessions: DEFAULT_PRICE_OUTLIER_MINIMUM_PRICED_SESSIONS,
                floor_usd: DEFAULT_PRICE_OUTLIER_FLOOR_USD,
                median_multiple: DEFAULT_PRICE_OUTLIER_MEDIAN_MULTIPLE,
            },
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
        config.pricing_snapshot_path = overrides.pricing_snapshot_path;
        if let Some(price_outlier) = overrides.price_outlier {
            if let Some(value) = price_outlier.max_sessions {
                config.price_outlier.max_sessions = value;
            }
            if let Some(value) = price_outlier.max_age_hours {
                config.price_outlier.max_age_hours = value;
            }
            if let Some(value) = price_outlier.minimum_priced_sessions {
                config.price_outlier.minimum_priced_sessions = value;
            }
            if let Some(value) = price_outlier.floor_usd {
                config.price_outlier.floor_usd = value;
            }
            if let Some(value) = price_outlier.median_multiple {
                config.price_outlier.median_multiple = value;
            }
        }
        validate_price_outlier_config(config.price_outlier)?;
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

fn validate_price_outlier_config(config: PriceOutlierConfig) -> Result<(), String> {
    if config.max_sessions == 0 || i64::try_from(config.max_sessions).is_err() {
        return Err("price_outlier.max_sessions must fit a positive SQLite limit".to_owned());
    }
    if config.max_age_hours == 0 || config.max_age_hours > (i64::MAX as u64) / 3_600 {
        return Err("price_outlier.max_age_hours must fit a positive SQLite time range".to_owned());
    }
    if config.minimum_priced_sessions == 0 {
        return Err("price_outlier.minimum_priced_sessions must be greater than zero".to_owned());
    }
    if config.minimum_priced_sessions > config.max_sessions {
        return Err("price_outlier.minimum_priced_sessions cannot exceed max_sessions".to_owned());
    }
    if !config.floor_usd.is_finite() || config.floor_usd < 0.0 {
        return Err("price_outlier.floor_usd must be finite and non-negative".to_owned());
    }
    if !config.median_multiple.is_finite() || config.median_multiple <= 0.0 {
        return Err(
            "price_outlier.median_multiple must be finite and greater than zero".to_owned(),
        );
    }
    Ok(())
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
        assert!(config.pricing_snapshot_path.is_none());
        assert_eq!(config.price_outlier.max_sessions, 16);
        assert_eq!(config.price_outlier.max_age_hours, 72);
        assert_eq!(config.price_outlier.minimum_priced_sessions, 8);
        assert_eq!(config.price_outlier.floor_usd, 2.0);
        assert_eq!(config.price_outlier.median_multiple, 3.0);
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
                "state_file": "/tmp/antiburn-state.json",
                "pricing_snapshot_path": "/tmp/model-pricing.json",
                "price_outlier": {
                    "max_sessions": 20,
                    "max_age_hours": 96,
                    "minimum_priced_sessions": 9,
                    "floor_usd": 3.0,
                    "median_multiple": 4.0
                }
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
        assert_eq!(
            config.pricing_snapshot_path.as_deref(),
            Some(std::path::Path::new("/tmp/model-pricing.json"))
        );
        assert_eq!(config.price_outlier.max_sessions, 20);
        assert_eq!(config.price_outlier.max_age_hours, 96);
        assert_eq!(config.price_outlier.minimum_priced_sessions, 9);
        assert_eq!(config.price_outlier.floor_usd, 3.0);
        assert_eq!(config.price_outlier.median_multiple, 4.0);
    }

    #[tokio::test]
    async fn config_rejects_path_traversal_and_unknown_fields() {
        for contents in [
            r#"{"app_identifier":"../other"}"#,
            r#"{"database_filename":"folder/file.sqlite3"}"#,
            r#"{"price_outlier":{"max_sessions":0}}"#,
            r#"{"price_outlier":{"max_age_hours":3000000000000000}}"#,
            r#"{"price_outlier":{"minimum_priced_sessions":17}}"#,
            r#"{"price_outlier":{"floor_usd":-1}}"#,
            r#"{"price_outlier":{"median_multiple":0}}"#,
            r#"{"unexpected":true}"#,
        ] {
            let temp = tempdir().unwrap();
            let path = temp.path().join("cli.json");
            fs::write(&path, contents).unwrap();
            assert!(Config::load(Some(path)).await.is_err());
        }
    }
}
