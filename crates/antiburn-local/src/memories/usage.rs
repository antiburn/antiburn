//! Which memory files the agent named in tool calls.
//!
//! The query reads tool inputs that the turn store already holds. It finds a
//! reference only when the path text appears in the input. A transcript that
//! Claude Code pruned leaves no record, so a missing fact means unknown.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;

const MEMORY_MARKER: &str = ".claude/projects/";

/// How a tool call touched a memory file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryAction {
    /// A `Read` call or a `Bash` command named the path.
    Referenced,
    /// A `Write`, `Edit`, or `MultiEdit` call targeted the path.
    Written,
}

/// One memory path named by one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryToolCall {
    pub path: PathBuf,
    pub slug: String,
    pub action: MemoryAction,
    pub session_id: String,
    pub environment_key: String,
    pub ts_ms: Option<i64>,
    /// The call ran in a delegated (subagent) scope.
    pub delegated: bool,
}

/// Usage facts for one memory path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryUsageFacts {
    pub last_referenced_ms: Option<i64>,
    pub last_written_ms: Option<i64>,
    pub reference_count: u32,
    pub write_count: u32,
    pub session_ids: BTreeSet<String>,
}

/// Returns every memory path that a Claude Code tool input names.
pub fn query_memory_tool_calls(
    conn: &Connection,
    home: &Path,
) -> rusqlite::Result<Vec<MemoryToolCall>> {
    // The `kind` and `tool_name` terms come first so that SQLite never reads
    // `tool_result` content.
    let mut statement = conn.prepare(
        "SELECT t.environment_key, t.session_id, t.ts_ms, t.scope,
                c.tool_name, c.content, c.normalized_fields_json
           FROM turn_content AS c
           JOIN turn AS t ON t.rowid = c.turn_rowid
          WHERE c.kind = 'tool_input'
            AND t.agent = 'claude-code'
            AND c.tool_name IN ('Read', 'Write', 'Edit', 'MultiEdit', 'Bash')
            AND (instr(c.normalized_fields_json, ?1) > 0
                 OR instr(c.content, ?1) > 0)",
    )?;
    let mut rows = statement.query([MEMORY_MARKER])?;
    let mut calls = Vec::new();
    while let Some(row) = rows.next()? {
        let environment_key: String = row.get(0)?;
        let session_id: String = row.get(1)?;
        let ts_ms: Option<i64> = row.get(2)?;
        let delegated = row.get::<_, String>(3)? == "delegated";
        let tool_name: String = row.get(4)?;
        let content: Vec<u8> = row.get(5)?;
        let fields: Option<String> = row.get(6)?;
        let content = String::from_utf8_lossy(&content);

        let (raw_paths, action) = if tool_name == "Bash" {
            (bash_paths(&content), MemoryAction::Referenced)
        } else if tool_name == "Read" {
            (
                file_paths(fields.as_deref(), &content, "read_file_path"),
                MemoryAction::Referenced,
            )
        } else {
            (
                file_paths(fields.as_deref(), &content, "file_edit_path"),
                MemoryAction::Written,
            )
        };
        let mut seen = BTreeSet::new();
        for raw in raw_paths {
            let Some((path, slug)) = normalise(&raw, home) else {
                continue;
            };
            if !seen.insert(path.clone()) {
                continue;
            }
            calls.push(MemoryToolCall {
                path,
                slug,
                action,
                session_id: session_id.clone(),
                environment_key: environment_key.clone(),
                ts_ms,
                delegated,
            });
        }
    }
    Ok(calls)
}

/// Folds calls into per-path facts. A call with no timestamp counts but does
/// not move a `last_*` value.
pub fn aggregate_memory_facts(calls: &[MemoryToolCall]) -> HashMap<PathBuf, MemoryUsageFacts> {
    let mut facts: HashMap<PathBuf, MemoryUsageFacts> = HashMap::new();
    for call in calls {
        let fact = facts.entry(call.path.clone()).or_default();
        fact.session_ids.insert(call.session_id.clone());
        let (count, last) = match call.action {
            MemoryAction::Referenced => (&mut fact.reference_count, &mut fact.last_referenced_ms),
            MemoryAction::Written => (&mut fact.write_count, &mut fact.last_written_ms),
        };
        *count = count.saturating_add(1);
        if let Some(ts_ms) = call.ts_ms {
            *last = Some(last.map_or(ts_ms, |current| current.max(ts_ms)));
        }
    }
    facts
}

