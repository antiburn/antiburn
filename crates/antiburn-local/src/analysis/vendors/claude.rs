// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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

use anyhow::Context;
use serde_json::Value;

use super::jsonl::{
    SKILL_BASE_MARKER, collect_skill_base_names_from_text, command_names_in_text,
    command_skill_name, parse_record, record_text,
};
use crate::analysis::framing::{BoundedJsonlReader, FramedRecord, RecordSkip};
use crate::analysis::interface::{
    NormalizedRecord, RawSource, RecordSink, SessionCollector, SessionInput, SessionSummary,
    VendorAdapter, VisitOutcome,
};
use crate::analysis::model::{NormalizedEvent, NormalizedSession, ToolCall, Usage};
use crate::analysis::source_validity::{AppendOnlyGuarantee, PinnedSource, SourceClaim};

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
                    let file = File::open(path)?;
                    self.visit_reader(BufReader::new(file), sink)?
                }
                RawSource::Jsonl(content) => {
                    let suffix: &[u8] = if content.ends_with('\n') { b"" } else { b"\n" };
                    let source = Cursor::new(content.as_bytes()).chain(suffix);
                    self.visit_reader(BufReader::new(source), sink)?
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
}

impl ClaudeAdapter {
    pub fn visit_claimed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        guarantee: AppendOnlyGuarantee,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        (|| -> anyhow::Result<VisitOutcome> {
            let RawSource::File(path) = &input.source else {
                anyhow::bail!("a claimed Claude source must be a file");
            };
            let mut pinned = match PinnedSource::open(path, claim.clone())? {
                Ok(pinned) => pinned,
                Err(reason) => return Ok(VisitOutcome::SourceChanged(reason)),
            };
            let limit = match guarantee {
                AppendOnlyGuarantee::Evidenced => claim.boundary,
                AppendOnlyGuarantee::Absent => u64::MAX,
            };
            let summary = self.visit_reader(BufReader::new(pinned.reader(limit)), sink)?;
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
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<SessionSummary> {
        let mut reader = BoundedJsonlReader::new(reader);
        let mut state = ClaudeStreamState::default();

        // TODO @agent: CH-005 will supply the real cancellation signal
        while let Some(record) = reader.next_record(&|| false) {
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

                    let has_skill_marker = record.contains(SKILL_BASE_MARKER);
                    let has_command_name = record.contains("<command-name>");
                    let text = (has_skill_marker || has_command_name).then(|| record_text(&value));
                    if has_skill_marker {
                        collect_skill_base_names_from_text(
                            text.as_deref().unwrap_or_default(),
                            &mut state.skill_base_names,
                        );
                    }

                    let Some(mut event) = parse_record(&value) else {
                        sink.record(NormalizedRecord::Unusable(
                            crate::analysis::framing::PartialReason::UnrecognizedRecordType,
                        ));
                        continue;
                    };

                    state.observe_model(event.model.as_deref());
                    state.dedup_usage(&mut event);
                    if has_command_name {
                        state.pending_commands.push((
                            state.ordinal,
                            command_names_in_text(text.as_deref().unwrap_or_default()),
                        ));
                    }
                    sink.record(NormalizedRecord::MetricsEvent(Box::new(event)));
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
        SessionSummary {
            cache_write_tokens_available: true,
            context_window: self.context_window,
            model,
            late_tools,
        }
    }
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
    use std::path::Path;

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
            .visit_claimed(&input, &claim, AppendOnlyGuarantee::Evidenced, &mut sink)
            .expect("visit changed source");

        assert_eq!(
            outcome,
            VisitOutcome::SourceChanged(crate::analysis::SourceChangedReason::HeadRegionMismatch)
        );
        assert!(sink.collector.into_session().is_err());
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
    fn a_mid_stream_read_failure_omits_the_whole_session() {
        let source = b"{\"type\":\"assistant\",\"message\":{\"id\":\"first\",\"role\":\"assistant\",\"content\":[]}}\n";
        let reader = BufReader::new(DataThenError::new(source));
        let mut collector = SessionCollector::new("claude", "read-failure");
        let result = ClaudeAdapter.visit_reader(reader, &mut collector);
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
}
