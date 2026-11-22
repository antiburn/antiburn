//! Check the public metadata schema with a standard Draft 2020-12 validator.
//!
//! Capture tests cover runtime bindings and byte budgets. This target also pins
//! producer JSON fidelity for consumer fingerprint checks.

#![cfg(feature = "cloud")]

use antiburn_cloud_capture::{
    CaptureMetadata, CapturedStream, MetadataEvidence, MetadataObservation, MetadataSection,
    MetadataSections, validate_section_bindings, validate_sections,
};
use flate2::read::GzDecoder;
use jsonschema::Validator;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;

const SCHEMA: &str = include_str!("../../../docs/cloud/capture-metadata.schema.json");
const GUIDE: &str = include_str!("../../../docs/cloud/capture-metadata.md");

/// Compile only local schema references with the explicit published draft.
fn validator() -> Validator {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    jsonschema::draft202012::new(&schema).unwrap()
}

/// Build the common core without inventing historical runtime evidence.
fn metadata(agent: &str, source_format: &str) -> Value {
    json!({
        "version": 1,
        "agent": agent,
        "source_format": source_format,
        "native_session_id": "synthetic-session",
        "native_agent_id": null,
        "source_agent_version": null,
        "version_observations": [],
        "version_scan_complete": false,
        "owned_version_complete": false,
        "relationship": {"kind": "unknown"},
        "history_completeness": "unknown",
        "sections": {}
    })
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Bind any JSON data value without changing the common section envelope.
fn local_section(data: Value) -> Value {
    json!({
        "schema": "example.schema-test.v1",
        "producer": {"name": "synthetic-test", "version": "1.0.0"},
        "observation": {"kind": "observed_at", "unix_millis": 1},
        "evidence": [{
            "kind": "local_observation",
            "method": "synthetic-test",
            "data_sha256": sha(&serde_json::to_vec(&data).unwrap())
        }],
        "data": data
    })
}

fn metadata_with_section() -> Value {
    let mut value = metadata("claude-code", "ClaudeJsonl");
    value["sections"]["example.validation"] = local_section(json!({"synthetic": true}));
    value
}

#[test]
fn schema_is_valid_draft_2020_12() {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    jsonschema::draft202012::meta::validate(&schema).unwrap();
    assert_eq!(validator().draft(), jsonschema::Draft::Draft202012);
}

#[test]
fn claude_and_opencode_share_the_common_metadata_structure() {
    let validator = validator();
    let mut claude = metadata("claude-code", "ClaudeJsonl");
    claude["source_agent_version"] = json!("2.1.309");
    claude["version_observations"] = json!([
        {"version": "2.1.309", "ownership": "owned"},
        {"version": "2.1.300", "ownership": "inherited"}
    ]);
    claude["version_scan_complete"] = json!(true);
    claude["owned_version_complete"] = json!(true);
    let mut opencode = metadata("opencode", "OpenCodeSqliteV2");
    opencode["relationship"] = json!({
        "kind": "child",
        "parent_source_id": "synthetic-parent",
        "proof": "opencode-session-v2-parent"
    });
    opencode["sections"]["example.native-evidence"] = json!({
        "schema": "example.native-evidence.v1",
        "producer": {"name": "synthetic-test", "version": "1.0.0"},
        "observation": {
            "kind": "source_position",
            "stream_id": "native-rows",
            "expanded_offset": 0,
            "future_position": null
        },
        "evidence": [{
            "kind": "native_stream",
            "stream_id": "native-rows",
            "expanded_sha256": "a".repeat(64),
            "future_evidence": {"nested": [false, null]}
        }],
        "data": {"synthetic": true, "future_data": [1, {"value": "kept"}]}
    });
    for value in [claude, opencode] {
        validator.validate(&value).unwrap();
        let parsed: CaptureMetadata = serde_json::from_value(value.clone()).unwrap();
        validate_sections(&parsed.sections).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), value);
    }
}

#[test]
fn any_json_data_and_unknown_section_envelope_fields_roundtrip() {
    let validator = validator();
    for data in [
        json!({"future": [{"nested": [null, false, {"value": "kept"}]}]}),
        json!([1, "text", {"future": true}, null]),
        json!("scalar"),
        json!(12.5),
        json!(-7),
        json!(true),
        Value::Null,
    ] {
        let mut value = metadata("opencode", "OpenCodeSqliteV2");
        let mut section = local_section(data);
        section["future_section"] = json!({"nested": ["kept"]});
        section["producer"]["future_producer"] = Value::Null;
        section["observation"]["future_observation"] = json!([true, 7]);
        section["evidence"][0]["future_evidence"] = json!({"value": "kept"});
        value["sections"]["example.flexible-data"] = section;
        validator.validate(&value).unwrap();
        let parsed: CaptureMetadata = serde_json::from_value(value.clone()).unwrap();
        validate_sections(&parsed.sections).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), value);
    }
}

