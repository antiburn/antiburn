//! Each Overview command's body, run once against the writer and once
//! against a [`Store::open_reader`] reader over the same file-backed
//! database, asserting the reader answers exactly what the writer does.
//!
//! A reader needs a real database file: two connections cannot share one
//! in-memory database.

use std::time::Duration;

use rusqlite::params;

use super::local_usage::{
    live_usage_settings, provider_usage_summary_for_store,
    session_limit_allocation_summary_for_store,
};
use super::usage::allowance_usage_for_store;
use crate::store::provider_limit::{FactorPoint, LANE_FIVE_HOUR};
use crate::store::{AnalysisRecord, Store};
use crate::store::{SessionKey, SessionRecord};
use antiburn_local::pricing::ModelTokens;

const PROVIDER: &str = "anthropic";
const AGENT: &str = "claude-code";
const MODEL: &str = "claude-sonnet-5";
const ACCOUNT_KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn file_backed_store() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().expect("creates a temp dir");
    let store = Store::open(directory.path()).expect("opens a file-backed store");
    (directory, store)
}

/// One bound session with a priced turn, a five-hour quota period and
/// observation, and a learned factor point: enough for every routed
/// command's body to return a non-empty answer.
fn seed_scenario(store: &Store, now: i64) -> SessionKey {
    let key = SessionKey::new("native", AGENT, "reader-routing");
    store
        .upsert_sessions(
            &[SessionRecord {
                key: key.clone(),
                source_kind: "inline".to_string(),
                source_label: "synthetic".to_string(),
                wsl_distro: None,
                title: Some("Reader routing".to_string()),
                title_source: None,
                cwd: None,
                surface: "unknown".to_string(),
                updated_at_epoch: Some(now),
                activity_cursor: "synthetic".to_string(),
                activity_source: "event".to_string(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: Some("synthetic".to_string()),
            }],
            &[],
        )
        .expect("stores the synthetic session");

    {
        let connection = store.lock();
        connection
            .execute(
                "INSERT OR IGNORE INTO session_evidence (
                     environment_key, agent, session_id, status, published_fence
                 ) VALUES (?1, ?2, ?3, 'ready', 1)",
                params![key.environment_key, key.agent, key.session_id],
            )
            .expect("publishes synthetic evidence");
        connection
            .execute(
                "INSERT INTO turn (
                     environment_key, agent, session_id, claim_fence, source_key,
                     thread_id, turn_index, scope, role, ts_ms, model, effort, speed,
                     input_tokens, cache_read_tokens, cache_write_tokens, output_tokens,
                     is_compaction_boundary, message_id, uuid, parent_uuid
                 ) VALUES (?1, ?2, ?3, 1, 'synthetic', 'synthetic', 0, 'main', 'assistant',
                           ?4, ?5, NULL, NULL, ?6, 0, 0, 0, 0, NULL, NULL, NULL)",
                params![
                    key.environment_key,
                    key.agent,
                    key.session_id,
                    (now - 9_000) * 1_000,
                    MODEL,
                    100_000_i64,
                ],
            )
            .expect("stores the synthetic turn");
        connection
            .execute(
                "INSERT INTO session_provider_account (
                     environment_key, agent, session_id, provider, account_key,
                     provenance, confidence, first_seen_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'provider_live', 'direct', '2026-01-01T00:00:00Z')",
                params![
                    key.environment_key,
                    key.agent,
                    key.session_id,
                    PROVIDER,
                    ACCOUNT_KEY
                ],
            )
            .expect("binds the synthetic account");
        connection
            .execute(
                "INSERT INTO provider_usage_period (
                     provider, account_key, window_id, window_kind, window_role,
                     scope_key, scope_label, duration_seconds, starts_at_epoch,
                     resets_at_epoch, first_observed_epoch, last_observed_epoch
                 ) VALUES (?1, ?2, 'five-hour', 'rolling', 'primaryShort',
                           'account', 'account', ?3, ?4, ?5, ?5, ?5)",
                params![PROVIDER, ACCOUNT_KEY, 18_000_i64, now - 18_000, now],
            )
            .expect("inserts a synthetic period");
        let period_id = connection.last_insert_rowid();
        connection
            .execute(
                "INSERT INTO provider_usage_observation (
                     period_id, provider, account_key, window_id, window_kind, window_role,
                     scope_key, scope_label, observed_at_epoch, used_percent, is_fresh,
                     is_authoritative, confidence, source_id, plan, plan_tier
                 ) VALUES (?1, ?2, ?3, 'five-hour', 'rolling', 'primaryShort',
                           'account', 'account', ?4, ?5, 1, 1, 'high', 'test', NULL, NULL)",
                params![period_id, PROVIDER, ACCOUNT_KEY, now, 20.0_f64],
            )
            .expect("inserts a synthetic observation");
    }

    store
        .upsert_factor_point(&FactorPoint {
            id: 0,
            provider: PROVIDER.to_string(),
            account_key: ACCOUNT_KEY.to_string(),
            lane: LANE_FIVE_HOUR.to_string(),
            effective_at_epoch: 0,
            usd_per_percent: 0.5,
            method: "delta".to_string(),
            sample_count: 1,
            plan: None,
            plan_tier: None,
        })
        .expect("stores a synthetic factor point");

    let tokens = ModelTokens {
        input_tokens: 100_000,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        cache_creation_1h_tokens: 0,
    };
    let breakdown = std::collections::HashMap::from([(MODEL.to_string(), tokens)]);
    store
        .save_analysis(
            &AnalysisRecord {
                key: key.clone(),
                model_breakdown_json: serde_json::to_string(&breakdown)
                    .expect("serializes the breakdown"),
                pricing_breakdown_json: "{}".to_string(),
                inclusive_models_json: "[]".to_string(),
                initial_context_json: None,
                source_summaries_json: None,
                provider_hints_json: None,
                source_fingerprint: "synthetic".to_string(),
                pricing_generation: 0,
                analyzed_generation: 0,
                parser_revision: 0,
                analyzer_revision: 0,
                metrics_schema_revision: 0,
            },
            None,
        )
        .expect("saves the synthetic analysis");

    key
}

