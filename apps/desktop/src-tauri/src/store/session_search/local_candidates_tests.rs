use std::path::Path;

use super::{SESSION_SEARCH_BUDGET_ERROR, SessionSearchResult};
use crate::store::{AnalysisRecord, SessionKey, SessionRecord, Store, apply_session_retention_in};

fn store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-local-candidates-test"))
        .expect("opens candidate store")
}

fn session(id: &str, title: &str, updated_at_epoch: i64) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new("native", "codex", id),
        source_kind: "file".to_owned(),
        source_label: format!("/synthetic/{id}.jsonl"),
        wsl_distro: None,
        title: Some(title.to_owned()),
        title_source: Some("vendor".to_owned()),
        cwd: Some("/synthetic/project".to_owned()),
        surface: "cli".to_owned(),
        updated_at_epoch: Some(updated_at_epoch),
        activity_cursor: format!("{updated_at_epoch}:1"),
        activity_source: "event".to_owned(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: None,
    }
}

fn analysis(key: SessionKey, model: &str) -> AnalysisRecord {
    AnalysisRecord {
        key,
        model_breakdown_json: "{}".to_owned(),
        pricing_breakdown_json: "{}".to_owned(),
        inclusive_models_json: serde_json::json!([{ "model": model }]).to_string(),
        initial_context_json: None,
        source_summaries_json: None,
        provider_hints_json: None,
        source_fingerprint: "1:1".to_owned(),
        pricing_generation: 1,
        analyzed_generation: 1,
        parser_revision: 1,
        analyzer_revision: 1,
        metrics_schema_revision: 1,
    }
}

fn candidates(store: &Store, query: &str) -> Vec<SessionSearchResult> {
    let (results, generation) = store
        .local_session_candidates(query, None)
        .expect("searches local candidates");
    assert_eq!(
        generation,
        store.session_search_generation().expect("reads generation")
    );
    results
}

#[test]
fn exact_id_and_title_precede_newer_prefixes_regardless_of_case() {
    let store = store();
    let mut records = vec![
        session("ALPHA", "Work on the old identifier", 1),
        session("exact-title", "Alpha", 2),
    ];
    records.extend((0..20).map(|index| {
        session(
            &format!("prefix-{index:02}"),
            "Alphabet alphabet alphabet",
            100 + index,
        )
    }));
    store
        .upsert_sessions(&records, &[])
        .expect("indexes fixtures");

    let results = candidates(&store, "  alpha  ");
    assert_eq!(results.len(), 8);
    assert_eq!(results[0].session_id, "ALPHA");
    assert_eq!(results[1].session_id, "exact-title");
}

#[test]
fn old_matches_outside_the_latest_five_hundred_sessions_remain_candidates() {
    let store = store();
    let mut records = vec![session("historical", "Rare aurora fix", 1)];
    records.extend((0..600).map(|index| {
        session(
            &format!("recent-{index:03}"),
            "Routine activity",
            100 + index,
        )
    }));
    store
        .upsert_sessions(&records, &[])
        .expect("indexes history");

    let results = candidates(&store, "aurora");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].session_id, "historical");
    assert_eq!(candidates(&store, "routine").len(), 8);
}

#[test]
fn natural_queries_strip_filler_and_match_any_remaining_prefix() {
    let store = store();
    store
        .upsert_sessions(
            &[
                session("migration", "PostgreSQL migrations", 1),
                session("compile", "Compiler diagnostics", 2),
                session("noise", "Find my session conversation", 3),
            ],
            &[],
        )
        .expect("indexes natural query fixtures");

    let results = candidates(
        &store,
        "Please FIND me the sessions about postgr compi missingword",
    );
    let ids = results
        .iter()
        .map(|result| result.session_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        ids,
        std::collections::HashSet::from(["migration", "compile"])
    );
    assert!(candidates(&store, "Please find me my sessions").is_empty());
    assert!(candidates(&store, " \n\t ").is_empty());
}