#[test]
fn fingerprints_preserve_producer_numbers_and_utf8_key_order() {
    let mut value = metadata("opencode", "OpenCodeSqliteV2");
    value["sections"]["example.numeric-fidelity"] = local_section(json!({
        "large": u64::MAX,
        "float": 1.0,
        "zero": -0.0,
        "\u{1f600}": "astral",
        "\u{e000}": "pua"
    }));
    validator().validate(&value).unwrap();
    let parsed: CaptureMetadata = serde_json::from_value(value).unwrap();
    validate_sections(&parsed.sections).unwrap();

    // This preimage fixes number text and U+E000 before U+1F600 independently of
    // the fingerprint implementation. Only the typed observation time is absent.
    let expected = concat!(
        r#"{"agent":"opencode","history_completeness":"unknown","native_agent_id":null,"#,
        r#""native_session_id":"synthetic-session","owned_version_complete":false,"#,
        r#""relationship":{"kind":"unknown"},"sections":{"example.numeric-fidelity":{"data":{"#,
        r#""float":1.0,"large":18446744073709551615,"zero":-0.0,"#,
        "\"\u{e000}\":\"pua\",\"\u{1f600}\":\"astral\"},",
        r#""evidence":[{"data_sha256":"4db924d4187204f46fb74a1c8496ac416d48197a99224719a3cbcc18f5858202","#,
        r#""kind":"local_observation","method":"synthetic-test"}],"observation":{"kind":"observed_at"},"#,
        r#""producer":{"name":"synthetic-test","version":"1.0.0"},"schema":"example.schema-test.v1"}},"#,
        r#""source_agent_version":null,"source_format":"OpenCodeSqliteV2","version":1,"#,
        r#""version_observations":[],"version_scan_complete":false}"#
    );
    assert_eq!(
        parsed.stable_fingerprint().unwrap(),
        sha(expected.as_bytes())
    );

    let wire = serde_json::to_vec(&parsed).unwrap();
    let decoded: CaptureMetadata = serde_json::from_slice(&wire).unwrap();
    assert_eq!(serde_json::to_vec(&decoded).unwrap(), wire);
    assert_eq!(
        decoded.stable_fingerprint().unwrap(),
        sha(expected.as_bytes())
    );
}

#[test]
fn section_data_preserves_numbers_beyond_machine_precision() {
    let section: MetadataSection = serde_json::from_str(concat!(
        r#"{"schema":"example.schema-test.v1","producer":{"name":"synthetic-test","version":"1.0.0"},"#,
        r#""observation":{"kind":"observed_at","unix_millis":1},"evidence":[{"kind":"local_observation","#,
        r#""method":"synthetic-test","data_sha256":"02b4b6602fcc69d4251921505b6d4040fb473d74d248bdd560f50d1467f59944"}],"#,
        r#""data":{"large_integer":1208925819614629174706177,"precise_decimal":0.12345678901234567890123456789}}"#
    ))
    .unwrap();
    let expected_data = concat!(
        r#"{"large_integer":1208925819614629174706177,"#,
        r#""precise_decimal":0.12345678901234567890123456789}"#
    );
    assert_eq!(serde_json::to_string(&section.data).unwrap(), expected_data);

    let mut captured: CaptureMetadata =
        serde_json::from_value(metadata("opencode", "OpenCodeSqliteV2")).unwrap();
    captured
        .sections
        .insert("example.precise-numbers".into(), section);
    validate_sections(&captured.sections).unwrap();
    let wire = serde_json::to_vec(&captured).unwrap();
    validator()
        .validate(&serde_json::from_slice::<Value>(&wire).unwrap())
        .unwrap();

    let expected = concat!(
        r#"{"agent":"opencode","history_completeness":"unknown","native_agent_id":null,"#,
        r#""native_session_id":"synthetic-session","owned_version_complete":false,"#,
        r#""relationship":{"kind":"unknown"},"sections":{"example.precise-numbers":{"data":{"#,
        r#""large_integer":1208925819614629174706177,"precise_decimal":0.12345678901234567890123456789},"#,
        r#""evidence":[{"data_sha256":"02b4b6602fcc69d4251921505b6d4040fb473d74d248bdd560f50d1467f59944","#,
        r#""kind":"local_observation","method":"synthetic-test"}],"observation":{"kind":"observed_at"},"#,
        r#""producer":{"name":"synthetic-test","version":"1.0.0"},"schema":"example.schema-test.v1"}},"#,
        r#""source_agent_version":null,"source_format":"OpenCodeSqliteV2","version":1,"#,
        r#""version_observations":[],"version_scan_complete":false}"#
    );
    assert_eq!(
        captured.stable_fingerprint().unwrap(),
        sha(expected.as_bytes())
    );
    let decoded: CaptureMetadata = serde_json::from_slice(&wire).unwrap();
    assert_eq!(
        serde_json::to_string(&decoded.sections["example.precise-numbers"].data).unwrap(),
        expected_data
    );
    assert_eq!(serde_json::to_vec(&decoded).unwrap(), wire);
    assert_eq!(
        decoded.stable_fingerprint().unwrap(),
        sha(expected.as_bytes())
    );
}

