use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, named_params, params};

use super::{
    Remediation, RemediationCursor, RemediationEvidenceVersion, RemediationOrigin, RemediationPage,
    RemediationRecord, RemediationResult, RemediationState, Store,
};

const DEFAULT_PAGE_SIZE: u32 = 50;
const MAX_PAGE_SIZE: u32 = 100;
const MAX_JSON_BYTES: usize = 65_536;

const REMEDIATION_COLUMNS: &str = "remediation_id, target_key, state, origin,
    environment_key, agent, source_format, workspace_key, baseline_session_id,
    baseline_source_generation, baseline_published_fence, baseline_source_fingerprint,
    baseline_processed_fingerprint, baseline_parser_revision, baseline_analyzer_revision,
    baseline_evidence_schema_revision, finding_json, change_json, boundary_json,
    verification_json, savings_json, revisions_json, verification_input_revision,
    evaluated_input_revision, created_at_epoch, updated_at_epoch, applied_at_epoch,
    verified_at_epoch, recurred_at_epoch";

impl Store {
    /// Insert a remediation only while its ready baseline evidence remains current.
    ///
    /// Returns `false` when the evidence changed or the target already has an active remediation.
    pub fn insert_awaiting_remediation(&self, remediation: &Remediation) -> Result<bool> {
        validate_remediation_json(remediation)?;
        let connection = self.lock();
        let inserted = connection.execute(
            "INSERT INTO remediation (
                remediation_id, target_key, state, origin, environment_key, agent,
                source_format, workspace_key, baseline_session_id,
                baseline_source_generation, baseline_published_fence,
                baseline_source_fingerprint, baseline_processed_fingerprint,
                baseline_parser_revision, baseline_analyzer_revision,
                baseline_evidence_schema_revision, finding_json, change_json, boundary_json,
                verification_json, savings_json, revisions_json, created_at_epoch,
                updated_at_epoch, applied_at_epoch
             )
             SELECT :id, :target, 'awaitingVerification', :origin, :environment, :agent,
                    :format, :workspace, :session, :generation, :fence,
                    :source_fingerprint, :processed_fingerprint, :parser_revision,
                    :analyzer_revision, :evidence_revision, :finding, :change, :boundary,
                    :verification, :savings, :revisions, :created, :created, :applied
               FROM session AS source
               JOIN session_evidence AS evidence
                 ON evidence.environment_key = source.environment_key
                AND evidence.agent = source.agent
                AND evidence.session_id = source.session_id
              WHERE source.environment_key = :environment AND source.agent = :agent
                AND source.session_id = :session AND source.source_generation = :generation
                AND source.source_fingerprint IS :source_fingerprint
                AND evidence.status = 'ready'
                AND evidence.analyzed_generation = :generation
                AND evidence.published_fence = :fence
                AND evidence.processed_fingerprint IS :processed_fingerprint
                AND evidence.parser_revision = :parser_revision
                AND evidence.analyzer_revision = :analyzer_revision
                AND evidence.evidence_schema_revision = :evidence_revision
                AND NOT EXISTS (
                    SELECT 1 FROM remediation
                     WHERE remediation_id = :id
                        OR (environment_key = :environment AND agent = :agent
                            AND target_key = :target AND state != 'recurred'))",
            named_params! {
                ":id": remediation.remediation_id,
                ":target": remediation.target_key,
                ":origin": RemediationOrigin::as_str(remediation.origin),
                ":environment": remediation.environment_key,
                ":agent": remediation.agent,
                ":format": remediation.source_format,
                ":workspace": remediation.workspace_key,
                ":session": remediation.baseline_session_id,
                ":generation": remediation.baseline_source_generation,
                ":fence": remediation.baseline_published_fence,
                ":source_fingerprint": remediation.baseline_source_fingerprint,
                ":processed_fingerprint": remediation.baseline_processed_fingerprint,
                ":parser_revision": remediation.baseline_parser_revision,
                ":analyzer_revision": remediation.baseline_analyzer_revision,
                ":evidence_revision": remediation.baseline_evidence_schema_revision,
                ":finding": remediation.finding_json,
                ":change": remediation.change_json,
                ":boundary": remediation.boundary_json,
                ":verification": remediation.verification_json,
                ":savings": remediation.savings_json,
                ":revisions": remediation.revisions_json,
                ":created": remediation.created_at_epoch,
                ":applied": remediation.applied_at_epoch,
            },
        )?;
        Ok(inserted == 1)
    }

    /// Return one remediation by its opaque id.
    pub fn remediation(&self, remediation_id: &str) -> Result<Option<RemediationRecord>> {
        let connection = self.lock();
        connection
            .query_row(
                &format!("SELECT {REMEDIATION_COLUMNS} FROM remediation WHERE remediation_id = ?1"),
                [remediation_id],
                remediation_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// List remediations with a bounded exclusive keyset cursor.
    pub fn remediations(
        &self,
        environment_key: &str,
        cursor: Option<&RemediationCursor>,
        limit: Option<u32>,
    ) -> Result<RemediationPage> {
        let limit = limit.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
        let connection = self.lock();
        let sql = format!(
            "SELECT {REMEDIATION_COLUMNS} FROM remediation
              WHERE environment_key = ?1
                AND (?2 IS NULL OR created_at_epoch < ?2
                     OR (created_at_epoch = ?2 AND remediation_id < ?3))
              ORDER BY created_at_epoch DESC, remediation_id DESC
              LIMIT ?4"
        );
        let mut statement = connection.prepare(&sql)?;
        let (cursor_epoch, cursor_id) = cursor
            .map(|value| {
                (
                    Some(value.created_at_epoch),
                    Some(value.remediation_id.as_str()),
                )
            })
            .unwrap_or((None, None));
        let mut remediations = statement
            .query_map(
                params![
                    environment_key,
                    cursor_epoch,
                    cursor_id,
                    i64::from(limit) + 1
                ],
                remediation_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let has_more = remediations.len() > limit as usize;
        remediations.truncate(limit as usize);
        let next_cursor = has_more.then(|| {
            let last = remediations
                .last()
                .expect("a page with more rows has a last remediation");
            RemediationCursor {
                created_at_epoch: last.created_at_epoch,
                remediation_id: last.remediation_id.clone(),
            }
        });
        Ok(RemediationPage {
            remediations,
            next_cursor,
        })
    }

    /// Read the oldest dirty remediation and its observed input revision.
    ///
    /// A repeated read can duplicate work, but the revision guard rejects stale results.
    pub fn next_dirty_remediation(&self) -> Result<Option<RemediationRecord>> {
        let connection = self.lock();
        connection
            .query_row(
                &format!(
                    "SELECT {REMEDIATION_COLUMNS} FROM remediation
                      WHERE evaluated_input_revision < verification_input_revision
                        AND state != 'recurred'
                      ORDER BY updated_at_epoch, remediation_id
                      LIMIT 1"
                ),
                [],
                remediation_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Replace a verification result only when its observed inputs are current.
    pub fn replace_remediation_result(
        &self,
        remediation_id: &str,
        observed_input_revision: i64,
        observed_evidence: &RemediationEvidenceVersion,
        result: &RemediationResult,
    ) -> Result<bool> {
        validate_versioned_json("verification_json", &result.verification_json)?;
        validate_versioned_json("savings_json", &result.savings_json)?;
        let connection = self.lock();
        let updated = connection.execute(
            "UPDATE remediation AS remediation
                SET state = ?3, verification_json = ?4, savings_json = ?5,
                    evaluated_input_revision = ?2, updated_at_epoch = ?6,
                    verified_at_epoch = CASE
                        WHEN ?3 IN ('verified', 'recurred')
                        THEN COALESCE(verified_at_epoch, ?6)
                        ELSE verified_at_epoch END,
                    recurred_at_epoch = CASE
                        WHEN ?3 = 'recurred' THEN COALESCE(recurred_at_epoch, ?6)
                        ELSE recurred_at_epoch END
              WHERE remediation_id = ?1 AND verification_input_revision = ?2
                AND evaluated_input_revision < ?2 AND ?6 >= created_at_epoch
                AND ((state = 'awaitingVerification'
                      AND ?3 IN ('awaitingVerification', 'verified'))
                  OR (state = 'verified' AND ?3 IN ('verified', 'recurred')))
                AND EXISTS (
                    SELECT 1 FROM session AS source
                    JOIN session_evidence AS evidence
                      ON evidence.environment_key = source.environment_key
                     AND evidence.agent = source.agent
                     AND evidence.session_id = source.session_id
                    WHERE source.environment_key = remediation.environment_key
                      AND source.agent = remediation.agent
                      AND source.session_id = remediation.baseline_session_id
                      AND source.source_generation = ?7
                      AND source.source_fingerprint IS ?9
                      AND evidence.status = 'ready'
                      AND evidence.analyzed_generation = ?7
                      AND evidence.published_fence = ?8
                      AND evidence.processed_fingerprint IS ?10
                      AND evidence.parser_revision = ?11
                      AND evidence.analyzer_revision = ?12
                      AND evidence.evidence_schema_revision = ?13)",
            params![
                remediation_id,
                observed_input_revision,
                RemediationState::as_str(result.state),
                result.verification_json,
                result.savings_json,
                result.evaluated_at_epoch,
                observed_evidence.source_generation,
                observed_evidence.published_fence,
                observed_evidence.source_fingerprint,
                observed_evidence.processed_fingerprint,
                observed_evidence.parser_revision,
                observed_evidence.analyzer_revision,
                observed_evidence.evidence_schema_revision,
            ],
        )?;
        Ok(updated == 1)
    }

    /// Mark active remediations dirty when their stored revision set is stale.
    pub fn reconcile_remediation_revisions(
        &self,
        revisions_json: &str,
        now_epoch: i64,
    ) -> Result<usize> {
        validate_versioned_json("revisions_json", revisions_json)?;
        let connection = self.lock();
        Ok(connection.execute(
            "UPDATE remediation
                SET revisions_json = ?1,
                    verification_input_revision = verification_input_revision + 1,
                    updated_at_epoch = ?2
              WHERE state != 'recurred' AND revisions_json != ?1",
            params![revisions_json, now_epoch],
        )?)
    }
}

pub(super) fn mark_remediations_dirty_in(
    connection: &rusqlite::Connection,
    environment_key: &str,
    agent: &str,
    now_epoch: i64,
) -> Result<usize> {
    Ok(connection.execute(
        "UPDATE remediation
            SET verification_input_revision = verification_input_revision + 1,
                updated_at_epoch = ?3
          WHERE environment_key = ?1 AND agent = ?2 AND state != 'recurred'",
        params![environment_key, agent, now_epoch],
    )?)
}

fn validate_remediation_json(remediation: &Remediation) -> Result<()> {
    for (name, value) in [
        ("finding_json", remediation.finding_json.as_str()),
        ("change_json", remediation.change_json.as_str()),
        ("boundary_json", remediation.boundary_json.as_str()),
        ("verification_json", remediation.verification_json.as_str()),
        ("savings_json", remediation.savings_json.as_str()),
        ("revisions_json", remediation.revisions_json.as_str()),
    ] {
        validate_versioned_json(name, value)?;
    }
    Ok(())
}

fn validate_versioned_json(name: &str, value: &str) -> Result<()> {
    ensure!(
        value.len() <= MAX_JSON_BYTES,
        "{name} exceeds {MAX_JSON_BYTES} bytes"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(value).with_context(|| format!("{name} is not valid JSON"))?;
    ensure!(
        parsed
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|version| version > 0),
        "{name} must contain a positive integer version"
    );
    Ok(())
}

fn remediation_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RemediationRecord> {
    let state = row
        .get::<_, String>(2)?
        .parse()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let origin = row
        .get::<_, String>(3)?
        .parse()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(RemediationRecord {
        remediation_id: row.get(0)?,
        target_key: row.get(1)?,
        state,
        origin,
        environment_key: row.get(4)?,
        agent: row.get(5)?,
        source_format: row.get(6)?,
        workspace_key: row.get(7)?,
        baseline_session_id: row.get(8)?,
        baseline_source_generation: row.get(9)?,
        baseline_published_fence: row.get(10)?,
        baseline_source_fingerprint: row.get(11)?,
        baseline_processed_fingerprint: row.get(12)?,
        baseline_parser_revision: row.get(13)?,
        baseline_analyzer_revision: row.get(14)?,
        baseline_evidence_schema_revision: row.get(15)?,
        finding_json: row.get(16)?,
        change_json: row.get(17)?,
        boundary_json: row.get(18)?,
        verification_json: row.get(19)?,
        savings_json: row.get(20)?,
        revisions_json: row.get(21)?,
        verification_input_revision: row.get(22)?,
        evaluated_input_revision: row.get(23)?,
        created_at_epoch: row.get(24)?,
        updated_at_epoch: row.get(25)?,
        applied_at_epoch: row.get(26)?,
        verified_at_epoch: row.get(27)?,
        recurred_at_epoch: row.get(28)?,
    })
}
