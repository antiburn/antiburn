use super::*;

#[cfg(test)]
mod admission_tests {
    use super::*;

    fn record() -> SessionRecord {
        SessionRecord {
            key: SessionKey::new("native", "opencode", "admission-fixture"),
            source_kind: "providerDb".into(),
            source_label: "opencode:admission-fixture".into(),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: Some("/synthetic".into()),
            surface: "cli".into(),
            updated_at_epoch: Some(1000),
            activity_cursor: "opencode-admission-v1:database-one:revision-one".into(),
            activity_source: "unknown".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: None,
        }
    }

    #[test]
    fn pending_admission_survives_restart_and_rejects_old_completion_after_new_generation() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let mut record = record();
        store
            .upsert_sessions(std::slice::from_ref(&record), &["opencode"])
            .unwrap();
        let old = store
            .pending_source_fingerprint(&record.key)
            .unwrap()
            .unwrap();
        assert!(
            store
                .claim_next_evidence(&["opencode"], 1000, 60)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .complete_source_fingerprint(&old, "strict-one", 1000)
                .unwrap()
        );
        let claim = store
            .claim_next_evidence(&["opencode"], 1000, 60)
            .unwrap()
            .unwrap();
        record.activity_cursor = "opencode-admission-v1:database-one:revision-two".into();
        store
            .upsert_sessions(std::slice::from_ref(&record), &["opencode"])
            .unwrap();
        let stale = store
            .pending_source_fingerprint(&record.key)
            .unwrap()
            .unwrap();
        assert!(stale.generation > claim.source_generation);
        assert!(
            store
                .session(&record.key)
                .unwrap()
                .unwrap()
                .source_fingerprint
                .is_none()
        );
        drop(store);
        let store = Store::open(directory.path()).unwrap();
        let current = store
            .pending_source_fingerprint(&record.key)
            .unwrap()
            .unwrap();
        assert_eq!(current.generation, stale.generation);
        assert!(
            store
                .claim_next_evidence(&["opencode"], 2000, 60)
                .unwrap()
                .is_none()
        );
        record.activity_cursor = "opencode-admission-v1:database-two:revision-two".into();
        store
            .upsert_sessions(std::slice::from_ref(&record), &["opencode"])
            .unwrap();
        assert!(
            !store
                .complete_source_fingerprint(&current, "stale-strict", 2000)
                .unwrap()
        );
        let current = store
            .pending_source_fingerprint(&record.key)
            .unwrap()
            .unwrap();
        assert!(
            store
                .complete_source_fingerprint(&current, "strict-two", 2000)
                .unwrap()
        );
        assert!(
            !store
                .complete_source_fingerprint(&current, "duplicate-strict", 2000)
                .unwrap()
        );
        let next = store
            .claim_next_evidence(&["opencode"], 2000, 60)
            .unwrap()
            .unwrap();
        assert!(next.source_generation > claim.source_generation);
        assert_eq!(
            store
                .session(&record.key)
                .unwrap()
                .unwrap()
                .source_fingerprint
                .as_deref(),
            Some("strict-two")
        );
    }

    #[test]
    fn unchanged_admission_does_not_cancel_a_hash_but_policy_and_recreation_do() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        let record = record();
        store
            .upsert_sessions(std::slice::from_ref(&record), &["opencode"])
            .unwrap();
        let pending = store
            .pending_source_fingerprint(&record.key)
            .unwrap()
            .unwrap();
        store
            .upsert_sessions(std::slice::from_ref(&record), &["opencode"])
            .unwrap();
        assert!(
            store
                .complete_source_fingerprint(&pending, "strict-one", 1000)
                .unwrap()
        );
        store
            .lock()
            .execute(
                "UPDATE session SET strict_fingerprint_pending = 1, source_fingerprint = NULL",
                [],
            )
            .unwrap();
        let pending = store
            .pending_source_fingerprint(&record.key)
            .unwrap()
            .unwrap();
        store
            .update_settings(|settings| settings.include_non_repo_folders = true)
            .unwrap();
        assert!(
            !store
                .complete_source_fingerprint(&pending, "stale-policy", 1000)
                .unwrap()
        );
        let pending = store
            .pending_source_fingerprint(&record.key)
            .unwrap()
            .unwrap();
        let guard = store
            .begin_session_write(&record.key, &record.source_label)
            .unwrap();
        store.delete_session_if_current(&guard).unwrap();
        store
            .upsert_sessions(std::slice::from_ref(&record), &["opencode"])
            .unwrap();
        assert!(
            !store
                .complete_source_fingerprint(&pending, "stale-incarnation", 1000)
                .unwrap()
        );
    }
}

