//! Claude Code adapter — the richest source of token + tool data.
//!
//! Claude stores one JSONL transcript per session at
//! `~/.claude/projects/<encoded>/<session_id>.jsonl`. Each line is a JSON
//! record; assistant lines carry `message.usage` (input/output/cache tokens)
//! and `message.content[]` with `text` / `thinking` / `tool_use` parts, while
//! user lines carry `tool_result` blocks that flag errors.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Read};
use std::path::Path;

use anyhow::Context;
use serde_json::Value;

use crate::analysis::framing::{BoundedJsonlReader, FramedRecord, PartialReason, RecordSkip};
use crate::analysis::initial_context::ClaudeContextAccumulator;
use crate::analysis::interface::{
    ContextSourceKind, EvidenceObservation, NormalizedRecord, RawSource, RecordSink,
    SessionCollector, SessionInput, SessionSummary, TurnContent, VendorAdapter, VisitOutcome,
};
use crate::analysis::model::{NormalizedEvent, NormalizedSession, ToolCall, Usage};
use crate::analysis::records::{
    RecordShape, evidence_observations, extract_content_parts, is_inert_recognized_eventless,
    is_inert_unrecognized, is_recognized_eventless, parse_record, record_discriminator,
    thread_identity_field,
};
use crate::analysis::source_validity::{AppendOnlyGuarantee, PinnedSource, SourceClaim};
use crate::analysis::threads::ThreadResolver;
use crate::discovery::SubagentMeta;

/// The marker Claude Code writes into a `Skill` tool's transcript output,
/// naming the skill's base directory. Its presence records the skill as one
/// that actually ran, distinct from a `<command-name>` that merely typed the
/// skill's slash command.
const SKILL_BASE_MARKER: &str = "Base directory for this skill:";

/// Flatten a record's message text — string content, or the `text` of its content
/// blocks — for scanning `<command-name>` tags and skill base-directory markers.
fn record_text(value: &Value) -> String {
    let Some(obj) = value.as_object() else {
        return String::new();
    };
    let content = obj
        .get("message")
        .and_then(|m| m.as_object())
        .and_then(|m| m.get("content"))
        .or_else(|| obj.get("content"));
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => {
            let mut out = String::new();
            for item in items {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    out.push_str(text);
                    out.push('\n');
                }
            }
            out
        }
        _ => String::new(),
    }
}

/// Record the skill name from every "Base directory for this skill: <path>" marker
/// in `text` — the set of skills that actually loaded this session.
fn collect_skill_base_names_from_text(text: &str, out: &mut HashSet<String>) {
    for line in text.lines() {
        if let Some((_, rest)) = line.split_once(SKILL_BASE_MARKER)
            && let Some(name) = skill_base_name_from_path(rest)
        {
            out.insert(name);
        }
    }
}

/// Skill name from a base-directory marker path: the final path segment, or its
/// parent when the path points straight at the `SKILL.md` file. Cross-platform
/// (splits on `/` and `\`).
fn skill_base_name_from_path(path: &str) -> Option<String> {
    let mut segments: Vec<&str> = path
        .trim()
        .trim_matches(['`', '"', '\''])
        .split(['/', '\\'])
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect();
    let last = segments.pop()?;
    if last.eq_ignore_ascii_case("SKILL.md") {
        return segments.pop().map(str::to_string);
    }
    Some(last.to_string())
}

/// The `<command-name>` values in `text`, leading `/` stripped:
/// `<command-name>/code-review</command-name>` → `"code-review"`.
fn command_names_in_text(text: &str) -> Vec<String> {
    const OPEN: &str = "<command-name>";
    const CLOSE: &str = "</command-name>";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start + OPEN.len()..];
        let Some(end) = after.find(CLOSE) else {
            break;
        };
        let name = after[..end].trim().trim_start_matches('/').trim();
        if !name.is_empty() {
            out.push(name.to_string());
        }
        rest = &after[end + CLOSE.len()..];
    }
    out
}

/// The skill base name a command resolves to, if any: a direct hit, or a
/// `plugin:skill` whose bare segment ran. `None` for non-skill commands.
fn command_skill_name(command: &str, skill_base_names: &HashSet<String>) -> Option<String> {
    if skill_base_names.contains(command) {
        return Some(command.to_string());
    }
    let bare = command.rsplit(':').next().unwrap_or(command);
    skill_base_names.contains(bare).then(|| bare.to_string())
}

/// The skill descriptions from a `skill_listing` attachment: each `- name:
/// description` line becomes a `ContextSource` observation for the named
/// skill. Only Claude's transcript writes this attachment type.
fn skill_listing_observations(value: &Value) -> Vec<EvidenceObservation> {
    let Some(attachment) = value.get("attachment") else {
        return Vec::new();
    };
    if attachment.get("type").and_then(Value::as_str) != Some("skill_listing") {
        return Vec::new();
    }
    attachment
        .get("content")
        .and_then(Value::as_str)
        .into_iter()
        .flat_map(|content| content.lines())
        .filter_map(|line| {
            let (name, description) = line.trim().strip_prefix("- ")?.split_once(':')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            let description = description.trim();
            Some(EvidenceObservation::ContextSource {
                kind: ContextSourceKind::Skill,
                name: name.to_owned(),
                description: (!description.is_empty()).then(|| description.to_owned()),
            })
        })
        .collect()
}

