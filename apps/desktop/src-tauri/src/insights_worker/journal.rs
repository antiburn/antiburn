//! One bounded native journal delivery turn, independent of projection freshness.

use std::future::Future;
use std::pin::Pin;

use antiburn_local::analysis::OpenCodeJournalReadError;
use antiburn_local::discovery::SessionSource;

use super::{
    LEASE_SECS, PassSignal, SessionKey, Store, analysis, backoff_secs, lease_renew_interval,
};
use crate::store::native_tool_results::journal::JournalFailure;

pub(super) fn owns_session(record: &super::SessionRecord) -> bool {
    record.key.environment_key == "native"
        && record.wsl_distro.is_none()
        && record.key.agent == "opencode"
        && record.source_kind == "providerDb"
}

type SourceFuture = Pin<Box<dyn Future<Output = Option<SessionSource>> + Send>>;
type SourceLocator<'a> = dyn Fn(&SessionKey) -> SourceFuture + Send + Sync + 'a;

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
    let Some(source) = locate(&claim.key).await else {
        if store.fail_opencode_journal(&claim, JournalFailure::Unavailable, clock(), retry_at())? {
            announce(&claim.key);
        }
        return Ok(true);
    };
    let signal = PassSignal::new();
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
            store.publish_opencode_journal_page(&claim, &page, clock())?
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
            store.fail_opencode_journal(&claim, failure, clock(), retry_at())?
        }
    };
    if applied {
        announce(&claim.key);
    }
    Ok(true)
}

#[cfg(test)]
pub(crate) mod tests;
