use super::*;
use antiburn_local::analysis::{RawSource, SessionInput};
use std::io::Write;

#[test]
fn sdk_cli_live_result_preserves_requests_and_partial_history_in_yaml_source_input() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.jsonl");
    let fixture = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/claude_characterization/sdk_cli_request_slot.jsonl"
    ));
    let (root, suffix) = fixture.split_once('\n').unwrap();
    std::fs::write(&path, format!("{root}\n")).unwrap();
    let input = SessionInput {
        agent: "claude".into(),
        session_id: "synthetic-sdk-session".into(),
        source: RawSource::File(path.clone()),
        source_format: SourceFormat::ClaudeJsonl,
        fork_parent_session_id: None,
    };
    let baseline =
        antiburn_local::analysis::file_live::read_native_file_page(&input, None, &|| false)
            .unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(suffix.as_bytes())
        .unwrap();
    let page = antiburn_local::analysis::file_live::read_native_file_page(
        &input,
        Some(&baseline.cursor),
        &|| false,
    )
    .unwrap();
    assert_eq!(page.results.len(), 1);
    assert!(page.results[0].live_eligible);
    let result = &page.results[0].observation;
    assert_eq!(
        result.admission(),
        antiburn_local::analysis::ToolResultAdmission::Admitted
    );
    assert!(result.facts.is_none());
    let store = store();
    store
        .lock()
        .execute(
            "INSERT INTO native_tool_result_publication (
            outcome, environment_key, agent, session_id, source_format,
            native_identity, native_position_json, native_revision, payload_json,
            complete, source_generation, claim_fence, live_eligible,
            session_incarnation, project_cwd)
         VALUES ('update', 'native', 'claude-code', ?1, ?2, 'sdk-result', ?3,
            1, ?4, 1, 1, 1, 1, 7, '/synthetic')",
            params![
                input.session_id,
                serde_json::to_string(&result.source_format).unwrap(),
                serde_json::to_string(&result.position).unwrap(),
                serde_json::to_string(result).unwrap()
            ],
        )
        .unwrap();
    let sequence = store.lock().last_insert_rowid();
    let converted = publication_event(
        &StorePublishedEvidenceReader::new(&store),
        publication(&store, sequence),
        Some("/synthetic"),
        None,
    )
    .unwrap();
    let source = PublishedEvidenceSource::new(OnePublication(converted), 1).unwrap();
    let batch = source.read_batch(PublicationCursor(0)).unwrap();
    let SourceEvent::Observation { observation, .. } = &batch.events[0] else {
        panic!("expected an admitted SDK result");
    };
    let antiburn_checks::domain::ObservationKind::ToolResult {
        message,
        category,
        status,
    } = &observation.kind
    else {
        panic!("expected a normalized tool result");
    };
    assert_eq!(category, "shell");
    assert_eq!(status, "completed");
    assert_eq!(message.value["tool"]["arguments"]["command"], "true");
    assert_eq!(message.value["tool"]["result"]["content"], "ok");
    assert_eq!(message.value["quality"]["result_content_available"], true);
    assert_eq!(message.value["quality"]["request_history_complete"], false);
    let context = &observation.context.value;
    assert_eq!(context["session"]["requests"][0]["id"], "sdk-request");
    assert_eq!(
        context["session"]["requests"][0]["text"],
        "Use the existing checkout."
    );
    assert_eq!(context["session"]["requests_complete"], false);
    assert_eq!(context["session"]["source_coverage"], "partial");
}
