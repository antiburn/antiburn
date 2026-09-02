//! Claude Code agent log discovery.
//!
//! Claude Code stores transcript logs under `~/.claude/projects/<encoded-path>/`.
//! The Claude desktop app also writes session manifests under
//! `~/Library/Application Support/Claude/claude-code-sessions/` (or the
//! platform-equivalent config directory), which can advance a session's recency
//! even when the underlying transcript file is not the freshest file on disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::discovery::scanner::{self, AgentKind, TitleSource};
use crate::discovery::{
    AgentExplorer, DirectSessionSource, ResolvedTitle, SessionLog, SessionSource, SurfacePaths,
    TitleLookupKind, WatchRoot, app_config_dir_in, collect_dirs_with_exts,
    extract_json_string_field, home_dir, recent_files_with_exts,
};
use async_trait::async_trait;
use serde::Deserialize;

pub struct ClaudeExplorer;

const MAX_CLAUDE_JOB_COUNT: usize = 10_000;
const MAX_CLAUDE_JOB_VALUE_BYTES: usize = 4096;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeForkJobState {
    #[serde(default)]
    interactive_lineage: bool,
    fork_session_id: Option<String>,
    fork_parent_session_id: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
    name: Option<String>,
    intent: Option<String>,
}

#[async_trait]
impl AgentExplorer for ClaudeExplorer {
    async fn discover_recent(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        let home = match home_dir() {
            Some(h) => h,
            None => return Vec::new(),
        };
        discover_recent_in(&home, now, since_secs).await
    }

    async fn discover_cwds(&self, now: i64, since_secs: i64) -> Vec<String> {
        let home = match home_dir() {
            Some(h) => h,
            None => return Vec::new(),
        };
        discover_cwds_in(&home, now, since_secs).await
    }

    /// Claude is the fallback agent: `infer_agent_type` returns
    /// `AgentKind::Claude` when no other explorer matches. We don't claim
    /// `.claude` paths here so the dispatcher's default-Claude semantics
    /// remain the single source of truth.
    ///
    /// Substring → `surface_paths` bucket: *(none — returns `false`)*
    ///
    /// `~/.claude/projects/**` is shared CLI + VS Code extension + Desktop;
    /// surface is disambiguated by the in-file `entrypoint` content marker
    /// (`claude-cli`, `claude-vscode`, `claude-desktop`, `sdk-ts`, …), not
    /// by path. See `session_surface_label` below.
    fn owns_path(&self, _path_lower: &str) -> bool {
        false
    }

    fn title_lookup_kind(&self) -> TitleLookupKind {
        TitleLookupKind::Direct
    }

    /// Direct file lookup by transcript filename stem. Avoids the default
    /// implementation's O(N) walk across all recent logs — Claude transcripts
    /// are named `{session_id}.jsonl`, so we can read the one file we care
    /// about. Keeps frontend title fetches fast even for trays full of rows.
    async fn session_title(&self, agent_session_id: &str) -> Option<ResolvedTitle> {
        let home = home_dir()?;
        let project_dirs = all_log_dirs_in(&home).await;
        session_title_with_dirs(&project_dirs, agent_session_id).await
    }

    /// Point query, same shape as `session_title`: a transcript lives at
    /// `<project_dir>/{session_id}.jsonl`, so an existence check against the
    /// project dirs replaces the default full-tree discover. A miss returns
    /// `Unsupported` (not `Missing`): a desktop-manifest session whose CLI
    /// transcript is gone doesn't exist at that path, so the caller must still
    /// fall back to the full discover to resolve it to its Inline source.
    async fn direct_session_source(&self, session_id: &str) -> DirectSessionSource {
        if let Some(home) = home_dir()
            && let Some(path) = locate_transcript_in(&home, session_id).await
        {
            return DirectSessionSource::Found(SessionSource::File(path));
        }
        DirectSessionSource::Unsupported
    }

