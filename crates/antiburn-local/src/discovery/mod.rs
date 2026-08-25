// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local discovery of AI coding-agent sessions.
//!
//! Discovery answers three questions about the machine it runs on: which agent
//! sessions exist, where each one ran, and what to call it. Everything here
//! reads documented on-disk layouts, read-only vendor databases, and bounded
//! WSL paths — discovery itself opens no socket, probes no running process,
//! and calls no vendor's local API. That is a property of this module, not a
//! rule antiburn holds everywhere: an embedding application that wants live
//! enrichment (inspecting a running agent, calling it over loopback) layers
//! that on top (see [`SessionMirror`]).
//!
//! # Layout
//!
//! - [`AgentExplorer`] is the per-agent contract: recent sessions, path
//!   ownership, surface classification, titles, sub-agents.
//! - [`agents`] holds one adapter per supported agent.
//! - [`Explorers`] binds a set of adapters together and provides the
//!   fan-out and point-lookup entry points callers actually use.
//! - [`scanner`] turns a session's bytes into [`scanner::SessionMetadata`].
//! - [`fork`] carries locally observed session lineage.
//!
//! # Extending an adapter
//!
//! Adapters are `&'static` values, so an application can construct its own
//! configured copies (extra mirror directories, a duplicate-fork detector) and
//! hand [`Explorers::new`] a lookup that returns them. [`Explorers::DISK`] is
//! the unconfigured, disk-only default.

pub mod agents;
pub mod fork;
pub mod scanner;
pub mod source_version;

use crate::model::AgentKind;
use crate::platform::environment::{DiscoveryEnvironment, WslEnvironmentInfo};
use async_trait::async_trait;
use futures_util::{StreamExt as _, stream};
use scanner::TitleSource;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

pub use fork::{DuplicateForkDetector, FORK_OBSERVATION_KEY, ForkObservation};
pub use source_version::{
    FingerprintInputs, SourceDescriptor, SourceStat, SourceVersion, Streamability,
};

/// How many session logs may have their metadata read concurrently. Bounds the
/// open-file and blocking-pool pressure of a whole-machine scan.
const LOG_METADATA_CONCURRENCY: usize = 16;

/// Upper bound on how much of a session file the metadata path reads.
///
/// Metadata, visibility, and lineage all live near the top of a transcript, so
/// a multi-gigabyte log never needs to be materialized in full to be described.
const SOURCE_PREVIEW_BYTES: u64 = 16 * 1024 * 1024;

/// Maximum transcript suffix read to recover semantic activity. Activity
/// records are append-oriented in the JSONL formats we support; a suffix is
/// enough to find the latest meaningful event without materialising a large
/// historical transcript on every scan.
pub const ACTIVITY_TAIL_BYTES: u64 = 256 * 1024;

/// A session whose transcript was written within this window is treated as
/// "live" — the mtime heartbeat, since no agent writes an explicit
/// in-progress flag. Shared by every surface that reports live sessions
/// (including [`LiveSessionIndex`]) so they all agree on what counts.
pub const ACTIVE_SESSION_WINDOW_SECS: i64 = 180;

/// Matcher precedence for path-based agent inference.
///
/// This is the single source of truth for explorer precedence, consumed by
/// [`Explorers::infer_agent_type`] (substring [`AgentExplorer::owns_path`]
/// cascade) and [`Explorers::infer_agent_and_surface`] (root-prefix
/// [`AgentExplorer::surface_paths`] cascade); a reorder here changes both
/// atomically. It is deliberately *not*
/// [`AgentKind::ALL`](crate::model::AgentKind::ALL), which is display order.
///
/// ## Precedence rationale (top → bottom)
///
/// Narrowest, most-specific matchers come first; broad / loose matchers
/// come last so they can't silently steal paths owned by a sibling.
///
/// 1. **Codex** — narrow `.codex/sessions/` fragment, unambiguous.
/// 2. **OpenCode** — XDG / `.local/share/opencode/` fragments, distinct
///    from any other agent's tree.
/// 3. **Cline** — `.cline/` plus extension-id substrings; specific enough
///    to win over any IDE-app-config match that happens to contain
///    `cline.cline`.
/// 4. **Kiro** — Kiro IDE-config-specific paths; no overlap.
/// 5. **Amp Code** — narrow `/amp/threads/` and `.amp/file-changes/`
///    fragments.
/// 6. **Pi** — narrow `/.pi/agent/` fragment; CLI-only.
/// 7. **Antigravity** — checked *before* Copilot/Cursor: its IDE chat tree
///    sits at `User/workspaceStorage/...` which a loosened Copilot/Cursor
///    matcher could otherwise claim.
/// 8. **Copilot** — checked *before* Cursor for the same reason: its
///    VS Code chatSessions path is more specific and must not be stolen by
///    a future relaxed Cursor substring match.
/// 9. **Windsurf** — `.codeium/windsurf/` and Windsurf-app-config paths;
///    specific.
/// 10. **Cursor** — broadest of the IDE-family matchers; placed late so
///     the narrower agents above win first.
/// 11. **Claude** — fallback default. `ClaudeExplorer::owns_path` returns
///     `false` by design, so iteration always falls through to the
///     `unwrap_or(Claude)` arm in [`Explorers::infer_agent_type`] — Claude owns
///     "the rest".
pub const MATCHER_PRECEDENCE: &[AgentKind] = &[
    AgentKind::Codex,
    AgentKind::OpenCode,
    AgentKind::Cline,
    AgentKind::Kiro,
    AgentKind::AmpCode,
    AgentKind::Pi,
    AgentKind::Antigravity,
    AgentKind::Copilot,
    AgentKind::Windsurf,
    AgentKind::Cursor,
    AgentKind::Claude,
];

/// A directory of session files an embedding application mirrors outside the
/// vendor's own layout.
///
/// Some vendors keep conversations somewhere discovery cannot read them
/// directly. An application that obtains them another way can write them to a
/// directory of its own and register it here; the adapter then walks it exactly
/// like a vendor directory. Discovery never creates, writes, or refreshes a
/// mirror — it only reads whatever is already there.
#[derive(Clone, Copy)]
pub struct SessionMirror {
    /// Resolves the mirror directory for a user home. `None` disables it.
    pub dir: fn(&Path) -> Option<PathBuf>,
    /// Lowercased, forward-slashed path substring that identifies the mirror
    /// in [`AgentExplorer::owns_path`]. `None` leaves mirror paths unclaimed.
    pub path_marker: Option<&'static str>,
}

impl SessionMirror {
    /// No mirror: the adapter reads only the vendor's own layout.
    pub const NONE: SessionMirror = SessionMirror {
        dir: |_| None,
        path_marker: None,
    };

    /// The mirror directory for `home`, when configured.
    pub fn dir_in(&self, home: &Path) -> Option<PathBuf> {
        (self.dir)(home)
    }

    /// Whether `path_lower` sits inside this mirror.
    pub fn owns(&self, path_lower: &str) -> bool {
        self.path_marker
            .is_some_and(|marker| path_lower.contains(marker))
    }

    /// The mirror's roots for [`SurfacePaths::mirror`], empty when unset.
    pub fn roots_in(&self, home: &Path) -> Vec<PathBuf> {
        self.dir_in(home).into_iter().collect()
    }
}

impl std::fmt::Debug for SessionMirror {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionMirror")
            .field("path_marker", &self.path_marker)
            .finish_non_exhaustive()
    }
}

/// A resolved session title, carrying provenance so consumers can distinguish
/// a user rename from an AI summarisation or a first-message fallback.
///
/// Returned by [`AgentExplorer::session_title`] (the point-query path) and
/// surfaced through the [`SessionTitleAndSurface`] batch path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTitle {
    pub text: String,
    pub source: TitleSource,
}

impl ResolvedTitle {
    /// Construct a title through the same whitespace and length policy used by
    /// transcript-derived metadata. Vendor indexes can contain full prompts,
    /// so every resolved-title path must enforce the boundary here as well.
    pub fn new(text: impl Into<String>, source: TitleSource) -> Self {
        Self {
            text: scanner::normalize_title(&text.into()),
            source,
        }
    }
}

/// Whether an agent's `session_title` is cheap enough to invoke on a
/// latency-sensitive caller (one query per requested id) or whether the caller
/// should prefer the batched `session_titles_and_surfaces` scan path.
///
/// `Direct` agents (Claude, Codex, OpenCode) have a per-session index
/// (per-id JSONL file, SQLite row keyed by id) so point queries are O(1).
/// `Scan` agents have no such index and would need to walk the full agent
/// tree per id — much cheaper to scan once and lookup against the result map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleLookupKind {
    Direct,
    Scan,
}

/// Result of a latency-sensitive point lookup. `Unsupported` means the caller
/// should use the generic fallback scan; `Missing` means the agent's direct
/// index is authoritative and the requested id is not present.
#[derive(Debug)]
pub enum DirectSessionSource {
    Found(SessionSource),
    Missing,
    Unsupported,
}

// Test-only sample paths:
//
// Each agent module exposes one or more `#[cfg(test)] pub(crate) fn
// sample_*_log_path` helpers that return a canonical example path under a
// synthetic `home`. They are consumed by `scanner::tests` to drive
// `infer_agent_type` tests so that path-shape knowledge stays co-located
// with the agent's `owns_path` implementation. If you change an agent's
// on-disk layout, update both `owns_path` and the sample helper.

/// One agent's view of the local machine.
///
/// Implementations are stateless and cheap to construct; discovery holds them
/// as `&'static dyn AgentExplorer`. Only [`Self::discover_recent`] and
/// [`Self::owns_path`] are required — every other hook has a default that is
/// correct (if unoptimized) for an agent that does not need it.
#[async_trait]
pub trait AgentExplorer: Send + Sync {
    async fn discover_recent(&self, now: i64, since_secs: i64) -> Vec<SessionLog>;

