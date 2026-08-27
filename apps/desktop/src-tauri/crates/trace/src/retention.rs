use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::LOG_FILE_PREFIX;

pub(crate) fn clean_old_logs(log_dir: &Path, max_age: Duration) -> std::io::Result<usize> {
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in std::fs::read_dir(log_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() || !is_log_file(&entry.file_name()) {
            continue;
        }
        let age = now
            .duration_since(entry.metadata()?.modified()?)
            .unwrap_or_default();
        if max_age.is_zero() || age > max_age {
            std::fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn is_log_file(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    name.starts_with(&format!("{LOG_FILE_PREFIX}.")) && name.ends_with(".log")
}

#[cfg(test)]
mod tests {
    use super::clean_old_logs;
    use std::time::Duration;

    #[test]
    fn zero_age_removes_only_matching_files() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("antiburn.2026-08-20-14.log"), "event").unwrap();
        std::fs::write(temp.path().join("other.log"), "event").unwrap();
        std::fs::write(temp.path().join("antiburn.txt"), "event").unwrap();
        std::fs::create_dir(temp.path().join("antiburn.directory.log")).unwrap();

        assert_eq!(clean_old_logs(temp.path(), Duration::ZERO).unwrap(), 1);
        assert!(!temp.path().join("antiburn.2026-08-20-14.log").exists());
        assert!(temp.path().join("other.log").exists());
        assert!(temp.path().join("antiburn.txt").exists());
        assert!(temp.path().join("antiburn.directory.log").is_dir());
    }

    #[test]
    fn large_age_removes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("antiburn.2026-08-20-14.log");
        std::fs::write(&file, "event").unwrap();
        std::fs::write(temp.path().join("other.log"), "event").unwrap();
        std::fs::write(temp.path().join("antiburn.txt"), "event").unwrap();
        std::fs::create_dir(temp.path().join("antiburn.directory.log")).unwrap();

        assert_eq!(
            clean_old_logs(temp.path(), Duration::from_secs(60 * 60)).unwrap(),
            0
        );
        assert!(file.exists());
        assert!(temp.path().join("other.log").exists());
        assert!(temp.path().join("antiburn.txt").exists());
        assert!(temp.path().join("antiburn.directory.log").is_dir());
    }

    #[test]
    fn missing_directory_returns_error_without_creating_it() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");

        assert!(clean_old_logs(&missing, Duration::ZERO).is_err());
        assert!(!missing.exists());
    }
}
