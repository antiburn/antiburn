//! Durable publication and reconciliation for source-native tool results.
//!
//! The outbox sequence orders committed delivery. The source position and
//! revision stay inside each event and never act as the delivery cursor. A
//! consumer must use [`Store::native_tool_result_publications_after`] and
//! persist its own position only after it accepts the returned events.

use antiburn_local::analysis::{
    SourceFormat, ToolResultChange, ToolResultObservation, ToolResultPosition,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::store::{SessionKey, Store};

#[derive(Clone, Copy, Default)]
pub(crate) struct PublicationPolicy<'a> {
    pub invalidation_effects:
        Option<&'a std::cell::Cell<super::trace_effects::InvalidationEffects>>,
    pub(crate) baseline: Option<u64>,
    pub(crate) history_complete: Option<bool>,
    pub(crate) gap_reason: Option<&'a str>,
    pub(crate) gap_kind: PublicationGapKind,
    pub(crate) suppress_coverage: bool,
    pub(crate) live_eligible: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationGapKind {
    #[default]
    Invalidated,
    Partial,
    SegmentReset,
}

impl PublicationGapKind {
    fn code(self) -> &'static str {
        match self {
            Self::Invalidated => "invalidated",
            Self::Partial => "partial",
            Self::SegmentReset => "segment_reset",
        }
    }
}

/// One parent-session snapshot supplied to a winning projection publication.
///
/// `complete` describes membership of the whole snapshot. A false value still
/// publishes its observed updates and removals, then records a coverage gap;
/// it never infers removals from absent identities. `source_format` identifies
/// the source even when a complete snapshot contains no results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultBatch {
    /// Source contract selected by discovery for this parent session.
    pub source_format: SourceFormat,
    /// Results observed in the same parse pass as the published projections.
    pub results: Vec<ToolResultObservation>,
    /// Whether the result membership is known to be complete.
    pub complete: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct ToolResultBatchRef<'a> {
    pub source_format: SourceFormat,
    pub results: &'a [ToolResultObservation],
    pub complete: bool,
}

impl<'a> From<&'a ToolResultBatch> for ToolResultBatchRef<'a> {
    fn from(batch: &'a ToolResultBatch) -> Self {
        Self {
            source_format: batch.source_format,
            results: &batch.results,
            complete: batch.complete,
        }
    }
}

/// Native-result and source-fingerprint options for one projection publish.
#[derive(Debug, Clone, Copy, Default)]
pub struct ToolResultPublishOptions<'a> {
    /// Verified fingerprint of the parent source.
    pub source_fingerprint: Option<&'a str>,
    /// Complete or partial native observation batch to reconcile after a win.
    pub batch: Option<&'a ToolResultBatch>,
}

/// A source-neutral durable publication event.
///
/// `publication_sequence` is the only delivery cursor. `native_position` and
/// `native_revision` describe producer order and mutation identity. Coverage
/// gaps have no native result identity or payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultPublication {
    /// Explicit session selection at the original publication, if known.
    pub session_selection: Option<antiburn_checks::context::ObservedAttribution>,
    /// Increasing database-wide sequence assigned at commit time.
    pub publication_sequence: i64,
    /// Whether this event adds or changes a result, removes one, or marks a gap.
    pub outcome: ToolResultPublicationOutcome,
    /// Owning local session.
    pub session: SessionKey,
    /// Source format for result events; absent for a coverage gap.
    pub source_format: Option<String>,
    /// Stable source-native logical identity, encoded as JSON.
    pub native_identity: Option<String>,
    /// Producer position, independent of publication sequence.
    pub native_position: Option<ToolResultPosition>,
    /// Producer revision, independent of publication sequence.
    pub native_revision: Option<i64>,
    /// Full normalized source observation, including completeness.
    pub payload: Option<ToolResultObservation>,
    /// Whether absent identities may have been inferred as removed.
    pub complete: bool,
    /// Source generation accepted by the winning publication.
    pub source_generation: i64,
    /// Winning worker claim fence.
    pub claim_fence: i64,
    /// Journal delivery marks historical results independently of source generations.
    pub live_eligible: Option<bool>,
    pub gap_reason: Option<String>,
    pub gap_kind: PublicationGapKind,
}

