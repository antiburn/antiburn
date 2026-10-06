use antiburn_local::analysis::{
    ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, FenceScope, PARSER_REVISION,
    SelectedContentCursor, SelectedContentPage, SelectedContentQueryError, SelectedContentRequest,
    query_turn_content_keyset_selected,
};
use rusqlite::{OptionalExtension, params};

use super::{EVIDENCE_BY_KEY_SQL, SessionKey, Store, evidence_from_row, turn_session_key};

pub(crate) const SELECTED_CONTENT_PROGRESS_REVISION: u32 = 1;

/// Shared local continuation state for selected-evidence checks.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct SelectedContentProgress {
    pub revision: u32,
    pub cursor: Option<SelectedContentCursor>,
}

impl Store {
    /// Validate publication freshness and read selected evidence in one transaction.
    pub fn published_turn_content_keyset_selected(
        &self,
        key: &SessionKey,
        request: SelectedContentRequest<'_>,
    ) -> Result<Option<SelectedContentPage>, SelectedContentQueryError> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let Some(evidence) = transaction
            .query_row(
                EVIDENCE_BY_KEY_SQL,
                params![key.environment_key, key.agent, key.session_id],
                evidence_from_row,
            )
            .optional()?
        else {
            return Ok(None);
        };
        let Some(fence) = evidence.published_fence else {
            return Ok(None);
        };
        let current: Option<(i64, Option<String>)> = transaction
            .query_row(
                "SELECT source_generation, source_fingerprint FROM session
                 WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                params![key.environment_key, key.agent, key.session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((generation, fingerprint)) = current else {
            return Ok(None);
        };
        if evidence.status.as_str() != "ready"
            || evidence.analyzed_generation != Some(generation)
            || evidence.processed_fingerprint != fingerprint
            || evidence.parser_revision != Some(PARSER_REVISION)
            || evidence.analyzer_revision != Some(ANALYZER_REVISION)
            || evidence.evidence_schema_revision != Some(EVIDENCE_SCHEMA_REVISION)
            || request.source_generation != generation
        {
            return Ok(None);
        }
        let page = query_turn_content_keyset_selected(
            &transaction,
            &turn_session_key(key),
            &FenceScope::single(fence),
            request,
        )?;
        transaction.commit()?;
        Ok(Some(page))
    }
}

#[cfg(test)]
mod tests;
