//! The folders that antiburn reads for each agent's sessions, for the Agents
//! step settings list. The command uses the same watch roots as the disk
//! watcher, so the list agrees with the scan.

use std::collections::HashSet;
use std::path::Path;

use antiburn_local::discovery::Explorers;
use antiburn_local::model::AgentKind;
use antiburn_local::paths::home_dir;

use super::*;
use crate::dto::{AgentSessionLocations, SessionLocation};

/// The session folders of each agent, in watch-root order.
#[tauri::command]
pub async fn agent_session_locations() -> CommandResult<Vec<AgentSessionLocations>> {
    run_blocking(|| {
        Ok(match home_dir() {
            Some(home) => agent_session_locations_for_home(&home),
            None => Vec::new(),
        })
    })
    .await
}

/// The body of [`agent_session_locations`] for a given `home`.
fn agent_session_locations_for_home(home: &Path) -> Vec<AgentSessionLocations> {
    AgentKind::ALL
        .iter()
        .map(|agent| {
            let mut seen = HashSet::new();
            let locations = Explorers::DISK
                .watch_roots_for(agent, home)
                .into_iter()
                .filter(|root| seen.insert(root.path.clone()))
                .map(|root| SessionLocation {
                    path: display_under_home(&root.path, home),
                    found: root.path.exists(),
                })
                .collect();
            AgentSessionLocations {
                agent: agent.slug().to_string(),
                locations,
            }
        })
        .collect()
}

/// Replace a leading `home` with `~`, so the list does not show the account
/// name. A path outside `home` shows in full.
fn display_under_home(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(stripped) if stripped.as_os_str().is_empty() => "~".to_string(),
        Ok(stripped) => format!("~/{}", stripped.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortens_paths_under_home_and_leaves_others_full() {
        let home = tempfile::tempdir().unwrap();
        let under_home = home.path().join(".codex").join("sessions");
        assert_eq!(
            display_under_home(&under_home, home.path()),
            "~/.codex/sessions"
        );
        assert_eq!(display_under_home(home.path(), home.path()), "~");

        let outside = tempfile::tempdir().unwrap();
        assert_eq!(
            display_under_home(outside.path(), home.path()),
            outside.path().display().to_string()
        );
    }

    #[test]
    fn marks_each_root_found_and_drops_duplicates() {
        let home = tempfile::tempdir().unwrap();
        let codex_sessions = home.path().join(".codex").join("sessions");
        std::fs::create_dir_all(&codex_sessions).unwrap();

        let locations = agent_session_locations_for_home(home.path());
        let codex = locations
            .iter()
            .find(|entry| entry.agent == AgentKind::Codex.slug())
            .expect("codex is one of AgentKind::ALL");

        // `~/.codex/sessions` is always a Codex watch root, with or without
        // a `CODEX_HOME` override in this process's environment, and this
        // test created it, so it reports as found.
        let default_root = codex
            .locations
            .iter()
            .find(|location| location.path == "~/.codex/sessions")
            .expect("~/.codex/sessions is always a Codex watch root");
        assert!(default_root.found);

        let claude = locations
            .iter()
            .find(|entry| entry.agent == AgentKind::Claude.slug())
            .expect("claude is one of AgentKind::ALL");
        // None of Claude's roots were created, and none repeat.
        assert!(claude.locations.iter().all(|location| !location.found));
        let mut seen = HashSet::new();
        assert!(
            claude
                .locations
                .iter()
                .all(|location| seen.insert(location.path.clone()))
        );
    }

    #[test]
    fn covers_every_agent_exactly_once() {
        let home = tempfile::tempdir().unwrap();
        let locations = agent_session_locations_for_home(home.path());
        assert_eq!(locations.len(), AgentKind::ALL.len());
        let mut seen = HashSet::new();
        assert!(
            locations
                .iter()
                .all(|entry| seen.insert(entry.agent.clone()))
        );
    }
}
