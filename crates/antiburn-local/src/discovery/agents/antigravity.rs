// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Antigravity log discovery.
//!
//! Three source kinds, all surfaced as `AgentKind::Antigravity`:
//!
//! 1. **IDE workspace storage** (Antigravity v1 layout only).
//!    Paths:
//!    - macOS: `~/Library/Application Support/Antigravity/User/workspaceStorage/*/chatSessions/*.json`
//!    - Linux: `~/.config/Antigravity/User/workspaceStorage/*/chatSessions/*.json`
//!    - Windows: `%APPDATA%\Antigravity\User\workspaceStorage\*\chatSessions\*.json`
//!
//!    Antigravity IDE 2.0 (verified on a live install 2026-05-25) moved its
//!    app-data directory to `Antigravity IDE/` (note the space + "IDE" suffix)
//!    and **no longer writes `chatSessions/`** under workspaceStorage. v2 IDE
//!    chat data lives in the Gemini brain layout described in (3) instead.
//!
//! 2. **A mirror directory**, when the embedding application registers one.
//!    Antigravity keeps some IDE conversations only in memory of a running
//!    process, so an application that obtains them another way can write them
//!    as `<cascadeId>.json` into a [`SessionMirror`] directory; this adapter
//!    then walks it like any vendor directory. Discovery itself never
//!    populates a mirror.
//!
//! 3. **Gemini brain memory** — agent session memory and reasoning traces
//!    under three sibling roots inside `$GEMINI_HOME` (default `~/.gemini`):
//!    - `antigravity-cli/brain/<uuid>/` — Antigravity CLI (the Go binary that
//!      replaced Gemini CLI as of 2026-06-18).
//!    - `antigravity-ide/brain/<uuid>/` — Antigravity IDE 2.0 (where v2 chat
//!      sessions actually live, including `.system_generated/logs/transcript.jsonl`).
//!    - `antigravity/brain/<uuid>/` — legacy single-binary Antigravity layout,
//!      preserved for back-compat with users on older builds.
//!
//!    Each tree is walked layout-tolerantly for `.json` and `.jsonl` files.
//!    `GEMINI_HOME` overrides the root for all three.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::discovery::scanner::AgentKind;
use crate::discovery::{
    AgentExplorer, SessionLog, SessionMirror, SessionSource, SurfacePaths, app_config_dir_in,
    collect_dirs_with_exts, dir_has_json_files, env_path_when_real_home, find_chat_session_dirs,
    home_dir, recent_files_with_exts,
};
use async_trait::async_trait;

const SESSION_FILE_EXTS: &[&str] = &["json", "jsonl"];

/// Antigravity discovery over the vendor's own layout, plus whatever the
/// embedding application has configured.
pub struct AntigravityExplorer {
    /// An application-maintained directory of `<cascadeId>.json` conversations.
    pub mirror: SessionMirror,
    /// Where the application records orchestrator → worker spawn edges for the
    /// cascades in [`Self::mirror`]. Antigravity does not persist the
    /// relationship itself (see [`antigravity_subagents`](super::antigravity_subagents)),
    /// so without this the adapter reports no sub-agents.
    pub spawn_edges_path: fn(&Path) -> Option<PathBuf>,
}

/// The unconfigured adapter: vendor layouts only, no mirror, no spawn edges.
pub static DISK_ANTIGRAVITY: AntigravityExplorer = AntigravityExplorer {
    mirror: SessionMirror::NONE,
    spawn_edges_path: |_| None,
};

impl AntigravityExplorer {
    /// The mirror directory, only when it actually holds cascade JSON. An
    /// empty mirror stays out of the walk.
    async fn populated_mirror_dir(&self, home: &Path) -> Option<PathBuf> {
        let dir = self.mirror.dir_in(home)?;
        dir_has_json_files(&dir).await.then_some(dir)
    }

    /// Sub-agent inputs for `home`: the spawn-edge sidecar and the mirror
    /// directory holding each worker's cascade. `None` when either is unset.
    fn subagent_inputs(&self, home: &Path) -> Option<(PathBuf, PathBuf)> {
        Some(((self.spawn_edges_path)(home)?, self.mirror.dir_in(home)?))
    }
}

