//! Bounded import of account-wide Codex allowance readings from rollout files.
//!
//! The importer selects `token_count.rate_limits` metadata. It does not retain
//! or log transcript message content.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::provider_usage::live::codex::is_sliding_reset_projection;
use crate::provider_usage::live::model::{
    Confidence, Freshness, ProviderUsageSnapshot, UsageScope, UsageSource, UsageWindow,
    UsageWindowKind, WindowRole,
};
use crate::store::Store;

/// Stable provenance for readings imported from a Codex rollout file.
pub(crate) const CODEX_ROLLOUT_BACKFILL_SOURCE: &str = "codex-rollout-backfill";

const OPENAI: &str = "openai";
const HISTORY_BACKFILL_SOURCE: &str = "live-history-backfill";
const BACKFILL_STATE_KEY: &str = "internal:providerUsageBackfillV1";
const MAX_BATCH_BYTES: usize = 256 * 1024;
const MAX_CANDIDATES_PER_BATCH: usize = 8;
const MAX_BATCH_DURATION: Duration = Duration::from_millis(500);
const MAX_LEGACY_HISTORY_SAMPLES: usize = 2_000;

/// A bounded set of complete rollout records and the safe resume offset.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RolloutBatch {
    pub readings: Vec<RolloutReading>,
    pub next_offset: u64,
    pub complete: bool,
    pub skipped_bytes: u64,
}

/// One account-wide allowance report from a rollout event.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RolloutReading {
    pub observed_at: OffsetDateTime,
    pub windows: Vec<UsageWindow>,
}

