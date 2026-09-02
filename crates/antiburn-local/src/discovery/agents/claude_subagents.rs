//! Claude sub-agent (orchestration) discovery.
//!
//! A Claude session that spawns sub-agents (Workflows / `Task` sub-agents) writes
//! each one as its own transcript under `<dir>/<sessionId>/subagents/agent-*.jsonl`,
//! a sibling tree to the parent's `<sessionId>.jsonl`. Discovery only reads
//! top-level `.jsonl`, so these never surface as ordinary sessions; the
//! orchestration layer reads them on demand.
//!
//! These helpers back the `AgentExplorer` sub-agent hooks on `ClaudeExplorer`
//! (see `claude.rs`), kept isolated here so the Claude explorer stays focused on
//! session/title discovery. Only Claude writes this tree today; every other
//! vendor uses the no-op trait defaults in `mod.rs`.

use std::path::{Path, PathBuf};

use crate::discovery::SubagentMeta;
use crate::discovery::scanner;

/// Lines to scan when deriving a sub-agent's label. The task prompt is the very
/// first record; the persona (`attributionAgent`) lands a couple of records
/// later, so a small window covers both fallbacks.
const SUBAGENT_LABEL_SCAN_LINES: usize = 16;
/// Max characters of the task prompt kept for a sub-agent label.
const SUBAGENT_LABEL_MAX_CHARS: usize = 80;

/// Map a parent transcript path to its `subagents` directory:
/// `<dir>/<stem>/subagents`. `None` if the path has no parent or stem.
///
/// `pub(super)` so the recency sweep in `claude.rs` can stat this directory
/// directly, without listing or reading the sub-agent files inside it.
pub(super) fn subagents_dir(parent_transcript: &Path) -> Option<PathBuf> {
    let dir = parent_transcript.parent()?;
    let stem = parent_transcript.file_stem()?.to_str()?;
    Some(dir.join(stem).join("subagents"))
}

/// List a parent transcript's sub-agent files (`agent-*.jsonl`), sorted for a
/// stable roster order. Empty when there's no `subagents/` dir — i.e. the
/// session is not an orchestrator. One `spawn_blocking` for the readdir.
pub(super) async fn list_subagents(parent_transcript: &Path) -> Vec<PathBuf> {
    let parent = parent_transcript.to_path_buf();
    tokio::task::spawn_blocking(move || list_subagents_blocking(&parent))
        .await
        .unwrap_or_default()
}

/// Blocking-context core of [`list_subagents`], for callers already inside a
/// `spawn_blocking` sweep (the subagent-recency walk in `claude.rs`).
pub(super) fn list_subagents_blocking(parent_transcript: &Path) -> Vec<PathBuf> {
    let Some(dir) = subagents_dir(parent_transcript) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().and_then(|e| e.to_str()) == Some("jsonl")
                && path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with("agent-"))
        })
        .collect();
    out.sort();
    out
}

/// Resolve a single sub-agent transcript by `(parent_transcript, subagent_id)`,
/// validating `subagent_id` against the same alphanumeric-plus-hyphen allowlist
/// used for session ids so a crafted id can't escape the `subagents/` dir.
pub(super) async fn locate_subagent(
    parent_transcript: &Path,
    subagent_id: &str,
) -> Option<PathBuf> {
    if subagent_id.is_empty()
        || !subagent_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return None;
    }
    let dir = subagents_dir(parent_transcript)?;
    let candidate = dir.join(format!("{subagent_id}.jsonl"));
    tokio::fs::try_exists(&candidate)
        .await
        .unwrap_or(false)
        .then_some(candidate)
}

/// The `agent-<hash>` file stem that identifies a sub-agent for deep-linking.
pub(super) fn subagent_id(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
}

/// The `agent-<hash>.meta.json` sidecar Claude writes next to each sub-agent
/// transcript, recording the `Task` spawn's short `description`, `agentType`
/// persona, and newer `toolUseId` linkage. Fields are absent on older
/// transcripts, hence optional.
/// Read a sub-agent's `.meta.json` sidecar (`agent-X.jsonl` → `agent-X.meta.json`),
/// returning `None` when it's missing or unparseable.
pub(super) async fn read_subagent_meta(path: &Path) -> Option<SubagentMeta> {
    let meta_path = path.with_extension("meta.json");
    let content = tokio::fs::read_to_string(&meta_path).await.ok()?;
    serde_json::from_str(&content).ok()
}

