use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use antiburn_local::analysis::{
    AppendOnlyGuarantee, RawSource, SessionInput, SessionMetricsAccumulator, SourceClaim,
    SourceFormat, VisitOutcome, reader_for,
};
use antiburn_local::discovery::scanner::{SessionMetadata, parse_session_metadata_str};
use antiburn_local::discovery::{
    FingerprintInputs, SessionLog, SessionSource, home_dir, source_version,
};
use antiburn_local::model::AgentKind;
use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::evidence::{AdmittedFile, BudgetExhausted, RequestBudget, TrustedRoot};
use crate::{
    Analysis, LOOKBACK_SECS, MAX_SESSIONS, PROTOCOL_VERSION, RemoteSession, Snapshot, now,
};

const NEUTRAL_SESSION_TITLE: &str = "Untitled session";

pub(crate) struct CompanionEntry {
    pub(crate) source: AdmittedFile,
    pub(crate) session_id: String,
    pub(crate) preview: String,
}

pub(crate) struct Entry {
    pub(crate) session: RemoteSession,
    pub(crate) agent_type: AgentKind,
    pub(crate) source: AdmittedFile,
    pub(crate) preview: String,
    pub(crate) head_hash: u64,
    pub(crate) codex_companions: Vec<CompanionEntry>,
    pub(crate) companion_roster_complete: bool,
}

struct ParsedCandidate {
    agent_type: AgentKind,
    source: AdmittedFile,
    preview: String,
    head_hash: u64,
    metadata: SessionMetadata,
    parent_session_id: Option<String>,
    is_subagent: bool,
    is_internal: bool,
    updated_at: i64,
}

struct Discovery {
    entries: Vec<Entry>,
    truncated: bool,
    scan_truncated: bool,
    skipped: usize,
}

struct RootSpec {
    agent_type: AgentKind,
    root: Arc<TrustedRoot>,
    start: PathBuf,
}

struct CandidateRetention {
    candidates: HashMap<(AgentKind, String), ParsedCandidate>,
    target_children: Vec<CompanionEntry>,
    companion_roster_complete: bool,
    result_limit_truncated: bool,
}

impl CandidateRetention {
    fn new() -> Self {
        Self {
            candidates: HashMap::new(),
            target_children: Vec::new(),
            companion_roster_complete: true,
            result_limit_truncated: false,
        }
    }

    fn retain(
        &mut self,
        candidate: ParsedCandidate,
        target: Option<(&str, &str)>,
        cutoff: i64,
        skipped: &mut usize,
    ) {
        retain_candidate(candidate, target, cutoff, self, skipped);
    }
}

fn trusted_roots() -> Vec<RootSpec> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let mut configured = vec![
        (
            AgentKind::Claude,
            home.join(".claude").join("projects"),
            PathBuf::new(),
        ),
        (
            AgentKind::Codex,
            home.join(".codex"),
            PathBuf::from("sessions"),
        ),
    ];
    if let Some(codex_home) = std::env::var_os("CODEX_HOME").map(PathBuf::from) {
        configured.push((AgentKind::Codex, codex_home, PathBuf::from("sessions")));
    }

    let mut seen = HashSet::new();
    configured
        .into_iter()
        .filter_map(|(agent_type, path, start)| {
            let root = TrustedRoot::open(&path).ok()?;
            if !seen.insert((agent_type, root.path().to_path_buf())) {
                return None;
            }
            Some(RootSpec {
                agent_type,
                root,
                start,
            })
        })
        .collect()
}

fn path_depth(path: &Path) -> usize {
    path.components().count()
}

fn should_descend(agent: AgentKind, relative: &Path) -> bool {
    match agent {
        // The Claude parent layout is <project>/<session>.jsonl. Session
        // subdirectories hold companions and are admitted only during export.
        AgentKind::Claude => path_depth(relative) <= 1,
        AgentKind::Codex => true,
        _ => false,
    }
}

fn is_candidate(agent: AgentKind, relative: &Path) -> bool {
    if relative.extension().and_then(|value| value.to_str()) != Some("jsonl") {
        return false;
    }
    match agent {
        AgentKind::Claude => path_depth(relative) == 2,
        AgentKind::Codex => true,
        _ => false,
    }
}

