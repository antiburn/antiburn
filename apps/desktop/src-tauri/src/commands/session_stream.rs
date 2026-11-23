//! Demand-driven activity delivery for the main Sessions list.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::Manager;
use tokio::sync::Mutex;

use crate::dto::ActivityEntry;
use crate::scan;
use crate::store::{
    ActiveCursor, DisabledAgents, MAX_ACTIVITY_DAYS, MIN_ACTIVITY_DAYS, RepositoryRecord, Store,
};

use super::{CommandResult, activity_entry, fail, run_blocking};

const BATCH_SIZE: usize = 50;

/// The main window owns one scan position. The lock serializes reads and replacement.
#[derive(Default)]
pub(crate) struct ActivityStreamState(Mutex<Option<ActivityStream>>);

struct ActivityStream {
    id: String,
    window_days: u32,
    excluded: DisabledAgents,
    position: Option<ActiveCursor>,
    membership_sequence: i64,
    next_read_sequence: u64,
    last_read: Option<(u32, ActivityBatch)>,
    next_refresh_sequence: u64,
    last_refresh: Option<(u32, ActivityBatch)>,
    repository_labels: Option<(i64, Arc<[RepositoryRecord]>)>,
}

/// The renderer controls demand. The shell keeps the scan position.
#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum ActivityStreamRequest {
    Open {
        id: String,
        window_days: u32,
    },
    Read {
        id: String,
        sequence: u32,
    },
    Refresh {
        id: String,
        reconcile: bool,
        sequence: u32,
    },
    Close {
        id: String,
    },
}

/// One bounded response. Open and refresh also report the range total and agents.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActivityBatch {
    entries: Vec<ActivityEntry>,
    exhausted: bool,
    agents: Option<Vec<String>>,
    total: Option<u64>,
    reset: bool,
    replayed_refresh: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_read_sequence: Option<u64>,
}