#[test]
fn literal_number_marker_objects_survive_data_extensions_and_fingerprints() {
    let literal = json!({"$serde_json::private::Number": "1"});
    let mut captured: CaptureMetadata = serde_json::from_value(metadata_with_section()).unwrap();
    let section = captured.sections.get_mut("example.validation").unwrap();
    section.data = literal.clone();
    section
        .unknown_fields
        .insert("future_value".into(), literal.clone());
    let MetadataEvidence::LocalObservation { data_sha256, .. } = &mut section.evidence[0] else {
        panic!("expected local evidence");
    };
    *data_sha256 = sha(&serde_json::to_vec(&literal).unwrap());
    validate_sections(&captured.sections).unwrap();

    let expected = concat!(
        r#"{"agent":"claude-code","history_completeness":"unknown","native_agent_id":null,"#,
        r#""native_session_id":"synthetic-session","owned_version_complete":false,"#,
        r#""relationship":{"kind":"unknown"},"sections":{"example.validation":{"data":{"#,
        r#""$serde_json::private::Number":"1"},"evidence":[{"data_sha256":"#,
        r#""1fdab8c71229bec72450c42c98ea7e162c4cfef47487bce4826b060e917ff552","#,
        r#""kind":"local_observation","method":"synthetic-test"}],"#,
        r#""future_value":{"$serde_json::private::Number":"1"},"observation":{"kind":"observed_at"},"#,
        r#""producer":{"name":"synthetic-test","version":"1.0.0"},"schema":"example.schema-test.v1"}},"#,
        r#""source_agent_version":null,"source_format":"ClaudeJsonl","version":1,"#,
        r#""version_observations":[],"version_scan_complete":false}"#
    );
    assert_eq!(
        captured.stable_fingerprint().unwrap(),
        sha(expected.as_bytes())
    );
    let wire = serde_json::to_vec(&captured).unwrap();
    let decoded: CaptureMetadata = serde_json::from_slice(&wire).unwrap();
    assert_eq!(decoded.sections["example.validation"].data, literal);
    assert_eq!(
        decoded.sections["example.validation"].unknown_fields["future_value"],
        literal
    );
    validate_sections(&decoded.sections).unwrap();
    assert_eq!(serde_json::to_vec(&decoded).unwrap(), wire);
    assert_eq!(
        decoded.stable_fingerprint().unwrap(),
        sha(expected.as_bytes())
    );
}

#[test]
fn library_marker_keys_stay_literal_in_nested_data_and_every_envelope() {
    for literal in [
        json!({"$serde_json::private::Number": "1"}),
        json!({"$serde_json::private::RawValue": "null"}),
    ] {
        let mut captured: CaptureMetadata =
            serde_json::from_value(metadata_with_section()).unwrap();
        let section = captured.sections.get_mut("example.validation").unwrap();
        section.data = json!({"nested": [literal.clone(), {"again": literal.clone()}]});
        let extension = json!([literal.clone(), {"nested": literal}]);
        section
            .unknown_fields
            .insert("future_value".into(), extension.clone());
        section
            .producer
            .unknown_fields
            .insert("future_value".into(), extension.clone());
        let MetadataObservation::ObservedAt { unknown_fields, .. } = &mut section.observation
        else {
            panic!("expected observation time");
        };
        unknown_fields.insert("future_value".into(), extension.clone());
        let MetadataEvidence::LocalObservation {
            data_sha256,
            unknown_fields,
            ..
        } = &mut section.evidence[0]
        else {
            panic!("expected local evidence");
        };
        *data_sha256 = sha(&serde_json::to_vec(&section.data).unwrap());
        unknown_fields.insert("future_value".into(), extension);
        validate_sections(&captured.sections).unwrap();
        validator()
            .validate(&serde_json::to_value(&captured).unwrap())
            .unwrap();

        let wire = serde_json::to_vec(&captured).unwrap();
        let decoded: CaptureMetadata = serde_json::from_slice(&wire).unwrap();
        assert_eq!(decoded, captured);
        validate_sections(&decoded.sections).unwrap();
        assert_eq!(serde_json::to_vec(&decoded).unwrap(), wire);
        assert_eq!(
            decoded.stable_fingerprint().unwrap(),
            captured.stable_fingerprint().unwrap()
        );
    }
}

