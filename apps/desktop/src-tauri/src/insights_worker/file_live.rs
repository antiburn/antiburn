//! One native file page runs independently of historical projection work.

use antiburn_local::analysis::file_live::{
    NativeFileCursor, NativeFilePage, NativeFileReadError, read_native_file_page,
};
use antiburn_local::analysis::{RawSource, SessionInput, ToolResultPosition, UserMessagePosition};

use crate::store::native_tool_results::file_delivery::{MAX_CURSOR_BYTES, source_format};
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
    let cursor = match claim
        .cursor_json
        .as_deref()
        .map(|encoded| {
            anyhow::ensure!(
                encoded.len() <= MAX_CURSOR_BYTES,
                "native file cursor exceeds its limit"
            );
            Ok::<_, anyhow::Error>(serde_json::from_str::<NativeFileCursor>(encoded)?)
        })
        .transpose()
    {
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
    let result = tokio::task::spawn_blocking(move || read(input, cursor)).await?;
    match result {
        Ok(page) => {
            let results = page
                .results
                .iter()
                .map(|result| {
                    let mut observation = result.observation.clone();
                    observation.position = ToolResultPosition::FileBytes {
                        record_start: result.position.record_start,
                        record_end: result.position.record_end,
                        item_index: result.position.item_index,
                    };
                    observation.revision = result.position.record_end;
                    if let Some(context) = &mut observation.context {
                        context.result_position = observation.position;
                        for (message, position) in context
                            .user_messages
                            .iter_mut()
                            .zip(&result.request_positions)
                        {
                            message.position = UserMessagePosition::FileBytes {
                                record_start: position.record_start,
                                record_end: position.record_end,
                            };
                        }
                    }
                    observation
                })
                .collect::<Vec<_>>();
            if store.publish_native_file_page(&claim, &page, &results, clock())? {
                announce(&claim.key);
            }
        }
        Err(error) => {
            let failure = error
                .downcast_ref::<NativeFileReadError>()
                .copied()
                .ok_or(error)?;
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