fn codex_origin(preview: &str) -> (Option<String>, bool, bool) {
    let Some(first) = preview.lines().next() else {
        return (None, false, false);
    };
    let Ok(value) = serde_json::from_str::<Value>(first) else {
        return (None, false, false);
    };
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return (None, false, false);
    }
    let payload = value.get("payload").unwrap_or(&value);
    let source = payload.get("source").unwrap_or(&Value::Null);
    let spawn = source.pointer("/subagent/thread_spawn");
    let parent = payload
        .get("parent_thread_id")
        .or_else(|| payload.get("parentThreadId"))
        .or_else(|| spawn.and_then(|value| value.get("parent_thread_id")))
        .or_else(|| spawn.and_then(|value| value.get("parentThreadId")))
        .and_then(Value::as_str)
        .filter(|id| crate::export::safe_identity(id))
        .map(str::to_owned);
    let source_text = serde_json::to_string(source)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let subagent =
        source.get("subagent").is_some() || source.as_str() == Some("subagent") || parent.is_some();
    let internal = ["guardian", "review", "compact", "compaction"]
        .iter()
        .any(|marker| source_text.contains(marker));
    (parent, subagent, internal)
}

fn modified_epoch(source: &AdmittedFile) -> i64 {
    source
        .stat
        .modified_nanos
        .and_then(|value| i64::try_from(value / 1_000_000_000).ok())
        .unwrap_or_default()
}

fn inspect_candidate(
    root: &RootSpec,
    relative: &Path,
    budget: &mut RequestBudget,
    target: Option<(&str, &str)>,
    cutoff: i64,
) -> Result<Option<ParsedCandidate>> {
    budget.inspect_candidate()?;
    let source = root.root.admit(relative)?;
    let updated_at = modified_epoch(&source);
    if updated_at < cutoff {
        let Some(("codex", target_id)) = target else {
            return Ok(None);
        };
        let origin = budget.read_origin(&source.file)?;
        let (parent, subagent, internal) = codex_origin(&origin.text());
        if !subagent || internal || parent.as_deref() != Some(target_id) {
            return Ok(None);
        }
    }
    let read = budget.read_preview(&source.file)?;
    let head_hash = source_version::head_hash_of(&read.bytes);
    let preview = read.text();
    let metadata = parse_session_metadata_str(&preview);
    let (parent_session_id, is_subagent, is_internal) = if root.agent_type == AgentKind::Codex {
        codex_origin(&preview)
    } else {
        (None, false, false)
    };
    Ok(Some(ParsedCandidate {
        agent_type: root.agent_type,
        source,
        preview,
        head_hash,
        metadata,
        parent_session_id,
        is_subagent,
        is_internal,
        updated_at,
    }))
}

fn enumerate(budget: &mut RequestBudget, target: Option<(&str, &str)>) -> Discovery {
    let roots = trusted_roots();
    enumerate_from_roots(budget, target, &roots)
}

