//! The Memories view: every Claude Code auto-memory project, with usage facts
//! from the stored tool calls.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use antiburn_local::discovery::agents::path_codec::decode_hyphenated_absolute_path;
use antiburn_local::memories::inventory::{
    HookSource, MemoryProjectInventory, scan_claude_memory_projects,
};
use antiburn_local::memories::usage::{
    MemoryUsageFacts, aggregate_memory_facts, query_memory_tool_calls,
};
use antiburn_local::paths::home_dir;
use tauri::Manager;

use super::{CommandResult, fail, run_blocking};
use crate::dto::{
    AgentMemoriesReport, DanglingIndexEntryDto, MemoryEditOutcome, MemoryEntryDto, MemoryFactsDto,
    MemoryProjectDto,
};
use crate::store::Store;
use crate::store::memories::{ProjectSession, claude_sessions_by_project_slug, sessions_since};

/// What one scan reads, before the display paths are known.
struct Gathered {
    inventories: Vec<MemoryProjectInventory>,
    sessions: HashMap<String, Vec<ProjectSession>>,
    facts: HashMap<PathBuf, MemoryUsageFacts>,
}

/// Lists every project that has Claude Code memories.
///
/// The command reads through its own connection, so a slow scan never holds
/// the main store or the Overview's reader.
#[tauri::command]
pub async fn list_agent_memories(app: tauri::AppHandle) -> CommandResult<AgentMemoriesReport> {
    let Some(home) = home_dir() else {
        return Ok(build_report(Gathered::empty(), &HashMap::new(), now_ms()));
    };
    let store = app.state::<Store>().inner().clone();
    let scan_home = home.clone();
    let gathered = run_blocking(move || {
        let reader = store
            .open_reader(crate::UI_READ_STORE_BUSY_TIMEOUT)
            .map_err(fail)?;
        let inventories = scan_claude_memory_projects(&scan_home);
        let connection = reader.lock();
        let calls = query_memory_tool_calls(&connection, &scan_home).map_err(fail)?;
        let sessions = claude_sessions_by_project_slug(&connection).map_err(fail)?;
        Ok(Gathered {
            inventories,
            sessions,
            facts: aggregate_memory_facts(&calls),
        })
    })
    .await?;

    let mut display_paths = HashMap::new();
    for inventory in &gathered.inventories {
        let sessions = gathered.sessions.get(&inventory.slug);
        if most_common_cwd(sessions.map_or(&[], Vec::as_slice)).is_none()
            && let Some(decoded) = decode_hyphenated_absolute_path(&inventory.slug, &home).await
        {
            display_paths.insert(inventory.slug.clone(), decoded.display().to_string());
        }
    }
    Ok(build_report(gathered, &display_paths, now_ms()))
}

/// Moves one memory file into antiburn's archive and removes its index line.
#[tauri::command]
pub async fn archive_agent_memory(
    app: tauri::AppHandle,
    slug: String,
    file_name: String,
    expected_size_bytes: u64,
    expected_modified_ms: Option<i64>,
) -> CommandResult<MemoryEditOutcome> {
    let (home, archive_root) = edit_roots(&app)?;
    run_blocking(move || {
        crate::agent_memory::archive_memory(
            &home,
            &archive_root,
            &slug,
            &file_name,
            expected_size_bytes,
            expected_modified_ms,
        )
    })
    .await
}

/// Puts an archived memory file and its index line back.
#[tauri::command]
pub async fn restore_agent_memory(
    app: tauri::AppHandle,
    slug: String,
    archive_id: String,
) -> CommandResult<MemoryEditOutcome> {
    let (home, archive_root) = edit_roots(&app)?;
    run_blocking(move || {
        crate::agent_memory::restore_memory(&home, &archive_root, &slug, &archive_id)
    })
    .await
}