/// Result of one durable native tool-result publication event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolResultPublicationOutcome {
    /// A new or corrected observation became current.
    Update,
    /// A source-declared or complete-snapshot removal became current.
    Removal,
    /// The source did not prove complete membership.
    CoverageGap,
    /// A complete snapshot restored source coverage after a gap.
    CoverageRestored,
}

/// Current reconciled state for one native logical result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentToolResultRecord {
    /// Owning local session.
    pub session: SessionKey,
    /// Exact producer contract name.
    pub source_format: String,
    /// Stable source-native logical identity, encoded as JSON.
    pub native_identity: String,
    /// Most recently accepted native position.
    pub native_position: ToolResultPosition,
    /// Most recently accepted producer revision.
    pub native_revision: i64,
    /// Latest source observation.
    pub result: ToolResultObservation,
    /// Whether required facts in the result are complete.
    pub complete: bool,
    /// Whether the logical result is currently removed.
    pub removed: bool,
    /// Event that produced this state.
    pub publication_sequence: i64,
}

impl Store {
    /// Reads a bounded page of committed native-result events after a cursor.
    ///
    /// The result is ordered by durable publication sequence, not source order
    /// or notification arrival. A notification only prompts this read; callers
    /// recover missed notifications by polling from their last accepted
    /// sequence. `limit` must be between 1 and 1000.
    pub fn native_tool_result_publications_after(
        &self,
        after_sequence: i64,
        limit: u16,
    ) -> Result<Vec<ToolResultPublication>> {
        ensure!(
            after_sequence >= 0,
            "publication sequence must be non-negative"
        );
        ensure!(
            (1..=1000).contains(&limit),
            "limit must be between 1 and 1000"
        );
        let connection = self.lock();
        read_publications_after(&connection, after_sequence, limit, None)
    }

    /// Reads publications for one exact stored project and environment.
    pub fn native_tool_result_publications_for_project_after(
        &self,
        environment_key: &str,
        project_root: &str,
        after_sequence: i64,
        limit: u16,
    ) -> Result<Vec<ToolResultPublication>> {
        ensure!(
            after_sequence >= 0,
            "publication sequence must be non-negative"
        );
        ensure!(
            (1..=1000).contains(&limit),
            "limit must be between 1 and 1000"
        );
        let connection = self.lock();
        read_publications_after(
            &connection,
            after_sequence,
            limit,
            Some((environment_key, project_root)),
        )
    }

    /// Reads the current state of active and removed native results for one
    /// session in source identity order.
    pub fn native_tool_results_current(
        &self,
        session: &SessionKey,
    ) -> Result<Vec<CurrentToolResultRecord>> {
        let connection = self.lock();
        read_current(&connection, session)
    }
}

pub(crate) fn delete_session_data_in(connection: &Connection, session: &SessionKey) -> Result<()> {
    super::request_origin::delete_session_binding_in(connection, session)?;
    connection.execute(
        "DELETE FROM burn_check_session_attachment WHERE branch_key = ?1",
        [session.to_identity_json()],
    )?;
    connection.execute(
        "DELETE FROM native_file_delivery WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![session.environment_key, session.agent, session.session_id],
    )?;
    connection.execute(
        "DELETE FROM opencode_journal_delivery WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![session.environment_key, session.agent, session.session_id],
    )?;
    connection.execute(
        "DELETE FROM native_tool_result_current
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![session.environment_key, session.agent, session.session_id],
    )?;
    connection.execute(
        "DELETE FROM native_tool_result_publication
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![session.environment_key, session.agent, session.session_id],
    )?;
    Ok(())
}

pub(crate) fn clear_local_data_in(connection: &Connection) -> Result<()> {
    super::request_origin::clear_bindings_in(connection)?;
    connection.execute("DELETE FROM opencode_journal_delivery", [])?;
    connection.execute("DELETE FROM native_file_delivery", [])?;
    connection.execute("DELETE FROM burn_check_session_attachment", [])?;
    connection.execute("DELETE FROM native_tool_result_current", [])?;
    connection.execute("DELETE FROM native_tool_result_publication", [])?;
    Ok(())
}

/// Reconciles one optional source snapshot inside the winning publication
/// transaction. The caller invokes this only after the claim-fence update wins.
pub(crate) fn reconcile_batch_in(
    transaction: &Transaction<'_>,
    session: &SessionKey,
    source_generation: i64,
    claim_fence: i64,
    batch: &ToolResultBatch,
) -> Result<()> {
    reconcile_with_policy(
        transaction,
        session,
        source_generation,
        claim_fence,
        batch,
        PublicationPolicy::default(),
    )
}

