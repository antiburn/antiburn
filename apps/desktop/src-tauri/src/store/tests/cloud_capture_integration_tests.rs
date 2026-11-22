use super::*;

fn claude_root(path: &str) -> SourceSelection {
    SourceSelection {
        source_kind: SourceKind::ClaudeJsonl,
        path: path.into(),
    }
}

#[test]
fn cloud_provider_wire_values_match_native_candidate_kinds() {
    for (kind, wire) in [
        (CaptureSourceKind::ClaudeJsonl, "\"claude_jsonl\""),
        (
            CaptureSourceKind::OpenCodeSqliteV2,
            "\"opencode_sqlite_v2\"",
        ),
        (CaptureSourceKind::CodexJsonl, "\"codex_jsonl\""),
        (CaptureSourceKind::PiJsonl, "\"pi_jsonl\""),
    ] {
        let native = SourceKind::from(kind);
        assert_eq!(serde_json::to_string(&native).unwrap(), wire);
        assert_eq!(serde_json::from_str::<SourceKind>(wire).unwrap(), native);
    }
    assert!(serde_json::from_str::<SourceKind>("\"unknown\"").is_err());
}

#[test]
fn cloud_request_ids_and_account_root_scopes_survive_restart_and_grant_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let roots = vec![claude_root("/Example/Native")];
    let first = store
        .prepare_cloud_capture(
            "request-one",
            "account-one",
            "personal-one",
            &roots,
            CONSENT_VERSION,
            1,
        )
        .unwrap();
    let next = store
        .prepare_cloud_capture(
            "request-two",
            "account-one",
            "personal-one",
            &roots,
            CONSENT_VERSION,
            2,
        )
        .unwrap();
    assert_eq!(first.roots, next.roots);
    let another = store
        .prepare_cloud_capture(
            "request-three",
            "account-two",
            "personal-two",
            &roots,
            CONSENT_VERSION,
            3,
        )
        .unwrap();
    assert_ne!(first.roots, another.roots);
    assert!(
        store
            .prepare_cloud_capture(
                "request-one",
                "account-two",
                "personal-two",
                &roots,
                CONSENT_VERSION,
                4
            )
            .is_err()
    );
    assert!(
        store
            .prepare_cloud_capture(
                "request-one",
                "account-one",
                "another-destination",
                &roots,
                CONSENT_VERSION,
                4
            )
            .is_err()
    );
    assert!(
        store
            .prepare_cloud_capture(
                "request-one",
                "account-one",
                "personal-one",
                &[claude_root("/Example/Other")],
                CONSENT_VERSION,
                4
            )
            .is_err()
    );
    store.cancel_cloud_capture_request(&first).unwrap();
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let repeated = store
        .prepare_cloud_capture(
            "request-one",
            "account-one",
            "personal-one",
            &roots,
            CONSENT_VERSION,
            10,
        )
        .unwrap();
    assert!(repeated.cancelled);
    assert_eq!(repeated.roots, first.roots);
    assert!(
        store
            .cloud_capture_request("request-one", "account-two", "personal-two")
            .is_err()
    );
}

#[test]
fn cloud_request_insert_failure_rolls_back_new_scope_keys() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    store
        .lock()
        .execute_batch(
            "CREATE TRIGGER reject_cloud_request BEFORE INSERT ON cloud_capture_request
             BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
        )
        .unwrap();
    assert!(
        store
            .prepare_cloud_capture(
                "request",
                "account",
                "personal",
                &[claude_root("/Example/Native")],
                CONSENT_VERSION,
                1
            )
            .is_err()
    );
    let scopes: u64 = store
        .lock()
        .query_row("SELECT count(*) FROM cloud_source_scope", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(scopes, 0);
}

#[test]
fn cloud_legacy_saved_requests_keep_exact_json_and_identities_without_source_access() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let raw = r#"{"request_id":"legacy","account_id":"account","destination_id":"personal","consent_version":"native-full-content-v1","roots":[{"path":"/Example/missing","source_scope_id":"old-scope"}],"sources":[{"identity":{"capture_id":"old-capture","account_id":"account","destination_id":"personal","source_id":"old-scope/session"},"transcript_path":"/Example/missing/session.jsonl"}],"discovery_complete":true,"cancelled":false}"#;
    store
        .lock()
        .execute(
            "INSERT INTO cloud_capture_request VALUES ('legacy', 'account', 'personal', 1, ?1)",
            [raw],
        )
        .unwrap();
    let request = store
        .cloud_capture_request("legacy", "account", "personal")
        .unwrap();
    assert_eq!(serde_json::to_string(&request).unwrap(), raw);
    let roots = [claude_root("/Example/missing")];
    assert_eq!(
        store
            .prepare_cloud_capture("legacy", "account", "personal", &roots, CONSENT_VERSION, 2)
            .unwrap(),
        request
    );
    assert_eq!(
        store
            .current_cloud_capture_request("account", "personal", &roots)
            .unwrap(),
        Some(request.clone())
    );
    assert_eq!(
        store.finish_cloud_discovery(&request, Vec::new()).unwrap(),
        request
    );
    let saved: String = store
        .lock()
        .query_row(
            "SELECT record_json FROM cloud_capture_request WHERE request_id = 'legacy'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(saved, raw);
}

