//! One bounded native journal delivery turn, independent of projection freshness.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use antiburn_local::analysis::OpenCodeJournalReadError;
use antiburn_local::discovery::SessionSource;

use super::{
    LEASE_SECS, PassSignal, SessionKey, Store, analysis, backoff_secs, lease_renew_interval,
};
use crate::store::native_tool_results::source_delivery::opencode_journal::JournalFailure;

pub(super) fn owns_session(record: &super::SessionRecord) -> bool {
    record.wsl_distro.is_none()
        && crate::store::native_opencode_sqlite_format(
            &record.key,
            &record.source_kind,
            &record.source_label,
        ) == Some(antiburn_local::analysis::SourceFormat::OpenCodeSqliteV1)
}

type SourceFuture = Pin<Box<dyn Future<Output = Option<SessionSource>> + Send>>;
type SourceLocator<'a> = dyn Fn(&SessionKey) -> SourceFuture + Send + Sync + 'a;

#[cfg(not(test))]
fn source_locate_timeout() -> Duration {
    Duration::from_secs((LEASE_SECS / 2) as u64)
}

#[cfg(test)]
fn source_locate_timeout() -> Duration {
    Duration::from_secs(1)
}

pub(super) async fn process_next(
    store: &Store,
    clock: &(dyn Fn() -> i64 + Send + Sync),
    announce: &(dyn Fn(&SessionKey) + Send + Sync),
    on_claimed: &(dyn Fn() + Send + Sync),
) -> anyhow::Result<bool> {
    process_next_with(store, clock, announce, on_claimed, &|key| {
        let session = key.session_id.clone();
        Box::pin(async move {
            analysis::locate(antiburn_local::model::AgentKind::OpenCode, &session, None).await
        })
    })
    .await
}

async fn process_next_with(
    store: &Store,
    clock: &(dyn Fn() -> i64 + Send + Sync),
    announce: &(dyn Fn(&SessionKey) + Send + Sync),
    on_claimed: &(dyn Fn() + Send + Sync),
    locate: &SourceLocator<'_>,
) -> anyhow::Result<bool> {
    let Some(claim) = store.claim_next_opencode_journal(clock(), LEASE_SECS)? else {
        return Ok(false);
    };
    on_claimed();
    let claim_started = std::time::Instant::now();
    ::tracing::debug!(
        event = "opencode_journal_claimed",
        session_id = %claim.key.session_id,
        retry_count = claim.retry_count,
        full_read = claim.requires_full_read,
        baseline_sequence = claim.baseline_sequence,
    );
    let retry_at = || clock() + backoff_secs(claim.retry_count);
    let cursor = match claim.cursor() {
        Ok(cursor) => cursor,
        Err(_) => {
            if store.fail_opencode_journal(
                &claim,
                JournalFailure::InvalidCursor,
                clock(),
                retry_at(),
            )? {
                announce(&claim.key);
            }
            return Ok(true);
        }
    };
    let locate_started = std::time::Instant::now();
    let located = tokio::time::timeout(source_locate_timeout(), locate(&claim.key)).await;
    let Some(source) = located.ok().flatten() else {
        ::tracing::warn!(
            event = "opencode_journal_source_unavailable",
            session_id = %claim.key.session_id,
            elapsed_ms = locate_started.elapsed().as_millis() as u64,
        );
        if store.fail_opencode_journal(&claim, JournalFailure::Unavailable, clock(), retry_at())? {
            announce(&claim.key);
        }
        return Ok(true);
    };
    ::tracing::debug!(
        event = "opencode_journal_source_located",
        session_id = %claim.key.session_id,
        elapsed_ms = locate_started.elapsed().as_millis() as u64,
    );
    let signal = PassSignal::new();
    let read_started = std::time::Instant::now();
    let future = analysis::read_opencode_journal_page(
        source,
        claim.key.session_id.clone(),
        cursor,
        claim.baseline_sequence,
        claim.requires_full_read,
        signal.clone(),
    );
    tokio::pin!(future);
    let mut progress = signal.progress();
    let result = loop {
        tokio::select! {
            result = &mut future => break result,
            () = tokio::time::sleep(lease_renew_interval()) => {
                let observed = signal.progress();
                if observed == progress { continue; }
                progress = observed;
                if !store.renew_opencode_journal_lease(&claim, clock(), LEASE_SECS)? {
                    signal.cancel();
                    let _ = future.await;
                    return Ok(true);
                }
            }
        }
    };
    let applied = match result {
        Ok(Some(page)) if !signal.cancelled() => {
            ::tracing::debug!(
                event = "opencode_journal_page_read",
                session_id = %claim.key.session_id,
                elapsed_ms = read_started.elapsed().as_millis() as u64,
                input_events = page.live_input_events,
                input_bytes = page.live_input_bytes,
                results = page.results.len(),
                has_more = page.has_more,
                snapshot_end = page.snapshot_end,
                history_complete = page.history_complete,
            );
            let publish_started = std::time::Instant::now();
            let applied = store.publish_opencode_journal_page(&claim, &page, clock())?;
            ::tracing::debug!(
                event = "opencode_journal_publish_finished",
                session_id = %claim.key.session_id,
                elapsed_ms = publish_started.elapsed().as_millis() as u64,
                applied,
            );
            applied
        }
        Ok(None) if !signal.cancelled() => {
            store.finish_opencode_journal_poll(&claim, clock())?;
            return Ok(true);
        }
        result => {
            let failure = if signal.cancelled() {
                JournalFailure::Cancelled
            } else {
                match result
                    .as_ref()
                    .err()
                    .and_then(|error| error.downcast_ref::<OpenCodeJournalReadError>())
                {
                    Some(OpenCodeJournalReadError::SourceChanged) => JournalFailure::SourceChanged,
                    Some(OpenCodeJournalReadError::IncompleteHistory) => {
                        JournalFailure::IncompleteHistory
                    }
                    Some(OpenCodeJournalReadError::Unavailable) => JournalFailure::Unavailable,
                    None => JournalFailure::ReadFailed,
                }
            };
            ::tracing::warn!(
                event = "opencode_journal_read_failed",
                session_id = %claim.key.session_id,
                failure = ?failure,
                retry_count = claim.retry_count,
                elapsed_ms = claim_started.elapsed().as_millis() as u64,
            );
            store.fail_opencode_journal(&claim, failure, clock(), retry_at())?
        }
    };
    if applied {
        ::tracing::debug!(
            event = "opencode_journal_page_published",
            session_id = %claim.key.session_id,
            elapsed_ms = claim_started.elapsed().as_millis() as u64,
        );
        announce(&claim.key);
    }
    Ok(true)
}

#[cfg(test)]
pub(crate) mod tests;