/// The result of one bounded, resumable backfill pass.
///
/// A large rollout advances by at most 256 KiB per pass. The background
/// scheduler requests another yielding pass only while `pending` is true.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BackfillBatch {
    pub imported_observations: usize,
    pub scanned_bytes: u64,
    pub completed_sources: usize,
    pub deferred_sources: usize,
    /// The scheduler can resume file work without waiting for its next tick.
    pub continue_soon: bool,
    /// The earliest deferred source that needs another local attempt.
    pub next_retry_epoch: Option<i64>,
    pub pending: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct BackfillState {
    #[serde(default)]
    legacy_history_imported: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct LegacyHistorySample {
    at: i64,
    #[serde(default)]
    pct: Option<f64>,
    fresh: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceFingerprint {
    bytes: u64,
    modified_epoch: Option<i64>,
    identity: String,
}

impl RolloutReading {
    /// Build one historical snapshot for a directly attributed account.
    pub(crate) fn snapshot(&self, account_key: &str) -> ProviderUsageSnapshot {
        ProviderUsageSnapshot {
            provider: OPENAI,
            account: Some(account_key.to_owned()),
            account_uuid: None,
            account_email: None,
            plan: None,
            plan_tier: None,
            observed_at: self.observed_at,
            source: UsageSource {
                id: CODEX_ROLLOUT_BACKFILL_SOURCE,
                label: "Codex local rollout".to_owned(),
                confidence: Confidence::High,
                // This is fresh at the recorded observation, not now.
                freshness: Freshness::Fresh,
            },
            windows: self.windows.clone(),
            supplemental: None,
            reset_credits: None,
        }
    }
}

/// Import a bounded page of retained provider facts without any provider call.
///
/// A partial file keeps its byte offset in the local store. The next pass
/// resumes there, and a file is complete only after every complete record was
/// read. The importer never reads a source that lacks direct account evidence.
pub(crate) fn import_backfill_batch(store: &Store, now_epoch: i64) -> Result<BackfillBatch> {
    let started = Instant::now();
    let mut state = load_state(store);
    let mut result = BackfillBatch::default();
    let cutoff = retention_cutoff(store, now_epoch)?;

    if !state.legacy_history_imported {
        let snapshots = legacy_history_snapshots(store, cutoff);
        store.record_provider_usage_snapshots(&snapshots)?;
        result.imported_observations += snapshots
            .iter()
            .map(|snapshot| snapshot.windows.len())
            .sum::<usize>();
        state.legacy_history_imported = true;
    }

    let candidates = store.provider_usage_backfill_candidates(
        cutoff,
        now_epoch,
        MAX_CANDIDATES_PER_BATCH + 1,
    )?;
    let has_more_ready = has_more_ready(&candidates);
    for candidate in candidates.iter().take(MAX_CANDIDATES_PER_BATCH) {
        if started.elapsed() >= MAX_BATCH_DURATION {
            result.pending = true;
            result.continue_soon = true;
            break;
        }
        let path = Path::new(&candidate.source_label);
        let fingerprint = match source_fingerprint(path) {
            Ok(fingerprint) => fingerprint,
            Err(_) => {
                let retry = store.defer_provider_usage_backfill_candidate(candidate, now_epoch)?;
                result.deferred_sources += 1;
                result.pending = true;
                result.next_retry_epoch = earliest(result.next_retry_epoch, retry);
                continue;
            }
        };
        if candidate.complete
            && fingerprint.bytes == candidate.source_bytes
            && fingerprint.modified_epoch == candidate.source_modified_epoch
            && fingerprint.identity == candidate.source_identity
        {
            store.touch_provider_usage_backfill_checkpoint(candidate, now_epoch)?;
            continue;
        }
        let offset = resume_offset(candidate, &fingerprint);
        let batch = match read_rollout_batch(path, offset, MAX_BATCH_BYTES) {
            Ok(batch) => batch,
            Err(_) => {
                let retry = store.defer_provider_usage_backfill_candidate(candidate, now_epoch)?;
                result.deferred_sources += 1;
                result.pending = true;
                result.next_retry_epoch = earliest(result.next_retry_epoch, retry);
                continue;
            }
        };
        let snapshots = batch
            .readings
            .iter()
            .filter(|reading| reading.observed_at.unix_timestamp() >= cutoff)
            .map(|reading| reading.snapshot(&candidate.account_key))
            .collect::<Vec<_>>();
        store.record_provider_usage_snapshots(&snapshots)?;
        result.imported_observations += snapshots
            .iter()
            .map(|snapshot| snapshot.windows.len())
            .sum::<usize>();
        result.scanned_bytes += batch.next_offset.saturating_sub(offset);

        let checkpoint = crate::store::usage_backfill::ProviderUsageBackfillCheckpoint {
            cursor_bytes: batch.next_offset,
            source_bytes: fingerprint.bytes,
            source_modified_epoch: fingerprint.modified_epoch,
            source_identity: fingerprint.identity,
            complete: batch.complete,
        };
        store.update_provider_usage_backfill_checkpoint(candidate, &checkpoint, now_epoch)?;
        if needs_retry(&batch, offset) {
            let mut deferred = candidate.clone();
            deferred.cursor_bytes = batch.next_offset;
            let retry = store.defer_provider_usage_backfill_candidate(&deferred, now_epoch)?;
            result.deferred_sources += 1;
            result.pending = true;
            result.next_retry_epoch = earliest(result.next_retry_epoch, retry);
            continue;
        }
        if batch.complete {
            result.completed_sources += 1;
        } else {
            result.pending = true;
            result.continue_soon = true;
        }
    }
    if has_more_ready {
        result.pending = true;
        result.continue_soon = true;
    }
    if let Some(retry) = store.provider_usage_backfill_next_retry_epoch(now_epoch)? {
        result.pending = true;
        result.next_retry_epoch = earliest(result.next_retry_epoch, retry);
    }
    store.write_provider_usage_backfill_state(
        &serde_json::to_string(&state).expect("backfill state is serializable"),
    )?;
    Ok(result)
}

fn earliest(current: Option<i64>, candidate: i64) -> Option<i64> {
    Some(current.map_or(candidate, |current| current.min(candidate)))
}

fn has_more_ready(
    candidates: &[crate::store::usage_backfill::ProviderUsageBackfillCandidate],
) -> bool {
    candidates
        .get(MAX_CANDIDATES_PER_BATCH)
        .is_some_and(|candidate| !candidate.complete)
}

fn needs_retry(batch: &RolloutBatch, offset: u64) -> bool {
    !batch.complete && batch.next_offset == offset
}

fn source_fingerprint(path: &Path) -> Result<SourceFingerprint> {
    let metadata = path
        .metadata()
        .with_context(|| format!("failed to inspect Codex rollout {}", path.display()))?;
    let modified_epoch = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_secs()).ok());
    Ok(SourceFingerprint {
        bytes: metadata.len(),
        modified_epoch,
        identity: source_identity(&metadata),
    })
}

