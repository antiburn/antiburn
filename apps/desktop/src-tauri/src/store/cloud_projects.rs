//! Bounded Cloud project suggestions from all retained local session metadata.
//! Cached CWD values can be inferred or stale. They are suggestions, never native
//! upload proof. Queries do not read or copy cached messages or source bodies.

use crate::store::Store;
use anyhow::{Result, ensure};
use rusqlite::params;
use serde::{Deserialize, Serialize};

const OPENCODE_HINTS: &str = "SELECT session_id FROM session
    WHERE environment_key='native' AND wsl_distro IS NULL
      AND agent='opencode' AND source_kind='providerDb'
      AND source_format='\"open_code_v2_sqlite\"'
      AND source_label='opencode:' || session_id || ':open_code_v2_sqlite'
      AND length(CAST(session_id AS BLOB)) BETWEEN 1 AND 128
      AND cwd=?1
    ORDER BY session_id COLLATE BINARY LIMIT ?2";
const SUGGESTION_PAGE: &str =
    "SELECT environment_key, cwd, json_group_array(DISTINCT agent), count(*)
    FROM session
    WHERE cwd IS NOT NULL AND cwd <> '' AND length(CAST(cwd AS BLOB)) <= 4096
      AND (environment_key, cwd) > (?1, ?2)
    GROUP BY environment_key, cwd
    ORDER BY environment_key, cwd
    LIMIT ?3";
const CLAUDE_SOURCE_PAGE: &str = "SELECT DISTINCT source_label FROM session
    WHERE environment_key='native' AND wsl_distro IS NULL
      AND agent='claude-code' AND source_kind='file' AND source_label > ?1
      AND length(CAST(source_label AS BLOB)) <= 4096
    ORDER BY source_label LIMIT ?2";

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
    /// Read bounded native-ID hints for displayed or selected local OpenCode folders.
    /// Give each folder a share so one large project cannot hide the rest.
    /// These IDs grant no database or CWD permission. Reopen each ID in an
    /// authorized source and check its current recorded CWD before use.
    pub(crate) fn cloud_opencode_session_ids(
        &self,
        cwds: &[String],
        limit: usize,
    ) -> Result<Vec<String>> {
        ensure!(
            cwds.len() <= 100
                && cwds.iter().all(|cwd| cwd.len() <= 4096)
                && limit > 0
                && limit <= 512,
            "Choose bounded cached project metadata"
        );
        let connection = self.lock();
        let mut statement = connection.prepare(OPENCODE_HINTS)?;
        let mut ids = Vec::new();
        for (index, cwd) in cwds.iter().enumerate() {
            let remaining = limit - ids.len();
            if remaining == 0 {
                break;
            }
            let share = remaining.div_ceil(cwds.len() - index);
            let rows = statement.query_map(params![cwd, share as u32], |row| row.get(0))?;
            ids.extend(rows.collect::<rusqlite::Result<Vec<String>>>()?);
        }
        Ok(ids)
    }

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
        let mut statement = connection.prepare(SUGGESTION_PAGE)?;
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
        let mut statement = connection.prepare(CLAUDE_SOURCE_PAGE)?;
        let rows = statement.query_map(params![after, limit as u32], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
    }
}

#[cfg(test)]
#[path = "tests/cloud_projects_integration_tests.rs"]
mod tests;