/// A sub-agent's display label. Prefers the `Task` spawn's own short label from
/// the `.meta.json` sidecar — its `description`, then its `agentType` persona.
/// Sibling sub-agents in a fan-out routinely share the same boilerplate prompt
/// prefix, so the first-prompt fallback below collapses to identical, truncated
/// labels; the sidecar `description` is distinct per spawn. When there's no
/// sidecar, falls back to the session **title** the standard title pipeline
/// extracts via [`scanner::parse_session_metadata`] — an explicit/AI/summary
/// title when Claude wrote one, else the first task prompt — then the persona
/// (`attributionAgent`), then a generic label.
pub(super) async fn subagent_label(path: &Path) -> String {
    if let Some(meta) = read_subagent_meta(path).await {
        for text in [meta.description, meta.agent_type].into_iter().flatten() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return truncate_chars(trimmed, SUBAGENT_LABEL_MAX_CHARS);
            }
        }
    }
    if let Some(title) = scanner::parse_session_metadata(path).await.title {
        let trimmed = title.trim();
        if !trimmed.is_empty() {
            return truncate_chars(trimmed, SUBAGENT_LABEL_MAX_CHARS);
        }
    }
    // No extractable title → the persona, then a generic label.
    let content = tokio::fs::read_to_string(path).await.unwrap_or_default();
    for line in content.lines().take(SUBAGENT_LABEL_SCAN_LINES) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(a) = v.get("attributionAgent").and_then(|x| x.as_str())
            && !a.trim().is_empty()
        {
            return a.to_string();
        }
    }
    "Sub-agent".to_string()
}