    /// Optional O(1)-ish point lookup for agents with a durable per-session
    /// index. The default fallback scans the agent's whole store via
    /// [`Self::discover_recent`].
    async fn direct_session_source(&self, _session_id: &str) -> DirectSessionSource {
        DirectSessionSource::Unsupported
    }

    /// Optional freshness fingerprint for database-backed sessions. Lets callers
    /// check caches before rendering a database-backed session into JSONL.
    async fn provider_db_fingerprint(
        &self,
        _db_path: &Path,
        _session_id: &str,
    ) -> Option<(u64, u64)> {
        None
    }

    /// Substring-based path-shape predicate: "is this path mine?"
    ///
    /// `path_lower` is already lowercased and forward-slashed (see
    /// [`normalize_for_matching`]). Implementations match on substrings
    /// unique to their agent — no `home` arg, no root resolution, so it
    /// stays fork-tolerant (unknown VS Code derivatives, custom install
    /// prefixes, etc.). Precedence between overlapping agents is the
    /// caller's job via [`MATCHER_PRECEDENCE`] ordering.
    ///
    /// **Substrings are NOT `#[cfg(target_os = "…")]`-gated on purpose.**
    /// Every platform-specific substring (macOS `/library/application support/…`,
    /// Linux `/.config/…`, Windows `/appdata/roaming/…`) is evaluated on
    /// every build target. This is deliberate so that foreign-origin paths
    /// (synced dotfile backups, fixtures, snapshots ingested from another
    /// machine) still classify correctly — recognition is a property of
    /// the string, not the running OS. Platform awareness belongs in
    /// [`Self::surface_paths`], which resolves roots via the cfg-aware
    /// `app_config_dir_in` helper. Do not propose cfg-gating these
    /// substrings as a "cleanup".
    ///
    /// Pair with [`Self::surface_paths`] for home-anchored classification.
    fn owns_path(&self, path_lower: &str) -> bool;

    /// Fast CWD-only discovery for repo detection.
    /// Default falls back to discover_recent + parse_session_metadata (parallel).
    async fn discover_cwds(&self, now: i64, since_secs: i64) -> Vec<String> {
        let logs = self.discover_recent(now, since_secs).await;
        bounded_log_tasks(logs, |log| async move {
            let source = log.source;
            session_source_metadata(&source, None)
                .await
                .and_then(|m| m.cwd)
        })
        .await
    }

    /// Scan recent sessions for this agent once, returning
    /// `(session_id, title, surface)` for every session whose `session_id`
    /// could be extracted. Used by the batched title-fetch path to amortize
    /// one scan across all requested IDs and to record the surface of every
    /// encountered session as a side effect.
    ///
    /// Default impl loads each `SessionLog`'s content once, parses
    /// metadata, and computes the surface via
    /// [`SessionLog::surface_label_with_content`] so bi-modal Claude /
    /// Codex disambiguation still works. Agents whose titles don't live
    /// in transcript content (OpenCode — SQLite) override this with a
    /// cheaper query.
    async fn session_titles_and_surfaces(&self) -> Vec<SessionTitleAndSurface> {
        default_session_titles_and_surfaces(self).await
    }

    /// Lookup-kind hint for title-fetch routing. See [`TitleLookupKind`].
    /// Defaults to `Scan` so a new agent inherits the safe batched-scan
    /// behavior without an explicit override.
    fn title_lookup_kind(&self) -> TitleLookupKind {
        TitleLookupKind::Scan
    }

    /// Resolve a title only from a durable vendor index or database, without
    /// opening transcript content. Background scanners can combine this with
    /// metadata they already read, avoiding a second unbounded transcript
    /// pass when an index has no row. Agents without a separate title store
    /// return `None`.
    async fn indexed_session_title(&self, _agent_session_id: &str) -> Option<ResolvedTitle> {
        None
    }

    /// Batch variant of [`Self::indexed_session_title`]. Adapters backed by a
    /// shared index should override this so the index is opened once per scan.
    async fn indexed_session_titles(
        &self,
        agent_session_ids: &[String],
    ) -> std::collections::HashMap<String, ResolvedTitle> {
        let mut titles = std::collections::HashMap::new();
        for session_id in agent_session_ids {
            if let Some(title) = self.indexed_session_title(session_id).await {
                titles.insert(session_id.clone(), title);
            }
        }
        titles
    }

    /// Resolve a single session's title by id, returning a [`ResolvedTitle`]
    /// with provenance. Direct-kind agents (Claude / Codex / OpenCode)
    /// override this with a point query against a per-session index. The
    /// default falls back to the batch path and looks up the requested id
    /// against the result — correct but O(N) per call, so prefer overriding
    /// for any agent a latency-sensitive caller will hit.
    async fn session_title(&self, agent_session_id: &str) -> Option<ResolvedTitle> {
        default_session_title(self, agent_session_id).await
    }

    /// Classify a `SessionLog` produced by this explorer to a stable surface
    /// label suitable for `tracing` fields and wire types. Returns
    /// one of `"cli"`, `"ide_desktop"`, or `"unknown"`.
    ///
    /// `content` is the raw session content when the caller has already
    /// loaded it; bi-modal content-disambiguated agents (Claude
    /// `entrypoint`, Codex `session_meta.source`) consult it first. `home`
    /// resolves the per-agent surface roots used for path-based matching.
    ///
    /// The default impl is sufficient for pure-path agents: it consults
    /// `surface_paths(home)`, prefix-matches the log path against each
    /// surface's roots, and falls back to [`Self::unmatched_surface`].
    /// Override only when a content marker should win over path matching
    /// (Claude, Codex) or when an inline-virtual label needs special
    /// handling (e.g. Claude's `claude-desktop:` short-circuit).
    fn session_surface_label(
        &self,
        log: &SessionLog,
        _content: Option<&str>,
        home: &Path,
    ) -> &'static str {
        classify_source_against_surface_paths(&log.source, &self.surface_paths(home))
            .unwrap_or_else(|| self.unmatched_surface())
    }

    /// Surface to return from the default [`Self::session_surface_label`]
    /// when no `surface_paths` root matches the log path.
    ///
    /// Defaults to `"unknown"` as a safe fallback for new agents that
    /// haven't yet declared their unmatched policy. Consumed by:
    ///   - the default [`Self::session_surface_label`] impl (every agent
    ///     except Claude and Codex), and
    ///   - Claude's `session_surface_label` override, which delegates
    ///     here after its `entrypoint` content check misses.
    ///
    /// Codex does NOT delegate here — its override returns `"unknown"`
    /// directly because the path-shared `~/.codex/sessions/**` tree
    /// requires content disambiguation. Override values in use today:
    ///   - Bi-modal path-based agents (Cursor, Copilot, Cline,
    ///     Antigravity): `"ide_desktop"` — preserves the historical
    ///     "anything-not-CLI is IDE" behavior since IDE roots can vary
    ///     across forked VS Code-family installs we don't enumerate.
    ///   - Single-surface agents (OpenCode, AmpCode → `"cli"`;
    ///     Kiro, Windsurf → `"ide_desktop"`): their only surface.
    ///   - Claude → `"cli"`: matches the historical default for
    ///     `~/.claude/projects/**` when no IDE root matches.
    fn unmatched_surface(&self) -> &'static str {
        "unknown"
    }

    /// Concrete directory roots this agent owns, grouped by surface.
    ///
    /// `home` is the synthetic-or-real user home, mirroring the
    /// `*_in(home)` convention used elsewhere in this module so tests can
    /// pass a `TempDir` without env mutation. Returns *root* directories
    /// only (no glob walking); callers do prefix matching to classify a
    /// concrete path back to `(agent, surface)`.
    ///
    /// Mono-surface agents return an empty vec for the unused surface.
    /// Agents whose CLI and IDE variants share a filesystem location
    /// (Codex `~/.codex/sessions/**`, Claude `~/.claude/projects/**`)
    /// expose the path only under its primary surface; callers must rely
    /// on `session_surface_label` with content to disambiguate.
    ///
    /// Default returns empty so the trait change is additive and
    /// explorers can opt in incrementally.
    fn surface_paths(&self, _home: &Path) -> SurfacePaths {
        SurfacePaths::default()
    }

    /// Optional per-agent hook for recovering a session ID from the on-disk
    /// path when the scanner's content parse didn't surface one. Called from
    /// `scanner::apply_metadata_from_path` only if `metadata.session_id` is
    /// still `None` after content parsing.
    ///
    /// Used by agents whose session ID lives only in the filename
    /// (Pi: `{timestamp}_{uuid}.jsonl` → UUID-suffix recovery). Default
    /// returns `None` so the trait stays additive.
    fn recover_session_id_from_path(&self, _file: &Path) -> Option<String> {
        None
    }

    /// Whether this vendor records sub-agent (orchestration) spawns at all.
    /// Gates the parent-session lookup in [`Explorers::list_subagents`]: when
    /// `false`, callers skip locating the parent transcript instead of scanning
    /// the vendor's tree only to apply the empty [`Self::list_subagents`]
    /// default. `true` only for vendors overriding the sub-agent hooks
    /// (Claude, Codex, Antigravity).
    fn supports_subagents(&self) -> bool {
        false
    }

    /// List a parent transcript's sub-agent transcript files, sorted for a
    /// stable roster order. Sub-agents are an orchestration concept: a parent
    /// session that spawns child agents writes each as its own transcript
    /// (Claude: `<id>/subagents/agent-*.jsonl`). Default returns empty — only
    /// agents that write a sub-agent tree override this, so every other vendor
    /// is correctly treated as "not an orchestrator" with no caller-side enum
    /// gate. `parent_transcript` is the parent session's on-disk file.
    async fn list_subagents(&self, _parent_transcript: &Path) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Resolve a single sub-agent transcript by `(parent_transcript,
    /// subagent_id)`, for analysis or deep-linking. Implementations MUST
    /// validate `subagent_id` so a crafted id can't escape the sub-agent
    /// directory. Default returns `None` (no sub-agents).
    async fn locate_subagent(
        &self,
        _parent_transcript: &Path,
        _subagent_id: &str,
    ) -> Option<PathBuf> {
        None
    }

    /// The deep-link id identifying a sub-agent transcript path (Claude's
    /// `agent-<hash>` file stem). Default returns `None`.
    fn subagent_id(&self, _path: &Path) -> Option<String> {
        None
    }

    /// A sub-agent's display label for the roster: the resolved session title,
    /// with vendor-specific fallbacks. Default is a generic label, used only
    /// when an overriding explorer hasn't supplied one.
    async fn subagent_label(&self, _path: &Path) -> String {
        "Sub-agent".to_string()
    }

    /// Read the sidecar metadata a vendor writes next to a sub-agent
    /// transcript. Default returns `None` (no sidecar).
    async fn subagent_meta(&self, _path: &Path) -> Option<SubagentMeta> {
        None
    }
}

