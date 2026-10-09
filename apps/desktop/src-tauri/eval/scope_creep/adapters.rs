use super::fixtures::Case;
use antiburn_local::analysis::jev_evidence::*;
use antiburn_local::analysis::{
    NormalizedRecord, RawSource, RecordSink, SessionInput, SessionSummary, SourceFormat, reader_for,
};

#[derive(Default)]
struct Sink(Vec<JevOperationMetadata>);

impl RecordSink for Sink {
    fn record(&mut self, record: NormalizedRecord) {
        if let NormalizedRecord::TurnContent(content) = record {
            for part in content.parts {
                for answer in part.metadata.user_answers {
                    let mut metadata = JevOperationMetadata::default();
                    metadata.user_answers.push(answer);
                    self.0.push(metadata);
                }
                for plan in part.metadata.plan_references {
                    let mut metadata = JevOperationMetadata::default();
                    metadata.plan_references.push(plan);
                    self.0.push(metadata);
                }
            }
        }
    }
    fn finish(&mut self, _: SessionSummary) {}
}

pub fn cases(cohort: &str, task: &str, work: &str, path: &str) -> Vec<Case> {
    let root = crate::support::capture::repository().join("crates/antiburn-local/tests/fixtures");
    let contracts = [
        (
            "claude",
            SourceFormat::ClaudeJsonl,
            "claude_characterization/scope_records.jsonl",
        ),
        (
            "codex",
            SourceFormat::CodexRolloutJsonl,
            "codex_characterization/scope_records.jsonl",
        ),
        (
            "pi",
            SourceFormat::PiV3Jsonl,
            "pi_characterization/scope_records.jsonl",
        ),
        (
            "opencode",
            SourceFormat::OpenCodeJsonl,
            "opencode_characterization/scope_native.jsonl",
        ),
    ];
    let mut cases = Vec::new();
    for (agent, format, fixture) in contracts {
        let source =
            std::fs::read_to_string(root.join(fixture)).expect("synthetic adapter fixture exists");
        let input = SessionInput {
            agent: agent.into(),
            session_id: "synthetic".into(),
            source_format: format,
            source: RawSource::Jsonl(source),
            fork_parent_session_id: None,
        };
        let mut sink = Sink::default();
        reader_for(agent)
            .visit(&input, &mut sink)
            .expect("production adapter reads pinned fixture");
        assert!(
            !sink.0.is_empty(),
            "adapter {agent} must retain typed scope records"
        );
        for (index, metadata) in sink.0.into_iter().enumerate() {
            if metadata.user_answers.iter().any(|a| a.source.truncated)
                || metadata.plan_references.iter().any(|p| p.source.truncated)
            {
                continue;
            }
            let proposal = metadata.plan_references.first().is_some_and(|plan| {
                plan.status == JevPlanStatus::Proposed
                    && plan.content_status == JevPlanContentStatus::Recorded
                    && plan.text.is_some()
                    && !plan.source.producer_revision.is_empty()
            });
            cases.push(Case {
                id: format!("{cohort}-adapter-{agent}-{index}"), scenario: "adapter".into(),
                expected: if proposal { "finding" } else { "unassessed" }.into(), authority_fixture: !proposal,
                task: format!("{task} The retained question or plan is not approved. Do not implement any additional feature."),
                work: work.into(), path: path.into(), format: if agent == "opencode" { SourceFormat::OpenCodeSqliteV2 } else { format },
                metadata: Some(metadata),
                family: String::new(), events: vec![], judgments: Default::default(),
                source_limit: format!("Production {agent} parser, pinned synthetic {fixture}. Metadata is inserted into a separate synthetic published task. This is adapter-output conformance, not end-to-end store ancestry. OpenCode wrapped JSONL characterizes shared extraction; Jev accepts only OpenCodeSqliteV2. Claude/Codex/Pi question origin stays unknown; plans do not infer approved versions. See docs/session-coverage.md for exact pins."),
            });
        }
    }
    cases
}
