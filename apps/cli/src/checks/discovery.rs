use std::path::PathBuf;

use crate::config::Config;

pub fn candidate_database_paths(config: &Config) -> Result<Vec<PathBuf>, String> {
    let data_root = match &config.data_root {
        Some(path) => path.clone(),
        None => dirs::data_dir()
            .ok_or_else(|| "could not locate the platform application data directory".to_owned())?,
    };
    let path = database_path(config, data_root);
    if !path.is_file() {
        return Err(format!(
            "Antiburn database was not found at {}; set data_root, app_identifier, or database_filename in the CLI config",
            path.display()
        ));
    }
    Ok(vec![path])
}

pub(crate) fn database_path(config: &Config, data_root: PathBuf) -> PathBuf {
    data_root
        .join(&config.app_identifier)
        .join(&config.database_filename)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::config::Config;

    use super::{candidate_database_paths, database_path};

    #[test]
    fn release_config_resolves_to_the_release_database_path() {
        let temp = tempdir().unwrap();
        let config = Config::defaults();
        assert_eq!(
            database_path(&config, temp.path().to_path_buf()),
            temp.path()
                .join("ai.antiburn.desktop")
                .join("antiburn.sqlite3")
        );
    }

    #[test]
    fn config_overrides_resolve_a_development_database_path() {
        let temp = tempdir().unwrap();
        let mut config = Config::defaults();
        config.app_identifier = "ai.antiburn.desktop.debug".into();
        config.database_filename = "antiburn-debug.sqlite3".into();
        config.data_root = Some(temp.path().to_path_buf());
        let path = database_path(&config, temp.path().to_path_buf());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, []).unwrap();

        assert_eq!(candidate_database_paths(&config).unwrap(), vec![path]);
    }

    #[test]
    fn discovery_does_not_create_missing_database_files() {
        let temp = tempdir().unwrap();
        let mut config = Config::defaults();
        config.data_root = Some(temp.path().to_path_buf());
        let error = candidate_database_paths(&config).unwrap_err();
        assert!(error.contains("database was not found"));
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}