async fn bounded_log_tasks<T, F, Fut>(logs: Vec<SessionLog>, task: F) -> Vec<T>
where
    F: FnMut(SessionLog) -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    stream::iter(logs)
        .map(task)
        .buffered(LOG_METADATA_CONCURRENCY)
        .filter_map(std::future::ready)
        .collect()
        .await
}

/// Lowercase + forward-slash a path / label string for prefix or substring
/// matching. Mirrors the normalization used by `infer_agent_type`.
pub fn normalize_for_matching(input: &str) -> String {
    input.replace('\\', "/").to_ascii_lowercase()
}

/// How a normalized `SessionSource` payload should be compared against a
/// surface root.
#[derive(Debug, Clone, Copy)]
enum PayloadKind {
    /// Absolute filesystem path — use `starts_with` (anchored match).
    AbsolutePath,
    /// Inline-virtual label produced by an agent (e.g. `antigravity-brain:<path>`,
    /// `cursor-store:<path>`). The label may embed a file path mid-string,
    /// so use `contains`. This is safe today because every Inline label
    /// that needs to match a surface root embeds the absolute path inside
    /// its body; labels that do not (`opencode:<id>`, `cursor-desktop:<ws>:<id>`)
    /// simply fail to match and fall through to `unmatched_surface()`.
    InlineLabel,
}

struct NormalizedPayload {
    text: String,
    kind: PayloadKind,
}

impl NormalizedPayload {
    fn from_source(source: &SessionSource) -> Self {
        match source {
            SessionSource::File(p) => Self::from_path(p),
            SessionSource::Inline { label, .. } => Self {
                text: normalize_for_matching(label),
                kind: PayloadKind::InlineLabel,
            },
            SessionSource::ProviderDb {
                agent, session_id, ..
            } => Self {
                text: normalize_for_matching(&format!("{agent}:{session_id}")),
                kind: PayloadKind::InlineLabel,
            },
        }
    }

    fn from_path(path: &Path) -> Self {
        Self {
            text: normalize_for_matching(&path.to_string_lossy()),
            kind: PayloadKind::AbsolutePath,
        }
    }

    fn matches_root(&self, root: &Path) -> bool {
        let root_norm = normalize_for_matching(&root.to_string_lossy());
        if root_norm.is_empty() {
            return false;
        }
        match self.kind {
            PayloadKind::AbsolutePath => self.text.starts_with(&root_norm),
            PayloadKind::InlineLabel => self.text.contains(&root_norm),
        }
    }

    fn classify(&self, surface_paths: &SurfacePaths) -> Option<&'static str> {
        if surface_paths.cli.iter().any(|root| self.matches_root(root)) {
            return Some("cli");
        }
        if surface_paths
            .ide_desktop
            .iter()
            .any(|root| self.matches_root(root))
        {
            return Some("ide_desktop");
        }
        // `mirror` is an organizational vec for an application's own copy of
        // conversations it obtained elsewhere. Today's two consumers
        // (antigravity, windsurf) mirror IDE-mode conversations, so we surface
        // those paths as `ide_desktop`. A future CLI-sourced mirror should be
        // registered under `cli` instead.
        if surface_paths
            .mirror
            .iter()
            .any(|root| self.matches_root(root))
        {
            return Some("ide_desktop");
        }
        None
    }
}

/// Classify a `SessionSource`'s path/label against the agent's `SurfacePaths`.
///
/// Returns `Some("cli")` / `Some("ide_desktop")` on first matching root,
/// `None` if no root matches. See [`PayloadKind`] for the per-source
/// matching strategy. All comparisons are performed on lowercased
/// forward-slashed strings so Windows-style backslash paths classify
/// correctly without producer-side normalization.
pub fn classify_source_against_surface_paths(
    source: &SessionSource,
    surface_paths: &SurfacePaths,
) -> Option<&'static str> {
    NormalizedPayload::from_source(source).classify(surface_paths)
}

/// Classify a raw filesystem path against the agent's `SurfacePaths`.
/// Sibling of [`classify_source_against_surface_paths`] for callers that
/// don't have a `SessionSource` in hand (e.g. inference over a candidate
/// path that hasn't been opened yet).
pub fn classify_path_against_surface_paths(
    path: &Path,
    surface_paths: &SurfacePaths,
) -> Option<&'static str> {
    NormalizedPayload::from_path(path).classify(surface_paths)
}

/// Per-surface directory roots returned by [`AgentExplorer::surface_paths`].
///
/// Paths are the same `PathBuf`s the explorer's `discover_recent` walks,
/// in `home`-relative form. Empty vecs mean "this agent does not own any
/// path under this surface" (e.g. mono-surface agents).
///
/// `mirror` is a third bucket for an embedding application's [`SessionMirror`]
/// directory. It exists so callers can distinguish mirrored content from the
/// vendor's own on-disk transcripts; the classifier maps it to `ide_desktop`
/// (see `NormalizedPayload::classify`) since today's consumers mirror IDE
/// conversations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SurfacePaths {
    pub cli: Vec<PathBuf>,
    pub ide_desktop: Vec<PathBuf>,
    pub mirror: Vec<PathBuf>,
}

/// VS Code-family desktop apps whose `User/globalStorage/<ext-id>/` trees
/// host third-party extensions (Cline, …). Shared between agents
/// so a new fork only needs to be added in one place.
pub const VS_CODE_FAMILY_APPS: &[&str] = &["Code", "Cursor", "Windsurf", "VSCodium"];

/// Build the list of `<app-config>/<app>/User/globalStorage/<ext>/tasks`
/// roots for each `(app, ext_id)` pair, using `app_config_dir_in` so paths
/// resolve correctly on macOS / Windows / Linux.
pub fn vs_code_global_storage_task_roots(home: &Path, ext_ids: &[&str]) -> Vec<PathBuf> {
    let mut roots = Vec::with_capacity(VS_CODE_FAMILY_APPS.len() * ext_ids.len());
    for app in VS_CODE_FAMILY_APPS {
        let base = app_config_dir_in(app, home)
            .join("User")
            .join("globalStorage");
        for ext in ext_ids {
            roots.push(base.join(ext).join("tasks"));
        }
    }
    roots
}

/// One row of the batch result produced by
/// [`AgentExplorer::session_titles_and_surfaces`]. Each row corresponds to
/// one discoverable session for the agent. Callers index by `session_id`
/// to resolve titles for a UI batch and to record the surface for
/// every encountered row (not just the requested ones).
#[derive(Debug, Clone)]
pub struct SessionTitleAndSurface {
    pub session_id: String,
    pub title: Option<String>,
    /// Provenance of `title` when present. Lets a consumer filter AI-title
    /// refinements out of rename detection.
    pub title_source: Option<TitleSource>,
    pub surface: &'static str,
}

/// Window for the title+surface batch scan. Matches the activity list's
/// 30-day window so any row a caller could possibly show is in range.
const SESSION_TITLE_SCAN_WINDOW_SECS: i64 = 60 * 60 * 24 * 30;

/// Shared default body for `AgentExplorer::session_titles_and_surfaces`.
/// Reads each `SessionLog`'s content once, parses metadata, and classifies
/// surface via `SessionLog::surface_label_with_content` (so Claude
/// `entrypoint` / Codex `session_meta.source` content peeks still apply).
async fn default_session_titles_and_surfaces<E: AgentExplorer + ?Sized>(
    explorer: &E,
) -> Vec<SessionTitleAndSurface> {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let logs = explorer
        .discover_recent(now, SESSION_TITLE_SCAN_WINDOW_SECS)
        .await;
    let home = home_dir().unwrap_or_default();

    bounded_log_tasks(logs, |log| {
        let home = home.clone();
        async move {
            let content = match &log.source {
                SessionSource::File(path) => tokio::fs::read_to_string(path).await.ok()?,
                SessionSource::Inline { content, .. } => content.clone(),
                SessionSource::ProviderDb { .. } => session_source_content(&log.source).await?,
            };
            let metadata = match &log.source {
                SessionSource::File(path) => {
                    scanner::parse_session_metadata_with_content(path, &content).await
                }
                SessionSource::Inline { .. } => scanner::parse_session_metadata_str(&content),
                SessionSource::ProviderDb { .. } => session_log_metadata(&log)
                    .await
                    .unwrap_or_else(|| scanner::parse_session_metadata_str(&content)),
            };
            let session_id = metadata.session_id?;
            let surface = log.surface_label_with_content(&content, &home);
            Some(SessionTitleAndSurface {
                session_id,
                title: metadata.title,
                title_source: metadata.title_source,
                surface,
            })
        }
    })
    .await
}