/// Removes one dangling line from a project's `MEMORY.md`.
#[tauri::command]
pub async fn remove_agent_memory_index_line(
    app: tauri::AppHandle,
    slug: String,
    line_number: usize,
    target: String,
) -> CommandResult<MemoryEditOutcome> {
    let (home, _) = edit_roots(&app)?;
    run_blocking(move || crate::agent_memory::remove_index_line(&home, &slug, line_number, &target))
        .await
}

fn edit_roots(app: &tauri::AppHandle) -> CommandResult<(PathBuf, PathBuf)> {
    let home = home_dir().ok_or_else(|| fail("The home folder is unavailable"))?;
    let archive_root = app
        .path()
        .app_data_dir()
        .map_err(fail)?
        .join("memory-archive");
    Ok((home, archive_root))
}

impl Gathered {
    fn empty() -> Self {
        Self {
            inventories: Vec::new(),
            sessions: HashMap::new(),
            facts: HashMap::new(),
        }
    }
}

fn now_ms() -> i64 {
    crate::scan::unix_now().saturating_mul(1000)
}

/// The cwd that most sessions share. A tie goes to the smaller path.
fn most_common_cwd(sessions: &[ProjectSession]) -> Option<String> {
    let mut counts: HashMap<&str, u32> = HashMap::new();
    for cwd in sessions.iter().filter_map(|session| session.cwd.as_deref()) {
        *counts.entry(cwd).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map(|(cwd, _)| cwd.to_owned())
}

fn build_report(
    gathered: Gathered,
    decoded_paths: &HashMap<String, String>,
    generated_at_ms: i64,
) -> AgentMemoriesReport {
    let Gathered {
        inventories,
        sessions,
        facts,
    } = gathered;
    let mut projects: Vec<MemoryProjectDto> = inventories
        .into_iter()
        .map(|inventory| {
            let project_sessions = sessions.get(&inventory.slug).map_or(&[][..], Vec::as_slice);
            project_dto(inventory, project_sessions, &facts, decoded_paths)
        })
        .collect();
    // `None` sorts below `Some`, so the reversed order puts it last.
    projects.sort_by(|a, b| {
        b.last_session_ms
            .cmp(&a.last_session_ms)
            .then_with(|| a.slug.cmp(&b.slug))
    });
    AgentMemoriesReport {
        generated_at_ms,
        writes_supported: cfg!(not(windows)),
        projects,
    }
}

fn project_dto(
    inventory: MemoryProjectInventory,
    sessions: &[ProjectSession],
    facts: &HashMap<PathBuf, MemoryUsageFacts>,
    decoded_paths: &HashMap<String, String>,
) -> MemoryProjectDto {
    let display_path = most_common_cwd(sessions)
        .or_else(|| decoded_paths.get(&inventory.slug).cloned())
        .unwrap_or_else(|| inventory.slug.clone());
    let memories = inventory
        .memories
        .iter()
        .map(|memory| {
            let usage = facts.get(&memory.path);
            let reference_count = usage.map_or(0, |usage| usage.reference_count);
            let write_count = usage.map_or(0, |usage| usage.write_count);
            let last_written_ms = usage.and_then(|usage| usage.last_written_ms);
            MemoryEntryDto {
                path: memory.path.display().to_string(),
                file_name: memory.file_name.clone(),
                title: memory.title.clone(),
                kind: memory.kind.map(|kind| kind.as_str().to_owned()),
                hook: memory.hook.clone(),
                hook_source: match memory.hook_source {
                    HookSource::Index => "index",
                    HookSource::Frontmatter => "frontmatter",
                    HookSource::Body => "body",
                }
                .to_owned(),
                body: memory.body.clone(),
                has_frontmatter: memory.has_frontmatter,
                truncated: memory.truncated,
                size_bytes: memory.size_bytes,
                modified_ms: memory.modified_ms,
                in_index: memory.in_index,
                facts: MemoryFactsDto {
                    last_referenced_ms: usage.and_then(|usage| usage.last_referenced_ms),
                    last_written_ms,
                    reference_count,
                    write_count,
                    sessions_since_written: last_written_ms
                        .map(|written| sessions_since(sessions, written)),
                    has_history: reference_count + write_count > 0,
                },
            }
        })
        .collect();
    MemoryProjectDto {
        slug: inventory.slug,
        display_path,
        memory_dir: path_text(&inventory.memory_dir),
        index_path: inventory.index_path.as_deref().map(path_text),
        session_count: u32::try_from(sessions.len()).unwrap_or(u32::MAX),
        last_session_ms: sessions.iter().filter_map(|session| session.last_ms).max(),
        dangling: inventory
            .dangling
            .into_iter()
            .map(|entry| DanglingIndexEntryDto {
                title: entry.title,
                target: entry.target,
                line_number: u32::try_from(entry.line_number).unwrap_or(u32::MAX),
            })
            .collect(),
        memories,
    }
}

fn path_text(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use antiburn_local::memories::inventory::{IndexEntry, MemoryFile};

    use super::*;

    fn session(id: &str, cwd: Option<&str>, started: i64, last: Option<i64>) -> ProjectSession {
        ProjectSession {
            session_id: id.into(),
            cwd: cwd.map(str::to_owned),
            started_ms: Some(started),
            last_ms: last,
        }
    }

    fn inventory(slug: &str) -> MemoryProjectInventory {
        let dir = PathBuf::from(format!("/h/.claude/projects/{slug}/memory"));
        MemoryProjectInventory {
            slug: slug.into(),
            memory_dir: dir.clone(),
            index_path: Some(dir.join("MEMORY.md")),
            entries: Vec::new(),
            dangling: vec![IndexEntry {
                title: "Gone".into(),
                target: "gone.md".into(),
                hook: None,
                line_number: 3,
            }],
            memories: vec![MemoryFile {
                path: dir.join("a.md"),
                file_name: "a.md".into(),
                title: "A".into(),
                kind: None,
                hook: Some("hook".into()),
                hook_source: HookSource::Index,
                body: "body".into(),
                has_frontmatter: true,
                truncated: false,
                size_bytes: 4,
                modified_ms: None,
                in_index: true,
            }],
        }
    }

    #[test]
    fn combines_inventory_sessions_and_facts() {
        let a = PathBuf::from("/h/.claude/projects/-new/memory/a.md");
        let gathered = Gathered {
            inventories: vec![inventory("-none"), inventory("-old"), inventory("-new")],
            sessions: HashMap::from([
                (
                    "-new".into(),
                    vec![
                        session("1", Some("/w/x"), 100, Some(900)),
                        session("2", Some("/w/y"), 600, Some(1_000)),
                        session("3", Some("/w/y"), 700, None),
                    ],
                ),
                ("-old".into(), vec![session("4", None, 10, Some(50))]),
            ]),
            facts: HashMap::from([(
                a,
                MemoryUsageFacts {
                    last_written_ms: Some(500),
                    write_count: 1,
                    ..MemoryUsageFacts::default()
                },
            )]),
        };
        let decoded = HashMap::from([("-none".to_owned(), "/decoded".to_owned())]);
        let report = build_report(gathered, &decoded, 7);

        let slugs: Vec<_> = report.projects.iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, ["-new", "-old", "-none"]);
        let new = &report.projects[0];
        assert_eq!(new.display_path, "/w/y");
        assert_eq!(new.session_count, 3);
        assert_eq!(new.last_session_ms, Some(1_000));
        let facts = &new.memories[0].facts;
        assert_eq!(facts.sessions_since_written, Some(2));
        assert!(facts.has_history);
        assert_eq!(new.memories[0].hook_source, "index");
        assert_eq!(new.dangling[0].line_number, 3);

        assert_eq!(report.projects[1].display_path, "-old");
        assert_eq!(report.projects[2].display_path, "/decoded");
        let none_facts = &report.projects[2].memories[0].facts;
        assert!(!none_facts.has_history);
        assert_eq!(none_facts.sessions_since_written, None);
    }
}
