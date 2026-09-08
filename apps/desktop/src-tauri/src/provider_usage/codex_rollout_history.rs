//! Bounded, resumable import of Codex rollout rate-limit readings into
//! durable provider usage observations.
//!
//! Every turn the Codex CLI runs, it appends a `token_count` event to its
//! session's rollout file, and that event carries the account's rate limits
//! as the server reported them on that turn (see
//! `provider_usage::live::sources::codex_rollout` for the live tail-reading
//! path this module shares its window parsing with). A machine that already
//! has months of rollout files therefore already has months of meter
//! readings sitting on disk; this module reads them into
//! `provider_usage_observation` so the limit factor has samples from
//! history, without waiting for a live poll.
//!
//! The importer selects only `token_count.rate_limits` metadata. It does not
//! retain or log transcript message content.
//!
//! # Bounded and resumable
//!
//! [`read_rollout_batch`] reads at most [`MAX_BATCH_BYTES`] per call, from an
//! explicit byte offset, and only ever returns complete JSONL records. The
//! offset it returns is saved as a checkpoint
//! (`store::codex_rollout_checkpoint`) and resumed on the next pass, so a
//! multi-megabyte rollout is read in slices across many background ticks
//! rather than all at once. [`import_rollout_batch`] is the per-pass entry
//! point: it looks at up to [`MAX_CANDIDATES_PER_PASS`] rollout files, or
//! stops early once [`MAX_PASS_DURATION`] has elapsed.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::live::model::{Confidence, Freshness, ProviderUsageSnapshot, UsageSource, UsageWindow};
use super::live::sources::codex_rollout::parse_windows;
use crate::store::Store;
use crate::store::codex_rollout_checkpoint::{RolloutCandidate, RolloutCheckpoint};

/// Stable provenance for an observation read from a Codex rollout file
/// rather than a live poll. `provider_usage::factor` marks a delta sample
/// whose closing observation carries this id with kind `rollout`.
pub(crate) const CODEX_ROLLOUT_SOURCE_ID: &str = "codex-rollout-backfill";

const OPENAI: &str = "openai";

/// The most one call to [`read_rollout_batch`] reads. Wide enough that a
/// session with normal turn cadence advances by many readings per pass,
/// small enough that one pass never stalls a background tick.
const MAX_BATCH_BYTES: usize = 256 * 1024;

/// Rollout files one call to [`import_rollout_batch`] may open.
const MAX_CANDIDATES_PER_PASS: usize = 8;

/// How long one call to [`import_rollout_batch`] may run before it stops
/// early and asks to be resumed on the next tick.
const MAX_PASS_DURATION: Duration = Duration::from_millis(500);

/// A bounded read of complete rollout records, and the offset it is safe to
/// resume from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RolloutBatch {
    pub readings: Vec<RolloutReading>,
    pub next_offset: u64,
    /// Whether `next_offset` has reached the end of the file as it stood at
    /// read time. A file that keeps growing is never "complete" for long.
    pub complete: bool,
}

/// One account-wide allowance report from a rollout event.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RolloutReading {
    pub observed_at: OffsetDateTime,
    pub plan: Option<String>,
    pub windows: Vec<UsageWindow>,
}

impl RolloutReading {
    /// Build one historical snapshot for a directly attributed account.
    ///
    /// `plan_tier` stays `None`: Codex rollout events carry `plan_type` but
    /// no finer-grained tier, unlike Claude's `rateLimitTier`.
    pub(crate) fn snapshot(&self, account_key: &str) -> ProviderUsageSnapshot {
        ProviderUsageSnapshot {
            provider: OPENAI,
            account: Some(account_key.to_owned()),
            account_uuid: None,
            account_email: None,
            plan: self.plan.clone(),
            plan_tier: None,
            observed_at: self.observed_at,
            source: UsageSource {
                id: CODEX_ROLLOUT_SOURCE_ID,
                label: "Codex local rollout history".to_owned(),
                confidence: Confidence::High,
                // Fresh at the recorded observation time, not now.
                freshness: Freshness::Fresh,
            },
            windows: self.windows.clone(),
            supplemental: None,
            reset_credits: None,
        }
    }
}

/// The result of one bounded, resumable import pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RolloutImportBatch {
    pub imported_observations: usize,
    pub completed_sources: usize,
    pub deferred_sources: usize,
    /// The scheduler can resume file work without waiting for its next tick.
    pub continue_soon: bool,
    /// The earliest deferred source that needs another local attempt.
    pub next_retry_epoch: Option<i64>,
    pub pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceFingerprint {
    bytes: u64,
    modified_epoch: Option<i64>,
    identity: String,
}

