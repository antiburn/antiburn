use super::super::evaluation_support::binding_identity::{SemanticReference, export_bindings};
use super::super::{Case, JevCheck, build_jev_context, input};
use super::{IgnoredInstructionsCheck, native, run_confirmation_capture_for_cases};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const SEMANTIC: &str = include_str!(
    "../../../../../../crates/antiburn-local/tests/fixtures/ignored_instructions/independent_confirmation_v2.semantic.json"
);
const BINDINGS: &str = include_str!(
    "../../../../../../crates/antiburn-local/tests/fixtures/ignored_instructions/independent_confirmation_v2.bindings.json"
);

fn normalized_text_bytes(text: &str) -> Vec<u8> {
    text.replace("\r\n", "\n").into_bytes()
}

fn semantic_cases() -> (Value, Vec<Case>) {
    let fixture: Value =
        serde_json::from_str(SEMANTIC).expect("semantic confirmation fixture is JSON");
    let cases = fixture["cases"]
        .as_array()
        .expect("semantic confirmation cases are an array")
        .iter()
        .map(|case| Case {
            id: case["id"]
                .as_str()
                .expect("semantic case id is text")
                .to_owned(),
            family: case["family"]
                .as_str()
                .expect("semantic case family is text")
                .to_owned(),
            expected: case["expected"]
                .as_str()
                .expect("semantic case outcome is text")
                .to_owned(),
            format: case["source_format"]
                .as_str()
                .expect("semantic case format is text")
                .to_owned(),
            instruction: case["instruction"]
                .as_str()
                .expect("semantic case instruction is text")
                .to_owned(),
            fixture: case.clone(),
        })
        .collect();
    (fixture, cases)
}

fn resolve_case(case: &Case) -> (String, Vec<String>, Vec<String>) {
    let source_input = input(case);
    let references = case.fixture["semantic_references"]
        .as_array()
        .expect("semantic references are an array")
        .iter()
        .map(|reference| SemanticReference {
            source: reference["instruction_source"]
                .as_str()
                .expect("reference source is text")
                .to_owned(),
            start_line: reference["start_line"]
                .as_u64()
                .expect("reference start line is an integer") as u32,
            end_line: reference["end_line"]
                .as_u64()
                .expect("reference end line is an integer") as u32,
            action_reference_id: reference["action_reference_id"]
                .as_str()
                .expect("reference action id is text")
                .to_owned(),
        })
        .collect::<Vec<_>>();
    let (revision, identities) = export_bindings(&source_input, &references)
        .unwrap_or_else(|error| panic!("{}: {error}", case.id));
    let citations = identities
        .iter()
        .map(|identity| identity.action_id.clone())
        .collect::<Vec<_>>();
    let exact = identities
        .iter()
        .map(|identity| format!("{}/{}", identity.rule_id, identity.action_id))
        .collect::<Vec<_>>();
    (revision, citations, exact)
}