/// One session's analysis state, as [`Store::hygiene_summary_rows`] reads it.
#[derive(Debug, Clone)]
pub struct HygieneSummaryRow {
    /// True when analysis reached a terminal state for the current source.
    pub settled: bool,
    /// Current ready evidence JSON, when the session has one.
    pub evidence_json: Option<String>,
}

/// The current UTC time as a fixed-width RFC 3339 stamp, so plain string
/// comparison of two values stays chronological.
pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// An epoch stamp as the RFC 3339 string the views parse.
///
/// Lives beside [`now_rfc3339`] because the two answer the same question in the
/// same spelling; every payload that carries a time uses one of them, so the
/// webview never has to guess a format.
pub fn iso_from_epoch(epoch: Option<i64>) -> String {
    let epoch = epoch.unwrap_or(0);
    time::OffsetDateTime::from_unix_timestamp(epoch)
        .ok()
        .and_then(|at| {
            at.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

/// Insert a `forkParent` relation naming `parent` as `key`'s fork parent.
///
/// The table's primary key covers `related_id`, so this only guards a
/// repeat call naming the same `parent` — it does not stop a second call
/// from giving `key` a second, different parent. A caller that must keep
/// one parent per session checks for an existing one first.
///
/// Returns whether this call inserted the row, using `changes()` rather
/// than the `execute` row count so a caller can log only a real insert.
pub(super) fn insert_fork_parent_in(
    connection: &Connection,
    key: &SessionKey,
    parent: &str,
) -> Result<bool> {
    connection.execute(
        "INSERT INTO session_relation
             (environment_key, agent, session_id, kind, related_id, label)
         VALUES (?1, ?2, ?3, 'forkParent', ?4, NULL)
         ON CONFLICT DO NOTHING",
        params![key.environment_key, key.agent, key.session_id, parent],
    )?;
    Ok(connection.changes() > 0)
}

/// Read the writer revision while the caller holds the Store mutex.
pub(super) fn revision_of(connection: &Connection) -> Revision {
    Revision(connection.total_changes())
}

/// Allocate an incarnation inside the write transaction. Rollback also restores the
/// counter.
pub(super) fn allocate_incarnation_in(connection: &Connection) -> Result<Incarnation> {
    let value = connection.query_row(
        "UPDATE session_incarnation_seq SET value = value + 1 WHERE id = 1 RETURNING value",
        [],
        |row| row.get::<_, u64>(0),
    )?;
    Ok(Incarnation(value))
}

/// What one [`upsert_session_in`] call found and wrote.
struct UpsertedSession {
    previous_generation: Option<i64>,
    source_generation: i64,
    activity_cursor_changed: bool,
    /// The row's incarnation after the write: the existing one on an update,
    /// a newly allocated one on an insert.
    incarnation: Incarnation,
}

pub(super) fn upsert_sessions_locked(
    connection: &mut Connection,
    records: &[SessionRecord],
    evidence_agents: &[&str],
) -> Result<SessionUpsertResult> {
    let tx = connection.transaction()?;
    let mut incarnations = Vec::with_capacity(records.len());
    for record in records {
        let source_returned = tx.query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM session_evidence
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                    AND status = 'failed' AND last_error = 'source-missing'
             )",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
            |row| row.get::<_, i64>(0),
        )? != 0;
        let upserted = upsert_session_in(&tx, record)?;
        let generation_increased = upserted
            .previous_generation
            .is_none_or(|previous| upserted.source_generation > previous);
        let evidence_marked_pending = evidence_agents.contains(&record.key.agent.as_str())
            && (generation_increased || upserted.activity_cursor_changed || source_returned);
        if evidence_marked_pending {
            mark_evidence_pending_in(&tx, &record.key)?;
        }
        incarnations.push((record.key.clone(), upserted.incarnation));
    }
    tx.commit()?;
    Ok((incarnations, revision_of(connection)))
}

fn upsert_session_in(connection: &Connection, record: &SessionRecord) -> Result<UpsertedSession> {
    selected_source::persist_selection_in(connection, record)?;
    let admission_revision = (record.key.environment_key == "native"
        && record.key.agent == "opencode"
        && record.source_kind == "providerDb"
        && (record.activity_cursor.starts_with("opencode-admission-v1:")
            || record.activity_cursor.starts_with("opencode-admission-v2:")))
    .then_some(record.activity_cursor.as_str());
    let source_path = admission_revision.and_then(|revision| {
        let value: serde_json::Value =
            serde_json::from_str(revision.strip_prefix("opencode-admission-v2:")?).ok()?;
        value.get(0)?.as_str().map(str::to_owned)
    });
    let previous_state = connection
        .query_row(
            "SELECT source_generation, activity_cursor, incarnation FROM session
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    Incarnation(row.get::<_, u64>(2)?),
                ))
            },
        )
        .optional()?;
    let previous_generation = previous_state
        .as_ref()
        .map(|(generation, _, _)| *generation);
    let activity_cursor_changed = previous_state
        .as_ref()
        .is_some_and(|(_, cursor, _)| cursor != &record.activity_cursor);
    // Conflict updates preserve the incarnation. Only new rows advance the counter.
    let incarnation = match previous_state {
        Some((_, _, incarnation)) => incarnation,
        None => allocate_incarnation_in(connection)?,
    };
    let now = now_rfc3339();
    connection.execute(
        "INSERT INTO session (
             environment_key, agent, session_id, source_kind, source_label, wsl_distro,
             title, title_source, cwd, surface, updated_at_epoch,
             activity_cursor, activity_source, subagent_count,
             first_seen_at, last_seen_at, source_fingerprint, source_generation,
              incarnation, admission_revision, strict_fingerprint_pending, fingerprint_source_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
                 CASE WHEN ?7 IS NOT NULL THEN ?8 ELSE NULL END,
                 ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15, ?16,
                 CASE WHEN ?16 IS NOT NULL THEN 1 ELSE 0 END,
                  ?17, ?18, CASE WHEN ?18 IS NOT NULL THEN 1 ELSE 0 END, ?19)
         ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
             source_kind = excluded.source_kind,
             source_label = excluded.source_label,
             wsl_distro = excluded.wsl_distro,
             title = COALESCE(excluded.title, session.title),
             title_source = CASE
                 WHEN excluded.title IS NOT NULL THEN excluded.title_source
                 WHEN session.title IS NOT NULL THEN session.title_source
                 ELSE NULL
             END,
             cwd = COALESCE(excluded.cwd, session.cwd),
             surface = excluded.surface,
             updated_at_epoch = CASE
                 WHEN session.activity_source = 'event'
                      AND excluded.activity_source <> 'event'
                     THEN session.updated_at_epoch
                 WHEN excluded.activity_source = 'event'
                      AND session.activity_source = 'event'
                     THEN MAX(COALESCE(excluded.updated_at_epoch, 0),
                              COALESCE(session.updated_at_epoch, 0))
                 WHEN excluded.activity_source = 'event'
                     THEN excluded.updated_at_epoch
                 ELSE MAX(COALESCE(excluded.updated_at_epoch, 0),
                          COALESCE(session.updated_at_epoch, 0))
             END,
             activity_cursor = excluded.activity_cursor,
             activity_source = CASE
                 WHEN excluded.activity_source = 'event'
                     THEN 'event'
                 WHEN session.activity_source = 'event'
                     THEN 'event'
                 ELSE excluded.activity_source
             END,
             subagent_count = excluded.subagent_count,
               source_fingerprint = CASE
                   WHEN excluded.agent = 'opencode'
                        AND session.source_label IS NOT excluded.source_label THEN NULL
                   WHEN excluded.admission_revision IS NOT NULL AND
                        (session.admission_revision IS NOT excluded.admission_revision
                         OR session.source_label IS NOT excluded.source_label)
                      THEN NULL
                  ELSE COALESCE(excluded.source_fingerprint, session.source_fingerprint) END,
               source_generation = CASE
                   WHEN excluded.agent = 'opencode'
                        AND session.source_label IS NOT excluded.source_label
                       THEN session.source_generation + 1
                   WHEN excluded.admission_revision IS NOT NULL AND
                       (session.admission_revision IS NOT excluded.admission_revision
                        OR session.source_label IS NOT excluded.source_label)
                      THEN session.source_generation + 1
                  WHEN excluded.source_fingerprint IS NULL THEN session.source_generation
                 WHEN session.source_fingerprint = excluded.source_fingerprint
                     THEN session.source_generation
                 ELSE session.source_generation + 1
              END,
                strict_fingerprint_pending = CASE
                    WHEN excluded.agent = 'opencode'
                         AND session.source_label IS NOT excluded.source_label THEN 1
                    WHEN excluded.admission_revision IS NOT NULL AND
                        (session.admission_revision IS NOT excluded.admission_revision
                         OR session.source_label IS NOT excluded.source_label
                         OR session.fingerprint_source_path IS NOT excluded.fingerprint_source_path)
                       THEN 1
                  WHEN excluded.source_fingerprint IS NOT NULL THEN 0
                  ELSE session.strict_fingerprint_pending END,
              admission_revision = excluded.admission_revision,
              fingerprint_source_path = excluded.fingerprint_source_path,
                fingerprint_next_attempt_at_epoch = CASE
                     WHEN excluded.agent = 'opencode'
                          AND session.source_label IS NOT excluded.source_label THEN NULL
                     WHEN excluded.admission_revision IS NOT NULL AND
                         (session.admission_revision IS NOT excluded.admission_revision
                          OR session.fingerprint_source_path IS NOT excluded.fingerprint_source_path
                          OR session.source_label IS NOT excluded.source_label) THEN NULL
                   ELSE session.fingerprint_next_attempt_at_epoch END,
                fingerprint_retry_count = CASE
                    WHEN excluded.agent = 'opencode'
                         AND session.source_label IS NOT excluded.source_label THEN 0
                    WHEN excluded.admission_revision IS NOT NULL AND
                         (session.admission_revision IS NOT excluded.admission_revision
                          OR session.fingerprint_source_path IS NOT excluded.fingerprint_source_path
                          OR session.source_label IS NOT excluded.source_label) THEN 0
                   ELSE session.fingerprint_retry_count END,
               fingerprint_in_flight = CASE
                   WHEN session.admission_revision IS NOT excluded.admission_revision
                        OR (excluded.agent = 'opencode'
                            AND session.source_label IS NOT excluded.source_label) THEN 0
                  ELSE session.fingerprint_in_flight END,
              last_seen_at = excluded.last_seen_at",
        params![
            record.key.environment_key,
            record.key.agent,
            record.key.session_id,
            record.source_kind,
            record.source_label,
            record.wsl_distro,
            record.title,
            record.title_source,
            record.cwd,
            record.surface,
            record.updated_at_epoch,
            record.activity_cursor,
            record.activity_source,
            record.subagent_count,
            now,
            record.source_fingerprint,
            incarnation.0,
            admission_revision,
            source_path,
        ],
    )?;

    if record.key.agent == "opencode"
        && (record.source_kind != "providerDb"
            || record.source_label != format!("opencode:{}", record.key.session_id))
    {
        connection.execute(
            "DELETE FROM opencode_journal_delivery
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )?;
    }

    // A fork parent is written only when the caller actually observed one.
    //
    // Absence is not evidence. Some adapters resolve lineage only when a
    // session opens. A later scan must not erase that relation.
    if let Some(parent) = &record.fork_parent_session_id {
        insert_fork_parent_in(connection, &record.key, parent)?;
    }
    // Read the row back inside the same transaction: the values are exact
    // and the incarnation is the one the row holds, whichever branch ran.
    let (source_generation, incarnation) = connection.query_row(
        "SELECT source_generation, incarnation FROM session
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![
            record.key.environment_key,
            record.key.agent,
            record.key.session_id
        ],
        |row| Ok((row.get::<_, i64>(0)?, Incarnation(row.get::<_, u64>(1)?))),
    )?;
    Ok(UpsertedSession {
        previous_generation,
        source_generation,
        activity_cursor_changed,
        incarnation,
    })
}