pub(crate) fn reconcile_with_policy(
    transaction: &Transaction<'_>,
    session: &SessionKey,
    source_generation: i64,
    claim_fence: i64,
    batch: &ToolResultBatch,
    policy: PublicationPolicy<'_>,
) -> Result<()> {
    reconcile_borrowed_with_policy(
        transaction,
        session,
        source_generation,
        claim_fence,
        batch.into(),
        policy,
    )
}

pub(crate) fn reconcile_borrowed_with_policy(
    transaction: &Transaction<'_>,
    session: &SessionKey,
    source_generation: i64,
    claim_fence: i64,
    batch: ToolResultBatchRef<'_>,
    policy: PublicationPolicy<'_>,
) -> Result<()> {
    let parent_format = serde_json::to_string(&batch.source_format)?;
    if !policy.suppress_coverage
        && let Some(complete) = policy.history_complete
    {
        publish_coverage_with_kind_in(
            transaction,
            session,
            source_generation,
            claim_fence,
            batch,
            complete,
            policy.gap_reason,
            policy.gap_kind,
        )?;
    }
    let mut present_by_format = std::collections::BTreeMap::<String, Vec<String>>::new();
    let mut changed = std::collections::BTreeSet::new();
    for result in batch.results {
        ensure!(
            result.session_id == session.session_id,
            "batch contains a different native session"
        );
        let source_format = serde_json::to_string(&result.source_format)?;
        let identity = native_identity(result)?;
        let position = serde_json::to_string(&result.position)?;
        let revision =
            i64::try_from(result.revision).context("native revision exceeds SQLite range")?;
        present_by_format
            .entry(source_format.clone())
            .or_default()
            .push(identity.clone());
        let previous: Option<(String, i64)> = transaction
            .query_row(
                "SELECT payload_json, is_removed FROM native_tool_result_current
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                    AND source_format = ?4 AND native_identity = ?5",
                params![
                    session.environment_key,
                    session.agent,
                    session.session_id,
                    source_format,
                    identity
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if result.change == ToolResultChange::Update
            && previous.as_ref().is_some_and(|(_, removed)| *removed != 0)
        {
            continue;
        }
        let mut result = std::borrow::Cow::Borrowed(result);
        if let Some((old, _)) = &previous {
            let old: ToolResultObservation = serde_json::from_str(old)?;
            if old.revision > result.revision {
                continue;
            }
            if old.finality {
                result.to_mut().context = old.context;
            }
        }
        let payload = serde_json::to_string(&result)?;
        if previous.as_ref().is_some_and(|(old, removed)| {
            old == &payload && *removed == i64::from(result.change == ToolResultChange::Removal)
        }) {
            continue;
        }

        let outcome = if result.change == ToolResultChange::Removal {
            ToolResultPublicationOutcome::Removal
        } else {
            ToolResultPublicationOutcome::Update
        };
        let sequence = insert_event(
            transaction,
            session,
            outcome,
            Some(&source_format),
            Some(&identity),
            Some(&position),
            Some(
                result
                    .context
                    .as_ref()
                    .map_or(result.position, |context| context.result_position),
            ),
            Some(revision),
            Some(&payload),
            result.completeness == antiburn_local::analysis::ToolResultCompleteness::Complete,
            source_generation,
            claim_fence,
            policy.live_eligible.or_else(|| {
                policy.baseline.map(|boundary| {
                    let position = result
                        .context
                        .as_ref()
                        .map_or(result.position, |context| context.result_position);
                    match position {
                        ToolResultPosition::OpenCodeEvent { sequence, .. } => sequence > boundary,
                        ToolResultPosition::OpenCodeV2Event { sequence, .. } => sequence > boundary,
                        ToolResultPosition::FileBytes { record_end, .. } => record_end > boundary,
                        ToolResultPosition::Jsonl { .. } => false,
                    }
                })
            }),
            None,
            PublicationGapKind::Invalidated,
        )?;
        transaction.execute(
            "INSERT INTO native_tool_result_current (
                 environment_key, agent, session_id, source_format, native_identity,
                 native_position_json, native_revision, payload_json, complete,
                  is_removed, publication_sequence, source_generation, claim_fence, owner_message_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(environment_key, agent, session_id, source_format, native_identity)
             DO UPDATE SET native_position_json = excluded.native_position_json,
                 native_revision = excluded.native_revision, payload_json = excluded.payload_json,
                 complete = excluded.complete, is_removed = excluded.is_removed,
                 publication_sequence = excluded.publication_sequence,
                  source_generation = excluded.source_generation, claim_fence = excluded.claim_fence,
                  owner_message_id = excluded.owner_message_id",
            params![
                session.environment_key, session.agent, session.session_id, source_format,
                identity, position, revision, payload,
                i64::from(result.completeness == antiburn_local::analysis::ToolResultCompleteness::Complete),
                i64::from(result.change == ToolResultChange::Removal), sequence,
                source_generation, claim_fence, result.record_id,
            ],
        )?;
        changed.insert((source_format, identity));
    }

    if batch.complete {
        let mut statement = transaction.prepare(
            "SELECT native_identity, native_position_json, native_revision, payload_json
               FROM native_tool_result_current
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                 AND source_format = ?4 AND is_removed = 0 AND event_kind = 'tool_result'",
        )?;
        let stale = statement
            .query_map(
                params![
                    session.environment_key,
                    session.agent,
                    session.session_id,
                    parent_format
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for (identity, position, revision, payload) in stale {
            if present_by_format
                .get(&parent_format)
                .is_some_and(|present| present.contains(&identity))
            {
                continue;
            }
            let sequence = insert_event(
                transaction,
                session,
                ToolResultPublicationOutcome::Removal,
                Some(&parent_format),
                Some(&identity),
                Some(&position),
                None,
                Some(revision),
                Some(&payload),
                true,
                source_generation,
                claim_fence,
                None,
                None,
                PublicationGapKind::Invalidated,
            )?;
            transaction.execute(
                "UPDATE native_tool_result_current SET is_removed = 1,
                    publication_sequence = ?6, source_generation = ?7, claim_fence = ?8
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                    AND source_format = ?4 AND native_identity = ?5 AND is_removed = 0",
                params![
                    session.environment_key,
                    session.agent,
                    session.session_id,
                    parent_format,
                    identity,
                    sequence,
                    source_generation,
                    claim_fence
                ],
            )?;
            changed.insert((parent_format.clone(), identity));
        }
    }

    for (format, identity) in changed {
        super::evidence::invalidate_native_identity_dependents_in(
            transaction,
            session,
            &format,
            &identity,
            super::evidence::EvidenceEventKind::ToolResult,
        )?
        .record(policy.invalidation_effects);
    }
    if !policy.suppress_coverage && policy.history_complete.is_none() {
        publish_coverage_with_kind_in(
            transaction,
            session,
            source_generation,
            claim_fence,
            batch,
            batch.complete,
            None,
            policy.gap_kind,
        )?;
    }
    Ok(())
}

pub(crate) fn publish_coverage_in(
    transaction: &Transaction<'_>,
    session: &SessionKey,
    source_generation: i64,
    claim_fence: i64,
    batch: &ToolResultBatch,
    complete: bool,
    reason: Option<&str>,
) -> Result<()> {
    publish_coverage_with_kind_in(
        transaction,
        session,
        source_generation,
        claim_fence,
        batch.into(),
        complete,
        reason,
        PublicationGapKind::Invalidated,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn publish_coverage_with_kind_in(
    transaction: &Transaction<'_>,
    session: &SessionKey,
    source_generation: i64,
    claim_fence: i64,
    batch: ToolResultBatchRef<'_>,
    complete: bool,
    reason: Option<&str>,
    kind: PublicationGapKind,
) -> Result<()> {
    let parent_format = serde_json::to_string(&batch.source_format)?;
    let previous_coverage: Option<(i64, Option<String>, String)> = transaction
        .query_row(
            "SELECT complete, gap_reason, gap_kind FROM native_tool_result_publication
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND source_format = ?4 AND outcome IN ('coverage_gap', 'coverage_complete')
              ORDER BY publication_sequence DESC LIMIT 1",
            params![
                session.environment_key,
                session.agent,
                session.session_id,
                parent_format
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let next_coverage = i64::from(complete);
    let empty_baseline = batch.results.is_empty()
        && !transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM native_tool_result_publication
             WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3)",
            params![session.environment_key, session.agent, session.session_id],
            |row| row.get::<_, bool>(0),
        )?;
    if previous_coverage.as_ref().is_some_and(|previous| {
        previous.0 != next_coverage || previous.1.as_deref() != reason || previous.2 != kind.code()
    }) || (previous_coverage.is_none() && (!complete || empty_baseline))
    {
        insert_event(
            transaction,
            session,
            if complete {
                ToolResultPublicationOutcome::CoverageRestored
            } else {
                ToolResultPublicationOutcome::CoverageGap
            },
            Some(&parent_format),
            None,
            None,
            None,
            None,
            None,
            complete,
            source_generation,
            claim_fence,
            None,
            reason,
            kind,
        )?;
    }
    Ok(())
}

pub(super) fn native_identity(result: &ToolResultObservation) -> Result<String> {
    // The producer IDs identify corrections; position is the stable fallback
    // for formats that do not provide a record or result identifier.
    let identity =
        if result.record_id.is_some() || result.call_id.is_some() || result.result_id.is_some() {
            serde_json::to_string(&(
                &result.session_id,
                &result.record_id,
                &result.call_id,
                &result.result_id,
            ))?
        } else {
            serde_json::to_string(&(&result.session_id, &result.position))?
        };
    Ok(identity)
}

#[allow(clippy::too_many_arguments)]
fn insert_event(
    transaction: &Transaction<'_>,
    session: &SessionKey,
    outcome: ToolResultPublicationOutcome,
    source_format: Option<&str>,
    identity: Option<&str>,
    position: Option<&str>,
    selection_position: Option<ToolResultPosition>,
    revision: Option<i64>,
    payload: Option<&str>,
    complete: bool,
    source_generation: i64,
    claim_fence: i64,
    live_eligible: Option<bool>,
    gap_reason: Option<&str>,
    gap_kind: PublicationGapKind,
) -> Result<i64> {
    let session_selection_json = match (source_format, identity, position) {
        (Some(format), Some(identity), Some(position)) => {
            super::observers::frozen_selection_json_in(
                transaction,
                session,
                format,
                identity,
                match selection_position {
                    Some(position) => position,
                    None => serde_json::from_str(position)?,
                },
            )?
        }
        _ => None,
    };
    let inserted = transaction.execute(
        "INSERT INTO native_tool_result_publication (
             outcome, environment_key, agent, session_id, source_format,
             native_identity, native_position_json, native_revision, payload_json,
             complete, source_generation, claim_fence, live_eligible, gap_reason,
              project_cwd, session_incarnation, gap_kind, session_selection_json)
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                 s.cwd, s.incarnation, ?15, ?16 FROM session s
          WHERE s.environment_key = ?2 AND s.agent = ?3 AND s.session_id = ?4",
        params![
            match outcome {
                ToolResultPublicationOutcome::Update => "update",
                ToolResultPublicationOutcome::Removal => "removal",
                ToolResultPublicationOutcome::CoverageGap => "coverage_gap",
                ToolResultPublicationOutcome::CoverageRestored => "coverage_complete",
            },
            session.environment_key,
            session.agent,
            session.session_id,
            source_format,
            identity,
            position,
            revision,
            payload,
            i64::from(complete),
            source_generation,
            claim_fence,
            live_eligible,
            gap_reason,
            gap_kind.code(),
            session_selection_json,
        ],
    )?;
    ensure!(inserted == 1, "native publication has no owning session");
    Ok(transaction.last_insert_rowid())
}

fn read_publications_after(
    connection: &Connection,
    after_sequence: i64,
    limit: u16,
    project: Option<(&str, &str)>,
) -> Result<Vec<ToolResultPublication>> {
    let mut statement = connection.prepare(
        "SELECT p.publication_sequence, p.outcome, p.environment_key, p.agent, p.session_id,
                source_format, native_identity, native_position_json, native_revision,
                payload_json, complete, source_generation, claim_fence, live_eligible, gap_reason,
                gap_kind, session_selection_json
           FROM native_tool_result_publication p
          WHERE p.publication_sequence > ?1 AND p.event_kind = 'tool_result'
            AND (?3 IS NULL OR (p.environment_key = ?3 AND EXISTS (
                SELECT 1 FROM session s WHERE s.environment_key = p.environment_key
                   AND s.agent = p.agent AND s.session_id = p.session_id AND s.cwd = ?4
                    AND (p.session_incarnation IS NULL OR p.project_cwd = ?4)
                   AND (p.session_incarnation IS NULL OR p.session_incarnation = s.incarnation))))
          ORDER BY p.publication_sequence LIMIT ?2",
    )?;
    let mut rows = statement.query(params![
        after_sequence,
        i64::from(limit),
        project.map(|(environment, _)| environment),
        project.map(|(_, root)| root)
    ])?;
    let mut events = Vec::new();
    let mut bytes = 0;
    while let Some(row) = rows.next()? {
        let payload: Option<String> = row.get(9)?;
        let selection: Option<String> = row.get(16)?;
        let size =
            payload.as_ref().map_or(0, String::len) + selection.as_ref().map_or(0, String::len);
        if !events.is_empty() && bytes + size > antiburn_local::analysis::MAX_TOOL_RESULT_BYTES {
            break;
        }
        bytes += size;
        events.push(decode_publication(row, payload)??);
    }
    Ok(events)
}

fn decode_publication(
    row: &rusqlite::Row<'_>,
    payload: Option<String>,
) -> rusqlite::Result<Result<ToolResultPublication>> {
    let position: Option<String> = row.get(7)?;
    let event = (|| -> Result<ToolResultPublication> {
        Ok(ToolResultPublication {
            session_selection: row
                .get::<_, Option<String>>(16)?
                .map(|json| serde_json::from_str(&json))
                .transpose()?,
            publication_sequence: row.get(0)?,
            outcome: match row.get::<_, String>(1)?.as_str() {
                "update" => ToolResultPublicationOutcome::Update,
                "removal" => ToolResultPublicationOutcome::Removal,
                "coverage_gap" => ToolResultPublicationOutcome::CoverageGap,
                "coverage_complete" => ToolResultPublicationOutcome::CoverageRestored,
                value => anyhow::bail!("unknown native-result publication outcome: {value}"),
            },
            session: SessionKey::new(
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ),
            source_format: row.get(5)?,
            native_identity: row.get(6)?,
            native_position: position
                .map(|value| serde_json::from_str(&value))
                .transpose()?,
            native_revision: row.get(8)?,
            payload: payload
                .map(|value| serde_json::from_str(&value))
                .transpose()?,
            complete: row.get::<_, i64>(10)? != 0,
            source_generation: row.get(11)?,
            claim_fence: row.get(12)?,
            live_eligible: row.get(13)?,
            gap_reason: row.get(14)?,
            gap_kind: match row.get::<_, String>(15)?.as_str() {
                "invalidated" => PublicationGapKind::Invalidated,
                "partial" => PublicationGapKind::Partial,
                "segment_reset" => PublicationGapKind::SegmentReset,
                value => anyhow::bail!("unknown native-result gap kind: {value}"),
            },
        })
    })();
    Ok(event)
}

fn read_current(
    connection: &Connection,
    session: &SessionKey,
) -> Result<Vec<CurrentToolResultRecord>> {
    let mut statement = connection.prepare(
        "SELECT source_format, native_identity, native_position_json, native_revision,
                payload_json, complete, is_removed, publication_sequence
           FROM native_tool_result_current
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
             AND event_kind = 'tool_result' ORDER BY source_format, native_identity",
    )?;
    let rows = statement
        .query_map(
            params![session.environment_key, session.agent, session.session_id],
            |row| {
                let source_format_json: String = row.get(0)?;
                let identity: String = row.get(1)?;
                let position: String = row.get(2)?;
                let payload: String = row.get(4)?;
                let decoded = (|| -> Result<_> {
                    Ok(CurrentToolResultRecord {
                        session: session.clone(),
                        source_format: serde_json::from_str(&source_format_json)?,
                        native_identity: identity,
                        native_position: serde_json::from_str(&position)?,
                        native_revision: row.get(3)?,
                        result: serde_json::from_str(&payload)?,
                        complete: row.get::<_, i64>(5)? != 0,
                        removed: row.get::<_, i64>(6)? != 0,
                        publication_sequence: row.get(7)?,
                    })
                })();
                Ok(decoded)
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().collect()
}

#[cfg(test)]
mod tests;
