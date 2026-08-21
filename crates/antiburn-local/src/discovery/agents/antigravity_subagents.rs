// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Antigravity sub-agent (orchestration) discovery.
//!
//! Antigravity's "Agent Manager" fans a human-launched cascade out into worker
//! agents, each of which is its **own cascade** with its own
//! `<cascadeId>.json` — structurally like Codex (separate rollouts), not Claude
//! (a `subagents/` subtree). The parent→child link is **not** on disk in the
//! encrypted brain store; the vendor only exposes it in each cascade's
//! trajectory metadata:
//!
//! ```text
//! trajectorySummaries[<cascadeId>].trajectoryMetadata = {
//!     parentConversationId: "<orchestrator cascadeId>",   // absent on the orchestrator
//!     subagentSpec: { typeName: "research", role: "Frontend Agent", … },
//!     nestingDepth: 1, …
//! }
//! ```
//!
//! Because that state is not addressable from a single transcript, an embedding
//! application that can read it records the edges in a cleartext sidecar of its
//! own and points [`AntigravityExplorer::spawn_edges_path`](super::antigravity::AntigravityExplorer::spawn_edges_path)
//! at it. Keep the sidecar **outside** the mirror directory, so mirror
//! housekeeping cannot delete it.
//!
//! This module owns that sidecar's shape ([`ChildEdge`], [`extract_spawn_edges`],
//! [`save_spawn_edges`]) and reads it back for the
//! [`AgentExplorer`](crate::discovery::AgentExplorer) sub-agent hooks, mirroring
//! [`codex_subagents`](super::codex_subagents) (whose source is a SQLite table
//! rather than a JSON sidecar). Two consequences this module handles, both like
//! Codex:
//!
//! 1. **Roster:** a cascade orchestrates when other cascades name it as their
//!    `parentConversationId`; the roster is those children, resolved to their
//!    `<childCascadeId>.json`.
//! 2. **No leaking:** each child is an ordinary cascade in the same tree
//!    discovery walks, so [`child_cascade_ids_in`] feeds
//!    `AntigravityExplorer::discover_recent`'s filter to keep workers out of the
//!    top-level session list.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Max characters kept for a sub-agent label (matches `codex_subagents` /
/// `claude_subagents`).
const SUBAGENT_LABEL_MAX_CHARS: usize = 80;

/// One spawned worker's edge, keyed in the sidecar by the worker's own
/// cascadeId: which orchestrator spawned it and what to call it in the roster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildEdge {
    /// The orchestrator cascadeId this worker was spawned by
    /// (`trajectoryMetadata.parentConversationId`).
    pub parent: String,
    /// Roster label, resolved from `subagentSpec.role` (e.g. "Frontend Agent"),
    /// falling back to `typeName`, then a generic label.
    pub label: String,
}

/// The on-disk sidecar: `{ "<childCascadeId>": { parent, label } }`.
pub type SpawnEdges = BTreeMap<String, ChildEdge>;

/// Build the spawn-edge map from a `GetAllCascadeTrajectories` response. A
/// cascade is a worker when its `trajectoryMetadata.parentConversationId` names
/// a *different* cascade; self-references (the orchestrator's own steps point at
/// itself) are skipped. `trajectoryMetadata` is normally a nested object but is
/// tolerated as an embedded JSON string. Returns empty when no cascade has a
/// parent (the common, non-orchestrating case) or the shape is unrecognized.
pub fn extract_spawn_edges(value: &Value) -> SpawnEdges {
    let mut out = SpawnEdges::new();
    let Some(summaries) = value.get("trajectorySummaries").and_then(Value::as_object) else {
        return out;
    };
    for (cascade_id, summary) in summaries {
        let Some(meta) = trajectory_metadata(summary) else {
            continue;
        };
        let parent = meta
            .get("parentConversationId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|p| !p.is_empty());
        let Some(parent) = parent else { continue };
        // The orchestrator's own steps carry its own id here; only a *different*
        // parent is a genuine spawn edge.
        if parent == cascade_id {
            continue;
        }
        out.insert(
            cascade_id.clone(),
            ChildEdge {
                parent: parent.to_string(),
                label: label_from_spec(meta.get("subagentSpec")),
            },
        );
    }
    out
}

/// `trajectoryMetadata` as an object, parsing the embedded-string form if the
/// LSP serialized it that way.
fn trajectory_metadata(summary: &Value) -> Option<Value> {
    match summary.get("trajectoryMetadata") {
        Some(obj @ Value::Object(_)) => Some(obj.clone()),
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).ok(),
        _ => None,
    }
}

