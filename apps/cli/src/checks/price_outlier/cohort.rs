use rusqlite::{Connection, OptionalExtension, params};

use super::SessionRow;
use crate::checks::model::SessionIdentity;

pub(super) fn target(
    connection: &Connection,
    identity: &SessionIdentity,
) -> rusqlite::Result<Option<SessionRow>> {
    connection
        .query_row(
            "SELECT s.environment_key, s.agent, s.session_id, s.updated_at_epoch,
                    a.model_breakdown_json, a.pricing_breakdown_json
               FROM session s
               LEFT JOIN session_analysis a
                 ON a.environment_key = s.environment_key
                AND a.agent = s.agent
                AND a.session_id = s.session_id
              WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3",
            params![
                identity.environment_key,
                identity.agent,
                identity.session_id
            ],
            row_from_sql,
        )
        .optional()
}

pub(super) fn neighbors(
    connection: &Connection,
    target: &SessionIdentity,
    target_epoch: i64,
    max_age_seconds: i64,
    limit: i64,
) -> rusqlite::Result<Vec<SessionRow>> {
    let since = target_epoch.saturating_sub(max_age_seconds);
    let until = target_epoch.saturating_add(max_age_seconds);
    let mut statement = connection.prepare(
        "SELECT s.environment_key, s.agent, s.session_id, s.updated_at_epoch,
                a.model_breakdown_json, a.pricing_breakdown_json
           FROM session s
           LEFT JOIN session_analysis a
             ON a.environment_key = s.environment_key
            AND a.agent = s.agent
            AND a.session_id = s.session_id
          WHERE s.updated_at_epoch BETWEEN ?1 AND ?2
            AND NOT (s.environment_key = ?3 AND s.agent = ?4 AND s.session_id = ?5)
          ORDER BY ABS(s.updated_at_epoch - ?6),
                   s.updated_at_epoch DESC, s.session_id DESC,
                   s.environment_key DESC, s.agent DESC
          LIMIT ?7",
    )?;
    let rows = statement.query_map(
        params![
            since,
            until,
            target.environment_key,
            target.agent,
            target.session_id,
            target_epoch,
            limit,
        ],
        row_from_sql,
    )?;
    rows.collect()
}

fn row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        identity: SessionIdentity {
            environment_key: row.get(0)?,
            agent: row.get(1)?,
            session_id: row.get(2)?,
        },
        updated_at_epoch: row.get(3)?,
        model_breakdown_json: row.get(4)?,
        pricing_breakdown_json: row.get(5)?,
    })
}
