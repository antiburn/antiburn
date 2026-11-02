//! The Memories view: every Claude Code auto-memory project, with usage facts
//! from the stored tool calls.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use antiburn_local::discovery::agents::path_codec::{
    decode_hyphenated_absolute_path, initial_hyphenated_path_state,
};
use antiburn_local::memories::inventory::{
    HookSource, MemoryProjectInventory, count_claude_memories, scan_claude_memory_project,
    scan_claude_memory_projects,
};
use antiburn_local::memories::usage::{
    MemoryAction, MemoryUsageFacts, aggregate_memory_facts, query_memory_tool_calls,
    query_memory_tool_calls_for_session,
};
use antiburn_local::paths::home_dir;
use tauri::Manager;

use super::{CommandResult, fail, run_blocking};
use crate::dto::{
    AgentMemoriesReport, DanglingIndexEntryDto, MemoryEditOutcome, MemoryEntryDto, MemoryFactsDto,
    MemoryIndexEntryDto, MemoryProjectDto, SessionMemoriesPayload, SessionMemoriesRequest,
    SessionMemoryTouchDto,
};
use crate::store::memories::{ProjectSession, claude_sessions_by_project_slug, sessions_since};
use crate::store::{SessionKey, Store};

/// How the display path of one project was found.
#[derive(Debug, Clone, Default)]
struct ProjectFolder {
    /// A path from the slug, used when no session recorded a cwd.
    decoded: Option<String>,
    /// The folder exists on disk now.
    exists: bool,
}

/// The most checks that the best-effort slug decoder makes on the file system.
const BEST_EFFORT_MAX_CHECKS: usize = 64;

async fn is_directory(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
}

/// Decodes a slug whose project folder is gone. The walk keeps the longest
/// existing directories and appends the rest as one name, so
/// `-home-dev-ai-barometer` becomes `/home/dev/ai-barometer` when only
/// `/home/dev` exists. The walk stays inside `home_boundary`.
async fn best_effort_decode_slug(slug: &str, home_boundary: &Path) -> Option<String> {
    let trimmed = slug.strip_prefix('-').unwrap_or(slug);
    let (mut current, offset) = initial_hyphenated_path_state(trimmed);
    let pieces: Vec<&str> = trimmed[offset..].split('-').collect();
    let mut pending = String::new();
    let mut matched = false;
    let mut checks = 0usize;
    for (index, piece) in pieces.iter().enumerate() {
        if !pending.is_empty() || index > 0 && pieces[index - 1].is_empty() {
            pending.push('-');
        }
        pending.push_str(piece);
        if index + 1 == pieces.len() || pending.is_empty() {
            continue;
        }
        let candidate = current.join(&pending);
        if !(candidate.starts_with(home_boundary) || home_boundary.starts_with(&candidate)) {
            continue;
        }
        checks += 1;
        if checks > BEST_EFFORT_MAX_CHECKS {
            return None;
        }
        if is_directory(&candidate).await {
            current = candidate;
            pending.clear();
            matched = true;
        }
    }
    if !matched {
        return None;
    }
    if !pending.is_empty() {
        current.push(&pending);
    }
    Some(current.display().to_string())
}

/// Runs the exact decoder. It needs a leading hyphen, so a Windows slug such
/// as `C--Users-x-app` gets one first.
async fn decode_slug_exact(slug: &str, home: &Path) -> Option<PathBuf> {
    if let Some(decoded) = decode_hyphenated_absolute_path(slug, home).await {
        return Some(decoded);
    }
    if slug.starts_with('-') {
        return None;
    }
    decode_hyphenated_absolute_path(&format!("-{slug}"), home).await
}

/// What one scan reads, before the display paths are known.
struct Gathered {
    inventories: Vec<MemoryProjectInventory>,
    sessions: HashMap<String, Vec<ProjectSession>>,
    facts: HashMap<PathBuf, MemoryUsageFacts>,
}