/// Roster label from a `subagentSpec`: `role` → `typeName` → generic.
fn label_from_spec(spec: Option<&Value>) -> String {
    let pick = |key: &str| {
        spec.and_then(|s| s.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    match pick("role").or_else(|| pick("typeName")) {
        Some(text) => truncate_chars(text, SUBAGENT_LABEL_MAX_CHARS),
        None => "Sub-agent".to_string(),
    }
}

/// Persist the spawn-edge sidecar at an explicit path. Written via temp-file +
/// rename so a concurrent reader never sees a partial file.
pub async fn save_spawn_edges(path: &Path, edges: &SpawnEdges) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("missing parent for {}", path.display()))?;
    tokio::fs::create_dir_all(parent).await?;
    let data = serde_json::to_vec_pretty(edges)?;
    let pid = std::process::id();
    for attempt in 0..8u32 {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = parent.join(format!(
            ".antigravity-spawn-edges.{pid}.{nonce}.{attempt}.tmp"
        ));
        let mut opts = tokio::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        match opts.open(&tmp).await {
            Ok(mut file) => {
                tokio::io::AsyncWriteExt::write_all(&mut file, &data).await?;
                // A tokio File buffers the write and finishes it on the
                // blocking pool. A drop does not wait for that work, so the
                // rename below can move an empty file into place. Flush the
                // buffer, then sync, so the data is on disk before the file
                // becomes visible under its real name.
                tokio::io::AsyncWriteExt::flush(&mut file).await?;
                file.sync_all().await?;
                drop(file);
                tokio::fs::rename(&tmp, &path).await?;
                return Ok(());
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(anyhow::anyhow!(
                    "create temp for {}: {}",
                    path.display(),
                    err
                ));
            }
        }
    }
    Err(anyhow::anyhow!(
        "failed to create unique temp file for {}",
        path.display()
    ))
}

async fn load_spawn_edges(path: &Path) -> SpawnEdges {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => SpawnEdges::new(),
    }
}

/// The Antigravity cascadeId identifying a worker for deep-linking — its file
/// stem (cascadeIds are UUIDs, so the stem is the id verbatim).
pub fn subagent_id(path: &Path) -> Option<String> {
    cascade_id_of(path)
}

/// Every cascadeId recorded as a worker in the sidecar. A cascade whose id is in
/// this set is a spawned worker and must be excluded from top-level discovery.
/// Empty when there are no spawn edges.
pub async fn child_cascade_ids_in(sidecar_path: &Path) -> HashSet<String> {
    load_spawn_edges(sidecar_path).await.into_keys().collect()
}

/// List a parent cascade's spawned workers as cascade paths, sorted for a
/// stable roster. Empty when the parent spawned nothing or no worker's cascade
/// file exists yet.
pub async fn list_subagents_in(
    parent_transcript: &Path,
    sidecar_path: &Path,
    cache_dir: &Path,
) -> Vec<PathBuf> {
    let Some(parent_id) = cascade_id_of(parent_transcript) else {
        return Vec::new();
    };
    let edges = load_spawn_edges(sidecar_path).await;
    // BTreeMap iteration is sorted by child id → stable roster order.
    let mut out = Vec::new();
    for (child_id, edge) in &edges {
        if edge.parent != parent_id {
            continue;
        }
        let path = cascade_cache_path(cache_dir, child_id);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            out.push(path);
        }
    }
    out
}

pub async fn locate_subagent_in(
    parent_transcript: &Path,
    subagent_id: &str,
    sidecar_path: &Path,
    cache_dir: &Path,
) -> Option<PathBuf> {
    if !is_valid_cascade_id(subagent_id) {
        return None;
    }
    let parent_id = cascade_id_of(parent_transcript)?;
    let edges = load_spawn_edges(sidecar_path).await;
    if edges.get(subagent_id).map(|e| e.parent.as_str()) != Some(parent_id.as_str()) {
        return None;
    }
    let path = cascade_cache_path(cache_dir, subagent_id);
    tokio::fs::try_exists(&path)
        .await
        .unwrap_or(false)
        .then_some(path)
}

pub async fn subagent_label_in(path: &Path, sidecar_path: &Path) -> String {
    let Some(id) = cascade_id_of(path) else {
        return "Sub-agent".to_string();
    };
    load_spawn_edges(sidecar_path)
        .await
        .get(&id)
        .map(|e| e.label.clone())
        .unwrap_or_else(|| "Sub-agent".to_string())
}

/// The cascadeId a cached-cascade path encodes: its file stem
/// (`<cascadeId>.json`). Antigravity cascadeIds are filename-safe UUIDs, so the
/// stem is the id verbatim.
fn cascade_id_of(path: &Path) -> Option<String> {
    path.file_stem()?.to_str().map(str::to_string)
}