/// The `uuid` set of `path`'s direct replay source, when `path` is a fork
/// sub-agent transcript. Empty when `path` is not a fork, or when its
/// `.meta.json` sidecar or its replay source cannot be read: this fails
/// open, so a session with an unreadable parent still shows its duplicate
/// records rather than silently dropping data.
///
/// A fork sub-agent transcript replays its parent agent's records (same
/// `uuid`) before it appends its own new records. The direct parent covers
/// a whole fork chain: the parent transcript already holds its own replayed
/// records, so the direct parent's `uuid` set covers the chain transitively.
fn replay_skip_uuids(path: &Path) -> HashSet<String> {
    let Some(meta) = read_fork_meta(path) else {
        return HashSet::new();
    };
    if !meta.is_fork {
        return HashSet::new();
    }
    let Some(parent_path) = fork_parent_path(path, meta.parent_agent_id.as_deref()) else {
        return HashSet::new();
    };
    let Ok(file) = File::open(&parent_path) else {
        return HashSet::new();
    };
    collect_record_uuids(BufReader::new(file))
}

/// Reads and parses `path`'s `.meta.json` sidecar. `None` when the sidecar
/// is missing or is not valid JSON in the expected shape — the caller treats
/// that the same as "not a fork".
fn read_fork_meta(path: &Path) -> Option<SubagentMeta> {
    let meta_path = path.with_extension("meta.json");
    let content = std::fs::read_to_string(meta_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// The direct replay source for a fork sub-agent transcript at `path`.
///
/// `parent_agent_id`, when present, names a sibling sub-agent transcript in
/// the same `subagents/` directory. Its absence means the fork's parent is
/// the top-level session: `path` sits at `<dir>/<session-id>/subagents/agent-*.jsonl`,
/// so the session's own transcript is `<dir>/<session-id>.jsonl`.
fn fork_parent_path(path: &Path, parent_agent_id: Option<&str>) -> Option<std::path::PathBuf> {
    let subagents_dir = path.parent()?;
    if let Some(parent_agent_id) = parent_agent_id {
        return Some(subagents_dir.join(format!("agent-{parent_agent_id}.jsonl")));
    }
    let session_dir = subagents_dir.parent()?;
    let session_id = session_dir.file_name()?.to_str()?;
    Some(session_dir.parent()?.join(format!("{session_id}.jsonl")))
}

/// Every `uuid` a JSONL source declares at the top level of a record. A line
/// that fails to parse as JSON, or carries no `uuid`, contributes nothing —
/// the caller only needs the identities it can be sure of.
fn collect_record_uuids(reader: impl BufRead) -> HashSet<String> {
    let mut uuids = HashSet::new();
    for line in reader.lines() {
        let Ok(line) = line else {
            break;
        };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(uuid) = thread_identity_field(&value, "uuid") {
            uuids.insert(uuid);
        }
    }
    uuids
}

pub struct ClaudeAdapter;

impl VendorAdapter for ClaudeAdapter {
    fn agent(&self) -> &'static str {
        "claude"
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        let mut collector = SessionCollector::new(input.agent.clone(), input.session_id.clone());
        self.visit(input, &mut collector)?;
        collector.into_session()
    }

    fn visit(
        &self,
        input: &SessionInput,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        (|| -> anyhow::Result<VisitOutcome> {
            let summary = match &input.source {
                RawSource::File(path) => {
                    let replayed_uuids = replay_skip_uuids(path);
                    let file = File::open(path)?;
                    self.visit_reader(BufReader::new(file), &|| false, sink, &replayed_uuids)?
                }
                RawSource::Jsonl(content) => {
                    let suffix: &[u8] = if content.ends_with('\n') { b"" } else { b"\n" };
                    let source = Cursor::new(content.as_bytes()).chain(suffix);
                    self.visit_reader(BufReader::new(source), &|| false, sink, &HashSet::new())?
                }
                RawSource::Sqlite(path) => {
                    anyhow::bail!(
                        "sqlite source must be handled by the sqlite adapter: {}",
                        path.display()
                    )
                }
            };
            sink.finish(summary);
            Ok(VisitOutcome::Unvalidated)
        })()
        .with_context(|| format!("reading claude session {}", input.session_id))
    }

    fn visit_claimed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        guarantee: AppendOnlyGuarantee,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        ClaudeAdapter::visit_claimed(self, input, claim, guarantee, cancel, sink)
    }
}