/// Counts memory files without loading their contents or session history.
#[tauri::command]
pub async fn count_agent_memories() -> CommandResult<usize> {
    let Some(home) = home_dir() else {
        return Ok(0);
    };
    run_blocking(move || count_claude_memories(&home).map_err(fail)).await
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
        let folder = if let Some(cwd) = most_common_cwd(sessions.map_or(&[], Vec::as_slice)) {
            ProjectFolder {
                decoded: None,
                exists: is_directory(Path::new(&cwd)).await,
            }
        } else if let Some(decoded) = decode_slug_exact(&inventory.slug, &home).await {
            ProjectFolder {
                exists: is_directory(&decoded).await,
                decoded: Some(decoded.display().to_string()),
            }
        } else {
            ProjectFolder {
                decoded: best_effort_decode_slug(&inventory.slug, &home).await,
                exists: false,
            }
        };
        display_paths.insert(inventory.slug.clone(), folder);
    }
    Ok(build_report(gathered, &display_paths, now_ms()))
}

/// Lists the memories that one session read or wrote.
///
/// The command reads through its own connection, like [`list_agent_memories`].
#[tauri::command]
pub async fn get_session_memories(
    app: tauri::AppHandle,
    request: SessionMemoriesRequest,
) -> CommandResult<SessionMemoriesPayload> {
    let Some(home) = home_dir() else {
        return Ok(SessionMemoriesPayload::default());
    };
    let store = app.state::<Store>().inner().clone();
    run_blocking(move || {
        let reader = store
            .open_reader(crate::UI_READ_STORE_BUSY_TIMEOUT)
            .map_err(fail)?;
        session_memories_for_store(&reader, &home, request)
    })
    .await
}

