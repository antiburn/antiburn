use std::fs;
use std::path::Path;

use rusqlite::params;
use serde_json::json;

use super::*;
use crate::store::{SessionKey, SessionRecord};

const AGENT: &str = "codex";

fn at(epoch: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(epoch).expect("valid timestamp")
}

fn line(timestamp: OffsetDateTime, rate_limits: Value) -> String {
    json!({
        "timestamp": timestamp.format(&Rfc3339).expect("timestamp"),
        "type": "event_msg",
        "payload": {"type": "token_count", "rate_limits": rate_limits},
    })
    .to_string()
}

fn window(percent: f64, minutes: i64, reset: Option<i64>) -> Value {
    json!({"used_percent": percent, "window_minutes": minutes, "resets_at": reset})
}

/// A Codex file session with one known account (the single-account
/// fallback), and one turn recent enough to be a candidate.
fn candidate_store(directory: &Path, source: &Path, session_id: &str, account: &str) -> Store {
    let store = Store::open(directory).expect("store");
    let now = 1_800_000_000;
    let key = SessionKey::new("native", AGENT, session_id);
    let record = SessionRecord {
        key: key.clone(),
        source_kind: "file".to_owned(),
        source_label: source.display().to_string(),
        wsl_distro: None,
        title: None,
        title_source: None,
        cwd: None,
        surface: "cli".to_owned(),
        updated_at_epoch: Some(now),
        activity_cursor: "synthetic".to_owned(),
        activity_source: "event".to_owned(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: Some(format!("synthetic:{session_id}")),
    };
    store
        .upsert_sessions(
            std::slice::from_ref(&record),
            &crate::agents::evidence_cohort(),
        )
        .expect("session");
    let connection = store.lock();
    connection
        .execute(
            "UPDATE session_evidence SET published_fence = 1
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
        )
        .expect("published evidence");
    connection
        .execute(
            "INSERT INTO turn (
                    environment_key, agent, session_id, claim_fence,
                    source_key, thread_id, turn_index, scope, role, ts_ms,
                    model, input_tokens, cache_read_tokens, cache_write_tokens,
                    output_tokens, is_compaction_boundary
                ) VALUES (?1, ?2, ?3, 1, 'synthetic', 'synthetic', 0, 'main',
                          'assistant', ?4, 'gpt-6-astra', 100, 0, 0, 20, 0)",
            params![key.environment_key, key.agent, key.session_id, now * 1_000],
        )
        .expect("turn");
    connection
        .execute(
            "INSERT OR IGNORE INTO provider_account_seen (agent, provider, account_key,
                 first_seen_epoch, last_seen_epoch)
             VALUES (?1, 'openai', ?2, 1, 1)",
            params![AGENT, account],
        )
        .expect("account");
    drop(connection);
    store
}

fn observation_count(store: &Store) -> i64 {
    store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM provider_usage_observation",
            [],
            |row| row.get(0),
        )
        .expect("observation count")
}

#[test]
fn a_reading_carries_its_percent_reset_and_plan() {
    let observed = at(1_800_000_000);
    let text = line(
        observed,
        json!({
            "limit_id": "codex",
            "primary": window(8.0, 300, Some(1_800_017_000)),
            "secondary": window(26.0, 10_080, Some(1_800_604_800)),
            "plan_type": "pro",
        }),
    );

    let reading = parse_rollout_line(text.as_bytes()).expect("reading");
    assert_eq!(reading.plan.as_deref(), Some("pro"));
    assert_eq!(reading.windows.len(), 2);
    assert_eq!(reading.windows[0].id, "five-hour");
    assert_eq!(reading.windows[0].used_percent, Some(8.0));
    assert_eq!(
        reading.windows[0].resets_at,
        OffsetDateTime::from_unix_timestamp(1_800_017_000).ok()
    );
    assert_eq!(reading.windows[1].id, "seven-day");
    assert!(reading.windows.iter().all(|window| window.authoritative));

    let snapshot = reading.snapshot(&"a".repeat(64));
    assert_eq!(snapshot.plan.as_deref(), Some("pro"));
    assert_eq!(snapshot.plan_tier, None);
    assert_eq!(snapshot.source.id, CODEX_ROLLOUT_SOURCE_ID);
}