#[async_trait]
impl AgentExplorer for AntigravityExplorer {
    async fn discover_recent(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        let home = match home_dir() {
            Some(h) => h,
            None => return Vec::new(),
        };
        let dirs = self.log_dirs_in(&home).await;
        let files = recent_files_with_exts(&dirs, now, since_secs, SESSION_FILE_EXTS).await;
        // Load history.jsonl once per scan; the synthesizer consults it for
        // CLI-origin brain transcripts. Honors GEMINI_HOME just like the
        // brain-dir walker does.
        let gemini_root =
            env_path_when_real_home(&home, "GEMINI_HOME").unwrap_or_else(|| home.join(".gemini"));
        let cli_history = read_cli_history(&gemini_root).await;

        // Agent-Manager workers are ordinary mirrored cascades (and brain
        // transcripts) in the same trees this walk covers, linked to their
        // orchestrator only by a sidecar edge. Drop them so a worker never
        // surfaces as a top-level session. Empty set → no-op in the common
        // no-orchestration case. Like Codex, this single chokepoint covers
        // every consumer, since they all funnel through `discover_recent`.
        let child_ids = match (self.spawn_edges_path)(&home) {
            Some(sidecar) => super::antigravity_subagents::child_cascade_ids_in(&sidecar).await,
            None => HashSet::new(),
        };

        let mut logs: Vec<SessionLog> = Vec::with_capacity(files.len());
        for file in files {
            if is_excluded_subagent(&file.path, &child_ids) {
                continue;
            }
            match classify_session_file(&file.path, cli_history.as_deref()).await {
                SessionFileDecision::Inline(payload) => logs.push(SessionLog {
                    environment: Default::default(),
                    agent_type: AgentKind::Antigravity,
                    source: SessionSource::Inline {
                        label: format!("antigravity-brain:{}", file.path.display()),
                        content: payload,
                    },
                    updated_at: Some(file.mtime_epoch),
                }),
                SessionFileDecision::File => logs.push(SessionLog {
                    environment: Default::default(),
                    agent_type: AgentKind::Antigravity,
                    source: SessionSource::File(file.path),
                    updated_at: Some(file.mtime_epoch),
                }),
                SessionFileDecision::Skip => {}
            }
        }
        logs
    }

    /// Owns every Antigravity tree: legacy `/antigravity/` brain + v1 IDE
    /// workspaceStorage, v2 IDE `/antigravity-ide/brain/`, CLI
    /// `/antigravity-cli/brain/`, and the configured mirror.
    ///
    /// Substring → `surface_paths` bucket:
    /// - `/antigravity/`           → `ide_desktop` (legacy v1 IDE workspaceStorage + brain)
    /// - `/antigravity-ide/brain/` → `ide_desktop` (v2 IDE brain)
    /// - `/antigravity-cli/brain/` → `cli` (CLI brain — replaces Gemini CLI 2026-06-18)
    /// - the mirror's `path_marker` → `mirror` (classifier maps to `ide_desktop`)
    fn owns_path(&self, path_lower: &str) -> bool {
        path_lower.contains("/antigravity/")
            || path_lower.contains("/antigravity-cli/brain/")
            || path_lower.contains("/antigravity-ide/brain/")
            || self.mirror.owns(path_lower)
    }

    /// Reuses `discover_recent` so brain transcripts go through the same
    /// Inline-cascade synthesis as the activity-list path.
    async fn discover_cwds(&self, now: i64, since_secs: i64) -> Vec<String> {
        let logs = self.discover_recent(now, since_secs).await;
        let mut set = tokio::task::JoinSet::new();
        for log in logs {
            let source = log.source;
            set.spawn(async move {
                crate::discovery::session_source_metadata(&source, None)
                    .await
                    .and_then(|metadata| metadata.cwd)
            });
        }
        let mut cwds = Vec::new();
        while let Some(result) = set.join_next().await {
            if let Ok(Some(cwd)) = result {
                cwds.push(cwd);
            }
        }
        cwds
    }