    // Claude is bi-modal AND its CLI / VS Code extension / Desktop all
    // write transcripts under the same `~/.claude/projects/**` tree, so
    // path alone is insufficient. Prefer the in-file `entrypoint` marker
    // emitted on each user message (`claude-vscode`, `claude-desktop`,
    // `claude-cli`, `sdk-ts`, …). When content is absent, defer to
    // `surface_paths` for the IDE manifest dir, and also treat the inline
    // `claude-desktop:` label produced by `desktop_manifest_session_log`
    // as IDE/Desktop (that prefix is not itself a `surface_paths` root).
    fn session_surface_label(
        &self,
        log: &SessionLog,
        content: Option<&str>,
        home: &Path,
    ) -> &'static str {
        if let Some(content) = content
            && let Some(label) = entrypoint_surface(content)
        {
            return label;
        }
        if let SessionSource::Inline { label, .. } = &log.source
            && label.starts_with("claude-desktop:")
        {
            return "ide_desktop";
        }
        crate::discovery::classify_source_against_surface_paths(
            &log.source,
            &self.surface_paths(home),
        )
        .unwrap_or_else(|| self.unmatched_surface())
    }

    // Pre-2.x Claude Code sessions don't carry an `entrypoint` and live
    // under `~/.claude/projects/**`; falling back to "cli" matches the
    // historical default for that tree when no IDE root matches.
    fn unmatched_surface(&self) -> &'static str {
        "cli"
    }

    /// CLI: `~/.claude/projects/**` (also touched by Desktop/VS Code; content's
    /// `entrypoint` marker disambiguates). IDE: `<app-config>/Claude/claude-code-sessions/**`.
    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        SurfacePaths {
            cli: vec![home.join(".claude").join("projects")],
            ide_desktop: vec![app_config_dir_in("Claude", home).join("claude-code-sessions")],
            mirror: Vec::new(),
        }
    }

    /// Transcripts, the desktop manifest dir, and interactive fork job state
    /// all move independently: a fork's `state.json` can change with no
    /// transcript write at all. Watch every root discovery reads.
    fn watch_roots(&self, home: &Path) -> Vec<WatchRoot> {
        vec![
            WatchRoot::recursive(home.join(".claude").join("projects")),
            WatchRoot::recursive(app_config_dir_in("Claude", home).join("claude-code-sessions")),
            WatchRoot::recursive(home.join(".claude").join("jobs")),
        ]
    }

    // ---- Orchestration: Claude writes each spawned sub-agent as its own
    // transcript under `<dir>/<sessionId>/subagents/agent-*.jsonl`. These hooks
    // expose that tree to the vendor-agnostic orchestration layer; the
    // implementation lives in the `claude_subagents` module.

    fn supports_subagents(&self) -> bool {
        true
    }

    async fn list_subagents(&self, parent_transcript: &Path) -> Vec<PathBuf> {
        super::claude_subagents::list_subagents(parent_transcript).await
    }

    async fn locate_subagent(
        &self,
        parent_transcript: &Path,
        subagent_id: &str,
    ) -> Option<PathBuf> {
        super::claude_subagents::locate_subagent(parent_transcript, subagent_id).await
    }

    fn subagent_id(&self, path: &Path) -> Option<String> {
        super::claude_subagents::subagent_id(path)
    }

    async fn subagent_label(&self, path: &Path) -> String {
        super::claude_subagents::subagent_label(path).await
    }

    async fn subagent_meta(&self, path: &Path) -> Option<crate::discovery::SubagentMeta> {
        super::claude_subagents::read_subagent_meta(path).await
    }
}

/// Inspect a Claude session's `entrypoint` field and classify the surface.
/// Returns `None` when no `entrypoint` is present within the first
/// [`MAX_LINES_TO_SCAN`] lines, so the caller can fall back to path-based
/// classification.
///
/// The first one or two records of a Claude session are usually
/// `queue-operation` events with no `entrypoint`; the next user/assistant
/// record carries one (`"entrypoint":"claude-vscode"`, `"claude-desktop"`,
/// `"claude-cli"`, `"sdk-ts"`, …).
pub fn entrypoint_surface(content: &str) -> Option<&'static str> {
    for line in content.lines().take(MAX_LINES_TO_SCAN) {
        if let Some(value) = extract_json_string_field(line, "entrypoint") {
            return Some(classify_entrypoint(value));
        }
    }
    None
}

/// Number of leading session lines scanned for the `entrypoint` marker. The
/// preamble (typically `queue-operation` records) doesn't include the field,
/// but it appears on the first user/assistant record. 8 lines is a generous
/// cap that avoids walking megabytes of transcript on a missing field.
const MAX_LINES_TO_SCAN: usize = 8;

fn classify_entrypoint(entrypoint: &str) -> &'static str {
    let lower = entrypoint.to_ascii_lowercase();
    if lower.contains("vscode")
        || lower.contains("desktop")
        || lower.contains("jetbrains")
        || lower.contains("intellij")
        || lower.contains("ide")
    {
        "ide_desktop"
    } else {
        // claude-cli, sdk-ts, sdk-python, terminal launches → treat as CLI.
        "cli"
    }
}

#[cfg(test)]
pub(crate) fn sample_log_path(home: &Path) -> PathBuf {
    home.join(".claude")
        .join("projects")
        .join("-Users-foo-bar")
        .join("session.jsonl")
}

/// Resolve a Claude session title given a pre-fetched list of project
/// directories. Reads the transcript named `{session_id}.jsonl` directly
/// (point query) and wraps the parsed title + provenance into a
/// [`ResolvedTitle`]. Returns `None` when no transcript matches or the
/// transcript has no extractable title.
pub async fn session_title_with_dirs(
    project_dirs: &[PathBuf],
    agent_session_id: &str,
) -> Option<ResolvedTitle> {
    let path = resolve_cli_transcript_path(project_dirs, agent_session_id).await?;
    let metadata = scanner::parse_session_metadata(&path).await;
    let text = metadata.title?;
    let source = metadata.title_source.unwrap_or(TitleSource::Explicit);
    Some(ResolvedTitle::new(text, source))
}

/// Point-locate a session transcript under a given home directory:
/// `<project_dir>/{session_id}.jsonl` across all project dirs. Backs the
/// `direct_session_source` override; separated for testability.
async fn locate_transcript_in(home: &Path, session_id: &str) -> Option<PathBuf> {
    let project_dirs = all_log_dirs_in(home).await;
    resolve_cli_transcript_path(&project_dirs, session_id).await
}

