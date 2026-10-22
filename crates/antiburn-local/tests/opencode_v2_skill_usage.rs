use antiburn_local::analysis::jev_evidence::JevNativeSkillOrigin;
use antiburn_local::analysis::{
    ContentAuthority, ContentPart, EvidenceObservation, EvidenceSource, EvidenceValue,
    NormalizedRecord, PartialReason, RawSource, RecordSink, SessionEvidenceAccumulator,
    SessionInput, SessionSummary, SourceFormat, SourceKind, TurnFacts, reader_for_input,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};

#[derive(Default)]
struct Capture {
    records: Vec<NormalizedRecord>,
    summary: Option<SessionSummary>,
}

impl RecordSink for Capture {
    fn record(&mut self, record: NormalizedRecord) {
        self.records.push(record);
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.summary = Some(summary);
    }
}

impl Capture {
    fn skill_uses(&self) -> Vec<(&str, bool)> {
        self.records
            .iter()
            .filter_map(|record| match record {
                NormalizedRecord::Observation(observation) => match observation.as_ref() {
                    EvidenceObservation::SkillInjection { name, invoked } => {
                        Some((name.as_str(), *invoked))
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    fn parts(&self) -> impl Iterator<Item = &ContentPart> {
        self.records.iter().flat_map(|record| match record {
            NormalizedRecord::TurnContent(content) => content.parts.as_slice(),
            _ => &[],
        })
    }
}

fn read(mut message: Value) -> (Capture, EvidenceSource) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!(
            "../src/discovery/agents/tests/fixtures/opencode_pinned_mixed.sql"
        ))
        .unwrap();
    connection.execute_batch(
        "INSERT INTO session_v2 (id,project_id,slug,directory,version,time_created,time_updated)
         VALUES ('ses_skill','global','skill','/repo','2.0.32',1000,1000)",
    ).unwrap();
    message["time"] = json!({"created":1000});
    connection
        .execute(
            "INSERT INTO session_message VALUES ('msg_skill','ses_skill',?1,0,1000,1000,?2)",
            params![message["type"].as_str().unwrap(), message.to_string()],
        )
        .unwrap();
    let input = SessionInput {
        agent: "opencode".into(),
        session_id: "ses_skill".into(),
        source: RawSource::Sqlite(path),
        source_format: SourceFormat::OpenCodeSqliteV2,
        fork_parent_session_id: None,
    };
    let reader = reader_for_input(&input);
    let source = EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::Sqlite,
        capabilities: reader.capabilities(&input),
    };
    let mut capture = Capture::default();
    reader.visit(&input, &mut capture).unwrap();
    (capture, source)
}

fn assert_skill_use(
    message: Value,
    origin: JevNativeSkillOrigin,
    id_pointer: &str,
    name_pointer: &str,
) {
    let (capture, source) = read(message);
    assert_eq!(capture.skill_uses(), vec![("review-core", true)]);
    assert!(capture.summary.as_ref().unwrap().coverage_gaps.is_empty());
    let part = capture
        .parts()
        .find(|part| part.metadata.recorded_skill_result.is_some())
        .unwrap();
    assert_eq!(part.text, "Read the code before changing it.");
    assert_eq!(part.authority, ContentAuthority::Unknown);
    assert!(part.metadata.human_text.is_none());
    assert!(part.metadata.user_text_history.is_none());
    assert!(part.metadata.user_answers.is_empty());
    let result = part.metadata.recorded_skill_result.as_ref().unwrap();
    assert!(result.call_id.is_none());
    let proof = result.native_skill.as_ref().unwrap();
    assert_eq!(proof.id, "review-core");
    assert_eq!(proof.display_name.as_deref(), Some("Code review"));
    assert_eq!(proof.origin, origin);
    for (pointer, length) in [
        (id_pointer, "review-core".len()),
        (name_pointer, "Code review".len()),
    ] {
        assert!(part.metadata.bindings.iter().any(|binding| {
            binding.pointer == pointer && binding.start == 0 && binding.end == length
        }));
    }
    let mut evidence = SessionEvidenceAccumulator::new(source);
    for record in &capture.records {
        evidence.observe(record);
    }
    evidence.observe_summary(capture.summary.as_ref().unwrap());
    let evidence = evidence.evidence(&TurnFacts::default());
    let sources = match evidence.context_sources {
        EvidenceValue::Complete(sources)
        | EvidenceValue::Partial {
            observed: sources, ..
        } => sources,
        EvidenceValue::Unsupported => panic!("recorded skill use must remain available"),
    };
    assert_eq!(sources.skills.len(), 1);
    assert!(sources.skills["review-core"].injected);
    assert!(sources.skills["review-core"].invoked);
    assert!(!sources.skills.contains_key("Code review"));
}

#[test]
fn prompt_attachment_counts_exact_skill_id_as_use_without_human_approval() {
    assert_skill_use(
        json!({"type":"user","text":"Review the change","skills":[{
            "id":"review-core","name":"Code review","text":"Read the code before changing it."
        }]}),
        JevNativeSkillOrigin::PromptAttachment { index: 0 },
        "/skills/0/id",
        "/skills/0/name",
    );
}

#[test]
fn skill_message_counts_exact_skill_id_as_use_without_human_approval() {
    assert_skill_use(
        json!({"type":"skill","skill":"review-core","name":"Code review",
            "text":"Read the code before changing it."}),
        JevNativeSkillOrigin::SkillMessage,
        "/skill",
        "/name",
    );
}

#[test]
fn malformed_native_skill_ids_stay_partial_without_skill_use() {
    for id in [
        json!(null),
        json!(false),
        json!(""),
        json!("bad id"),
        json!("path/skill"),
        json!("x".repeat(257)),
    ] {
        for prompt_attachment in [true, false] {
            let message = if prompt_attachment {
                json!({"type":"user","text":"Review the change","skills":[{
                    "id":id,"name":"review-core","text":"Skill content"
                }]})
            } else {
                json!({"type":"skill","skill":id,"name":"review-core","text":"Skill content"})
            };
            let (capture, _) = read(message);
            assert!(capture.skill_uses().is_empty());
            assert!(
                capture
                    .parts()
                    .all(|part| part.metadata.recorded_skill_result.is_none())
            );
            assert!(
                capture
                    .summary
                    .unwrap()
                    .coverage_gaps
                    .contains(&PartialReason::AttributionIncomplete)
            );
        }
    }
}

#[test]
fn malformed_prompt_attachment_does_not_hide_valid_skill_use() {
    let (capture, _) = read(json!({"type":"user","text":"Review the change","skills":[
        {"id":"bad id","name":"review-core","text":"Invalid identity"},
        {"id":"review-core","name":"Code review","text":"Valid skill content"}
    ]}));
    assert_eq!(capture.skill_uses(), vec![("review-core", true)]);
    assert!(
        capture
            .summary
            .as_ref()
            .unwrap()
            .coverage_gaps
            .contains(&PartialReason::AttributionIncomplete)
    );
    let proof = capture
        .parts()
        .find_map(|part| {
            part.metadata
                .recorded_skill_result
                .as_ref()?
                .native_skill
                .as_ref()
        })
        .unwrap();
    assert_eq!(
        proof.origin,
        JevNativeSkillOrigin::PromptAttachment { index: 1 }
    );
}
