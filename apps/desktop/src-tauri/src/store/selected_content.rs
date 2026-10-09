use antiburn_local::analysis::{
    ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, FenceScope, PARSER_REVISION,
    ScopedSelectedContentPage, SelectedContentCursor, SelectedContentPage,
    SelectedContentQueryError, SelectedContentRequest, SelectedContentScope,
    query_turn_content_keyset_scoped, query_turn_content_keyset_selected,
};
use rusqlite::{OptionalExtension, params};

use super::{EVIDENCE_BY_KEY_SQL, SessionKey, Store, evidence_from_row, turn_session_key};

pub(crate) const SELECTED_CONTENT_PROGRESS_REVISION: u32 = 1;

const NEARBY_CONTENT_IDENTITIES_SQL: &str = "SELECT turn.turn_index, content.part_index,
    COALESCE(turn.uuid, turn.message_id), turn.thread_id
    FROM turn JOIN turn_content AS content ON content.turn_rowid = turn.rowid
    WHERE turn.environment_key = ?1 AND turn.agent = ?2 AND turn.session_id = ?3
    AND turn.claim_fence = ?4 AND turn.source_key = ?5 AND turn.turn_index <= ?6
    AND (turn.turn_index, content.part_index) <= (?6, ?7)
    ORDER BY turn.turn_index DESC, turn.rowid DESC, content.part_index DESC LIMIT 256";

const FOLLOWING_CONTENT_IDENTITIES_SQL: &str = "SELECT turn.turn_index, content.part_index,
    COALESCE(turn.uuid, turn.message_id), turn.thread_id
    FROM turn JOIN turn_content AS content ON content.turn_rowid = turn.rowid
    WHERE turn.environment_key = ?1 AND turn.agent = ?2 AND turn.session_id = ?3
    AND turn.claim_fence = ?4 AND turn.source_key = ?5 AND turn.turn_index >= ?6
    AND (turn.turn_index, content.part_index) > (?6, ?7)
    ORDER BY turn.turn_index, turn.rowid, content.part_index LIMIT 256";

/// Shared local continuation state for selected-evidence checks.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct SelectedContentProgress {
    pub revision: u32,
    pub cursor: Option<SelectedContentCursor>,
}

impl Store {
    /// Resolve cited source keys by indexed turn position under the publication guard.
    pub(crate) fn published_turn_content_identities_selected(
        &self,
        key: &SessionKey,
        source_generation: i64,
        references: &[&antiburn_local::analysis::jev_evidence::ContentEventReference],
        context_ids: &std::collections::BTreeSet<&str>,
        selection: antiburn_local::analysis::JevInputSelection,
    ) -> Result<Option<antiburn_local::analysis::PublishedContent>, SelectedContentQueryError> {
        use antiburn_local::analysis::ignored_instructions::sha256_hex;
        self.with_published_content(key, source_generation, |connection, fence| {
            let mut content = antiburn_local::analysis::PublishedContent {
                publication_fence: fence,
                source_generation: Some(source_generation),
                ..Default::default()
            };
            if references.len() > antiburn_local::analysis::MAX_CONTENT_QUERY_PARTS {
                content.coverage.parts_capped = true;
                return Ok(content);
            }
            let mut seen = std::collections::BTreeSet::new();
            let mut anchor_scope = None;
            for reference in references {
                if !seen.insert(reference.id.clone()) {
                    continue;
                }
                let mut statement = connection.prepare(
                    "SELECT source_key, thread_id FROM turn
                     WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                     AND claim_fence = ?4 AND turn_index = ?5
                     AND COALESCE(uuid, message_id) = ?6 AND scope = 'main'
                     LIMIT 257",
                )?;
                let candidates = statement
                    .query_map(
                        params![
                            key.environment_key,
                            key.agent,
                            key.session_id,
                            fence,
                            reference.turn_index,
                            reference.native_record_id
                        ],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                if candidates.len() > 256 {
                    continue;
                }
                for (source_key, thread_id) in candidates
                    .into_iter()
                    .collect::<std::collections::BTreeSet<_>>()
                {
                    if sha256_hex(source_key.as_bytes()) != reference.source_key_digest
                        || sha256_hex(thread_id.as_bytes()) != reference.thread_digest
                    {
                        continue;
                    }
                    let scope = SelectedContentScope {
                        source_key,
                        thread_id,
                        turn_index: reference.turn_index,
                        part_index: reference.part_index,
                        native_record_ids: Some(
                            reference.native_record_id.iter().cloned().collect(),
                        ),
                    };
                    if reference.id == references[0].id {
                        anchor_scope = Some(scope.clone());
                    }
                    let selected = antiburn_local::analysis::query_turn_content_exact_selected(
                        connection,
                        &turn_session_key(key),
                        &FenceScope::single(fence),
                        source_generation,
                        selection,
                        &scope,
                    )?;
                    content
                        .parts
                        .extend(selected.parts.into_iter().filter(|part| {
                            part.turn_index == reference.turn_index
                                && part.part_index == reference.part_index
                                && part.uuid.as_ref().or(part.message_id.as_ref())
                                    == reference.native_record_id.as_ref()
                        }));
                }
            }
            if let Some(anchor_scope) = anchor_scope
                && context_ids.iter().any(|id| !seen.contains(*id))
            {
                let mut candidates = Vec::new();
                for sql in [
                    NEARBY_CONTENT_IDENTITIES_SQL,
                    FOLLOWING_CONTENT_IDENTITIES_SQL,
                ] {
                    let mut statement = connection.prepare(sql)?;
                    candidates.extend(
                        statement
                            .query_map(
                                params![
                                    key.environment_key,
                                    key.agent,
                                    key.session_id,
                                    fence,
                                    anchor_scope.source_key,
                                    anchor_scope.turn_index,
                                    anchor_scope.part_index
                                ],
                                |row| {
                                    Ok((
                                        row.get::<_, u64>(0)?,
                                        row.get::<_, u32>(1)?,
                                        row.get::<_, Option<String>>(2)?,
                                        row.get::<_, String>(3)?,
                                    ))
                                },
                            )?
                            .collect::<rusqlite::Result<Vec<_>>>()?,
                    );
                }
                let source_digest = sha256_hex(anchor_scope.source_key.as_bytes());
                for (turn_index, part_index, native_record_id, thread_id) in candidates {
                    let id = sha256_hex(
                        format!(
                            "{source_digest}\0{}\0{turn_index}\0{part_index}",
                            native_record_id.as_deref().unwrap_or("ordinal")
                        )
                        .as_bytes(),
                    );
                    if !context_ids.contains(id.as_str())
                        || seen.contains(&id)
                        || thread_id != anchor_scope.thread_id
                    {
                        continue;
                    }
                    let scope = SelectedContentScope {
                        turn_index,
                        part_index,
                        native_record_ids: Some(native_record_id.into_iter().collect()),
                        ..anchor_scope.clone()
                    };
                    content.parts.extend(
                        antiburn_local::analysis::query_turn_content_exact_selected(
                            connection,
                            &turn_session_key(key),
                            &FenceScope::single(fence),
                            source_generation,
                            selection,
                            &scope,
                        )?
                        .parts,
                    );
                }
            }
            Ok(content)
        })
    }

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
