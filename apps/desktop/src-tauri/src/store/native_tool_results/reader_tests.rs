use antiburn_checks::source::{FileEventSource, PublicationCursor, SourceEvent};
use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, RawSource, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SourceCapabilities, SourceKind, reader_for,
};

use crate::published_evidence_reader::StorePublishedEvidenceReader;

use super::*;

fn parsed_pi_result() -> ToolResultObservation {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/pi_characterization/core_linear_v3.jsonl"
    ))
    .replace("synthetic", "session-1");
    let input = SessionInput {
        agent: "pi".into(),
        session_id: "session-1".into(),
        source: RawSource::Jsonl(source),
        source_format: SourceFormat::PiV3Jsonl,
        fork_parent_session_id: None,
    };
    let mut sink = CompositeSink::new(
        SessionMetricsAccumulator::new("pi", "session-1"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "pi".into(),
            session_id: "session-1".into(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::pi(),
        }),
    );
    reader_for("pi").visit(&input, &mut sink).unwrap();
    sink.tool_result_observations()[0].clone()
}

#[test]
fn production_reader_uses_serialized_prefix_and_keeps_it_frozen_on_correction() {
    let store = store();
    let mut session = session();
    session.key.agent = "pi".into();
    let (record, claim) = claim_and_record_for(&store, session);
    let first = parsed_pi_result();
    let context = first.context.as_ref().unwrap();
    assert!(context.main_session && context.complete_through_event);
    assert_eq!(context.user_messages.len(), 1);
    assert!(publish(
        &store,
        &record,
        &claim,
        &batch_with_format(SourceFormat::PiV3Jsonl, vec![first.clone()], true,)
    ));
    let source = FileEventSource::new(StorePublishedEvidenceReader::new(&store), 20).unwrap();
    let page = source.read_batch(PublicationCursor(0)).unwrap();
    let SourceEvent::Observation {
        observation: original,
        ..
    } = &page.events[0]
    else {
        panic!("expected a result");
    };
    assert!(original.complete);
    assert_eq!(
        original.context.value["session"]["requests"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        original.context.value["session"]["requests"][0]["text"],
        "Review the small function. Keep billing unchanged."
    );

    let mut correction = first.clone();
    correction.revision += 1;
    correction.position = ToolResultPosition::Jsonl {
        record_index: 20,
        item_index: 0,
    };
    correction.content = Some("corrected result".into());
    let mut late = context.user_messages[0].clone();
    late.message_id = "later-request".into();
    late.parts[0].text = "A later request must not enter the earlier prefix.".into();
    correction
        .context
        .as_mut()
        .unwrap()
        .user_messages
        .push(late);
    let (next, claim) = next_claim(&store, &record.key);
    assert!(publish(
        &store,
        &next,
        &claim,
        &batch_with_format(
            SourceFormat::PiV3Jsonl,
            vec![first, correction.clone()],
            true,
        )
    ));
    let corrected_page = source.read_batch(page.next).unwrap();
    assert_eq!(corrected_page.events.len(), 1);
    let SourceEvent::Observation {
        observation: corrected,
        ..
    } = &corrected_page.events[0]
    else {
        panic!("expected a correction");
    };
    assert_eq!(original.context, corrected.context);
    assert_eq!(original.native_order, corrected.native_order);

    let (next, claim) = next_claim(&store, &record.key);
    assert!(publish(
        &store,
        &next,
        &claim,
        &batch_with_format(SourceFormat::PiV3Jsonl, vec![correction], true,)
    ));
    assert!(
        source
            .read_batch(corrected_page.next)
            .unwrap()
            .events
            .is_empty()
    );
}

#[test]
fn production_reader_rejects_missing_context_and_child_scope_as_unsafe_updates() {
    for child in [false, true] {
        let store = store();
        let (record, claim) = claim_and_record(&store);
        let mut result = result("output", 1, ToolResultChange::Update);
        if child {
            let mut context = parsed_pi_result().context.unwrap();
            context.main_session = false;
            result.context = Some(context);
        }
        assert!(publish(&store, &record, &claim, &batch(vec![result], true)));
        let source = FileEventSource::new(StorePublishedEvidenceReader::new(&store), 20).unwrap();
        let page = source.read_batch(PublicationCursor(0)).unwrap();
        assert_eq!(page.events.len(), 1);
        let SourceEvent::UnsafeUpdate {
            cursor,
            branch,
            reason,
        } = &page.events[0]
        else {
            panic!("unsafe source admission must revoke prior evidence");
        };
        assert_eq!(*cursor, PublicationCursor(1));
        assert_eq!(
            branch.0,
            SessionKey::new("native", "opencode", "session-1").to_identity_json()
        );
        assert!(!reason.is_empty());
        assert_eq!(page.next, *cursor);
        assert!(source.read_batch(page.next).unwrap().events.is_empty());
    }
}

#[test]
fn production_reader_filters_project_and_environment_before_page_limit() {
    let store = store();
    for (id, environment, project) in [
        ("other-project", "native", "/home/test/other"),
        ("other-environment", "remote", "/home/test/repo"),
        ("session-1", "native", "/home/test/repo"),
        ("other-project-after", "native", "/home/test/other"),
        ("session-2", "native", "/home/test/repo"),
    ] {
        let mut session = session();
        session.key.session_id = id.into();
        session.key.environment_key = environment.into();
        session.cwd = Some(project.into());
        let (record, claim) = claim_and_record_for(&store, session);
        let mut result = result("output", 1, ToolResultChange::Update);
        result.session_id = id.into();
        result.context = Some(antiburn_local::analysis::ToolResultContext {
            result_position: result.position,
            main_session: true,
            complete_through_event: false,
            user_messages: vec![],
        });
        assert!(publish(&store, &record, &claim, &batch(vec![result], true)));
    }
    let source = FileEventSource::new(
        StorePublishedEvidenceReader::for_project(&store, "native", "/home/test/repo"),
        1,
    )
    .unwrap();
    let page = source.read_batch(PublicationCursor(0)).unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.next, PublicationCursor(3));
    let SourceEvent::Observation { observation, .. } = &page.events[0] else {
        panic!("expected the matching project result");
    };
    assert_eq!(
        observation.branch.0,
        SessionKey::new("native", "opencode", "session-1").to_identity_json()
    );
    assert_eq!(
        observation.context.value["session"]["requests_complete"],
        false
    );
    let second = source.read_batch(page.next).unwrap();
    assert_eq!(second.events.len(), 1);
    assert_eq!(second.next, PublicationCursor(5));
    let SourceEvent::Observation { observation, .. } = &second.events[0] else {
        panic!("expected the next matching project result");
    };
    assert_eq!(
        observation.branch.0,
        SessionKey::new("native", "opencode", "session-2").to_identity_json()
    );
    let end = source.read_batch(second.next).unwrap();
    assert!(end.events.is_empty());
    assert_eq!(end.next, second.next);
    assert!(!end.has_more);
}