pub(super) fn replace_relations_in(
    connection: &Connection,
    key: &SessionKey,
    kind: RelationKind,
    relations: &[RelationRecord],
) -> Result<()> {
    connection.execute(
        "DELETE FROM session_relation
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND kind = ?4",
        params![
            key.environment_key,
            key.agent,
            key.session_id,
            kind.as_str()
        ],
    )?;
    for relation in relations {
        connection.execute(
            "INSERT INTO session_relation
                 (environment_key, agent, session_id, kind, related_id, label)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT DO UPDATE SET label = excluded.label",
            params![
                key.environment_key,
                key.agent,
                key.session_id,
                relation.kind.as_str(),
                relation.related_id,
                relation.label,
            ],
        )?;
    }
    Ok(())
}

pub(super) fn mark_evidence_pending_in(connection: &Connection, key: &SessionKey) -> Result<()> {
    connection.execute(
        "INSERT INTO session_evidence (environment_key, agent, session_id)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
             status = 'pending', last_error = NULL,
             next_attempt_at_epoch = NULL, retry_count = 0",
        params![key.environment_key, key.agent, key.session_id],
    )?;
    Ok(())
}

/// Delete one session and its owned rows. Returns the deleted row's
/// incarnation, or `None` when there was no row.
pub(super) fn delete_session_in(
    connection: &Connection,
    key: &SessionKey,
) -> Result<Option<Incarnation>> {
    let parameters = params![key.environment_key, key.agent, key.session_id];
    connection.execute(
        "DELETE FROM session_relation
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        parameters,
    )?;
    connection.execute(
        "DELETE FROM session_analysis
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        parameters,
    )?;
    // The v11 `ON DELETE CASCADE` also covers this row, but the cascade rides
    // on the per-connection `foreign_keys` pragma; deleting explicitly keeps
    // session deletion pragma-independent.
    connection.execute(
        "DELETE FROM session_evidence
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        parameters,
    )?;
    // Same reasoning as `session_evidence` above: the v15 and v28 cascades
    // from `session` cover `turn`, `turn_content`, and `session_coverage`,
    // but this stays explicit and pragma-independent. `turn_content` has no
    // direct session key, so it is deleted through `turn`'s rowid.
    delete_turn_rows_in(connection, key)?;
    // Same reasoning again: the v30 cascade from `session` covers
    // `source_resume`, but this stays explicit and pragma-independent.
    connection.execute(
        "DELETE FROM source_resume
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        parameters,
    )?;
    let removed = connection
        .query_row(
            "DELETE FROM session
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
          RETURNING incarnation",
            parameters,
            |row| row.get::<_, u64>(0),
        )
        .optional()?;
    if removed.is_some() {
        burn_check::forget_session_usage_in(connection, key)?;
        native_tool_results::delete_session_data_in(connection, key)?;
        Store::delete_burn_check_session_in(connection, key)?;
    }
    Ok(removed.map(Incarnation))
}

