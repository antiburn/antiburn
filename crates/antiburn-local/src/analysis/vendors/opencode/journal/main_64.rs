//! Decode the pinned main-64 journal body without a cold attachment.

use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    session_id: String,
    owner_id: Option<String>,
    through_sequence: u64,
    prefix_hash: [u8; 32],
    parser_revision: i64,
    live: State,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    revision: u32,
    baseline: u64,
    origin: [u8; 32],
    boundary: [u8; 32],
    requests: request_context::RequestHistory,
    parts: BTreeMap<String, Part>,
    contexts: VecDeque<(u64, ToolResultContext)>,
    next_context: u64,
    history_safe: bool,
    evidence_gap: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Part {
    state: SavedPart,
    context: Option<u64>,
    last_sequence: u64,
    unsafe_ownership: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedPart {
    identity: Option<(String, Option<String>, Option<String>, String)>,
    revision: u64,
    first_terminal: Option<u64>,
    context: Option<ToolResultContext>,
    invalid: bool,
}

pub(super) fn decode(bytes: &[u8]) -> anyhow::Result<OpenCodeJournalCursor> {
    ensure!(
        bytes.len() <= 1024 * 1024,
        "journal cursor exceeds its state limit"
    );
    let shape: Value = serde_json::from_slice(bytes)?;
    let cursor_fields = [
        "session_id",
        "owner_id",
        "through_sequence",
        "prefix_hash",
        "parser_revision",
        "live",
    ];
    ensure!(
        shape
            .as_object()
            .is_some_and(|object| object.len() == cursor_fields.len()
                && cursor_fields
                    .iter()
                    .all(|field| object.contains_key(*field))),
        OpenCodeJournalReadError::SourceChanged
    );
    let fields = [
        "root",
        "messages",
        "parts",
        "removed_messages",
        "bytes",
        "incomplete",
        "unsafe_identity",
    ];
    ensure!(
        shape["live"]["requests"]
            .as_object()
            .is_some_and(|object| object.len() == fields.len()
                && fields.iter().all(|field| object.contains_key(*field))),
        OpenCodeJournalReadError::SourceChanged
    );
    let old: Cursor = serde_json::from_slice(bytes)?;
    ensure!(
        old.parser_revision == 64
            && old.live.revision == 2
            && old.live.baseline <= old.through_sequence
            && i64::try_from(old.through_sequence).is_ok()
            && old.live.parts.len() <= LIVE_PARTS
            && old.live.contexts.len() <= LIVE_CONTEXTS,
        OpenCodeJournalReadError::SourceChanged
    );
    let context_count = old.live.contexts.len() as u64;
    ensure!(
        old.live.next_context <= old.through_sequence + 1
            && old.live.next_context >= context_count
            && (context_count > 0 || old.live.next_context == 0),
        OpenCodeJournalReadError::SourceChanged
    );
    // The producer allocates consecutive keys and evicts only the cache prefix.
    let first_context = old.live.next_context - context_count;
    ensure!(
        old.live
            .contexts
            .iter()
            .zip(first_context..old.live.next_context)
            .all(|((key, _), allocated)| *key == allocated),
        OpenCodeJournalReadError::SourceChanged
    );
    ensure!(
        old.live
            .parts
            .values()
            .all(|part| part.last_sequence <= old.through_sequence
                && part.state.revision <= part.last_sequence + 1
                && part
                    .state
                    .first_terminal
                    .is_none_or(|first| first <= part.last_sequence && part.state.revision > 0)
                && part.context.is_none_or(|id| id < old.live.next_context
                    && part.state.first_terminal.is_some_and(|first| id <= first))),
        OpenCodeJournalReadError::SourceChanged
    );
    let parts = old
        .live
        .parts
        .into_iter()
        .map(|(id, part)| {
            (
                id,
                LivePart {
                    state: PartState {
                        identity: part.state.identity,
                        revision: part.state.revision,
                        first_terminal: part.state.first_terminal,
                        context: part.state.context,
                        invalid: part.state.invalid,
                        facts: None,
                    },
                    context: part.context,
                    last_sequence: part.last_sequence,
                    unsafe_ownership: part.unsafe_ownership,
                },
            )
        })
        .collect();
    Ok(OpenCodeJournalCursor {
        session_id: old.session_id,
        owner_id: old.owner_id,
        through_sequence: old.through_sequence,
        prefix_hash: old.prefix_hash,
        parser_revision: 64,
        main_64: true,
        cold_replay: None,
        live: Some(LiveState {
            revision: 2,
            baseline: old.live.baseline,
            origin: old.live.origin,
            boundary: old.live.boundary,
            requests: old.live.requests,
            parts,
            contexts: old.live.contexts,
            next_context: old.live.next_context,
            history_safe: old.live.history_safe,
            evidence_gap: old.live.evidence_gap,
            public: Default::default(),
            public_baseline: None,
        }),
    })
}

#[cfg(test)]
mod tests;
