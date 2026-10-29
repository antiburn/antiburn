//! Typed native and attachment events share the existing durable outbox.

use antiburn_checks::context::ObservedAttribution;
use antiburn_checks::domain::{ActivationId, BranchId, EvidenceVersion, SessionStartReason};
use antiburn_checks::source::PublishedContext;
use antiburn_local::analysis::{
    NativeEvidenceBatch, NativeEvidenceFact, NativeEvidencePosition, NativeMessageObservation,
    PublicMessageObservation, PublicMessagePayload, ResponseCycleObservation, ToolResultChange,
    ToolResultObservation, UserMessageChange, UserMessageObservation,
};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use super::publications::{
    PublicationGapKind, PublicationPolicy, ToolResultBatch, ToolResultPublicationOutcome,
};
use crate::store::{SessionKey, Store};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceEventKind {
    ToolResult,
    UserMessage,
    AssistantMessage,
    ToolCall,
    ResponseCycle,
    SessionStart,
    SessionIdle,
}

impl EvidenceEventKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::ToolResult => "tool_result",
            Self::UserMessage => "user_message",
            Self::AssistantMessage => "assistant_message",
            Self::ToolCall => "tool_call",
            Self::ResponseCycle => "response_cycle",
            Self::SessionStart => "session_start",
            Self::SessionIdle => "session_idle",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeEvidencePayload {
    ToolResult(ToolResultObservation),
    UserMessage(UserMessageObservation),
    PublicMessage(PublicMessageObservation),
    ResponseCycle(ResponseCycleObservation),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionObserverKey {
    pub activation: ActivationId,
    pub branch: BranchId,
    pub incarnation: i64,
    pub attachment_epoch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostSessionPayload {
    Start {
        reason: SessionStartReason,
    },
    Idle {
        activity_revision: u64,
        idle_seconds: u32,
        last_activity_at_ms: Option<i64>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostSessionObservation {
    pub observer: SessionObserverKey,
    pub observed_at_ms: i64,
    pub attribution: ObservedAttribution,
    #[serde(deserialize_with = "deserialize_published_context")]
    pub context: PublishedContext,
    pub payload: HostSessionPayload,
}

fn deserialize_published_context<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<PublishedContext, D::Error> {
    #[derive(Deserialize)]
    struct StoredContext {
        requests: Vec<(String, serde_json::Value)>,
        session_id: String,
        agent_name: String,
        project_root: Option<String>,
        version: String,
        main_session: bool,
        complete_through_event: bool,
        #[serde(default)]
        inventories: antiburn_checks::context::CurrentInventories,
        #[serde(default)]
        session_selection: Option<ObservedAttribution>,
    }
    let context = StoredContext::deserialize(deserializer)?;
    Ok(PublishedContext {
        requests: context.requests,
        session_id: context.session_id,
        agent_name: context.agent_name,
        project_root: context.project_root,
        version: context.version,
        main_session: context.main_session,
        complete_through_event: context.complete_through_event,
        inventories: context.inventories,
        session_selection: context.session_selection,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EvidencePublicationPayload {
    Native(NativeEvidencePayload),
    Host(HostSessionObservation),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidencePublication {
    #[serde(default)]
    pub gap_kind: PublicationGapKind,
    #[serde(default)]
    pub session_selection: Option<ObservedAttribution>,
    pub publication_sequence: i64,
    pub session: SessionKey,
    pub event_kind: EvidenceEventKind,
    pub outcome: ToolResultPublicationOutcome,
    pub payload: Option<EvidencePublicationPayload>,
    pub native_identity: Option<String>,
    pub native_position: Option<NativeEvidencePosition>,
    pub native_revision: Option<i64>,
    pub first_eligible_publication: Option<i64>,
    pub target: Option<SessionObserverKey>,
    pub source_format: Option<String>,
    pub complete: bool,
    pub source_generation: i64,
    pub claim_fence: i64,
    pub live_eligible: Option<bool>,
    pub gap_reason: Option<String>,
    pub dependencies: Vec<NativeCurrentBinding>,
    pub source_evidence_version: Option<EvidenceVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeCurrentBinding {
    pub identity: String,
    pub event_kind: EvidenceEventKind,
    pub source_format: String,
    pub revision: i64,
    pub publication_sequence: i64,
    pub payload_sha256: String,
}

impl NativeCurrentBinding {
    pub fn version(&self) -> EvidenceVersion {
        EvidenceVersion(super::super::burn_checks::payload_sha256(
            &serde_json::to_string(self).expect("native binding serializes"),
        ))
    }
}

/// Parameters identify an activation. The attachment comparison also fences old epochs.
pub(crate) fn target_predicate(
    alias: &str,
    check: usize,
    scope: usize,
    generation: usize,
) -> String {
    format!(
        "({alias}.target_check_id IS NULL OR ({alias}.target_check_id = ?{check}
        AND {alias}.target_scope_key = ?{scope} AND {alias}.target_generation = ?{generation}
        AND EXISTS(SELECT 1 FROM burn_check_session_attachment target_attachment
          WHERE target_attachment.check_id = {alias}.target_check_id
            AND target_attachment.activation_scope_key = {alias}.target_scope_key
            AND target_attachment.generation = {alias}.target_generation
            AND target_attachment.branch_key = {alias}.target_branch_key
            AND target_attachment.incarnation = {alias}.target_incarnation
            AND target_attachment.observer_epoch = {alias}.target_attachment_epoch)))"
    )
}

impl Store {
    pub fn native_evidence_publications_after(
        &self,
        after: i64,
        limit: u16,
    ) -> Result<Vec<EvidencePublication>> {
        read_in(&self.lock(), after, limit, None, None)
    }

    pub fn native_evidence_publications_for_project_after(
        &self,
        environment: &str,
        cwd: &str,
        activation: Option<&ActivationId>,
        after: i64,
        limit: u16,
    ) -> Result<Vec<EvidencePublication>> {
        read_in(
            &self.lock(),
            after,
            limit,
            Some((environment, cwd)),
            activation,
        )
    }
}

fn read_in(
    connection: &Connection,
    after: i64,
    limit: u16,
    project: Option<(&str, &str)>,
    activation: Option<&ActivationId>,
) -> Result<Vec<EvidencePublication>> {
    ensure!(
        after >= 0 && (1..=1000).contains(&limit),
        "invalid evidence page bounds"
    );
    let target = target_predicate("p", 5, 6, 7);
    let mut statement = connection.prepare(&format!("SELECT p.publication_sequence, p.outcome,
        p.environment_key, p.agent, p.session_id, p.source_format, p.native_identity,
        p.native_position_json, p.native_revision, p.payload_json, p.complete, p.source_generation,
        p.claim_fence, p.live_eligible, p.gap_reason, p.event_kind, p.first_eligible_publication,
        p.dependency_binding_json, p.session_selection_json, p.gap_kind FROM native_tool_result_publication p
        WHERE p.publication_sequence > ?1 AND {target}
          AND (?3 IS NULL OR (p.environment_key = ?3 AND EXISTS(SELECT 1 FROM session s
            WHERE s.environment_key = p.environment_key AND s.agent = p.agent AND s.session_id = p.session_id
              AND s.cwd = ?4 AND (p.session_incarnation IS NULL OR
                (p.session_incarnation = s.incarnation AND p.project_cwd = ?4)))))
        ORDER BY p.publication_sequence LIMIT ?2"))?;
    let scope =
        activation.map(|id| crate::check_store::StoreCheckStore::activation_scope_key(&id.scope));
    let mut rows = statement.query(params![
        after,
        limit,
        project.map(|p| p.0),
        project.map(|p| p.1),
        activation.map(|id| &id.definition_id),
        scope,
        activation.map(|id| id.generation.0)
    ])?;
    let mut events = Vec::new();
    let mut bytes = 0;
    while let Some(row) = rows.next()? {
        let payload: Option<String> = row.get(9)?;
        let binding: Option<String> = row.get(17)?;
        let selection: Option<String> = row.get(18)?;
        let size = payload.as_ref().map_or(0, String::len)
            + binding.as_ref().map_or(0, String::len)
            + selection.as_ref().map_or(0, String::len);
        if !events.is_empty() && bytes + size > antiburn_local::analysis::MAX_TOOL_RESULT_BYTES {
            break;
        }
        bytes += size;
        let kind: EvidenceEventKind =
            serde_json::from_value(serde_json::Value::String(row.get(15)?))?;
        let source_evidence_version = match (
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<i64>>(8)?,
            payload.as_deref(),
        ) {
            (Some(identity), Some(revision), Some(payload)) => Some(
                NativeCurrentBinding {
                    identity,
                    event_kind: kind,
                    source_format: row
                        .get::<_, Option<String>>(5)?
                        .ok_or_else(|| anyhow::anyhow!("native evidence has no format"))?,
                    revision,
                    publication_sequence: row.get(0)?,
                    payload_sha256: super::super::burn_checks::payload_sha256(payload),
                }
                .version(),
            ),
            _ => None,
        };
        let payload = payload
            .map(|json| -> Result<_> {
                Ok(match kind {
                    EvidenceEventKind::ToolResult => EvidencePublicationPayload::Native(
                        NativeEvidencePayload::ToolResult(serde_json::from_str(&json)?),
                    ),
                    EvidenceEventKind::UserMessage => EvidencePublicationPayload::Native(
                        NativeEvidencePayload::UserMessage(serde_json::from_str(&json)?),
                    ),
                    EvidenceEventKind::AssistantMessage | EvidenceEventKind::ToolCall => {
                        EvidencePublicationPayload::Native(NativeEvidencePayload::PublicMessage(
                            serde_json::from_str(&json)?,
                        ))
                    }
                    EvidenceEventKind::ResponseCycle => EvidencePublicationPayload::Native(
                        NativeEvidencePayload::ResponseCycle(serde_json::from_str(&json)?),
                    ),
                    EvidenceEventKind::SessionStart | EvidenceEventKind::SessionIdle => {
                        EvidencePublicationPayload::Host(serde_json::from_str(&json)?)
                    }
                })
            })
            .transpose()?;
        let target = match &payload {
            Some(EvidencePublicationPayload::Host(host)) => Some(host.observer.clone()),
            _ => None,
        };
        let position: Option<String> = row.get(7)?;
        events.push(EvidencePublication {
            gap_kind: serde_json::from_value(serde_json::Value::String(row.get(19)?))?,
            session_selection: selection
                .map(|json| serde_json::from_str(&json))
                .transpose()?,
            publication_sequence: row.get(0)?,
            outcome: match row.get::<_, String>(1)?.as_str() {
                "update" => ToolResultPublicationOutcome::Update,
                "removal" => ToolResultPublicationOutcome::Removal,
                "coverage_gap" => ToolResultPublicationOutcome::CoverageGap,
                "coverage_complete" => ToolResultPublicationOutcome::CoverageRestored,
                value => anyhow::bail!("unknown evidence outcome: {value}"),
            },
            session: SessionKey::new(
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ),
            source_format: row.get(5)?,
            native_identity: row.get(6)?,
            native_position: position.map(|p| serde_json::from_str(&p)).transpose()?,
            native_revision: row.get(8)?,
            payload,
            complete: row.get(10)?,
            source_generation: row.get(11)?,
            claim_fence: row.get(12)?,
            live_eligible: row.get(13)?,
            gap_reason: row.get(14)?,
            event_kind: kind,
            first_eligible_publication: row.get(16)?,
            target,
            dependencies: binding
                .map(|b| serde_json::from_str(&b))
                .transpose()?
                .unwrap_or_default(),
            source_evidence_version,
        });
    }
    Ok(events)
}

pub(crate) fn message_payload(message: &NativeMessageObservation) -> NativeEvidencePayload {
    match message {
        NativeMessageObservation::User(m) => NativeEvidencePayload::UserMessage(m.clone()),
        NativeMessageObservation::Public(m) => NativeEvidencePayload::PublicMessage(m.clone()),
        NativeMessageObservation::ToolResult(m) => NativeEvidencePayload::ToolResult(m.clone()),
    }
}

impl NativeEvidencePayload {
    fn source_format(&self) -> antiburn_local::analysis::SourceFormat {
        match self {
            Self::ToolResult(m) => m.source_format,
            Self::UserMessage(m) => m.message.source_format,
            Self::PublicMessage(m) => m.source_format,
            Self::ResponseCycle(m) => m.source_format,
        }
    }
    pub fn kind(&self) -> EvidenceEventKind {
        match self {
            Self::ToolResult(_) => EvidenceEventKind::ToolResult,
            Self::UserMessage(_) => EvidenceEventKind::UserMessage,
            Self::PublicMessage(m) => match m.payload {
                PublicMessagePayload::AssistantText { .. } => EvidenceEventKind::AssistantMessage,
                PublicMessagePayload::ToolCall { .. } => EvidenceEventKind::ToolCall,
            },
            Self::ResponseCycle(_) => EvidenceEventKind::ResponseCycle,
        }
    }
    pub fn identity(&self) -> Result<String> {
        Ok(match self {
            Self::ToolResult(m) => super::publications::native_identity(m)?,
            Self::UserMessage(m) => {
                serde_json::to_string(&("user_text", &m.message.session_id, &m.message.message_id))?
            }
            Self::PublicMessage(m) => serde_json::to_string(&(
                self.kind().name(),
                &m.session_id,
                &m.record_id,
                &m.part_id,
                &m.call_id,
            ))?,
            Self::ResponseCycle(m) => serde_json::to_string(&("turn_end", &m.session_id, &m.key))?,
        })
    }
    fn owner(&self) -> Option<&str> {
        match self {
            Self::ToolResult(m) => m.record_id.as_deref(),
            Self::UserMessage(m) => Some(&m.message.message_id),
            Self::PublicMessage(m) => Some(&m.record_id),
            Self::ResponseCycle(_) => None,
        }
    }
    pub fn position(&self) -> NativeEvidencePosition {
        match self {
            Self::ToolResult(m) => m.position,
            Self::UserMessage(m) => match m.message.position {
                antiburn_local::analysis::UserMessagePosition::FileBytes {
                    record_start,
                    record_end,
                } => NativeEvidencePosition::FileBytes {
                    record_start,
                    record_end,
                    item_index: 0,
                },
                antiburn_local::analysis::UserMessagePosition::Jsonl { record_index } => {
                    NativeEvidencePosition::Jsonl {
                        record_index,
                        item_index: 0,
                    }
                }
                antiburn_local::analysis::UserMessagePosition::OpenCodeEvent { sequence } => {
                    NativeEvidencePosition::OpenCodeEvent {
                        sequence,
                        item_index: 0,
                    }
                }
                antiburn_local::analysis::UserMessagePosition::OpenCodeV2Event { sequence } => {
                    NativeEvidencePosition::OpenCodeV2Event {
                        sequence,
                        item_index: 0,
                    }
                }
                antiburn_local::analysis::UserMessagePosition::OpenCodeSqlite { .. } => {
                    m.context.first_eligible_position
                }
            },
            Self::PublicMessage(m) => m.position,
            Self::ResponseCycle(m) => m.position,
        }
    }
    pub fn revision(&self) -> u64 {
        match self {
            Self::ToolResult(m) => m.revision,
            Self::UserMessage(m) => m.revision,
            Self::PublicMessage(m) => m.revision,
            Self::ResponseCycle(m) => m.revision,
        }
    }
    fn first_position(&self) -> NativeEvidencePosition {
        match self {
            Self::ToolResult(m) => m.context.as_ref().map_or(m.position, |c| c.result_position),
            Self::UserMessage(m) => m.context.first_eligible_position,
            Self::PublicMessage(m) => m
                .context
                .as_ref()
                .map_or(m.position, |c| c.first_eligible_position),
            Self::ResponseCycle(m) => m.context.first_eligible_position,
        }
    }
    fn session(&self) -> &str {
        match self {
            Self::ToolResult(m) => &m.session_id,
            Self::UserMessage(m) => &m.message.session_id,
            Self::PublicMessage(m) => &m.session_id,
            Self::ResponseCycle(m) => &m.session_id,
        }
    }
    fn removed(&self) -> bool {
        match self {
            Self::ToolResult(m) => m.change == ToolResultChange::Removal,
            Self::UserMessage(m) => m.message.change == UserMessageChange::Removal,
            Self::PublicMessage(m) => m.change == ToolResultChange::Removal,
            Self::ResponseCycle(m) => m.change == ToolResultChange::Removal,
        }
    }
    pub fn json(&self) -> Result<String> {
        Ok(match self {
            Self::ToolResult(m) => serde_json::to_string(m)?,
            Self::UserMessage(m) => serde_json::to_string(m)?,
            Self::PublicMessage(m) => serde_json::to_string(m)?,
            Self::ResponseCycle(m) => serde_json::to_string(m)?,
        })
    }
    fn eligible(&self) -> bool {
        match self {
            Self::ToolResult(m) => m.finality,
            Self::UserMessage(m) => m.message.change == UserMessageChange::Present,
            Self::PublicMessage(m) => m.eligible,
            Self::ResponseCycle(_) => true,
        }
    }
}

struct PublicationBudget {
    head: i64,
    count: usize,
    bytes: usize,
    count_limit: usize,
    byte_limit: usize,
}

impl PublicationBudget {
    fn apply(
        &mut self,
        tx: &Transaction<'_>,
        operation: impl FnOnce() -> Result<()>,
        effects: Option<&std::cell::Cell<super::trace_effects::InvalidationEffects>>,
    ) -> Result<bool> {
        let before = effects.map(std::cell::Cell::get);
        tx.execute_batch("SAVEPOINT native_evidence_output")?;
        operation()?;
        let (count, bytes, head): (usize, usize, i64) = tx.query_row(
            "SELECT COUNT(*), COALESCE(SUM(COALESCE(length(CAST(payload_json AS BLOB)), 0)
                + COALESCE(length(CAST(dependency_binding_json AS BLOB)), 0)
                + COALESCE(length(CAST(session_selection_json AS BLOB)), 0)), 0),
                COALESCE(MAX(publication_sequence), ?1)
             FROM native_tool_result_publication WHERE publication_sequence > ?1",
            [self.head],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if self.count + count > self.count_limit || self.bytes + bytes > self.byte_limit {
            tx.execute_batch("ROLLBACK TO native_evidence_output; RELEASE native_evidence_output")?;
            if let (Some(effects), Some(before)) = (effects, before) {
                effects.set(before);
            }
            ensure!(
                count <= self.count_limit && bytes <= self.byte_limit,
                "one native evidence output exceeds its page budget"
            );
            return Ok(false);
        }
        tx.execute_batch("RELEASE native_evidence_output")?;
        self.count += count;
        self.bytes += bytes;
        self.head = head;
        Ok(true)
    }
}

pub(crate) fn reconcile_evidence_in(
    tx: &Transaction<'_>,
    session: &SessionKey,
    generation: i64,
    fence: i64,
    batch: &NativeEvidenceBatch,
    policy: PublicationPolicy<'_>,
    now_ms: i64,
) -> Result<bool> {
    let format = serde_json::to_string(&batch.source_format)?;
    let mut unsafe_reason = None;
    for event in &batch.events {
        if let NativeEvidenceFact::AdmissionFailure(failure) = &event.fact {
            ensure!(
                failure.source_format == batch.source_format
                    && failure.session_id == session.session_id,
                "unsafe admission source mismatch"
            );
            unsafe_reason =
                Some(super::admission::reason(failure.reason).ok_or_else(|| {
                    anyhow::anyhow!("admission failure must have an unsafe reason")
                })?);
        }
    }
    if let Some(reason) = unsafe_reason {
        super::admission::invalidate_in(tx, session, reason)?.record(policy.invalidation_effects);
    }
    super::observers::rebind_source_in(tx, session, batch.source_format)?
        .record(policy.invalidation_effects);
    let journal = batch.source_format == antiburn_local::analysis::SourceFormat::OpenCodeSqliteV1;
    let mut budget = PublicationBudget {
        head: tx.query_row(
            "SELECT COALESCE(MAX(publication_sequence), 0) FROM native_tool_result_publication",
            [],
            |row| row.get(0),
        )?,
        count: 0,
        bytes: 0,
        count_limit: if journal { 40 } else { 256 },
        byte_limit: if journal {
            4 * 1024 * 1024
        } else {
            12 * 1024 * 1024
        },
    };
    ensure!(
        serde_json::to_vec(batch)?.len() <= budget.byte_limit,
        "native evidence page exceeds its byte budget"
    );
    let has_admission_failure = batch
        .events
        .iter()
        .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)));
    if !policy.suppress_coverage
        && !has_admission_failure
        && let Some(complete) = policy.history_complete
    {
        let results = batch
            .events
            .iter()
            .filter_map(|event| match &event.fact {
                NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(result)) => {
                    Some(result.clone())
                }
                _ => None,
            })
            .collect();
        if !budget.apply(
            tx,
            || {
                super::publications::publish_coverage_with_kind_in(
                    tx,
                    session,
                    generation,
                    fence,
                    (&ToolResultBatch {
                        source_format: batch.source_format,
                        results,
                        complete: false,
                    })
                        .into(),
                    complete,
                    policy.gap_reason,
                    policy.gap_kind,
                )
            },
            policy.invalidation_effects,
        )? {
            return Ok(false);
        }
    }
    for event in &batch.events {
        match &event.fact {
            NativeEvidenceFact::AdmissionFailure(failure) => {
                ensure!(
                    failure.source_format == batch.source_format
                        && failure.session_id == session.session_id,
                    "unsafe admission source mismatch"
                );
                let reason = super::admission::reason(failure.reason).ok_or_else(|| {
                    anyhow::anyhow!("admission failure must have an unsafe reason")
                })?;
                if !budget.apply(
                    tx,
                    || {
                        super::publications::publish_coverage_with_kind_in(
                            tx,
                            session,
                            generation,
                            fence,
                            (&ToolResultBatch {
                                source_format: batch.source_format,
                                results: Vec::new(),
                                complete: false,
                            })
                                .into(),
                            false,
                            Some(reason),
                            PublicationGapKind::Invalidated,
                        )
                    },
                    policy.invalidation_effects,
                )? {
                    return Ok(false);
                }
            }
            NativeEvidenceFact::Activity(activity) => {
                ensure!(
                    activity.source_format == batch.source_format
                        && activity.session_id == session.session_id,
                    "activity source mismatch"
                );
                if event.live_eligible && after_baseline(activity.position, policy.baseline) {
                    super::observers::activity_in(tx, session, activity, now_ms)?
                        .record(policy.invalidation_effects);
                }
            }
            NativeEvidenceFact::SessionSelection(selection) => {
                ensure!(
                    selection.source_format == batch.source_format
                        && selection.session_id == session.session_id,
                    "selection source mismatch"
                );
                super::observers::selection_in(tx, session, selection)?;
            }
            NativeEvidenceFact::MessageRemoval(removal) => {
                ensure!(
                    removal.source_format == batch.source_format
                        && removal.session_id == session.session_id,
                    "removal source mismatch"
                );
                let remaining = budget.count_limit.saturating_sub(budget.count);
                let mut stmt = tx.prepare("SELECT native_identity FROM native_tool_result_current
                    WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND source_format = ?4
                      AND owner_message_id = ?5 AND is_removed = 0 ORDER BY native_identity LIMIT ?6")?;
                let identities = stmt
                    .query_map(
                        params![
                            session.environment_key,
                            session.agent,
                            session.session_id,
                            format,
                            removal.message_id,
                            remaining + 1
                        ],
                        |r| r.get::<_, String>(0),
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                if identities.is_empty() {
                    let bound: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM native_tool_result_current
                        WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND source_format = ?4 AND owner_message_id = ?5)",
                        params![session.environment_key, session.agent, session.session_id, format, removal.message_id], |r| r.get(0))?;
                    if !bound
                        && !budget.apply(
                            tx,
                            || {
                                super::publications::publish_coverage_with_kind_in(
                                    tx,
                                    session,
                                    generation,
                                    fence,
                                    (&ToolResultBatch {
                                        source_format: batch.source_format,
                                        results: Vec::new(),
                                        complete: false,
                                    })
                                        .into(),
                                    false,
                                    Some("native-message-removal-unbound"),
                                    PublicationGapKind::Invalidated,
                                )
                            },
                            policy.invalidation_effects,
                        )?
                    {
                        return Ok(false);
                    }
                }
                for identity in identities {
                    if !budget.apply(
                        tx,
                        || {
                            remove_current_in(
                                tx, session, &format, &identity, removal, generation, fence,
                            )?
                            .record(policy.invalidation_effects);
                            Ok(())
                        },
                        policy.invalidation_effects,
                    )? {
                        return Ok(false);
                    }
                }
            }
            NativeEvidenceFact::Message(message) => {
                let payload = message_payload(message);
                ensure!(
                    payload.source_format() == batch.source_format
                        && payload.session() == session.session_id,
                    "native message source mismatch"
                );
                if super::admission::source_reason_in(tx, session)?.is_some() {
                    continue;
                }
                let first_position = payload.first_position();
                let live = event.live_eligible && after_baseline(first_position, policy.baseline);
                if !budget.apply(tx, || {
                if let NativeEvidencePayload::ToolResult(result) = &payload {
                    super::publications::reconcile_with_policy(
                        tx,
                        session,
                        generation,
                        fence,
                        &ToolResultBatch {
                            source_format: batch.source_format,
                            results: vec![result.clone()],
                            complete: false,
                        },
                        PublicationPolicy {
                            suppress_coverage: true,
                            live_eligible: Some(live),
                            ..policy
                        },
                    )?;
                    tx.execute("UPDATE native_tool_result_current SET owner_message_id = ?6 WHERE environment_key = ?1
                        AND agent = ?2 AND session_id = ?3 AND source_format = ?4 AND native_identity = ?5",
                        params![session.environment_key, session.agent, session.session_id, format, payload.identity()?, payload.owner()])?;
                } else {
                    upsert_in(tx, session, &format, generation, fence, payload, live)?.record(policy.invalidation_effects);
                }
                Ok(())
                }, policy.invalidation_effects)? { return Ok(false); }
            }
            NativeEvidenceFact::ResponseCycle(cycle) => {
                ensure!(
                    cycle.source_format == batch.source_format
                        && cycle.session_id == session.session_id,
                    "native cycle source mismatch"
                );
                if super::admission::source_reason_in(tx, session)?.is_some() {
                    continue;
                }
                ensure!(
                    matches!(
                        (&cycle.key, batch.source_format),
                        (
                            antiburn_local::analysis::NativeCycleKey::CodexTurn { .. },
                            antiburn_local::analysis::SourceFormat::CodexRolloutJsonl
                        ) | (
                            antiburn_local::analysis::NativeCycleKey::OpenCodeV2Execution { .. },
                            antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2
                        )
                    ),
                    "cycle key does not match its native source"
                );
                if !budget.apply(
                    tx,
                    || {
                        upsert_in(
                            tx,
                            session,
                            &format,
                            generation,
                            fence,
                            NativeEvidencePayload::ResponseCycle(cycle.clone()),
                            event.live_eligible
                                && after_baseline(
                                    cycle.context.first_eligible_position,
                                    policy.baseline,
                                ),
                        )?
                        .record(policy.invalidation_effects);
                        Ok(())
                    },
                    policy.invalidation_effects,
                )? {
                    return Ok(false);
                }
            }
        }
    }
    if !policy.suppress_coverage
        && !has_admission_failure
        && policy.history_complete.is_none()
        && !budget.apply(
            tx,
            || {
                super::publications::publish_coverage_with_kind_in(
                    tx,
                    session,
                    generation,
                    fence,
                    (&ToolResultBatch {
                        source_format: batch.source_format,
                        results: Vec::new(),
                        complete: false,
                    })
                        .into(),
                    policy.history_complete.unwrap_or(batch.history_complete),
                    policy.gap_reason,
                    policy.gap_kind,
                )
            },
            policy.invalidation_effects,
        )?
    {
        return Ok(false);
    }
    Ok(true)
}

fn after_baseline(position: NativeEvidencePosition, baseline: Option<u64>) -> bool {
    baseline.is_none_or(|baseline| match position {
        NativeEvidencePosition::FileBytes { record_end, .. } => record_end > baseline,
        NativeEvidencePosition::OpenCodeEvent { sequence, .. }
        | NativeEvidencePosition::OpenCodeV2Event { sequence, .. } => sequence > baseline,
        NativeEvidencePosition::Jsonl { .. } => false,
    })
}

fn upsert_in(
    tx: &Transaction<'_>,
    session: &SessionKey,
    format: &str,
    generation: i64,
    fence: i64,
    mut payload: NativeEvidencePayload,
    live: bool,
) -> Result<super::trace_effects::InvalidationEffects> {
    ensure!(
        payload.session() == session.session_id,
        "evidence session mismatch"
    );
    let identity = payload.identity()?;
    let previous: Option<(i64, String, bool, Option<i64>)> = tx.query_row("SELECT native_revision, payload_json, is_removed,
        first_eligible_publication FROM native_tool_result_current WHERE environment_key = ?1 AND agent = ?2
        AND session_id = ?3 AND source_format = ?4 AND native_identity = ?5",
        params![session.environment_key, session.agent, session.session_id, format, identity],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
    let revision = i64::try_from(payload.revision())?;
    if previous
        .as_ref()
        .is_some_and(|old| old.0 > revision || (old.2 && !payload.removed()))
    {
        return Ok(Default::default());
    }
    if let Some((_, old, _, Some(_))) = &previous {
        match &mut payload {
            NativeEvidencePayload::PublicMessage(m) => {
                let prior: PublicMessageObservation = serde_json::from_str(old)?;
                m.context = prior.context;
            }
            NativeEvidencePayload::UserMessage(m) => {
                let prior: UserMessageObservation = serde_json::from_str(old)?;
                m.context = prior.context;
            }
            _ => {}
        }
    }
    let encoded = payload.json()?;
    if previous
        .as_ref()
        .is_some_and(|old| old.1 == encoded && old.2 == payload.removed())
    {
        return Ok(Default::default());
    }
    let mut bindings = Vec::new();
    if let NativeEvidencePayload::ResponseCycle(cycle) = &payload {
        if cycle.messages.len() > 4096 {
            return Ok(Default::default());
        }
        for member in &cycle.messages {
            let member = message_payload(member);
            let Some(binding) = current_binding_in(tx, session, &member.identity()?)? else {
                return Ok(Default::default());
            };
            if binding.revision != i64::try_from(member.revision())?
                || binding.payload_sha256
                    != super::super::burn_checks::payload_sha256(&member.json()?)
            {
                return Ok(Default::default());
            }
            bindings.push(binding);
        }
    }
    let binding_json = serde_json::to_string(&bindings)?;
    if matches!(payload, NativeEvidencePayload::ResponseCycle(_))
        && encoded.len() + binding_json.len() > 4 * 1024 * 1024
    {
        return Ok(Default::default());
    }
    let position = serde_json::to_string(&payload.position())?;
    let first = previous.and_then(|old| old.3);
    let sequence: i64 = tx
        .query_row(
            "SELECT seq + 1 FROM sqlite_sequence WHERE name = 'native_tool_result_publication'",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(1);
    let first =
        first.or_else(|| (live && payload.eligible() && !payload.removed()).then_some(sequence));
    let selection = super::observers::frozen_selection_json_in(
        tx,
        session,
        format,
        &identity,
        payload.first_position(),
    )?;
    tx.execute("INSERT INTO native_tool_result_publication(outcome, environment_key, agent, session_id, source_format,
        native_identity, native_position_json, native_revision, payload_json, complete, source_generation, claim_fence,
        live_eligible, project_cwd, session_incarnation, event_kind, first_eligible_publication, owner_message_id, dependency_binding_json,
        session_selection_json)
        SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?11, ?12, s.cwd, s.incarnation, ?13, ?14, ?15, ?16, ?17
          FROM session s WHERE s.environment_key = ?2 AND s.agent = ?3 AND s.session_id = ?4",
        params![if payload.removed() { "removal" } else { "update" }, session.environment_key, session.agent,
            session.session_id, format, identity, position, revision, encoded, generation, fence, live,
            payload.kind().name(), first, payload.owner(), binding_json, selection])?;
    let sequence = tx.last_insert_rowid();
    tx.execute("INSERT INTO native_tool_result_current(environment_key, agent, session_id, source_format, native_identity,
        native_position_json, native_revision, payload_json, complete, is_removed, publication_sequence, source_generation,
        claim_fence, event_kind, first_eligible_publication, owner_message_id)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
        ON CONFLICT(environment_key, agent, session_id, source_format, native_identity) DO UPDATE SET
        native_position_json = excluded.native_position_json, native_revision = excluded.native_revision,
        payload_json = excluded.payload_json, is_removed = excluded.is_removed, publication_sequence = excluded.publication_sequence,
        source_generation = excluded.source_generation, claim_fence = excluded.claim_fence,
        first_eligible_publication = excluded.first_eligible_publication",
        params![session.environment_key, session.agent, session.session_id, format, identity, position, revision,
            encoded, payload.removed(), sequence, generation, fence, payload.kind().name(), first, payload.owner()])?;
    invalidate_native_identity_dependents_in(tx, session, format, &identity, payload.kind())
}

pub(crate) fn current_binding_in(
    connection: &Connection,
    session: &SessionKey,
    identity: &str,
) -> Result<Option<NativeCurrentBinding>> {
    if super::admission::source_reason_in(connection, session)?.is_some() {
        return Ok(None);
    }
    let binding = connection.query_row("SELECT c.event_kind, c.source_format, c.native_revision, c.publication_sequence, c.payload_json
        FROM native_tool_result_current c JOIN native_tool_result_publication p USING(publication_sequence)
        JOIN session s ON s.environment_key = c.environment_key AND s.agent = c.agent AND s.session_id = c.session_id
        WHERE c.environment_key = ?1 AND c.agent = ?2 AND c.session_id = ?3 AND c.native_identity = ?4 AND c.is_removed = 0
          AND p.session_incarnation = s.incarnation AND p.project_cwd IS s.cwd",
        params![session.environment_key, session.agent, session.session_id, identity], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get(2)?, r.get(3)?, r.get::<_, String>(4)?))
        }).optional()?.map(|(kind, format, revision, sequence, payload)| -> Result<_> { Ok(NativeCurrentBinding {
            identity: identity.into(), event_kind: serde_json::from_value(serde_json::Value::String(kind))?, source_format: format,
            revision, publication_sequence: sequence, payload_sha256: super::super::burn_checks::payload_sha256(&payload),
        }) }).transpose()?;
    if let Some(binding) = &binding
        && super::admission::publication_reason_in(connection, binding.publication_sequence)?
            .is_some()
    {
        return Ok(None);
    }
    Ok(binding)
}

fn remove_current_in(
    tx: &Transaction<'_>,
    session: &SessionKey,
    format: &str,
    identity: &str,
    removal: &antiburn_local::analysis::NativeMessageRemoval,
    generation: i64,
    fence: i64,
) -> Result<super::trace_effects::InvalidationEffects> {
    let position = &removal.position;
    let revision = removal.revision;
    let (kind, payload, old_revision): (String, String, u64) = tx.query_row("SELECT event_kind, payload_json, native_revision
        FROM native_tool_result_current WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
          AND source_format = ?4 AND native_identity = ?5", params![session.environment_key, session.agent, session.session_id, format, identity],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let revision = revision.max(
        old_revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("removal revision exhausted"))?,
    );
    let payload = match kind.as_str() {
        "tool_result" => {
            let mut result: ToolResultObservation = serde_json::from_str(&payload)?;
            result.change = ToolResultChange::Removal;
            result.position = *position;
            result.revision = revision;
            serde_json::to_string(&result)?
        }
        "user_message" => {
            let mut message: UserMessageObservation = serde_json::from_str(&payload)?;
            message.message.change = UserMessageChange::Removal;
            message.message.parts.clear();
            message.revision = revision;
            serde_json::to_string(&message)?
        }
        "assistant_message" | "tool_call" => {
            let mut message: PublicMessageObservation = serde_json::from_str(&payload)?;
            message.change = ToolResultChange::Removal;
            message.position = *position;
            message.revision = revision;
            serde_json::to_string(&message)?
        }
        "response_cycle" => {
            let mut cycle: ResponseCycleObservation = serde_json::from_str(&payload)?;
            cycle.change = ToolResultChange::Removal;
            cycle.position = *position;
            cycle.revision = revision;
            serde_json::to_string(&cycle)?
        }
        _ => anyhow::bail!("unknown current native evidence kind"),
    };
    tx.execute("INSERT INTO native_tool_result_publication(outcome, environment_key, agent, session_id, source_format,
        native_identity, native_position_json, native_revision, payload_json, complete, source_generation, claim_fence,
        live_eligible, project_cwd, session_incarnation, event_kind, first_eligible_publication, owner_message_id, session_selection_json)
        SELECT 'removal', c.environment_key, c.agent, c.session_id, c.source_format, c.native_identity, ?6, ?7,
        ?10, c.complete, ?8, ?9, 1, s.cwd, s.incarnation, c.event_kind, c.first_eligible_publication, c.owner_message_id, p.session_selection_json
        FROM native_tool_result_current c JOIN session s USING(environment_key, agent, session_id)
        JOIN native_tool_result_publication p USING(publication_sequence)
        WHERE c.environment_key = ?1 AND c.agent = ?2 AND c.session_id = ?3 AND c.source_format = ?4 AND c.native_identity = ?5",
        params![session.environment_key, session.agent, session.session_id, format, identity, serde_json::to_string(position)?,
            i64::try_from(revision)?, generation, fence, payload])?;
    let sequence = tx.last_insert_rowid();
    tx.execute("UPDATE native_tool_result_current SET is_removed = 1, publication_sequence = ?6, native_revision = ?7,
        native_position_json = ?8, payload_json = ?9 WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND source_format = ?4 AND native_identity = ?5",
        params![session.environment_key, session.agent, session.session_id, format, identity, sequence, i64::try_from(revision)?, serde_json::to_string(position)?, payload])?;
    invalidate_native_identity_dependents_in(
        tx,
        session,
        format,
        identity,
        serde_json::from_value(serde_json::Value::String(kind))?,
    )
}

