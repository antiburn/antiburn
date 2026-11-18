//! Bounded Cloud project suggestions from all retained local session metadata.
//! Cached CWD values can be inferred or stale. They are suggestions, never native
//! upload proof. Queries do not read or copy cached messages or source bodies.

use crate::store::Store;
use anyhow::{Result, ensure};
use rusqlite::params;
use serde::{Deserialize, Serialize};

/// Keyset position across execution environments and recorded CWD suggestions.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Cursor {
    pub environment: String,
    pub cwd: String,
}

/// One cached folder suggestion across all agents. Its environment namespace
/// prevents a remote or WSL path from being treated as a local folder.
#[derive(Clone, Debug)]
pub(crate) struct Suggestion {
    pub environment: String,
    pub cwd: String,
    pub agents: Vec<String>,
    pub session_count: u64,
}

impl Store {
    /// Page over every retained CWD, without a recency window or repository cap.
    /// Run in admitted blocking work. At most 256 small metadata rows are returned.
    pub(crate) fn cloud_project_suggestions_page(
        &self,
        after: Option<&Cursor>,
        limit: usize,
    ) -> Result<Vec<Suggestion>> {
        ensure!(limit > 0 && limit <= 256, "Choose a bounded project page");
        let (environment, cwd) = after.map_or(("", ""), |cursor| {
            (cursor.environment.as_str(), cursor.cwd.as_str())
        });
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT environment_key, cwd, json_group_array(DISTINCT agent), count(*)
             FROM session
             WHERE cwd IS NOT NULL AND cwd <> '' AND length(CAST(cwd AS BLOB)) <= 4096
               AND (environment_key, cwd) > (?1, ?2)
             GROUP BY environment_key, cwd
             ORDER BY environment_key, cwd
             LIMIT ?3",
        )?;
        let rows = statement.query_map(params![environment, cwd, limit as u32], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u64>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (environment, cwd, agents, session_count) = row?;
            let agents: Vec<String> = serde_json::from_str(&agents)?;
            ensure!(
                environment.len() <= 256
                    && agents.len() <= 64
                    && agents.iter().all(|agent| agent.len() <= 64),
                "Project metadata exceeds its limit"
            );
            Ok(Suggestion {
                environment,
                cwd,
                agents,
                session_count,
            })
        })
        .collect()
    }

    /// Reuse cached native Claude source paths without enumerating provider
    /// directories. The capture library must still qualify each source root.
    pub(crate) fn cloud_claude_source_paths_page(
        &self,
        after: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        ensure!(limit > 0 && limit <= 512, "Choose a bounded source page");
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT DISTINCT source_label FROM session
             WHERE environment_key='native' AND wsl_distro IS NULL
               AND agent='claude' AND source_kind='file' AND source_label > ?1
               AND length(CAST(source_label AS BLOB)) <= 4096
             ORDER BY source_label LIMIT ?2",
        )?;
        let rows = statement.query_map(params![after, limit as u32], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
    }
}

#[cfg(test)]
#[path = "tests/cloud_projects_integration_tests.rs"]
mod tests;