#[test]
fn raw_metadata_decoding_keeps_duplicate_field_and_nesting_checks() {
    let original = metadata_with_section();
    let section = &original["sections"]["example.validation"];
    for (field, value) in [
        ("schema", section),
        ("name", &section["producer"]),
        ("unix_millis", &section["observation"]),
        ("method", &section["evidence"][0]),
    ] {
        let source = serde_json::to_string(value).unwrap();
        let duplicate = format!("{{\"{field}\":null,{}", &source[1..]);
        let error = match field {
            "schema" => serde_json::from_str::<MetadataSection>(&duplicate).unwrap_err(),
            "name" => serde_json::from_str::<antiburn_cloud_capture::MetadataProducer>(&duplicate)
                .unwrap_err(),
            "unix_millis" => serde_json::from_str::<MetadataObservation>(&duplicate).unwrap_err(),
            "method" => serde_json::from_str::<MetadataEvidence>(&duplicate).unwrap_err(),
            _ => unreachable!(),
        };
        assert!(error.to_string().contains("duplicate metadata field"));
    }
    for depth in [16, 129] {
        let mut value = original.clone();
        value["sections"]["example.validation"]["data"] =
            (0..depth).fold(Value::Null, |nested, _| json!([nested]));
        let wire = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            serde_json::from_slice::<CaptureMetadata>(&wire).is_ok(),
            depth == 16
        );
    }
}

#[test]
fn required_core_and_envelope_fields_cannot_be_omitted() {
    let validator = validator();
    let original = metadata_with_section();
    for field in original.as_object().unwrap().keys() {
        let mut value = original.clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(!validator.is_valid(&value), "missing core field {field}");
    }
    let section_path = "/sections/example.validation";
    for (suffix, fields) in [
        (
            "",
            vec!["schema", "producer", "observation", "evidence", "data"],
        ),
        ("/producer", vec!["name", "version"]),
        ("/observation", vec!["kind", "unix_millis"]),
        ("/evidence/0", vec!["kind", "method", "data_sha256"]),
    ] {
        for field in fields {
            let mut value = original.clone();
            value
                .pointer_mut(&format!("{section_path}{suffix}"))
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(!validator.is_valid(&value), "missing {suffix}/{field}");
            assert!(serde_json::from_value::<CaptureMetadata>(value).is_err());
        }
    }
}

#[test]
fn wrong_types_versions_and_extra_core_fields_are_rejected() {
    let validator = validator();
    let original = metadata_with_section();
    for version in [
        Value::Null,
        json!("1"),
        json!(0),
        json!(2),
        json!(true),
        json!(1.5),
    ] {
        let mut value = original.clone();
        value["version"] = version;
        assert!(!validator.is_valid(&value));
    }
    for (path, invalid) in [
        ("/agent", json!(7)),
        ("/source_format", json!("SQLite")),
        ("/native_session_id", Value::Null),
        ("/native_agent_id", json!([])),
        ("/source_agent_version", json!(2)),
        ("/version_observations", json!({})),
        ("/version_scan_complete", json!("false")),
        ("/owned_version_complete", Value::Null),
        ("/relationship", json!({"kind": "child"})),
        ("/history_completeness", json!("complete")),
        ("/sections", json!([])),
        ("/sections/example.validation/schema", json!({})),
        ("/sections/example.validation/producer/name", Value::Null),
        ("/sections/example.validation/producer/version", json!(1)),
        (
            "/sections/example.validation/observation/kind",
            json!("future_kind"),
        ),
        (
            "/sections/example.validation/observation/unix_millis",
            json!("1"),
        ),
        ("/sections/example.validation/evidence/0", json!({})),
        (
            "/sections/example.validation/evidence/0/data_sha256",
            json!(9),
        ),
    ] {
        let mut value = original.clone();
        *value.pointer_mut(path).unwrap() = invalid;
        assert!(
            !validator.is_valid(&value),
            "invalid type or value at {path}"
        );
    }
    for field in ["capture_version", "recorded_versions", "future_core_field"] {
        let mut value = original.clone();
        value[field] = json!(1);
        assert!(!validator.is_valid(&value), "extra core field {field}");
    }
}