const NATIVE_IDENTITY_DEPENDENTS_SQL: &str = "
    SELECT d.candidate_id FROM burn_check_candidate_dependency d
    JOIN burn_check_candidate c USING(candidate_id)
    WHERE d.session_key_json = ?1 AND d.source_format = ?2
      AND CASE WHEN json_valid(d.dependency_binding_json)
        THEN json_type(d.dependency_binding_json, '$.NativeCurrent') = 'object' ELSE 0 END
      AND CASE WHEN json_valid(d.dependency_binding_json)
        THEN json_extract(d.dependency_binding_json, '$.NativeCurrent.identity') END = ?3
      AND d.event_kind = ?4
      AND c.state IN ('pending', 'claimed', 'dispatching', 'answered', 'accepted')
    UNION
    SELECT d.candidate_id FROM burn_check_observation o
    JOIN burn_check_candidate_dependency d
      ON d.observation_id = o.observation_id AND d.session_key_json = o.session_key_json
    JOIN burn_check_candidate c ON c.candidate_id = d.candidate_id
      AND c.check_id = o.check_id AND c.activation_scope_key = o.activation_scope_key
    WHERE o.session_key_json = ?1 AND o.source_format = ?2 AND o.native_identity = ?3
      AND o.event_kind = ?4 AND d.dependency_binding_json IS NULL
      AND d.native_revision IS NOT NULL AND d.event_kind = 'tool_result'
      AND c.state IN ('pending', 'claimed', 'dispatching', 'answered', 'accepted')";