#[test]
fn a_model_scoped_limit_bucket_is_excluded() {
    let observed = at(1_800_000_000);
    let text = line(
        observed,
        json!({"limit_id": "model-gpt", "primary": window(50.0, 10_080, Some(1_800_604_800))}),
    );
    assert!(parse_rollout_line(text.as_bytes()).is_none());
}

#[test]
fn a_zero_reading_with_a_projected_reset_keeps_its_reading_but_drops_the_boundary() {
    let observed = at(1_800_000_000);
    let projected = (observed + time::Duration::seconds(300 * 60)).unix_timestamp();
    let text = line(
        observed,
        json!({"primary": window(0.0, 300, Some(projected))}),
    );

    let reading = parse_rollout_line(text.as_bytes()).expect("reading");
    assert_eq!(reading.windows[0].used_percent, Some(0.0));
    assert_eq!(reading.windows[0].resets_at, None);
    assert!(
        reading.windows[0].authoritative,
        "a sliding projection is still a stated reading, just without a committed reset"
    );
}

#[test]
fn leaves_an_incomplete_tail_for_a_later_batch() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("rollout.jsonl");
    let complete = line(
        at(1_800_000_000),
        json!({"primary": window(3.0, 300, Some(1_800_017_000))}),
    );
    let tail = line(
        at(1_800_000_060),
        json!({"primary": window(4.0, 300, Some(1_800_017_000))}),
    );
    fs::write(&path, format!("{complete}\n{}", &tail[..tail.len() / 2])).expect("rollout");

    let batch = read_rollout_batch(&path, 0, MAX_BATCH_BYTES).expect("batch");
    assert_eq!(batch.readings.len(), 1);
    assert!(!batch.complete);
    assert_eq!(batch.next_offset, complete.len() as u64 + 1);
}

#[test]
fn waits_for_an_incomplete_first_record_until_the_source_grows() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("rollout.jsonl");
    let record = line(
        at(1_800_000_000),
        json!({"primary": window(3.0, 300, Some(1_800_017_000))}),
    );
    let split = record.len() / 2;
    fs::write(&path, &record[..split]).expect("partial rollout");

    let incomplete = read_rollout_batch(&path, 0, MAX_BATCH_BYTES).expect("batch");
    assert!(needs_retry(&incomplete, 0));

    fs::write(&path, format!("{record}\n")).expect("completed rollout");
    let complete = read_rollout_batch(&path, 0, MAX_BATCH_BYTES).expect("batch");
    assert!(!needs_retry(&complete, 0));
    assert!(complete.complete);
    assert_eq!(complete.readings.len(), 1);
}

#[test]
fn a_record_longer_than_the_batch_is_skipped_in_bounded_fragments_not_forever() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("rollout.jsonl");
    fs::write(&path, format!("{}\n", "x".repeat(MAX_BATCH_BYTES + 1))).expect("rollout");

    let first = read_rollout_batch(&path, 0, MAX_BATCH_BYTES).expect("batch");
    assert!(first.readings.is_empty());
    assert!(!first.complete);
    assert_eq!(
        first.next_offset, MAX_BATCH_BYTES as u64,
        "the oversized record is skipped forward rather than re-read"
    );

    let second = read_rollout_batch(&path, first.next_offset, MAX_BATCH_BYTES).expect("batch");
    assert!(
        second.complete,
        "the next pass reaches the trailing newline"
    );
}

