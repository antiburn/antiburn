use super::*;
use crate::analysis::jev_evidence::{
    ContentEventReference, JevOperationMetadata, JevOperationState,
};

fn action(id: &str, kind: &str, tool: &str, text: &str) -> ContentAction {
    ContentAction {
        reference: ContentEventReference {
            id: id.into(),
            source_key_digest: "source".into(),
            thread_digest: "thread".into(),
            turn_index: 1,
            native_record_id: Some(id.into()),
            part_index: 0,
            stable: true,
        },
        timestamp_ms: None,
        turn_role: "assistant".into(),
        turn_scope: "main".into(),
        authority: "assistant".into(),
        kind: kind.into(),
        text: text.into(),
        tool_name: Some(tool.into()),
        tool_call_id: Some("call".into()),
        normalized_fields: None,
        metadata: JevOperationMetadata::default(),
        truncated: false,
        context_only: false,
    }
}

#[test]
fn request_ranges_keep_native_values_and_unknown_semantics() {
    for (source, tool, expected) in [
        (SourceFormat::ClaudeJsonl, "Read", JevReadUnit::Lines),
        (SourceFormat::PiV3Jsonl, "read", JevReadUnit::Lines),
        (SourceFormat::OpenCodeSqliteV2, "read", JevReadUnit::Lines),
        (
            SourceFormat::CodexRolloutJsonl,
            "read_file",
            JevReadUnit::Unknown,
        ),
        (
            SourceFormat::OpenCodeSqliteV2,
            "mcp__custom__read",
            JevReadUnit::Unknown,
        ),
    ] {
        let mut actions = [action(
            "request",
            "tool_input",
            tool,
            r#"{"arguments":{"path":"./a.rs","offset":4,"limit":8,"startLine":3,"endLine":11,"byte_offset":12,"byte_limit":16}}"#,
        )];
        normalize_read_evidence(source, &mut actions);
        let request = actions[0].metadata.read_request.as_ref().unwrap();
        assert_eq!(request.paths, ["./a.rs"]);
        assert_eq!(request.extent.unit, expected);
        assert_eq!(request.extent.offset, Some(4));
        assert_eq!(request.native_extent["startLine"], 3);
        assert_eq!(request.native_extent["byte_limit"], 16);
        assert_eq!(request.cwd, None);
    }
    let mut actions = [action(
        "invalid",
        "tool_input",
        "Read",
        r#"{"file_path":"a.rs","offset":-1,"limit":"20"}"#,
    )];
    normalize_read_evidence(SourceFormat::ClaudeJsonl, &mut actions);
    let request = actions[0].metadata.read_request.as_ref().unwrap();
    assert_eq!(request.extent.offset, None);
    assert_eq!(request.extent.limit, None);
    assert_eq!(request.native_extent["offset"], -1);
    assert_eq!(request.native_extent["limit"], "20");
}