/// A worker's cached cascade path. cascadeIds are UUIDs (filename-safe), so the
/// file is `<cascadeId>.json` directly.
fn cascade_cache_path(cache_dir: &Path, cascade_id: &str) -> PathBuf {
    cache_dir.join(format!("{cascade_id}.json"))
}

/// Positive allowlist for a subagent id reaching filesystem resolution: UUID
/// characters only, rejecting path-traversal ids before any fs work (mirrors
/// `codex_subagents::is_valid_thread_id`).
fn is_valid_cascade_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Truncate to at most `max` characters (not bytes — safe for UTF-8), appending
/// an ellipsis when clipped (matches `codex_subagents`).
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
    use serde_json::json;
    use tempfile::TempDir;

    // The real cascadeIds captured from a live Agent-Manager fan-out
    // ("Multi-Agent Repository Investigation Setup" → 5 research workers).
    const ORCH: &str = "2aabe6e8-dc5f-4257-8f71-08fbb11ae6f3";
    const FRONTEND: &str = "0ecd1325-823d-4803-a9b1-5462db214ce4";
    const BACKEND: &str = "c756d02c-bbcf-40b1-a37c-5067f1dc7f24";

    /// A `GetAllCascadeTrajectories`-shaped response: the orchestrator (no
    /// parent) plus two workers pointing at it, matching the live payload.
    fn trajectories_response() -> Value {
        json!({
            "trajectorySummaries": {
                ORCH: {
                    "summary": "Multi-Agent Repository Investigation Setup",
                    "trajectoryMetadata": { "initializationStateId": "1dc791f7" }
                },
                FRONTEND: {
                    "summary": "Planning Daily AI Wrap-up Feature",
                    "trajectoryMetadata": {
                        "parentConversationId": ORCH,
                        "nestingDepth": 1,
                        "subagentSpec": { "typeName": "research", "role": "Frontend Agent" }
                    }
                },
                BACKEND: {
                    "summary": "Planning Daily AI Wrap-up Feature",
                    "trajectoryMetadata": {
                        "parentConversationId": ORCH,
                        "subagentSpec": { "typeName": "research", "role": "Backend Agent" }
                    }
                }
            }
        })
    }

    #[test]
    fn extract_spawn_edges_maps_workers_to_orchestrator_with_role_labels() {
        let edges = extract_spawn_edges(&trajectories_response());
        assert_eq!(edges.len(), 2, "two workers, orchestrator excluded");
        assert_eq!(edges[FRONTEND].parent, ORCH);
        assert_eq!(edges[FRONTEND].label, "Frontend Agent");
        assert_eq!(edges[BACKEND].label, "Backend Agent");
        assert!(!edges.contains_key(ORCH), "orchestrator has no parent edge");
    }

    #[test]
    fn extract_spawn_edges_parses_embedded_string_metadata_and_falls_back_to_type_name() {
        // trajectoryMetadata serialized as a JSON string; subagentSpec has no role.
        let value = json!({
            "trajectorySummaries": {
                FRONTEND: {
                    "trajectoryMetadata":
                        format!("{{\"parentConversationId\":\"{ORCH}\",\"subagentSpec\":{{\"typeName\":\"research\"}}}}")
                }
            }
        });
        let edges = extract_spawn_edges(&value);
        assert_eq!(edges[FRONTEND].parent, ORCH);
        assert_eq!(edges[FRONTEND].label, "research");
    }

    #[test]
    fn extract_spawn_edges_skips_self_reference_and_empty_parent() {
        let value = json!({
            "trajectorySummaries": {
                ORCH: { "trajectoryMetadata": { "parentConversationId": ORCH } },
                FRONTEND: { "trajectoryMetadata": { "parentConversationId": "" } }
            }
        });
        assert!(extract_spawn_edges(&value).is_empty());
    }

    #[test]
    fn extract_spawn_edges_empty_for_unrecognized_shape() {
        assert!(extract_spawn_edges(&json!({})).is_empty());
        assert!(extract_spawn_edges(&json!({ "trajectorySummaries": [] })).is_empty());
    }

    /// Seed a temp sidecar + the workers' cached `<id>.json` files.
    async fn seed(dir: &Path, edges: &[(&str, &str, &str)], cached: &[&str]) -> (PathBuf, PathBuf) {
        let cache = dir.join("antigravity-api");
        tokio::fs::create_dir_all(&cache).await.unwrap();
        for id in cached {
            tokio::fs::write(cascade_cache_path(&cache, id), "{}")
                .await
                .unwrap();
        }
        let map: SpawnEdges = edges
            .iter()
            .map(|(child, parent, label)| {
                (
                    child.to_string(),
                    ChildEdge {
                        parent: parent.to_string(),
                        label: label.to_string(),
                    },
                )
            })
            .collect();
        let sidecar = dir.join("antigravity-spawn-edges.json");
        tokio::fs::write(&sidecar, serde_json::to_vec(&map).unwrap())
            .await
            .unwrap();
        (sidecar, cache)
    }

    fn orch_transcript(cache: &Path) -> PathBuf {
        cascade_cache_path(cache, ORCH)
    }

    #[tokio::test]
    async fn list_subagents_returns_cached_children_sorted_skips_uncached() {
        let tmp = TempDir::new().unwrap();
        // BACKEND has a cached file; FRONTEND's edge exists but its file isn't
        // fetched yet → skipped (try_exists guard).
        let (sidecar, cache) = seed(
            tmp.path(),
            &[
                (FRONTEND, ORCH, "Frontend Agent"),
                (BACKEND, ORCH, "Backend Agent"),
            ],
            &[BACKEND],
        )
        .await;
        let found = list_subagents_in(&orch_transcript(&cache), &sidecar, &cache).await;
        assert_eq!(found, vec![cascade_cache_path(&cache, BACKEND)]);
    }

    #[tokio::test]
    async fn list_subagents_empty_for_non_orchestrator() {
        let tmp = TempDir::new().unwrap();
        let (sidecar, cache) = seed(
            tmp.path(),
            &[(FRONTEND, ORCH, "Frontend Agent")],
            &[FRONTEND],
        )
        .await;
        // A worker (FRONTEND) is not itself a parent → empty roster.
        let worker = cascade_cache_path(&cache, FRONTEND);
        assert!(
            list_subagents_in(&worker, &sidecar, &cache)
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn locate_subagent_resolves_valid_rejects_unrelated_and_traversal() {
        let tmp = TempDir::new().unwrap();
        let (sidecar, cache) = seed(
            tmp.path(),
            &[(FRONTEND, ORCH, "Frontend Agent")],
            &[FRONTEND],
        )
        .await;
        let orch = orch_transcript(&cache);
        assert_eq!(
            locate_subagent_in(&orch, FRONTEND, &sidecar, &cache).await,
            Some(cascade_cache_path(&cache, FRONTEND)),
        );
        // Not a child of this parent / traversal id both reject.
        assert!(
            locate_subagent_in(&orch, BACKEND, &sidecar, &cache)
                .await
                .is_none()
        );
        assert!(
            locate_subagent_in(&orch, "../../etc/passwd", &sidecar, &cache)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn subagent_label_reads_sidecar_then_falls_back() {
        let tmp = TempDir::new().unwrap();
        let (sidecar, cache) = seed(
            tmp.path(),
            &[(FRONTEND, ORCH, "Frontend Agent")],
            &[FRONTEND],
        )
        .await;
        assert_eq!(
            subagent_label_in(&cascade_cache_path(&cache, FRONTEND), &sidecar).await,
            "Frontend Agent"
        );
        // Unknown cascade → generic label.
        assert_eq!(
            subagent_label_in(&cascade_cache_path(&cache, BACKEND), &sidecar).await,
            "Sub-agent"
        );
    }

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let tmp = TempDir::new().unwrap();
        // The sidecar is written outside the cascade directory so mirror
        // housekeeping cannot prune it.
        let sidecar = tmp.path().join("state").join("spawn-edges.json");
        let edges = extract_spawn_edges(&trajectories_response());
        save_spawn_edges(&sidecar, &edges).await.unwrap();
        assert_eq!(load_spawn_edges(&sidecar).await, edges);
        assert!(!sidecar.starts_with(tmp.path().join("antigravity-api")));
    }

    #[tokio::test]
    async fn child_cascade_ids_come_from_the_sidecar_keys() {
        let tmp = TempDir::new().unwrap();
        let (sidecar, _cache) = seed(
            tmp.path(),
            &[
                (FRONTEND, ORCH, "Frontend Agent"),
                (BACKEND, ORCH, "Backend Agent"),
            ],
            &[],
        )
        .await;
        let ids = child_cascade_ids_in(&sidecar).await;
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(FRONTEND) && ids.contains(BACKEND));
        assert!(!ids.contains(ORCH));
        // A missing sidecar is simply "no orchestration".
        assert!(
            child_cascade_ids_in(&tmp.path().join("missing.json"))
                .await
                .is_empty()
        );
    }

    #[test]
    fn round_trip_subagent_id_from_cache_path() {
        let p = cascade_cache_path(Path::new("/cache"), FRONTEND);
        assert_eq!(subagent_id(&p).as_deref(), Some(FRONTEND));
    }

    #[test]
    fn is_valid_cascade_id_rejects_traversal() {
        assert!(is_valid_cascade_id(ORCH));
        assert!(!is_valid_cascade_id(""));
        assert!(!is_valid_cascade_id("../x"));
        assert!(!is_valid_cascade_id("a/b"));
    }
}
