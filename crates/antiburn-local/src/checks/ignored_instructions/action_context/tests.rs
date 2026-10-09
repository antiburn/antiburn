use super::*;
use crate::checks::ignored_instructions::assessment::tests::{event, input};
use crate::checks::ignored_instructions::build_assessment_plan;

fn command_comparison(command: &str) -> (String, CandidateComparison) {
    let text = serde_json::json!({"command": command}).to_string();
    let mut action = event("request", 1, "assistant", "main", &text);
    action.kind = "tool_input".to_owned();
    action.tool_name = Some("Bash".to_owned());
    action.tool_call_id = Some("call".to_owned());
    let plan = build_assessment_plan(input(
        vec![action],
        "Do not pass `SECRET_MARKER` to the formatter.",
    ));
    (text, plan.comparisons[0].clone())
}

#[test]
fn quoted_and_unquoted_here_documents_bind_input_without_copying_the_body() {
    for (marker, expands) in [
        ("'INPUT_END'", false),
        ("\"INPUT_END\"", false),
        ("INPUT_END", true),
    ] {
        let command = format!("format-check --stdin <<{marker}\né SECRET_MARKER\nINPUT_END\n");
        let (source, mut comparison) = command_comparison(&command);
        comparison.action_text_start = source.find("SECRET_MARKER").unwrap();
        comparison.action_text_end = comparison.action_text_start + "SECRET_MARKER".len();
        let context = here_document_context(&command, &source, &comparison).unwrap();
        assert_eq!(context.header, command.lines().next().unwrap());
        assert_eq!(context.delimiter, "INPUT_END");
        assert_eq!(context.body_expands, expands);
        assert!(context.selected_range_intersects_input);
        let saved = serde_json::to_string(&context).unwrap();
        assert!(!saved.contains("SECRET_MARKER"));
        assert!(!saved.contains('é'));
    }
}

#[test]
fn a_command_header_is_not_mislabeled_as_here_document_input() {
    let command = "format-check <<'END'\nSECRET_MARKER\nEND";
    let (source, mut comparison) = command_comparison(command);
    comparison.action_text_end = source.find("\\n").unwrap();
    assert!(
        !here_document_context(command, &source, &comparison)
            .unwrap()
            .selected_range_intersects_input
    );
}

#[test]
fn unsupported_shell_shapes_do_not_get_invented_input_context() {
    for command in [
        "printf '<<END'\nSECRET_MARKER\nEND",
        "format-check <<END\nSECRET_MARKER",
        "first <<A <<B\ntext\nA\nB",
        "first <<END\ntext\nEND\nsecond",
        "first | second <<END\ntext\nEND",
        "first; second <<END\ntext\nEND",
        "first <<$END\ntext\n$END",
        "first <<'END\ntext\nEND",
    ] {
        let (source, comparison) = command_comparison(command);
        assert!(
            here_document_context(command, &source, &comparison).is_none(),
            "{command}"
        );
    }
    let command = "first <<END\ntext\nEND";
    let (source, comparison) = command_comparison(command);
    assert!(here_document_context(command, &format!("{source}{source}"), &comparison).is_none());
}

#[test]
fn tab_stripping_and_crlf_boundaries_are_explicit() {
    let command = "first <<-'END'\n\tSECRET_MARKER\n\tEND\n";
    let (source, comparison) = command_comparison(command);
    assert!(
        here_document_context(command, &source, &comparison)
            .unwrap()
            .selected_range_intersects_input
    );
    let command = "first <<END\r\ntext\r\nEND\r\n";
    let (source, comparison) = command_comparison(command);
    assert!(here_document_context(command, &source, &comparison).is_none());
}

#[test]
fn requested_changes_preserve_both_move_roles_and_deletion() {
    let operations = vec![
        EditPathOperation::Move {
            from: "protected/old.rs".to_owned(),
            to: "src/new.rs".to_owned(),
        },
        EditPathOperation::Delete {
            path: "protected/obsolete.rs".to_owned(),
        },
        EditPathOperation::Add {
            path: "src/added.rs".to_owned(),
        },
        EditPathOperation::Update {
            path: "src/updated.rs".to_owned(),
        },
    ];
    let changes = path_changes(&operations);
    assert_eq!(changes.len(), 5);
    assert_eq!(
        changes[0],
        PathChange {
            path: "protected/old.rs",
            change: RequestedPathChange::MoveFrom
        }
    );
    assert_eq!(
        changes[1],
        PathChange {
            path: "src/new.rs",
            change: RequestedPathChange::MoveTo
        }
    );
    assert_eq!(changes[2].change, RequestedPathChange::Delete);
    assert_eq!(changes[3].change, RequestedPathChange::Create);
    assert_eq!(changes[4].change, RequestedPathChange::Modify);
    assert!(path_changes(&[]).is_empty());
    assert_eq!(
        rule_path_candidates("Never change `protected\\` files."),
        ["protected\\"]
    );
}

#[test]
fn explicit_non_posix_shell_does_not_get_posix_input_context() {
    let command = "format-check <<END\nSECRET_MARKER\nEND";
    let (_, comparison) = command_comparison(command);
    for shell in ["/bin/fish", "powershell", "cmd.exe", "unknown"] {
        let source = serde_json::json!({"command":command, "shell":shell}).to_string();
        assert!(here_document_context(command, &source, &comparison).is_none());
    }
    let source = serde_json::json!({"command":command, "shell":"/bin/bash"}).to_string();
    assert!(here_document_context(command, &source, &comparison).is_some());
}
#[test]
fn recorded_moves_preserve_inside_and_unknown_path_values() {
    for (from, to) in [
        ("protected/a.rs", "src/a.rs"),
        ("src/a.rs", "protected/a.rs"),
        ("protected/a.rs", "protected/b.rs"),
        ("src/a.rs", "src/b.rs"),
        ("protected/a.rs", "$ROOT/a.rs"),
    ] {
        let operations = [EditPathOperation::Move {
            from: from.to_owned(),
            to: to.to_owned(),
        }];
        assert_eq!(
            path_changes(&operations),
            [
                PathChange {
                    path: from,
                    change: RequestedPathChange::MoveFrom
                },
                PathChange {
                    path: to,
                    change: RequestedPathChange::MoveTo
                },
            ]
        );
    }
}
#[test]
fn leading_blank_and_comment_lines_do_not_change_the_recorded_input_recipient() {
    use crate::checks::ignored_instructions::assessment::tests::{event, input};
    use crate::checks::ignored_instructions::build_assessment_plan;
    let command = "# Unicode é🦀\n\n  # recorded context\nproof-check --stdin <<'INPUT_END'\nBOUND_INPUT\nINPUT_END\n";
    let mut action = event("command", 1, "assistant", "main", command);
    action.kind = "tool_input".to_owned();
    action.tool_name = Some("Bash".to_owned());
    let plan = build_assessment_plan(input(
        vec![action],
        "Do not pass BOUND_INPUT to proof-check.",
    ));
    let context = here_document_context(command, command, &plan.comparisons[0]).unwrap();
    assert_eq!(context.header, "proof-check --stdin <<'INPUT_END'");
    assert!(!context.body_expands);
    assert!(context.selected_range_intersects_input);
    let continuation = command.replacen("# Unicode é🦀", "# recorded context \\", 1);
    assert!(here_document_context(&continuation, &continuation, &plan.comparisons[0]).is_none());
}