#[test]
fn schema_checks_names_counts_and_observation_evidence_kinds() {
    let validator = validator();
    let original = metadata_with_section();
    for key in [
        "unscoped",
        "example..value",
        "example/value",
        "example.é",
        "example.value\n",
    ] {
        let mut value = metadata("claude-code", "ClaudeJsonl");
        value["sections"][key] = original["sections"]["example.validation"].clone();
        assert!(!validator.is_valid(&value), "invalid section key {key:?}");
    }
    let evidence = original["sections"]["example.validation"]["evidence"][0].clone();
    for count in [0, 8, 9] {
        let mut value = original.clone();
        value["sections"]["example.validation"]["evidence"] =
            Value::Array(vec![evidence.clone(); count]);
        assert_eq!(validator.is_valid(&value), count == 8);
    }
    for count in [8, 9] {
        let mut value = metadata("claude-code", "ClaudeJsonl");
        for index in 0..count {
            value["sections"][format!("example.section-{index}")] =
                original["sections"]["example.validation"].clone();
        }
        assert_eq!(validator.is_valid(&value), count == 8);
    }
    let mut value = original;
    value["sections"]["example.validation"]["observation"] = json!({
        "kind": "source_position", "stream_id": "native-rows", "expanded_offset": 0
    });
    assert!(!validator.is_valid(&value));
    value["sections"]["example.validation"]["evidence"] = json!([{
        "kind": "native_stream", "stream_id": "native-rows", "expanded_sha256": "a".repeat(64)
    }]);
    validator.validate(&value).unwrap();
    for field in ["kind", "stream_id", "expanded_offset"] {
        let mut missing = value.clone();
        missing["sections"]["example.validation"]["observation"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            !validator.is_valid(&missing),
            "missing native observation {field}"
        );
        assert!(serde_json::from_value::<CaptureMetadata>(missing).is_err());
    }
    for field in ["kind", "stream_id", "expanded_sha256"] {
        let mut missing = value.clone();
        missing["sections"]["example.validation"]["evidence"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            !validator.is_valid(&missing),
            "missing native evidence {field}"
        );
        assert!(serde_json::from_value::<CaptureMetadata>(missing).is_err());
    }
    value["sections"]["example.validation"]["observation"] =
        json!({"kind": "observed_at", "unix_millis": 1});
    assert!(!validator.is_valid(&value));
}

#[test]
fn maintained_examples_have_valid_structure_and_exact_evidence_bindings() {
    let lf_guide = GUIDE.replace("\r\n", "\n");
    let crlf_guide = lf_guide.replace('\n', "\r\n");
    for guide in [lf_guide, crlf_guide] {
        // The guide declares LF bytes for native evidence, not checkout line endings.
        let guide = guide.replace("\r\n", "\n");
        let examples: Vec<&str> = guide
            .split("```json\n")
            .skip(1)
            .map(|block| block.split_once("```").unwrap().0)
            .collect();
        assert_eq!(examples.len(), 4);
        let validator = validator();
        let common: Value = serde_json::from_str(examples[0]).unwrap();
        let git: Value = serde_json::from_str(examples[1]).unwrap();
        let _: Value = serde_json::from_str(examples[2]).unwrap();
        let billing: Value = serde_json::from_str(examples[3]).unwrap();
        validator.validate(&common).unwrap();
        let mut claude = metadata("claude-code", "ClaudeJsonl");
        claude["sections"]["antiburn.git-checkout"] = git;
        claude["sections"]["example.session-billing"] = billing;
        validator.validate(&claude).unwrap();
        let parsed: CaptureMetadata = serde_json::from_value(claude).unwrap();
        let native = CapturedStream {
            stream_id: "transcript".into(),
            role: "native_transcript".into(),
            schema: "claude-jsonl".into(),
            expanded_bytes: examples[2].len() as u64,
            expanded_sha256: sha(examples[2].as_bytes()),
            chunks: vec![],
        };
        validate_section_bindings(&parsed.sections, &[native]).unwrap();
    }
}

#[test]
fn existing_reference_metadata_validates_without_rewriting_fixture_bytes() {
    let mut bytes = Vec::new();
    GzDecoder::new(
        include_bytes!(
            "../../../fixtures/cloud/capture-v1/reference/main.capture-metadata.chunk-00000.gz"
        )
        .as_slice(),
    )
    .read_to_end(&mut bytes)
    .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    validator().validate(&value).unwrap();
    let sections: MetadataSections = serde_json::from_value(value["sections"].clone()).unwrap();
    validate_sections(&sections).unwrap();
}