#[test]
fn production_reader_keeps_delimiters_and_escaped_session_identity_parts_distinct() {
    let store = store();
    let keys = [
        SessionKey::new("native/opencode/part", "opencode", "end"),
        SessionKey::new("native", "opencode", "part/opencode/end"),
        SessionKey::new("ssh:quoted\"host", "opencode", "back\\slash/雪"),
    ];
    for key in &keys {
        let mut session = session();
        session.key = key.clone();
        let (record, claim) = claim_and_record_for(&store, session);
        let mut result = result("output", 1, ToolResultChange::Update);
        result.session_id = key.session_id.clone();
        result.context = Some(antiburn_local::analysis::ToolResultContext {
            result_position: result.position,
            main_session: true,
            complete_through_event: false,
            user_messages: vec![],
        });
        assert!(publish(&store, &record, &claim, &batch(vec![result], true)));
    }
    let source = FileEventSource::new(StorePublishedEvidenceReader::new(&store), 20).unwrap();
    let page = source.read_batch(PublicationCursor(0)).unwrap();
    assert_eq!(page.events.len(), keys.len());
    let mut branches = std::collections::BTreeSet::new();
    for (event, key) in page.events.iter().zip(keys) {
        let SourceEvent::Observation { observation, .. } = event else {
            panic!("expected an admitted tool result");
        };
        let parts: (String, String, String) = serde_json::from_str(&observation.branch.0).unwrap();
        assert_eq!(parts, (key.environment_key, key.agent, key.session_id));
        branches.insert(&observation.branch);
    }
    assert_eq!(branches.len(), page.events.len());
}
