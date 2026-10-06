use super::*;

#[path = "independent_confirmation/native.rs"]
mod native;

#[path = "independent_confirmation/v2.rs"]
mod confirmation_v2;

const CONFIRMATION: &str = include_str!(
    "../../../../../crates/antiburn-local/tests/fixtures/ignored_instructions/independent_confirmation.json"
);

fn independent_cases() -> Vec<Case> {
    let fixture: Value =
        serde_json::from_str(CONFIRMATION).expect("independent confirmation fixture is JSON");
    fixture["cases"]
        .as_array()
        .expect("independent confirmation has cases")
        .iter()
        .map(|fixture| Case {
            id: fixture["id"]
                .as_str()
                .expect("confirmation case id is text")
                .to_owned(),
            family: fixture["family"]
                .as_str()
                .expect("confirmation case family is text")
                .to_owned(),
            expected: fixture["expected"]
                .as_str()
                .expect("confirmation case expected outcome is text")
                .to_owned(),
            format: fixture["format"]
                .as_str()
                .expect("confirmation case format is text")
                .to_owned(),
            instruction: fixture["instruction"]
                .as_str()
                .expect("confirmation case instruction is text")
                .to_owned(),
            fixture: fixture.clone(),
        })
        .collect()
}

fn validate_independent_projection(case: &Case) {
    let input = input(case);
    let expected = case.fixture["selected"]
        .as_array()
        .expect("confirmation selected actions are an array");
    assert_eq!(input.content.actions.len(), expected.len(), "{}", case.id);
    for (action, selected) in input.content.actions.iter().zip(expected) {
        assert_eq!(action.reference.id, selected[0], "{}", case.id);
        assert_eq!(action.text, selected[2], "{}", case.id);
        let field = match selected[1].as_str().expect("selected action field is text") {
            "AssistantMessage" => "assistant_text",
            "BashCommandInput" | "FileEditPath" | "ReadFilePath" | "SearchFilesQuery"
            | "OtherToolInput" => "tool_input",
            field => panic!("unknown selected field {field}"),
        };
        assert_eq!(action.kind, field, "{}", case.id);
    }
    let context = build_jev_context(&input).expect("confirmation input builds context");
    let plan = IgnoredInstructionsCheck
        .prepare(&context)
        .expect("confirmation context prepares check");
    let actual = plan
        .prepared
        .comparisons
        .iter()
        .map(|comparison| comparison.reference.action_id.as_str())
        .collect::<BTreeSet<_>>();
    let candidates = case.fixture["candidates"]
        .as_array()
        .expect("confirmation candidates are an array")
        .iter()
        .map(|id| id.as_str().expect("confirmation candidate id is text"))
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, candidates, "{}", case.id);
    let packed = pack_work_items(&plan.work_items);
    let payload = serde_json::to_string(
        &packed
            .batches
            .iter()
            .map(|batch| &batch.request)
            .collect::<Vec<_>>(),
    )
    .expect("confirmation request payload serializes");
    assert!(!payload.contains(&case.id));
    assert!(
        !payload.contains(
            case.fixture["rationale"]
                .as_str()
                .expect("confirmation rationale is text")
        )
    );
    native::validate(case, expected);
}

#[test]
fn independent_confirmation_frozen_labels_and_native_projection() {
    let cases = independent_cases();
    assert_eq!(cases.len(), 48);
    let mut ids = BTreeSet::new();
    let mut families = BTreeMap::new();
    let mut formats = BTreeMap::new();
    for case in &cases {
        assert!(ids.insert(&case.id));
        *families.entry(&case.family).or_insert(0) += 1;
        *formats.entry(&case.format).or_insert(0) += 1;
        assert!(!case.fixture["rationale"].as_str().unwrap().is_empty());
        assert_eq!(
            case.expected == "finding",
            !case.fixture["citations"].as_array().unwrap().is_empty()
        );
        for citation in case.fixture["citations"].as_array().unwrap() {
            assert!(
                case.fixture["candidates"]
                    .as_array()
                    .unwrap()
                    .contains(citation)
            );
        }
        validate_independent_projection(case);
    }
    assert_eq!(families.len(), 8);
    assert!(families.values().all(|count| *count == 6));
    assert_eq!(formats.len(), 6);
    assert!(formats.values().all(|count| *count == 8));
}

async fn run_confirmation_capture_for_cases(
    name: &str,
    scope: &str,
    cases: Vec<Case>,
    fixture_sha256: String,
) {
    assert_eq!(
        std::env::var("ANTIBURN_CONFIRMATION_PROMPTS_FROZEN").as_deref(),
        Ok("1")
    );
    let _guard = LiveRunGuard::acquire();
    let capture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../.agent-artifacts/reviews")
        .join(name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(capture)
        .expect("preserve the first independent confirmation capture");
    for case in &cases {
        validate_independent_projection(case);
    }
    let client = jev_client::TypeSafeClient::new(
        std::env::var("TYPESAFE_API_KEY").expect("authorized confirmation key"),
    )
    .expect("authorized confirmation key creates client");
    let usage = Arc::new(Mutex::new(RunUsage::default()));
    let mut rows = Vec::new();
    for case in &cases {
        let mut row = live_case(case, &client, &usage).await;
        row["correct"] = json!(row["correct"] == true && row["citation_correct"] == true);
        rows.push(row);
        let report = json!({
            "quality_v2": evaluation_support::scoring::score_v2(
                &scoring_inventory(&cases),
                &rows,
            ),
            "model": PINNED_MODEL,
            "fixture_sha256":fixture_sha256,
            "expected_cases":48,
            "evaluated_cases":rows.len(),
            "scope":scope,
            "revisions":IgnoredInstructionsCheck.revisions(),
            "parser_revision":antiburn_local::analysis::PARSER_REVISION,
            "by_family":grouped_metrics(&rows,"family"),
            "by_source":grouped_metrics(&rows,"source_format"),
            "rows":rows,
        });
        use std::io::{Seek, Write};
        file.set_len(0)
            .expect("confirmation capture can be truncated");
        file.rewind().expect("confirmation capture can be rewound");
        file.write_all(
            &serde_json::to_vec_pretty(&report).expect("confirmation report serializes"),
        )
        .expect("confirmation capture accepts report");
        file.sync_all().expect("confirmation capture syncs to disk");
    }
    assert_eq!(rows.len(), 48);
    let quality = evaluation_support::scoring::score_v2(&scoring_inventory(&cases), &rows);
    assert!(
        evaluation_support::scoring::passes_v2(&quality),
        "preserve the capture and report each mismatch; do not relabel or tune on this confirmation set: {quality}"
    );
}
