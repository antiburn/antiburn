use super::*;

#[tokio::test]
async fn restored_remote_snapshot_refreshes_the_fingerprint_and_reaches_ready() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(temp.path()).unwrap();
    let data = br#"{"type":"user","uuid":"u1","timestamp":"2026-09-15T10:00:00Z","message":{"role":"user","content":"Review this synthetic transcript"}}
{"type":"assistant","uuid":"a1","timestamp":"2026-09-15T10:00:01Z","message":{"id":"m1","role":"assistant","model":"claude-sonnet-4-6","content":[{"type":"text","text":"Reviewed"}],"usage":{"input_tokens":10,"output_tokens":2}}}
"#;
    let manifest = manifest(data);
    let bundle = temp.path().join("bundle");
    transfer(&bundle, &manifest, data, false);
    let snapshot = temp.path().join("snapshot");
    unpack(&bundle, &snapshot, &manifest.session).unwrap();
    let path = snapshot.join(manifest.file_name(0));
    let mut record = cached_record(&path);
    let original_fingerprint = parent_fingerprint(&path).unwrap();
    record.source_fingerprint = Some(original_fingerprint.clone());
    let key = record.key.clone();
    store
        .upsert_confirmed_session(&record, &crate::agents::evidence_cohort())
        .unwrap();

    // Keep the original file allocated so the replacement has a different identity.
    fs::rename(&path, snapshot.join("original")).unwrap();
    let clock = || 100;
    let runner = |record: &SessionRecord, signal, fence| {
        run_pass(record.clone(), signal, fence, store.clone())
    };
    assert!(
        crate::insights_worker::process_next(&store, &clock, &runner, &|_| {}, &|_, _| {}, &|| {},)
            .await
            .unwrap()
    );
    assert_eq!(
        store
            .sessions_with_missing_source_for(std::slice::from_ref(&key))
            .unwrap(),
        vec![key.clone()]
    );
    fs::write(&path, data).unwrap();
    let restored_fingerprint = parent_fingerprint(&path).unwrap();
    assert_ne!(restored_fingerprint, original_fingerprint);

    confirm_restored_snapshot(&store, record).unwrap();
    assert_eq!(
        store.session(&key).unwrap().unwrap().source_fingerprint,
        Some(restored_fingerprint)
    );
    assert!(
        crate::insights_worker::process_next(&store, &clock, &runner, &|_| {}, &|_, _| {}, &|| {},)
            .await
            .unwrap()
    );
    assert_eq!(
        store.evidence(&key).unwrap().unwrap().status,
        crate::store::EvidenceStatus::Ready
    );
}