#[cfg(unix)]
fn source_identity(metadata: &std::fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;

    format!("{}:{}", metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn source_identity(metadata: &std::fs::Metadata) -> String {
    metadata
        .created()
        .ok()
        .and_then(|created| created.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos().to_string())
        .unwrap_or_default()
}

fn resume_offset(
    candidate: &crate::store::usage_backfill::ProviderUsageBackfillCandidate,
    fingerprint: &SourceFingerprint,
) -> u64 {
    let replaced = (!candidate.source_identity.is_empty()
        && !fingerprint.identity.is_empty()
        && fingerprint.identity != candidate.source_identity)
        || fingerprint.bytes < candidate.cursor_bytes
        || (fingerprint.identity.is_empty()
            && fingerprint.modified_epoch != candidate.source_modified_epoch)
        || (fingerprint.bytes == candidate.cursor_bytes
            && fingerprint.modified_epoch != candidate.source_modified_epoch);
    if replaced {
        0
    } else {
        candidate.cursor_bytes.min(fingerprint.bytes)
    }
}

fn load_state(store: &Store) -> BackfillState {
    store
        .internal_value(BACKFILL_STATE_KEY)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn retention_cutoff(store: &Store, now_epoch: i64) -> Result<i64> {
    let retention_days = store.settings()?.session_data_retention_days;
    let days = if retention_days < 0 {
        90
    } else {
        i64::from(retention_days).clamp(1, 90)
    };
    Ok(now_epoch.saturating_sub(days.saturating_mul(86_400)))
}

fn legacy_history_snapshots(store: &Store, cutoff: i64) -> Vec<ProviderUsageSnapshot> {
    let Some(raw) = store.internal_value("internal:liveUsageHistoryV2") else {
        return Vec::new();
    };
    let Ok(series) = serde_json::from_str::<BTreeMap<String, Vec<LegacyHistorySample>>>(&raw)
    else {
        return Vec::new();
    };
    series
        .into_iter()
        .filter_map(|(key, samples)| legacy_window_key(&key).map(|parts| (parts, samples)))
        .flat_map(|((provider, account_key, window), samples)| {
            samples
                .into_iter()
                .filter(move |sample| sample.at >= cutoff)
                .filter_map(move |sample| {
                    let percent = valid_percent(sample.pct)?;
                    let observed_at = OffsetDateTime::from_unix_timestamp(sample.at).ok()?;
                    Some(ProviderUsageSnapshot {
                        provider,
                        account: Some(account_key.clone()),
                        account_uuid: None,
                        account_email: None,
                        plan: None,
                        plan_tier: None,
                        observed_at,
                        source: UsageSource {
                            id: HISTORY_BACKFILL_SOURCE,
                            label: "Retained local usage history".to_owned(),
                            confidence: Confidence::Medium,
                            freshness: if sample.fresh {
                                Freshness::Fresh
                            } else {
                                Freshness::Stale
                            },
                        },
                        windows: vec![UsageWindow {
                            id: window.id.to_owned(),
                            role: window.role.clone(),
                            kind: window.kind.clone(),
                            scope: UsageScope::Account,
                            used_percent: Some(percent),
                            starts_at: None,
                            resets_at: None,
                            authoritative: false,
                        }],
                        supplemental: None,
                        reset_credits: None,
                    })
                })
        })
        .take(MAX_LEGACY_HISTORY_SAMPLES)
        .collect()
}

struct LegacyWindow {
    id: &'static str,
    role: WindowRole,
    kind: UsageWindowKind,
}

fn legacy_window_key(key: &str) -> Option<(&'static str, String, LegacyWindow)> {
    let mut parts = key.splitn(3, ':');
    let provider = legacy_provider(parts.next()?)?;
    let account_key = parts.next()?;
    let window = match parts.next()? {
        "seven-day" => LegacyWindow {
            id: "seven-day",
            role: WindowRole::PrimaryLong,
            kind: UsageWindowKind::Weekly,
        },
        "five-hour" => LegacyWindow {
            id: "five-hour",
            role: WindowRole::PrimaryShort,
            kind: UsageWindowKind::Rolling,
        },
        _ => return None,
    };
    is_opaque_account_key(account_key).then_some((provider, account_key.to_owned(), window))
}

fn legacy_provider(provider: &str) -> Option<&'static str> {
    (provider == OPENAI).then_some(OPENAI)
}

fn is_opaque_account_key(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_percent(percent: Option<f64>) -> Option<f64> {
    percent.filter(|percent| percent.is_finite() && (0.0..=100.0).contains(percent))
}

/// Read complete JSONL records inside one explicit byte budget.
///
/// The returned offset never advances past an unfinished record at EOF. A
/// record longer than the budget is skipped in fixed-size fragments, without
/// allocating the record or interpreting its content.
pub(crate) fn read_rollout_batch(
    path: &Path,
    offset: u64,
    byte_limit: usize,
) -> Result<RolloutBatch> {
    let mut file = File::open(path)
        .with_context(|| format!("failed to open Codex rollout {}", path.display()))?;
    let file_len = file
        .metadata()
        .with_context(|| format!("failed to inspect Codex rollout {}", path.display()))?
        .len();
    let offset = offset.min(file_len);
    let byte_limit = byte_limit.clamp(1, MAX_BATCH_BYTES);
    file.seek(SeekFrom::Start(offset))?;

    let mut bytes = vec![0; byte_limit];
    let read = file.read(&mut bytes)?;
    bytes.truncate(read);
    if bytes.is_empty() {
        return Ok(RolloutBatch {
            readings: Vec::new(),
            next_offset: offset,
            complete: offset == file_len,
            skipped_bytes: 0,
        });
    }

    let Some(last_newline) = bytes.iter().rposition(|byte| *byte == b'\n') else {
        let at_eof = offset.saturating_add(bytes.len() as u64) == file_len;
        return Ok(RolloutBatch {
            readings: Vec::new(),
            next_offset: if at_eof {
                offset
            } else {
                offset.saturating_add(bytes.len() as u64)
            },
            complete: false,
            skipped_bytes: if at_eof { 0 } else { bytes.len() as u64 },
        });
    };

    let mut readings = Vec::new();
    for line in bytes[..last_newline].split(|byte| *byte == b'\n') {
        if let Some(reading) = parse_rollout_line(line) {
            readings.push(reading);
        }
    }
    let next_offset = offset.saturating_add(last_newline as u64 + 1);
    Ok(RolloutBatch {
        readings,
        next_offset,
        complete: next_offset == file_len,
        skipped_bytes: 0,
    })
}

fn parse_rollout_line(line: &[u8]) -> Option<RolloutReading> {
    let value: Value = serde_json::from_slice(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let rate_limits = payload.get("rate_limits")?.as_object()?;
    if !matches!(
        rate_limits.get("limit_id").and_then(Value::as_str),
        None | Some("codex")
    ) {
        return None;
    }
    let observed_at = OffsetDateTime::parse(value.get("timestamp")?.as_str()?, &Rfc3339).ok()?;
    let windows = ["primary", "secondary"]
        .into_iter()
        .filter_map(|key| rate_limits.get(key))
        .filter_map(|window| parse_window(window, observed_at))
        .collect::<Vec<_>>();
    (!windows.is_empty()).then_some(RolloutReading {
        observed_at,
        windows,
    })
}

fn parse_window(value: &Value, observed_at: OffsetDateTime) -> Option<UsageWindow> {
    let used_percent = value.get("used_percent")?.as_f64()?;
    if !(0.0..=100.0).contains(&used_percent) {
        return None;
    }
    let minutes = value.get("window_minutes")?.as_i64()?;
    let (id, role, kind) = match minutes {
        10_080 => (
            "seven-day",
            WindowRole::PrimaryLong,
            UsageWindowKind::Weekly,
        ),
        300 => (
            "five-hour",
            WindowRole::PrimaryShort,
            UsageWindowKind::Rolling,
        ),
        _ => return None,
    };
    let resets_at = value
        .get("resets_at")
        .and_then(Value::as_i64)
        .and_then(|epoch| OffsetDateTime::from_unix_timestamp(epoch).ok())
        .filter(|reset| {
            !is_sliding_reset_projection(*reset, observed_at, minutes * 60, used_percent)
        });
    let authoritative = resets_at.is_some();
    Some(UsageWindow {
        id: id.to_owned(),
        role,
        kind,
        scope: UsageScope::Account,
        used_percent: Some(used_percent),
        starts_at: None,
        resets_at,
        authoritative,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rusqlite::params;
    use serde_json::json;

    use super::*;

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

    fn candidate_store(directory: &Path, source: &Path, session_id: &str, account: &str) -> Store {
        let store = Store::open(directory).expect("store");
        let now = 1_800_000_000;
        let record = crate::store::SessionRecord {
            key: crate::store::SessionKey::new("native", "codex", session_id),
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
        let connection = store.test_lock();
        connection
            .execute(
                "UPDATE session_evidence SET published_fence = 1
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                params![
                    record.key.environment_key,
                    record.key.agent,
                    record.key.session_id
                ],
            )
            .expect("published evidence");
        connection
            .execute(
                "INSERT INTO session_provider_account (
                        environment_key, agent, session_id, provider, account_key,
                        provenance, confidence, first_seen_at
                    ) VALUES (?1, ?2, ?3, 'openai', ?4, 'provider_live', 'direct', 'synthetic')",
                params![
                    record.key.environment_key,
                    record.key.agent,
                    record.key.session_id,
                    account,
                ],
            )
            .expect("account");
        connection
            .execute(
                "INSERT INTO turn (
                        environment_key, agent, session_id, claim_fence,
                        source_key, thread_id, turn_index, scope, role, ts_ms,
                        model, input_tokens, cache_read_tokens, cache_write_tokens,
                        output_tokens, is_compaction_boundary
                    ) VALUES (?1, ?2, ?3, 1, 'synthetic', 'synthetic', 0, 'main',
                              'assistant', ?4, 'gpt-6-astra', 100, 0, 0, 20, 0)",
                params![
                    record.key.environment_key,
                    record.key.agent,
                    record.key.session_id,
                    now * 1_000,
                ],
            )
            .expect("turn");
        drop(connection);
        store
    }

    fn observation_count(store: &Store) -> i64 {
        store
            .test_lock()
            .query_row(
                "SELECT COUNT(*) FROM provider_usage_observation",
                [],
                |row| row.get(0),
            )
            .expect("observation count")
    }

    #[test]
    fn imports_only_the_supported_account_wide_windows() {
        let observed = at(1_800_000_000);
        let line = line(
            observed,
            json!({
                "limit_id": "codex",
                "primary": window(8.0, 300, Some(1_800_017_000)),
                "secondary": window(26.0, 10_080, Some(1_800_604_800)),
            }),
        );

        let reading = parse_rollout_line(line.as_bytes()).expect("reading");
        assert_eq!(reading.windows.len(), 2);
        assert_eq!(reading.windows[0].id, "five-hour");
        assert_eq!(reading.windows[1].id, "seven-day");
        assert!(reading.windows.iter().all(|window| window.authoritative));
    }

    #[test]
    fn excludes_other_limit_buckets_and_unproven_boundaries() {
        let observed = at(1_800_000_000);
        let other = line(
            observed,
            json!({"limit_id": "model-gpt", "primary": window(50.0, 10_080, Some(1_800_604_800))}),
        );
        assert!(parse_rollout_line(other.as_bytes()).is_none());

        let projected = line(
            observed,
            json!({"primary": window(0.0, 300, Some(1_800_018_000))}),
        );
        let reading = parse_rollout_line(projected.as_bytes()).expect("reading");
        assert_eq!(reading.windows[0].resets_at, None);
        assert!(!reading.windows[0].authoritative);
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
    fn skips_a_very_long_record_without_growing_the_batch() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("rollout.jsonl");
        fs::write(&path, format!("{}\n", "x".repeat(MAX_BATCH_BYTES + 1))).expect("rollout");

        let batch = read_rollout_batch(&path, 0, MAX_BATCH_BYTES).expect("batch");
        assert!(batch.readings.is_empty());
        assert_eq!(batch.skipped_bytes, MAX_BATCH_BYTES as u64);
        assert!(!batch.complete);
    }

    #[test]
    fn imports_the_bounded_legacy_cache_once() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = Store::open_in_memory(directory.path()).expect("store");
        let account = "a".repeat(64);
        store.set_internal_value(
            "internal:liveUsageHistoryV2",
            &format!(
                r#"{{"openai:{account}:seven-day":[{{"at":1800000000,"pct":12.0,"fresh":false}}]}}"#
            ),
        );

        let first = import_backfill_batch(&store, 1_800_000_001).expect("backfill");
        let second = import_backfill_batch(&store, 1_800_000_001).expect("backfill");
        assert_eq!(first.imported_observations, 1);
        assert_eq!(second.imported_observations, 0);
        assert!(!first.pending);
        assert!(!second.pending);
    }

    #[test]
    fn imports_direct_rollouts_idempotently_and_skips_ambiguous_sessions() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store_dir = directory.path().join("store");
        let source = directory.path().join("direct.jsonl");
        let ambiguous = directory.path().join("ambiguous.jsonl");
        let account = "a".repeat(64);
        let other_account = "b".repeat(64);
        let observed = at(1_800_000_000);
        fs::write(
            &source,
            format!(
                "{}\n",
                line(
                    observed,
                    json!({"primary": window(20.0, 300, Some(1_800_017_000)),
                           "secondary": window(30.0, 10_080, Some(1_800_604_800))}),
                )
            ),
        )
        .expect("direct rollout");
        fs::write(
            &ambiguous,
            format!(
                "{}\n",
                line(
                    observed,
                    json!({"primary": window(40.0, 300, Some(1_800_017_000))}),
                )
            ),
        )
        .expect("ambiguous rollout");
        let store = candidate_store(&store_dir, &source, "direct", &account);
        let ambiguous_store = candidate_store(&store_dir, &ambiguous, "ambiguous", &account);
        ambiguous_store
            .test_lock()
            .execute(
                "INSERT INTO session_provider_account (
                        environment_key, agent, session_id, provider, account_key,
                        provenance, confidence, first_seen_at
                    ) VALUES ('native', 'codex', 'ambiguous', 'openai', ?1,
                              'provider_live', 'direct', 'synthetic')",
                params![other_account],
            )
            .expect("second account");

        let first = import_backfill_batch(&store, 1_800_000_001).expect("first import");
        assert_eq!(first.imported_observations, 2);
        assert_eq!(observation_count(&store), 2);
        crate::provider_usage::ledger::reconcile(&store, 1_800_000_001);
        let connection = store.test_lock();
        let mut statement = connection
            .prepare(
                "SELECT metric, percent, partial
                   FROM provider_usage_session_allocation
                  ORDER BY metric",
            )
            .expect("allocations");
        let allocations = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .expect("allocation rows")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("allocation values");
        assert_eq!(
            allocations,
            vec![
                ("fiveHour".to_owned(), 20.0, 1),
                ("weekly".to_owned(), 30.0, 1),
            ]
        );
        drop(statement);
        drop(connection);

        drop(store);
        let reopened = Store::open(&store_dir).expect("reopen");
        let repeated = import_backfill_batch(&reopened, 1_800_000_001).expect("repeat import");
        assert_eq!(repeated.imported_observations, 0);
        assert_eq!(observation_count(&reopened), 2);
    }

    #[test]
    fn resumes_a_large_rollout_without_losing_the_tail_observation() {
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
            rollout.push_str(
                "{\"type\":\"ignored\",\"padding\":\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"}\n",
            );
        }
        rollout.push_str(&tail);
        rollout.push('\n');
        fs::write(&source, rollout).expect("large rollout");
        let store = candidate_store(directory.path(), &source, "large", &account);

        let first_batch = import_backfill_batch(&store, 1_800_000_001).expect("first batch");
        assert!(first_batch.pending);
        assert!(first_batch.continue_soon);
        assert!(first_batch.scanned_bytes <= MAX_BATCH_BYTES as u64);
        let second_batch = import_backfill_batch(&store, 1_800_000_001).expect("second batch");
        assert_eq!(second_batch.imported_observations, 1);
        assert_eq!(observation_count(&store), 2);
    }

    #[test]
    fn defers_unavailable_and_incomplete_sources_while_importing_other_sources() {
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
        let partial_store = candidate_store(directory.path(), &partial, "partial", &account);
        let ready_store = candidate_store(directory.path(), &ready, "ready", &account);

        let first = import_backfill_batch(&store, 1_800_000_001).expect("first import");
        assert_eq!(first.deferred_sources, 2);
        assert!(first.pending);
        assert!(!first.continue_soon);
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
        let recovered = import_backfill_batch(&store, 1_800_000_061).expect("recovery import");
        assert_eq!(recovered.imported_observations, 2);
        assert_eq!(observation_count(&store), 3);
        drop((partial_store, ready_store));
    }

    #[test]
    fn resumes_append_only_files_and_restarts_replaced_files() {
        let candidate = crate::store::usage_backfill::ProviderUsageBackfillCandidate {
            key: crate::store::SessionKey::new("native", "codex", "synthetic"),
            source_label: "/synthetic/rollout.jsonl".to_owned(),
            account_key: "a".repeat(64),
            cursor_bytes: 400,
            source_bytes: 400,
            source_modified_epoch: Some(10),
            source_identity: "17:23".to_owned(),
            complete: true,
        };

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
    fn completed_candidates_do_not_request_another_immediate_batch() {
        let candidate = crate::store::usage_backfill::ProviderUsageBackfillCandidate {
            key: crate::store::SessionKey::new("native", "codex", "synthetic"),
            source_label: "/synthetic/rollout.jsonl".to_owned(),
            account_key: "a".repeat(64),
            cursor_bytes: 0,
            source_bytes: 0,
            source_modified_epoch: None,
            source_identity: String::new(),
            complete: true,
        };
        let candidates = vec![candidate; MAX_CANDIDATES_PER_BATCH + 1];

        assert!(!has_more_ready(&candidates));
    }
}