impl ClaudeAdapter {
    pub fn visit_claimed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        guarantee: AppendOnlyGuarantee,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        (|| -> anyhow::Result<VisitOutcome> {
            let RawSource::File(path) = &input.source else {
                anyhow::bail!("a claimed Claude source must be a file");
            };
            let replayed_uuids = replay_skip_uuids(path);
            let mut pinned = match PinnedSource::open(path, claim.clone())? {
                Ok(pinned) => pinned,
                Err(reason) => return Ok(VisitOutcome::SourceChanged(reason)),
            };
            let limit = match guarantee {
                AppendOnlyGuarantee::Evidenced => claim.boundary,
                AppendOnlyGuarantee::Absent => u64::MAX,
            };
            let summary = self.visit_reader(
                BufReader::new(pinned.reader(limit)),
                cancel,
                sink,
                &replayed_uuids,
            )?;
            let outcome = match guarantee {
                AppendOnlyGuarantee::Evidenced => match pinned.recheck_prefix()? {
                    Some(reason) => VisitOutcome::SourceChanged(reason),
                    None => VisitOutcome::AcceptedPrefix {
                        boundary: claim.boundary,
                    },
                },
                AppendOnlyGuarantee::Absent => match pinned.recheck_full()? {
                    Some(reason) => VisitOutcome::SourceChanged(reason),
                    None => VisitOutcome::AcceptedFull,
                },
            };
            if matches!(outcome, VisitOutcome::SourceChanged(_)) {
                return Ok(outcome);
            }
            sink.finish(summary);
            Ok(outcome)
        })()
        .with_context(|| format!("reading claimed Claude session {}", input.session_id))
    }

    fn visit_reader(
        &self,
        reader: impl BufRead,
        cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
        replayed_uuids: &HashSet<String>,
    ) -> anyhow::Result<SessionSummary> {
        let mut reader = BoundedJsonlReader::new(reader);
        let mut state = ClaudeStreamState::default();

        while let Some(record) = reader.next_record(cancel) {
            match record {
                FramedRecord::Skipped(skip) => match skip {
                    RecordSkip::Oversized { .. } | RecordSkip::IncompleteTail { .. } => {
                        sink.record(NormalizedRecord::Unusable(skip.partial_reason()));
                    }
                    RecordSkip::ReadFailed { index, kind } => {
                        anyhow::bail!("Claude record {index} read failed: {kind:?}");
                    }
                    RecordSkip::Cancelled { index } => {
                        anyhow::bail!("Claude record {index} read was cancelled");
                    }
                },
                FramedRecord::Complete { bytes, .. } => {
                    let record = std::str::from_utf8(bytes)
                        .context("Claude transcript record is not valid UTF-8")?;
                    let Ok(value) = serde_json::from_str::<Value>(record) else {
                        sink.record(NormalizedRecord::Unusable(
                            crate::analysis::framing::PartialReason::MalformedRecord,
                        ));
                        continue;
                    };

                    // A fork sub-agent replays its parent's records with the
                    // parent's own `uuid` before it appends its own new
                    // records. A replayed record is not new work: skip it
                    // here, before it can become a turn row, duplicate
                    // usage, or any other evidence signal.
                    if thread_identity_field(&value, "uuid")
                        .is_some_and(|uuid| replayed_uuids.contains(&uuid))
                    {
                        continue;
                    }

                    state.context.observe(&value);
                    for observation in skill_listing_observations(&value)
                        .into_iter()
                        .chain(evidence_observations(&value))
                    {
                        sink.record(NormalizedRecord::Observation(Box::new(observation)));
                    }
                    let has_skill_marker = record.contains(SKILL_BASE_MARKER);
                    let has_command_name = record.contains("<command-name>");
                    let text = (has_skill_marker || has_command_name).then(|| record_text(&value));
                    if has_skill_marker {
                        collect_skill_base_names_from_text(
                            text.as_deref().unwrap_or_default(),
                            &mut state.skill_base_names,
                        );
                    }

                    let Some(mut event) = parse_record(&value, RecordShape::Claude) else {
                        let allowlisted = is_recognized_eventless(&value);
                        let structurally_inert = if allowlisted {
                            is_inert_recognized_eventless(&value)
                        } else {
                            is_inert_unrecognized(&value)
                        };
                        // A known eventless record cannot own a late tool call.
                        // Other records fail closed when no parsed event owns the marker.
                        let inert = structurally_inert && (allowlisted || !has_command_name);
                        if !inert || !allowlisted {
                            sink.record(NormalizedRecord::Observation(Box::new(
                                crate::analysis::interface::EvidenceObservation::UnrecognizedType {
                                    discriminator: record_discriminator(&value),
                                    inert,
                                },
                            )));
                        }
                        if !inert {
                            sink.record(NormalizedRecord::Unusable(
                                crate::analysis::framing::PartialReason::UnrecognizedRecordType,
                            ));
                        }
                        continue;
                    };

                    let link = event
                        .parent_uuid
                        .as_deref()
                        .or(event.logical_parent_uuid.as_deref());
                    event.thread_id = state.threads.resolve(event.uuid.as_deref(), link);
                    state.observe_model(event.model.as_deref());
                    state.dedup_usage(&mut event);
                    if has_command_name {
                        let commands = command_names_in_text(text.as_deref().unwrap_or_default());
                        event.may_resolve_late_tool = commands.iter().any(|command| {
                            command_skill_name(command, &state.skill_base_names).is_some()
                                || !is_builtin_command(command)
                        });
                        event.late_tool_candidate_is_builtin = !commands.is_empty()
                            && commands.iter().all(|command| {
                                command_skill_name(command, &state.skill_base_names).is_none()
                                    && is_builtin_command(command)
                            });
                        state.pending_commands.push((state.ordinal, commands));
                    }
                    let content_parts = extract_content_parts(&value, event.role);
                    sink.record(NormalizedRecord::MetricsEvent(Box::new(event)));
                    if !content_parts.is_empty() {
                        sink.record(NormalizedRecord::TurnContent(Box::new(TurnContent {
                            parts: content_parts,
                        })));
                    }
                    state.ordinal += 1;
                }
            }
        }

        Ok(state.into_summary())
    }
}

#[derive(Default)]
struct ClaudeStreamState {
    max_usage_by_message_id: HashMap<String, Usage>,
    context_window: Option<u64>,
    first_model: Option<String>,
    best_priceable: Option<(String, f64)>,
    last_seen_model: Option<String>,
    skill_base_names: HashSet<String>,
    pending_commands: Vec<(usize, Vec<String>)>,
    ordinal: usize,
    context: ClaudeContextAccumulator,
    threads: ThreadResolver,
}

impl ClaudeStreamState {
    fn dedup_usage(&mut self, event: &mut NormalizedEvent) {
        let Some(id) = event.message_id.clone() else {
            return;
        };
        let current = event.usage;
        let previous = self
            .max_usage_by_message_id
            .get(&id)
            .copied()
            .unwrap_or_default();
        event.usage = Usage {
            input_tokens: current.input_tokens.saturating_sub(previous.input_tokens),
            output_tokens: current.output_tokens.saturating_sub(previous.output_tokens),
            cache_read_tokens: current
                .cache_read_tokens
                .saturating_sub(previous.cache_read_tokens),
            cache_creation_tokens: current
                .cache_creation_tokens
                .saturating_sub(previous.cache_creation_tokens),
        };
        self.max_usage_by_message_id.insert(
            id,
            Usage {
                input_tokens: current.input_tokens.max(previous.input_tokens),
                output_tokens: current.output_tokens.max(previous.output_tokens),
                cache_read_tokens: current.cache_read_tokens.max(previous.cache_read_tokens),
                cache_creation_tokens: current
                    .cache_creation_tokens
                    .max(previous.cache_creation_tokens),
            },
        );
    }