/// Shared default body for `AgentExplorer::session_title`. Falls back to the
/// batch path and looks up `agent_session_id` against the result map. This
/// is O(N) per call (scans the full agent tree) — agents on a latency-sensitive
/// path should override with a point query.
async fn default_session_title<E: AgentExplorer + ?Sized>(
    explorer: &E,
    agent_session_id: &str,
) -> Option<ResolvedTitle> {
    let rows = explorer.session_titles_and_surfaces().await;
    let row = rows
        .into_iter()
        .find(|r| r.session_id == agent_session_id)?;
    let text = row.title?;
    let source = row.title_source.unwrap_or(TitleSource::Explicit);
    Some(ResolvedTitle::new(text, source))
}

/// Resolves an [`AgentKind`] to the explorer that handles it.
///
/// All current explorers are `&'static` values, so the returned references are
/// free. An embedding application that configures an adapter (a
/// [`SessionMirror`], a [`DuplicateForkDetector`]) declares its own configured
/// statics and supplies a lookup that returns them.
pub type ExplorerLookup = fn(&AgentKind) -> &'static dyn AgentExplorer;

/// The set of agent adapters discovery dispatches through, plus every entry
/// point that fans out over them.
///
/// [`Explorers::DISK`] is the built-in, unconfigured set. Applications that
/// extend an adapter build their own with [`Explorers::new`] and call the same
/// methods.
#[derive(Clone, Copy)]
pub struct Explorers {
    lookup: ExplorerLookup,
}

impl std::fmt::Debug for Explorers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Explorers")
    }
}

/// The unconfigured adapter for each agent: vendor layouts only, no mirrors and
/// no application-supplied detectors.
pub fn disk_explorer_for(kind: &AgentKind) -> &'static dyn AgentExplorer {
    match kind {
        AgentKind::Claude => &agents::claude::ClaudeExplorer,
        AgentKind::Codex => &agents::codex::CodexExplorer,
        AgentKind::Cursor => &agents::cursor::DISK_CURSOR,
        AgentKind::Copilot => &agents::copilot::CopilotExplorer,
        AgentKind::Cline => &agents::cline::ClineExplorer,
        AgentKind::OpenCode => &agents::opencode::OpenCodeExplorer,
        AgentKind::Kiro => &agents::kiro::KiroExplorer,
        AgentKind::AmpCode => &agents::amp_code::AmpCodeExplorer,
        AgentKind::Antigravity => &agents::antigravity::DISK_ANTIGRAVITY,
        AgentKind::Windsurf => &agents::windsurf::DISK_WINDSURF,
        AgentKind::Pi => &agents::pi::PiExplorer,
    }
}

impl Explorers {
    /// The built-in, disk-only adapter set.
    pub const DISK: Explorers = Explorers {
        lookup: disk_explorer_for,
    };

    /// An adapter set backed by an application's own lookup.
    pub const fn new(lookup: ExplorerLookup) -> Self {
        Self { lookup }
    }

    /// The explorer handling `agent`.
    pub fn get(&self, agent: &AgentKind) -> &'static dyn AgentExplorer {
        (self.lookup)(agent)
    }

    /// Resolve one `(agent, session_id)` pair's title.
    pub async fn session_title_for(
        &self,
        agent: &AgentKind,
        session_id: &str,
    ) -> Option<ResolvedTitle> {
        self.get(agent).session_title(session_id).await
    }

    /// Resolve a batch from one agent's durable index or database only.
    pub async fn indexed_session_titles_for(
        &self,
        agent: &AgentKind,
        session_ids: &[String],
    ) -> std::collections::HashMap<String, ResolvedTitle> {
        self.get(agent).indexed_session_titles(session_ids).await
    }

    /// The lookup-kind hint, so a caller can route between per-id point queries
    /// and the batched scan path without taking a scan lock just to ask.
    pub fn title_lookup_kind_for(&self, agent: &AgentKind) -> TitleLookupKind {
        self.get(agent).title_lookup_kind()
    }

    /// Infer the agent from a file path using each agent's `owns_path`.
    ///
    /// Iterates [`MATCHER_PRECEDENCE`] and returns the first matching agent.
    /// Claude is the fallback — its `owns_path` returns false by design, so the
    /// iteration naturally falls through to the `unwrap_or` arm. To reorder
    /// precedence, edit [`MATCHER_PRECEDENCE`]; this function tracks it
    /// automatically.
    pub fn infer_agent_type(&self, path: &Path) -> AgentKind {
        let path_lower = normalize_for_matching(&path.to_string_lossy());
        MATCHER_PRECEDENCE
            .iter()
            .copied()
            .find(|ty| self.get(ty).owns_path(&path_lower))
            .unwrap_or(AgentKind::Claude)
    }

    /// The per-surface path registry for a given agent, resolved against `home`.
    pub fn surface_paths_for(&self, agent: &AgentKind, home: &Path) -> SurfacePaths {
        self.get(agent).surface_paths(home)
    }

    /// The per-agent filename-based session-id recovery hook. Called by
    /// `scanner::apply_metadata_from_path` when content parsing did not surface
    /// a `session_id`. Most agents return `None`; Pi recovers the UUID suffix
    /// from its `{timestamp}_{uuid}.jsonl` filenames.
    pub fn recover_session_id_from_path(&self, agent: &AgentKind, file: &Path) -> Option<String> {
        self.get(agent).recover_session_id_from_path(file)
    }

    /// Classify `path` to its owning `(AgentKind, surface)` by asking each
    /// explorer for its surface-scoped roots and prefix-matching the
    /// lowercased, forward-slashed form of `path` against them.
    ///
    /// Iteration order follows [`MATCHER_PRECEDENCE`], matching
    /// [`Self::infer_agent_type`]. If no explorer claims the path (or the
    /// matching explorer exposes its roots under both surfaces, e.g.
    /// path-shared Codex), the function falls back to
    /// [`Self::infer_agent_type`] and returns `"unknown"` as the surface —
    /// callers with session content in hand can then disambiguate via
    /// [`SessionLog::surface_label_with_content`].
    ///
    /// `home` is required so platform-conditional roots (Windows `%APPDATA%`,
    /// XDG, etc.) resolve correctly under both production and synthetic
    /// test homes.
    pub fn infer_agent_and_surface(&self, path: &Path, home: &Path) -> (AgentKind, &'static str) {
        for agent in MATCHER_PRECEDENCE {
            let sp = self.surface_paths_for(agent, home);
            if let Some(surface) = classify_path_against_surface_paths(path, &sp) {
                return (*agent, surface);
            }
        }
        // Fallback: the substring matcher handles agents whose CLI / IDE
        // variants share a filesystem location (Codex) or paths that don't
        // sit under any registered root.
        (self.infer_agent_type(path), "unknown")
    }

    /// Scan an agent's tree once and recover every visible row's title +
    /// surface in a single pass.
    pub async fn session_titles_and_surfaces_for(
        &self,
        agent: &AgentKind,
    ) -> Vec<SessionTitleAndSurface> {
        self.get(agent).session_titles_and_surfaces().await
    }

    /// Freshness fingerprint for a database-backed session.
    pub async fn provider_db_fingerprint(
        &self,
        agent: &AgentKind,
        db_path: &Path,
        session_id: &str,
    ) -> Option<(u64, u64)> {
        self.get(agent)
            .provider_db_fingerprint(db_path, session_id)
            .await
    }
}

#[derive(Debug, Clone)]
pub enum SessionSource {
    File(PathBuf),
    Inline {
        label: String,
        content: String,
    },
    ProviderDb {
        agent: AgentKind,
        db_path: PathBuf,
        session_id: String,
    },
}

#[derive(Debug, Clone)]
pub struct SessionLog {
    pub agent_type: AgentKind,
    pub source: SessionSource,
    pub updated_at: Option<i64>,
    /// Originating execution environment; part of this session's identity.
    pub environment: DiscoveryEnvironment,
}

/// Metadata a vendor writes in a sidecar next to a sub-agent transcript.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct SubagentMeta {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "agentType", default)]
    pub agent_type: Option<String>,
    #[serde(rename = "toolUseId", default)]
    pub tool_use_id: Option<String>,
}

impl SessionLog {
    /// Returns the execution environment used by downstream Git and lookups.
    pub fn environment(&self) -> DiscoveryEnvironment {
        self.environment.clone()
    }

    /// Environment-aware identity for discovery dedupe and incremental cursors.
    pub fn dedupe_key(&self) -> String {
        format!("{}|{}", self.environment().key(), self.source_label())
    }

    /// Persisted cursor key. Native sessions retain the pre-WSL label format
    /// for cursor compatibility; WSL sessions add the environment namespace.
    pub fn cursor_key(&self) -> String {
        if self.environment.is_native() {
            self.source_label()
        } else {
            self.dedupe_key()
        }
    }

    pub fn source_label(&self) -> String {
        match &self.source {
            SessionSource::File(path) => path.to_string_lossy().to_string(),
            SessionSource::Inline { label, .. } => label.clone(),
            SessionSource::ProviderDb {
                agent, session_id, ..
            } => format!("{agent}:{session_id}"),
        }
    }