pub(crate) fn apply_session_retention_in(
    connection: &Connection,
    retention_days: i32,
    now_epoch: i64,
) -> Result<usize> {
    if retention_days == RETAIN_SESSION_DATA_FOREVER {
        Store::apply_provider_usage_retention_in(connection, retention_days, now_epoch)?;
        return Ok(0);
    }

    let cutoff = now_epoch.saturating_sub(i64::from(retention_days).saturating_mul(86_400));
    let mut statement = connection.prepare(
        "SELECT environment_key, agent, session_id
           FROM session
          WHERE COALESCE(
                    updated_at_epoch,
                    CAST(strftime('%s', last_seen_at) AS INTEGER)
                ) < ?1",
    )?;
    let keys = statement
        .query_map([cutoff], |row| {
            Ok(SessionKey::new(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    let mut removed = 0;
    for key in keys {
        if delete_session_in(connection, &key)?.is_some() {
            removed += 1;
        }
    }
    Store::prune_burn_check_candidate_payloads_in(connection, cutoff)?;
    Store::apply_provider_usage_retention_in(connection, retention_days, now_epoch)?;
    Ok(removed)
}

pub(super) fn turn_session_key(key: &SessionKey) -> TurnSessionKey<'_> {
    TurnSessionKey {
        environment_key: &key.environment_key,
        agent: &key.agent,
        session_id: &key.session_id,
    }
}

/// This session's current `published_fence`, or `None` when it has never
/// published. Shared by `Store::publish_projections` (deciding the fence
/// this pass's winning rows join) and `FencedTurnRowStore` (widening a
/// mid-pass read for a source that resumed — see [`FenceScope`]).
pub(super) fn read_published_fence(
    connection: &Connection,
    key: &SessionKey,
) -> rusqlite::Result<Option<i64>> {
    connection
        .query_row(
            "SELECT published_fence FROM session_evidence
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()
        .map(Option::flatten)
}

pub(super) fn delete_turn_rows_in(connection: &Connection, key: &SessionKey) -> Result<usize> {
    Ok(delete_turn_rows(connection, &turn_session_key(key))?)
}

pub(super) fn evidence_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EvidenceRow> {
    let status: EvidenceStatus = row
        .get::<_, String>(3)?
        .parse()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(EvidenceRow {
        key: SessionKey::new(
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ),
        status,
        analyzed_generation: row.get(4)?,
        processed_fingerprint: row.get(5)?,
        parser_revision: row.get(6)?,
        analyzer_revision: row.get(7)?,
        evidence_schema_revision: row.get(8)?,
        evidence_json: row.get(9)?,
        retry_count: row.get(10)?,
        claim_fence: row.get(11)?,
        claimed_at_epoch: row.get(12)?,
        lease_expires_at_epoch: row.get(13)?,
        next_attempt_at_epoch: row.get(14)?,
        analyzed_at_epoch: row.get(15)?,
        last_error: row.get(16)?,
        published_fence: row.get(17)?,
    })
}

/// Every full-record session reader uses this column list and join. One copy
/// keeps schema changes aligned with [`session_from_row`].
pub(super) const SESSION_SELECT_SQL: &str =
    "SELECT environment_key, agent, session_id, source_kind, source_label, wsl_distro,
                    title, title_source, cwd, surface, updated_at_epoch,
                    activity_cursor, activity_source, subagent_count,
                    (SELECT related_id FROM session_relation r
                       WHERE r.environment_key = s.environment_key
                         AND r.agent = s.agent
                         AND r.session_id = s.session_id
                         AND r.kind = 'forkParent'
                       LIMIT 1),
                    s.source_fingerprint
               FROM session s";

pub(super) fn session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        key: SessionKey::new(
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ),
        source_kind: row.get(3)?,
        source_label: row.get(4)?,
        wsl_distro: row.get(5)?,
        title: row.get(6)?,
        title_source: row.get(7)?,
        cwd: row.get(8)?,
        surface: row.get(9)?,
        updated_at_epoch: row.get(10)?,
        activity_cursor: row.get(11)?,
        activity_source: row.get(12)?,
        subagent_count: row.get(13)?,
        fork_parent_session_id: row.get(14)?,
        source_fingerprint: row.get(15)?,
    })
}

/// Writes and reads turn rows for one claimed evidence pass.
///
/// Mutations require the matching evidence claim to remain `processing`.
/// Completed, requeued, superseded, and deleted claims cannot write again.
///
/// Holds an owned [`Store`] handle (cheap: it shares the app's one
/// connection, see [`Store`]'s doc comment) plus the session key and claim
/// fence the durable worker already knows for this pass. `Store` cannot
/// implement [`TurnRowStore`] directly — reading and writing rows needs the
/// session key and fence, and `Store` itself does not carry either.
#[derive(Clone)]
pub struct FencedTurnRowStore {
    store: Store,
    key: SessionKey,
    claim_fence: i64,
    /// Source keys the worker has told this pass genuinely resumed (see
    /// [`TurnRowStore::note_resumed_source`]). A mid-pass fact query widens
    /// its fence scope to include these sources' already-published rows,
    /// since only their new rows have moved to `claim_fence` so far.
    resumed_sources: Arc<Mutex<Vec<String>>>,
}

impl FencedTurnRowStore {
    pub fn new(store: Store, key: SessionKey, claim_fence: i64) -> Self {
        Self {
            store,
            key,
            claim_fence,
            resumed_sources: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn require_active_claim(&self, connection: &Connection) -> Result<(), TurnRowError> {
        let active = connection.query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM session_evidence AS evidence
                 JOIN session USING (environment_key, agent, session_id)
                  WHERE evidence.environment_key = ?1 AND evidence.agent = ?2
                    AND evidence.session_id = ?3 AND evidence.claim_fence = ?4
                    AND evidence.status = 'processing'
                    AND evidence.published_fence IS NOT ?4
             )",
            params![
                self.key.environment_key,
                self.key.agent,
                self.key.session_id,
                self.claim_fence,
            ],
            |row| row.get::<_, bool>(0),
        )?;
        if !active {
            return Err(TurnRowError("evidence claim is no longer active".into()));
        }
        Ok(())
    }

    /// The source keys the worker has told this pass genuinely resumed so
    /// far, cloned out of the mutex for a single query call.
    fn resumed_sources(&self) -> Vec<String> {
        self.resumed_sources
            .lock()
            .expect("resumed_sources mutex poisoned")
            .clone()
    }

    /// Builds the fence scope for a mid-pass fact query against `resumed`:
    /// the claim fence alone when nothing has resumed this pass, or a union
    /// with the session's published fence over the resumed source keys
    /// otherwise. `resumed` must be a slice this call's caller keeps alive
    /// for as long as the returned scope is in use.
    fn fact_query_scope<'a>(
        &self,
        connection: &Connection,
        resumed: &'a [String],
    ) -> Result<FenceScope<'a>, TurnRowError> {
        if resumed.is_empty() {
            return Ok(FenceScope::single(self.claim_fence));
        }
        let published_fence =
            read_published_fence(connection, &self.key)?.unwrap_or(self.claim_fence);
        Ok(FenceScope {
            claim_fence: self.claim_fence,
            published: Some(PublishedScope {
                fence: published_fence,
                source_keys: resumed,
            }),
        })
    }
}

