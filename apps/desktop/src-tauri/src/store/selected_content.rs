use antiburn_local::analysis::{
    ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, FenceScope, PARSER_REVISION,
    ScopedSelectedContentPage, SelectedContentPage, SelectedContentQueryError,
    SelectedContentRequest, SelectedContentScope, query_turn_content_keyset_scoped,
    query_turn_content_keyset_selected,
};
use rusqlite::{OptionalExtension, params};

use super::{EVIDENCE_BY_KEY_SQL, SessionKey, Store, evidence_from_row, turn_session_key};

impl Store {
    /// Validate publication freshness and read selected evidence in one transaction.
    pub fn published_turn_content_keyset_selected(
        &self,
        key: &SessionKey,
        request: SelectedContentRequest<'_>,
    ) -> Result<Option<SelectedContentPage>, SelectedContentQueryError> {
        self.with_published_content(key, request.source_generation, |connection, fence| {
            query_turn_content_keyset_selected(
                connection,
                &turn_session_key(key),
                &FenceScope::single(fence),
                request,
            )
        })
    }

    pub fn published_turn_content_keyset_scoped(
        &self,
        key: &SessionKey,
        request: SelectedContentRequest<'_>,
        scope: &SelectedContentScope,
    ) -> Result<Option<ScopedSelectedContentPage>, SelectedContentQueryError> {
        self.with_published_content(key, request.source_generation, |connection, fence| {
            query_turn_content_keyset_scoped(
                connection,
                &turn_session_key(key),
                &FenceScope::single(fence),
                request,
                scope,
            )
        })
    }

    pub(crate) fn with_published_content<T>(
        &self,
        key: &SessionKey,
        source_generation: i64,
        read: impl FnOnce(&rusqlite::Connection, i64) -> Result<T, SelectedContentQueryError>,
    ) -> Result<Option<T>, SelectedContentQueryError> {
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
            || source_generation != generation
        {
            return Ok(None);
        }
        let page = read(&transaction, fence)?;
        transaction.commit()?;
        Ok(Some(page))
    }
}

#[cfg(test)]
mod tests;