    // Antigravity is bi-modal:
    //   - CLI:    `~/.gemini/antigravity-cli/brain/**`.
    //   - IDE:    workspaceStorage, the mirror, legacy `antigravity/brain/`,
    //             v2 `antigravity-ide/brain/`.
    // Trait default classifies via `surface_paths`; inline
    // `antigravity-brain:<absolute-path>` labels classify correctly because
    // the embedded path contains one of the `surface_paths` root substrings.
    fn unmatched_surface(&self) -> &'static str {
        "ide_desktop"
    }

    /// CLI: `<gemini>/antigravity-cli/brain/`. IDE: v1 + v2 brain trees and
    /// Antigravity IDE workspaceStorage. `mirror`: the configured mirror
    /// directory, if any. `GEMINI_HOME` overrides the gemini root.
    fn surface_paths(&self, home: &Path) -> SurfacePaths {
        let gemini_root =
            env_path_when_real_home(home, "GEMINI_HOME").unwrap_or_else(|| home.join(".gemini"));
        SurfacePaths {
            cli: vec![gemini_root.join("antigravity-cli").join("brain")],
            ide_desktop: vec![
                gemini_root.join("antigravity-ide").join("brain"),
                gemini_root.join("antigravity").join("brain"),
                app_config_dir_in("Antigravity", home)
                    .join("User")
                    .join("workspaceStorage"),
            ],
            mirror: self.mirror.roots_in(home),
        }
    }

    // ---- Orchestration: Antigravity's Agent Manager spawns each worker as its
    // own cascade (own `<cascadeId>.json`), linked to the orchestrator by
    // `trajectoryMetadata.parentConversationId`. That link is not on disk, so
    // these hooks read it from the sidecar the embedding application records.
    // Implementation lives in `antigravity_subagents`.

    fn supports_subagents(&self) -> bool {
        true
    }

    async fn list_subagents(&self, parent_transcript: &Path) -> Vec<PathBuf> {
        let Some((sidecar, cascades)) = home_dir().and_then(|home| self.subagent_inputs(&home))
        else {
            return Vec::new();
        };
        super::antigravity_subagents::list_subagents_in(parent_transcript, &sidecar, &cascades)
            .await
    }

    async fn locate_subagent(
        &self,
        parent_transcript: &Path,
        subagent_id: &str,
    ) -> Option<PathBuf> {
        let (sidecar, cascades) = self.subagent_inputs(&home_dir()?)?;
        super::antigravity_subagents::locate_subagent_in(
            parent_transcript,
            subagent_id,
            &sidecar,
            &cascades,
        )
        .await
    }

    fn subagent_id(&self, path: &Path) -> Option<String> {
        super::antigravity_subagents::subagent_id(path)
    }

    async fn subagent_label(&self, path: &Path) -> String {
        let Some(sidecar) = home_dir().and_then(|home| (self.spawn_edges_path)(&home)) else {
            return "Sub-agent".to_string();
        };
        super::antigravity_subagents::subagent_label_in(path, &sidecar).await
    }
}

impl AntigravityExplorer {
    /// Every directory that can hold an Antigravity session: IDE workspace
    /// storage, the configured mirror, and the Gemini brain trees.
    async fn log_dirs_in(&self, home: &Path) -> Vec<PathBuf> {
        let ws_root = app_config_dir_in("Antigravity", home)
            .join("User")
            .join("workspaceStorage");
        let user_root = app_config_dir_in("Antigravity", home).join("User");

        let mut dirs = BTreeSet::new();
        for dir in find_chat_session_dirs(&ws_root).await {
            dirs.insert(dir);
        }
        for dir in find_chat_session_dirs(&user_root).await {
            dirs.insert(dir);
        }

        if let Some(mirror) = self.populated_mirror_dir(home).await {
            dirs.insert(mirror);
        }

        // Antigravity brain memory across CLI, IDE 2.0, and legacy roots.
        let brain_dirs = gemini_brain_dirs_in(home).await;
        let brain_dirs_count = brain_dirs.len();
        for dir in brain_dirs {
            dirs.insert(dir);
        }

        ::tracing::debug!(
            target: "antiburn::discovery::antigravity",
            total_dirs = dirs.len(),
            brain_dirs = brain_dirs_count,
            "Antigravity session directories discovered"
        );

        dirs.into_iter().collect()
    }
}

/// Collect Antigravity brain directories under `$GEMINI_HOME` (or
/// `~/.gemini` if `GEMINI_HOME` is unset), across all three known sibling
/// roots:
/// - `antigravity-cli/brain/` — Antigravity CLI (the Go binary)
/// - `antigravity-ide/brain/` — Antigravity IDE 2.0
/// - `antigravity/brain/`     — legacy single-binary layout
///
/// Brain memory is stored under uuid/numeric subdirectories that contain
/// `.json` and/or `.jsonl` files. The walker is layout-tolerant: any
/// directory in any of the three trees with at least one matching file is
/// returned.
async fn gemini_brain_dirs_in(home: &Path) -> Vec<PathBuf> {
    let gemini_home = env_path_when_real_home(home, "GEMINI_HOME");
    gemini_brain_dirs_for_overrides(home, gemini_home.as_deref()).await
}