    /// Classify this session as CLI vs IDE/Desktop using path/label only.
    /// Use [`Self::surface_label_with_content`] when raw session content is
    /// available — bi-modal agents that share a filesystem location
    /// (Claude, Codex) return `"unknown"` here and only disambiguate with
    /// content.
    ///
    /// `home` is the user home directory used to resolve per-agent surface
    /// roots; production callers pass `home_dir().unwrap_or_default()`,
    /// tests pass a synthetic `TempDir` so synthetic paths stay scoped to
    /// their fixture.
    ///
    /// Classification always uses [`Explorers::DISK`]. A configured adapter can
    /// only add [`SurfacePaths::mirror`] roots, and the classifier maps those to
    /// the same label those adapters return from `unmatched_surface`, so the
    /// answer does not depend on which adapter set the caller holds.
    pub fn surface_label(&self, home: &Path) -> &'static str {
        Explorers::DISK
            .get(&self.agent_type)
            .session_surface_label(self, None, home)
    }

    /// Like [`Self::surface_label`] but lets the classifier peek at the
    /// raw session content. Used by callers that have already loaded it.
    pub fn surface_label_with_content(&self, content: &str, home: &Path) -> &'static str {
        Explorers::DISK
            .get(&self.agent_type)
            .session_surface_label(self, Some(content), home)
    }
}

pub async fn session_source_content(source: &SessionSource) -> Option<String> {
    match source {
        SessionSource::File(path) => match tokio::fs::read_to_string(path).await {
            Ok(content) => Some(content),
            Err(error) => {
                ::tracing::debug!(
                    event = "session_source_unreadable",
                    path = %path.display(),
                    error = %error
                );
                None
            }
        },
        SessionSource::Inline { content, .. } => Some(content.clone()),
        SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path,
            session_id,
        } => {
            #[cfg(any(test, debug_assertions))]
            record_tracked_provider_db_render(db_path);
            agents::opencode::render_db_session(db_path.clone(), session_id.clone()).await
        }
        SessionSource::ProviderDb {
            agent,
            db_path,
            session_id,
        } => {
            tracing::error!(
                agent = %agent,
                db_path = %db_path.display(),
                session_id = %session_id,
                "unsupported provider db session content source"
            );
            None
        }
    }
}

/// Reads enough file content for metadata, visibility, and local observations
/// without materializing an oversized source. Non-file sources retain the
/// whole-content behavior of [`session_source_content`].
pub async fn session_source_preview(source: &SessionSource) -> Option<String> {
    let SessionSource::File(path) = source else {
        return session_source_content(source).await;
    };
    use tokio::io::AsyncReadExt;
    let file = open_file_for_head_read(path).await?;
    let mut bytes = Vec::new();
    file.take(SOURCE_PREVIEW_BYTES)
        .read_to_end(&mut bytes)
        .await
        .ok()?;
    preview_from_owned(bytes)
}

fn preview_from_owned(bytes: Vec<u8>) -> Option<String> {
    match String::from_utf8(bytes) {
        Ok(content) => Some(content),
        Err(error) if error.utf8_error().error_len().is_none() => {
            let valid = error.utf8_error().valid_up_to();
            String::from_utf8(error.into_bytes()[..valid].to_vec()).ok()
        }
        Err(_) => None,
    }
}

/// Read a bounded, line-aligned suffix of a source file.
///
/// The first bytes are discarded when the bound starts in the middle of a
/// JSONL record. Inline and provider-db sources retain their normal whole
/// content behavior because they are already bounded by their source adapter.
/// A single final JSONL record larger than [`ACTIVITY_TAIL_BYTES`] is therefore
/// intentionally omitted. The scan preserves a previously cached semantic
/// timestamp in that case; first discovery falls back to the source mtime.
pub async fn session_source_tail(source: &SessionSource) -> Option<String> {
    let SessionSource::File(path) = source else {
        return session_source_content(source).await;
    };
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut file = tokio::fs::File::open(path).await.ok()?;
    let len = file.metadata().await.ok()?.len();
    let start = len.saturating_sub(ACTIVITY_TAIL_BYTES);
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start)).await.ok()?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).await.ok()?;
    if start > 0
        && let Some(newline) = bytes.iter().position(|byte| *byte == b'\n')
    {
        bytes.drain(..=newline);
    }
    match String::from_utf8(bytes) {
        Ok(content) => Some(content),
        Err(error) if error.utf8_error().error_len().is_none() => {
            let valid = error.utf8_error().valid_up_to();
            String::from_utf8(error.into_bytes()[..valid].to_vec()).ok()
        }
        Err(_) => None,
    }
}

#[derive(Debug, Clone)]
pub struct SourceRead {
    pub metadata: scanner::SessionMetadata,
    pub stat: Option<SourceStat>,
    pub head_hash: Option<u64>,
    pub content: Option<String>,
}

pub async fn session_log_read(log: &SessionLog) -> Option<SourceRead> {
    let mut read = session_source_read(&log.source, Some(log.agent_type)).await?;
    if log.agent_type == AgentKind::Codex
        && !log.environment().is_native()
        && let SessionSource::File(path) = &log.source
        && let Some(session_id) = read.metadata.session_id.as_deref()
        && let Some(title) = agents::codex::index_title_for_rollout(path, session_id).await
    {
        read.metadata.title = Some(title);
        read.metadata.title_source = Some(TitleSource::AiGenerated);
    }
    if let DiscoveryEnvironment::Wsl { distribution, .. } = log.environment()
        && let Some(cwd) = read
            .metadata
            .cwd
            .as_deref()
            .filter(|cwd| cwd.starts_with('/'))
    {
        read.metadata.cwd = Some(
            crate::platform::environment::wsl_to_windows_path(&distribution, cwd)
                .ok()?
                .to_string_lossy()
                .to_string(),
        );
    }
    Some(read)
}

pub async fn session_log_metadata(log: &SessionLog) -> Option<scanner::SessionMetadata> {
    session_log_read(log).await.map(|read| read.metadata)
}

async fn session_source_metadata(
    source: &SessionSource,
    agent_type: Option<AgentKind>,
) -> Option<scanner::SessionMetadata> {
    session_source_read(source, agent_type)
        .await
        .map(|read| read.metadata)
}

async fn session_source_read(
    source: &SessionSource,
    agent_type: Option<AgentKind>,
) -> Option<SourceRead> {
    match source {
        SessionSource::File(path) => {
            let (stat, head_hash, content) = read_file_source(path)
                .await
                .map_or((None, None, None), |(stat, head_hash, content)| {
                    (stat, Some(head_hash), content)
                });
            let metadata = scanner::parse_session_metadata_with_agent(
                path,
                content.as_deref().unwrap_or_default(),
                agent_type,
            )
            .await;
            Some(SourceRead {
                metadata,
                stat,
                head_hash,
                content,
            })
        }
        SessionSource::Inline { content, .. } => {
            let mut metadata = scanner::parse_session_metadata_str(content);
            metadata.agent_type = agent_type;
            let content = if matches!(agent_type, Some(AgentKind::Claude | AgentKind::Codex)) {
                Some(content.clone())
            } else {
                None
            };
            Some(SourceRead {
                metadata,
                stat: None,
                head_hash: None,
                content,
            })
        }
        SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path,
            session_id,
        } => {
            let metadata =
                agents::opencode::db_session_metadata(db_path.clone(), session_id.clone()).await?;
            let content = if matches!(agent_type, Some(AgentKind::Claude | AgentKind::Codex)) {
                session_source_content(source).await
            } else {
                None
            };
            Some(SourceRead {
                metadata,
                stat: None,
                head_hash: None,
                content,
            })
        }
        SessionSource::ProviderDb {
            agent,
            db_path,
            session_id,
        } => {
            tracing::error!(
                agent = %agent,
                db_path = %db_path.display(),
                session_id = %session_id,
                "unsupported provider db session metadata source"
            );
            None
        }
    }
}

async fn read_file_source(path: &Path) -> Option<(Option<SourceStat>, u64, Option<String>)> {
    use tokio::io::AsyncReadExt;

    let file = open_file_for_head_read(path).await?;
    let stat = SourceStat::from_open_file(&file).await;
    let mut bytes = Vec::new();
    file.take(SOURCE_PREVIEW_BYTES)
        .read_to_end(&mut bytes)
        .await
        .ok()?;
    let head_hash = source_version::head_hash_of(&bytes);
    let content = preview_from_owned(bytes);
    Some((stat, head_hash, content))
}

async fn open_file_for_head_read(path: &Path) -> Option<tokio::fs::File> {
    #[cfg(any(test, debug_assertions))]
    record_tracked_head_read(path);
    tokio::fs::File::open(path).await.ok()
}

#[cfg(any(test, debug_assertions))]
static TRACKED_PROVIDER_DB_RENDERS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, usize>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[doc(hidden)]
#[cfg(any(test, debug_assertions))]
pub fn track_provider_db_renders(path: &Path) {
    TRACKED_PROVIDER_DB_RENDERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(path.to_path_buf(), 0);
}

#[cfg(any(test, debug_assertions))]
fn record_tracked_provider_db_render(path: &Path) {
    let mut renders = TRACKED_PROVIDER_DB_RENDERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(count) = renders.get_mut(path) {
        *count += 1;
    }
}

#[doc(hidden)]
#[cfg(any(test, debug_assertions))]
pub fn take_tracked_provider_db_renders(path: &Path) -> usize {
    TRACKED_PROVIDER_DB_RENDERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(path)
        .unwrap_or(0)
}

#[cfg(any(test, debug_assertions))]
static TRACKED_HEAD_READS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, usize>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[doc(hidden)]
#[cfg(any(test, debug_assertions))]
pub fn track_head_reads(path: &Path) {
    TRACKED_HEAD_READS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(path.to_path_buf(), 0);
}

