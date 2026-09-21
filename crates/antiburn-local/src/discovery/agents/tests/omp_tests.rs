use super::*;
use std::time::UNIX_EPOCH;
use tempfile::TempDir;

fn omp_sessions_dir(home: &Path) -> PathBuf {
    home.join(".omp").join("agent").join("sessions")
}

#[test]
fn agent_dir_defaults_to_the_omp_config_root() {
    // `env_path_when_real_home` reads the process environment only for the
    // real home directory, so a synthetic home always resolves the default.
    let home = Path::new("/synthetic/home");
    assert_eq!(config_root_in(home), home.join(".omp"));
    assert_eq!(agent_dir_in(home), home.join(".omp").join("agent"));
}

/// Runs `body` with `HOME` pointed at `home` and `vars` applied, then
/// restores the previous environment.
fn with_env<T>(home: &Path, vars: &[(&str, &Path)], body: impl FnOnce() -> T) -> T {
    let mut previous = vec![("HOME", std::env::var_os("HOME"))];
    // SAFETY: every caller is serialised, so no other test thread can
    // observe the partial environment.
    unsafe { std::env::set_var("HOME", home) };
    for (key, value) in vars {
        previous.push((key, std::env::var_os(key)));
        unsafe { std::env::set_var(key, value) };
    }
    let result = body();
    for (key, value) in previous.into_iter().rev() {
        unsafe {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
    result
}

#[test]
#[serial_test::serial]
fn pi_config_dir_renames_the_omp_root() {
    let home = TempDir::new().unwrap();
    let renamed = Path::new(".omp-work");
    let (root, agent) = with_env(home.path(), &[("PI_CONFIG_DIR", renamed)], || {
        (config_root_in(home.path()), agent_dir_in(home.path()))
    });
    assert_eq!(root, home.path().join(".omp-work"));
    assert_eq!(agent, home.path().join(".omp-work").join("agent"));
}

#[test]
#[serial_test::serial]
fn a_coding_agent_dir_inside_the_omp_root_replaces_the_agent_dir() {
    let home = TempDir::new().unwrap();
    let inside = home.path().join(".omp").join("agent-alt");
    let agent = with_env(home.path(), &[("PI_CODING_AGENT_DIR", &inside)], || {
        agent_dir_in(home.path())
    });
    assert_eq!(agent, inside);
}

#[test]
#[serial_test::serial]
fn a_coding_agent_dir_outside_the_omp_root_stays_with_pi() {
    // Pi reads the same variable. A path outside the OMP root names a Pi
    // tree, so OMP keeps its own default instead of claiming it.
    let home = TempDir::new().unwrap();
    let outside = home.path().join("elsewhere").join("agent");
    let agent = with_env(home.path(), &[("PI_CODING_AGENT_DIR", &outside)], || {
        agent_dir_in(home.path())
    });
    assert_eq!(agent, home.path().join(".omp").join("agent"));
}

#[tokio::test]
async fn test_log_dirs_finds_subdirs() {
    let home = TempDir::new().unwrap();
    let sessions = omp_sessions_dir(home.path());
    let project_dir = sessions.join("--Users-test-projects-foo--");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let dirs = log_dirs_in(home.path()).await;
    assert_eq!(dirs, vec![project_dir]);
}

#[tokio::test]
async fn test_log_dirs_graceful_when_missing() {
    let home = TempDir::new().unwrap();
    let dirs = log_dirs_in(home.path()).await;
    assert!(dirs.is_empty());
}

#[tokio::test]
async fn test_discover_recent_finds_jsonl_files() {
    let home = TempDir::new().unwrap();
    let sessions = omp_sessions_dir(home.path());
    let project_dir = sessions.join("--Users-test--");
    tokio::fs::create_dir_all(&project_dir).await.unwrap();

    let session_file =
        project_dir.join("2026-05-26T01-02-03-000Z_019e61cd-aaaa-bbbb-cccc-dddddddddddd.jsonl");
    tokio::fs::write(
        &session_file,
        r#"{"type":"session","cwd":"/Users/test","version":3}"#,
    )
    .await
    .unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let dirs = log_dirs_in(home.path()).await;
    let results = recent_files_with_exts(&dirs, now, 86_400, &["jsonl"]).await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].path, session_file);
}

#[tokio::test]
async fn test_owns_path_matches_omp_session_path() {
    assert!(OmpExplorer.owns_path("/users/foo/.omp/agent/sessions/proj/a.jsonl"));
}

#[tokio::test]
async fn test_owns_path_rejects_pi_path() {
    assert!(!OmpExplorer.owns_path("/users/foo/.pi/agent/sessions/proj/a.jsonl"));
}

#[tokio::test]
async fn test_recover_session_id_from_path_extracts_uuid_suffix() {
    let path =
        Path::new("/tmp/2026-05-26T01-02-03-000Z_019e61cd-aaaa-bbbb-cccc-dddddddddddd.jsonl");
    assert_eq!(
        OmpExplorer.recover_session_id_from_path(path).as_deref(),
        Some("019e61cd-aaaa-bbbb-cccc-dddddddddddd")
    );
}