const GEMINI_BRAIN_SUBROOTS: &[&str] = &["antigravity-cli", "antigravity-ide", "antigravity"];

async fn gemini_brain_dirs_for_overrides(home: &Path, gemini_home: Option<&Path>) -> Vec<PathBuf> {
    let gemini_root = gemini_home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".gemini"));
    let mut results = Vec::new();
    for subroot in GEMINI_BRAIN_SUBROOTS {
        let brain_root = gemini_root.join(subroot).join("brain");
        collect_dirs_with_exts(&brain_root, &mut results, SESSION_FILE_EXTS).await;
    }
    results
}

/// What `discover_recent` should do with one discovered session file.
enum SessionFileDecision {
    /// Surface as a synthesized inline cascade (a brain main transcript whose
    /// metadata header we could build).
    Inline(String),
    /// Surface the file as-is.
    File,
    /// Drop it — a brain sidecar / `transcript_full.jsonl` that isn't a session.
    Skip,
}

/// Decide how to surface one discovered file. Split out of `discover_recent` so
/// the brain-transcript fallback is unit-testable without the discovery walk.
async fn classify_session_file(
    file: &Path,
    cli_history: Option<&[CliHistoryEntry]>,
) -> SessionFileDecision {
    // Brain transcripts carry no structured cwd/session_id at the JSON level.
    // Transform the main transcript into the same cascade shape the local-API
    // probe emits so the agent-agnostic scanner picks up session_id, cwd, and
    // title via its existing extractors.
    if is_brain_transcript_main(file) {
        return match synthesize_brain_inline_payload(file, cli_history).await {
            Some((_uuid, payload)) => SessionFileDecision::Inline(payload),
            // Synthesis can fail (the transcript became unreadable, or it lives
            // under an unrecognized brain subroot). Fall back to surfacing the raw
            // transcript file rather than dropping the session entirely — `main`
            // would otherwise have emitted it as a plain `SessionSource::File`.
            None => SessionFileDecision::File,
        };
    }
    // Under a brain `<uuid>/` tree the only session is the main transcript
    // (handled above). `transcript_full.jsonl` and the sidecar artifacts that
    // share the UUID dir — `task.md.metadata.json`,
    // `implementation_plan.md.metadata.json`, memory files, … — are not sessions.
    // Emitting them would create spurious SessionLogs and, more damagingly, let a
    // sidecar `.json` shadow the real transcript when a session is located by its
    // brain UUID (`locate_session_source` substring-matches the UUID and would
    // otherwise pick the sidecar), leaving the session with no analyzable
    // transcript.
    if brain_origin_of(file).is_some() {
        return SessionFileDecision::Skip;
    }
    SessionFileDecision::File
}

/// True when `path` is a brain transcript whose contents we want to surface
/// as a SessionLog. Today that's the user-facing `transcript.jsonl`; every
/// other artifact under the brain `<uuid>/` tree (including
/// `transcript_full.jsonl`) is skipped by the `brain_origin_of` guard in
/// `discover_recent` to avoid emitting non-session files.
fn is_brain_transcript_main(path: &Path) -> bool {
    brain_uuid_dir(path).is_some()
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.eq_ignore_ascii_case("transcript.jsonl"))
            .unwrap_or(false)
}

/// Returns the `<uuid>` segment from a brain transcript path shaped like
/// `…/brain/<uuid>/.system_generated/logs/<file>`. Layout-strict so it
/// doesn't accidentally claim unrelated brain artifacts.
fn brain_uuid_dir(file: &Path) -> Option<&Path> {
    let logs = file.parent()?;
    if logs.file_name().and_then(|n| n.to_str()) != Some("logs") {
        return None;
    }
    let sysgen = logs.parent()?;
    if sysgen.file_name().and_then(|n| n.to_str()) != Some(".system_generated") {
        return None;
    }
    let uuid_dir = sysgen.parent()?;
    let brain = uuid_dir.parent()?;
    if brain.file_name().and_then(|n| n.to_str()) != Some("brain") {
        return None;
    }
    Some(uuid_dir)
}