#[cfg(any(test, debug_assertions))]
fn record_tracked_head_read(path: &Path) {
    let mut reads = TRACKED_HEAD_READS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(count) = reads.get_mut(path) {
        *count += 1;
    }
}

#[doc(hidden)]
#[cfg(any(test, debug_assertions))]
pub fn take_tracked_head_reads(path: &Path) -> usize {
    TRACKED_HEAD_READS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(path)
        .unwrap_or(0)
}

/// Extract a JSON string value (`"field":"value"`) from a single line, as a
/// borrowed `&str`. Returns `None` when the field is absent OR when the
/// value contains a JSON escape sequence (backslash).
///
/// Substring-scan to avoid full JSON parsing on the hot path. Rejecting
/// escapes is deliberate: the per-agent surface classifiers only consume short,
/// escape-free identifiers like `"claude-vscode"`, `"claude-desktop"`, or
/// `"cli"`. If a future agent release ever emits an escaped value we'd rather
/// fall back to path-based classification than silently misclassify on a
/// truncated value.
pub fn extract_json_string_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    let needle = format!("\"{field}\":\"");
    let start = line.find(&needle)? + needle.len();
    let rest = line.get(start..)?;
    let end = rest.find('"')?;
    let value = rest.get(..end)?;
    if value.contains('\\') {
        return None;
    }
    Some(value)
}

#[derive(Debug, Clone)]
pub struct DiscoveredFile {
    pub path: PathBuf,
    pub mtime_epoch: i64,
}

/// Find files in the given directories whose modification time is within
/// `since_secs` of `now`, and whose extension matches `exts`.
///
/// One `spawn_blocking` for the whole scan: per-op `tokio::fs` here used to
/// spawn a blocking micro-task per readdir/stat, and concurrent discovery
/// multiplied that into blocking-pool spawner contention.
pub async fn recent_files_with_exts(
    dirs: &[PathBuf],
    now: i64,
    since_secs: i64,
    exts: &[&str],
) -> Vec<DiscoveredFile> {
    let cutoff = now - since_secs;
    let dirs = dirs.to_vec();
    let exts: Vec<String> = exts.iter().map(|e| e.to_string()).collect();

    tokio::task::spawn_blocking(move || {
        let mut results = Vec::new();
        for dir in &dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();

                // Only consider files with matching extensions
                let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                    continue;
                };
                if !exts.iter().any(|allowed| allowed.eq_ignore_ascii_case(ext)) {
                    continue;
                }

                // Only consider regular files (follow symlinks)
                let Ok(metadata) = std::fs::metadata(&path) else {
                    continue;
                };
                if !metadata.is_file() {
                    continue;
                }

                let Ok(mtime) = metadata.modified() else {
                    continue;
                };
                let Ok(duration) = mtime.duration_since(UNIX_EPOCH) else {
                    continue;
                };
                let mtime_epoch = duration.as_secs() as i64;

                if mtime_epoch >= cutoff {
                    results.push(DiscoveredFile { path, mtime_epoch });
                }
            }
        }
        results
    })
    .await
    .unwrap_or_default()
}

/// Recursively collect directories that contain at least one file with
/// an extension listed in `exts` (case-insensitive). The whole walk runs as
/// one `spawn_blocking` task (see [`recent_files_with_exts`]).
pub async fn collect_dirs_with_exts(root: &Path, results: &mut Vec<PathBuf>, exts: &[&str]) {
    let root = root.to_path_buf();
    let exts: Vec<String> = exts.iter().map(|e| e.to_string()).collect();

    let found = tokio::task::spawn_blocking(move || {
        let mut found = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };

            let mut has_match = false;
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    stack.push(path);
                } else if file_type.is_file()
                    && !has_match
                    && let Some(ext) = path.extension().and_then(|e| e.to_str())
                    && exts.iter().any(|allowed| allowed.eq_ignore_ascii_case(ext))
                {
                    has_match = true;
                }
            }

            if has_match {
                found.push(dir);
            }
        }
        found
    })
    .await
    .unwrap_or_default();

    results.extend(found);
}

/// Whether `dir` directly contains at least one `.json` file.
///
/// Used to skip a mirror directory that exists but holds nothing, so an empty
/// mirror never joins the discovery walk.
pub async fn dir_has_json_files(dir: &Path) -> bool {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return false;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.path().extension().and_then(|value| value.to_str()) == Some("json") {
            return true;
        }
    }
    false
}

/// Resolve the user's home directory.
///
/// Returns `None` if the home directory cannot be determined.
/// Uses `HOME` on Unix/macOS and `USERPROFILE`/`HOMEDRIVE`+`HOMEPATH` on Windows.
pub fn home_dir() -> Option<PathBuf> {
    crate::paths::home_dir()
}

/// Read an environment variable as a `PathBuf`, but only when `home` matches
/// the real user home directory. Tests pass synthetic homes and must never
/// pick up the developer's real environment — centralizing the guard here
/// keeps that contract in one place.
pub fn env_path_when_real_home(home: &Path, var: &str) -> Option<PathBuf> {
    if home_dir().as_deref() == Some(home) {
        std::env::var_os(var).map(PathBuf::from)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopPlatform {
    Macos,
    Windows,
    Linux,
}

pub fn current_desktop_platform() -> DesktopPlatform {
    if cfg!(target_os = "macos") {
        DesktopPlatform::Macos
    } else if cfg!(target_os = "windows") {
        DesktopPlatform::Windows
    } else {
        DesktopPlatform::Linux
    }
}

pub fn app_config_dir_in(app: &str, home: &Path) -> PathBuf {
    let is_real_home = home_dir().as_deref() == Some(home);
    let appdata = if is_real_home {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        None
    };
    let xdg_config_home = if is_real_home {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
    } else {
        None
    };

    app_config_dir_for_platform(
        app,
        home,
        current_desktop_platform(),
        appdata.as_deref(),
        xdg_config_home.as_deref(),
    )
}

pub fn app_config_dir_for_platform(
    app: &str,
    home: &Path,
    platform: DesktopPlatform,
    appdata: Option<&Path>,
    xdg_config_home: Option<&Path>,
) -> PathBuf {
    if platform == DesktopPlatform::Macos {
        home.join("Library").join("Application Support").join(app)
    } else if platform == DesktopPlatform::Windows {
        if let Some(appdata) = appdata {
            appdata.join(app)
        } else {
            home.join("AppData").join("Roaming").join(app)
        }
    } else {
        let base = xdg_config_home
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join(".config"));
        base.join(app)
    }
}

/// Recursively find directories named `chatSessions` under a workspaceStorage
/// root. The whole walk runs as one `spawn_blocking` task (see
/// [`recent_files_with_exts`]).
pub async fn find_chat_session_dirs(root: &Path) -> Vec<PathBuf> {
    let root = root.to_path_buf();

    tokio::task::spawn_blocking(move || {
        let mut results = Vec::new();
        let mut stack = vec![root];

        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };

            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    if path.file_name().and_then(|n| n.to_str()) == Some("chatSessions") {
                        results.push(path);
                    } else {
                        stack.push(path);
                    }
                }
            }
        }

        results
    })
    .await
    .unwrap_or_default()
}

fn collect_discovered_sessions(per_agent_logs: Vec<Vec<SessionLog>>) -> Vec<SessionLog> {
    let mut results = Vec::new();
    for logs in per_agent_logs {
        results.extend(logs);
    }
    results
}

fn dedupe_environment_sessions(logs: &mut Vec<SessionLog>) {
    let mut seen = std::collections::HashSet::new();
    logs.retain(|log| seen.insert(log.dedupe_key()));
}

/// Per-agent timeout for the fast CWD path.
const AGENT_CWD_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);
/// Per-agent timeout for the per-session count path.
const AGENT_SESSION_COUNT_TIMEOUT: Duration = Duration::from_secs(30);

impl Explorers {
    /// Discover file-backed agent stores exposed through WSL's mounted namespace.
    /// SQLite-backed stores are deliberately left to their provider-specific reader;
    /// opening a live Linux SQLite database through 9P is not sufficiently reliable.
    async fn discover_wsl_file_sessions(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        let mut logs = Vec::new();
        for info in crate::platform::environment::discover_wsl_environments().await {
            logs.extend(
                self.discover_wsl_file_sessions_in(&info, now, since_secs)
                    .await,
            );
        }
        logs
    }

    /// Discovers one already-resolved distro. Keeping this separate prevents direct
    /// detail/title/sub-agent requests from scanning every mounted distribution.
    async fn discover_wsl_file_sessions_in(
        &self,
        info: &WslEnvironmentInfo,
        now: i64,
        since_secs: i64,
    ) -> Vec<SessionLog> {
        let mut logs = self
            .discover_wsl_non_opencode_sessions_in(info, now, since_secs)
            .await;
        logs.extend(agents::opencode::discover_recent_in_wsl(info, now, since_secs).await);
        logs
    }

    /// File-backed WSL discovery shared by full session and metadata-only paths.
    /// OpenCode is excluded because its fast CWD path must not export transcripts.
    pub async fn discover_wsl_non_opencode_sessions_in(
        &self,
        info: &WslEnvironmentInfo,
        now: i64,
        since_secs: i64,
    ) -> Vec<SessionLog> {
        let mut logs = agents::codex::discover_recent_in_wsl(info, now, since_secs).await;
        logs.extend(agents::claude::discover_recent_in_wsl(info, now, since_secs).await);
        let roots = [
            info.context.home.join(".cursor/chats"),
            info.context.home.join(".copilot/session-state"),
            info.context.home.join(".cline/tasks"),
            info.context.home.join(".kiro"),
            info.context.home.join(".local/share/amp/threads"),
            info.context.home.join(".pi/agent/sessions"),
        ];
        for root in roots {
            let mut dirs = Vec::new();
            collect_dirs_with_exts(&root, &mut dirs, &["json", "jsonl"]).await;
            for file in recent_files_with_exts(&dirs, now, since_secs, &["json", "jsonl"]).await {
                logs.push(SessionLog {
                    agent_type: self.infer_agent_type(&file.path),
                    source: SessionSource::File(file.path),
                    updated_at: Some(file.mtime_epoch),
                    environment: info.context.environment.clone(),
                });
            }
        }
        logs
    }

