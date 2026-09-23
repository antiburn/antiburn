use serde_json::Value;

const CASES: &str = include_str!("fixtures/ignored_instructions/cases.json");

#[test]
fn labeled_synthetic_cases_are_complete_and_reference_submitted_events() {
    let fixture: Value = serde_json::from_str(CASES).expect("valid synthetic case fixture");
    assert_eq!(fixture["schema_version"], 1);
    assert!(
        fixture["source_policy"]
            .as_str()
            .unwrap()
            .contains("Synthetic")
    );

    let cases = fixture["cases"].as_array().expect("cases array");
    assert_eq!(cases.len(), 8);
    let mut ids = std::collections::BTreeSet::new();
    let mut families = std::collections::BTreeSet::new();

    for case in cases {
        let id = case["id"].as_str().expect("case ID");
        assert!(ids.insert(id), "duplicate case ID: {id}");
        families.insert(case["family"].as_str().expect("case family"));
        assert!(case["instruction"]["text"].as_str().is_some());

        let submitted_events: std::collections::BTreeSet<&str> = case["evidence_chunks"]
            .as_array()
            .expect("evidence chunks")
            .iter()
            .flat_map(|chunk| chunk["events"].as_array().expect("events"))
            .map(|event| event["id"].as_str().expect("event ID"))
            .collect();
        for field in ["finding_event_ids", "counterevidence_event_ids"] {
            if let Some(references) = case["expected"][field].as_array() {
                for reference in references {
                    let event_id = reference.as_str().expect("event reference");
                    assert!(
                        submitted_events.contains(event_id),
                        "{id} references absent event {event_id}"
                    );
                }
            }
        }
    }

    assert_eq!(
        families,
        [
            "code_and_architecture",
            "communication",
            "workflow_and_tools"
        ]
        .into_iter()
        .collect()
    );
}