/// True when `path` is a spawned Agent-Manager worker — the cascadeId it encodes
/// is in the sidecar's child set. Covers both mirrored cascades
/// (`<cascadeId>.json`) and brain transcripts (`…/brain/<cascadeId>/…`). Used by
/// `discover_recent` to keep workers out of the top-level list. A path whose
/// cascadeId can't be derived is never excluded (fail-open keeps real sessions),
/// matching `codex::is_excluded_subagent`.
fn is_excluded_subagent(path: &Path, child_ids: &HashSet<String>) -> bool {
    antigravity_cascade_id_of(path).is_some_and(|id| child_ids.contains(&id))
}

/// The cascadeId a discovered Antigravity path encodes: the brain `<uuid>` dir
/// for a brain transcript, else the file stem for a `<cascadeId>.json`.
fn antigravity_cascade_id_of(path: &Path) -> Option<String> {
    if let Some(uuid_dir) = brain_uuid_dir(path) {
        return uuid_dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string);
    }
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
}

/// Which Antigravity subroot a brain transcript lives under. Determines
/// which structured cwd source to consult.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrainOrigin {
    /// `~/.gemini/antigravity-cli/brain/<uuid>/...` — has structured
    /// `history.jsonl` workspace records.
    Cli,
    /// `~/.gemini/antigravity-ide/brain/<uuid>/...` — chats embed `Active
    /// Document: <path>` inside USER_INPUT `<ADDITIONAL_METADATA>` blocks
    /// (an IDE-defined contract).
    Ide,
    /// `~/.gemini/antigravity/brain/<uuid>/...` — legacy single-binary
    /// layout. No structured cwd source; best-effort prose sniff only.
    Legacy,
}

/// Classify a brain transcript path by its `.gemini/<subroot>/brain/...`
/// parent. Returns `None` if the path isn't under one of the known subroots.
fn brain_origin_of(file: &Path) -> Option<BrainOrigin> {
    let lower = file
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if lower.contains("/antigravity-cli/brain/") {
        Some(BrainOrigin::Cli)
    } else if lower.contains("/antigravity-ide/brain/") {
        Some(BrainOrigin::Ide)
    } else if lower.contains("/antigravity/brain/") {
        Some(BrainOrigin::Legacy)
    } else {
        None
    }
}

/// One entry from `~/.gemini/antigravity-cli/history.jsonl`. The CLI writes
/// one of these per user-launched session (no UUID — we correlate to the
/// brain transcript via the first step's `created_at`).
#[derive(Debug, Clone)]
struct CliHistoryEntry {
    /// User's first prompt — usable as the session title.
    display: String,
    /// Absolute working directory of the CLI when the session started.
    workspace: String,
    /// Session start time in **epoch seconds** (file stores ms; we
    /// normalize on read).
    timestamp_secs: i64,
}

/// Read `<gemini_root>/antigravity-cli/history.jsonl` and return its
/// entries. Returns `None` if the file is absent or unreadable; callers
/// fall back to prose sniffing in that case.
async fn read_cli_history(gemini_root: &Path) -> Option<Vec<CliHistoryEntry>> {
    // Cap the in-memory read so a corrupted or maliciously crafted history
    // file can't balloon the daemon's memory. 16 MiB is ~50k typical entries.
    const HISTORY_MAX_BYTES: u64 = 16 * 1024 * 1024;
    let path = gemini_root.join("antigravity-cli").join("history.jsonl");
    if let Ok(meta) = tokio::fs::metadata(&path).await
        && meta.len() > HISTORY_MAX_BYTES
    {
        ::tracing::warn!(
            size = meta.len(),
            "antigravity-cli history.jsonl exceeds size cap; skipping"
        );
        return None;
    }
    let raw = tokio::fs::read_to_string(&path).await.ok()?;
    let mut out = Vec::new();
    for line in raw.lines() {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let (Some(display), Some(workspace), Some(timestamp_ms)) = (
            value.get("display").and_then(|v| v.as_str()),
            value.get("workspace").and_then(|v| v.as_str()),
            value.get("timestamp").and_then(|v| v.as_i64()),
        ) else {
            // Skip malformed/partial entries (e.g. older schemas, crashed
            // writes) instead of discarding the whole file.
            ::tracing::debug!("skipping antigravity-cli history entry with missing/typed fields");
            continue;
        };
        out.push(CliHistoryEntry {
            display: display.to_string(),
            workspace: workspace.to_string(),
            timestamp_secs: timestamp_ms / 1000,
        });
    }
    Some(out)
}