#[test]
fn resumes_append_only_files_and_restarts_replaced_files() {
    let candidate = RolloutCandidate {
        key: SessionKey::new("native", AGENT, "synthetic"),
        source_label: "/synthetic/rollout.jsonl".to_owned(),
        account_key: "a".repeat(64),
        cursor_bytes: 400,
        source_bytes: 400,
        source_modified_epoch: Some(10),
        source_identity: "17:23".to_owned(),
        complete: true,
    };

    // Appended: more bytes, same identity — resume from the saved cursor.
    assert_eq!(
        resume_offset(
            &candidate,
            &SourceFingerprint {
                bytes: 480,
                modified_epoch: Some(11),
                identity: "17:23".to_owned(),
            }
        ),
        400
    );
    // Truncated back to the cursor with a new modification time: replaced.
    assert_eq!(
        resume_offset(
            &candidate,
            &SourceFingerprint {
                bytes: 400,
                modified_epoch: Some(11),
                identity: "17:23".to_owned(),
            }
        ),
        0
    );
    // Shrunk below the cursor: replaced.
    assert_eq!(
        resume_offset(
            &candidate,
            &SourceFingerprint {
                bytes: 300,
                modified_epoch: Some(10),
                identity: "17:23".to_owned(),
            }
        ),
        0
    );
    // A different inode/device: replaced, regardless of size.
    assert_eq!(
        resume_offset(
            &candidate,
            &SourceFingerprint {
                bytes: 480,
                modified_epoch: Some(11),
                identity: "19:29".to_owned(),
            }
        ),
        0
    );
}

#[test]
fn imports_a_direct_rollout_and_skips_it_on_a_later_pass() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store_dir = directory.path().join("store");
    let source = directory.path().join("direct.jsonl");
    let account = "a".repeat(64);
    let observed = at(1_800_000_000);
    fs::write(
        &source,
        format!(
            "{}\n",
            line(
                observed,
                json!({"primary": window(20.0, 300, Some(1_800_017_000)),
                       "secondary": window(30.0, 10_080, Some(1_800_604_800)),
                       "plan_type": "plus"}),
            )
        ),
    )
    .expect("direct rollout");
    let store = candidate_store(&store_dir, &source, "direct", &account);

    let first = import_rollout_batch(&store, 1_800_000_001).expect("first import");
    assert_eq!(first.imported_observations, 2);
    assert_eq!(observation_count(&store), 2);

    let connection = store.lock();
    let mut statement = connection
        .prepare(
            "SELECT window_role, used_percent, reported_resets_at_epoch, plan, source_id
               FROM provider_usage_observation
              ORDER BY window_role",
        )
        .expect("prepare");
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .expect("rows")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect");
    assert_eq!(
        rows,
        vec![
            (
                "primaryLong".to_string(),
                30.0,
                1_800_604_800,
                "plus".to_string(),
                CODEX_ROLLOUT_SOURCE_ID.to_string(),
            ),
            (
                "primaryShort".to_string(),
                20.0,
                1_800_017_000,
                "plus".to_string(),
                CODEX_ROLLOUT_SOURCE_ID.to_string(),
            ),
        ]
    );
    drop(statement);
    drop(connection);

    let repeated = import_rollout_batch(&store, 1_800_000_001).expect("repeat import");
    assert_eq!(
        repeated.imported_observations, 0,
        "an unchanged file is not re-read"
    );
    assert_eq!(observation_count(&store), 2);
}

#[test]
fn readings_older_than_the_retention_cutoff_are_not_imported() {
    let directory = tempfile::tempdir().expect("tempdir");
    let store_dir = directory.path().join("store");
    let source = directory.path().join("aged.jsonl");
    let account = "a".repeat(64);
    let old = line(
        at(1_000),
        json!({"primary": window(10.0, 300, Some(1_017_000))}),
    );
    let recent = line(
        at(1_800_000_000),
        json!({"primary": window(15.0, 300, Some(1_800_017_000))}),
    );
    fs::write(&source, format!("{old}\n{recent}\n")).expect("rollout");
    let store = candidate_store(&store_dir, &source, "aged", &account);

    let batch = import_rollout_batch(&store, 1_800_000_001).expect("import");
    assert_eq!(
        batch.imported_observations, 1,
        "only the reading inside the retention window is imported"
    );
}