pub(crate) fn invalidate_native_identity_dependents_in(
    tx: &Transaction<'_>,
    session: &SessionKey,
    format: &str,
    identity: &str,
    kind: EvidenceEventKind,
) -> Result<super::trace_effects::InvalidationEffects> {
    let mut stmt = tx.prepare(NATIVE_IDENTITY_DEPENDENTS_SQL)?;
    let ids = stmt
        .query_map(
            params![session.to_identity_json(), format, identity, kind.name()],
            |r| r.get::<_, String>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    invalidate_candidates_in(tx, ids)
}

pub(crate) fn invalidate_native_dependents_in(
    tx: &Transaction<'_>,
    session: &SessionKey,
) -> Result<super::trace_effects::InvalidationEffects> {
    let mut stmt = tx.prepare(
        "SELECT DISTINCT d.candidate_id FROM burn_check_candidate_dependency d
        JOIN burn_check_candidate c USING(candidate_id) WHERE d.session_key_json = ?1
        AND (d.dependency_binding_json IS NOT NULL
          OR (d.native_revision IS NOT NULL AND d.event_kind = 'tool_result'))
        AND c.state IN ('pending', 'claimed', 'dispatching', 'answered', 'accepted')",
    )?;
    let ids = stmt
        .query_map([session.to_identity_json()], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    invalidate_candidates_in(tx, ids)
}

fn invalidate_candidates_in(
    tx: &Transaction<'_>,
    ids: Vec<String>,
) -> Result<super::trace_effects::InvalidationEffects> {
    let mut effects = super::trace_effects::InvalidationEffects::default();
    for id in ids {
        if !publication_dependencies_current_in(tx, &id)? {
            effects.unfinished_stale += tx.execute(
                "UPDATE burn_check_candidate SET state = 'stale' WHERE candidate_id = ?1
            AND state IN ('pending', 'claimed', 'dispatching', 'answered')",
                [&id],
            )?;
            effects.accepted_evidence_revoked += tx.execute("UPDATE burn_check_assessment_history SET evidence_valid = 0 WHERE candidate_id = ?1 AND evidence_valid = 1", [&id])?;
            effects.current_status_revoked += tx.execute(
                "UPDATE burn_check_current_status SET evidence_valid = 0 WHERE evidence_valid = 1 AND assessment_id IN
            (SELECT assessment_id FROM burn_check_assessment_history WHERE candidate_id = ?1)",
                [&id],
            )?;
        }
    }
    Ok(effects)
}

fn publication_dependencies_current_in(connection: &Connection, candidate: &str) -> Result<bool> {
    use super::observers::DependencyBinding;

    let mut statement = connection.prepare(
        "SELECT session_key_json, native_revision, event_kind, dependency_binding_json
         FROM burn_check_candidate_dependency WHERE candidate_id = ?1",
    )?;
    let rows = statement
        .query_map([candidate], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut has_native = false;
    for (session, revision, kind, json) in rows {
        let Some(json) = json else {
            has_native |= revision.is_some() && kind == "tool_result";
            continue;
        };
        // Saved binding decode failures invalidate this candidate, not its source transaction.
        let Ok(binding) = serde_json::from_str::<DependencyBinding>(&json) else {
            return Ok(false);
        };
        if matches!(binding, DependencyBinding::NativeCurrent(_)) {
            if serde_json::from_str::<(String, String, String)>(&session).is_err() {
                return Ok(false);
            }
            has_native = true;
        }
    }
    // Native continuity does not revoke valid host-only or activity-only history.
    if !has_native {
        return Ok(true);
    }
    let (session, scope): (String, String) = connection.query_row(
        "SELECT session_key_json, activation_scope_key FROM burn_check_candidate
         WHERE candidate_id = ?1",
        [candidate],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if serde_json::from_str::<(String, String, String)>(&session).is_err()
        || serde_json::from_str::<serde_json::Value>(&scope).is_err()
    {
        return Ok(false);
    }
    super::observers::typed_dependencies_current_in(connection, candidate)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod invalidation_tests {
    use super::*;

    #[test]
    fn native_identity_invalidation_uses_indexed_typed_and_observation_lookups() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        let connection = store.lock();
        let mut statement = connection
            .prepare(&format!(
                "EXPLAIN QUERY PLAN {NATIVE_IDENTITY_DEPENDENTS_SQL}"
            ))
            .unwrap();
        let plan = statement
            .query_map(
                params!["session", "format", "identity", "tool_result"],
                |row| row.get::<_, String>(3),
            )
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        for (index, identity) in [
            (
                "burn_check_candidate_dependency_native_identity",
                "<expr>=?",
            ),
            (
                "burn_check_observation_native_identity",
                "native_identity=?",
            ),
        ] {
            assert!(
                plan.iter().any(|step| {
                    step.contains(index)
                        && step.contains("session_key_json=?")
                        && step.contains("source_format=?")
                        && step.contains(identity)
                        && step.contains("event_kind=?")
                }),
                "{plan:?}"
            );
        }
        assert!(
            plan.iter().any(|step| {
                step.contains("burn_check_candidate_dependency_observation")
                    && step.contains("observation_id=?")
                    && step.contains("session_key_json=?")
            }),
            "{plan:?}"
        );
    }
}