    /// Per-session WSL CWDs. OpenCode uses its single metadata query; other
    /// file-backed agents retain their normal metadata parsing.
    async fn discover_wsl_cwds_for_repo_discovery(&self, now: i64, since_secs: i64) -> Vec<String> {
        let mut set = tokio::task::JoinSet::new();
        for info in crate::platform::environment::discover_wsl_environments().await {
            let explorers = *self;
            set.spawn(async move {
                let (mut cwds, logs) = tokio::join!(
                    agents::opencode::discover_cwds_in_wsl(&info, now, since_secs),
                    explorers.discover_wsl_non_opencode_sessions_in(&info, now, since_secs),
                );
                for log in logs {
                    if let Some(cwd) = session_log_metadata(&log)
                        .await
                        .and_then(|metadata| metadata.cwd)
                    {
                        cwds.push(cwd);
                    }
                }
                cwds
            });
        }
        let mut cwds = Vec::new();
        while let Some(result) = set.join_next().await {
            if let Ok(found) = result {
                cwds.extend(found);
            }
        }
        cwds
    }

    /// Collect every agent's recent session logs, native and WSL, deduped.
    ///
    /// Quiet counterpart to [`Self::discover_recent_sessions_with_progress`] for
    /// callers that report their own progress (or none).
    pub async fn discover_recent_sessions(&self, now: i64, since_secs: i64) -> Vec<SessionLog> {
        // One spawn per agent, driven off the `AgentKind::ALL` registry so a new
        // agent is wired up in exactly one place rather than in each fan-out's
        // hand-written roster. Order is irrelevant: `collect_discovered_sessions`
        // merges the per-agent batches.
        let mut set = tokio::task::JoinSet::new();
        for t in AgentKind::ALL {
            let explorer = self.get(t);
            set.spawn(async move { explorer.discover_recent(now, since_secs).await });
        }
        let mut per_agent_logs: Vec<Vec<SessionLog>> = Vec::new();
        while let Some(result) = set.join_next().await {
            if let Ok(logs) = result {
                per_agent_logs.push(logs);
            }
        }

        let mut logs = collect_discovered_sessions(per_agent_logs);
        logs.extend(self.discover_wsl_file_sessions(now, since_secs).await);
        dedupe_environment_sessions(&mut logs);
        logs
    }

    /// Like [`Self::discover_recent_sessions`] but calls `on_agent_done`
    /// each time an agent explorer completes, enabling per-agent progress
    /// reporting.
    ///
    /// The callback receives `(agent_name, sessions_found, completed_count, total_agents)`.
    pub async fn discover_recent_sessions_with_progress(
        &self,
        now: i64,
        since_secs: i64,
        mut on_agent_done: impl FnMut(&str, usize, usize, usize),
    ) -> Vec<SessionLog> {
        let mut set = tokio::task::JoinSet::new();
        for t in AgentKind::ALL {
            let explorer = self.get(t);
            let label = t.display_label();
            set.spawn(async move { (label, explorer.discover_recent(now, since_secs).await) });
        }

        let total = set.len();
        let mut completed = 0;
        let mut per_agent_logs: Vec<Vec<SessionLog>> = Vec::new();

        while let Some(result) = set.join_next().await {
            completed += 1;
            if let Ok((name, logs)) = result {
                ::tracing::info!(
                    event = "repo_discovery_agent_done",
                    agent = name,
                    sessions = logs.len(),
                    completed,
                    total,
                );
                on_agent_done(name, logs.len(), completed, total);
                per_agent_logs.push(logs);
            }
        }

        let mut logs = collect_discovered_sessions(per_agent_logs);
        logs.extend(self.discover_wsl_file_sessions(now, since_secs).await);
        dedupe_environment_sessions(&mut logs);
        logs
    }

    /// Fast CWD-only discovery for repo detection. Calls `discover_cwds()` on each
    /// agent in parallel. The callback receives `(agent_name, cwds_found, completed, total)`.
    pub async fn discover_cwds_with_progress(
        &self,
        now: i64,
        since_secs: i64,
        mut on_agent_done: impl FnMut(&str, usize, usize, usize),
    ) -> Vec<String> {
        use tokio::task::JoinSet;
        use tokio::time::timeout;

        async fn with_cwd_timeout<F>(agent: &'static str, future: F) -> (&'static str, Vec<String>)
        where
            F: std::future::Future<Output = Vec<String>>,
        {
            match timeout(AGENT_CWD_DISCOVERY_TIMEOUT, future).await {
                Ok(cwds) => (agent, cwds),
                Err(_) => {
                    ::tracing::warn!(
                        event = "repo_discovery_agent_timeout",
                        agent,
                        timeout_secs = AGENT_CWD_DISCOVERY_TIMEOUT.as_secs(),
                        "agent CWD discovery timed out"
                    );
                    (agent, Vec::new())
                }
            }
        }

        let mut set = JoinSet::new();
        for t in AgentKind::ALL {
            let explorer = self.get(t);
            let label = t.display_label();
            set.spawn(async move {
                with_cwd_timeout(label, explorer.discover_cwds(now, since_secs)).await
            });
        }

        let total = set.len();
        let mut completed = 0;
        let mut all_cwds = Vec::new();

        while let Some(result) = set.join_next().await {
            completed += 1;
            if let Ok((name, cwds)) = result {
                ::tracing::info!(
                    event = "repo_discovery_agent_done",
                    agent = name,
                    cwds = cwds.len(),
                    completed,
                    total,
                );
                on_agent_done(name, cwds.len(), completed, total);
                all_cwds.extend(cwds);
            }
        }

        all_cwds.extend(
            self.discover_wsl_cwds_for_repo_discovery(now, since_secs)
                .await,
        );
        all_cwds.sort();
        all_cwds.dedup();

        all_cwds
    }

    /// Per-cwd count of recent AI sessions — one increment per session log.
    ///
    /// Unlike [`Self::discover_cwds_with_progress`] (which dedups, and whose
    /// per-agent `discover_cwds` overrides collapse to one cwd per project dir),
    /// this counts every recent session via each agent's required
    /// `discover_recent`, so callers can report how many AI sessions ran in each
    /// working directory. Map keys are the unique cwds (usable as the cwd set
    /// for repo resolution); values are session counts summed across all agents.
    ///
    /// This is the per-session path the `discover_cwds` overrides optimize away,
    /// so it is heavier than [`Self::discover_cwds_with_progress`].
    pub async fn discover_cwd_counts_with_progress(
        &self,
        now: i64,
        since_secs: i64,
        mut on_agent_done: impl FnMut(&str, usize, usize, usize),
    ) -> std::collections::HashMap<String, u32> {
        use tokio::task::JoinSet;

        let mut set = JoinSet::new();
        for t in AgentKind::ALL {
            let explorer = self.get(t);
            let label = t.display_label();
            set.spawn(async move {
                cwds_per_recent_session(
                    label,
                    explorer.discover_recent(now, since_secs),
                    AGENT_SESSION_COUNT_TIMEOUT,
                )
                .await
            });
        }

        let total = set.len();
        let mut completed = 0;
        let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        while let Some(result) = set.join_next().await {
            completed += 1;
            if let Ok((name, cwds)) = result {
                ::tracing::info!(
                    event = "repo_discovery_count_agent_done",
                    agent = name,
                    sessions = cwds.len(),
                    completed,
                    total,
                );
                on_agent_done(name, cwds.len(), completed, total);
                add_cwd_occurrences(&mut counts, cwds);
            }
        }
        add_cwd_occurrences(
            &mut counts,
            self.discover_wsl_cwds_for_repo_discovery(now, since_secs)
                .await,
        );
        counts
    }

    /// Locate a single session's transcript source by `(agent, session_id)`,
    /// ignoring recency. Scans only the requested agent's tree (one explorer), so
    /// the cost is bounded to that vendor rather than every agent. Matches first
    /// on the same id recovery the live path uses, then falls back to a substring
    /// match on the source label so agents whose filename embeds the id (Codex,
    /// Pi) still resolve. Returns `None` when no transcript matches (deleted,
    /// rotated, or never stored locally).
    pub async fn locate_session_source(
        &self,
        agent: &AgentKind,
        session_id: &str,
    ) -> Option<SessionSource> {
        if session_id.is_empty() {
            return None;
        }

        match self.get(agent).direct_session_source(session_id).await {
            DirectSessionSource::Found(source) => return Some(source),
            DirectSessionSource::Missing => return None,
            DirectSessionSource::Unsupported => {}
        }

        let now = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // `since_secs == now` → cutoff 0 → every session regardless of age.
        let logs = self.get(agent).discover_recent(now, now).await;

        // Prefer an exact id match; only fall back to a label substring match (for
        // agents that embed the id in the filename, e.g. Codex/Pi) when no exact
        // match exists, so a coincidental substring can't shadow the real session.
        logs.iter()
            .find(|log| self.session_id_of_log(log) == session_id)
            .or_else(|| {
                logs.iter()
                    .find(|log| log.source_label().contains(session_id))
            })
            .map(|log| log.source.clone())
    }