/// Find the history entry whose `timestamp_secs` matches `created_at_secs`
/// within ±5 seconds (the CLI logs the session-start timestamp in
/// `history.jsonl` and uses the same instant as the brain transcript's
/// first step's `created_at`, but the two writes can lag slightly).
fn find_cli_history_entry(
    history: &[CliHistoryEntry],
    created_at_secs: i64,
) -> Option<&CliHistoryEntry> {
    history
        .iter()
        .find(|entry| (entry.timestamp_secs - created_at_secs).abs() <= 5)
}

/// Prepend a one-line JSON metadata header to a brain `transcript.jsonl`
/// so the agent-agnostic scanner can extract `sessionId`, `cwd`, and
/// `title` from line 1, then receive the unmodified transcript as the
/// upload body. The header carries only what the scanner needs; we don't
/// rebuild the transcript into the API-cache cascade shape.
///
/// `cli_history` is the parsed contents of
/// `~/.gemini/antigravity-cli/history.jsonl` (when available); it is the
/// authoritative `timestamp → workspace` map for CLI-origin sessions.
/// Callers load it once per `discover_recent` pass and pass `None` for
/// non-CLI origins or when the file is missing.
///
/// Returns `(uuid, payload_string)` or `None` if the file can't be read
/// or isn't laid out as expected.
async fn synthesize_brain_inline_payload(
    file: &Path,
    cli_history: Option<&[CliHistoryEntry]>,
) -> Option<(String, String)> {
    let uuid = brain_uuid_dir(file)?
        .file_name()
        .and_then(|n| n.to_str())?
        .to_string();
    let origin = brain_origin_of(file)?;
    let raw = tokio::fs::read_to_string(file).await.ok()?;

    let (cwd, title) = match origin {
        BrainOrigin::Cli => cli_cwd_and_title_from_history(&raw, cli_history),
        BrainOrigin::Ide | BrainOrigin::Legacy => brain_cwd_and_title_from_prose(&raw),
    };

    let mut header = serde_json::Map::new();
    header.insert("sessionId".to_string(), uuid.clone().into());
    header.insert("source".to_string(), "antigravity_brain".into());
    if let Some(cwd) = cwd {
        header.insert("cwd".to_string(), cwd.into());
    }
    if let Some(title) = title {
        header.insert("title".to_string(), title.into());
    }
    let header_line = serde_json::Value::Object(header).to_string();
    Some((uuid, format!("{header_line}\n{raw}")))
}

/// CLI origin: look up the workspace + display in `history.jsonl` by
/// correlating to the brain transcript's first step `created_at`.
fn cli_cwd_and_title_from_history(
    raw: &str,
    history: Option<&[CliHistoryEntry]>,
) -> (Option<String>, Option<String>) {
    let history = match history {
        Some(h) => h,
        None => return (None, None),
    };
    let created = match first_step_created_at_secs(raw) {
        Some(ts) => ts,
        None => return (None, None),
    };
    match find_cli_history_entry(history, created) {
        Some(entry) => (Some(entry.workspace.clone()), Some(entry.display.clone())),
        None => (None, None),
    }
}

/// IDE / legacy: extract cwd from prose `Active Document:` or
/// `[label](file:///path)` patterns, and title from the first USER_INPUT
/// step's `<USER_REQUEST>` block.
fn brain_cwd_and_title_from_prose(raw: &str) -> (Option<String>, Option<String>) {
    let mut cwd: Option<String> = None;
    let mut title: Option<String> = None;
    for line in raw.lines() {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let raw_content = value.get("content").and_then(|v| v.as_str()).unwrap_or("");
        if cwd.is_none()
            && let Some(uri) = sniff_base_uri_from_step(raw_content)
        {
            cwd = Some(uri_to_directory(&uri));
        }
        if title.is_none() && value.get("type").and_then(|v| v.as_str()) == Some("USER_INPUT") {
            let prose = strip_user_input_wrappers(raw_content);
            if !prose.is_empty() {
                title = Some(prose);
            }
        }
        if cwd.is_some() && title.is_some() {
            break;
        }
    }
    (cwd, title)
}