impl TurnRowStore for FencedTurnRowStore {
    fn write_turn_rows(&self, rows: &[TurnRow]) -> Result<(), TurnRowError> {
        // One transaction per batch: one fsync for the batch instead of
        // one for each row, and the lock is held only for the batch.
        let mut connection = self.store.lock();
        let transaction = connection.transaction()?;
        self.require_active_claim(&transaction)?;
        insert_turn_rows(
            &transaction,
            &turn_session_key(&self.key),
            self.claim_fence,
            rows,
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn query_turn_facts(&self) -> Result<TurnFacts, TurnRowError> {
        let connection = self.store.lock();
        let resumed = self.resumed_sources();
        let scope = self.fact_query_scope(&connection, &resumed)?;
        Ok(query_turn_facts(
            &connection,
            &turn_session_key(&self.key),
            &scope,
        )?)
    }

    fn query_model_breakdown(
        &self,
    ) -> Result<
        std::collections::BTreeMap<String, antiburn_local::pricing::ModelTokens>,
        TurnRowError,
    > {
        let connection = self.store.lock();
        let resumed = self.resumed_sources();
        let scope = self.fact_query_scope(&connection, &resumed)?;
        Ok(query_model_breakdown(
            &connection,
            &turn_session_key(&self.key),
            &scope,
        )?)
    }

    fn query_model_runs(&self) -> Result<Vec<ModelRun>, TurnRowError> {
        let connection = self.store.lock();
        let resumed = self.resumed_sources();
        let scope = self.fact_query_scope(&connection, &resumed)?;
        Ok(query_model_runs(
            &connection,
            &turn_session_key(&self.key),
            &scope,
        )?)
    }

    fn query_pricing_breakdown(
        &self,
    ) -> Result<
        std::collections::BTreeMap<String, antiburn_local::pricing::ModelTokens>,
        TurnRowError,
    > {
        let connection = self.store.lock();
        let resumed = self.resumed_sources();
        let scope = self.fact_query_scope(&connection, &resumed)?;
        Ok(query_pricing_breakdown(
            &connection,
            &turn_session_key(&self.key),
            &scope,
        )?)
    }

    fn write_coverage_record(&self, record: &SessionCoverageRecord) -> Result<(), TurnRowError> {
        let mut connection = self.store.lock();
        let transaction = connection.transaction()?;
        self.require_active_claim(&transaction)?;
        insert_coverage_record(
            &transaction,
            &turn_session_key(&self.key),
            self.claim_fence,
            record,
        )?;
        transaction.commit()?;
        Ok(())
    }

    // R3: the coverage record is rebuilt complete every pass, resumed or
    // full, so it never needs the published-fence widening — the claim
    // fence alone always has this pass's whole record.
    fn query_coverage_record(&self) -> Result<Option<SessionCoverageRecord>, TurnRowError> {
        let connection = self.store.lock();
        Ok(query_coverage_record(
            &connection,
            &turn_session_key(&self.key),
            &FenceScope::single(self.claim_fence),
        )?)
    }

    /// Not scoped to `self.claim_fence` — `source_resume` carries no fence
    /// column, unlike every other row this store reads or writes.
    fn read_resume(&self, source_key: &str) -> Result<Option<StoredResume>, TurnRowError> {
        let connection = self.store.lock();
        Ok(query_source_resume(
            &connection,
            &turn_session_key(&self.key),
            source_key,
        )?)
    }

    fn write_resume(&self, source_key: &str, resume: StoredResume) -> Result<(), TurnRowError> {
        let mut connection = self.store.lock();
        let transaction = connection.transaction()?;
        self.require_active_claim(&transaction)?;
        insert_source_resume(
            &transaction,
            &turn_session_key(&self.key),
            source_key,
            &resume,
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn drop_resume(&self, source_key: &str) -> Result<(), TurnRowError> {
        let mut connection = self.store.lock();
        let transaction = connection.transaction()?;
        self.require_active_claim(&transaction)?;
        delete_source_resume(&transaction, &turn_session_key(&self.key), source_key)?;
        transaction.commit()?;
        Ok(())
    }

    fn delete_rows_for_source(&self, source_key: &str) -> Result<(), TurnRowError> {
        let mut connection = self.store.lock();
        let transaction = connection.transaction()?;
        self.require_active_claim(&transaction)?;
        delete_source_rows_at_fence(
            &transaction,
            &turn_session_key(&self.key),
            source_key,
            self.claim_fence,
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn note_resumed_source(&self, source_key: &str) {
        self.resumed_sources
            .lock()
            .expect("resumed_sources mutex poisoned")
            .push(source_key.to_string());
    }
}