/// Truncate to at most `max` characters (not bytes — safe for UTF-8), appending
/// an ellipsis when clipped.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn subagents_dir_maps_to_sibling_tree() {
        let parent = PathBuf::from("/p/-Users-foo-bar/abc-123.jsonl");
        assert_eq!(
            subagents_dir(&parent).unwrap(),
            PathBuf::from("/p/-Users-foo-bar/abc-123/subagents"),
        );
    }

    #[tokio::test]
    async fn list_subagents_finds_only_agent_jsonl() {
        let home = TempDir::new().unwrap();
        let project = home.path().join("-Users-foo-bar");
        let subs = project.join("sess-1").join("subagents");
        tokio::fs::create_dir_all(&subs).await.unwrap();
        tokio::fs::write(subs.join("agent-aaa.jsonl"), "{}")
            .await
            .unwrap();
        tokio::fs::write(subs.join("agent-bbb.jsonl"), "{}")
            .await
            .unwrap();
        // Noise that must be ignored: wrong extension and wrong prefix.
        tokio::fs::write(subs.join("agent-ccc.txt"), "x")
            .await
            .unwrap();
        tokio::fs::write(subs.join("notes.jsonl"), "{}")
            .await
            .unwrap();

        let parent = project.join("sess-1.jsonl");
        let found = list_subagents(&parent).await;
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with("agent-"))
        }));
    }

    #[tokio::test]
    async fn list_subagents_empty_without_subagents_dir() {
        let home = TempDir::new().unwrap();
        let project = home.path().join("-Users-foo-bar");
        tokio::fs::create_dir_all(&project).await.unwrap();
        let parent = project.join("sess-1.jsonl");
        assert!(list_subagents(&parent).await.is_empty());
    }

    #[tokio::test]
    async fn locate_subagent_resolves_and_rejects_traversal() {
        let home = TempDir::new().unwrap();
        let subs = home
            .path()
            .join("-Users-foo-bar")
            .join("sess-1")
            .join("subagents");
        tokio::fs::create_dir_all(&subs).await.unwrap();
        tokio::fs::write(subs.join("agent-aaa.jsonl"), "{}")
            .await
            .unwrap();
        let parent = home.path().join("-Users-foo-bar").join("sess-1.jsonl");

        assert_eq!(
            locate_subagent(&parent, "agent-aaa").await.unwrap(),
            subs.join("agent-aaa.jsonl"),
        );
        // Unknown id → None; traversal attempt rejected by the allowlist.
        assert!(locate_subagent(&parent, "agent-zzz").await.is_none());
        assert!(locate_subagent(&parent, "../../etc/passwd").await.is_none());
    }

    #[tokio::test]
    async fn subagent_label_uses_session_title_pipeline() {
        let home = TempDir::new().unwrap();
        let subs = home.path().join("p").join("s").join("subagents");
        tokio::fs::create_dir_all(&subs).await.unwrap();
        let file = subs.join("agent-aaa.jsonl");
        // The title pipeline prefers an AI/summary title over the (verbose) first
        // prompt — the same resolution top-level sessions get.
        let body = concat!(
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"I'm investigating how the build pipeline works end to end"}}"#,
            "\n",
            r#"{"type":"summary","summary":"Build pipeline investigation"}"#,
        );
        tokio::fs::write(&file, body).await.unwrap();
        assert_eq!(subagent_label(&file).await, "Build pipeline investigation");
    }

    #[tokio::test]
    async fn subagent_label_falls_back_to_prompt_then_persona() {
        let home = TempDir::new().unwrap();
        let subs = home.path().join("p").join("s").join("subagents");
        tokio::fs::create_dir_all(&subs).await.unwrap();

        // No slug, but a first task prompt → the prompt (truncated).
        let prompt_file = subs.join("agent-prompt.jsonl");
        tokio::fs::write(
            &prompt_file,
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"Investigate the build pipeline"}}"#,
        )
        .await
        .unwrap();
        assert_eq!(
            subagent_label(&prompt_file).await,
            "Investigate the build pipeline"
        );

        // No slug and no user text → persona (attributionAgent).
        let persona_file = subs.join("agent-persona.jsonl");
        tokio::fs::write(
            &persona_file,
            r#"{"type":"assistant","attributionAgent":"Explore","message":{"role":"assistant","content":[{"type":"text","text":"x"}]}}"#,
        )
        .await
        .unwrap();
        assert_eq!(subagent_label(&persona_file).await, "Explore");
    }

    #[tokio::test]
    async fn subagent_label_prefers_meta_description_over_shared_prompt() {
        let home = TempDir::new().unwrap();
        let subs = home.path().join("p").join("s").join("subagents");
        tokio::fs::create_dir_all(&subs).await.unwrap();
        let file = subs.join("agent-aaa.jsonl");
        // A long, shared boilerplate prompt — the kind that collapses to an
        // identical label across siblings once truncated to 80 chars.
        tokio::fs::write(
            &file,
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"I'm working in a desktop app at /home/avery/projects/demo-app. I need to understand the analytics pipeline end to end."}}"#,
        )
        .await
        .unwrap();
        // The sidecar carries the distinct per-spawn description.
        tokio::fs::write(
            subs.join("agent-aaa.meta.json"),
            r#"{"agentType":"Explore","description":"Trace session analytics data flow","toolUseId":"toolu_1"}"#,
        )
        .await
        .unwrap();
        assert_eq!(
            subagent_label(&file).await,
            "Trace session analytics data flow"
        );
    }

    #[tokio::test]
    async fn subagent_label_falls_back_to_meta_agent_type_then_pipeline() {
        let home = TempDir::new().unwrap();
        let subs = home.path().join("p").join("s").join("subagents");
        tokio::fs::create_dir_all(&subs).await.unwrap();

        // Sidecar with a blank description → use its agentType.
        let typed = subs.join("agent-typed.jsonl");
        tokio::fs::write(
            &typed,
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"Investigate the build pipeline"}}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            subs.join("agent-typed.meta.json"),
            r#"{"agentType":"Explore","description":"   "}"#,
        )
        .await
        .unwrap();
        assert_eq!(subagent_label(&typed).await, "Explore");

        // No sidecar at all → the existing title pipeline still wins.
        let bare = subs.join("agent-bare.jsonl");
        tokio::fs::write(
            &bare,
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"Investigate the build pipeline"}}"#,
        )
        .await
        .unwrap();
        assert_eq!(
            subagent_label(&bare).await,
            "Investigate the build pipeline"
        );
    }
}