fn enumerate_from_roots(
    budget: &mut RequestBudget,
    target: Option<(&str, &str)>,
    roots: &[RootSpec],
) -> Discovery {
    let cutoff = now().saturating_sub(LOOKBACK_SECS);
    let mut retained = CandidateRetention::new();
    let mut skipped = 0;
    let mut truncated = false;

    'roots: for root in roots {
        if target.is_some_and(|(agent, _)| root.agent_type.to_string() != agent) {
            continue;
        }
        let mut stack = vec![root.start.clone()];
        while let Some(directory) = stack.pop() {
            if budget.check_deadline().is_err() {
                truncated = true;
                break 'roots;
            }
            let path = root.root.path().join(&directory);
            let Ok(entries) = std::fs::read_dir(path) else {
                continue;
            };
            for entry in entries {
                if budget.visit_entry().is_err() {
                    truncated = true;
                    break 'roots;
                }
                let Ok(entry) = entry else {
                    skipped += 1;
                    continue;
                };
                let Ok(kind) = entry.file_type() else {
                    skipped += 1;
                    continue;
                };
                let relative = directory.join(entry.file_name());
                if kind.is_dir() && !kind.is_symlink() && should_descend(root.agent_type, &relative)
                {
                    stack.push(relative);
                    continue;
                }
                if !kind.is_file() || kind.is_symlink() || !is_candidate(root.agent_type, &relative)
                {
                    continue;
                }
                match inspect_candidate(root, &relative, budget, target, cutoff) {
                    Ok(Some(candidate)) => retained.retain(candidate, target, cutoff, &mut skipped),
                    Ok(None) => {}
                    Err(error) => {
                        skipped += 1;
                        if error.is::<BudgetExhausted>() {
                            truncated = true;
                            break 'roots;
                        }
                    }
                }
            }
        }
    }

    let scan_truncated = truncated;
    truncated |= retained.result_limit_truncated;
    let mut candidates: Vec<_> = retained.candidates.into_values().collect();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.updated_at));
    let mut entries = Vec::new();
    for candidate in candidates {
        let session_id = candidate.metadata.session_id.as_deref().unwrap_or_default();
        let log = SessionLog {
            agent_type: candidate.agent_type,
            source: SessionSource::File(candidate.source.display_path()),
            updated_at: Some(candidate.updated_at),
            environment: Default::default(),
        };
        let home = home_dir().unwrap_or_default();
        let title = candidate
            .metadata
            .title
            .clone()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| NEUTRAL_SESSION_TITLE.to_owned());
        entries.push(Entry {
            session: RemoteSession {
                agent: candidate.agent_type.to_string(),
                session_id: session_id.to_owned(),
                title: title.chars().take(200).collect(),
                cwd: candidate
                    .metadata
                    .cwd
                    .clone()
                    .map(|cwd| cwd.chars().take(1024).collect()),
                surface: log
                    .surface_label_with_content(&candidate.preview, &home)
                    .to_owned(),
                updated_at: Some(candidate.updated_at),
            },
            agent_type: candidate.agent_type,
            source: candidate.source,
            preview: candidate.preview,
            head_hash: candidate.head_hash,
            codex_companions: Vec::new(),
            companion_roster_complete: retained.companion_roster_complete,
        });
    }
    if target.is_some()
        && let Some(entry) = entries.first_mut()
    {
        retained
            .target_children
            .sort_by(|left, right| left.session_id.cmp(&right.session_id));
        entry.codex_companions = retained.target_children;
    }

    apply_codex_titles(&mut entries, roots, budget, &mut truncated);
    Discovery {
        entries,
        truncated,
        scan_truncated,
        skipped,
    }
}

fn retain_candidate(
    candidate: ParsedCandidate,
    target: Option<(&str, &str)>,
    cutoff: i64,
    retained: &mut CandidateRetention,
    skipped: &mut usize,
) {
    let Some(session_id) = candidate.metadata.session_id.clone() else {
        *skipped += 1;
        return;
    };
    if !crate::export::safe_identity(&session_id) {
        *skipped += 1;
        return;
    }
    if let Some((target_agent, target_id)) = target
        && candidate.agent_type == AgentKind::Codex
        && target_agent == "codex"
        && candidate.is_subagent
        && !candidate.is_internal
        && candidate.parent_session_id.as_deref() == Some(target_id)
    {
        if retained.target_children.len() == crate::export::MAX_FILES - 1 {
            retained.companion_roster_complete = false;
            return;
        }
        retained.target_children.push(CompanionEntry {
            source: candidate.source,
            session_id,
            preview: candidate.preview,
        });
        return;
    }
    if candidate.is_subagent || candidate.is_internal || candidate.updated_at < cutoff {
        return;
    }
    if let Some((target_agent, target_id)) = target
        && (candidate.agent_type.to_string() != target_agent || session_id != target_id)
    {
        return;
    }
    let key = (candidate.agent_type, session_id);
    if let Some(existing) = retained.candidates.get_mut(&key) {
        if candidate.updated_at > existing.updated_at {
            *existing = candidate;
        }
        return;
    }
    if retained.candidates.len() < MAX_SESSIONS {
        retained.candidates.insert(key, candidate);
        return;
    }

    retained.result_limit_truncated = true;
    let Some((oldest_key, oldest_updated)) = retained
        .candidates
        .iter()
        .min_by_key(|(_, candidate)| candidate.updated_at)
        .map(|(key, candidate)| (key.clone(), candidate.updated_at))
    else {
        return;
    };
    if candidate.updated_at > oldest_updated {
        retained.candidates.remove(&oldest_key);
        retained.candidates.insert(key, candidate);
    }
}

