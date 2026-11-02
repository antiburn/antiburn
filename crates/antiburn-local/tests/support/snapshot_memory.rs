use super::analysis::{
    AdapterSnapshot, MAX_NATIVE_EVIDENCE_BYTES, MAX_NATIVE_EVIDENCE_OBSERVATIONS, StreamSnapshot,
};

pub fn encoded_size(value: &impl serde::Serialize) -> usize {
    postcard::to_allocvec(value)
        .expect("memory test value encodes")
        .len()
}

pub fn generic_evidence_bound() -> usize {
    MAX_NATIVE_EVIDENCE_BYTES
        + MAX_NATIVE_EVIDENCE_OBSERVATIONS * encoded_size(&MAX_NATIVE_EVIDENCE_BYTES)
        + encoded_size(&MAX_NATIVE_EVIDENCE_OBSERVATIONS)
        + encoded_size(&false)
}

pub fn assert_snapshot_bounds(
    snapshot: &StreamSnapshot,
    legacy_adapter: AdapterSnapshot,
    legacy_bound: usize,
    public_adapter_bytes: usize,
    public_adapter_bound: usize,
) {
    assert_eq!(
        snapshot.adapter.0.len(),
        legacy_adapter.0.len() + public_adapter_bytes
    );
    assert!(public_adapter_bytes <= public_adapter_bound);
    assert!(snapshot.native_evidence_json.len() <= MAX_NATIVE_EVIDENCE_OBSERVATIONS);
    let payload_bytes: usize = snapshot.native_evidence_json.iter().map(String::len).sum();
    assert!(payload_bytes <= MAX_NATIVE_EVIDENCE_BYTES);
    let generic_bytes = encoded_size(&(
        &snapshot.native_evidence_json,
        snapshot.native_evidence_complete,
    ));
    assert!(generic_bytes <= generic_evidence_bound());

    // Keep the fixture's old component ceiling. Empty added fields remain in it.
    let mut legacy = snapshot.clone();
    legacy.adapter = legacy_adapter;
    legacy.native_evidence_json.clear();
    legacy.native_evidence_complete = true;
    legacy.native_admission_failure_json = None;
    let legacy_bytes = legacy.encode().len();
    assert!(
        legacy_bytes <= legacy_bound,
        "legacy snapshot is {legacy_bytes} bytes; bound is {legacy_bound}"
    );
    let empty_generic_bytes = encoded_size(&(Vec::<String>::new(), true));
    let adapter_prefix_bytes =
        encoded_size(&snapshot.adapter.0.len()) - encoded_size(&legacy.adapter.0.len());
    let failure_bytes = encoded_size(&snapshot.native_admission_failure_json)
        - encoded_size(&Option::<String>::None);
    assert_eq!(
        snapshot.encode().len(),
        legacy_bytes + generic_bytes - empty_generic_bytes
            + public_adapter_bytes
            + adapter_prefix_bytes
            + failure_bytes
    );

    let adapter_prefix_bound = encoded_size(&(legacy.adapter.0.len() + public_adapter_bound))
        - encoded_size(&legacy.adapter.0.len());
    let failure = super::analysis::NativeAdmissionFailure {
        source_format: super::analysis::SourceFormat::AntigravityWorkspaceChatJson,
        session_id: "\0".repeat(4096),
        position: super::analysis::ToolResultPosition::FileBytes {
            record_start: u64::MAX,
            record_end: u64::MAX,
            item_index: u32::MAX,
        },
        reason: super::analysis::ToolResultCompleteness::UnsafeOwnership,
    };
    let failure_bound = encoded_size(&Some(serde_json::to_string(&failure).unwrap()))
        - encoded_size(&Option::<String>::None);
    assert!(failure_bytes <= failure_bound);
    let total_bound = legacy_bound + generic_evidence_bound() - empty_generic_bytes
        + public_adapter_bound
        + adapter_prefix_bound
        + failure_bound;
    assert!(snapshot.encode().len() <= total_bound);
    assert_eq!(
        StreamSnapshot::decode(&snapshot.encode()).unwrap().encode(),
        snapshot.encode()
    );
    eprintln!(
        "snapshot bytes: legacy={legacy_bytes}, generic={generic_bytes}, public_adapter={public_adapter_bytes}, failure_state={failure_bytes}, adapter_prefix={adapter_prefix_bytes}, total={}, bound={total_bound}",
        snapshot.encode().len()
    );
}