/// Return one batch per request. Refresh the head without advancing the tail
/// unless reconciliation finds writes that may have changed retained rows.
#[tauri::command]
pub(crate) async fn session_activity_stream(
    app: tauri::AppHandle,
    state: tauri::State<'_, ActivityStreamState>,
    request: ActivityStreamRequest,
) -> CommandResult<Option<ActivityBatch>> {
    let mut active = state.0.lock().await;
    let at_head = matches!(
        &request,
        ActivityStreamRequest::Open { .. } | ActivityStreamRequest::Refresh { .. }
    );
    let reconcile = matches!(
        &request,
        ActivityStreamRequest::Refresh {
            reconcile: true,
            ..
        }
    );
    let (id, read_sequence, refresh_sequence) = match request {
        ActivityStreamRequest::Open { id, window_days } => {
            let excluded = app.state::<Store>().settings_snapshot().disabled_agents;
            *active = Some(ActivityStream {
                id: id.clone(),
                window_days: window_days.clamp(MIN_ACTIVITY_DAYS, MAX_ACTIVITY_DAYS),
                excluded,
                position: None,
                membership_sequence: 0,
                next_read_sequence: 0,
                last_read: None,
                next_refresh_sequence: 0,
                last_refresh: None,
                repository_labels: None,
            });
            (id, None, None)
        }
        ActivityStreamRequest::Read { id, sequence } => (id, Some(sequence), None),
        ActivityStreamRequest::Refresh { id, sequence, .. } => (id, None, Some(sequence)),
        ActivityStreamRequest::Close { id } => {
            if active.as_ref().is_some_and(|stream| stream.id == id) {
                *active = None;
                run_blocking(move || app.state::<Store>().stop_activity_tracking().map_err(fail))
                    .await?;
            }
            return Ok(None);
        }
    };
    let stream = active
        .as_mut()
        .filter(|stream| stream.id == id)
        .ok_or_else(|| "session activity stream is closed".to_owned())?;
    // A timed-out reply can arrive after the native scan advances.
    if let Some(sequence) = read_sequence
        && let Some(batch) = request_replay(
            stream.next_read_sequence,
            &stream.last_read,
            sequence,
            "read",
        )?
    {
        return Ok(Some(batch.clone()));
    }
    // Replay a lost refresh response without changing the cursor or watermark.
    if let Some(sequence) = refresh_sequence
        && let Some(batch) = request_replay(
            stream.next_refresh_sequence,
            &stream.last_refresh,
            sequence,
            "refresh",
        )?
    {
        return Ok(Some(ActivityBatch {
            replayed_refresh: true,
            ..batch.clone()
        }));
    }
    let now = scan::unix_now();
    let zone = jiff::tz::TimeZone::system();
    let range_start = jiff::Timestamp::from_second(now)
        .map_err(fail)?
        .to_zoned(zone.clone())
        .date()
        .checked_sub(jiff::Span::new().days(i64::from(stream.window_days - 1)))
        .and_then(|date| date.to_zoned(zone))
        .map_err(fail)?
        .timestamp()
        .as_second();
    // Keep the rolling buffer for active rows near the calendar boundary.
    let since = range_start.min(now - i64::from(stream.window_days) * 86_400);
    let excluded = stream.excluded.clone();
    let tail = stream.position.clone();
    let refresh = at_head && tail.is_some();
    let membership_sequence = stream.membership_sequence;
    let next_read_sequence = stream.next_read_sequence;
    let mut repository_labels = stream.repository_labels.clone();
    let (batch, next_position, sequence, repository_labels) = run_blocking(move || {
        let store = app.state::<Store>();
        if at_head && !refresh {
            store
                .start_activity_tracking(&excluded, false)
                .map_err(fail)?;
        }
        // Capture the watermark before reading rows. A concurrent admission
        // will be detected on a later head refresh.
        let mut sequence = if at_head {
            store.activity_membership_sequence().map_err(fail)?
        } else {
            membership_sequence
        };
        let after = if at_head { None } else { tail.as_ref() };
        let mut rows = store
            .stream_recent_sessions(since, BATCH_SIZE + 1, &excluded, after)
            .map_err(fail)?;
        let reset = if let (true, Some(tail)) = (refresh, &tail) {
            if rows.len() <= BATCH_SIZE
                || (reconcile
                    && store
                        .activity_range_changed(tail.epoch.max(since))
                        .map_err(fail)?)
            {
                true
            } else if sequence > membership_sequence {
                store
                    .activity_has_admissions(
                        membership_sequence,
                        tail.epoch,
                        rows[BATCH_SIZE - 1].updated_at_epoch.unwrap_or_default(),
                        &excluded,
                    )
                    .map_err(fail)?
            } else {
                false
            }
        } else {
            false
        };
        if reset {
            // Start tracking before re-reading the head so a concurrent write
            // cannot fall between the new snapshot and its observation interval.
            store
                .start_activity_tracking(&excluded, true)
                .map_err(fail)?;
            sequence = store.activity_membership_sequence().map_err(fail)?;
            rows = store
                .stream_recent_sessions(since, BATCH_SIZE + 1, &excluded, None)
                .map_err(fail)?;
        }
        let exhausted = if refresh && !reset {
            store
                .stream_recent_sessions(since, 1, &excluded, tail.as_ref())
                .map_err(fail)?
                .is_empty()
        } else {
            rows.len() <= BATCH_SIZE
        };
        rows.truncate(BATCH_SIZE);
        let next_position = rows.last().map(|row| ActiveCursor {
            epoch: row.updated_at_epoch.unwrap_or_default(),
            session_id: row.key.session_id.clone(),
            environment_key: row.key.environment_key.clone(),
            agent: row.key.agent.clone(),
        });
        let agents = at_head
            .then(|| store.activity_agents(since, &excluded))
            .transpose()
            .map_err(fail)?;
        let total = at_head
            .then(|| store.activity_session_count(range_start, now, &excluded))
            .transpose()
            .map_err(fail)?;
        let repositories = if rows.iter().any(|row| {
            row.key.remote_host_id().is_none()
                && row.cwd.as_ref().is_some_and(|cwd| !cwd.is_empty())
        }) {
            // Read the revision first so a concurrent label change invalidates the cache.
            let revision = store.activity_repository_revision().map_err(fail)?;
            if !repository_labels
                .as_ref()
                .is_some_and(|(cached, _)| *cached == revision)
            {
                repository_labels = Some((revision, store.repositories().map_err(fail)?.into()));
            }
            repository_labels
                .as_ref()
                .map(|(_, labels)| labels.as_ref())
                .unwrap_or_default()
        } else {
            &[]
        };
        let entries = rows
            .into_iter()
            .map(|row| activity_entry(&store, repositories, row, now).map_err(fail))
            .collect::<Result<Vec<_>, _>>()?;
        if entries
            .iter()
            .any(|entry| entry.cost.is_none() && !entry.models.is_empty())
        {
            crate::runtime_pricing::request_refresh(&app);
        }
        if reset {
            store.publish_activity_tracking().map_err(fail)?;
        }
        Ok((
            ActivityBatch {
                entries,
                exhausted,
                agents,
                total,
                reset,
                replayed_refresh: false,
                next_read_sequence: reset.then_some(next_read_sequence),
            },
            next_position,
            sequence,
            repository_labels,
        ))
    })
    .await?;
    if batch.reset || (!refresh && next_position.is_some()) {
        stream.position = next_position;
    }
    if batch.reset {
        // Keep the sequence across a reset and report it to the renderer.
        // An older read cannot advance the new tail.
        stream.last_read = None;
    } else if let Some(sequence) = read_sequence {
        stream.next_read_sequence = u64::from(sequence) + 1;
        stream.last_read = Some((sequence, batch.clone()));
    } else if refresh && let Some((_, replay)) = &mut stream.last_read {
        replay.exhausted = batch.exhausted;
    }
    if let Some(sequence) = refresh_sequence {
        stream.next_refresh_sequence = u64::from(sequence) + 1;
        stream.last_refresh = Some((sequence, batch.clone()));
    }
    stream.membership_sequence = sequence;
    stream.repository_labels = repository_labels;
    Ok(Some(batch))
}