fn apply_codex_titles(
    entries: &mut [Entry],
    roots: &[RootSpec],
    budget: &mut RequestBudget,
    truncated: &mut bool,
) {
    let wanted: HashSet<String> = entries
        .iter()
        .filter(|entry| entry.agent_type == AgentKind::Codex)
        .map(|entry| entry.session.session_id.clone())
        .collect();
    if wanted.is_empty() {
        return;
    }
    let mut titles = HashMap::new();
    for root in roots
        .iter()
        .filter(|root| root.agent_type == AgentKind::Codex)
    {
        let Ok(source) = root.root.admit(Path::new("session_index.jsonl")) else {
            continue;
        };
        let Ok(read) = budget.read_title_index(&source.file) else {
            *truncated = true;
            break;
        };
        *truncated |= read.truncated;
        let mut bytes = read.bytes.as_slice();
        if read.truncated
            && let Some(last_newline) = bytes.iter().rposition(|byte| *byte == b'\n')
        {
            bytes = &bytes[..last_newline];
        }
        for line in bytes.split(|byte| *byte == b'\n') {
            let Ok(value) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            let Some(id) = value
                .get("id")
                .or_else(|| value.get("session_id"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if !wanted.contains(id) {
                continue;
            }
            let Some(title) = value
                .get("thread_name")
                .or_else(|| value.get("title"))
                .and_then(Value::as_str)
                .filter(|title| !title.trim().is_empty())
            else {
                continue;
            };
            titles.insert(id.to_owned(), title.chars().take(200).collect::<String>());
        }
    }
    for entry in entries {
        if let Some(title) = titles.get(&entry.session.session_id) {
            entry.session.title.clone_from(title);
        }
    }
}

pub(crate) fn discover_matching_with_budget(
    agent: &str,
    session_id: &str,
    budget: &mut RequestBudget,
) -> Result<Entry> {
    let discovery = enumerate(budget, Some((agent, session_id)));
    resolve_matching(discovery)
}

fn resolve_matching(discovery: Discovery) -> Result<Entry> {
    ensure!(
        !discovery.scan_truncated,
        "Session was not resolved within the bounded recent-session scan"
    );
    discovery
        .entries
        .into_iter()
        .next()
        .with_context(|| "Session is no longer in the recent discovery window; refresh the host")
}

pub async fn list() -> Snapshot {
    let mut budget = RequestBudget::new();
    let discovery = enumerate(&mut budget, None);
    Snapshot {
        version: PROTOCOL_VERSION,
        collected_at: now(),
        sessions: discovery
            .entries
            .into_iter()
            .map(|entry| entry.session)
            .collect(),
        truncated: discovery.truncated,
        skipped: discovery.skipped,
        lookback_secs: LOOKBACK_SECS,
    }
}

pub async fn analyze(agent: &str, session_id: &str) -> Result<Analysis> {
    let mut budget = RequestBudget::new();
    let entry = discover_matching_with_budget(agent, session_id, &mut budget)?;
    analyze_entry(agent, session_id, entry)
}

fn analyze_entry(agent: &str, session_id: &str, entry: Entry) -> Result<Analysis> {
    ensure!(
        entry.source.stat.size <= 512 * 1024 * 1024,
        "Session exceeds the 512 MiB analysis limit"
    );
    let claim = SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
        stat: entry.source.stat.clone(),
        head_hash: Some(entry.head_hash),
    });
    let input = SessionInput {
        agent: agent.to_owned(),
        session_id: session_id.to_owned(),
        source: RawSource::File(entry.source.descriptor_path()),
        source_format: if agent == "codex" {
            SourceFormat::CodexRolloutJsonl
        } else {
            SourceFormat::ClaudeJsonl
        },
        fork_parent_session_id: None,
    };
    let mut sink = SessionMetricsAccumulator::new(agent, session_id);
    let deadline = Instant::now() + Duration::from_secs(40);
    let outcome = reader_for(agent).visit_claimed(
        &input,
        &claim,
        AppendOnlyGuarantee::Absent,
        &|| Instant::now() >= deadline,
        &mut sink,
    )?;
    ensure!(
        Instant::now() < deadline,
        "Analysis exceeded its time limit"
    );
    ensure!(
        matches!(
            outcome,
            VisitOutcome::AcceptedFull | VisitOutcome::AcceptedPrefix { .. }
        ),
        "Session changed during analysis; retry"
    );
    let metrics = sink.metrics();
    ensure!(
        metrics.event_count > 0,
        "No supported metrics were found in this session"
    );
    Ok(Analysis {
        version: PROTOCOL_VERSION,
        collected_at: now(),
        session: entry.session,
        metrics,
        coverage: "Parent transcript only; delegated sessions are not combined. Provider allowances and checks are not collected.".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::BudgetLimits;

    fn codex_root(directory: &Path) -> RootSpec {
        let root_path = directory.join(".codex");
        std::fs::create_dir_all(root_path.join("sessions")).unwrap();
        RootSpec {
            agent_type: AgentKind::Codex,
            root: TrustedRoot::open(&root_path).unwrap(),
            start: PathBuf::from("sessions"),
        }
    }

    fn write_codex(path: &Path, id: &str, parent: Option<&str>) {
        let source = parent.map_or_else(
            || serde_json::json!("cli"),
            |parent| {
                serde_json::json!({
                    "subagent": {"thread_spawn": {"parent_thread_id": parent}}
                })
            },
        );
        let record = serde_json::json!({
            "type": "session_meta",
            "payload": {"id": id, "source": source}
        });
        std::fs::write(path, format!("{record}\n")).unwrap();
    }

    fn age_transcript(path: &Path) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_times(
            std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1)),
        )
        .unwrap();
    }

    #[test]
    fn expired_history_does_not_starve_recent_targets_or_hide_old_children() {
        let old_store = tempfile::tempdir().unwrap();
        let recent_store = tempfile::tempdir().unwrap();
        let old_root = codex_root(old_store.path());
        let recent_root = codex_root(recent_store.path());
        for index in 0..65 {
            let path = old_root
                .root
                .path()
                .join(format!("sessions/old-{index}.jsonl"));
            write_codex(&path, &format!("old-{index}"), None);
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(1024 * 1024)
                .unwrap();
            age_transcript(&path);
        }
        let child = old_root.root.path().join("sessions/child.jsonl");
        write_codex(&child, "child", Some("parent"));
        let header = std::fs::read_to_string(&child).unwrap();
        std::fs::write(&child, format!("{}{header}", " ".repeat(1024))).unwrap();
        age_transcript(&child);
        write_codex(
            &recent_root.root.path().join("sessions/parent.jsonl"),
            "parent",
            None,
        );
        let mut roots = [old_root, recent_root];
        for _ in 0..2 {
            let list = enumerate_from_roots(&mut RequestBudget::new(), None, &roots);
            assert!(!list.truncated);
            assert_eq!(list.entries.len(), 1);
            let target =
                enumerate_from_roots(&mut RequestBudget::new(), Some(("codex", "parent")), &roots);
            let entry = resolve_matching(target).unwrap();
            assert_eq!(entry.session.session_id, "parent");
            assert_eq!(entry.codex_companions.len(), 1);
            assert_eq!(entry.codex_companions[0].session_id, "child");
            assert!(entry.companion_roster_complete);
            roots.reverse();
        }
    }

    #[test]
    fn targeted_scan_does_not_spend_preview_budget_on_another_agent() {
        let store = tempfile::tempdir().unwrap();
        let claude = store.path().join(".claude/projects");
        std::fs::create_dir_all(claude.join("project")).unwrap();
        let path = claude.join("project/large.jsonl");
        std::fs::write(
            &path,
            br#"{"type":"user","sessionId":"large","message":{"role":"user","content":"fixture"}}
"#,
        )
        .unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_len(1024 * 1024)
            .unwrap();
        let claude = RootSpec {
            agent_type: AgentKind::Claude,
            root: TrustedRoot::open(&claude).unwrap(),
            start: PathBuf::new(),
        };
        let codex = codex_root(store.path());
        write_codex(
            &codex.root.path().join("sessions/parent.jsonl"),
            "parent",
            None,
        );
        let mut budget = RequestBudget::with_limits(BudgetLimits {
            preview_bytes: 512,
            ..BudgetLimits::default()
        });
        let target = enumerate_from_roots(&mut budget, Some(("codex", "parent")), &[claude, codex]);
        assert_eq!(
            resolve_matching(target).unwrap().session.session_id,
            "parent"
        );
    }

    #[test]
    fn incomplete_old_origin_scan_still_rejects_target_export() {
        let store = tempfile::tempdir().unwrap();
        let root = codex_root(store.path());
        let child = root.root.path().join("sessions/child.jsonl");
        std::fs::write(
            &child,
            format!(
                "{}{}",
                " ".repeat(2048),
                r#"{"type":"session_meta","payload":{"id":"child","parent_thread_id":"parent"}}"#
            ),
        )
        .unwrap();
        age_transcript(&child);
        let mut budget = RequestBudget::with_limits(BudgetLimits {
            preview_bytes: 512,
            ..BudgetLimits::default()
        });
        let discovery = enumerate_from_roots(&mut budget, Some(("codex", "parent")), &[root]);
        assert!(discovery.scan_truncated);
        assert!(resolve_matching(discovery).is_err());
    }

    #[test]
    fn tiny_entry_budget_marks_enumeration_truncated() {
        let mut budget = RequestBudget::with_limits(BudgetLimits {
            directory_entries: 0,
            candidates: 1,
            preview_bytes: 1,
            label_bytes: 1,
            title_index_bytes: 1,
            elapsed: Duration::from_secs(1),
        });
        let discovery = enumerate(&mut budget, None);
        // Missing roots are a complete empty result; an existing configured
        // root reaches the injected barrier and reports truncation.
        if !trusted_roots().is_empty() {
            assert!(discovery.truncated);
        }
    }

    #[test]
    fn parent_found_before_codex_companion_truncation_is_rejected() {
        let parent_store = tempfile::tempdir().unwrap();
        let child_store = tempfile::tempdir().unwrap();
        let parent_root = codex_root(parent_store.path());
        let child_root = codex_root(child_store.path());
        write_codex(
            &parent_root.root.path().join("sessions/parent.jsonl"),
            "parent",
            None,
        );
        write_codex(
            &child_root.root.path().join("sessions/child.jsonl"),
            "child",
            Some("parent"),
        );
        let mut budget = RequestBudget::with_limits(BudgetLimits {
            candidates: 1,
            ..BudgetLimits::default()
        });

        let discovery = enumerate_from_roots(
            &mut budget,
            Some(("codex", "parent")),
            &[parent_root, child_root],
        );

        assert_eq!(discovery.entries.len(), 1);
        assert!(discovery.scan_truncated);
        assert!(resolve_matching(discovery).is_err());
    }

    #[test]
    fn optional_title_index_truncation_does_not_reject_complete_target_scan() {
        let store = tempfile::tempdir().unwrap();
        let root = codex_root(store.path());
        write_codex(
            &root.root.path().join("sessions/parent.jsonl"),
            "parent",
            None,
        );
        std::fs::write(
            root.root.path().join("session_index.jsonl"),
            br#"{"id":"parent","thread_name":"title beyond tiny budget"}
"#,
        )
        .unwrap();
        let mut budget = RequestBudget::with_limits(BudgetLimits {
            title_index_bytes: 8,
            ..BudgetLimits::default()
        });

        let discovery = enumerate_from_roots(&mut budget, Some(("codex", "parent")), &[root]);

        assert!(discovery.truncated);
        assert!(!discovery.scan_truncated);
        assert_eq!(
            resolve_matching(discovery).unwrap().session.session_id,
            "parent"
        );
    }

    #[test]
    fn analyze_never_reopens_a_replacement_path_after_admission() {
        let store = tempfile::tempdir().unwrap();
        let root = codex_root(store.path());
        let relative = Path::new("sessions/original.jsonl");
        let path = root.root.path().join(relative);
        let fixture = include_str!(
            "../../antiburn-local/tests/fixtures/codex_characterization/task_complete_errors.jsonl"
        );
        std::fs::write(&path, fixture).unwrap();
        let mut budget = RequestBudget::new();
        let entry = resolve_matching(enumerate_from_roots(
            &mut budget,
            Some(("codex", "synthetic-task-complete-errors")),
            &[root],
        ))
        .unwrap();
        std::fs::rename(&path, path.with_extension("admitted")).unwrap();
        std::fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"id":"synthetic-task-complete-errors","source":"cli"}}
{"type":"event_msg","payload":{"type":"user_message","message":"outside-replacement-marker"}}
"#,
        )
        .unwrap();

        match analyze_entry("codex", "synthetic-task-complete-errors", entry) {
            Ok(analysis) => assert!(analysis.metrics.tokens_out > 0),
            Err(error) => assert!(!error.to_string().contains("outside-replacement-marker")),
        }
    }
}
