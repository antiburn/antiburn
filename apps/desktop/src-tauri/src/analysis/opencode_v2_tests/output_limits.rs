use super::*;
use antiburn_local::analysis::jev_evidence::{
    JevOperationState, prepare_session_content, select_session_content,
};
use antiburn_local::analysis::{JevInputField, JevInputSelection, NativeToolOutcome, ShellState};
use serde_json::json;

#[tokio::test]
async fn unavailable_v2_output_stays_partial_through_desktop_selected_publication() {
    for (kind, body, message) in [
        (
            "assistant",
            "[Old tool result content cleared]",
            json!({
                "time":{"created":5000}, "agent":"build",
                "model":{"id":"claude-sonnet-4-6","providerID":"anthropic"},
                "content":[{"type":"tool","id":"cleared-read","name":"read",
                    "time":{"created":5000,"completed":5001},
                    "state":{"status":"completed","input":{"filePath":"src/lib.rs"},
                        "metadata":{},"content":[{"type":"text","text":"[Old tool result content cleared]"}]}}]
            }),
        ),
        (
            "assistant",
            "Shell command output is no longer available.",
            json!({
                "time":{"created":5000}, "agent":"build",
                "model":{"id":"claude-sonnet-4-6","providerID":"anthropic"},
                "content":[{"type":"tool","id":"unavailable-shell","name":"shell",
                    "time":{"created":5000,"completed":5001},
                    "state":{"status":"completed","input":{"command":"synthetic"},
                        "metadata":{"status":"completed","exit":7,"truncated":false},
                        "content":[{"type":"text","text":"Shell command output is no longer available."}]}}]
            }),
        ),
        (
            "shell",
            "Shell command output is no longer available.",
            json!({
                "time":{"created":5000},"shellID":"sh_unavailable","command":"synthetic",
                "status":"exited","exit":7,
                "output":{"output":"Shell command output is no longer available.",
                    "truncated":false,"cursor":43,"size":43}
            }),
        ),
    ] {
        let (directory, path) = database();
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO session_message (id,session_id,type,seq,data)
             VALUES ('msg_unavailable','ses_parent',?1,10,?2)",
                params![kind, message.to_string()],
            )
            .unwrap();
        let (store, key, _) =
            publish_fixture_with_prompts(directory.path(), path, PublishedEvidence::Ready, &[])
                .await;
        let selection = JevInputSelection::from_fields(&[
            JevInputField::BashCommandOutput,
            JevInputField::ReadFileOutput,
            JevInputField::ReadFileResult,
        ]);
        let page = store
            .published_turn_content_offset_selected(&key, None, &BTreeMap::new(), 0, selection)
            .unwrap()
            .unwrap();
        assert_eq!(page.coverage.stored_truncated_parts, 1);
        let content = select_session_content(
            &prepare_session_content("session", SourceFormat::OpenCodeSqliteV2, page),
            selection,
        );
        let action = content
            .actions
            .iter()
            .find(|action| action.text == body)
            .unwrap();
        assert!(action.truncated);
        assert_eq!(action.metadata.state, JevOperationState::Completed);
        assert!(!content.complete);
        assert!(
            content
                .limitations
                .iter()
                .any(|limit| limit == "truncated_source_content")
        );
        if body.starts_with("Shell") {
            let Some(NativeToolOutcome::Shell { state, exit, .. }) = &action.metadata.outcome
            else {
                panic!("expected a retained shell outcome")
            };
            assert_eq!(*exit, Some(7));
            assert_eq!(
                *state,
                Some(if kind == "shell" {
                    ShellState::Exited
                } else {
                    ShellState::Completed
                }),
            );
        }
    }
}