#[test]
fn bm25_places_a_multi_term_match_before_newer_single_term_matches() {
    let store = store();
    store
        .upsert_sessions(
            &[
                session("both", "Aurora nebula", 1),
                session("first", "Aurora routine", 3),
                session("second", "Nebula routine", 2),
            ],
            &[],
        )
        .expect("indexes relevance fixtures");

    let results = candidates(&store, "aurora nebula");
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].session_id, "both");
}

#[test]
fn quotes_and_operators_are_search_text_and_cannot_change_query_structure() {
    let store = store();
    store
        .upsert_sessions(
            &[
                session("operator", "NOT NEAR OR", 1),
                session("needle", "Aurora", 2),
                session("unrelated", "Unrelated work", 3),
            ],
            &[],
        )
        .expect("indexes syntax fixtures");

    for query in ["NOT", "NEAR", "OR"] {
        let results = candidates(&store, query);
        assert_eq!(results.len(), 1, "query: {query}");
        assert_eq!(results[0].session_id, "operator");
    }
    for query in ["\"aurora\"", "\"aurora", "aurora\""] {
        let results = candidates(&store, query);
        assert_eq!(results.len(), 1, "query: {query}");
        assert_eq!(results[0].session_id, "needle");
    }
    for query in ["\"", "(", ")", "-", "*", "title:missing"] {
        assert!(candidates(&store, query).is_empty(), "query: {query}");
    }
    let results = candidates(&store, "' OR 1=1 --");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].session_id, "operator");
}

#[test]
fn query_length_counts_characters_and_retains_only_twelve_search_terms() {
    let store = store();
    store
        .upsert_sessions(&[session("beyond-limit", "thirteenth", 1)], &[])
        .expect("indexes term limit fixture");

    assert!(candidates(&store, &"é".repeat(200)).is_empty());
    assert!(
        store
            .local_session_candidates(&"é".repeat(201), None)
            .is_err()
    );
    assert!(candidates(&store, "aa bb cc dd ee ff gg hh ii jj kk ll thirteenth").is_empty());
    assert_eq!(
        candidates(&store, "aa bb cc dd ee ff gg hh ii jj kk thirteenth").len(),
        1
    );
}

#[test]
fn shared_ids_keep_their_environment_agent_and_model_identity() {
    let store = store();
    let native = session("shared", "Scoped candidate", 1);
    let mut wsl = native.clone();
    wsl.key.environment_key = "wsl:ubuntu-24.04".to_owned();
    wsl.wsl_distro = Some("Ubuntu-24.04".to_owned());
    let mut other_agent = native.clone();
    other_agent.key.agent = "claude-code".to_owned();
    let mut no_analysis = native.clone();
    no_analysis.key.agent = "copilot".to_owned();
    store
        .upsert_sessions(
            &[
                native.clone(),
                wsl.clone(),
                other_agent.clone(),
                no_analysis,
            ],
            &[],
        )
        .expect("indexes scoped identities");
    for (record, model) in [
        (&native, "native-model"),
        (&wsl, "wsl-model"),
        (&other_agent, "other-model"),
    ] {
        store
            .save_analysis(&analysis(record.key.clone(), model), None)
            .expect("publishes scoped model");
    }

    let results = candidates(&store, "shared");
    assert_eq!(results.len(), 4);
    for result in results {
        let expected_model = match (result.environment_key.as_str(), result.agent.as_str()) {
            ("native", "codex") => {
                assert_eq!(result.wsl_distro, None);
                vec!["native-model"]
            }
            ("wsl:ubuntu-24.04", "codex") => {
                assert_eq!(result.wsl_distro.as_deref(), Some("Ubuntu-24.04"));
                vec!["wsl-model"]
            }
            ("native", "claude-code") => vec!["other-model"],
            ("native", "copilot") => vec![],
            identity => panic!("unexpected identity: {identity:?}"),
        };
        assert_eq!(result.models, expected_model);
    }
}