/// Paths from the normalized field, else from `file_path` in the raw input.
fn file_paths(fields: Option<&str>, content: &str, field: &str) -> Vec<String> {
    let from_fields = fields
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|value| value.pointer(&format!("/values/{field}")).cloned())
        .map(|value| match value {
            // The store wraps the value as JSON text: `{"paths":[...]}`.
            Value::String(text) => serde_json::from_str(&text).unwrap_or(Value::Null),
            other => other,
        })
        .and_then(|value| {
            value.get("paths").and_then(Value::as_array).map(|paths| {
                paths
                    .iter()
                    .filter_map(|path| path.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
        });
    if let Some(paths) = from_fields {
        return paths;
    }
    serde_json::from_str::<Value>(content)
        .ok()
        .and_then(|value| value.get("file_path")?.as_str().map(str::to_owned))
        .into_iter()
        .collect()
}

/// A character that ends a path inside a shell command.
fn ends_path(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | '\\' | ';' | '|' | ')' | '>')
}

/// Path-like text around every `.claude/projects/` in a raw Bash input.
fn bash_paths(content: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for (index, _) in content.match_indices(MEMORY_MARKER) {
        let before = &content[..index];
        let start = before.rfind(ends_path).map_or(0, |at| {
            at + before[at..].chars().next().map_or(1, char::len_utf8)
        });
        let prefix = &before[start..];
        if prefix != "~/" && !prefix.starts_with('/') {
            continue;
        }
        let tail = &content[index..];
        let end = tail.find(ends_path).unwrap_or(tail.len());
        let path = format!("{prefix}{}", &tail[..end]);
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// Expands `~` and keeps only `home/.claude/projects/<slug>/memory/**/*.md`.
fn normalise(raw: &str, home: &Path) -> Option<(PathBuf, String)> {
    let path = match raw.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(raw),
    };
    let rest = path
        .strip_prefix(home.join(".claude").join("projects"))
        .ok()?;
    let mut parts = rest.components();
    let slug = parts.next()?.as_os_str().to_str()?.to_owned();
    if parts.next()?.as_os_str() != "memory" {
        return None;
    }
    let mut tail = parts.peekable();
    tail.peek()?;
    if !tail.all(|part| matches!(part, std::path::Component::Normal(_))) {
        return None;
    }
    (path.extension()? == "md").then_some((path, slug))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::TURN_MIGRATIONS;

    const HOME: &str = "/Users/dev";
    const MEM: &str = "/Users/dev/.claude/projects/-work-app/memory";

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                environment_key TEXT NOT NULL, agent TEXT NOT NULL, session_id TEXT NOT NULL,
                PRIMARY KEY (environment_key, agent, session_id)) STRICT;",
        )
        .unwrap();
        for migration in TURN_MIGRATIONS {
            conn.execute_batch(migration).unwrap();
        }
        conn
    }

    struct Seed<'a> {
        agent: &'a str,
        session: &'a str,
        scope: &'a str,
        ts_ms: Option<i64>,
        kind: &'a str,
        tool: &'a str,
        content: String,
        fields: Option<String>,
    }

    impl<'a> Seed<'a> {
        fn new(tool: &'a str, content: String) -> Self {
            Self {
                agent: "claude-code",
                session: "s1",
                scope: "main",
                ts_ms: Some(1_000),
                kind: "tool_input",
                tool,
                content,
                fields: None,
            }
        }

        fn insert(self, conn: &Connection, index: usize) {
            conn.execute(
                "INSERT OR IGNORE INTO session VALUES ('native', ?1, ?2)",
                (self.agent, self.session),
            )
            .unwrap();
            conn.execute(
                "INSERT INTO turn (rowid, environment_key, agent, session_id, claim_fence,
                    source_key, thread_id, turn_index, scope, role, ts_ms, input_tokens,
                    cache_read_tokens, cache_write_tokens, output_tokens, is_compaction_boundary)
                 VALUES (?1, 'native', ?2, ?3, 1, 'src', 'th', ?1, ?4, 'assistant', ?5, 0, 0, 0, 0, 0)",
                (index as i64, self.agent, self.session, self.scope, self.ts_ms),
            )
            .unwrap();
            conn.execute(
                "INSERT INTO turn_content (turn_rowid, part_index, kind, content, truncated,
                    authority, tool_name, normalized_fields_json)
                 VALUES (?1, 0, ?2, CAST(?3 AS BLOB), 0, 'assistant', ?4, ?5)",
                (
                    index as i64,
                    self.kind,
                    self.content,
                    self.tool,
                    self.fields,
                ),
            )
            .unwrap();
        }
    }

    fn wrapped(field: &str, path: &str) -> Option<String> {
        let inner = serde_json::json!({ "paths": [path] }).to_string();
        Some(serde_json::json!({ "values": { field: inner } }).to_string())
    }

    fn bash(command: &str) -> Seed<'static> {
        Seed::new(
            "Bash",
            serde_json::json!({ "command": command }).to_string(),
        )
    }

    #[test]
    fn extracts_calls_and_aggregates_facts() {
        let conn = connection();
        let a = format!("{MEM}/a.md");
        let b = format!("{MEM}/b.md");
        let mut seeds = Vec::new();

        let mut read = Seed::new("Read", "{}".into());
        read.fields = wrapped("read_file_path", &a);
        read.ts_ms = Some(100);
        seeds.push(read);

        let mut cat = bash(&format!("cat {a}"));
        cat.ts_ms = Some(300);
        seeds.push(cat);

        let mut two = bash(&format!("cat {a} {b} | head; echo '{MEM}/MEMORY.md'"));
        two.ts_ms = Some(200);
        seeds.push(two);

        let mut write = Seed::new("Write", "{}".into());
        write.fields = wrapped("file_edit_path", &a);
        write.ts_ms = Some(400);
        seeds.push(write);

        let mut edit = Seed::new("Edit", serde_json::json!({ "file_path": a }).to_string());
        edit.ts_ms = Some(500);
        edit.session = "s2";
        seeds.push(edit);

        let mut delegated = bash(&format!("sed -n 1,5p {b}"));
        delegated.scope = "delegated";
        delegated.ts_ms = Some(600);
        seeds.push(delegated);

        let mut null_ts = bash(&format!("cat {b}"));
        null_ts.ts_ms = None;
        seeds.push(null_ts);

        seeds.push(bash("cat ~/.claude/projects/-work-app/memory/tilde.md"));
        seeds.push(bash(
            "cat /etc/hosts /Users/dev/.claude/projects/-work-app/x.md",
        ));
        seeds.push(bash(&format!("cat {MEM}/notes.txt")));
        seeds.push(bash("cat $HOME/.claude/projects/-work-app/memory/env.md"));

        let mut result_row = Seed::new("Read", a.clone());
        result_row.kind = "tool_result";
        seeds.push(result_row);

        let mut other_agent = bash(&format!("cat {a}"));
        other_agent.agent = "codex";
        seeds.push(other_agent);

        for (index, seed) in seeds.into_iter().enumerate() {
            seed.insert(&conn, index + 1);
        }

        let calls = query_memory_tool_calls(&conn, Path::new(HOME)).unwrap();
        let facts = aggregate_memory_facts(&calls);

        let fact_a = &facts[Path::new(&a)];
        assert_eq!(fact_a.reference_count, 3);
        assert_eq!(fact_a.write_count, 2);
        assert_eq!(fact_a.last_referenced_ms, Some(300));
        assert_eq!(fact_a.last_written_ms, Some(500));
        assert_eq!(fact_a.session_ids.len(), 2);

        let fact_b = &facts[Path::new(&b)];
        assert_eq!(fact_b.reference_count, 3);
        assert_eq!(fact_b.last_referenced_ms, Some(600));
        assert_eq!(fact_b.last_written_ms, None);
        assert!(
            calls
                .iter()
                .any(|call| call.path == Path::new(&b) && call.delegated)
        );
        assert!(
            calls
                .iter()
                .any(|call| call.path == Path::new(&b) && call.ts_ms.is_none())
        );

        let tilde = Path::new(MEM).join("tilde.md");
        assert_eq!(facts[&tilde].reference_count, 1);
        assert!(calls.iter().all(|call| call.slug == "-work-app"));
        // MEMORY.md counts as a path. The three rejected paths do not.
        let mut keys: Vec<_> = facts
            .keys()
            .map(|key| key.file_name().unwrap().to_str().unwrap())
            .collect();
        keys.sort();
        assert_eq!(keys, ["MEMORY.md", "a.md", "b.md", "tilde.md"]);
    }
}