/// First parseable JSONL line's `created_at` → epoch seconds. The brain
/// transcript's step 0 always carries this — it's the session-start
/// instant we correlate to CLI history.
fn first_step_created_at_secs(raw: &str) -> Option<i64> {
    raw.lines().find_map(|line| {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        let ts = value.get("created_at").and_then(|v| v.as_str())?;
        parse_iso8601_to_epoch_secs(ts)
    })
}

/// Strip a leading `file://` and, if the path looks like a file (has an
/// extension), return its parent directory. The scanner stores `cwd`
/// verbatim, so we normalize here.
fn uri_to_directory(uri: &str) -> String {
    let path_str = uri.strip_prefix("file://").unwrap_or(uri);
    let path = Path::new(path_str);
    if path.extension().is_some() {
        path.parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| path_str.to_string())
    } else {
        path_str.to_string()
    }
}

/// Parse an ISO 8601 / RFC 3339 timestamp string to epoch seconds. Returns
/// `None` on parse failure. We use this only for correlating brain
/// transcripts to CLI history entries.
fn parse_iso8601_to_epoch_secs(s: &str) -> Option<i64> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(|dt| dt.unix_timestamp())
}

/// Pull a `file://`-form cwd hint out of one step's prose `content`.
///
/// Two known emitters:
/// - IDE 2.0 USER_INPUT: `Active Document: /abs/path/file.ext (LANGUAGE_RUST)`
///   inside `<ADDITIONAL_METADATA>`. The parent dir is the cwd; we still
///   return the file path here and let the scanner's `extract_cwd_from_value`
///   → `normalize_cwd_path` collapse it to the parent (`looks_like_file`
///   based on extension).
/// - CLI PLANNER_RESPONSE: a markdown link `[label](file:///abs/path)` that
///   echoes the project directory.
fn sniff_base_uri_from_step(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("Active Document: ") {
            let path = match rest.rfind(" (LANGUAGE_") {
                Some(idx) => &rest[..idx],
                None => rest,
            }
            .trim();
            if !path.is_empty() {
                return Some(format!("file://{}", path));
            }
        }
    }
    if let Some(start) = content.find("](file://") {
        let after = &content[start + 2..];
        if let Some(end) = after.find(')') {
            let uri = &after[..end];
            if !uri.is_empty() {
                return Some(uri.to_string());
            }
        }
    }
    None
}

/// Strip the `<USER_REQUEST>...</USER_REQUEST>` wrapper (and adjacent
/// `<ADDITIONAL_METADATA>` / `<USER_SETTINGS_CHANGE>` blocks) from a
/// USER_INPUT step's content so the scanner's title extractor sees clean
/// user prose.
fn strip_user_input_wrappers(content: &str) -> String {
    let mut out = String::new();
    let mut search_start = 0usize;
    while let Some(open_rel) = content[search_start..].find("<USER_REQUEST>") {
        let open = search_start + open_rel + "<USER_REQUEST>".len();
        let Some(close_rel) = content[open..].find("</USER_REQUEST>") else {
            break;
        };
        let close = open + close_rel;
        let segment = content[open..close].trim();
        if !segment.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(segment);
        }
        search_start = close + "</USER_REQUEST>".len();
    }
    if out.is_empty() {
        // No wrapper found — return the original prose (trimmed) so we don't
        // accidentally swallow the user's text on a non-standard layout.
        content.trim().to_string()
    } else {
        out
    }
}

#[cfg(test)]
pub(crate) fn sample_workspace_log_path(home: &Path) -> PathBuf {
    app_config_dir_in("Antigravity", home)
        .join("User")
        .join("workspaceStorage")
        .join("x")
        .join("chatSessions")
        .join("session.json")
}

#[cfg(test)]
pub(crate) fn sample_cli_brain_log_path(home: &Path) -> PathBuf {
    home.join(".gemini")
        .join("antigravity-cli")
        .join("brain")
        .join("12345")
        .join("trace.jsonl")
}

#[cfg(test)]
pub(crate) fn sample_ide_brain_log_path(home: &Path) -> PathBuf {
    home.join(".gemini")
        .join("antigravity-ide")
        .join("brain")
        .join("3afb6691-6ba3-4a01-bd37-df54a0c1ee82")
        .join(".system_generated")
        .join("logs")
        .join("transcript.jsonl")
}

#[cfg(test)]
#[path = "tests/antigravity_tests.rs"]
mod tests;
