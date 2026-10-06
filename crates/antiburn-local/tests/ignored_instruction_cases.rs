use serde_json::Value;

const CASES: &str = include_str!("fixtures/ignored_instructions/cases.json");
const RICH_CASES: &str = include_str!("fixtures/ignored_instructions/rich_scenarios.tsv");
const HELD_OUT: &str = include_str!("fixtures/ignored_instructions/rich_scenarios_heldout.txt");

#[test]
fn all_240_inventory_cases_have_materialized_events_and_valid_explicit_references() {
    use std::collections::BTreeSet;

    let inventory_ids = RICH_CASES
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.split('|').next().unwrap())
        .collect::<BTreeSet<_>>();
    let heldout_ids = HELD_OUT
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect::<BTreeSet<_>>();
    let mut materialized = BTreeSet::new();
    for (heldout, text, expected_count) in [
        (
            true,
            include_str!("fixtures/ignored_instructions/heldout_evidence.json"),
            48,
        ),
        (
            false,
            include_str!("fixtures/ignored_instructions/development_inventory_evidence.json"),
            192,
        ),
    ] {
        let fixture: Value = serde_json::from_str(text).unwrap();
        assert_eq!(fixture["schema_version"], 1);
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), expected_count);
        for case in cases {
            let id = case["id"].as_str().unwrap();
            assert!(inventory_ids.contains(id), "unknown fixture {id}");
            assert!(materialized.insert(id.to_owned()), "duplicate fixture {id}");
            assert_eq!(heldout_ids.contains(id), heldout, "wrong split for {id}");
            let events = case["events"].as_array().unwrap();
            assert!(!events.is_empty(), "missing events for {id}");
            for event in events {
                let fields = event.as_array().unwrap();
                assert_eq!(fields.len(), 2, "invalid event for {id}");
                assert!(!fields[0].as_str().unwrap().is_empty());
                assert!(fields[1].is_string(), "invalid event text for {id}");
            }
            let mut citations = BTreeSet::new();
            if let Some(references) = case["citations"].as_array() {
                for reference in references {
                    let reference = reference.as_str().unwrap();
                    assert!(citations.insert(reference), "duplicate citation for {id}");
                    let index = reference
                        .strip_prefix('e')
                        .unwrap()
                        .parse::<usize>()
                        .unwrap();
                    assert!(index < events.len(), "absent citation {reference} for {id}");
                }
            }
        }
    }
    assert_eq!(materialized.len(), 240);
    assert_eq!(
        materialized
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        inventory_ids
    );
}

#[test]
fn labeled_synthetic_cases_are_complete_and_reference_submitted_events() {
    let fixture: Value = serde_json::from_str(CASES).expect("valid synthetic case fixture");
    assert_eq!(fixture["schema_version"], 2);
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

#[test]
fn rich_scenario_inventory_has_240_labeled_cases_and_a_frozen_heldout_split() {
    use antiburn_local::analysis::SourceFormat;

    let source_formats = [
        SourceFormat::ClaudeJsonl,
        SourceFormat::CodexRolloutJsonl,
        SourceFormat::OpenCodeSqliteV2,
        SourceFormat::PiV3Jsonl,
        SourceFormat::CursorCliAgentJsonl,
        SourceFormat::AntigravityBrainJsonl,
    ];
    let is_source_format = |name: &str| {
        source_formats.iter().any(|format| match format {
            SourceFormat::ClaudeJsonl => name == "ClaudeJsonl",
            SourceFormat::CodexRolloutJsonl => name == "CodexRolloutJsonl",
            SourceFormat::OpenCodeSqliteV2 => name == "OpenCodeSqliteV2",
            SourceFormat::PiV3Jsonl => name == "PiV3Jsonl",
            SourceFormat::CursorCliAgentJsonl => name == "CursorCliAgentJsonl",
            SourceFormat::AntigravityBrainJsonl => name == "AntigravityBrainJsonl",
            _ => false,
        })
    };
    let expected_families = [
        ("commands", 30),
        ("assistant_communication", 24),
        ("prerequisites_exceptions", 32),
        ("paths_reads_searches", 24),
        ("instruction_scope_provenance", 20),
        ("chunking_retrieval", 28),
        ("candidate_selection", 20),
        ("lifecycle_incremental", 24),
        ("provider_response_handling", 16),
        ("projection_isolation", 22),
    ];
    let mut ids = std::collections::BTreeSet::new();
    let mut case_families = std::collections::BTreeMap::new();
    let mut family_counts = std::collections::BTreeMap::new();
    for (line_number, line) in RICH_CASES.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('|').collect::<Vec<_>>();
        assert!(
            matches!(fields.len(), 6 | 7),
            "case line {} fields",
            line_number + 1
        );
        let id = fields[0];
        let family = fields[1];
        let outcome = fields[2];
        let (source_format, instruction, evidence, rationale) = if fields.len() == 7 {
            (fields[3], fields[4], fields[5], fields[6])
        } else {
            ("ClaudeJsonl", fields[3], fields[4], fields[5])
        };
        assert!(ids.insert(id), "duplicate case ID: {id}");
        assert!(
            !id.is_empty()
                && id.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                }),
            "unstable case ID: {id}"
        );
        assert!(expected_families.iter().any(|(name, _)| *name == family));
        assert!(matches!(
            outcome,
            "finding" | "no_finding" | "pending" | "unassessed"
        ));
        assert!(
            is_source_format(source_format),
            "{id} has unknown source format"
        );
        assert!(!instruction.trim().is_empty(), "{id} has no instruction");
        assert!(
            !evidence.trim().is_empty(),
            "{id} has no selected-evidence description"
        );
        assert!(
            !rationale.trim().is_empty(),
            "{id} has no outcome rationale"
        );
        case_families.insert(id, family);
        *family_counts.entry(family).or_insert(0usize) += 1;
    }
    assert_eq!(ids.len(), 240);
    for (family, expected) in expected_families {
        assert_eq!(
            family_counts.get(family),
            Some(&expected),
            "family {family}"
        );
    }

    let held_out_lines = HELD_OUT
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty() && !id.starts_with('#'))
        .collect::<Vec<_>>();
    let held_out = held_out_lines
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        held_out_lines.len(),
        held_out.len(),
        "held-out IDs must be unique"
    );
    assert_eq!(
        held_out.len(),
        48,
        "held-out split must contain 20% of cases"
    );
    assert!(
        held_out.iter().all(|id| ids.contains(id)),
        "held-out ID is unknown"
    );
    let mut held_out_counts = std::collections::BTreeMap::new();
    for id in &held_out {
        let family = case_families[id];
        *held_out_counts.entry(family).or_insert(0usize) += 1;
    }
    for (family, expected) in [
        ("commands", 6),
        ("assistant_communication", 5),
        ("prerequisites_exceptions", 6),
        ("paths_reads_searches", 5),
        ("instruction_scope_provenance", 4),
        ("chunking_retrieval", 6),
        ("candidate_selection", 4),
        ("lifecycle_incremental", 5),
        ("provider_response_handling", 3),
        ("projection_isolation", 4),
    ] {
        assert_eq!(held_out_counts.get(family), Some(&expected));
    }
    let held_out_families = held_out
        .iter()
        .map(|id| case_families[id])
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(held_out_families.len(), 10);
}