/// Internal: find ALL Claude log directories under a given home directory.
///
/// Separated from `all_log_dirs` for testability.
async fn all_log_dirs_in(home: &Path) -> Vec<PathBuf> {
    let projects_dir = home.join(".claude").join("projects");
    tokio::task::spawn_blocking(move || {
        let Ok(entries) = std::fs::read_dir(&projects_dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Fast CWD-only discovery: decode project directory names without reading files.
/// Falls back to reading the first .jsonl file when path decoding fails
/// (e.g., TCC-protected paths where filesystem validation can't confirm segments).
async fn discover_cwds_in(home: &Path, now: i64, since_secs: i64) -> Vec<String> {
    let cutoff = now - since_secs;
    let project_dirs = all_log_dirs_in(home).await;
    let mut cwds = Vec::new();

    for dir in &project_dirs {
        let recent_jsonl = first_recent_jsonl(dir, cutoff).await;
        if recent_jsonl.is_none() {
            continue;
        }

        let encoded = match dir.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };

        // Try fast decode from directory name first.
        if let Some(decoded) = scanner::decode_hyphenated_absolute_path(encoded, home).await {
            cwds.push(decoded.to_string_lossy().to_string());
            continue;
        }

        // Fallback: read the session file to extract CWD (handles TCC paths
        // where the decode can't validate segments against the filesystem).
        if let Some(jsonl_path) = recent_jsonl
            && let Some(cwd) = scanner::parse_session_metadata(&jsonl_path).await.cwd
        {
            cwds.push(cwd);
        }
    }
    cwds
}

/// Return the first `.jsonl` file in `dir` modified after `cutoff`, if any.
/// One `spawn_blocking` for the whole directory scan.
async fn first_recent_jsonl(dir: &Path, cutoff: i64) -> Option<PathBuf> {
    let dir = dir.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Ok(meta) = entry.metadata()
                && meta.is_file()
                && let Ok(mtime) = meta.modified()
                && let Ok(dur) = mtime.duration_since(std::time::UNIX_EPOCH)
                && (dur.as_secs() as i64) >= cutoff
            {
                return Some(path);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// Blocking-context helpers for the subagent-recency sweep: the callers batch
/// a whole tree walk into one `spawn_blocking` task, so these do plain
/// `std::fs` work instead of spawning a blocking micro-task per fs op.
fn file_mtime_epoch_blocking(path: &Path) -> Option<i64> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
}

fn max_subagent_mtime_blocking(parent_transcript: &Path) -> Option<i64> {
    let subagents = super::claude_subagents::list_subagents_blocking(parent_transcript);
    let mut max_mtime = None;
    for subagent in subagents {
        let Some(mtime) = file_mtime_epoch_blocking(&subagent) else {
            continue;
        };
        max_mtime = Some(max_mtime.map_or(mtime, |current: i64| current.max(mtime)));
    }
    max_mtime
}

/// Bump each discovered session's recency to its newest sub-agent transcript,
/// so an orchestrator whose parent transcript is idle (all the work is in the
/// sub-agents) still sorts as active. One `spawn_blocking` sweep across every
/// discovered on-disk parent.
async fn update_discovered_with_subagent_mtimes(discovered: &mut HashMap<String, SessionLog>) {
    let parents: Vec<(String, PathBuf)> = discovered
        .iter()
        .filter_map(|(key, log)| match &log.source {
            SessionSource::File(parent) => Some((key.clone(), parent.clone())),
            _ => None,
        })
        .collect();

    let mtimes: Vec<(String, i64)> = tokio::task::spawn_blocking(move || {
        parents
            .into_iter()
            .filter_map(|(key, parent)| {
                max_subagent_mtime_blocking(&parent).map(|mtime| (key, mtime))
            })
            .collect()
    })
    .await
    .unwrap_or_default();

    for (key, subagent_mtime) in mtimes {
        let Some(log) = discovered.get_mut(&key) else {
            continue;
        };
        let current = log.updated_at.unwrap_or_default();
        if subagent_mtime > current {
            log.updated_at = Some(subagent_mtime);
        }
    }
}

/// Find parents whose *sub-agents* are recent even when the parent transcript
/// itself is not. One `spawn_blocking` for the whole project-dir walk.
async fn recent_subagent_parent_logs(project_dirs: &[PathBuf], cutoff: i64) -> Vec<SessionLog> {
    let project_dirs = project_dirs.to_vec();
    tokio::task::spawn_blocking(move || {
        let mut logs = Vec::new();
        for project_dir in &project_dirs {
            let Ok(entries) = std::fs::read_dir(project_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_dir() {
                    continue;
                }
                let Some(session_id) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                let parent = project_dir.join(format!("{session_id}.jsonl"));
                if !parent.try_exists().unwrap_or(false) {
                    continue;
                }
                let Some(subagent_mtime) = max_subagent_mtime_blocking(&parent) else {
                    continue;
                };
                if subagent_mtime < cutoff {
                    continue;
                }
                let updated_at = file_mtime_epoch_blocking(&parent)
                    .map_or(subagent_mtime, |parent_mtime| {
                        parent_mtime.max(subagent_mtime)
                    });
                logs.push(SessionLog {
                    environment: Default::default(),
                    agent_type: AgentKind::Claude,
                    source: SessionSource::File(parent),
                    updated_at: Some(updated_at),
                });
            }
        }
        logs
    })
    .await
    .unwrap_or_default()
}

async fn merge_recent_subagent_parent_logs(
    discovered: &mut HashMap<String, SessionLog>,
    project_dirs: &[PathBuf],
    cutoff: i64,
) {
    for log in recent_subagent_parent_logs(project_dirs, cutoff).await {
        merge_session_log(discovered, log);
    }
}

async fn desktop_manifest_dirs_in(home: &Path) -> Vec<PathBuf> {
    let root = app_config_dir_in("Claude", home).join("claude-code-sessions");
    let mut dirs = Vec::new();
    collect_dirs_with_exts(&root, &mut dirs, &["json"]).await;
    dirs
}

async fn discover_recent_in(home: &Path, now: i64, since_secs: i64) -> Vec<SessionLog> {
    discover_recent_including_subagent_parents(home, now, since_secs).await
}

/// WSL entry point with the same parent-only and child-recency semantics as
/// native Claude discovery. Child transcripts never become top-level sessions;
/// their mtimes only promote the owning parent.
pub async fn discover_recent_in_wsl(
    info: &crate::platform::environment::WslEnvironmentInfo,
    now: i64,
    since_secs: i64,
) -> Vec<SessionLog> {
    let mut logs =
        discover_recent_including_subagent_parents(&info.context.home, now, since_secs).await;
    for log in &mut logs {
        log.environment = info.context.environment.clone();
    }
    logs
}

async fn discover_recent_including_subagent_parents(
    home: &Path,
    now: i64,
    since_secs: i64,
) -> Vec<SessionLog> {
    let project_dirs = all_log_dirs_in(home).await;
    let mut discovered: HashMap<String, SessionLog> =
        recent_files_with_exts(&project_dirs, now, since_secs, &["jsonl"])
            .await
            .into_iter()
            .map(|file| {
                let log = SessionLog {
                    environment: Default::default(),
                    agent_type: AgentKind::Claude,
                    source: SessionSource::File(file.path),
                    updated_at: Some(file.mtime_epoch),
                };
                (log.source_label(), log)
            })
            .collect();
    update_discovered_with_subagent_mtimes(&mut discovered).await;
    merge_recent_subagent_parent_logs(&mut discovered, &project_dirs, now - since_secs).await;

    let manifest_dirs = desktop_manifest_dirs_in(home).await;
    for manifest in recent_files_with_exts(&manifest_dirs, now, since_secs, &["json"]).await {
        if let Some(log) =
            desktop_manifest_session_log(home, &project_dirs, manifest.path, manifest.mtime_epoch)
                .await
        {
            merge_session_log(&mut discovered, log);
        }
    }

    for log in interactive_fork_job_logs(home, &project_dirs, now - since_secs).await {
        // The job adapter enriches a title-only transcript with the explicit
        // parent and CWD, so it must replace the plain file candidate even when
        // both files have the same one-second mtime.
        discovered.insert(log.source_label(), log);
    }

    let mut logs = discovered.into_values().collect::<Vec<_>>();
    logs.sort_by(|a, b| {
        a.updated_at
            .unwrap_or_default()
            .cmp(&b.updated_at.unwrap_or_default())
            .then_with(|| a.source_label().cmp(&b.source_label()))
    });
    logs
}

fn valid_claude_session_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CLAUDE_JOB_VALUE_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn valid_claude_job_cwd(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CLAUDE_JOB_VALUE_BYTES
        && !value.contains('\0')
        && (Path::new(value).is_absolute() || value.starts_with('/'))
}

async fn interactive_fork_job_logs(
    home: &Path,
    project_dirs: &[PathBuf],
    cutoff: i64,
) -> Vec<SessionLog> {
    let jobs_root = home.join(".claude").join("jobs");
    let Ok(mut entries) = tokio::fs::read_dir(&jobs_root).await else {
        return Vec::new();
    };
    let mut logs = Vec::new();
    let mut job_count = 0;
    while job_count < MAX_CLAUDE_JOB_COUNT {
        let Ok(Some(entry)) = entries.next_entry().await else {
            break;
        };
        job_count += 1;
        let state_path = entry.path().join("state.json");
        let Ok(state_content) = tokio::fs::read_to_string(&state_path).await else {
            continue;
        };
        let Ok(state) = serde_json::from_str::<ClaudeForkJobState>(&state_content) else {
            continue;
        };
        let (Some(child_id), Some(parent_id), Some(session_id), Some(cwd)) = (
            state.fork_session_id.as_deref(),
            state.fork_parent_session_id.as_deref(),
            state.session_id.as_deref(),
            state.cwd.as_deref(),
        ) else {
            continue;
        };
        if !state.interactive_lineage
            || child_id != session_id
            || child_id == parent_id
            || !valid_claude_session_id(child_id)
            || !valid_claude_session_id(parent_id)
            || !valid_claude_job_cwd(cwd)
        {
            continue;
        }

        let transcript_path = resolve_cli_transcript_path(project_dirs, child_id).await;
        let transcript_mtime = transcript_path
            .as_deref()
            .and_then(file_mtime_epoch_blocking)
            .unwrap_or_default();
        let state_mtime = file_mtime_epoch_blocking(&state_path).unwrap_or_default();
        let updated_at = transcript_mtime.max(state_mtime);
        if updated_at < cutoff {
            continue;
        }
        let transcript = match transcript_path.as_deref() {
            Some(path) => tokio::fs::read_to_string(path).await.unwrap_or_default(),
            None => String::new(),
        };
        let title = state
            .name
            .as_deref()
            .or(state.intent.as_deref())
            .filter(|value| !value.trim().is_empty() && value.len() <= MAX_CLAUDE_JOB_VALUE_BYTES);
        let header = serde_json::json!({
            "type": "session",
            "sessionId": child_id,
            "cwd": cwd,
            "entrypoint": "cli",
            "aiTitle": title,
            "forkedFrom": { "sessionId": parent_id }
        });
        let content = if transcript.is_empty() {
            header.to_string()
        } else {
            format!("{header}\n{transcript}")
        };
        let label = transcript_path
            .as_deref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("claude-fork-job:{child_id}"));
        logs.push(SessionLog {
            environment: Default::default(),
            agent_type: AgentKind::Claude,
            source: SessionSource::Inline { label, content },
            updated_at: Some(updated_at),
        });
    }
    logs
}

/// Build a session from one Claude desktop session manifest.
///
/// The manifest must be a JSON object that names a CLI transcript in
/// `cliSessionId`. The `claude-code-sessions` tree also holds sidecar files
/// that are not sessions, such as the `scheduled-tasks.json` task
/// configuration. A manifest without `cliSessionId` has no transcript and
/// therefore no token records. Both give a session that analysis can never
/// assess, so this function returns `None` for them.
async fn desktop_manifest_session_log(
    home: &Path,
    project_dirs: &[PathBuf],
    manifest_path: PathBuf,
    manifest_mtime_epoch: i64,
) -> Option<SessionLog> {
    let content = tokio::fs::read_to_string(&manifest_path).await.ok()?;
    let mut manifest = serde_json::from_str::<serde_json::Value>(&content).ok()?;
    let cli_session_id = manifest
        .get("cliSessionId")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())?
        .to_owned();

    if let Some(transcript_path) = resolve_cli_transcript_path(project_dirs, &cli_session_id).await
    {
        return Some(SessionLog {
            environment: Default::default(),
            agent_type: AgentKind::Claude,
            source: SessionSource::File(transcript_path),
            updated_at: Some(manifest_mtime_epoch),
        });
    }

    let label = format!(
        "claude-desktop:{}",
        manifest_path
            .strip_prefix(home)
            .unwrap_or(&manifest_path)
            .display()
    );
    // The transcript is not on disk yet. Name the session by the CLI session
    // id so a later scan that finds the transcript maps to the same session.
    if let Some(object) = manifest.as_object_mut() {
        object.insert(
            "sessionId".to_string(),
            serde_json::Value::String(cli_session_id),
        );
    }
    Some(SessionLog {
        environment: Default::default(),
        agent_type: AgentKind::Claude,
        source: SessionSource::Inline {
            label,
            content: manifest.to_string(),
        },
        updated_at: Some(manifest_mtime_epoch),
    })
}

/// Resolve the on-disk path for a Claude CLI transcript by `(project_dirs,
/// session_id)`. Validates `session_id` against a positive allowlist
/// (alphanumeric + hyphen) to prevent path traversal from a crafted JSONL
/// `sessionId` field — Claude session IDs are UUIDs in practice, so this is
/// strictly safer than the broader file-name space.
async fn resolve_cli_transcript_path(
    project_dirs: &[PathBuf],
    cli_session_id: &str,
) -> Option<PathBuf> {
    if cli_session_id.is_empty()
        || !cli_session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return None;
    }
    let candidates: Vec<PathBuf> = project_dirs
        .iter()
        .map(|dir| dir.join(format!("{cli_session_id}.jsonl")))
        .collect();
    // One blocking task for the whole probe, not one per existence check.
    tokio::task::spawn_blocking(move || {
        candidates
            .into_iter()
            .find(|candidate| candidate.try_exists().unwrap_or(false))
    })
    .await
    .ok()
    .flatten()
}

fn merge_session_log(discovered: &mut HashMap<String, SessionLog>, incoming: SessionLog) {
    let key = incoming.source_label();
    match discovered.get(&key) {
        None => {
            discovered.insert(key, incoming);
        }
        Some(existing) => {
            if incoming.updated_at.unwrap_or_default() > existing.updated_at.unwrap_or_default() {
                discovered.insert(key, incoming);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::set_file_mtime;
    use tempfile::TempDir;

    #[tokio::test]
    async fn interactive_fork_job_enriches_waiting_child_with_parent_and_cwd() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("tests/fixtures/claude-cli-job-fork.json")).unwrap();
        let home = TempDir::new().unwrap();
        let project = home.path().join(".claude/projects/-repo");
        let jobs = home.path().join(".claude/jobs/claude-child-fork");
        tokio::fs::create_dir_all(&project).await.unwrap();
        tokio::fs::create_dir_all(&jobs).await.unwrap();
        let transcript = project.join("claude-child-fork.jsonl");
        let transcript_content = fixture["child_transcript"]
            .as_array()
            .unwrap()
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        tokio::fs::write(&transcript, transcript_content)
            .await
            .unwrap();
        let state = jobs.join("state.json");
        tokio::fs::write(&state, fixture["job_state"].to_string())
            .await
            .unwrap();
        set_file_mtime(&transcript, 1_700_000_000);
        set_file_mtime(&state, 1_700_000_000);

        let logs = interactive_fork_job_logs(home.path(), &[project], 1_699_999_999).await;

        assert_eq!(logs.len(), 1);
        let SessionSource::Inline { content, .. } = &logs[0].source else {
            panic!("fork job should produce enriched inline content")
        };
        let metadata = scanner::parse_session_metadata_str(content);
        assert_eq!(metadata.session_id.as_deref(), Some("claude-child-fork"));
        assert_eq!(
            metadata.cwd.as_deref(),
            Some("/home/avery/projects/demo-app")
        );
        // The enrichment states the lineage in Claude's own `forkedFrom`
        // shape, which is what a lineage consumer reads back.
        let header: serde_json::Value =
            serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(
            header
                .pointer("/forkedFrom/sessionId")
                .and_then(|v| v.as_str()),
            Some("claude-parent-fork")
        );
    }

    #[tokio::test]
    async fn ordinary_background_job_is_not_promoted_to_interactive_fork() {
        let home = TempDir::new().unwrap();
        let jobs = home.path().join(".claude/jobs/child-1");
        tokio::fs::create_dir_all(&jobs).await.unwrap();
        let state = jobs.join("state.json");
        tokio::fs::write(
            &state,
            r#"{"interactiveLineage":false,"forkSessionId":"child-1","forkParentSessionId":"parent-1","sessionId":"child-1","cwd":"/repo"}"#,
        )
        .await
        .unwrap();
        set_file_mtime(&state, 1_700_000_000);

        let logs = interactive_fork_job_logs(home.path(), &[], 1_699_999_999).await;

        assert!(logs.is_empty());
    }

    #[test]
    fn claude_job_cwd_accepts_wsl_paths_and_rejects_relative_paths() {
        assert!(valid_claude_job_cwd("/workspace/repo"));
        assert!(!valid_claude_job_cwd("workspace/repo"));
        assert!(!valid_claude_job_cwd("/workspace/repo\0ignored"));
    }

    #[tokio::test]
    async fn test_locate_transcript_in_point_queries_across_project_dirs() {
        let home = TempDir::new().unwrap();
        let projects_dir = home.path().join(".claude").join("projects");
        let dir_a = projects_dir.join("-Users-foo-a");
        let dir_b = projects_dir.join("-Users-foo-b");
        tokio::fs::create_dir_all(&dir_a).await.unwrap();
        tokio::fs::create_dir_all(&dir_b).await.unwrap();
        tokio::fs::write(dir_a.join("other-session.jsonl"), "{}")
            .await
            .unwrap();
        let target = dir_b.join("sess-1.jsonl");
        tokio::fs::write(&target, "{}").await.unwrap();

        assert_eq!(
            locate_transcript_in(home.path(), "sess-1").await,
            Some(target)
        );
        // A miss returns None so `direct_session_source` can fall back to the
        // full discover (desktop-manifest / inline sessions).
        assert_eq!(locate_transcript_in(home.path(), "missing").await, None);
    }

    #[tokio::test]
    async fn test_all_log_dirs_returns_all_directories() {
        let home = TempDir::new().unwrap();
        let projects_dir = home.path().join(".claude").join("projects");
        tokio::fs::create_dir_all(&projects_dir).await.unwrap();

        tokio::fs::create_dir(projects_dir.join("-Users-foo-bar"))
            .await
            .unwrap();
        tokio::fs::create_dir(projects_dir.join("-Users-baz-qux"))
            .await
            .unwrap();
        tokio::fs::create_dir(projects_dir.join("-home-user-project"))
            .await
            .unwrap();

        let result = all_log_dirs_in(home.path()).await;
        assert_eq!(result.len(), 3);
    }

    #[tokio::test]
    async fn test_all_log_dirs_returns_empty_when_no_projects_dir() {
        let home = TempDir::new().unwrap();
        let result = all_log_dirs_in(home.path()).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_all_log_dirs_ignores_files() {
        let home = TempDir::new().unwrap();
        let projects_dir = home.path().join(".claude").join("projects");
        tokio::fs::create_dir_all(&projects_dir).await.unwrap();

        // Create a file (not a directory)
        tokio::fs::write(projects_dir.join("some-file"), "not a dir")
            .await
            .unwrap();
        tokio::fs::create_dir(projects_dir.join("-Users-foo-bar"))
            .await
            .unwrap();

        let result = all_log_dirs_in(home.path()).await;
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn test_all_log_dirs_graceful_when_claude_dir_missing() {
        // Same for all_log_dirs: missing ~/.claude/ should not error.
        let home = TempDir::new().unwrap();
        let result = all_log_dirs_in(home.path()).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_discover_recent_uses_desktop_manifest_recency_for_cli_transcript() {
        let home = TempDir::new().unwrap();
        let now: i64 = 1_700_000_000;
        let since_secs: i64 = 86_400;

        let project_dir = home
            .path()
            .join(".claude")
            .join("projects")
            .join("-Users-foo-bar");
        tokio::fs::create_dir_all(&project_dir).await.unwrap();

        let transcript = project_dir.join("cli-session.jsonl");
        tokio::fs::write(&transcript, "{}\n").await.unwrap();
        set_file_mtime(&transcript, now - (since_secs + 10));

        let manifest_dir = app_config_dir_in("Claude", home.path())
            .join("claude-code-sessions")
            .join("workspace")
            .join("window");
        tokio::fs::create_dir_all(&manifest_dir).await.unwrap();

        let manifest = manifest_dir.join("local_session.json");
        tokio::fs::write(
            &manifest,
            r#"{"sessionId":"local-session","cliSessionId":"cli-session","cwd":"/Users/foo/bar"}"#,
        )
        .await
        .unwrap();
        set_file_mtime(&manifest, now - 5);

        let logs = discover_recent_in(home.path(), now, since_secs).await;
        assert_eq!(logs.len(), 1);
        match &logs[0].source {
            SessionSource::File(path) => assert_eq!(path, &transcript),
            SessionSource::Inline { .. } => panic!("expected transcript file source"),
            SessionSource::ProviderDb { .. } => panic!("expected transcript file source"),
        }
        assert_eq!(logs[0].updated_at, Some(now - 5));
    }

    #[tokio::test]
    async fn test_discover_recent_falls_back_to_manifest_when_cli_transcript_missing() {
        let home = TempDir::new().unwrap();
        let now: i64 = 1_700_000_000;
        let since_secs: i64 = 86_400;

        let manifest_dir = app_config_dir_in("Claude", home.path())
            .join("claude-code-sessions")
            .join("workspace")
            .join("window");
        tokio::fs::create_dir_all(&manifest_dir).await.unwrap();

        let manifest = manifest_dir.join("local_session.json");
        tokio::fs::write(
            &manifest,
            r#"{"sessionId":"local-session","cliSessionId":"cli-session","cwd":"/Users/foo/bar"}"#,
        )
        .await
        .unwrap();
        set_file_mtime(&manifest, now - 5);

        let logs = discover_recent_in(home.path(), now, since_secs).await;
        assert_eq!(logs.len(), 1);
        match &logs[0].source {
            SessionSource::Inline { label, content } => {
                assert!(label.starts_with("claude-desktop:"));
                assert!(content.contains("\"sessionId\":\"cli-session\""));
            }
            SessionSource::File(path) => panic!("unexpected file source: {}", path.display()),
            SessionSource::ProviderDb { .. } => panic!("unexpected provider db source"),
        }
        assert_eq!(logs[0].updated_at, Some(now - 5));
    }

    #[tokio::test]
    async fn test_discover_recent_skips_desktop_session_sidecar_files() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "tests/fixtures/claude-desktop-session-sidecars.json"
        ))
        .unwrap();
        let home = TempDir::new().unwrap();
        let now: i64 = 1_700_000_000;
        let since_secs: i64 = 86_400;

        let project_dir = home
            .path()
            .join(".claude")
            .join("projects")
            .join("-home-avery-projects-demo-app");
        tokio::fs::create_dir_all(&project_dir).await.unwrap();

        // The transcript is older than the cutoff. Only the manifest can
        // promote it, so this also proves the manifest scan still works.
        let cli_session_id = fixture["manifest"]["cliSessionId"].as_str().unwrap();
        let transcript = project_dir.join(format!("{cli_session_id}.jsonl"));
        tokio::fs::write(&transcript, "{}\n").await.unwrap();
        set_file_mtime(&transcript, now - (since_secs + 10));

        let manifest_dir = app_config_dir_in("Claude", home.path())
            .join("claude-code-sessions")
            .join("workspace")
            .join("window");
        tokio::fs::create_dir_all(&manifest_dir).await.unwrap();
        for (file_name, fixture_key) in [
            ("local_session.json", "manifest"),
            ("local_no_cli_session.json", "manifest_without_cli_session"),
            ("scheduled-tasks.json", "scheduled_tasks_sidecar"),
        ] {
            let path = manifest_dir.join(file_name);
            tokio::fs::write(&path, fixture[fixture_key].to_string())
                .await
                .unwrap();
            set_file_mtime(&path, now - 5);
        }

        let logs = discover_recent_in(home.path(), now, since_secs).await;
        assert_eq!(logs.len(), 1);
        match &logs[0].source {
            SessionSource::File(path) => assert_eq!(path, &transcript),
            SessionSource::Inline { label, .. } => panic!("unexpected inline source: {label}"),
            SessionSource::ProviderDb { .. } => panic!("unexpected provider db source"),
        }
        assert_eq!(logs[0].updated_at, Some(now - 5));
    }

    #[tokio::test]
    async fn test_discover_recent_includes_parent_when_subagent_is_recent() {
        let home = TempDir::new().unwrap();
        let now: i64 = 1_700_000_000;
        let since_secs: i64 = 86_400;

        let project = home
            .path()
            .join(".claude")
            .join("projects")
            .join("-Users-foo-bar");
        let subagents = project.join("sess-1").join("subagents");
        tokio::fs::create_dir_all(&subagents).await.unwrap();

        let parent = project.join("sess-1.jsonl");
        tokio::fs::write(&parent, "{}\n").await.unwrap();
        set_file_mtime(&parent, now - (since_secs + 10));

        let subagent = subagents.join("agent-aaa.jsonl");
        tokio::fs::write(&subagent, "{}\n").await.unwrap();
        set_file_mtime(&subagent, now - 5);

        let logs = discover_recent_including_subagent_parents(home.path(), now, since_secs).await;

        assert_eq!(logs.len(), 1);
        match &logs[0].source {
            SessionSource::File(path) => assert_eq!(path, &parent),
            SessionSource::Inline { .. } => panic!("expected transcript file source"),
            SessionSource::ProviderDb { .. } => panic!("expected transcript file source"),
        }
        assert_eq!(logs[0].updated_at, Some(now - 5));
    }

    #[tokio::test]
    async fn wsl_discovery_promotes_child_recency_without_leaking_child() {
        let home = TempDir::new().unwrap();
        let now: i64 = 1_700_000_000;
        let since_secs: i64 = 86_400;
        let project = home.path().join(".claude/projects/-home-dev-repo");
        let subagents = project.join("sess-1/subagents");
        tokio::fs::create_dir_all(&subagents).await.unwrap();
        let parent = project.join("sess-1.jsonl");
        tokio::fs::write(&parent, "{}\n").await.unwrap();
        set_file_mtime(&parent, now - (since_secs + 10));
        let child = subagents.join("agent-child.jsonl");
        tokio::fs::write(&child, "{}\n").await.unwrap();
        set_file_mtime(&child, now - 5);
        let info = crate::platform::environment::WslEnvironmentInfo {
            context: crate::platform::environment::AgentContext {
                environment: crate::platform::environment::DiscoveryEnvironment::Wsl {
                    distribution: "Ubuntu".into(),
                    user: "dev".into(),
                },
                home: home.path().to_path_buf(),
                platform_home: PathBuf::from("/home/dev"),
            },
            distribution: "Ubuntu".into(),
            user: "dev".into(),
        };

        let logs = discover_recent_in_wsl(&info, now, since_secs).await;

        assert_eq!(logs.len(), 1);
        assert!(matches!(&logs[0].source, SessionSource::File(path) if path == &parent));
        assert_eq!(logs[0].updated_at, Some(now - 5));
        assert_eq!(logs[0].environment.wsl_distro(), Some("Ubuntu"));
    }

    #[tokio::test]
    async fn test_discover_recent_includes_subagent_recency_without_a_caller_side_gate() {
        let home = TempDir::new().unwrap();
        let now: i64 = 1_700_000_000;
        let since_secs: i64 = 86_400;

        let project = home
            .path()
            .join(".claude")
            .join("projects")
            .join("-Users-foo-bar");
        let subagents = project.join("sess-1").join("subagents");
        tokio::fs::create_dir_all(&subagents).await.unwrap();

        let parent = project.join("sess-1.jsonl");
        tokio::fs::write(&parent, "{}\n").await.unwrap();
        set_file_mtime(&parent, now - (since_secs + 10));

        let subagent = subagents.join("agent-aaa.jsonl");
        tokio::fs::write(&subagent, "{}\n").await.unwrap();
        set_file_mtime(&subagent, now - 5);

        let logs = discover_recent_including_subagent_parents(home.path(), now, since_secs).await;

        assert_eq!(logs.len(), 1);
        match &logs[0].source {
            SessionSource::File(path) => assert_eq!(path, &parent),
            SessionSource::Inline { .. } => panic!("expected transcript file source"),
            SessionSource::ProviderDb { .. } => panic!("expected transcript file source"),
        }
        assert_eq!(logs[0].updated_at, Some(now - 5));
    }

    #[tokio::test]
    async fn test_discover_recent_uses_newer_subagent_mtime_for_recent_parent() {
        let home = TempDir::new().unwrap();
        let now: i64 = 1_700_000_000;
        let since_secs: i64 = 86_400;

        let project = home
            .path()
            .join(".claude")
            .join("projects")
            .join("-Users-foo-bar");
        let subagents = project.join("sess-1").join("subagents");
        tokio::fs::create_dir_all(&subagents).await.unwrap();

        let parent = project.join("sess-1.jsonl");
        tokio::fs::write(&parent, "{}\n").await.unwrap();
        set_file_mtime(&parent, now - 50);

        let subagent = subagents.join("agent-aaa.jsonl");
        tokio::fs::write(&subagent, "{}\n").await.unwrap();
        set_file_mtime(&subagent, now - 5);

        let logs = discover_recent_including_subagent_parents(home.path(), now, since_secs).await;

        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].updated_at, Some(now - 5));
    }

    #[tokio::test]
    async fn test_resolve_cli_transcript_path_rejects_path_traversal() {
        // session_id is sourced from JSONL files on disk — a crafted
        // `sessionId` field must NOT escape the project directory.
        let home = TempDir::new().unwrap();
        let projects = home.path().join(".claude").join("projects").join("dir1");
        tokio::fs::create_dir_all(&projects).await.unwrap();
        let dirs = vec![projects];
        for bad in ["../etc/passwd", "..\\..\\windows", "abc/../def", ""] {
            assert!(resolve_cli_transcript_path(&dirs, bad).await.is_none());
        }
    }

    #[tokio::test]
    async fn test_session_title_with_dirs_returns_resolved_title_and_source() {
        use crate::discovery::scanner::TitleSource;

        let home = TempDir::new().unwrap();
        let project = home
            .path()
            .join(".claude")
            .join("projects")
            .join("-Users-foo-bar");
        tokio::fs::create_dir_all(&project).await.unwrap();
        let session_id = "11111111-2222-3333-4444-555555555555";
        let body = format!(
            r#"{{"type":"custom-title","sessionId":"{session_id}","customTitle":"Renamed manually"}}"#,
        );
        tokio::fs::write(project.join(format!("{session_id}.jsonl")), body)
            .await
            .unwrap();

        let dirs = vec![project.clone()];
        let resolved = session_title_with_dirs(&dirs, session_id).await.unwrap();
        assert_eq!(resolved.text, "Renamed manually");
        assert_eq!(resolved.source, TitleSource::UserRename);
    }
}