    /// Resolves one session within an explicit caller-supplied environment
    /// identity.
    ///
    /// `None` or an empty distro preserves native lookup behavior. WSL matching is
    /// case-insensitive and mounted-only; no stopped distribution is started.
    pub async fn locate_session_source_in_environment(
        &self,
        agent: &AgentKind,
        session_id: &str,
        wsl_distro: Option<&str>,
    ) -> Option<SessionSource> {
        let Some(distro) = wsl_distro.filter(|value| !value.is_empty()) else {
            return self.locate_session_source(agent, session_id).await;
        };
        let now = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default();
        let info = crate::platform::environment::discover_wsl_environments()
            .await
            .into_iter()
            .find(|info| info.distribution.eq_ignore_ascii_case(distro))?;
        let logs = self.discover_wsl_file_sessions_in(&info, now, now).await;
        logs.into_iter()
            .filter(|log| log.agent_type == *agent)
            .filter(|log| {
                log.environment
                    .wsl_distro()
                    .is_some_and(|candidate| candidate.eq_ignore_ascii_case(distro))
            })
            .find(|log| {
                self.session_id_of_log(log) == session_id || log.source_label().contains(session_id)
            })
            .map(|log| log.source)
    }

    /// Recover the session id a [`SessionLog`] carries when shaped into an
    /// upstream input. Kept alongside the locator so the live-metrics path and
    /// the single-session locator agree on how a source maps back to an id.
    fn session_id_of_log(&self, log: &SessionLog) -> String {
        match &log.source {
            SessionSource::File(path) => self
                .recover_session_id_from_path(&log.agent_type, path)
                .or_else(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .map(str::to_string)
                })
                .unwrap_or_default(),
            SessionSource::Inline { label, .. } => label.clone(),
            SessionSource::ProviderDb { session_id, .. } => session_id.clone(),
        }
    }

    /// Index the logs whose heartbeat falls within `window_secs` of `now`.
    pub fn live_session_index(
        &self,
        logs: &[SessionLog],
        now: i64,
        window_secs: i64,
    ) -> LiveSessionIndex {
        let entries = logs
            .iter()
            .filter(|log| {
                log.updated_at
                    .is_some_and(|updated| now.saturating_sub(updated) < window_secs)
            })
            .map(|log| {
                (
                    log.agent_type.to_string(),
                    self.session_id_of_log(log),
                    log.source_label(),
                    log.environment.key(),
                )
            })
            .collect();
        LiveSessionIndex { entries }
    }

    /// List a parent session's sub-agent transcript files. Sub-agents are an
    /// orchestration concept — a session that spawned its own workers, recorded
    /// per-vendor (Claude writes a `<sessionId>/subagents/` tree; Codex links them
    /// via the `thread_spawn_edges` table). Dispatched through [`AgentExplorer`], so
    /// a vendor whose explorer doesn't override the sub-agent hooks returns empty
    /// with no enum gate here. Empty also when the parent isn't an orchestrator.
    pub async fn list_subagents(&self, agent: &AgentKind, parent_session_id: &str) -> Vec<PathBuf> {
        // Vendors without sub-agent support would locate the parent transcript
        // only to apply the empty trait default — skip the lookup entirely.
        if !self.get(agent).supports_subagents() {
            return Vec::new();
        }
        let Some(SessionSource::File(parent)) =
            self.locate_session_source(agent, parent_session_id).await
        else {
            return Vec::new();
        };
        self.list_subagents_for_transcript(agent, &parent).await
    }

    /// Lists direct sub-agent rollouts in the parent's native or mounted WSL store.
    /// The parent lookup is constrained to `wsl_distro` before vendor-specific
    /// relationship validation runs, preventing same-id cross-distro attachment.
    pub async fn list_subagents_in_environment(
        &self,
        agent: &AgentKind,
        parent_session_id: &str,
        wsl_distro: Option<&str>,
    ) -> Vec<PathBuf> {
        if !self.get(agent).supports_subagents() {
            return Vec::new();
        }
        let Some(SessionSource::File(parent)) = self
            .locate_session_source_in_environment(agent, parent_session_id, wsl_distro)
            .await
        else {
            return Vec::new();
        };
        self.list_subagents_for_transcript(agent, &parent).await
    }

    /// Path-based variant of [`Self::list_subagents`] for callers that already
    /// hold the parent's on-disk transcript path, skipping the id → source
    /// resolution. Dispatched through the parent agent's explorer.
    pub async fn list_subagents_for_transcript(
        &self,
        agent: &AgentKind,
        parent_transcript: &Path,
    ) -> Vec<PathBuf> {
        self.get(agent).list_subagents(parent_transcript).await
    }

    /// Resolve a single sub-agent's transcript source for analysis or deep-linking,
    /// keyed by the parent session id and the sub-agent's `agent-<hash>` id.
    /// Mirrors [`Self::locate_session_source`] but descends into the parent's
    /// `subagents/` tree via the parent agent's explorer. `None` for any agent
    /// whose explorer doesn't support sub-agents.
    pub async fn locate_subagent_source(
        &self,
        agent: &AgentKind,
        parent_session_id: &str,
        subagent_id: &str,
    ) -> Option<SessionSource> {
        if !self.get(agent).supports_subagents() {
            return None;
        }
        let SessionSource::File(parent) =
            self.locate_session_source(agent, parent_session_id).await?
        else {
            return None;
        };
        self.get(agent)
            .locate_subagent(&parent, subagent_id)
            .await
            .map(SessionSource::File)
    }

    /// Resolves one child transcript under an environment-constrained parent.
    /// Returns `None` when the distro is unmounted, the vendor lacks orchestration,
    /// or the child is not actually linked to the requested parent.
    pub async fn locate_subagent_source_in_environment(
        &self,
        agent: &AgentKind,
        parent_session_id: &str,
        subagent_id: &str,
        wsl_distro: Option<&str>,
    ) -> Option<SessionSource> {
        if !self.get(agent).supports_subagents() {
            return None;
        }
        let SessionSource::File(parent) = self
            .locate_session_source_in_environment(agent, parent_session_id, wsl_distro)
            .await?
        else {
            return None;
        };
        self.get(agent)
            .locate_subagent(&parent, subagent_id)
            .await
            .map(SessionSource::File)
    }

    /// The deep-link id for a sub-agent transcript path, per the parent agent's
    /// explorer. `None` for agents without sub-agent support.
    pub fn subagent_id(&self, agent: &AgentKind, path: &Path) -> Option<String> {
        self.get(agent).subagent_id(path)
    }

    /// A sub-agent's display label for the roster, per the parent agent's explorer.
    pub async fn subagent_label(&self, agent: &AgentKind, path: &Path) -> String {
        self.get(agent).subagent_label(path).await
    }

    /// Sidecar metadata for a sub-agent transcript, per the parent agent's
    /// explorer.
    pub async fn subagent_meta(&self, agent: &AgentKind, path: &Path) -> Option<SubagentMeta> {
        self.get(agent).subagent_meta(path).await
    }
}

async fn cwds_per_recent_session<F>(
    agent: &'static str,
    recent: F,
    timeout_after: Duration,
) -> (&'static str, Vec<String>)
where
    F: std::future::Future<Output = Vec<SessionLog>>,
{
    let logs = match tokio::time::timeout(timeout_after, recent).await {
        Ok(logs) => logs,
        Err(_) => {
            ::tracing::warn!(
                event = "repo_discovery_count_agent_timeout",
                agent,
                timeout_secs = timeout_after.as_secs(),
                "agent session-count discovery timed out"
            );
            return (agent, Vec::new());
        }
    };
    let cwds = bounded_log_tasks(logs, |log| async move {
        session_log_metadata(&log)
            .await
            .and_then(|metadata| metadata.cwd)
    })
    .await;
    (agent, cwds)
}

fn add_cwd_occurrences(
    counts: &mut std::collections::HashMap<String, u32>,
    cwds: impl IntoIterator<Item = String>,
) {
    for cwd in cwds {
        *counts.entry(cwd).or_insert(0) += 1;
    }
}

/// The sessions that are live right now — transcript heartbeat within the
/// active window — indexed for matching against a consumer's
/// `(agent slug, agent session id)` keys.
pub struct LiveSessionIndex {
    /// One row per live log: `(agent slug, recovered session id, source label,
    /// environment key)`.
    entries: Vec<(String, String, String, String)>,
}

impl LiveSessionIndex {
    /// Whether the key `(agent_slug, session_id, environment_key)` belongs to a
    /// live session. Matching mirrors [`Explorers::locate_session_source`]: an
    /// exact recovered-id match first, then a source-label substring match so
    /// agents whose filename embeds the id (Codex, Pi) still resolve.
    pub fn contains(&self, agent_slug: &str, session_id: &str, environment_key: &str) -> bool {
        if session_id.is_empty() {
            return false;
        }
        self.entries
            .iter()
            .any(|(slug, recovered, label, environment)| {
                slug == agent_slug
                    && environment == environment_key
                    && (recovered == session_id || label.contains(session_id))
            })
    }
}

/// Set a file's modification time to a specific Unix epoch timestamp.
///
/// Test helper exposed at the module level for use by submodule tests. Uses the
/// `filetime` crate for cross-platform correctness (avoids timezone issues with
/// the `touch` command).
#[cfg(test)]
pub(crate) fn set_file_mtime(path: &Path, epoch_secs: i64) {
    let ft = filetime::FileTime::from_unix_time(epoch_secs, 0);
    filetime::set_file_mtime(path, ft).expect("failed to set file mtime");
}

#[cfg(test)]
mod tests;