#[test]
fn resumes_a_large_rollout_without_losing_the_tail_reading() {
    let directory = tempfile::tempdir().expect("tempdir");
    let source = directory.path().join("large.jsonl");
    let account = "a".repeat(64);
    let observed = at(1_800_000_000);
    let first = line(
        observed,
        json!({"primary": window(10.0, 300, Some(1_800_017_000))}),
    );
    let tail = line(
        at(1_800_000_001),
        json!({"primary": window(40.0, 300, Some(1_800_017_000))}),
    );
    let mut rollout = format!("{first}\n");
    while rollout.len() <= MAX_BATCH_BYTES + 64 {
        rollout
            .push_str("{\"type\":\"ignored\",\"padding\":\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"}\n");
    }
    rollout.push_str(&tail);
    rollout.push('\n');
    fs::write(&source, rollout).expect("large rollout");
    let store = candidate_store(directory.path(), &source, "large", &account);

    let first_batch = import_rollout_batch(&store, 1_800_000_001).expect("first batch");
    assert!(first_batch.pending);
    assert!(first_batch.continue_soon);
    let second_batch = import_rollout_batch(&store, 1_800_000_001).expect("second batch");
    assert_eq!(second_batch.imported_observations, 1);
    assert_eq!(observation_count(&store), 2);
}

#[test]
fn defers_unavailable_and_incomplete_sources_while_importing_others() {
    let directory = tempfile::tempdir().expect("tempdir");
    let missing = directory.path().join("missing.jsonl");
    let partial = directory.path().join("partial.jsonl");
    let ready = directory.path().join("ready.jsonl");
    let account = "a".repeat(64);
    let partial_record = line(
        at(1_800_000_001),
        json!({"primary": window(20.0, 300, Some(1_800_017_000))}),
    );
    fs::write(&partial, &partial_record[..partial_record.len() / 2]).expect("partial rollout");
    fs::write(
        &ready,
        format!(
            "{}\n",
            line(
                at(1_800_000_002),
                json!({"primary": window(30.0, 300, Some(1_800_017_000))}),
            )
        ),
    )
    .expect("ready rollout");
    let store = candidate_store(directory.path(), &missing, "missing", &account);
    let _partial_store = candidate_store(directory.path(), &partial, "partial", &account);
    let _ready_store = candidate_store(directory.path(), &ready, "ready", &account);

    let first = import_rollout_batch(&store, 1_800_000_001).expect("first import");
    assert_eq!(first.deferred_sources, 2);
    assert!(first.pending);
    assert_eq!(first.imported_observations, 1);

    fs::write(
        &missing,
        format!(
            "{}\n",
            line(
                at(1_800_000_003),
                json!({"primary": window(40.0, 300, Some(1_800_017_000))}),
            )
        ),
    )
    .expect("missing recovery");
    fs::write(&partial, format!("{partial_record}\n")).expect("partial recovery");
    let recovered = import_rollout_batch(&store, 1_800_000_061).expect("recovery import");
    assert_eq!(recovered.imported_observations, 2);
    assert_eq!(observation_count(&store), 3);
}

#[test]
fn completed_sources_do_not_request_another_immediate_batch() {
    let candidate = RolloutCandidate {
        key: SessionKey::new("native", AGENT, "synthetic"),
        source_label: "/synthetic/rollout.jsonl".to_owned(),
        account_key: "a".repeat(64),
        cursor_bytes: 0,
        source_bytes: 0,
        source_modified_epoch: None,
        source_identity: String::new(),
        complete: true,
    };
    let candidates = vec![candidate; MAX_CANDIDATES_PER_PASS + 1];

    assert!(!has_more_ready(&candidates));
}
