//! Claude Code sessions grouped by project folder, for the Memories view.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;

/// The folder name after `.claude/projects/` in a transcript path.
const PROJECTS_MARKER: &str = ".claude/projects/";

/// One Claude Code session in a project folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectSession {
    pub session_id: String,
    pub cwd: Option<String>,
    pub started_ms: Option<i64>,
    pub last_ms: Option<i64>,
}

/// Groups every Claude Code session by the project slug in its transcript
/// path. A session whose path has no slug is skipped.
pub(crate) fn claude_sessions_by_project_slug(
    conn: &Connection,
) -> Result<HashMap<String, Vec<ProjectSession>>> {
    let mut statement = conn.prepare(
        "SELECT s.session_id, s.source_label, s.cwd, s.started_at_epoch,
                MIN(t.ts_ms), MAX(t.ts_ms)
           FROM session AS s
           LEFT JOIN turn AS t
             ON t.environment_key = s.environment_key
            AND t.agent = s.agent
            AND t.session_id = s.session_id
          WHERE s.agent = 'claude-code'
            AND s.environment_key NOT LIKE 'ssh:%'
          GROUP BY s.environment_key, s.agent, s.session_id",
    )?;
    let mut rows = statement.query([])?;
    let mut by_slug: HashMap<String, Vec<ProjectSession>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let source_label: String = row.get(1)?;
        let Some(slug) = project_slug(&source_label) else {
            continue;
        };
        let started_epoch: Option<i64> = row.get(3)?;
        let first_turn_ms: Option<i64> = row.get(4)?;
        by_slug.entry(slug).or_default().push(ProjectSession {
            session_id: row.get(0)?,
            cwd: row.get(2)?,
            started_ms: started_epoch
                .and_then(|seconds| seconds.checked_mul(1000))
                .or(first_turn_ms),
            last_ms: row.get(5)?,
        });
    }
    Ok(by_slug)
}

/// Counts the sessions that start after `after_ms`.
pub(crate) fn sessions_since(sessions: &[ProjectSession], after_ms: i64) -> u32 {
    let count = sessions
        .iter()
        .filter(|session| session.started_ms.is_some_and(|started| started > after_ms))
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

fn project_slug(source_label: &str) -> Option<String> {
    let normalised = source_label.replace('\\', "/");
    let rest = normalised.split_once(PROJECTS_MARKER)?.1;
    let slug = rest.split('/').next()?;
    (!slug.is_empty()).then(|| slug.to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::store::{SessionKey, SessionRecord, Store};

    fn record(session_id: &str, source_label: &str, cwd: Option<&str>) -> SessionRecord {
        SessionRecord {
            key: SessionKey::new("native", "claude-code", session_id),
            source_kind: "file".into(),
            source_label: source_label.into(),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: cwd.map(str::to_owned),
            surface: "cli".into(),
            updated_at_epoch: None,
            activity_cursor: String::new(),
            activity_source: "mtime".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: None,
        }
    }

    fn add_turn(conn: &Connection, rowid: i64, session_id: &str, ts_ms: i64) {
        conn.execute(
            "INSERT INTO turn (rowid, environment_key, agent, session_id, claim_fence,
                source_key, thread_id, turn_index, scope, role, ts_ms, input_tokens,
                cache_read_tokens, cache_write_tokens, output_tokens, is_compaction_boundary)
             VALUES (?1, 'native', 'claude-code', ?2, 1, 'src', 'th', ?1, 'main',
                'assistant', ?3, 0, 0, 0, 0, 0)",
            (rowid, session_id, ts_ms),
        )
        .unwrap();
    }

    #[test]
    fn groups_sessions_by_slug_and_derives_start_and_last_activity() {
        let store = Store::open_in_memory(Path::new("/tmp/antiburn-test-state")).unwrap();
        store
            .upsert_sessions(
                &[
                    record(
                        "a",
                        "/h/.claude/projects/-work-app/a.jsonl",
                        Some("/work/app"),
                    ),
                    record(
                        "b",
                        "C:\\Users\\x\\.claude\\projects\\-work-app\\b.jsonl",
                        None,
                    ),
                    record("c", "/h/.claude/projects/-other/c.jsonl", Some("/other")),
                    record("d", "/elsewhere/d.jsonl", None),
                ],
                &[],
            )
            .unwrap();
        let conn = store.lock();
        add_turn(&conn, 1, "a", 5_000);
        add_turn(&conn, 2, "a", 9_000);
        add_turn(&conn, 3, "b", 20_000);
        conn.execute(
            "UPDATE session SET started_at_epoch = 7 WHERE session_id = 'c'",
            [],
        )
        .unwrap();

        let by_slug = claude_sessions_by_project_slug(&conn).unwrap();
        assert_eq!(by_slug.len(), 2);
        let mut app = by_slug["-work-app"].clone();
        app.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        assert_eq!(app[0].started_ms, Some(5_000));
        assert_eq!(app[0].last_ms, Some(9_000));
        assert_eq!(app[0].cwd.as_deref(), Some("/work/app"));
        assert_eq!(app[1].started_ms, Some(20_000));
        assert_eq!(by_slug["-other"][0].started_ms, Some(7_000));
        assert_eq!(by_slug["-other"][0].last_ms, None);

        assert_eq!(sessions_since(&app, 5_000), 1);
        assert_eq!(sessions_since(&app, 0), 2);
        assert_eq!(sessions_since(&by_slug["-other"], 7_000), 0);
    }
}
