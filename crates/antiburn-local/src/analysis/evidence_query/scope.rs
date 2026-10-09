use super::*;
use serde::{Deserialize, Serialize};

/// Trusted source and ancestry limits. Apply these before reading bodies or
/// charging coverage. Native IDs come from a complete source ancestor chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedContentScope {
    pub source_key: String,
    pub thread_id: String,
    pub turn_index: u64,
    pub part_index: u32,
    pub native_record_ids: Option<BTreeSet<String>>,
}

pub(super) const SCOPE_PREDICATE: &str = "(:content_scope IS NULL OR (
    turn.source_key = json_extract(:content_scope, '$.source_key')
    AND turn.thread_id = json_extract(:content_scope, '$.thread_id')
    AND turn.scope = 'main'
    AND (turn.turn_index, content.part_index) <=
        (json_extract(:content_scope, '$.turn_index'), json_extract(:content_scope, '$.part_index'))
    AND (json_type(:content_scope, '$.native_record_ids') = 'null' OR
        COALESCE(turn.uuid, turn.message_id) IN (
            SELECT value FROM json_each(:content_scope, '$.native_record_ids')))))";

/// Metadata-only proof from the same publication as the selected page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedContentBoundary {
    pub(crate) scope: SelectedContentScope,
    pub(crate) publication_fence: i64,
    pub(crate) source_generation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedSelectedContentPage {
    pub page: SelectedContentPage,
    pub boundary: Option<PublishedContentBoundary>,
}

pub(super) fn query_boundary(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    fence: &FenceScope<'_>,
    scope: &SelectedContentScope,
    generation: i64,
) -> rusqlite::Result<Option<PublishedContentBoundary>> {
    let (claim, published, sources) = scope_bind_values(fence);
    let scope_json = serde_json::to_string(scope)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let sql = format!(
        "SELECT EXISTS (
        SELECT 1 FROM turn JOIN turn_content AS content ON content.turn_rowid = turn.rowid
        WHERE turn.environment_key = :environment_key AND turn.agent = :agent
        AND turn.session_id = :session_id
        AND (turn.claim_fence = :claim_fence OR (turn.claim_fence = :published_fence
            AND turn.source_key IN (SELECT value FROM json_each(:source_keys))))
        AND turn.turn_index = json_extract(:content_scope, '$.turn_index')
        AND content.part_index = json_extract(:content_scope, '$.part_index')
        AND {SCOPE_PREDICATE})"
    );
    let exists: bool = conn.query_row(
        &sql,
        rusqlite::named_params! {
            ":environment_key": key.environment_key,
            ":agent": key.agent,
            ":session_id": key.session_id,
            ":claim_fence": claim,
            ":published_fence": published,
            ":source_keys": sources,
            ":content_scope": scope_json,
        },
        |row| row.get(0),
    )?;
    Ok(exists.then(|| PublishedContentBoundary {
        scope: scope.clone(),
        publication_fence: fence.claim_fence,
        source_generation: generation,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::evidence_query::tests::{KEY, base_row, test_connection};
    use crate::analysis::rows::insert_turn_rows;

    #[test]
    fn boundary_query_binds_named_scope_and_fence_values() {
        let conn = test_connection();
        let row = base_row("source", 0);
        insert_turn_rows(&conn, &KEY, 3, &[row]).unwrap();
        let content_scope = SelectedContentScope {
            source_key: "source".to_owned(),
            thread_id: "source".to_owned(),
            turn_index: 0,
            part_index: 0,
            native_record_ids: None,
        };
        conn.execute(
            "INSERT INTO turn_content (turn_rowid, part_index, kind, content, truncated, authority)
             SELECT rowid, 0, 'assistant', CAST('body' AS BLOB), 0, 'assistant' FROM turn",
            [],
        )
        .unwrap();

        let boundary = query_boundary(&conn, &KEY, &FenceScope::single(3), &content_scope, 7)
            .unwrap()
            .unwrap();
        assert_eq!(boundary.publication_fence, 3);
        assert_eq!(boundary.source_generation, 7);
        assert_eq!(boundary.scope, content_scope);
    }
}