    fn observe_model(&mut self, model: Option<&str>) {
        let Some(model) = model else {
            return;
        };
        if self.last_seen_model.as_deref() == Some(model) {
            return;
        }
        self.last_seen_model = Some(model.to_string());
        if let Some(window) = model_context_window(model) {
            self.context_window = Some(
                self.context_window
                    .map_or(window, |current| current.max(window)),
            );
        }
        if self.first_model.is_none() {
            self.first_model = Some(model.to_string());
        }
        if let Some(pricing) = crate::analysis::pricing::lookup_pricing(model) {
            let rank = pricing.input_cost_per_token + pricing.output_cost_per_token;
            if self
                .best_priceable
                .as_ref()
                .is_none_or(|(_, current_rank)| rank > *current_rank)
            {
                self.best_priceable = Some((model.to_string(), rank));
            }
        }
    }

    fn into_summary(self) -> SessionSummary {
        let (initial_context, skill_descriptions) = self
            .context
            .finish(crate::analysis::tool_catalog::embedded());
        let model = self
            .best_priceable
            .map(|(model, _)| model)
            .or(self.first_model);
        let mut late_tools = Vec::new();
        for (ordinal, commands) in self.pending_commands {
            for command in commands {
                if let Some(skill) = command_skill_name(&command, &self.skill_base_names) {
                    let mut call = ToolCall::new("skill");
                    call.detail = Some(skill);
                    late_tools.push((ordinal, call));
                }
            }
        }
        // A capped thread resolver means some records past the cap could not
        // be linked into their real thread: the same kind of attribution
        // loss the cache group's unresolved-parent-link check reports.
        let mut coverage_gaps = Vec::new();
        if self.threads.capped() {
            coverage_gaps.push(PartialReason::AttributionIncomplete);
        }
        SessionSummary {
            cache_write_tokens_available: true,
            context_window: self.context_window,
            model,
            started_at_ms: None,
            coverage_gaps,
            late_tools,
            initial_context,
            skill_descriptions,
        }
    }
}

fn is_builtin_command(command: &str) -> bool {
    const BUILTINS: &[&str] = &[
        "clear",
        "compact",
        "context",
        "cost",
        "doctor",
        "exit",
        "export",
        "help",
        "hooks",
        "ide",
        "init",
        "login",
        "logout",
        "mcp",
        "memory",
        "model",
        "permissions",
        "plugin",
        "privacy-settings",
        "release-notes",
        "remote-control",
        "rename",
        "resume",
        "review",
        "security-review",
        "stats",
        "status",
        "statusline",
        "terminal-setup",
        "upgrade",
        "vim",
    ];
    BUILTINS
        .iter()
        .any(|builtin| command.eq_ignore_ascii_case(builtin))
}