/// [`get_session_memories`]'s body, over a borrowed [`Store`] and home folder,
/// so a test can run it without a Tauri app.
fn session_memories_for_store(
    store: &Store,
    home: &Path,
    request: SessionMemoriesRequest,
) -> CommandResult<SessionMemoriesPayload> {
    let key = SessionKey::for_origin(
        &request.agent,
        &request.session_id,
        request.wsl_distro.as_deref(),
        request.remote_host_id.as_deref(),
    )
    .map_err(str::to_owned)?;
    if key.remote_host_id().is_some() || key.agent != "claude-code" {
        return Ok(SessionMemoriesPayload::default());
    }
    let calls = {
        let connection = store.lock();
        query_memory_tool_calls_for_session(
            &connection,
            home,
            &key.environment_key,
            &key.session_id,
        )
        .map_err(fail)?
    };

    // One row per (path, action).
    let mut grouped: HashMap<(PathBuf, bool), SessionMemoryTouchDto> = HashMap::new();
    let mut titles: HashMap<String, HashMap<PathBuf, String>> = HashMap::new();
    for call in calls {
        let written = call.action == MemoryAction::Written;
        let entry = grouped
            .entry((call.path.clone(), written))
            .or_insert_with(|| {
                let file_name = call
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                SessionMemoryTouchDto {
                    slug: call.slug.clone(),
                    path: path_text(&call.path),
                    file_name,
                    title: String::new(),
                    action: if written { "written" } else { "referenced" }.to_owned(),
                    count: 0,
                    last_ms: None,
                    exists: call.path.is_file(),
                }
            });
        entry.count = entry.count.saturating_add(1);
        entry.last_ms = entry.last_ms.max(call.ts_ms);
        titles.entry(call.slug).or_default();
    }
    for (slug, map) in &mut titles {
        if let Some(project) = scan_claude_memory_project(home, slug) {
            for memory in project.memories {
                map.insert(memory.path, memory.title);
            }
        }
    }
    let mut entries: Vec<SessionMemoryTouchDto> = grouped
        .into_iter()
        .map(|((path, _), mut entry)| {
            entry.title = titles
                .get(&entry.slug)
                .and_then(|map| map.get(&path))
                .cloned()
                .unwrap_or_else(|| {
                    path.file_stem()
                        .map(|stem| stem.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
            entry
        })
        .collect();
    // `None` sorts below `Some`, so the reversed order puts it last.
    entries.sort_by(|a, b| {
        b.last_ms
            .cmp(&a.last_ms)
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.action.cmp(&b.action))
    });
    Ok(SessionMemoriesPayload { entries })
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
    decoded_paths: &HashMap<String, ProjectFolder>,
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
    decoded_paths: &HashMap<String, ProjectFolder>,
) -> MemoryProjectDto {
    let folder = decoded_paths
        .get(&inventory.slug)
        .cloned()
        .unwrap_or_default();
    let display_path = most_common_cwd(sessions)
        .or(folder.decoded)
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
                index_entry: memory
                    .index_entry
                    .as_ref()
                    .map(|entry| MemoryIndexEntryDto {
                        title: entry.title.clone(),
                        hook: entry.hook.clone(),
                        line_number: u32::try_from(entry.line_number).unwrap_or(u32::MAX),
                    }),
                frontmatter: memory.frontmatter.clone(),
                body: memory.body.clone(),
                truncated: memory.truncated,
                size_bytes: memory.size_bytes,
                modified_ms: memory.modified_ms,
                in_index: memory.index_entry.is_some(),
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
        agent: "claude-code".to_string(),
        slug: inventory.slug,
        display_path,
        folder_exists: folder.exists,
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
    #[cfg(not(windows))]
    use crate::store::SessionRecord;

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
                frontmatter: None,
                body: "body".into(),
                truncated: false,
                size_bytes: 4,
                modified_ms: None,
                index_entry: Some(IndexEntry {
                    title: "A".into(),
                    target: "a.md".into(),
                    hook: Some("hook".into()),
                    line_number: 1,
                }),
            }],
        }
    }

    #[tokio::test]
    async fn best_effort_decode_joins_the_missing_tail_with_hyphens() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("dev")).unwrap();
        let slug = format!("{}-ai-barometer", home.path().join("dev").display()).replace('/', "-");
        let decoded = best_effort_decode_slug(&slug, home.path()).await;
        assert_eq!(
            decoded,
            Some(
                home.path()
                    .join("dev")
                    .join("ai-barometer")
                    .display()
                    .to_string()
            )
        );
    }

    #[tokio::test]
    async fn best_effort_decode_returns_none_when_nothing_exists() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            best_effort_decode_slug("-nowhere-at-all", home.path()).await,
            None
        );
        assert_eq!(best_effort_decode_slug("plain", home.path()).await, None);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn best_effort_decode_ignores_drive_slugs_on_posix() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(
            best_effort_decode_slug("C--Users-dev-gone", home.path()).await,
            None
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn best_effort_decode_handles_drive_slugs_on_windows() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("dev")).unwrap();
        let target = home.path().join("dev").join("gone");
        // Only the drive colon and the separators are encoded. The temp path
        // can hold `~` and `.`, which the walk must find on disk as they are.
        let slug = target.display().to_string().replace(['\\', ':'], "-");
        assert_eq!(
            best_effort_decode_slug(&slug, home.path()).await,
            Some(target.display().to_string())
        );
    }

    #[tokio::test]
    async fn best_effort_decode_does_not_probe_outside_home() {
        let base = tempfile::tempdir().unwrap();
        let home = base.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(base.path().join("outside").join("child")).unwrap();
        let slug = format!("{}-outside-child", base.path().display()).replace('/', "-");
        // The walk skips `outside` because it is neither inside home nor above it.
        assert_eq!(
            best_effort_decode_slug(&slug, &home).await,
            Some(base.path().join("outside-child").display().to_string())
        );
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
        let decoded = HashMap::from([(
            "-none".to_owned(),
            ProjectFolder {
                decoded: Some("/decoded".to_owned()),
                exists: false,
            },
        )]);
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
        assert!(!report.projects[2].folder_exists);
        let none_facts = &report.projects[2].memories[0].facts;
        assert!(!none_facts.has_history);
        assert_eq!(none_facts.sessions_since_written, None);
    }

    #[cfg(not(windows))]
    fn memories_request(session_id: &str, remote: Option<&str>) -> SessionMemoriesRequest {
        SessionMemoriesRequest {
            agent: "claude-code".into(),
            session_id: session_id.into(),
            wsl_distro: None,
            remote_host_id: remote.map(str::to_owned),
        }
    }

    #[cfg(not(windows))]
    fn add_tool_input(conn: &rusqlite::Connection, rowid: i64, session: &str, ts: i64, cmd: &str) {
        conn.execute(
            "INSERT INTO turn (rowid, environment_key, agent, session_id, claim_fence,
                source_key, thread_id, turn_index, scope, role, ts_ms, input_tokens,
                cache_read_tokens, cache_write_tokens, output_tokens, is_compaction_boundary)
             VALUES (?1, 'native', 'claude-code', ?2, 1, 'src', 'th', ?1, 'main',
                'assistant', ?3, 0, 0, 0, 0, 0)",
            (rowid, session, ts),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO turn_content (turn_rowid, part_index, kind, content, truncated,
                authority, tool_name, normalized_fields_json)
             VALUES (?1, 0, 'tool_input', CAST(?2 AS BLOB), 0, 'assistant', 'Bash', NULL)",
            (rowid, serde_json::json!({ "command": cmd }).to_string()),
        )
        .unwrap();
    }

    // The memory path scanner matches POSIX paths only.
    #[cfg(not(windows))]
    #[test]
    fn session_memories_group_by_path_and_action_with_titles() {
        let home = tempfile::tempdir().unwrap();
        let memory = home.path().join(".claude/projects/-work-app/memory");
        std::fs::create_dir_all(&memory).unwrap();
        std::fs::write(memory.join("a.md"), "---\nname: Alpha\n---\nbody\n").unwrap();
        let a = memory.join("a.md");
        let gone = memory.join("gone.md");
        let store = Store::open_in_memory(Path::new("/tmp/antiburn-memories-test")).unwrap();
        let record = |id: &str| SessionRecord {
            key: SessionKey::new("native", "claude-code", id),
            source_kind: "file".into(),
            source_label: format!("/h/.claude/projects/-work-app/{id}.jsonl"),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: None,
            surface: "cli".into(),
            updated_at_epoch: None,
            activity_cursor: String::new(),
            activity_source: "mtime".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: None,
        };
        store
            .upsert_sessions(&[record("s1"), record("s2")], &[])
            .unwrap();
        {
            let conn = store.lock();
            add_tool_input(&conn, 1, "s1", 100, &format!("cat {}", a.display()));
            add_tool_input(&conn, 2, "s1", 300, &format!("cat {}", a.display()));
            add_tool_input(&conn, 3, "s1", 200, &format!("cat {}", gone.display()));
            add_tool_input(&conn, 4, "s2", 900, &format!("cat {}", a.display()));
        }
        let payload =
            session_memories_for_store(&store, home.path(), memories_request("s1", None)).unwrap();
        assert_eq!(payload.entries.len(), 2);
        let first = &payload.entries[0];
        assert_eq!(first.title, "Alpha");
        assert_eq!(first.count, 2);
        assert_eq!(first.last_ms, Some(300));
        assert_eq!(first.action, "referenced");
        assert!(first.exists);
        assert_eq!(first.slug, "-work-app");
        let second = &payload.entries[1];
        assert_eq!(second.title, "gone");
        assert!(!second.exists);

        let remote =
            session_memories_for_store(&store, home.path(), memories_request("s1", Some("h")))
                .unwrap();
        assert!(remote.entries.is_empty());
    }
}
