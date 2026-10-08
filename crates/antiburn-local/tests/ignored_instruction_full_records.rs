use std::collections::{BTreeMap, BTreeSet};

use antiburn_local::analysis::SourceFormat;
use antiburn_local::analysis::ignored_instructions::*;
use antiburn_local::analysis::jev::*;
use serde_json::{Value, json};

#[path = "support/full_inventory_native.rs"]
mod native;
#[path = "support/full_record_schema.rs"]
mod schema;

const INVENTORY: &str = include_str!("fixtures/ignored_instructions/rich_scenarios.tsv");
const DEVELOPMENT: &str =
    include_str!("fixtures/ignored_instructions/development_inventory_evidence.json");
const HELDOUT: &str = include_str!("fixtures/ignored_instructions/heldout_evidence.json");
const SPLIT: &str = include_str!("fixtures/ignored_instructions/rich_scenarios_heldout.txt");
const EXPECTATIONS: &str =
    include_str!("fixtures/ignored_instructions/full_record_expectations.tsv");
type NativeRequest = (&'static str, Value);
type SelectedField = (&'static str, String);

fn format(name: &str) -> SourceFormat {
    match name {
        "ClaudeJsonl" => SourceFormat::ClaudeJsonl,
        "CodexRolloutJsonl" => SourceFormat::CodexRolloutJsonl,
        "OpenCodeSqliteV2" => SourceFormat::OpenCodeSqliteV2,
        "PiV3Jsonl" => SourceFormat::PiV3Jsonl,
        "CursorCliAgentJsonl" => SourceFormat::CursorCliAgentJsonl,
        "AntigravityBrainJsonl" => SourceFormat::AntigravityBrainJsonl,
        other => panic!("unknown source format {other}"),
    }
}

// The reference projection uses authored fields. It does not call the normalizer.
fn expected_event(kind: &str, text: &str) -> (Option<NativeRequest>, Option<SelectedField>) {
    match kind {
        "assistant" => (None, Some(("AssistantMessage", text.into()))),
        "bash" => (
            Some(("bash", json!({"command":text}))),
            Some(("BashCommandInput", text.into())),
        ),
        "bash_encoded" => (
            Some((
                "bash",
                json!({"arguments":json!({"command":text}).to_string()}),
            )),
            Some(("BashCommandInput", text.into())),
        ),
        "edit_path" | "read" | "edit_content" => {
            let path = if kind == "edit_content" {
                "src/App.tsx"
            } else {
                text
            };
            let name = if kind == "read" { "read" } else { "edit" };
            let field = if kind == "read" {
                "ReadFilePath"
            } else {
                "FileEditPath"
            };
            let arguments = if kind == "edit_content" {
                json!({"path":path,"newText":text})
            } else {
                json!({"path":path})
            };
            (
                Some((name, arguments)),
                Some((field, json!({"paths":[path]}).to_string())),
            )
        }
        "edit_paths" => {
            let mut paths = text.lines().collect::<Vec<_>>();
            paths.sort_unstable();
            paths.dedup();
            (
                Some((
                    "MultiEdit",
                    json!({"edits":paths.iter().map(|path| json!({"filePath":path})).collect::<Vec<_>>()}),
                )),
                Some(("FileEditPath", json!({"paths":paths}).to_string())),
            )
        }
        "rename" => {
            let paths = text.lines().collect::<Vec<_>>();
            assert_eq!(paths.len(), 2);
            let operation = json!({"operation":"move","from":paths[0],"to":paths[1]});
            let patch = format!(
                "*** Begin Patch\n*** Update File: {}\n*** Move to: {}\n@@\n-old\n+new\n*** End Patch",
                paths[0], paths[1]
            );
            let mut paths = paths;
            paths.sort_unstable();
            paths.dedup();
            (
                Some(("apply_patch", Value::String(patch))),
                Some((
                    "FileEditPath",
                    json!({"paths":paths,"operations":[operation]}).to_string(),
                )),
            )
        }
        "patch" => {
            let paths = text
                .lines()
                .filter_map(|line| {
                    [
                        "*** Add File: ",
                        "*** Delete File: ",
                        "*** Update File: ",
                        "*** Move to: ",
                    ]
                    .iter()
                    .find_map(|prefix| line.strip_prefix(prefix))
                })
                .collect::<Vec<_>>();
            let operations = text
                .lines()
                .filter_map(|line| {
                    [
                        ("*** Add File: ", "add"),
                        ("*** Delete File: ", "delete"),
                        ("*** Update File: ", "update"),
                    ]
                    .iter()
                    .find_map(|(prefix, operation)| {
                        line.strip_prefix(prefix)
                            .map(|path| json!({"operation":operation,"path":path}))
                    })
                })
                .collect::<Vec<_>>();
            (
                Some(("apply_patch", Value::String(text.into()))),
                Some((
                    "FileEditPath",
                    json!({"paths":paths,"operations":operations}).to_string(),
                )),
            )
        }
        "search" => {
            let arguments: Value = serde_json::from_str(text).unwrap();
            let selected = arguments
                .as_object()
                .unwrap()
                .iter()
                .filter(|(key, _)| {
                    ["pattern", "query", "path", "glob", "include"].contains(&key.as_str())
                })
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<serde_json::Map<_, _>>();
            (
                Some(("grep", arguments)),
                Some(("SearchFilesQuery", Value::Object(selected).to_string())),
            )
        }
        "other" => {
            let arguments: Value = serde_json::from_str(text).unwrap();
            let name = match arguments["tool"].as_str() {
                Some("resetdatabase") => "resetdatabase",
                Some("deploy-helper") => "deploy-helper",
                Some("delete_file") => "delete_file",
                None => "issue_tool",
                Some(other) => panic!("unreviewed authored tool name {other}"),
            };
            // The tool name remains metadata; the authored input retains its own keys.
            (
                Some((name, arguments.clone())),
                Some(("OtherToolInput", arguments.to_string())),
            )
        }
        "malformed_bash" | "malformed_edit" | "malformed_read" => {
            let name = match kind {
                "malformed_bash" => "bash",
                "malformed_edit" => "edit",
                _ => "read",
            };
            (Some((name, serde_json::from_str(text).unwrap())), None)
        }
        "user" | "system" | "unknown_authority" | "missing_tool" | "bash_output"
        | "read_output" | "search_output" | "other_output" => (None, None),
        other => panic!("unreviewed authored field {other}"),
    }
}

fn expected_context(source: &str, kind: &str, text: &str) -> Option<SelectedField> {
    match kind {
        "user" => Some(("UserMessage", text.to_owned())),
        "bash_output"
            if matches!(
                source,
                "PiV3Jsonl" | "OpenCodeSqliteV2" | "CursorCliAgentJsonl"
            ) =>
        {
            Some(("BashCommandOutput", text.to_owned()))
        }
        _ => None,
    }
}

fn expected_request_state(source: &str) -> &'static str {
    match source {
        "PiV3Jsonl" => "Pending",
        "OpenCodeSqliteV2" => "Running",
        "ClaudeJsonl" | "CodexRolloutJsonl" | "CursorCliAgentJsonl" | "AntigravityBrainJsonl" => {
            "Unknown"
        }
        _ => panic!("unreviewed source {source}"),
    }
}

fn unavailable(instruction: &str, fixture: &Value) -> BTreeSet<String> {
    let mut missing = fixture["unavailable"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    let lower = instruction.to_lowercase();
    if lower.contains("approval") || lower.contains("approved") {
        missing.insert("unvalidated_authorization".into());
    }
    if fixture["provenance"] == "current_file_comparison" {
        missing.insert("historical_instruction_activation".into());
    }
    if lower.starts_with("if tests fail")
        || lower.starts_with("if checks fail")
        || lower.contains("tests pass")
        || lower.contains("verification passes")
    {
        missing.insert("independent_execution_outcome".into());
    }
    if instruction == "Rule applies to `src/private/**`."
        || instruction.starts_with("Rule frontmatter scopes")
    {
        missing.insert("binding_rule_body".into());
    }
    if instruction.starts_with("Twenty project rules") {
        missing.insert("concrete_rules".into());
    }
    if fixture["id"] == "candidate-low-overlap" {
        missing.insert("credential_content_or_authoritative_path_policy".into());
    }
    if fixture["id"] == "path-read-wrong-file" {
        missing.insert("dependency_change_trigger_in_excluded_edit_content".into());
    }
    missing
}

fn outcome<'a>(id: &str, authored: &'a str, missing: &BTreeSet<String>) -> (&'a str, String) {
    if id == "command-tool-name-not-intent" {
        return ("finding", "The literal rule permits a delete_file call only for deletion. The authored call lists candidates, so that exception does not apply. This does not prove a deletion occurred.".into());
    }
    if id == "assistant-negation" || id == "projection-user-excluded" {
        return (
            "no_finding",
            "The selected text contains no prohibited success or approval claim.".into(),
        );
    }
    if id == "assistant-false-success-report" || id == "assistant-missing-error-report" {
        return ("conditional", "This per-event fixture has no accepted result/request binding. Raw selected output and the assistant report cannot establish the required execution outcome.".into());
    }
    if authored == "conditional" {
        return (
            "conditional",
            format!(
                "Unresolved source or policy evidence: {}. Conditional does not establish violation or clean eligibility.",
                if missing.is_empty() {
                    "native identity, completion, response validation, or literal policy meaning"
                        .into()
                } else {
                    missing.iter().cloned().collect::<Vec<_>>().join(", ")
                }
            ),
        );
    }
    (authored, "Independent authored-event review: selected requests or scoped reports establish this comparison; execution success, user authorization, and dedicated edit content remain outside the claim.".into())
}

fn lifecycle(operation: &str) -> Value {
    match operation {
        "resume" => {
            json!({"state":"reuse_validated_only","invalidate_on":["model","selection","normalization","projection","chunking","questions","reducer","reference","selected_context","source_fence"],"redispatch_completed":false,"excluded_text_changes":"reuse_semantic_answers_with_fresh_source_fence"})
        }
        "append" => {
            json!({"state":"reconcile_new_selected_evidence","unchanged_items":"reuse","affected_obligations":"invalidate","new_items":"assess"})
        }
        "changed_input" | "stale_progress" | "corrupt_progress" => {
            json!({"state":"invalidate","reuse":false,"clean_from_saved_state":false})
        }
        "worker_fence" => {
            json!({"state":"conditional","requires":"worker generation, enablement, incarnation, branch, deletion, and publication-fence facts","stale_publication":false})
        }
        "outcome_unknown" | "cancelled" => {
            json!({"state":"blocked_unknown","automatic_redispatch":false,"provider_nonbilling":"unavailable"})
        }
        "none" => json!({"state":"not_applicable"}),
        _ => {
            json!({"state":"incomplete_or_invalid_response","clean":false,"validated_checkpoints":"retain_under_exact_identity"})
        }
    }
}

fn finding_targets(id: &str, state: &str) -> Vec<String> {
    if state != "finding" {
        return Vec::new();
    }
    let indices: &[usize] = match id {
        "path-read-wrong-file"
        | "path-search-wrong-query"
        | "prerequisite-negative-exception"
        | "chunk-candidate-falls-at-batch-end"
        | "chunk-candidate-next-batch"
        | "lifecycle-append-new-event" => &[1],
        "candidate-multiple-actions" => &[0, 1],
        _ => &[0],
    };
    indices.iter().map(|index| format!("e{index}")).collect()
}

fn expanded_text(fixture: &Value, index: usize, kind: &str, text: &str) -> String {
    let prefix = fixture["prefix_bytes"].as_u64().unwrap_or(0);
    if index != 0 || prefix == 0 {
        return text.into();
    }
    let prefix = usize::try_from(prefix).unwrap();
    let unit = if kind == "bash" {
        "# Background explanation.\n"
    } else {
        fixture["prefix_text"]
            .as_str()
            .unwrap_or("Background explanation.\n")
    };
    format!("{}{text}", unit.repeat(prefix.div_ceil(unit.len())))
}

fn assert_native_binding(
    action: &ContentAction,
    source: &str,
    request: Option<&(&str, Value)>,
    selected: Option<&(&str, String)>,
) {
    let record_id = match source {
        "ClaudeJsonl" | "PiV3Jsonl" | "OpenCodeSqliteV2" => Some("native-record"),
        "AntigravityBrainJsonl" => Some("1"),
        _ => None,
    };
    assert_eq!(action.reference.native_record_id.as_deref(), record_id);
    assert_eq!(action.reference.part_index, 0);
    assert_eq!(
        action.tool_call_id.as_deref(),
        if (request.is_some() || (action.kind == "tool_result" && source != "CursorCliAgentJsonl"))
            && source != "AntigravityBrainJsonl"
        {
            Some("native-call")
        } else {
            None
        },
        "{source} {} call identity",
        action.kind
    );
    if let (Some((name, arguments)), Some((field, text))) = (request, selected) {
        let fields = action.normalized_fields.as_ref().unwrap();
        let category = match *name {
            "bash" => "bash_command",
            "edit" | "MultiEdit" | "apply_patch" => "file_edit",
            "read" => "read_file",
            "grep" => "search_files",
            _ => "other_tool",
        };
        assert_eq!(fields.category.unwrap().as_str(), category);
        assert!(!fields.malformed);
        assert!(
            fields
                .values
                .iter()
                .any(|(key, value)| format!("{key:?}") == *field && value == text)
        );
        if matches!(*name, "bash" | "edit" | "read" | "grep")
            && arguments.get("arguments").is_none()
        {
            let prefix = match source {
                "ClaudeJsonl" => "/message/content/0/input/",
                "PiV3Jsonl" => "/message/content/0/arguments/",
                "OpenCodeSqliteV2" => "/state/input/",
                "CodexRolloutJsonl" => "/payload/arguments/",
                "CursorCliAgentJsonl" => "/input/",
                "AntigravityBrainJsonl" => "/tool_calls/0/args/",
                _ => unreachable!(),
            };
            for binding in &action.metadata.bindings {
                let key = binding.pointer.strip_prefix(prefix).unwrap();
                let source_text = arguments[key].as_str().unwrap();
                assert_eq!(binding.start, 0);
                assert_eq!(binding.end, source_text.len());
                assert_eq!(
                    source_text.get(binding.start..binding.end),
                    Some(source_text)
                );
            }
            assert!(!action.metadata.bindings.is_empty());
        }
    }
}

fn request_citation_assertions(
    plan: &JevCheckPlan<AssessmentPlan>,
    context: &JevSessionContext,
) -> usize {
    let mut items = plan.work_items.clone();
    for item in &plan.work_items {
        let answers = item
            .questions
            .iter()
            .filter_map(|(key, question)| {
                let JevQuestion::Choice { criteria, .. } = question else {
                    return None;
                };
                criteria.contains_key("conflict").then(|| {
                    (
                        key.clone(),
                        JevAnswer::Choice {
                            choice: "conflict".into(),
                            confidence: 1.0,
                            probabilities: criteria
                                .keys()
                                .map(|option| (option.clone(), f64::from(option == "conflict")))
                                .collect(),
                        },
                    )
                })
            })
            .collect::<BTreeMap<_, _>>();
        if answers.len() == item.questions.len() {
            let result = JevWorkItemResult {
                request_id: "offline-request-shape".into(),
                work_item_id: item.id.clone(),
                answers,
                evidence: item.window.evidence.clone(),
                model: PINNED_MODEL.into(),
                usage: JevUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            };
            if let Some(followup) = IgnoredInstructionsCheck
                .reconcile(item, &result, context)
                .unwrap()
            {
                items.push(followup);
            }
        }
    }
    let packing = pack_work_items(&items);
    assert!(packing.skipped_item_ids.is_empty());
    for batch in &packing.batches {
        assert_eq!(
            validate_jev_request(&batch.request).unwrap(),
            batch.serialized_bytes
        );
        assert!(batch.request.questions.len() <= MAX_QUESTIONS_PER_REQUEST);
        let serialized = serde_json::to_string(&batch.request).unwrap();
        for private in [
            "native-record",
            "native-call",
            "frozen_label",
            "contract_reason",
            "finding_citations",
            "source_pointer",
        ] {
            assert!(
                !serialized.contains(private),
                "local record data reached request: {private}"
            );
        }
        assert!(
            batch
                .work_item_ids
                .iter()
                .all(|id| items.iter().any(|item| &item.id == id))
        );
    }
    packing.batches.len()
}

#[test]
fn full_inventory_has_independent_native_projection_request_and_citation_records() {
    for (source, expected_hash) in [
        (
            INVENTORY,
            "3e7293c81635cc5ee434334d05129f3fb9f72ae0bf3c18436d6a8c7e807b3c09",
        ),
        (
            DEVELOPMENT,
            "ab173586e6580ab980136b32306f303b46e73a41ef3017cf0a9f128d59018349",
        ),
        (
            HELDOUT,
            "01105042c1405068c55f9169b6ac0e527c8c329d781ff9ba336a2a0b9fd50538",
        ),
        (
            SPLIT,
            "fb6782778c48499e85f1d229c085251bf6641ec2407a9ce7940fbb7cda92ee5f",
        ),
    ] {
        assert_eq!(
            sha256_hex(source.as_bytes()),
            expected_hash,
            "frozen source changed"
        );
    }
    let heldout = SPLIT
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect::<BTreeSet<_>>();
    assert_eq!(heldout.len(), 48);
    let inventory = INVENTORY
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let fields = line.split('|').collect::<Vec<_>>();
            (fields[0], fields)
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(inventory.len(), 240);
    let mut expectations = BTreeMap::new();
    for line in EXPECTATIONS
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let (state, case_ids) = line.split_once('|').unwrap();
        assert!(["finding", "no_finding", "pending", "conditional"].contains(&state));
        for id in case_ids.split_whitespace() {
            assert!(
                expectations.insert(id, state).is_none(),
                "duplicate expectation {id}"
            );
        }
    }
    assert_eq!(
        expectations.keys().copied().collect::<BTreeSet<_>>(),
        inventory.keys().copied().collect()
    );
    let mut records = Vec::new();
    let mut ids = BTreeSet::new();
    let mut native_assertions = 0;
    let mut request_assertions = 0;
    let mut contradictions = Vec::new();
    for (source, data) in [
        ("development_inventory_evidence.json", DEVELOPMENT),
        ("heldout_evidence.json", HELDOUT),
    ] {
        let data: Value = serde_json::from_str(data).unwrap();
        for (case_index, fixture) in data["cases"].as_array().unwrap().iter().enumerate() {
            let id = fixture["id"].as_str().unwrap();
            assert!(ids.insert(id.to_owned()));
            let fields = &inventory[id];
            let explicit = fields.len() == 7;
            let source_name = if explicit { fields[3] } else { "ClaudeJsonl" };
            let instruction = fields[if explicit { 4 } else { 3 }];
            let source_format = format(source_name);
            let missing = unavailable(instruction, fixture);
            let (expected, reason) = outcome(id, expectations[id], &missing);
            if expected != fields[2] && !(expected == "conditional" && fields[2] == "unassessed") {
                contradictions.push(json!({"id":id,"frozen_label":fields[2],"independent_state":expected,"reason":reason}));
            }
            let mut projections = Vec::new();
            let mut actions = Vec::new();
            for (event_index, event) in fixture["events"].as_array().unwrap().iter().enumerate() {
                let kind = event[0].as_str().unwrap();
                let expanded =
                    expanded_text(fixture, event_index, kind, event[1].as_str().unwrap());
                let authored = expanded.as_str();
                let (request, selected) = expected_event(kind, authored);
                let selected_context = expected_context(source_name, kind, authored);
                let output_tool = match kind {
                    "bash_output" => Some("bash"),
                    "read_output" => Some("read"),
                    "search_output" => Some("grep"),
                    "other_output" => Some("issue_tool"),
                    _ => None,
                };
                let observable_native = request.is_some()
                    || matches!(kind, "assistant" | "user")
                    || (kind == "system" && source_name != "AntigravityBrainJsonl")
                    || (output_tool.is_some()
                        && (source_name != "AntigravityBrainJsonl" || kind == "bash_output"));
                let mut native_binding = json!({"state":"conditional","reason":"The authored authority or result-only shape has no lossless native envelope in this fixture builder. Source characterization owns that shape."});
                if observable_native {
                    let role = if kind == "system" {
                        "system"
                    } else if kind == "user" {
                        "user"
                    } else {
                        "assistant"
                    };
                    let projected = native::project(
                        source_format,
                        role,
                        request.clone(),
                        authored,
                        output_tool,
                    );
                    let expected_texts = selected
                        .as_ref()
                        .or(selected_context.as_ref())
                        .map(|(_, text)| text.as_str())
                        .into_iter()
                        .collect::<Vec<_>>();
                    let actual_texts = projected
                        .actions
                        .iter()
                        .map(|action| action.text.as_str())
                        .collect::<Vec<_>>();
                    assert_eq!(
                        actual_texts, expected_texts,
                        "{id} e{event_index} {source_name}"
                    );
                    native_assertions += 1;
                    if projected.actions.is_empty() {
                        native_binding = json!({"state":"verified_no_selected_evidence","reason":if kind.starts_with("malformed") {"malformed_known_tool_has_no_other_tool_fallback"} else {"excluded_authority_or_output"}});
                    }
                    for action in &projected.actions {
                        assert_native_binding(
                            action,
                            source_name,
                            request.as_ref(),
                            selected.as_ref(),
                        );
                        assert_eq!(
                            action.authority,
                            match kind {
                                "user" => "user",
                                "bash_output" => "tool",
                                _ => "assistant",
                            },
                            "{id}"
                        );
                        if let Some((field, _)) = selected.as_ref().or(selected_context.as_ref()) {
                            let availability = projected
                                .field_availability
                                .iter()
                                .find(|value| format!("{:?}", value.field) == *field)
                                .unwrap();
                            assert_eq!(availability.observed_parts, 1, "{id} {field}");
                        }
                        let operation_state = if request.is_some() {
                            expected_request_state(source_name)
                        } else if kind == "bash_output" && source_name == "OpenCodeSqliteV2" {
                            "Completed"
                        } else {
                            "Unknown"
                        };
                        native_binding = json!({"state":"observed","native_record_id":action.reference.native_record_id,"part_index":action.reference.part_index,"tool_call_id":action.tool_call_id,"decoded_field_bindings":action.metadata.bindings,"operation_state":operation_state});
                        if request.is_some() {
                            assert_eq!(
                                format!("{:?}", action.metadata.state),
                                expected_request_state(source_name)
                            );
                        }
                        let mut bound = action.clone();
                        bound.reference.id = format!("e{event_index}");
                        bound.reference.turn_index = u64::try_from(event_index).unwrap();
                        bound.reference.thread_digest =
                            if fixture["sibling_first"] == true && event_index == 0 {
                                "sibling"
                            } else {
                                "main"
                            }
                            .into();
                        bound.truncated = fixture["truncated"] == true && event_index == 0;
                        actions.push(bound);
                    }
                }
                projections.push(json!({"event_id":format!("e{event_index}"),"authored_kind":kind,"source_pointer":format!("{source}#/cases/{case_index}/events/{event_index}"),"normalized_request":request.as_ref().map(|(name, input)| json!({"tool":name,"input":input})),"expected_selected":selected.as_ref().map(|(field, text)| json!({"field":field,"text":text})),"availability":if selected.is_some() { "observed" } else if kind.starts_with("malformed") { "malformed" } else if matches!(kind,"system"|"unknown_authority"|"missing_tool") { "unsupported_authority_or_identity" } else { "excluded" },"native_binding":native_binding}));
                let projection = projections.last_mut().unwrap();
                projection["expected_context"] = selected_context
                    .as_ref()
                    .map(|(field, text)| json!({"field": field, "text": text}))
                    .unwrap_or(Value::Null);
                if selected_context.is_some() {
                    projection["availability"] = json!("observed_context");
                }
            }
            for projection in &mut projections {
                let selected = &projection["expected_selected"];
                let (category, field) = match selected["field"].as_str() {
                    Some("BashCommandInput") => (Some("bash_command"), Some("bash_command_input")),
                    Some("FileEditPath") => (Some("file_edit"), Some("file_edit_path")),
                    Some("ReadFilePath") => (Some("read_file"), Some("read_file_path")),
                    Some("SearchFilesQuery") => (Some("search_files"), Some("search_files_query")),
                    Some("OtherToolInput") => (Some("other_tool"), Some("other_tool_input")),
                    _ => (None, None),
                };
                projection["expected_normalized_fields"] = json!({"category":category,"selected_values":field.map(|field|json!({field:selected["text"]})),"malformed":projection["authored_kind"].as_str().unwrap().starts_with("malformed"),"excluded_values":{"state":"not_selected","text":"unavailable_to_check"}});
            }
            let provenance = if fixture["provenance"] == "current_file_comparison" {
                InstructionProvenance::CurrentFileComparison
            } else {
                InstructionProvenance::RecordedInjection
            };
            let scope = if fixture["scope"] == "global" {
                InstructionScope::Global
            } else {
                InstructionScope::Project
            };
            let padding =
                usize::try_from(fixture["instruction_padding_bytes"].as_u64().unwrap_or(0))
                    .unwrap();
            let reference_text = format!(
                "# Evaluation policy\n\n{}\n{instruction}",
                "Background information. ".repeat(padding / 24)
            );
            let instruction_start = reference_text.len() - instruction.len();
            let snapshot =
                snapshot_from_text("AGENTS.md", reference_text.clone(), provenance, scope).unwrap();
            let content = SessionContentEvidence {
                session_identity_digest: id.into(),
                source_format,
                publication_fence: 1,
                selected_input_digest: String::new(),
                actions,
                instructions: vec![snapshot],
                complete: fixture["complete"].as_bool().unwrap_or(true),
                limitations: Vec::new(),
                excluded_thinking_parts: 0,
                field_availability: Vec::new(),
            };
            let context = build_jev_context(&AssessmentInput {
                content: content.clone(),
                prior_history_complete: fixture["prior_history_complete"].as_bool().unwrap_or(true),
                activity_after_ms: None,
                boundary_positions: BTreeMap::new(),
                source_generation: 1,
                source_fingerprint: None,
                incarnation: 1,
                comparison_after: None,
            })
            .unwrap();
            assert!(
                content
                    .actions
                    .iter()
                    .map(|action| action.text.len())
                    .sum::<usize>()
                    <= 262144,
                "{id} selected fixture byte budget"
            );
            let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
            request_assertions += request_citation_assertions(&plan, &context);
            let packing = pack_work_items(&plan.work_items);
            assert!(packing.skipped_item_ids.is_empty(), "{id}");
            for batch in &packing.batches {
                assert!(batch.serialized_bytes <= MAX_REQUEST_BYTES);
                assert!(
                    batch
                        .work_item_ids
                        .iter()
                        .all(|work_id| plan.work_items.iter().any(|item| &item.id == work_id))
                );
                request_assertions += 1;
            }
            for comparison in &plan.prepared.comparisons {
                let action = content
                    .actions
                    .iter()
                    .find(|action| action.reference.id == comparison.reference.action_id)
                    .unwrap();
                assert_eq!(
                    &action.text[comparison.action_text_start..comparison.action_text_end],
                    comparison.action.text,
                    "{id} action citation range"
                );
                assert!(
                    content.instructions.iter().any(|snapshot| snapshot.id
                        == comparison.reference.instruction_id
                        && snapshot.digest == comparison.reference.instruction_digest
                        && snapshot
                            .sections
                            .iter()
                            .any(|section| section.id == comparison.reference.rule_id
                                && section.start_line == comparison.reference.start_line
                                && section.end_line == comparison.reference.end_line)),
                    "{id} rule citation range"
                );
            }
            let required_targets = finding_targets(id, expected);
            for target in &required_targets {
                assert!(
                    plan.prepared
                        .comparisons
                        .iter()
                        .any(|comparison| &comparison.reference.action_id == target),
                    "{id}: candidate selection dropped independently authored target {target}"
                );
                assert!(
                    projections.iter().any(|event| event["event_id"] == *target
                        && event["expected_selected"].is_object()),
                    "{id}: citation target is not selected"
                );
            }
            let candidates = projections.iter().filter(|event| event["expected_selected"].is_object()).map(|event| json!({"event_id":event["event_id"],"rule_source":"AGENTS.md","rule_text":instruction,"eligibility":"selected_possible_candidate","judgment":"independent_semantic_assessment_required","citation_target":event["source_pointer"]})).collect::<Vec<_>>();
            let operation = fixture["operation"].as_str().unwrap_or("none");
            records.push(json!({"id":id,"family":fields[1],"split":if heldout.contains(id) {"HELDOUT"} else {"DEVELOPMENT"},"source_format":source_name,"source_pointer":format!("{source}#/cases/{case_index}"),"instruction":{"text":instruction,"source":"AGENTS.md","scope":if fixture["scope"]=="global" {"global"} else {"project"},"provenance":if fixture["provenance"]=="current_file_comparison" {"current_file_comparison"} else {"synthetic_recorded_injection"},"historical_native_capture":"unavailable_for_accepted_product_sources"},"trigger":{"reference":instruction,"event_ids":candidates.iter().map(|candidate|candidate["event_id"].clone()).collect::<Vec<_>>(),"state":if missing.is_empty() {"selected_evidence"} else {"conditional"}},"exception":{"reference":instruction,"authorization":"excluded_unknown","state":if instruction.contains("except") || instruction.contains("unless") {"evaluate_selected_clause_only"} else {"not_stated"}},"completion_boundary":{"authored":fixture["boundary"],"state":if fixture["boundary"].is_string() {"synthetic_boundary_only"} else {"not_recorded"},"native_final_boundary":"unavailable","complete_interval":fixture["complete"].as_bool().unwrap_or(true),"prior_history_complete":fixture["prior_history_complete"].as_bool().unwrap_or(true)},"events":projections,"expected_candidates":candidates,"expected":{"state":expected,"frozen_label":fields[2],"reason":reason,"finding_citations":fixture["citations"],"native_completion_dependent":"conditional"},"forbidden_claims":["requested_command_proves_success","assistant_report_proves_authorization","edit_path_proves_edit_body","current_file_proves_historical_activation","scope_match_alone_proves_violation","missing_or_excluded_evidence_proves_clean","synthetic_boundary_proves_native_final_response","unknown_dispatch_was_not_billed"],"permitted_limitations":missing,"unavailable_private_labels":{"state":"unavailable","human_confirmed":false},"lifecycle":lifecycle(operation),"stage_budgets":{"native_fixture_bytes":{"state":"bounded","max":65536},"selected_page_bytes":{"state":"bounded","max":262144},"provider_request_bytes":{"state":"bounded","max":MAX_REQUEST_BYTES},"provider_calls":{"state":"forbidden","max":0},"model_quality":{"state":"unmeasured"},"question_count":{"state":"conditional","reason":"Depends on eligible segmentation and strategy; no inferred fixed question count."}},"tested_stages":["native_adapter","sink","fenced_query","selected_projection","request_packing","local_citation_ownership"],"conditional_stages":["model_judgment","native_completion","worker_publication","private_session_quality"]}));
            let record = records.last_mut().unwrap();
            record["instruction"]["snapshot_text"] = json!(reference_text);
            record["instruction"]["expected_authored_rule_range"] = json!({"container":"decoded_reference_text","start":instruction_start,"end":instruction_start+instruction.len(),"text":instruction});
            let native_events = record["events"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|event| event["native_binding"]["state"] != "conditional")
                .count();
            record["stage_verification"] = json!({"native_adapter":{"state":if native_events==0 {"conditional_no_supported_envelope"} else if native_events == record["events"].as_array().unwrap().len() {"verified"} else {"partial_supported_envelopes"},"event_count":native_events},"request_packing":{"state":if packing.batches.is_empty() {"no_eligible_work"} else {"verified"}},"semantic_outcome":{"state":"independently_authored_not_model_measured"},"publication":{"state":"conditional_not_executed"}});
            record["expected"]["finding_citations"] = json!(required_targets);
            record["unavailable_evidence"] = json!(missing.iter().map(|evidence|json!({"evidence":evidence,"state":"conditional_unavailable","cannot_resolve":true})).collect::<Vec<_>>());
            record["lifecycle"]["authored_operation"] = json!(operation);
            record["source_contract"] = json!({"producer_release_range":{"state":"unavailable"},"shape_pin":match source_name {"ClaudeJsonl"=>"message.content text/tool_use/tool_result blocks with uuid and exact tool_use_id", "CodexRolloutJsonl"=>"session_meta plus response_item message/function_call/function_call_output with exact call_id", "OpenCodeSqliteV2"=>"session/message/part tables with JSON data, tool state running input or completed output", "PiV3Jsonl"=>"session version 3 plus message content text/toolCall and toolResult role with exact toolCallId", "CursorCliAgentJsonl"=>"cursor_source agent_transcript header and role/message.content tool-use/tool-result blocks", "AntigravityBrainJsonl"=>"PLANNER_RESPONSE content/tool_calls.args and USER_INPUT.userInput.userResponse; RUN_COMMAND content for command result", _=>unreachable!()},"contract_references":["docs/session-coverage.md","docs/smart-burn-checks.md"],"scope":"synthetic native companion only; uncharacterized envelopes, real private labels, historical native policy, and native completion remain unavailable"});
            record["stage_budgets"]["question_count"] = json!({"state":"bounded_per_request","max":MAX_QUESTIONS_PER_REQUEST,"total_plan_count":"conditional_on_eligible_rules_and_strategy"});
            record["trigger"]["condition_clause"] = if instruction.to_lowercase().starts_with("if ")
            {
                json!({"text":instruction.split_once(',').map(|(condition,_)|condition).unwrap_or(instruction),"state":"conditional_on_selected_evidence"})
            } else {
                json!({"state":"not_stated"})
            };
            record["exception"]["clause"] = ["except ","unless "].iter().find_map(|marker|instruction.find(marker).map(|start|json!({"text":&instruction[start..],"start":start,"end":instruction.len(),"state":"conditional_if_authorization_or_result_required"}))).unwrap_or_else(||json!({"state":"not_stated"}));
            record["completion_boundary"]["event_id"] = match fixture["boundary"].as_str() {
                Some("first_response" | "operation_start") => json!("e0"),
                Some("final_response") => fixture["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, event)| event[0] == "assistant")
                    .map(|(index, _)| json!(format!("e{index}")))
                    .unwrap_or(Value::Null),
                _ => Value::Null,
            };
            record["authored_transforms"] = json!({"prefix_bytes":fixture["prefix_bytes"],"prefix_text":fixture["prefix_text"],"instruction_padding_bytes":fixture["instruction_padding_bytes"],"filler_events":fixture["filler_events"],"page_cut":fixture["page_cut"],"sibling_first":fixture["sibling_first"],"native_pagination_and_branch_reproduction":"conditional_not_executed_by_per_event_companions"});
        }
    }
    assert_eq!(ids, inventory.keys().map(|id| (*id).to_owned()).collect());
    assert_eq!(records.len(), 240);
    assert_eq!(
        records
            .iter()
            .filter(|record| record["split"] == "HELDOUT")
            .count(),
        48
    );
    assert!(native_assertions >= 240);
    assert!(request_assertions > 100);
    let artifact = json!({"schema_version":3,"record_count":240,"policy":"Independent source-field projection and authored-event policy. No current engine or model output defines expected text. Frozen evidence and labels are immutable. Conditional means unknown, not false. Native envelopes are synthetic per-event companions, not producer-wide support or native lifecycle reproduction.","sources_sha256":{"rich_scenarios.tsv":sha256_hex(INVENTORY.as_bytes()),"heldout_evidence.json":sha256_hex(HELDOUT.as_bytes()),"development_inventory_evidence.json":sha256_hex(DEVELOPMENT.as_bytes()),"rich_scenarios_heldout.txt":sha256_hex(SPLIT.as_bytes()),"full_record_expectations.tsv":sha256_hex(EXPECTATIONS.as_bytes())},"native_projection_assertions":native_assertions,"packed_request_assertions":request_assertions,"label_contradictions":contradictions,"cases":records});
    schema::validate(&artifact).unwrap();
    for (pointer, value) in [
        ("/cases/0/expected/state", json!(false)),
        (
            "/cases/0/expected/finding_citations/0",
            json!("absent-event"),
        ),
        ("/cases/0/instruction/scope", Value::Null),
        ("/cases/0/stage_budgets/provider_calls/max", json!(1)),
        (
            "/cases/0/events/0/expected_selected/field",
            json!("UserMessage"),
        ),
    ] {
        let mut invalid = artifact.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(
            schema::validate(&invalid).is_err(),
            "validator accepted invalid {pointer}"
        );
    }
    if let Ok(path) = std::env::var("JEV_FULL_RECORDS_OUTPUT") {
        std::fs::write(path, serde_json::to_vec_pretty(&artifact).unwrap()).unwrap();
    }
    println!("{}", serde_json::to_string(&json!({"records":240,"native_projection_assertions":native_assertions,"packed_request_assertions":request_assertions,"label_contradictions":artifact["label_contradictions"]})).unwrap());
}
