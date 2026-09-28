//! Read-only access to exact passages in published retained session content.

use anyhow::Result;
use rusqlite::{OptionalExtension as _, params};

use super::Store;
use super::session_search::{SessionSearchResult, model_names, path_label, repository_label};

const REFERENCE_PREFIX_BYTES: usize = 64;
const ALL_RETAINED_CONTENT: &str =
    "content.kind IN ('user','assistant','thinking','tool_input','tool_result')";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RetainedContentRow {
    pub turn_rowid: i64,
    pub source_key: String,
    pub thread_id: String,
    pub turn_index: i64,
    pub scope: String,
    pub part_index: i64,
    pub kind: String,
    pub content: String,
    pub reference_bytes: Vec<u8>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoredEvidenceReference {
    pub environment_key: String,
    pub agent: String,
    pub session_id: String,
    pub source_generation: i64,
    pub published_fence: i64,
    pub source_key: String,
    pub thread_id: String,
    pub scope: String,
    pub turn_rowid: i64,
    pub turn_index: i64,
    pub part_index: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RetainedContext {
    pub session: SessionSearchResult,
    pub previous: Option<RetainedContentRow>,
    pub matched: RetainedContentRow,
    pub next: Option<RetainedContentRow>,
}

impl Store {
    pub(crate) fn fetch_published_session_context(
        &self,
        reference: &StoredEvidenceReference,
    ) -> Result<Option<RetainedContext>> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let session = session_result_in(
            &transaction,
            &reference.environment_key,
            &reference.agent,
            &reference.session_id,
        )?;
        let Some(session) = session else {
            transaction.commit()?;
            return Ok(None);
        };
        let sql = format!(
            "SELECT turn.rowid, turn.source_key, turn.thread_id, turn.turn_index,
                    turn.scope, content.part_index, content.kind, content.content,
                    content.truncated
               FROM turn
               JOIN turn_content AS content ON content.turn_rowid = turn.rowid
               JOIN session ON session.environment_key = turn.environment_key
                    AND session.agent = turn.agent AND session.session_id = turn.session_id
               JOIN session_evidence AS evidence
                 ON evidence.environment_key = turn.environment_key
                AND evidence.agent = turn.agent AND evidence.session_id = turn.session_id
              WHERE turn.environment_key = ?1 AND turn.agent = ?2 AND turn.session_id = ?3
                AND session.source_generation = ?4
                AND evidence.analyzed_generation = session.source_generation
                AND evidence.published_fence = ?5 AND turn.claim_fence = ?5
                AND turn.source_key = ?6 AND turn.thread_id = ?7 AND turn.scope = ?8
                AND turn.rowid = ?9 AND turn.turn_index = ?10 AND content.part_index = ?11
                AND {ALL_RETAINED_CONTENT}"
        );
        let matched = transaction
            .query_row(
                &sql,
                params![
                    reference.environment_key,
                    reference.agent,
                    reference.session_id,
                    reference.source_generation,
                    reference.published_fence,
                    reference.source_key,
                    reference.thread_id,
                    reference.scope,
                    reference.turn_rowid,
                    reference.turn_index,
                    reference.part_index,
                ],
                content_row,
            )
            .optional()?;
        let Some(matched) = matched else {
            transaction.commit()?;
            return Ok(None);
        };
        let previous = adjacent_content(&transaction, reference, false)?;
        let next = adjacent_content(&transaction, reference, true)?;
        transaction.commit()?;
        Ok(Some(RetainedContext {
            session,
            previous,
            matched,
            next,
        }))
    }
}

fn session_result_in(
    connection: &rusqlite::Connection,
    environment_key: &str,
    agent: &str,
    session_id: &str,
) -> Result<Option<SessionSearchResult>> {
    Ok(connection
        .query_row(
            "SELECT session.environment_key, session.agent, session.session_id,
                    session.wsl_distro, NULLIF(session.title, ''), '',
                    COALESCE(session.cwd, ''), analysis.inclusive_models_json,
                    session.updated_at_epoch
               FROM session
               LEFT JOIN session_analysis AS analysis
                 ON analysis.environment_key = session.environment_key
                AND analysis.agent = session.agent AND analysis.session_id = session.session_id
              WHERE session.environment_key = ?1 AND session.agent = ?2
                AND session.session_id = ?3",
            params![environment_key, agent, session_id],
            |row| {
                let repository = row.get::<_, String>(5)?;
                let cwd = row.get::<_, String>(6)?;
                let models = row
                    .get::<_, Option<String>>(7)?
                    .unwrap_or_else(|| "[]".to_owned());
                let updated_at_epoch = row.get::<_, i64>(8)?;
                Ok(SessionSearchResult {
                    environment_key: row.get(0)?,
                    agent: row.get(1)?,
                    session_id: row.get(2)?,
                    wsl_distro: row.get(3)?,
                    title: row.get(4)?,
                    repository: repository_label(&repository, &cwd),
                    cwd_label: path_label(&cwd),
                    models: model_names(&models),
                    updated_at_epoch: (updated_at_epoch != 0).then_some(updated_at_epoch),
                })
            },
        )
        .optional()?)
}

fn content_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RetainedContentRow> {
    let content = row.get::<_, Vec<u8>>(7)?;
    Ok(RetainedContentRow {
        turn_rowid: row.get(0)?,
        source_key: row.get(1)?,
        thread_id: row.get(2)?,
        turn_index: row.get(3)?,
        scope: row.get(4)?,
        part_index: row.get(5)?,
        kind: row.get(6)?,
        content: String::from_utf8_lossy(&content).into_owned(),
        reference_bytes: content
            .iter()
            .take(REFERENCE_PREFIX_BYTES)
            .copied()
            .collect(),
        truncated: row.get(8)?,
    })
}

fn adjacent_content(
    connection: &rusqlite::Connection,
    reference: &StoredEvidenceReference,
    next: bool,
) -> Result<Option<RetainedContentRow>> {
    let comparison = if next { ">" } else { "<" };
    let order = if next { "ASC" } else { "DESC" };
    let sql = format!(
        "SELECT turn.rowid, turn.source_key, turn.thread_id, turn.turn_index,
                turn.scope, content.part_index, content.kind, content.content,
                content.truncated
           FROM turn
           JOIN turn_content AS content ON content.turn_rowid = turn.rowid
          WHERE turn.environment_key = ?1 AND turn.agent = ?2 AND turn.session_id = ?3
            AND turn.claim_fence = ?4 AND turn.source_key = ?5
            AND turn.thread_id = ?6 AND turn.scope = ?7
            AND (turn.turn_index, turn.rowid, content.part_index) {comparison} (?8, ?9, ?10)
            AND {ALL_RETAINED_CONTENT}
          ORDER BY turn.turn_index {order}, turn.rowid {order}, content.part_index {order}
          LIMIT 1"
    );
    Ok(connection
        .query_row(
            &sql,
            params![
                reference.environment_key,
                reference.agent,
                reference.session_id,
                reference.published_fence,
                reference.source_key,
                reference.thread_id,
                reference.scope,
                reference.turn_index,
                reference.turn_rowid,
                reference.part_index,
            ],
            content_row,
        )
        .optional()?)
}
