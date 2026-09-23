use super::input::{InstructionSnapshot, sha256_hex};
use crate::analysis::SourceFormat;
use crate::analysis::evidence_query::PublishedContent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentEventReference {
    pub id: String,
    pub source_key_digest: String,
    pub thread_digest: String,
    pub turn_index: u64,
    pub native_record_id: Option<String>,
    pub part_index: u32,
    pub stable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentAction {
    pub reference: ContentEventReference,
    pub timestamp_ms: Option<i64>,
    pub turn_role: String,
    pub turn_scope: String,
    pub authority: String,
    pub kind: String,
    pub text: String,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionContentEvidence {
    pub session_identity_digest: String,
    pub source_format: SourceFormat,
    pub publication_fence: i64,
    pub selected_input_digest: String,
    pub actions: Vec<ContentAction>,
    pub instructions: Vec<InstructionSnapshot>,
    pub complete: bool,
    pub limitations: Vec<String>,
    pub excluded_thinking_parts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentReferenceResolution<'a> {
    Found(&'a ContentAction),
    Stale,
}

/// Resolves a citation only against the exact assessment input that created it.
pub fn resolve_content_reference<'a>(
    evidence: &'a SessionContentEvidence,
    publication_fence: i64,
    selected_input_digest: &str,
    reference_id: &str,
) -> ContentReferenceResolution<'a> {
    if evidence.publication_fence != publication_fence
        || evidence.selected_input_digest != selected_input_digest
    {
        return ContentReferenceResolution::Stale;
    }
    evidence
        .actions
        .iter()
        .find(|action| action.reference.id == reference_id)
        .map(ContentReferenceResolution::Found)
        .unwrap_or(ContentReferenceResolution::Stale)
}

/// Converts the same persisted normalized content shape for every supported
/// agent. Thinking is retained locally but excluded from assessment input.
pub fn prepare_session_content(
    session_id: &str,
    source_format: SourceFormat,
    published: PublishedContent,
    instructions: Vec<InstructionSnapshot>,
) -> SessionContentEvidence {
    let mut actions = Vec::new();
    let mut limitations = Vec::new();
    let mut excluded_thinking_parts = 0u32;

    for item in published.parts {
        if item.part.kind.as_str() == "thinking" {
            excluded_thinking_parts = excluded_thinking_parts.saturating_add(1);
            continue;
        }
        let native_record_id = item.uuid.or(item.message_id);
        let stable = item.stable_event_identity && native_record_id.is_some();
        let source_key_digest = sha256_hex(item.source_key.as_bytes());
        let thread_digest = sha256_hex(item.thread_id.as_bytes());
        let identity = format!(
            "{}\0{}\0{}\0{}",
            source_key_digest,
            native_record_id.as_deref().unwrap_or("ordinal"),
            item.turn_index,
            item.part_index
        );
        actions.push(ContentAction {
            reference: ContentEventReference {
                id: sha256_hex(identity.as_bytes()),
                source_key_digest,
                thread_digest,
                turn_index: item.turn_index,
                native_record_id,
                part_index: item.part_index,
                stable,
            },
            timestamp_ms: item.ts_ms,
            turn_role: item.role.to_owned(),
            turn_scope: item.scope,
            authority: item.part.authority.as_str().to_owned(),
            kind: item.part.kind.as_str().to_owned(),
            text: item.part.text,
            tool_name: item.part.tool_name,
            tool_call_id: item.part.tool_call_id,
            truncated: item.part.truncated,
        });
    }

    if published.coverage.parts_capped {
        limitations.push("content_part_limit".to_owned());
    }
    if published.coverage.bytes_capped {
        limitations.push("content_byte_limit".to_owned());
    }
    if published.coverage.oversized_parts > 0 {
        limitations.push("oversized_content_part".to_owned());
    }
    if published.coverage.stored_truncated_parts > 0 || actions.iter().any(|item| item.truncated) {
        limitations.push("truncated_source_content".to_owned());
    }
    if actions.is_empty() {
        limitations.push("no_usable_content_events".to_owned());
    }
    if actions.iter().any(|item| !item.reference.stable) {
        limitations.push("some_events_have_only_snapshot_local_ordinals".to_owned());
    }
    if actions.iter().any(|item| item.authority == "unknown") {
        limitations.push("some_content_authority_is_unknown".to_owned());
    }
    if actions.iter().any(|item| {
        matches!(item.kind.as_str(), "tool_input" | "tool_result")
            && (item.tool_name.is_none() || item.tool_call_id.is_none())
    }) {
        limitations.push("some_tool_parts_lack_native_call_identity".to_owned());
    }
    for instruction in &instructions {
        if instruction.provenance.as_str() == "current_file_comparison" {
            limitations.push("current_instruction_file_not_historical_proof".to_owned());
        }
        if instruction
            .sections
            .iter()
            .any(|section| !section.evaluable)
        {
            limitations.push("instruction_section_exceeds_limit".to_owned());
        }
        if !instruction.limitations.is_empty() {
            limitations.push("instruction_snapshot_has_limits".to_owned());
        }
    }
    limitations.sort();
    limitations.dedup();
    let complete = limitations.is_empty();

    let session_identity_digest = sha256_hex(session_id.as_bytes());
    let mut digest_input = Vec::with_capacity(
        actions
            .iter()
            .map(|action| action.text.len().saturating_add(128))
            .sum::<usize>()
            .saturating_add(instructions.len().saturating_mul(128)),
    );
    digest_input.extend_from_slice(session_identity_digest.as_bytes());
    digest_input.extend_from_slice(
        format!(
            "{source_format:?}:{}:{}:{}",
            crate::analysis::PARSER_REVISION,
            published.publication_fence,
            published.source_generation.unwrap_or_default()
        )
        .as_bytes(),
    );
    for action in &actions {
        digest_input.extend_from_slice(action.reference.id.as_bytes());
        digest_input.extend_from_slice(
            action
                .timestamp_ms
                .unwrap_or_default()
                .to_le_bytes()
                .as_slice(),
        );
        digest_input.extend_from_slice(action.turn_role.as_bytes());
        digest_input.extend_from_slice(action.turn_scope.as_bytes());
        digest_input.extend_from_slice(action.authority.as_bytes());
        digest_input.extend_from_slice(action.kind.as_bytes());
        digest_input.extend_from_slice(action.text.as_bytes());
        digest_input.extend_from_slice(action.tool_name.as_deref().unwrap_or("").as_bytes());
        digest_input.extend_from_slice(action.tool_call_id.as_deref().unwrap_or("").as_bytes());
    }
    for instruction in &instructions {
        digest_input.extend_from_slice(instruction.id.as_bytes());
        digest_input.extend_from_slice(instruction.digest.as_bytes());
        digest_input.extend_from_slice(instruction.provenance.as_str().as_bytes());
        digest_input.extend_from_slice(format!("{:?}", instruction.scope).as_bytes());
    }
    SessionContentEvidence {
        session_identity_digest,
        source_format,
        publication_fence: published.publication_fence,
        selected_input_digest: sha256_hex(&digest_input),
        actions,
        instructions,
        complete,
        limitations,
        excluded_thinking_parts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::evidence_query::{ContentQueryCoverage, PublishedContentPart};
    use crate::analysis::interface::{ContentAuthority, ContentKind, ContentPart};

    #[test]
    fn preparation_is_vendor_neutral_and_excludes_thinking() {
        let content = PublishedContent {
            publication_fence: 1,
            source_generation: None,
            parts: vec![
                PublishedContentPart {
                    source_key: "source".to_owned(),
                    thread_id: "thread".to_owned(),
                    turn_index: 2,
                    role: "assistant",
                    scope: "main".to_owned(),
                    ts_ms: Some(10),
                    uuid: Some("event".to_owned()),
                    message_id: None,
                    part_index: 0,
                    part: ContentPart::new(ContentKind::AssistantText, "hello"),
                    stable_event_identity: true,
                },
                PublishedContentPart {
                    source_key: "source".to_owned(),
                    thread_id: "thread".to_owned(),
                    turn_index: 2,
                    role: "assistant",
                    scope: "main".to_owned(),
                    ts_ms: Some(10),
                    uuid: Some("event".to_owned()),
                    message_id: None,
                    part_index: 1,
                    part: ContentPart::new(ContentKind::Thinking, "private thought"),
                    stable_event_identity: true,
                },
            ],
            coverage: ContentQueryCoverage::default(),
        };
        let result = prepare_session_content(
            "test-session",
            SourceFormat::ClaudeJsonl,
            content,
            Vec::new(),
        );
        assert_eq!(result.actions.len(), 1);
        assert_eq!(
            result.actions[0].authority,
            ContentAuthority::Assistant.as_str()
        );
        assert_eq!(result.excluded_thinking_parts, 1);
        assert!(result.complete);
        assert!(matches!(
            resolve_content_reference(
                &result,
                result.publication_fence,
                &result.selected_input_digest,
                &result.actions[0].reference.id
            ),
            ContentReferenceResolution::Found(_)
        ));
        assert_eq!(
            resolve_content_reference(
                &result,
                result.publication_fence + 1,
                &result.selected_input_digest,
                &result.actions[0].reference.id
            ),
            ContentReferenceResolution::Stale
        );
    }

    #[test]
    fn preparation_preserves_branch_scope_and_marks_ambiguous_or_truncated_parts() {
        let mut ambiguous = ContentPart::new(ContentKind::ToolInput, "run command")
            .with_authority(ContentAuthority::Unknown);
        ambiguous.truncated = true;
        let content = PublishedContent {
            publication_fence: 8,
            source_generation: Some(4),
            parts: vec![PublishedContentPart {
                source_key: "child-source".to_owned(),
                thread_id: "child-thread".to_owned(),
                turn_index: 3,
                role: "assistant",
                scope: "delegated".to_owned(),
                ts_ms: Some(20),
                uuid: Some("child-event".to_owned()),
                message_id: None,
                part_index: 0,
                part: ambiguous,
                stable_event_identity: true,
            }],
            coverage: ContentQueryCoverage {
                stored_truncated_parts: 1,
                ..ContentQueryCoverage::default()
            },
        };

        let result = prepare_session_content(
            "branch-session",
            SourceFormat::ClaudeJsonl,
            content,
            Vec::new(),
        );
        assert_eq!(result.actions.len(), 1);
        assert_eq!(result.actions[0].turn_scope, "delegated");
        assert_eq!(result.actions[0].authority, "unknown");
        assert_eq!(
            result.actions[0].reference.thread_digest,
            sha256_hex(b"child-thread")
        );
        assert!(
            result
                .limitations
                .contains(&"some_content_authority_is_unknown".to_owned())
        );
        assert!(
            result
                .limitations
                .contains(&"some_tool_parts_lack_native_call_identity".to_owned())
        );
        assert!(
            result
                .limitations
                .contains(&"truncated_source_content".to_owned())
        );
        assert!(!result.complete);
    }
}