#[test]
#[ignore = "offline semantic-reference binding export; updates the v2 binding sidecar"]
fn resolve_independent_v2_bindings_offline() {
    let (semantic, cases) = semantic_cases();
    let mut bindings: Value =
        serde_json::from_str(BINDINGS).expect("confirmation bindings are JSON");
    assert_eq!(bindings["binding_status"], "resolved");
    assert_eq!(
        bindings["semantic_sha256"],
        Sha256::digest(normalized_text_bytes(SEMANTIC))
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let semantic_by_id = semantic["cases"]
        .as_array()
        .expect("semantic confirmation has cases")
        .iter()
        .map(|case| (case["id"].as_str().expect("semantic case ID is text"), case))
        .collect::<BTreeMap<_, _>>();
    let entries = bindings["cases"]
        .as_array_mut()
        .expect("confirmation bindings have cases");
    assert_eq!(entries.len(), cases.len());
    for entry in entries {
        let id = entry["id"].as_str().expect("binding case ID is text");
        let case = cases
            .iter()
            .find(|case| case.id == id)
            .unwrap_or_else(|| panic!("unknown binding case: {id}"));
        let semantic_case = semantic_by_id[id];
        let input_hash =
            Sha256::digest(serde_json::to_vec(semantic_case).expect("semantic case serializes"))
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
        assert_eq!(entry["input_hash"], input_hash, "{id} input hash");
        assert_eq!(case.expected, semantic_case["expected"], "{id} outcome");
        let (revision, citations, exact) = resolve_case(case);
        entry["revisions"] = json!(revision);
        entry["citation_ids"] = json!(citations);
        entry["bindings"] = json!(exact);
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../../crates/antiburn-local/tests/fixtures/ignored_instructions/independent_confirmation_v2.bindings.json",
    );
    let temporary = path.with_extension("bindings.json.pending");
    let bytes = serde_json::to_vec_pretty(&bindings).expect("bindings serialize");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .expect("create temporary v2 binding sidecar");
    std::io::Write::write_all(&mut file, &bytes).expect("write v2 binding sidecar");
    file.sync_all().expect("sync v2 binding sidecar");
    std::fs::rename(&temporary, &path).expect("replace v2 binding sidecar");
}

fn native_selected(case: &Case) -> Vec<Value> {
    input(case)
        .content
        .actions
        .iter()
        .map(|action| {
            let index = action.reference.id[1..]
                .parse::<usize>()
                .expect("selected action id has a numeric event index");
            let kind = case.fixture["events"][index][0]
                .as_str()
                .expect("selected event kind is text");
            let field = match kind {
                "assistant" => "AssistantMessage",
                "bash" => "BashCommandInput",
                "edit_path" | "edit_content" => "FileEditPath",
                "read" => "ReadFilePath",
                "search" => "SearchFilesQuery",
                "other" => "OtherToolInput",
                other => panic!("unexpected selected event type: {other}"),
            };
            json!([action.reference.id, field, action.text])
        })
        .collect()
}

fn scheduled_cases() -> Vec<Case> {
    let (semantic, cases) = semantic_cases();
    let bindings: Value = serde_json::from_str(BINDINGS).expect("confirmation bindings are JSON");
    assert_eq!(bindings["binding_status"], "resolved");
    assert_eq!(
        bindings["cases"]
            .as_array()
            .expect("confirmation bindings have cases")
            .len(),
        cases.len()
    );
    let by_id = bindings["cases"]
        .as_array()
        .expect("confirmation bindings have cases")
        .iter()
        .map(|entry| {
            (
                entry["id"].as_str().expect("binding case id is text"),
                entry,
            )
        })
        .collect::<BTreeMap<_, _>>();
    cases
        .into_iter()
        .map(|mut case| {
            let binding = by_id[case.id.as_str()];
            let semantic_case = semantic["cases"]
                .as_array()
                .expect("semantic confirmation has cases")
                .iter()
                .find(|entry| entry["id"] == case.id)
                .expect("binding case has a semantic case");
            let input_hash = binding["input_hash"]
                .as_str()
                .expect("binding input hash is text");
            let content = input(&case).content;
            let context = build_jev_context(&input(&case)).expect("semantic case builds context");
            let plan = IgnoredInstructionsCheck
                .prepare(&context)
                .expect("semantic case prepares check");
            let candidates = plan
                .prepared
                .comparisons
                .iter()
                .map(|comparison| comparison.reference.action_id.clone())
                .collect::<Vec<_>>();
            case.fixture["citations"] = binding["citation_ids"].clone();
            case.fixture["expected_bindings"] = binding["bindings"].clone();
            case.fixture["binding_identities"] = binding["bindings"].clone();
            case.fixture["input_hash"] = binding["input_hash"].clone();
            case.fixture["binding_revisions"] = binding["revisions"].clone();
            case.fixture["label_review"] = Value::Null;
            case.fixture["selected"] = json!(native_selected(&case));
            case.fixture["candidates"] = json!(candidates);
            case.fixture["source_pointer"] = json!(format!(
                "independent_confirmation_v2:{}:{}:{}",
                semantic_case["id"]
                    .as_str()
                    .expect("semantic case id is text"),
                input_hash,
                content.selected_input_digest
            ));
            case
        })
        .collect()
}

#[test]
fn independent_v2_semantic_labels_resolve_to_exact_production_bindings() {
    let (semantic, cases) = semantic_cases();
    let bindings: Value = serde_json::from_str(BINDINGS).expect("confirmation bindings are JSON");
    let by_id = bindings["cases"]
        .as_array()
        .expect("confirmation bindings have cases")
        .iter()
        .map(|binding| {
            (
                binding["id"].as_str().expect("binding case id is text"),
                binding,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut formats = BTreeMap::new();
    let mut families = BTreeMap::new();
    let mut ids = std::collections::BTreeSet::new();
    let mut binding_differences = Vec::new();
    for case in &cases {
        assert!(ids.insert(case.id.as_str()), "duplicate semantic case ID");
        *formats.entry(case.format.as_str()).or_insert(0) += 1;
        *families.entry(case.family.as_str()).or_insert(0) += 1;
        let semantic_case = semantic["cases"]
            .as_array()
            .expect("semantic confirmation has cases")
            .iter()
            .find(|value| value["id"] == case.id)
            .expect("binding case has a semantic case");
        let input_hash =
            Sha256::digest(serde_json::to_vec(semantic_case).expect("semantic case serializes"))
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
        let (revision, citations, exact) = resolve_case(case);
        let frozen = by_id[case.id.as_str()];
        assert_eq!(frozen["input_hash"], input_hash, "{} input hash", case.id);
        for (field, actual) in [
            ("revisions", json!(revision)),
            ("bindings", json!(exact)),
            ("citation_ids", json!(citations)),
        ] {
            if frozen[field] != actual {
                binding_differences.push(format!(
                    "{} {field}: frozen={}, production={actual}",
                    case.id, frozen[field]
                ));
            }
        }
    }
    assert!(
        binding_differences.is_empty(),
        "{}",
        binding_differences.join("\n")
    );
    assert_eq!(cases.len(), 48);
    assert_eq!(by_id.len(), 48);
    assert_eq!(formats.len(), 6);
    assert!(formats.values().all(|count| *count == 8));
    assert_eq!(families.len(), 8);
    assert!(families.values().all(|count| *count == 6));
    assert!(
        cases
            .iter()
            .all(|case| by_id.contains_key(case.id.as_str()))
    );
    assert!(
        cases
            .iter()
            .filter(|case| case.expected == "finding")
            .all(|case| !case.fixture["semantic_references"]
                .as_array()
                .unwrap()
                .is_empty())
    );
    assert!(
        cases
            .iter()
            .filter(|case| case.expected != "finding")
            .all(|case| case.fixture["semantic_references"]
                .as_array()
                .unwrap()
                .is_empty())
    );
    assert!(semantic["cases"].as_array().unwrap().iter().all(|case| {
        case["rationale"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
    }));
    let semantic_hash = Sha256::digest(normalized_text_bytes(SEMANTIC))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(bindings["semantic_sha256"], semantic_hash);
}

#[test]
fn independent_v2_native_projection_matches_synthetic_selected_inputs() {
    for case in scheduled_cases() {
        let selected = case.fixture["selected"]
            .as_array()
            .expect("native selected actions are an array")
            .clone();
        native::validate(&case, &selected);
    }
}

#[tokio::test]
async fn independent_v2_provider_payloads_exclude_frozen_labels() {
    for case in scheduled_cases() {
        super::super::validate_confirmation_payload_isolation(&case).await;
    }
}

#[tokio::test]
#[ignore = "billable independent v2 confirmation; run only after prompt preflight"]
async fn independent_confirmation_v2_live_after_prompt_freeze() {
    let suite_bytes = [
        normalized_text_bytes(SEMANTIC),
        normalized_text_bytes(BINDINGS),
    ]
    .concat();
    let fixture_sha256 = Sha256::digest(suite_bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    run_confirmation_capture_for_cases(
        "jev-independent-confirmation-v2-results.json",
        "Fresh isolated semantic v2 labels with offline production binding IDs and native selected-input equivalence; shared production runner/client.",
        scheduled_cases(),
        fixture_sha256,
    )
    .await;
}
