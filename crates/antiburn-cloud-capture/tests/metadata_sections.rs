#![cfg(feature = "cloud")]

use antiburn_cloud_capture::{
    CaptureMetadata, CapturedStream, Error, MetadataEvidence, MetadataObservation, MetadataSection,
    MetadataSections, Relationship, validate_section_bindings, validate_sections,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn sections() -> MetadataSections {
    serde_json::from_str(include_str!(
        "../../../fixtures/cloud/capture-v1/sections.json"
    ))
    .unwrap()
}

fn native_stream() -> CapturedStream {
    CapturedStream {
        stream_id: "transcript".into(),
        role: "native_transcript".into(),
        schema: "claude-jsonl".into(),
        expanded_bytes: 390,
        expanded_sha256: "d435bab180bea7840ae99a76912980d7c1a172265e18fc4256016089b0e04f44".into(),
        chunks: vec![],
    }
}

fn native_section(sections: &mut MetadataSections) -> &mut MetadataSection {
    sections.get_mut("antiburn.allowance-contribution").unwrap()
}

fn local_section(sections: &mut MetadataSections) -> &mut MetadataSection {
    sections.get_mut("antiburn.capture-checkout").unwrap()
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn synthetic_sections_bind_to_exact_native_bytes_and_local_data() {
    let sections = sections();
    validate_sections(&sections).unwrap();
    validate_section_bindings(&sections, &[native_stream()]).unwrap();
    let bytes = include_bytes!("../../../fixtures/cloud/capture-v1/project/main.jsonl");
    assert_eq!(bytes.len() as u64, native_stream().expanded_bytes);
    assert_eq!(sha(bytes), native_stream().expanded_sha256);
    let local = &sections["antiburn.capture-checkout"];
    let MetadataEvidence::LocalObservation { data_sha256, .. } = &local.evidence[0] else {
        panic!("expected local evidence");
    };
    assert_eq!(*data_sha256, sha(&serde_json::to_vec(&local.data).unwrap()));
}

#[test]
fn native_row_streams_can_bind_source_position_evidence() {
    let mut sections = sections();
    let section = native_section(&mut sections);
    let MetadataObservation::SourcePosition { stream_id, .. } = &mut section.observation else {
        panic!("expected native observation");
    };
    *stream_id = "native-rows".into();
    let MetadataEvidence::NativeStream { stream_id, .. } = &mut section.evidence[0] else {
        panic!("expected native evidence");
    };
    *stream_id = "native-rows".into();
    let mut stream = native_stream();
    stream.stream_id = "native-rows".into();
    stream.role = "native_rows".into();
    stream.schema = "antiburn-sqlite-native".into();
    validate_section_bindings(&sections, &[stream]).unwrap();
}

#[test]
fn missing_observation_and_evidence_fail_deserialization_or_validation() {
    for field in ["observation", "evidence"] {
        let mut value = serde_json::to_value(sections()).unwrap();
        value["antiburn.capture-checkout"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(serde_json::from_value::<MetadataSections>(value).is_err());
    }
    let mut sections = sections();
    local_section(&mut sections).evidence.clear();
    assert!(matches!(
        validate_sections(&sections),
        Err(Error::InvalidMetadata)
    ));
}

#[test]
fn section_keys_require_bounded_ascii_namespaces() {
    let section = sections().into_values().next().unwrap();
    for key in [
        "".to_owned(),
        "antiburn".to_owned(),
        ".section".to_owned(),
        "antiburn.".to_owned(),
        "antiburn..section".to_owned(),
        "antiburn.section/path".to_owned(),
        "antiburn.séction".to_owned(),
        format!("antiburn.{}", "a".repeat(120)),
    ] {
        let sections = [(key, section.clone())].into_iter().collect();
        assert!(matches!(
            validate_sections(&sections),
            Err(Error::InvalidMetadata)
        ));
    }
    let sections = [(format!("a.{}", "b".repeat(126)), section)]
        .into_iter()
        .collect();
    validate_sections(&sections).unwrap();
}

#[test]
fn section_count_and_compact_json_sizes_are_bounded() {
    let section = sections().into_values().next().unwrap();
    let mut many: MetadataSections = (0..8)
        .map(|index| (format!("test.section-{index}"), section.clone()))
        .collect();
    validate_sections(&many).unwrap();
    many.insert("test.ninth".into(), section.clone());
    assert!(matches!(validate_sections(&many), Err(Error::Limit)));

    let mut large = section;
    large.data = json!({"padding": "x".repeat(16 * 1024)});
    assert!(matches!(
        validate_sections(&[("test.large".into(), large.clone())].into_iter().collect()),
        Err(Error::Limit)
    ));
    large.data = json!({"padding": "x".repeat(12 * 1024)});
    let mut total = MetadataSections::new();
    total.insert("test.first".into(), large.clone());
    total.insert("test.second".into(), large.clone());
    validate_sections(&total).unwrap();
    total.insert("test.third".into(), large);
    assert!(matches!(validate_sections(&total), Err(Error::Limit)));
}

#[test]
fn section_nesting_includes_every_envelope_and_the_metadata_root() {
    for (location, enclosing_depth) in [
        ("data", 3),
        ("section", 3),
        ("producer", 4),
        ("observation", 4),
        ("evidence", 5),
    ] {
        for object in [false, true] {
            for extra_depth in [0, 1, 5] {
                let supported = extra_depth == 0;
                let mut value = Value::Null;
                for _ in 0..(127 - enclosing_depth + extra_depth) {
                    value = if object {
                        Value::Object([("nested".into(), value)].into_iter().collect())
                    } else {
                        Value::Array(vec![value])
                    };
                }
                let mut section = local_section(&mut sections()).clone();
                match location {
                    "data" => section.data = value,
                    "section" => {
                        section.unknown_fields.insert("future".into(), value);
                    }
                    "producer" => {
                        section
                            .producer
                            .unknown_fields
                            .insert("future".into(), value);
                    }
                    "observation" => {
                        let MetadataObservation::ObservedAt { unknown_fields, .. } =
                            &mut section.observation
                        else {
                            panic!("expected local observation");
                        };
                        unknown_fields.insert("future".into(), value);
                    }
                    "evidence" => {
                        let MetadataEvidence::LocalObservation { unknown_fields, .. } =
                            &mut section.evidence[0]
                        else {
                            panic!("expected local evidence");
                        };
                        unknown_fields.insert("future".into(), value);
                    }
                    _ => unreachable!(),
                }
                let MetadataEvidence::LocalObservation { data_sha256, .. } =
                    &mut section.evidence[0]
                else {
                    panic!("expected local evidence");
                };
                *data_sha256 = sha(&serde_json::to_vec(&section.data).unwrap());
                let sections = [("test.nesting".into(), section)].into_iter().collect();
                let metadata = CaptureMetadata {
                    version: 1,
                    agent: "claude-code".into(),
                    source_format: "ClaudeJsonl".into(),
                    native_session_id: "synthetic-session".into(),
                    native_agent_id: None,
                    source_agent_version: None,
                    version_observations: vec![],
                    version_scan_complete: false,
                    owned_version_complete: false,
                    relationship: Relationship::Unknown,
                    history_completeness: "unknown".into(),
                    sections,
                };
                let validation = validate_sections(&metadata.sections);
                if supported {
                    validation.unwrap();
                    metadata.stable_fingerprint().unwrap();
                } else {
                    assert!(
                        matches!(validation, Err(Error::Limit)),
                        "{location}, object={object}, extra_depth={extra_depth}: {validation:?}"
                    );
                    assert!(metadata.stable_fingerprint().is_err());
                }
            }
        }
    }
}

#[test]
fn native_evidence_requires_exact_unique_native_stream_and_digest() {
    let sections = sections();
    assert!(matches!(
        validate_section_bindings(&sections, &[]),
        Err(Error::InvalidMetadata)
    ));
    for changed in ["id", "digest", "role", "length"] {
        let mut stream = native_stream();
        match changed {
            "id" => stream.stream_id = "other".into(),
            "digest" => stream.expanded_sha256 = "a".repeat(64),
            "role" => stream.role = "capture_metadata".into(),
            "length" => stream.expanded_bytes = 0,
            _ => unreachable!(),
        }
        assert!(matches!(
            validate_section_bindings(&sections, &[stream]),
            Err(Error::InvalidMetadata)
        ));
    }
    assert!(matches!(
        validate_section_bindings(&sections, &[native_stream(), native_stream()]),
        Err(Error::InvalidMetadata)
    ));
}

#[test]
fn source_position_is_strictly_inside_the_stream_and_requires_its_evidence() {
    let mut sections = sections();
    let MetadataObservation::SourcePosition {
        expanded_offset, ..
    } = &mut native_section(&mut sections).observation
    else {
        panic!("expected source position");
    };
    *expanded_offset = 389;
    validate_section_bindings(&sections, &[native_stream()]).unwrap();
    let MetadataObservation::SourcePosition {
        expanded_offset, ..
    } = &mut native_section(&mut sections).observation
    else {
        unreachable!();
    };
    *expanded_offset = 390;
    assert!(matches!(
        validate_section_bindings(&sections, &[native_stream()]),
        Err(Error::InvalidMetadata)
    ));
    native_section(&mut sections).observation = MetadataObservation::SourcePosition {
        stream_id: "other".into(),
        expanded_offset: 0,
        unknown_fields: Default::default(),
    };
    assert!(matches!(
        validate_sections(&sections),
        Err(Error::InvalidMetadata)
    ));
}

#[test]
fn local_evidence_binds_data_and_does_not_prove_a_historical_position() {
    let mut sections = sections();
    local_section(&mut sections).data["historical_checkout"] = json!(true);
    assert!(matches!(
        validate_sections(&sections),
        Err(Error::InvalidMetadata)
    ));
    let mut sections = self::sections();
    let local = local_section(&mut sections).clone();
    native_section(&mut sections).evidence = local.evidence;
    native_section(&mut sections).data = local.data;
    assert!(matches!(
        validate_sections(&sections),
        Err(Error::InvalidMetadata)
    ));
    let mut sections = self::sections();
    let evidence = native_section(&mut sections).evidence.clone();
    local_section(&mut sections).evidence = evidence;
    assert!(matches!(
        validate_sections(&sections),
        Err(Error::InvalidMetadata)
    ));
}

#[test]
fn identifiers_timestamps_digests_and_evidence_counts_are_checked() {
    let original = serde_json::to_value(sections()).unwrap();
    for (path, invalid) in [
        ("/schema", json!("unsafe/schema")),
        ("/producer/name", json!("unsafe name")),
        ("/producer/version", json!("1/2")),
        ("/observation/unix_millis", json!(0)),
        ("/observation/unix_millis", json!(253_402_300_800_000_u64)),
        ("/evidence/0/method", json!("unsafe/method")),
        ("/evidence/0/data_sha256", json!("A".repeat(64))),
    ] {
        let mut value = original.clone();
        *value["antiburn.capture-checkout"]
            .pointer_mut(path)
            .unwrap() = invalid;
        let sections = serde_json::from_value(value).unwrap();
        assert!(matches!(
            validate_sections(&sections),
            Err(Error::InvalidMetadata)
        ));
    }
    let mut sections = sections();
    let local = local_section(&mut sections);
    local.evidence = vec![local.evidence[0].clone(); 8];
    validate_sections(&sections).unwrap();
    let local = local_section(&mut sections);
    local.evidence.push(local.evidence[0].clone());
    assert!(matches!(validate_sections(&sections), Err(Error::Limit)));
}

#[test]
fn unknown_fields_roundtrip_at_every_envelope_level_and_in_data() {
    let mut value = serde_json::to_value(sections()).unwrap();
    for section in value.as_object_mut().unwrap().values_mut() {
        section["future_section"] = json!({"enabled": true});
        section["producer"]["future_producer"] = json!(7);
        section["observation"]["future_observation"] = json!("retained");
        section["evidence"][0]["future_evidence"] = json!([1, 2]);
    }
    let parsed: MetadataSections = serde_json::from_value(value.clone()).unwrap();
    validate_sections(&parsed).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), value);
    let section: MetadataSection = serde_json::from_value(json!({
        "schema": "test.v1",
        "producer": {"name": "test", "version": "1"},
        "observation": {"kind": "observed_at", "unix_millis": 1},
        "evidence": [],
        "data": {"unknown_data": {"nested": [null, false, "value"]}}
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(&section).unwrap()["data"],
        json!({"unknown_data": {"nested": [null, false, "value"]}})
    );
    let mut sections = sections();
    local_section(&mut sections)
        .unknown_fields
        .insert("schema".into(), Value::Null);
    assert!(matches!(
        validate_sections(&sections),
        Err(Error::InvalidMetadata)
    ));
}