#[test]
fn deletion_changes_generation_and_removes_only_the_exact_session_key() {
    let store = store();
    let native = session("shared", "Erasable candidate", 1);
    let mut wsl = native.clone();
    wsl.key.environment_key = "wsl:ubuntu-24.04".to_owned();
    wsl.wsl_distro = Some("Ubuntu-24.04".to_owned());
    store
        .upsert_sessions(&[native.clone(), wsl], &[])
        .expect("indexes deletion fixtures");
    let (before, generation) = store.local_session_candidates("erasable", None).unwrap();
    assert_eq!(before.len(), 2);

    assert!(store.delete_session(&native.key).unwrap().is_some());
    let (after, changed_generation) = store.local_session_candidates("erasable", None).unwrap();
    assert!(changed_generation > generation);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].environment_key, "wsl:ubuntu-24.04");
    assert_eq!(
        changed_generation,
        store.session_search_generation().unwrap()
    );
    assert!(store.delete_session(&native.key).unwrap().is_none());
    assert_eq!(
        changed_generation,
        store.session_search_generation().unwrap()
    );
}

#[test]
fn retention_and_clear_advance_generation_without_leaving_candidates() {
    let store = store();
    store
        .upsert_sessions(
            &[
                session("expired", "Erasable candidate", 1),
                session("retained", "Erasable candidate", 2_000_000_000),
            ],
            &[],
        )
        .expect("indexes retention fixtures");
    let initial_generation = store.session_search_generation().unwrap();
    assert_eq!(
        apply_session_retention_in(&store.lock(), 30, 2_000_000_000).unwrap(),
        1
    );
    let (retained, retained_generation) = store.local_session_candidates("erasable", None).unwrap();
    assert!(retained_generation > initial_generation);
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].session_id, "retained");

    assert_eq!(store.clear_local_session_data().unwrap().0, 1);
    let (cleared, cleared_generation) = store.local_session_candidates("erasable", None).unwrap();
    assert!(cleared_generation > retained_generation);
    assert!(cleared.is_empty());
    assert_eq!(
        cleared_generation,
        store.session_search_generation().unwrap()
    );
    store.rebuild_session_search_index().unwrap();
    assert!(candidates(&store, "erasable").is_empty());
}

#[test]
fn transcript_content_does_not_match_or_enter_candidate_metadata() {
    let store = store();
    store
        .upsert_sessions(&[session("metadata", "Metadata title", 1)], &[])
        .expect("indexes metadata fixture");
    let transcript = "Synthetictranscriptonlyneedle";
    {
        let connection = store.lock();
        connection
            .execute_batch(
                "INSERT INTO turn (
                     environment_key, agent, session_id, claim_fence, source_key,
                     thread_id, turn_index, scope, role, input_tokens,
                     cache_read_tokens, cache_write_tokens, output_tokens,
                     is_compaction_boundary
                 ) VALUES (
                     'native', 'codex', 'metadata', 1, 'synthetic',
                     'main', 0, 'main', 'user', 0, 0, 0, 0, 0
                 );",
            )
            .expect("stores transcript turn");
        connection
            .execute(
                "INSERT INTO turn_content (turn_rowid, part_index, kind, content, truncated)
                 VALUES (?1, 0, 'text', ?2, 0)",
                rusqlite::params![connection.last_insert_rowid(), transcript.as_bytes()],
            )
            .expect("stores transcript content");
    }

    assert!(candidates(&store, transcript).is_empty());
    let results = candidates(&store, "metadata");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title.as_deref(), Some("Metadata title"));
    assert!(!format!("{:?}", results[0]).contains(transcript));
}

#[test]
fn vm_budget_interrupts_candidates_and_restores_the_connection() {
    let store = store();
    let records = (0..200)
        .map(|index| session(&format!("budget-{index:03}"), "Budget common match", index))
        .collect::<Vec<_>>();
    store
        .upsert_sessions(&records, &[])
        .expect("indexes fixtures");

    let error = store
        .local_session_candidates_with_vm_step_limit("budget", 1, None)
        .expect_err("interrupts candidate ranking");
    assert_eq!(error.to_string(), SESSION_SEARCH_BUDGET_ERROR);
    let count: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM session AS first CROSS JOIN session AS second",
            [],
            |row| row.get(0),
        )
        .expect("removes the progress handler after interruption");
    assert_eq!(count, 40_000);
    assert_eq!(candidates(&store, "budget").len(), 8);
}