/// Import a bounded page of retained Codex rollout readings, without any
/// provider call.
///
/// A partial file keeps its byte offset in the local store. The next pass
/// resumes there, and a file is complete only after every complete record
/// it holds has been read. A reading older than the observation retention
/// cutoff is never imported, since a later retention pass would only delete
/// it again.
pub(crate) fn import_rollout_batch(store: &Store, now_epoch: i64) -> Result<RolloutImportBatch> {
    let started = Instant::now();
    let mut result = RolloutImportBatch::default();
    let cutoff = store.provider_usage_retention_cutoff_epoch(now_epoch)?;

    let candidates =
        store.provider_usage_rollout_candidates(cutoff, now_epoch, MAX_CANDIDATES_PER_PASS + 1)?;
    let has_more_ready = has_more_ready(&candidates);
    for candidate in candidates.iter().take(MAX_CANDIDATES_PER_PASS) {
        if started.elapsed() >= MAX_PASS_DURATION {
            result.pending = true;
            result.continue_soon = true;
            break;
        }
        let path = Path::new(&candidate.source_label);
        let fingerprint = match source_fingerprint(path) {
            Ok(fingerprint) => fingerprint,
            Err(_) => {
                defer(
                    store,
                    candidate,
                    candidate.cursor_bytes,
                    now_epoch,
                    &mut result,
                )?;
                continue;
            }
        };
        if candidate.complete
            && fingerprint.bytes == candidate.source_bytes
            && fingerprint.modified_epoch == candidate.source_modified_epoch
            && fingerprint.identity == candidate.source_identity
        {
            store.touch_rollout_checkpoint(&candidate.key, now_epoch)?;
            continue;
        }
        let offset = resume_offset(candidate, &fingerprint);
        let batch = match read_rollout_batch(path, offset, MAX_BATCH_BYTES) {
            Ok(batch) => batch,
            Err(_) => {
                defer(
                    store,
                    candidate,
                    candidate.cursor_bytes,
                    now_epoch,
                    &mut result,
                )?;
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

        let checkpoint = RolloutCheckpoint {
            cursor_bytes: batch.next_offset,
            source_bytes: fingerprint.bytes,
            source_modified_epoch: fingerprint.modified_epoch,
            source_identity: fingerprint.identity,
            complete: batch.complete,
        };
        store.upsert_rollout_checkpoint(
            &candidate.key,
            &candidate.source_label,
            &checkpoint,
            now_epoch,
        )?;

        if needs_retry(&batch, offset) {
            defer(store, candidate, batch.next_offset, now_epoch, &mut result)?;
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
    if let Some(retry) = store.rollout_checkpoint_next_retry_epoch(now_epoch)? {
        result.pending = true;
        result.next_retry_epoch = earliest(result.next_retry_epoch, retry);
    }
    Ok(result)
}

fn defer(
    store: &Store,
    candidate: &RolloutCandidate,
    cursor_bytes: u64,
    now_epoch: i64,
    result: &mut RolloutImportBatch,
) -> Result<()> {
    let retry = store.defer_rollout_checkpoint(
        &candidate.key,
        &candidate.source_label,
        cursor_bytes,
        now_epoch,
    )?;
    result.deferred_sources += 1;
    result.pending = true;
    result.next_retry_epoch = earliest(result.next_retry_epoch, retry);
    Ok(())
}

fn earliest(current: Option<i64>, candidate: i64) -> Option<i64> {
    Some(current.map_or(candidate, |current| current.min(candidate)))
}

fn has_more_ready(candidates: &[RolloutCandidate]) -> bool {
    candidates
        .get(MAX_CANDIDATES_PER_PASS)
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

/// The byte offset to resume a rollout file from: the saved cursor, unless
/// the file was replaced (a different identity, or shrunk below the
/// cursor), in which case it restarts from zero.
fn resume_offset(candidate: &RolloutCandidate, fingerprint: &SourceFingerprint) -> u64 {
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

/// Read complete JSONL records inside one explicit byte budget.
///
/// The returned offset never advances past an unfinished record at EOF. A
/// record longer than the budget is skipped in fixed-size fragments,
/// without allocating the record or interpreting its content.
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
    })
}

/// One rollout line, when it is an `event_msg`/`token_count` event carrying
/// a non-null `rate_limits` whose `limit_id` is `"codex"`, null, or absent —
/// the account-wide bucket, not a model- or feature-scoped one.
fn parse_rollout_line(line: &[u8]) -> Option<RolloutReading> {
    let value: Value = serde_json::from_slice(line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let rate_limits = payload
        .get("rate_limits")
        .filter(|value| !value.is_null())?;
    let limit_id = rate_limits.get("limit_id").and_then(Value::as_str);
    if !matches!(limit_id, None | Some("codex")) {
        return None;
    }
    let observed_at = OffsetDateTime::parse(value.get("timestamp")?.as_str()?, &Rfc3339).ok()?;
    let windows = parse_windows(rate_limits, observed_at)?;
    if windows.is_empty() {
        return None;
    }
    let plan = rate_limits
        .get("plan_type")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(RolloutReading {
        observed_at,
        plan,
        windows,
    })
}

#[cfg(test)]
mod tests;