#[test]
fn provider_usage_summary_matches_through_the_reader() {
    let (_directory, writer) = file_backed_store();
    seed_scenario(&writer, crate::scan::unix_now());
    let reader = writer
        .open_reader(Duration::from_millis(100))
        .expect("opens a reader");

    let mut from_writer =
        provider_usage_summary_for_store(&writer, None).expect("the writer computes a summary");
    let mut from_reader =
        provider_usage_summary_for_store(&reader, None).expect("the reader computes a summary");
    // `generated_at` stamps the moment each call ran; each call reads its own
    // clock, so it is not part of the property under test.
    from_writer.generated_at.clear();
    from_reader.generated_at.clear();
    assert_eq!(from_writer, from_reader);
    assert!(
        !from_writer.providers.is_empty(),
        "the seeded session attributes to a provider"
    );
}

#[test]
fn allowance_usage_matches_through_the_reader() {
    let (_directory, writer) = file_backed_store();
    let now = crate::scan::unix_now();
    seed_scenario(&writer, now);
    let reader = writer
        .open_reader(Duration::from_millis(100))
        .expect("opens a reader");

    let from_writer =
        allowance_usage_for_store(&writer, now, 0).expect("the writer computes a summary");
    let from_reader =
        allowance_usage_for_store(&reader, now, 0).expect("the reader computes a summary");
    assert_eq!(from_writer, from_reader);
    assert!(
        !from_writer.accounts.is_empty(),
        "the seeded account has quota evidence"
    );
}

#[test]
fn session_limit_allocations_match_through_the_reader() {
    let (_directory, writer) = file_backed_store();
    let now = crate::scan::unix_now();
    seed_scenario(&writer, now);
    let reader = writer
        .open_reader(Duration::from_millis(100))
        .expect("opens a reader");

    let from_writer = session_limit_allocation_summary_for_store(&writer, now)
        .expect("the writer computes allocations");
    let from_reader = session_limit_allocation_summary_for_store(&reader, now)
        .expect("the reader computes allocations");
    assert_eq!(from_writer.allocations, from_reader.allocations);
    assert!(
        !from_writer.allocations.is_empty(),
        "the seeded session earns an allocation"
    );
}

#[test]
fn live_usage_settings_match_through_the_reader() {
    let (_directory, writer) = file_backed_store();
    let settings = writer.settings().expect("reads the default settings");
    let reader = writer
        .open_reader(Duration::from_millis(100))
        .expect("opens a reader");

    assert_eq!(live_usage_settings(&writer), Some(settings.clone()));
    assert_eq!(live_usage_settings(&reader), Some(settings));
}