#[test]
fn result_matching_rejects_cross_thread_source_and_duplicate_call_ids() {
    let request = action("request", "tool_input", "Read", r#"{"file_path":"a.rs"}"#);
    let result = action("result", "tool_result", "Read", "é");
    for mismatch in [
        "thread",
        "source",
        "tool",
        "call",
        "duplicate",
        "later_duplicate",
        "nonread_duplicate",
        "order",
    ] {
        let mut input = request.clone();
        let mut output = result.clone();
        match mismatch {
            "thread" => output.reference.thread_digest = "other".into(),
            "source" => output.reference.source_key_digest = "other".into(),
            "tool" => output.tool_name = Some("read_file".into()),
            "call" => output.tool_call_id = None,
            _ => {}
        }
        input.metadata.state = JevOperationState::Completed;
        let mut actions = if mismatch == "duplicate" {
            vec![input.clone(), input, output]
        } else if mismatch == "later_duplicate" {
            vec![input.clone(), output, input]
        } else if mismatch == "nonread_duplicate" {
            let mut other = input.clone();
            other.tool_name = Some("Bash".into());
            vec![input, output, other]
        } else if mismatch == "order" {
            vec![output, input]
        } else {
            vec![input, output]
        };
        normalize_read_evidence(SourceFormat::ClaudeJsonl, &mut actions);
        let result = actions
            .iter()
            .find_map(|a| a.metadata.read_result.as_ref())
            .unwrap();
        assert_eq!(result.request_reference_id, None, "{mismatch}");
        assert_eq!(result.recorded_output_bytes, 2);
        assert_eq!(result.returned_extent, None);
        assert_eq!(result.status, JevReadStatus::Unknown);
    }
}

#[test]
fn unqualified_native_metadata_and_nonread_facts_are_discarded() {
    let mut request = action("request", "tool_input", "Read", r#"{"file_path":"a.rs"}"#);
    let mut output = action("output", "tool_result", "Read", "observed");
    let forged = JevReadResult {
        reference_id: "injected".into(),
        request_reference_id: Some("injected-request".into()),
        status: JevReadStatus::Success,
        kind: JevReadResultKind::File,
        recorded_output_bytes: 8,
        recorded_output_digest: crate::checks::ignored_instructions::sha256_hex(b"observed"),
        returned_extent: Some(JevReadExtent {
            unit: JevReadUnit::Lines,
            offset: Some(1),
            limit: Some(1),
            end_inclusive: Some(1),
        }),
        truncated: false,
        recorded_file_version: Some("injected-version".into()),
        extent_contract: Some(CLAUDE_NATIVE_CONTRACT.into()),
    };
    output.metadata.state = JevOperationState::Completed;
    output.metadata.read_result = Some(forged.clone());
    normalize_read_evidence(
        SourceFormat::ClaudeJsonl,
        std::slice::from_mut(&mut request),
    );
    let mut nonread = action("nonread", "tool_result", "Bash", "observed");
    nonread.metadata.read_request = request.metadata.read_request.clone();
    nonread.metadata.read_result = Some(forged.clone());
    let mut unnamed = nonread.clone();
    unnamed.tool_name = None;
    let mut actions = [request, output, nonread, unnamed];
    normalize_read_evidence(SourceFormat::ClaudeJsonl, &mut actions);
    let result = actions[1].metadata.read_result.as_ref().unwrap();
    assert_eq!(result.status, JevReadStatus::Unknown);
    assert_eq!(result.returned_extent, None);
    assert_eq!(result.extent_contract, None);
    assert_eq!(result.recorded_file_version, None);
    assert_eq!(result.request_reference_id.as_deref(), Some("request"));
    for action in &actions[2..] {
        assert!(action.metadata.read_request.is_none());
        assert!(action.metadata.read_result.is_none());
    }
}

#[test]
fn result_extent_requires_the_accepted_producer_shape() {
    let complete = "<path>/project/a.rs</path>\n<type>file</type>\n<content>\n\n3: é\n4: four\n\n(End of file - total 4 lines)\n</content>";
    for (source, text, truncated) in [
        (SourceFormat::ClaudeJsonl, complete.to_owned(), false),
        (SourceFormat::PiV3Jsonl, complete.to_owned(), false),
        (
            SourceFormat::OpenCodeSqliteV2,
            complete.replace("4: four", "6: six"),
            false,
        ),
        (
            SourceFormat::OpenCodeSqliteV2,
            complete.replace("total 4", "total 400"),
            false,
        ),
        (SourceFormat::OpenCodeSqliteV2, complete.to_owned(), true),
    ] {
        let mut result = action("result", "tool_result", "read", &text);
        result.truncated = truncated;
        let mut actions = [result];
        normalize_read_evidence(source, &mut actions);
        assert_eq!(
            actions[0]
                .metadata
                .read_result
                .as_ref()
                .unwrap()
                .returned_extent,
            None
        );
    }
    let mut actions = [action(
        "empty",
        "tool_result",
        "read",
        "<path>/project/empty</path>\n<type>file</type>\n<content>\n\n\n(End of file - total 0 lines)\n</content>",
    )];
    normalize_read_evidence(SourceFormat::OpenCodeSqliteV2, &mut actions);
    let result = actions[0].metadata.read_result.as_ref().unwrap();
    assert_eq!(result.kind, JevReadResultKind::File);
    assert_eq!(result.returned_extent, None);
    assert_eq!(result.status, JevReadStatus::Success);
}

#[test]
fn selected_read_metadata_changes_evidence_identity_only_when_opted_in() {
    use crate::analysis::jev::JevInputSelection;
    use crate::analysis::jev_evidence::{SessionContentEvidence, select_session_content};
    let mut actions = vec![action(
        "request",
        "tool_input",
        "read",
        r#"{"filePath":"a.rs","offset":1,"limit":2}"#,
    )];
    normalize_read_evidence(SourceFormat::OpenCodeSqliteV2, &mut actions);
    let mut evidence = SessionContentEvidence {
        session_identity_digest: "session".into(),
        source_format: SourceFormat::OpenCodeSqliteV2,
        publication_fence: 1,
        selected_input_digest: String::new(),
        actions,
        instructions: vec![],
        complete: true,
        limitations: vec![],
        excluded_thinking_parts: 0,
        field_availability: vec![],
    };
    let selection = JevInputSelection::from_fields(&[JevInputField::ReadFileRequest]);
    let before = select_session_content(&evidence, selection);
    let store =
        crate::analysis::jev_evidence::selected_evidence_store(&before.actions, selection, 1)
            .unwrap();
    let retrieved: JevReadRequest = serde_json::from_str(
        store
            .get("request", JevInputField::ReadFileRequest)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        Some(&retrieved),
        before.actions[0].metadata.read_request.as_ref()
    );
    let legacy = select_session_content(
        &evidence,
        crate::checks::ignored_instructions::INPUT_SELECTION,
    );
    evidence.actions[0]
        .metadata
        .read_request
        .as_mut()
        .unwrap()
        .extent
        .limit = Some(10);
    assert_ne!(
        before.selected_input_digest,
        select_session_content(&evidence, selection).selected_input_digest
    );
    assert_eq!(
        legacy,
        select_session_content(
            &evidence,
            crate::checks::ignored_instructions::INPUT_SELECTION
        )
    );
}

#[test]
fn changed_output_digests_do_not_invent_file_versions() {
    let mut actions = [action("result", "tool_result", "Read", "é")];
    normalize_read_evidence(SourceFormat::ClaudeJsonl, &mut actions);
    let first = actions[0].metadata.read_result.clone().unwrap();
    actions[0].text = "xx".into();
    normalize_read_evidence(SourceFormat::ClaudeJsonl, &mut actions);
    let second = actions[0].metadata.read_result.as_ref().unwrap();
    assert_eq!(first.recorded_output_bytes, second.recorded_output_bytes);
    assert_ne!(first.recorded_output_digest, second.recorded_output_digest);
    assert_eq!(first.recorded_file_version, None);
    assert_eq!(second.recorded_file_version, None);
}