/// The context window for a recognized Claude model family. Unknown model ids
/// stay unavailable rather than inheriting a misleading 200k guess.
fn model_context_window(model: &str) -> Option<u64> {
    let m = model.to_ascii_lowercase();
    let is_1m = m.contains("opus-4")
        || m.contains("fable-5")
        || m.contains("sonnet-5")
        || m.contains("sonnet-4-5")
        || m.contains("sonnet-4-6")
        || m.contains("sonnet-4-7")
        || m.contains("sonnet-4-8")
        || m.contains("sonnet-4-9");
    if is_1m {
        Some(1_000_000)
    } else if m.contains("haiku")
        || m.contains("claude-3")
        || m.contains("sonnet-4-0")
        || m.contains("sonnet-4-202")
    {
        Some(200_000)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::{self, BufReader, Error, Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::analysis::{PartialReason, RecordCoverage};
    use crate::discovery::source_version::head_hash_of;
    use crate::discovery::{FingerprintInputs, SourceStat};
    use tempfile::TempDir;

    const FIRST_RECORD: &str = concat!(
        r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","content":[{"type":"text","text":"first"}]}}"#,
        "\n",
    );
    const SECOND_RECORD: &str = concat!(
        r#"{"type":"assistant","timestamp":"2024-06-01T12:01:00Z","message":{"role":"assistant","content":[{"type":"text","text":"second"}]}}"#,
        "\n",
    );

    fn file_input(path: &Path) -> SessionInput {
        SessionInput {
            agent: "claude".to_string(),
            session_id: "claimed-session".to_string(),
            source: RawSource::File(path.to_path_buf()),
        }
    }

    fn claim_for_path(path: &Path) -> SourceClaim {
        let file = File::open(path).expect("open source for claim");
        let stat = SourceStat::from_open_std_file(&file).expect("stat source for claim");
        let bytes = std::fs::read(path).expect("read source for claim");
        SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
            stat,
            head_hash: Some(head_hash_of(&bytes)),
        })
    }

    fn write_source(directory: &TempDir, bytes: &[u8]) -> std::path::PathBuf {
        let path = directory.path().join("session.jsonl");
        std::fs::write(&path, bytes).expect("write source");
        path
    }

    #[test]
    fn a_record_whose_newline_is_past_the_boundary_is_not_committed() {
        let directory = TempDir::new().expect("tempdir");
        let split = SECOND_RECORD.len() / 2;
        let generation = [FIRST_RECORD.as_bytes(), &SECOND_RECORD.as_bytes()[..split]].concat();
        let path = write_source(&directory, &generation);
        let claim = claim_for_path(&path);
        OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open source for append")
            .write_all(&SECOND_RECORD.as_bytes()[split..])
            .expect("complete second record");
        let input = file_input(&path);
        let mut collector = SessionCollector::new("claude", "claimed-session");

        let outcome = ClaudeAdapter
            .visit_claimed(
                &input,
                &claim,
                AppendOnlyGuarantee::Evidenced,
                &|| false,
                &mut collector,
            )
            .expect("visit claimed prefix");

        assert_eq!(
            outcome,
            VisitOutcome::AcceptedPrefix {
                boundary: claim.boundary,
            }
        );
        assert_eq!(collector.coverage(), RecordCoverage::Partial);
        assert_eq!(
            collector.partial_reasons(),
            &std::collections::BTreeSet::from([PartialReason::IncompleteTail])
        );
        assert_eq!(
            collector
                .into_session()
                .expect("accepted prefix must publish")
                .events
                .len(),
            1
        );
    }

    #[test]
    fn a_source_changed_read_cannot_publish() {
        let directory = TempDir::new().expect("tempdir");
        let source = [FIRST_RECORD.as_bytes(), SECOND_RECORD.as_bytes()].concat();
        let path = write_source(&directory, &source);
        let claim = claim_for_path(&path);
        let input = file_input(&path);
        let mut sink = HeadMutatingSink::new(&path);

        let outcome = ClaudeAdapter
            .visit_claimed(
                &input,
                &claim,
                AppendOnlyGuarantee::Evidenced,
                &|| false,
                &mut sink,
            )
            .expect("visit changed source");

        assert_eq!(
            outcome,
            VisitOutcome::SourceChanged(crate::analysis::SourceChangedReason::HeadRegionMismatch)
        );
        assert!(sink.collector.into_session().is_err());
    }

    #[test]
    fn a_cancelled_claimed_read_does_not_finish_the_sink() {
        let directory = TempDir::new().expect("tempdir");
        let path = write_source(&directory, FIRST_RECORD.as_bytes());
        let claim = claim_for_path(&path);
        let input = file_input(&path);
        let mut collector = SessionCollector::new("claude", "claimed-session");

        let result = ClaudeAdapter.visit_claimed(
            &input,
            &claim,
            AppendOnlyGuarantee::Absent,
            &|| true,
            &mut collector,
        );

        assert!(result.is_err());
        assert!(collector.into_session().is_err());
    }

    #[test]
    fn an_accepted_prefix_publishes_its_records() {
        let directory = TempDir::new().expect("tempdir");
        let path = write_source(&directory, FIRST_RECORD.as_bytes());
        let claim = claim_for_path(&path);
        let input = file_input(&path);
        let mut collector = SessionCollector::new("claude", "claimed-session");

        let outcome = ClaudeAdapter
            .visit_claimed(
                &input,
                &claim,
                AppendOnlyGuarantee::Evidenced,
                &|| false,
                &mut collector,
            )
            .expect("visit stable prefix");

        assert_eq!(
            outcome,
            VisitOutcome::AcceptedPrefix {
                boundary: claim.boundary,
            }
        );
        assert_eq!(
            collector
                .into_session()
                .expect("accepted prefix must publish")
                .events
                .len(),
            1
        );
    }

    #[test]
    fn a_plain_claude_read_reports_unvalidated() {
        let input = SessionInput {
            agent: "claude".to_string(),
            session_id: "plain-session".to_string(),
            source: RawSource::Jsonl(FIRST_RECORD.to_string()),
        };
        let mut sink = CountingSink::default();

        let outcome = ClaudeAdapter
            .visit(&input, &mut sink)
            .expect("visit plain source");

        assert_eq!(outcome, VisitOutcome::Unvalidated);
        assert_eq!(sink.finishes, 1);
    }

    #[test]
    fn content_capture_maps_text_thinking_tool_use_and_tool_result() {
        use crate::analysis::interface::ContentKind;

        let assistant_record = serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "hello there"},
                    {"type": "thinking", "thinking": "pondering"},
                    {"type": "tool_use", "name": "Bash", "input": {"command": "ls"}},
                ]
            }
        })
        .to_string();
        let tool_result_record = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]
            }
        })
        .to_string();
        let input = SessionInput {
            agent: "claude".to_string(),
            session_id: "content-session".to_string(),
            source: RawSource::Jsonl(format!("{assistant_record}\n{tool_result_record}\n")),
        };
        let mut sink = ContentCapturingSink::default();

        ClaudeAdapter
            .visit(&input, &mut sink)
            .expect("visit content session");

        assert_eq!(sink.contents.len(), 2, "one TurnContent per turn");
        let assistant_parts = &sink.contents[0].parts;
        assert_eq!(assistant_parts.len(), 3);
        assert_eq!(assistant_parts[0].kind, ContentKind::AssistantText);
        assert_eq!(assistant_parts[0].text, "hello there");
        assert_eq!(assistant_parts[1].kind, ContentKind::Thinking);
        assert_eq!(assistant_parts[1].text, "pondering");
        assert_eq!(assistant_parts[2].kind, ContentKind::ToolInput);
        assert_eq!(assistant_parts[2].text, r#"{"command":"ls"}"#);

        let tool_result_parts = &sink.contents[1].parts;
        assert_eq!(tool_result_parts.len(), 1);
        assert_eq!(tool_result_parts[0].kind, ContentKind::ToolResult);
        assert_eq!(tool_result_parts[0].text, "ok");
    }

    #[test]
    fn a_mid_stream_read_failure_omits_the_whole_session() {
        let source = b"{\"type\":\"assistant\",\"message\":{\"id\":\"first\",\"role\":\"assistant\",\"content\":[]}}\n";
        let reader = BufReader::new(DataThenError::new(source));
        let mut collector = SessionCollector::new("claude", "read-failure");
        let result = ClaudeAdapter.visit_reader(reader, &|| false, &mut collector, &HashSet::new());
        assert!(result.is_err());
    }

    struct HeadMutatingSink {
        collector: SessionCollector,
        path: std::path::PathBuf,
        mutated: bool,
    }

    impl HeadMutatingSink {
        fn new(path: &Path) -> Self {
            Self {
                collector: SessionCollector::new("claude", "claimed-session"),
                path: path.to_path_buf(),
                mutated: false,
            }
        }
    }

    impl RecordSink for HeadMutatingSink {
        fn record(&mut self, record: NormalizedRecord) {
            if !self.mutated {
                let mut file = OpenOptions::new()
                    .write(true)
                    .open(&self.path)
                    .expect("open source for mutation");
                file.seek(SeekFrom::Start(0)).expect("seek source head");
                file.write_all(b"[").expect("rewrite source head");
                file.sync_all().expect("sync source mutation");
                self.mutated = true;
            }
            self.collector.record(record);
        }

        fn finish(&mut self, summary: SessionSummary) {
            self.collector.finish(summary);
        }
    }

    #[derive(Default)]
    struct CountingSink {
        finishes: usize,
    }

    impl RecordSink for CountingSink {
        fn record(&mut self, _record: NormalizedRecord) {}

        fn finish(&mut self, _summary: SessionSummary) {
            self.finishes += 1;
        }
    }

    /// Collects every `TurnContent` record a visit emits, in order.
    #[derive(Default)]
    struct ContentCapturingSink {
        contents: Vec<TurnContent>,
    }

    impl RecordSink for ContentCapturingSink {
        fn record(&mut self, record: NormalizedRecord) {
            if let NormalizedRecord::TurnContent(content) = record {
                self.contents.push(*content);
            }
        }

        fn finish(&mut self, _summary: SessionSummary) {}
    }

    struct DataThenError {
        data: Vec<u8>,
        returned_data: bool,
    }

    impl DataThenError {
        fn new(data: &[u8]) -> Self {
            Self {
                data: data.to_vec(),
                returned_data: false,
            }
        }
    }

    #[test]
    fn builtin_commands_do_not_reserve_late_metric_candidates() {
        assert!(is_builtin_command("clear"));
        assert!(is_builtin_command("COMPACT"));
        assert!(is_builtin_command("model"));
        assert!(!is_builtin_command("orbit-tracker"));
    }

    impl Read for DataThenError {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if !self.returned_data {
                let count = output.len().min(self.data.len());
                output[..count].copy_from_slice(&self.data[..count]);
                self.returned_data = true;
                return Ok(count);
            }

            Err(Error::other("synthetic read failure"))
        }
    }

    #[test]
    fn skill_base_name_from_path_takes_dir_or_skill_md_parent() {
        assert_eq!(
            skill_base_name_from_path("/home/avery/.claude/skills/grill-me"),
            Some("grill-me".to_string())
        );
        assert_eq!(
            skill_base_name_from_path("/home/avery/.claude/skills/code-review/SKILL.md"),
            Some("code-review".to_string())
        );
        // Windows separators.
        assert_eq!(
            skill_base_name_from_path("C:\\u\\.claude\\skills\\plan"),
            Some("plan".to_string())
        );
    }

    #[test]
    fn command_names_in_text_extracts_and_strips_slash() {
        let text = "<command-message>code-review</command-message>\n\
                    <command-name>/code-review</command-name>\n\
                    <command-args>changelist</command-args>";
        assert_eq!(command_names_in_text(text), vec!["code-review".to_string()]);
        assert_eq!(command_names_in_text("no tags here"), Vec::<String>::new());
    }

    #[test]
    fn command_skill_name_matches_directly_and_via_plugin_namespace() {
        let names: HashSet<String> = ["frontend-design".to_string(), "code-review".to_string()]
            .into_iter()
            .collect();
        // Direct hit.
        assert_eq!(
            command_skill_name("code-review", &names),
            Some("code-review".to_string())
        );
        // `plugin:skill` resolves to its bare segment.
        assert_eq!(
            command_skill_name("frontend-design:frontend-design", &names),
            Some("frontend-design".to_string())
        );
        // A command that didn't run as a skill is rejected (no base-dir marker).
        assert_eq!(command_skill_name("clear", &names), None);
    }

    #[test]
    fn claude_tool_result_user_record_is_a_tool_event() {
        let result = serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]
            }
        });
        let prompt = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": [{"type": "text", "text": "go"}]}
        });
        assert_eq!(
            parse_record(&result, RecordShape::Claude).unwrap().role,
            crate::analysis::model::Role::Tool
        );
        assert_eq!(
            parse_record(&prompt, RecordShape::Claude).unwrap().role,
            crate::analysis::model::Role::User
        );
    }

    #[test]
    fn message_usage_speed_is_parsed() {
        let record = serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "usage": {"input_tokens": 10, "output_tokens": 5, "speed": "fast"}
            }
        });
        let ev = parse_record(&record, RecordShape::Claude).expect("record should parse");
        assert_eq!(ev.speed.as_deref(), Some("fast"));
    }

    #[test]
    fn top_level_speed_is_parsed_when_usage_carries_none() {
        let record = serde_json::json!({
            "type": "assistant",
            "message": {"role": "assistant", "usage": {"input_tokens": 10}},
            "speed": "standard"
        });
        let ev = parse_record(&record, RecordShape::Claude).expect("record should parse");
        assert_eq!(ev.speed.as_deref(), Some("standard"));
    }

    #[test]
    fn missing_speed_leaves_it_none() {
        let record = serde_json::json!({
            "type": "assistant",
            "message": {"role": "assistant", "usage": {"input_tokens": 10}}
        });
        let ev = parse_record(&record, RecordShape::Claude).expect("record should parse");
        assert_eq!(ev.speed, None);
    }

    #[test]
    fn claude_compact_boundary_record_sets_compaction_flag() {
        let record = serde_json::json!({
            "type": "system",
            "subtype": "compact_boundary",
            "timestamp": "2024-06-01T12:05:00Z",
            "content": "Compacted conversation"
        });

        let ev = parse_record(&record, RecordShape::Claude).expect("compact_boundary should parse");
        assert_eq!(ev.role, crate::analysis::model::Role::System);
        assert!(ev.is_compaction_boundary);
    }

    #[test]
    fn claude_compact_boundary_parses_manual_trigger_and_sizes() {
        let record = serde_json::json!({
            "type": "system",
            "subtype": "compact_boundary",
            "timestamp": "2024-06-01T12:05:00Z",
            "compactMetadata": {
                "trigger": "manual",
                "preTokens": 196_000,
                "postTokens": 11_000,
            }
        });

        let ev = parse_record(&record, RecordShape::Claude).expect("compact_boundary should parse");
        assert_eq!(
            ev.compaction_trigger,
            Some(crate::analysis::model::CompactionTrigger::Manual)
        );
        assert_eq!(ev.compaction_pre_tokens, Some(196_000));
        assert_eq!(ev.compaction_post_tokens, Some(11_000));
    }

    #[test]
    fn claude_compact_boundary_parses_auto_trigger() {
        let record = serde_json::json!({
            "type": "system",
            "subtype": "compact_boundary",
            "timestamp": "2024-06-01T12:05:00Z",
            "compactMetadata": {
                "trigger": "auto",
                "preTokens": 200_000,
                "postTokens": 12_000,
            }
        });

        let ev = parse_record(&record, RecordShape::Claude).expect("compact_boundary should parse");
        assert_eq!(
            ev.compaction_trigger,
            Some(crate::analysis::model::CompactionTrigger::Auto)
        );
    }

    #[test]
    fn claude_compact_boundary_without_metadata_leaves_trigger_and_sizes_none() {
        let record = serde_json::json!({
            "type": "system",
            "subtype": "compact_boundary",
            "timestamp": "2024-06-01T12:05:00Z",
        });

        let ev = parse_record(&record, RecordShape::Claude).expect("compact_boundary should parse");
        assert!(ev.is_compaction_boundary);
        assert_eq!(ev.compaction_trigger, None);
        assert_eq!(ev.compaction_pre_tokens, None);
        assert_eq!(ev.compaction_post_tokens, None);
    }

    #[test]
    fn claude_compact_boundary_without_post_tokens_leaves_it_none() {
        // Some older records omit postTokens entirely.
        let record = serde_json::json!({
            "type": "system",
            "subtype": "compact_boundary",
            "timestamp": "2024-06-01T12:05:00Z",
            "compactMetadata": {
                "trigger": "auto",
                "preTokens": 196_000,
            }
        });

        let ev = parse_record(&record, RecordShape::Claude).expect("compact_boundary should parse");
        assert_eq!(
            ev.compaction_trigger,
            Some(crate::analysis::model::CompactionTrigger::Auto)
        );
        assert_eq!(ev.compaction_pre_tokens, Some(196_000));
        assert_eq!(ev.compaction_post_tokens, None);
    }

    #[test]
    fn unrelated_system_records_do_not_set_compaction_flag() {
        // No subtype at all.
        let plain = serde_json::json!({
            "type": "system",
            "timestamp": "2024-06-01T12:05:00Z",
            "content": "hook ran"
        });
        let ev = parse_record(&plain, RecordShape::Claude).expect("system record should parse");
        assert!(!ev.is_compaction_boundary);

        // A different subtype.
        let other = serde_json::json!({
            "type": "system",
            "subtype": "turn_limit_reached",
            "content": "stop"
        });
        let ev = parse_record(&other, RecordShape::Claude).expect("system record should parse");
        assert!(!ev.is_compaction_boundary);
    }

    /* ------------------------------------------------------------------
     * Fork sub-agent replay skip.
     * ------------------------------------------------------------------ */

    /// Writes `<home>/<project>/<session>/subagents/agent-<id>.jsonl` with
    /// `content` and returns its path. `home` stays alive for the caller.
    fn write_subagent_file(home: &Path, session: &str, id: &str, content: &str) -> PathBuf {
        let subs = home.join("project").join(session).join("subagents");
        std::fs::create_dir_all(&subs).expect("create subagents dir");
        let path = subs.join(format!("agent-{id}.jsonl"));
        std::fs::write(&path, content).expect("write subagent transcript");
        path
    }

    fn write_subagent_meta(path: &Path, meta_json: &str) {
        std::fs::write(path.with_extension("meta.json"), meta_json).expect("write meta.json");
    }

    #[test]
    fn fork_parent_path_uses_the_sibling_agent_when_parent_agent_id_is_present() {
        let path = PathBuf::from("/p/-Users-foo-bar/sess-1/subagents/agent-bbbb.jsonl");
        assert_eq!(
            fork_parent_path(&path, Some("aaaa")).unwrap(),
            PathBuf::from("/p/-Users-foo-bar/sess-1/subagents/agent-aaaa.jsonl"),
        );
    }

    #[test]
    fn fork_parent_path_falls_back_to_the_main_transcript_without_a_parent_agent_id() {
        let path = PathBuf::from("/p/-Users-foo-bar/sess-1/subagents/agent-bbbb.jsonl");
        assert_eq!(
            fork_parent_path(&path, None).unwrap(),
            PathBuf::from("/p/-Users-foo-bar/sess-1.jsonl"),
        );
    }

    #[test]
    fn read_fork_meta_is_none_when_the_sidecar_is_missing() {
        let home = TempDir::new().unwrap();
        let path = write_subagent_file(home.path(), "sess-1", "aaaa", "{}");
        assert!(read_fork_meta(&path).is_none());
    }

    #[test]
    fn read_fork_meta_is_none_for_malformed_json() {
        let home = TempDir::new().unwrap();
        let path = write_subagent_file(home.path(), "sess-1", "aaaa", "{}");
        write_subagent_meta(&path, "not json");
        assert!(read_fork_meta(&path).is_none());
    }

    #[test]
    fn read_fork_meta_parses_is_fork_and_parent_agent_id() {
        let home = TempDir::new().unwrap();
        let path = write_subagent_file(home.path(), "sess-1", "bbbb", "{}");
        write_subagent_meta(
            &path,
            r#"{"agentType":"fork","isFork":true,"parentAgentId":"aaaa","spawnDepth":2,"model":"inherit"}"#,
        );
        let meta = read_fork_meta(&path).expect("meta.json must parse");
        assert!(meta.is_fork);
        assert_eq!(meta.parent_agent_id.as_deref(), Some("aaaa"));
    }

    #[test]
    fn replay_skip_uuids_is_empty_when_the_sidecar_does_not_mark_a_fork() {
        let home = TempDir::new().unwrap();
        let path = write_subagent_file(home.path(), "sess-1", "aaaa", "{}");
        write_subagent_meta(
            &path,
            r#"{"agentType":"general-purpose","toolUseId":"toolu_x","spawnDepth":1,"model":"sonnet"}"#,
        );
        assert!(replay_skip_uuids(&path).is_empty());
    }

    #[test]
    fn replay_skip_uuids_is_empty_when_the_parent_file_is_missing() {
        // `isFork` names a parent agent id with no matching file on disk —
        // read failure and unreadable meta.json fail open the same way, so
        // this stands in for both: nothing is skipped, today's duplicate
        // rows and degraded coverage stay exactly as they were.
        let home = TempDir::new().unwrap();
        let path = write_subagent_file(home.path(), "sess-1", "bbbb", "{}");
        write_subagent_meta(
            &path,
            r#"{"agentType":"fork","isFork":true,"parentAgentId":"missing-parent"}"#,
        );
        assert!(replay_skip_uuids(&path).is_empty());
    }

    #[test]
    fn replay_skip_uuids_collects_the_direct_parents_uuids_with_a_parent_agent_id() {
        let home = TempDir::new().unwrap();
        write_subagent_file(
            home.path(),
            "sess-1",
            "aaaa",
            concat!(
                r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"hi"}}"#,
                "\n",
                r#"{"type":"assistant","uuid":"u2","parentUuid":"u1","message":{"id":"m1","role":"assistant","content":[]}}"#,
                "\n",
            ),
        );
        let fork_path = write_subagent_file(
            home.path(),
            "sess-1",
            "bbbb",
            concat!(
                r#"{"type":"user","uuid":"u1","parentUuid":null,"message":{"role":"user","content":"hi"}}"#,
                "\n",
                r#"{"type":"assistant","uuid":"u3","parentUuid":"u1","message":{"id":"m2","role":"assistant","content":[]}}"#,
                "\n",
            ),
        );
        write_subagent_meta(
            &fork_path,
            r#"{"agentType":"fork","isFork":true,"parentAgentId":"aaaa"}"#,
        );
        let skip = replay_skip_uuids(&fork_path);
        assert_eq!(skip, HashSet::from(["u1".to_string(), "u2".to_string()]));
    }

    #[test]
    fn replay_skip_uuids_collects_the_main_transcripts_uuids_without_a_parent_agent_id() {
        let home = TempDir::new().unwrap();
        let project = home.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("sess-1.jsonl"),
            r#"{"type":"user","uuid":"root-1","parentUuid":null,"message":{"role":"user","content":"hi"}}"#,
        )
        .unwrap();
        let fork_path = write_subagent_file(
            home.path(),
            "sess-1",
            "bbbb",
            r#"{"type":"user","uuid":"root-1","parentUuid":null,"message":{"role":"user","content":"hi"}}"#,
        );
        write_subagent_meta(&fork_path, r#"{"agentType":"fork","isFork":true}"#);
        let skip = replay_skip_uuids(&fork_path);
        assert_eq!(skip, HashSet::from(["root-1".to_string()]));
    }

    #[test]
    fn visit_reader_skips_records_whose_uuid_is_in_the_replay_set() {
        let source = concat!(
            r#"{"type":"assistant","uuid":"replayed-1","timestamp":"2024-06-01T12:00:00Z","message":{"id":"m1","role":"assistant","model":"claude-sonnet-4-6","usage":{"input_tokens":10,"output_tokens":5},"content":[{"type":"text","text":"replayed"}]}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"new-1","timestamp":"2024-06-01T12:00:05Z","message":{"id":"m2","role":"assistant","model":"claude-sonnet-4-6","usage":{"input_tokens":4,"output_tokens":2},"content":[{"type":"text","text":"new"}]}}"#,
            "\n",
        );
        let reader = BufReader::new(source.as_bytes());
        let mut collector = SessionCollector::new("claude", "fork-child");
        let replayed = HashSet::from(["replayed-1".to_string()]);
        let summary = ClaudeAdapter
            .visit_reader(reader, &|| false, &mut collector, &replayed)
            .expect("read must succeed");
        collector.finish(summary);
        let session = collector.into_session().expect("session must build");
        let uuids: Vec<Option<String>> = session
            .events
            .iter()
            .map(|event| event.uuid.clone())
            .collect();
        assert_eq!(uuids, vec![Some("new-1".to_string())]);
    }
}
