//! One native file page runs independently of historical projection work.

use antiburn_local::analysis::file_live::{
    NativeFileCursor, NativeFilePage, NativeFileReadError, read_native_file_page,
};
use antiburn_local::analysis::{RawSource, SessionInput};

use crate::store::native_tool_results::source_delivery::native_file::source_format;
use crate::store::{SessionKey, Store};

pub(crate) async fn process_next(
    store: &Store,
    clock: &(dyn Fn() -> i64 + Send + Sync),
    announce: &(dyn Fn(&SessionKey) + Send + Sync),
) -> anyhow::Result<bool> {
    process_next_with(store, clock, announce, |input, cursor| {
        read_native_file_page(&input, cursor.as_ref(), &|| false)
    })
    .await
}

async fn process_next_with(
    store: &Store,
    clock: &(dyn Fn() -> i64 + Send + Sync),
    announce: &(dyn Fn(&SessionKey) + Send + Sync),
    read: impl FnOnce(SessionInput, Option<NativeFileCursor>) -> anyhow::Result<NativeFilePage>
    + Send
    + 'static,
) -> anyhow::Result<bool> {
    let Some(claim) = store.claim_next_native_file(clock(), super::LEASE_SECS)? else {
        return Ok(false);
    };
    let cursor = match claim.cursor() {
        Ok(cursor) => cursor,
        Err(error) => {
            ::tracing::warn!(event = "native_file_cursor_invalid", error = %error);
            if store.fail_native_file(
                &claim,
                NativeFileReadError::InvalidCursor,
                clock(),
                clock(),
            )? {
                announce(&claim.key);
            }
            return Ok(true);
        }
    };
    let input = SessionInput {
        agent: if claim.key.agent == "claude-code" {
            "claude".into()
        } else {
            claim.key.agent.clone()
        },
        session_id: claim.key.session_id.clone(),
        source: RawSource::File(claim.source_label.clone().into()),
        source_format: source_format(&claim.key.agent).expect("file claim has a supported source"),
        fork_parent_session_id: None,
    };
    let result = match tokio::task::spawn_blocking(move || read(input, cursor)).await {
        Ok(result) => result,
        Err(error) => {
            ::tracing::error!(event = "native_file_read_task_failed", error = %error);
            let retry_at = clock() + super::backoff_secs(claim.retry_count);
            if store.fail_native_file(&claim, NativeFileReadError::ReadFailed, clock(), retry_at)? {
                announce(&claim.key);
            }
            return Ok(true);
        }
    };
    match result {
        Ok(page) => {
            if store.publish_native_file_page(&claim, &page, &[], clock())? {
                announce(&claim.key);
            }
        }
        Err(error) => {
            let failure = match error.downcast_ref::<NativeFileReadError>() {
                Some(failure) => *failure,
                None => {
                    ::tracing::warn!(
                        event = "native_file_read_failed_untyped",
                        agent = %claim.key.agent,
                        error = %error
                    );
                    NativeFileReadError::ReadFailed
                }
            };
            if failure != NativeFileReadError::Cancelled {
                ::tracing::warn!(event = "native_file_read_failed", agent = %claim.key.agent, error = %failure);
            }
            let retry_at = clock() + super::backoff_secs(claim.retry_count);
            if store.fail_native_file(&claim, failure, clock(), retry_at)?
                && failure != NativeFileReadError::Cancelled
            {
                announce(&claim.key);
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