/// Replay the latest completed request of this kind. Reject gaps and older requests.
fn request_replay<'a, T>(
    next: u64,
    last: &'a Option<(u32, T)>,
    sequence: u32,
    kind: &str,
) -> CommandResult<Option<&'a T>> {
    if let Some((cached, batch)) = last
        && *cached == sequence
    {
        return Ok(Some(batch));
    }
    if u64::from(sequence) != next {
        return Err(format!("session activity {kind} sequence is out of order"));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::request_replay;

    #[test]
    fn read_replay_rejects_stale_and_future_sequences_before_and_after_reset() {
        let last = Some((1, "second batch"));
        assert_eq!(
            request_replay(2, &last, 1, "read").unwrap(),
            Some(&"second batch")
        );
        assert!(request_replay(2, &last, 0, "read").is_err());
        assert!(request_replay(2, &last, 3, "read").is_err());
        assert_eq!(request_replay(2, &last, 2, "read").unwrap(), None);

        let reset: Option<(u32, &str)> = None;
        assert!(request_replay(2, &reset, 1, "read").is_err());
        assert_eq!(request_replay(2, &reset, 2, "read").unwrap(), None);
    }

    #[test]
    fn refresh_replay_accepts_the_latest_response_with_or_without_a_reset() {
        for reset in [false, true] {
            let last = Some((1, reset));
            assert_eq!(
                request_replay(2, &last, 1, "refresh").unwrap(),
                Some(&reset)
            );
            assert!(request_replay(2, &last, 0, "refresh").is_err());
            assert!(request_replay(2, &last, 3, "refresh").is_err());
            assert_eq!(request_replay(2, &last, 2, "refresh").unwrap(), None);
        }
    }
}