#[test]
fn cloud_request_ids_cannot_change_provider_or_consent_version() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let roots = [claude_root("/Example/source")];
    let original = store
        .prepare_cloud_capture("request", "account", "personal", &roots, CONSENT_VERSION, 1)
        .unwrap();
    assert!(
        store
            .prepare_cloud_capture(
                "request",
                "account",
                "personal",
                &roots,
                SOURCE_CONSENT_VERSION,
                2
            )
            .is_err()
    );
    let changed = [SourceSelection {
        source_kind: SourceKind::OpenCodeSqliteV2,
        path: roots[0].path.clone(),
    }];
    assert!(
        store
            .prepare_cloud_capture(
                "request",
                "account",
                "personal",
                &changed,
                SOURCE_CONSENT_VERSION,
                2
            )
            .is_err()
    );
    assert!(
        store
            .current_cloud_capture_request("account", "personal", &changed)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .cloud_capture_request("request", "account", "personal")
            .unwrap(),
        original
    );
}

#[test]
fn cloud_unknown_or_retargeted_saved_locators_fail_without_rewriting_records() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    store.initialize_cloud_setup().unwrap();
    let roots = [SourceSelection {
        source_kind: SourceKind::OpenCodeSqliteV2,
        path: "/Example/opencode.db".into(),
    }];
    let request = store
        .prepare_cloud_capture(
            "request",
            "account",
            "personal",
            &roots,
            SOURCE_CONSENT_VERSION,
            1,
        )
        .unwrap();
    let source = SelectedSource::Located {
        identity: CaptureIdentity {
            capture_id: "capture".into(),
            account_id: "account".into(),
            destination_id: "personal".into(),
            source_id: format!(
                "{}/opencode/b3BlbmNvZGUuZGI/c2VzX21haW4",
                request.roots[0].source_scope_id
            ),
        },
        locator: SourceLocator {
            source_kind: SourceKind::OpenCodeSqliteV2,
            source_scope_id: request.roots[0].source_scope_id.clone(),
            relative_path: "opencode.db".into(),
            native_session_id: "ses_main".into(),
            native_agent_id: None,
        },
    };
    let saved = store
        .finish_cloud_discovery(&request, vec![source])
        .unwrap();
    assert_eq!(
        store
            .cloud_capture_request("request", "account", "personal")
            .unwrap(),
        saved
    );
    let raw = serde_json::to_string(&saved).unwrap();
    for changed in [
        raw.replace(
            "\"native_session_id\":\"ses_main\"",
            "\"native_session_id\":\"ses_child\"",
        ),
        raw.replace("opencode_sqlite_v2", "unknown_source"),
        raw.replace(
            "\"relative_path\":\"opencode.db\"",
            "\"relative_path\":\"other.db\"",
        ),
        raw.replace(
            "\"relative_path\":\"opencode.db\"",
            "\"relative_path\":\"../opencode.db\"",
        ),
        raw.replace(
            "\"native_agent_id\":null",
            "\"native_agent_id\":\"persona\"",
        ),
    ] {
        assert_rejected_without_rewrite(&store, &saved, &roots, &changed);
    }
}

fn assert_rejected_without_rewrite(
    store: &Store,
    saved: &CaptureRequest,
    roots: &[SourceSelection],
    changed: &str,
) {
    store
        .lock()
        .execute(
            "UPDATE cloud_capture_request SET record_json = ?1 WHERE request_id = ?2",
            params![changed, saved.request_id],
        )
        .unwrap();
    assert!(
        store
            .cloud_capture_request(&saved.request_id, &saved.account_id, &saved.destination_id)
            .is_err()
    );
    assert!(
        store
            .current_cloud_capture_request(&saved.account_id, &saved.destination_id, roots)
            .is_err()
    );
    assert!(store.cancel_cloud_capture_request(saved).is_err());
    let unchanged: String = store
        .lock()
        .query_row(
            "SELECT record_json FROM cloud_capture_request WHERE request_id = ?1",
            [&saved.request_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(unchanged, changed);
}

#[test]
fn cloud_typed_claude_locators_reject_changed_paths_sessions_and_child_ids_without_rewrite() {
    for (agent, relative_path, native_key) in [
        (None, "session-main.jsonl", "session-main"),
        (
            Some("child"),
            "session-main/subagents/agent-child.jsonl",
            "session-main/subagents/agent-child",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        let roots = [claude_root("/Example/claude")];
        let request = store
            .prepare_cloud_capture("request", "account", "personal", &roots, CONSENT_VERSION, 1)
            .unwrap();
        let source = SelectedSource::Located {
            identity: CaptureIdentity {
                capture_id: "frozen-capture".into(),
                account_id: "account".into(),
                destination_id: "personal".into(),
                source_id: format!("{}/{native_key}", request.roots[0].source_scope_id),
            },
            locator: SourceLocator {
                source_kind: SourceKind::ClaudeJsonl,
                source_scope_id: request.roots[0].source_scope_id.clone(),
                relative_path: relative_path.into(),
                native_session_id: "session-main".into(),
                native_agent_id: agent.map(str::to_owned),
            },
        };
        let saved = store
            .finish_cloud_discovery(&request, vec![source])
            .unwrap();
        assert_eq!(
            store
                .cloud_capture_request("request", "account", "personal")
                .unwrap(),
            saved
        );
        assert_eq!(
            store
                .current_cloud_capture_request("account", "personal", &roots)
                .unwrap(),
            Some(saved.clone())
        );
        let raw = serde_json::to_string(&saved).unwrap();
        for changed in [
            raw.replace(
                &format!("\"relative_path\":\"{relative_path}\""),
                "\"relative_path\":\"session-other.jsonl\"",
            ),
            raw.replace(
                "\"native_session_id\":\"session-main\"",
                "\"native_session_id\":\"session-other\"",
            ),
            raw.replace(
                &format!(
                    "\"native_agent_id\":{}",
                    serde_json::to_string(&agent).unwrap()
                ),
                "\"native_agent_id\":\"other\"",
            ),
        ] {
            assert_ne!(changed, raw);
            assert_rejected_without_rewrite(&store, &saved, &roots, &changed);
        }
    }
}
