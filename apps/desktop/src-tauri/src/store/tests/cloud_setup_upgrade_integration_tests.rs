use super::Store;

#[test]
fn fresh_cloud_setup_does_not_create_retired_request_schema() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let retired_objects: usize = store
        .lock()
        .query_row(
            "SELECT count(*) FROM sqlite_schema
             WHERE name IN ('cloud_capture_request', 'cloud_capture_request_owner', 'cloud_source_scope')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retired_objects, 0);
}

#[test]
fn cloud_setup_upgrade_preserves_completed_request_without_enabling_uploads() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    // Use a frozen copy of a completed v1 request.
    let request = r#"{"request_id":"legacy","account_id":"account","destination_id":"personal","consent_version":"native-full-content-v1","roots":[{"path":"/Example/missing","source_scope_id":"old-scope"}],"sources":[{"identity":{"capture_id":"old-capture","account_id":"account","destination_id":"personal","source_id":"old-scope/session"},"transcript_path":"/Example/missing/session.jsonl"}],"discovery_complete":true,"cancelled":false}"#;
    {
        let connection = store.lock();
        connection
            .execute_batch(
                "CREATE TABLE cloud_setup (
                    account_id TEXT NOT NULL,
                    workspace_id TEXT NOT NULL,
                    draft_json TEXT NOT NULL CHECK(length(draft_json) <= 524288),
                    PRIMARY KEY (account_id, workspace_id)
                );
                CREATE TABLE cloud_source_scope (
                    account_id TEXT NOT NULL,
                    root_path TEXT NOT NULL,
                    source_scope_id TEXT NOT NULL UNIQUE,
                    PRIMARY KEY (account_id, root_path)
                );
                CREATE TABLE cloud_capture_request (
                    request_id TEXT PRIMARY KEY,
                    account_id TEXT NOT NULL,
                    destination_id TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    record_json TEXT NOT NULL CHECK(length(record_json) <= 1048576)
                );
                CREATE INDEX cloud_capture_request_owner
                    ON cloud_capture_request(account_id, destination_id, created_at);
                INSERT INTO cloud_source_scope VALUES ('account', '/Example/missing', 'old-scope');",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cloud_capture_request VALUES ('legacy', 'account', 'personal', 1, ?1)",
                [request],
            )
            .unwrap();
    }
    drop(store);

    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let origin = "https://cloud.example";
    assert!(
        store
            .cloud_sync_policy_for_origins("account", "personal", origin, origin)
            .unwrap()
            .is_none()
    );
    assert!(!store.cloud_device_upload_enabled().unwrap());

    let (saved_request, scope_id): (Vec<u8>, String) = store
        .lock()
        .query_row(
            "SELECT CAST(request.record_json AS BLOB), scope.source_scope_id
             FROM cloud_capture_request request INDEXED BY cloud_capture_request_owner
             JOIN cloud_source_scope scope USING (account_id)
             WHERE request.request_id = 'legacy' AND request.account_id = 'account'
               AND request.destination_id = 'personal' AND request.created_at = 1
               AND scope.root_path = '/Example/missing'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(saved_request, request.as_bytes());
    assert_eq!(scope_id, "old-scope");
}
