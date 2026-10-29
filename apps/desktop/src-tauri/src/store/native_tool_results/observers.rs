//! Attachment observation uses source receipts, not UI activity or source timestamps.

use antiburn_checks::context::ObservedAttribution;
use antiburn_checks::domain::{ActivationId, BranchId, SessionStartReason};
use antiburn_checks::source::PublishedContext;
use antiburn_local::analysis::{
    NativeActivityObservation, NativeSessionSelectionObservation, ToolResultPosition,
};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use super::evidence::{
    HostSessionObservation, HostSessionPayload, NativeCurrentBinding, SessionObserverKey,
};
use crate::store::{SessionKey, Store};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum DependencyBinding {
    NativeCurrent(NativeCurrentBinding),
    HostObservation {
        observer: SessionObserverKey,
    },
    SessionActivity {
        observer: SessionObserverKey,
        activity_revision: u64,
    },
}

#[derive(Serialize, Deserialize)]
struct SourceSelection {
    incarnation: i64,
    source_label: String,
    cwd: String,
    position: ToolResultPosition,
    selection: ObservedAttribution,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture_binding: Option<(String, i64)>,
}

fn capture_binding_in(
    connection: &Connection,
    session: &SessionKey,
) -> Result<Option<(String, i64)>> {
    Ok(connection
        .query_row(
            "SELECT capture_epoch, claim_fence FROM opencode_v2_capture
         WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![session.environment_key, session.agent, session.session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

fn current_selection_in(
    connection: &Connection,
    session: &SessionKey,
) -> Result<Option<ObservedAttribution>> {
    let binding: (i64, String, Option<String>) = connection.query_row(
        "SELECT incarnation, source_label, cwd FROM session
        WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![session.environment_key, session.agent, session.session_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    for table in [
        "native_file_delivery",
        "opencode_journal_delivery",
        "opencode_v2_capture",
    ] {
        let json: Option<String> = connection
            .query_row(
                &format!(
                    "SELECT source_selection_json FROM {table}
            WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3"
                ),
                params![session.environment_key, session.agent, session.session_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if let Some(json) = json {
            let selected: SourceSelection = serde_json::from_str(&json)?;
            if table == "opencode_v2_capture"
                && (selected.capture_binding.is_none()
                    || selected.capture_binding != capture_binding_in(connection, session)?)
            {
                continue;
            }
            if (
                selected.incarnation,
                selected.source_label,
                Some(selected.cwd),
            ) == binding
            {
                return Ok(Some(selected.selection));
            }
        }
    }
    Ok(None)
}

pub(crate) fn frozen_selection_json_in(
    connection: &Connection,
    session: &SessionKey,
    format: &str,
    identity: &str,
    position: ToolResultPosition,
) -> Result<Option<String>> {
    let previous: Option<Option<String>> = connection
        .query_row(
            "SELECT p.session_selection_json FROM native_tool_result_current c
             JOIN native_tool_result_publication p USING(publication_sequence)
             JOIN session s ON s.environment_key = c.environment_key AND s.agent = c.agent
               AND s.session_id = c.session_id
             WHERE c.environment_key = ?1 AND c.agent = ?2 AND c.session_id = ?3
               AND c.source_format = ?4 AND c.native_identity = ?5
               AND (p.session_incarnation IS NULL OR
                 (p.session_incarnation = s.incarnation AND p.project_cwd IS s.cwd))",
            params![
                session.environment_key,
                session.agent,
                session.session_id,
                format,
                identity
            ],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(selection) = previous {
        // A correction retains unknown selection as well as known selection.
        return Ok(selection);
    }
    let format: antiburn_local::analysis::SourceFormat = serde_json::from_str(format)?;
    let table = match format {
        antiburn_local::analysis::SourceFormat::OpenCodeSqliteV1 => "opencode_journal_delivery",
        antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2 => "opencode_v2_capture",
        antiburn_local::analysis::SourceFormat::ClaudeJsonl
        | antiburn_local::analysis::SourceFormat::CodexRolloutJsonl
        | antiburn_local::analysis::SourceFormat::PiV3Jsonl => "native_file_delivery",
        _ => return Ok(None),
    };
    let json: Option<String> = connection
        .query_row(
            &format!(
                "SELECT d.source_selection_json FROM {table} d
            JOIN session s USING(environment_key, agent, session_id)
            WHERE d.environment_key = ?1 AND d.agent = ?2 AND d.session_id = ?3"
            ),
            params![session.environment_key, session.agent, session.session_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let Some(json) = json else { return Ok(None) };
    let selected: SourceSelection = serde_json::from_str(&json)?;
    if table == "opencode_v2_capture"
        && (selected.capture_binding.is_none()
            || selected.capture_binding != capture_binding_in(connection, session)?)
    {
        return Ok(None);
    }
    let binding: (i64, String, Option<String>) = connection.query_row(
        "SELECT incarnation, source_label, cwd FROM session
         WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![session.environment_key, session.agent, session.session_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if (
        selected.incarnation,
        selected.source_label,
        Some(selected.cwd),
    ) != binding
        || order(selected.position) > order(position)
    {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string(&selected.selection)?))
}

pub(crate) fn typed_dependencies_current_in(
    connection: &Connection,
    candidate: &str,
) -> Result<bool> {
    if !super::admission::candidate_current_in(connection, candidate)? {
        return Ok(false);
    }
    let mut statement = connection.prepare("SELECT session_key_json, dependency_binding_json
        FROM burn_check_candidate_dependency WHERE candidate_id = ?1 AND dependency_binding_json IS NOT NULL")?;
    let rows = statement
        .query_map([candidate], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (session, json) in rows {
        match serde_json::from_str::<DependencyBinding>(&json)? {
            DependencyBinding::NativeCurrent(binding) => {
                let (environment, agent, session_id): (String, String, String) =
                    serde_json::from_str(&session)?;
                if super::evidence::current_binding_in(
                    connection,
                    &SessionKey::new(environment, agent, session_id),
                    &binding.identity,
                )?
                .as_ref()
                    != Some(&binding)
                {
                    return Ok(false);
                }
            }
            DependencyBinding::HostObservation { observer } => {
                if !observer_current_in(connection, &observer, None)? {
                    return Ok(false);
                }
            }
            DependencyBinding::SessionActivity {
                observer,
                activity_revision,
            } => {
                if !observer_current_in(connection, &observer, Some(activity_revision))? {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn observer_current_in(
    connection: &Connection,
    observer: &SessionObserverKey,
    activity_revision: Option<u64>,
) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM burn_check_session_attachment t
         JOIN burn_check_activation a USING(check_id, activation_scope_key)
         JOIN session s ON json_array(s.environment_key, s.agent, s.session_id) = t.branch_key
         WHERE t.check_id = ?1 AND t.activation_scope_key = ?2 AND t.generation = ?3
           AND t.branch_key = ?4 AND t.incarnation = ?5 AND t.observer_epoch = ?6
           AND (?7 IS NULL OR (t.activity_revision = ?7 AND t.continuity_eligible = 1))
           AND a.generation = t.generation AND a.enabled = 1 AND s.incarnation = t.incarnation
           AND s.source_label = t.source_label
           AND s.cwd = json_extract(t.activation_scope_key, '$.session_cwd'))",
        params![
            observer.activation.definition_id,
            crate::check_store::StoreCheckStore::activation_scope_key(&observer.activation.scope),
            observer.activation.generation.0,
            observer.branch.0,
            observer.incarnation,
            observer.attachment_epoch,
            activity_revision
        ],
        |row| row.get(0),
    )?)
}

pub(crate) fn attach_in(
    tx: &Transaction<'_>,
    activation: &ActivationId,
    session: &SessionKey,
    incarnation: i64,
    receipt: &str,
    now_ms: i64,
) -> Result<()> {
    let scope = crate::check_store::StoreCheckStore::activation_scope_key(&activation.scope);
    let branch = session.to_identity_json();
    let row: Option<(Option<String>, String)> = tx.query_row("SELECT t.observer_epoch, s.source_label
        FROM burn_check_session_attachment t JOIN session s ON s.environment_key = ?5 AND s.agent = ?6 AND s.session_id = ?7
        WHERE t.check_id = ?1 AND t.activation_scope_key = ?2 AND t.generation = ?3 AND t.branch_key = ?4
          AND t.incarnation = s.incarnation", params![activation.definition_id, scope, activation.generation.0, branch,
            session.environment_key, session.agent, session.session_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let Some((None, source_label)) = row else {
        return Ok(());
    };
    let format = if session.agent == "opencode" {
        if source_label.ends_with(":open_code_v2_sqlite") {
            antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2
        } else {
            antiburn_local::analysis::SourceFormat::OpenCodeSqliteV1
        }
    } else {
        super::source_delivery::native_file::source_format(&session.agent)
            .ok_or_else(|| anyhow::anyhow!("unsupported observer source"))?
    };
    let epoch: String = tx.query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))?;
    tx.execute("UPDATE burn_check_session_attachment SET observer_epoch = ?5, source_label = ?6, source_format = ?7,
        source_receipt = ?8, observation_started_at_ms = ?9, idle_deadline_ms = ?9 + 120000, continuity_eligible = 1
        WHERE check_id = ?1 AND activation_scope_key = ?2 AND generation = ?3 AND branch_key = ?4 AND observer_epoch IS NULL",
        params![activation.definition_id, scope, activation.generation.0, branch, epoch, source_label,
            serde_json::to_string(&format)?, receipt, now_ms])?;
    let observer = SessionObserverKey {
        activation: activation.clone(),
        branch: BranchId(branch.clone()),
        incarnation,
        attachment_epoch: epoch,
    };
    let selection = current_selection_in(tx, session)?;
    tx.execute(
        "UPDATE burn_check_session_attachment SET session_selection_json = ?5
        WHERE check_id = ?1 AND activation_scope_key = ?2 AND generation = ?3 AND branch_key = ?4",
        params![
            activation.definition_id,
            scope,
            activation.generation.0,
            branch,
            selection.as_ref().map(serde_json::to_string).transpose()?
        ],
    )?;
    let publication = publish_host_in(
        tx,
        session,
        &serde_json::to_string(&format)?,
        &HostSessionObservation {
            observer,
            observed_at_ms: now_ms,
            attribution: selection.clone().unwrap_or_default(),
            context: host_context(session, selection),
            payload: HostSessionPayload::Start {
                reason: SessionStartReason::Attached,
            },
        },
    )?;
    tx.execute(
        "UPDATE burn_check_session_attachment SET start_publication = ?5
        WHERE check_id = ?1 AND activation_scope_key = ?2 AND generation = ?3 AND branch_key = ?4",
        params![
            activation.definition_id,
            scope,
            activation.generation.0,
            branch,
            publication
        ],
    )?;
    Ok(())
}

fn host_context(session: &SessionKey, selection: Option<ObservedAttribution>) -> PublishedContext {
    PublishedContext {
        requests: Vec::new(),
        session_id: session.session_id.clone(),
        agent_name: session.agent.clone(),
        project_root: None,
        version: "attachment-observer-v1".into(),
        main_session: true,
        complete_through_event: false,
        inventories: Default::default(),
        session_selection: selection,
    }
}

fn publish_host_in(
    tx: &Transaction<'_>,
    session: &SessionKey,
    format: &str,
    host: &HostSessionObservation,
) -> Result<i64> {
    let observer = &host.observer;
    let scope =
        crate::check_store::StoreCheckStore::activation_scope_key(&observer.activation.scope);
    let kind = match host.payload {
        HostSessionPayload::Start { .. } => "session_start",
        HostSessionPayload::Idle { .. } => "session_idle",
    };
    let inserted = tx.execute("INSERT INTO native_tool_result_publication(outcome, environment_key, agent, session_id,
        source_format, payload_json, complete, source_generation, claim_fence, live_eligible,
        project_cwd, session_incarnation, event_kind, target_check_id, target_scope_key, target_generation,
        target_branch_key, target_incarnation, target_attachment_epoch)
        SELECT 'update', ?1, ?2, ?3, ?4, ?5, 1, s.source_generation, 0, 1, s.cwd, s.incarnation,
            ?6, ?7, ?8, ?9, ?10, ?11, ?12 FROM session s
        WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3 AND s.incarnation = ?11",
        params![session.environment_key, session.agent, session.session_id, format, serde_json::to_string(host)?, kind,
            observer.activation.definition_id, scope, observer.activation.generation.0, observer.branch.0, observer.incarnation, observer.attachment_epoch])?;
    ensure!(inserted == 1, "observer source changed");
    Ok(tx.last_insert_rowid())
}

fn order(position: ToolResultPosition) -> (u8, u64, u64, u32) {
    match position {
        ToolResultPosition::FileBytes {
            record_start,
            record_end,
            item_index,
        } => (0, record_start, record_end, item_index),
        ToolResultPosition::Jsonl {
            record_index,
            item_index,
        } => (1, record_index, 0, item_index),
        ToolResultPosition::OpenCodeEvent {
            sequence,
            item_index,
        } => (2, sequence, 0, item_index),
        ToolResultPosition::OpenCodeV2Event {
            sequence,
            item_index,
        } => (3, sequence, 0, item_index),
    }
}

pub(crate) fn activity_in(
    tx: &Transaction<'_>,
    session: &SessionKey,
    activity: &NativeActivityObservation,
    now_ms: i64,
) -> Result<super::trace_effects::InvalidationEffects> {
    let mut effects = super::trace_effects::InvalidationEffects::default();
    ensure!(
        !activity.token.is_empty() && activity.token.len() <= 4096,
        "invalid activity token"
    );
    let mut stmt = tx.prepare("SELECT t.check_id, t.activation_scope_key, t.generation, t.observer_epoch, t.last_activity_token, t.last_activity_order_json
        FROM burn_check_session_attachment t JOIN session s ON json_array(s.environment_key, s.agent, s.session_id) = t.branch_key
        JOIN burn_check_activation a USING(check_id, activation_scope_key)
        WHERE t.branch_key = ?1 AND t.incarnation = s.incarnation AND t.source_label = s.source_label
          AND s.cwd = json_extract(t.activation_scope_key, '$.session_cwd') AND t.generation = a.generation AND a.enabled = 1
          AND t.observer_epoch IS NOT NULL")?;
    let rows = stmt
        .query_map([session.to_identity_json()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (check, scope, generation, epoch, token, previous) in rows {
        let previous = previous
            .map(|p| serde_json::from_str::<ToolResultPosition>(&p))
            .transpose()?;
        if token.as_deref() == Some(&activity.token)
            || previous.is_some_and(|p| order(p) >= order(activity.position))
        {
            continue;
        }
        tx.execute("UPDATE burn_check_session_attachment SET last_activity_token = ?5, last_activity_order_json = ?6,
            last_activity_at_ms = ?7, activity_revision = activity_revision + 1, idle_deadline_ms = ?8 + 120000,
            idle_emitted_revision = NULL, idle_publication = NULL WHERE check_id = ?1 AND activation_scope_key = ?2
            AND generation = ?3 AND observer_epoch = ?4",
            params![check, scope, generation, epoch, activity.token, serde_json::to_string(&activity.position)?, activity.native_at_ms, now_ms])?;
        effects.unfinished_stale += tx.execute("UPDATE burn_check_candidate SET state = 'stale' WHERE check_id = ?1 AND activation_scope_key = ?2 AND generation = ?3
            AND branch_key = ?4 AND state IN ('pending', 'claimed', 'dispatching', 'answered')
            AND EXISTS(SELECT 1 FROM burn_check_candidate_dependency d WHERE d.candidate_id = burn_check_candidate.candidate_id
              AND CASE WHEN json_valid(d.dependency_binding_json)
                THEN json_type(d.dependency_binding_json, '$.SessionActivity') = 'object' ELSE 0 END)",
            params![check, scope, generation, session.to_identity_json()])?;
    }
    Ok(effects)
}

pub(crate) fn rebind_source_in(
    tx: &Transaction<'_>,
    session: &SessionKey,
    format: antiburn_local::analysis::SourceFormat,
) -> Result<super::trace_effects::InvalidationEffects> {
    let mut effects = super::trace_effects::InvalidationEffects::default();
    let mut statement = tx.prepare("SELECT t.check_id, t.activation_scope_key, t.generation, s.source_label
        FROM burn_check_session_attachment t JOIN burn_check_activation a USING(check_id, activation_scope_key)
        JOIN session s ON json_array(s.environment_key, s.agent, s.session_id) = t.branch_key
        WHERE t.branch_key = ?1 AND t.incarnation = s.incarnation AND t.observer_epoch IS NOT NULL
          AND t.generation = a.generation AND a.enabled = 1 AND s.cwd = json_extract(t.activation_scope_key, '$.session_cwd')
          AND (t.source_label IS NOT s.source_label OR t.source_format IS NOT ?2)")?;
    let format = serde_json::to_string(&format)?;
    let rows = statement
        .query_map(params![session.to_identity_json(), format], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for (check, scope, generation, source_label) in rows {
        tx.execute("UPDATE burn_check_session_attachment SET source_label = ?5, source_format = ?6,
            source_receipt = NULL, last_activity_token = NULL, last_activity_order_json = NULL, last_activity_at_ms = NULL,
            session_selection_json = NULL, activity_revision = activity_revision + 1, continuity_eligible = 0,
            observation_started_at_ms = NULL, idle_deadline_ms = NULL, idle_emitted_revision = NULL, idle_publication = NULL
            WHERE check_id = ?1 AND activation_scope_key = ?2 AND generation = ?3 AND branch_key = ?4",
            params![check, scope, generation, session.to_identity_json(), source_label, format])?;
        effects.unfinished_stale += tx.execute("UPDATE burn_check_candidate SET state = 'stale' WHERE check_id = ?1 AND activation_scope_key = ?2
            AND generation = ?3 AND branch_key = ?4 AND state IN ('pending', 'claimed', 'dispatching', 'answered')
            AND EXISTS(SELECT 1 FROM burn_check_candidate_dependency d WHERE d.candidate_id = burn_check_candidate.candidate_id
              AND CASE WHEN json_valid(d.dependency_binding_json)
                THEN json_type(d.dependency_binding_json, '$.SessionActivity') = 'object' ELSE 0 END)",
            params![check, scope, generation, session.to_identity_json()])?;
    }
    Ok(effects)
}

pub(crate) fn selection_in(
    tx: &Transaction<'_>,
    session: &SessionKey,
    selection: &NativeSessionSelectionObservation,
) -> Result<()> {
    let attribution = ObservedAttribution {
        model: selection.selection.model_key(),
        fast_mode: selection.selection.fast_mode,
    };
    let (incarnation, source_label, cwd): (i64, String, String) = tx.query_row(
        "SELECT incarnation, source_label, cwd FROM session
        WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![session.environment_key, session.agent, session.session_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let state = SourceSelection {
        incarnation,
        source_label,
        cwd,
        position: selection.position,
        selection: attribution.clone(),
        capture_binding: if selection.source_format
            == antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2
        {
            capture_binding_in(tx, session)?
        } else {
            None
        },
    };
    for table in [
        "native_file_delivery",
        "opencode_journal_delivery",
        "opencode_v2_capture",
    ] {
        let previous: Option<String> = tx
            .query_row(
                &format!(
                    "SELECT source_selection_json FROM {table}
            WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3"
                ),
                params![session.environment_key, session.agent, session.session_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if let Some(previous) = previous {
            let previous: SourceSelection = serde_json::from_str(&previous)?;
            if previous.incarnation == state.incarnation
                && previous.source_label == state.source_label
                && previous.cwd == state.cwd
                && previous.capture_binding == state.capture_binding
                && order(previous.position) >= order(state.position)
            {
                return Ok(());
            }
        }
        tx.execute(&format!("UPDATE {table} SET source_selection_json = ?4 WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3"),
            params![session.environment_key, session.agent, session.session_id, serde_json::to_string(&state)?])?;
    }
    tx.execute("UPDATE burn_check_session_attachment SET session_selection_json = ?2 WHERE branch_key = ?1
        AND incarnation = (SELECT incarnation FROM session WHERE environment_key = ?3 AND agent = ?4 AND session_id = ?5)",
        params![session.to_identity_json(), serde_json::to_string(&attribution)?, session.environment_key, session.agent, session.session_id])?;
    Ok(())
}

pub(crate) fn break_continuity_in(
    tx: &Transaction<'_>,
    session: &SessionKey,
) -> Result<super::trace_effects::InvalidationEffects> {
    let mut effects = super::trace_effects::InvalidationEffects::default();
    tx.execute(
        "UPDATE burn_check_session_attachment SET session_selection_json = NULL
        WHERE branch_key = ?1 AND source_format = '\"open_code_v2_sqlite\"'",
        [session.to_identity_json()],
    )?;
    tx.execute("UPDATE burn_check_session_attachment SET continuity_eligible = 0, idle_deadline_ms = NULL,
        activity_revision = activity_revision + 1, idle_emitted_revision = NULL WHERE branch_key = ?1 AND continuity_eligible = 1",
        [session.to_identity_json()])?;
    effects.unfinished_stale += tx.execute("UPDATE burn_check_candidate SET state = 'stale' WHERE branch_key = ?1
        AND state IN ('pending', 'claimed', 'dispatching', 'answered') AND EXISTS(
          SELECT 1 FROM burn_check_candidate_dependency d WHERE d.candidate_id = burn_check_candidate.candidate_id
            AND CASE WHEN json_valid(d.dependency_binding_json)
              THEN json_type(d.dependency_binding_json, '$.SessionActivity') = 'object' ELSE 0 END)", [session.to_identity_json()])?;
    effects += super::evidence::invalidate_native_dependents_in(tx, session)?;
    Ok(effects)
}

pub(crate) fn frontier_in(
    tx: &Transaction<'_>,
    session: &SessionKey,
    receipt: &str,
    usable: bool,
    now_ms: i64,
) -> Result<()> {
    if !usable {
        return Ok(());
    }
    tx.execute("UPDATE burn_check_session_attachment SET source_receipt = ?2,
        idle_deadline_ms = CASE WHEN continuity_eligible = 0 THEN ?3 + 120000 ELSE idle_deadline_ms END,
        observation_started_at_ms = CASE WHEN continuity_eligible = 0 THEN ?3 ELSE observation_started_at_ms END,
        continuity_eligible = 1 WHERE branch_key = ?1 AND observer_epoch IS NOT NULL
        AND incarnation = (SELECT incarnation FROM session WHERE environment_key = ?4 AND agent = ?5 AND session_id = ?6)
        AND source_label = (SELECT source_label FROM session WHERE environment_key = ?4 AND agent = ?5 AND session_id = ?6)
        AND json_extract(activation_scope_key, '$.session_cwd') =
            (SELECT cwd FROM session WHERE environment_key = ?4 AND agent = ?5 AND session_id = ?6)",
        params![session.to_identity_json(), receipt, now_ms, session.environment_key, session.agent, session.session_id])?;
    let mut stmt = tx.prepare("SELECT t.check_id, t.activation_scope_key, t.generation, t.incarnation, t.observer_epoch,
        t.activity_revision, t.last_activity_at_ms, t.source_format, t.session_selection_json
        FROM burn_check_session_attachment t JOIN burn_check_activation a USING(check_id, activation_scope_key)
        JOIN session s ON json_array(s.environment_key, s.agent, s.session_id) = t.branch_key
        WHERE t.branch_key = ?1 AND t.source_receipt = ?2 AND t.continuity_eligible = 1 AND t.idle_deadline_ms <= ?3
          AND t.idle_emitted_revision IS NULL AND t.generation = a.generation AND a.enabled = 1
          AND t.incarnation = s.incarnation AND t.source_label = s.source_label
          AND s.cwd = json_extract(t.activation_scope_key, '$.session_cwd') LIMIT 128")?;
    let rows = stmt
        .query_map(params![session.to_identity_json(), receipt, now_ms], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, u64>(5)?,
                r.get::<_, Option<i64>>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (
        check,
        scope,
        generation,
        incarnation,
        epoch,
        revision,
        last_activity,
        format,
        selection,
    ) in rows
    {
        let selection = selection
            .map(|s| serde_json::from_str::<ObservedAttribution>(&s))
            .transpose()?;
        let observer = SessionObserverKey {
            activation: ActivationId {
                definition_id: check.clone(),
                scope: serde_json::from_str(&scope)?,
                generation: antiburn_checks::domain::Generation(generation),
            },
            branch: BranchId(session.to_identity_json()),
            incarnation,
            attachment_epoch: epoch.clone(),
        };
        let publication = publish_host_in(
            tx,
            session,
            &format,
            &HostSessionObservation {
                observer,
                observed_at_ms: now_ms,
                attribution: selection.clone().unwrap_or_default(),
                context: host_context(session, selection),
                payload: HostSessionPayload::Idle {
                    activity_revision: revision,
                    idle_seconds: 120,
                    last_activity_at_ms: last_activity,
                },
            },
        )?;
        tx.execute("UPDATE burn_check_session_attachment SET idle_emitted_revision = ?5, idle_publication = ?6
            WHERE check_id = ?1 AND activation_scope_key = ?2 AND generation = ?3 AND observer_epoch = ?4",
            params![check, scope, generation, epoch, revision, publication])?;
    }
    Ok(())
}

impl Store {
    pub(crate) fn needs_opencode_v2_idle_frontier(&self, now_ms: i64) -> Result<bool> {
        Ok(self.lock().query_row("SELECT EXISTS(SELECT 1 FROM burn_check_session_attachment t
            JOIN burn_check_activation a USING(check_id, activation_scope_key)
            WHERE t.source_format = '\"open_code_v2_sqlite\"' AND t.observer_epoch IS NOT NULL
              AND EXISTS(SELECT 1 FROM session s WHERE json_array(s.environment_key, s.agent, s.session_id) = t.branch_key
                AND s.incarnation = t.incarnation AND s.source_label = t.source_label
                AND s.cwd = json_extract(t.activation_scope_key, '$.session_cwd'))
              AND t.generation = a.generation AND a.enabled = 1
              AND (t.continuity_eligible = 0 OR (t.idle_deadline_ms <= ?1 AND t.idle_emitted_revision IS NULL)))",
            [now_ms], |r| r.get(0))?)
    }

    pub(crate) fn initialize_session_observers(
        &self,
        activation: &ActivationId,
        now_ms: i64,
    ) -> Result<usize> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let scope = crate::check_store::StoreCheckStore::activation_scope_key(&activation.scope);
        let mut stmt = tx.prepare("SELECT s.environment_key, s.agent, s.session_id, s.incarnation,
            CASE WHEN v.capture_epoch IS NOT NULL THEN json_array(v.capture_epoch, v.claim_fence, v.last_sequence)
              WHEN s.agent = 'opencode' THEN j.cursor_json ELSE f.cursor_json END
            FROM burn_check_session_attachment t JOIN session s
              ON t.branch_key = json_array(s.environment_key, s.agent, s.session_id) AND t.incarnation = s.incarnation
            JOIN burn_check_activation a USING(check_id, activation_scope_key)
            LEFT JOIN native_file_delivery f USING(environment_key, agent, session_id)
            LEFT JOIN opencode_journal_delivery j USING(environment_key, agent, session_id)
            LEFT JOIN opencode_v2_capture v USING(environment_key, agent, session_id)
            WHERE t.check_id = ?1 AND t.activation_scope_key = ?2 AND t.generation = ?3 AND a.generation = t.generation
              AND a.enabled = 1 AND t.observer_epoch IS NULL AND s.cwd = ?4 AND s.environment_key = ?5 AND s.wsl_distro IS NULL
              AND NOT EXISTS(SELECT 1 FROM session_relation r WHERE r.environment_key = s.environment_key AND r.agent = s.agent
                AND ((r.session_id = s.session_id AND r.kind = 'forkParent') OR (r.related_id = s.session_id AND r.kind = 'subagent')))
              AND ((f.incarnation = s.incarnation AND f.source_label = s.source_label AND f.cwd = s.cwd
                   AND f.parser_revision = ?6 AND f.pending = 0 AND f.blocked = 0 AND f.last_error IS NULL AND f.cursor_json IS NOT NULL)
                OR (j.incarnation = s.incarnation AND j.source_label = s.source_label AND j.cwd = s.cwd
                  AND j.parser_revision = ?6 AND j.pending = 0 AND j.blocked = 0 AND j.cursor_json IS NOT NULL)
                OR (v.incarnation = s.incarnation AND v.source_label = s.source_label AND v.cwd = s.cwd
                  AND v.capture_epoch <> '' AND v.lease_expires_at_epoch > ?7)) LIMIT 128")?;
        let rows = stmt
            .query_map(
                params![
                    activation.definition_id,
                    scope,
                    activation.generation.0,
                    activation.scope.session_cwd,
                    activation.scope.environment,
                    antiburn_local::analysis::PARSER_REVISION,
                    now_ms / 1000
                ],
                |r| {
                    Ok((
                        SessionKey::new(
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ),
                        r.get::<_, i64>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for (session, incarnation, receipt) in &rows {
            attach_in(&tx, activation, session, *incarnation, receipt, now_ms)?;
        }
        tx.commit()?;
        Ok(rows.len())
    }

    pub(crate) fn next_session_idle_deadline(&self) -> Result<Option<i64>> {
        Ok(self.lock().query_row("SELECT MIN(t.idle_deadline_ms) FROM burn_check_session_attachment t
            JOIN burn_check_activation a USING(check_id, activation_scope_key)
            WHERE t.observer_epoch IS NOT NULL AND t.continuity_eligible = 1 AND t.idle_emitted_revision IS NULL
              AND EXISTS(SELECT 1 FROM session s WHERE json_array(s.environment_key, s.agent, s.session_id) = t.branch_key
                AND s.incarnation = t.incarnation AND s.source_label = t.source_label
                AND s.cwd = json_extract(t.activation_scope_key, '$.session_cwd') AND s.wsl_distro IS NULL
                AND NOT EXISTS(SELECT 1 FROM session_relation r WHERE r.environment_key = s.environment_key AND r.agent = s.agent
                  AND ((r.session_id = s.session_id AND r.kind = 'forkParent') OR (r.related_id = s.session_id AND r.kind = 'subagent'))))
              AND t.generation = a.generation AND a.enabled = 1", [], |r| r.get(0))?)
    }

    pub(crate) fn request_due_session_idle_catchup(&self, now_ms: i64) -> Result<usize> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut stmt = tx.prepare("SELECT DISTINCT t.branch_key FROM burn_check_session_attachment t
            JOIN burn_check_activation a USING(check_id, activation_scope_key)
            WHERE t.observer_epoch IS NOT NULL AND t.continuity_eligible = 1 AND t.idle_emitted_revision IS NULL
              AND EXISTS(SELECT 1 FROM session s WHERE json_array(s.environment_key, s.agent, s.session_id) = t.branch_key
                AND s.incarnation = t.incarnation AND s.source_label = t.source_label
                AND s.cwd = json_extract(t.activation_scope_key, '$.session_cwd') AND s.wsl_distro IS NULL
                AND NOT EXISTS(SELECT 1 FROM session_relation r WHERE r.environment_key = s.environment_key AND r.agent = s.agent
                  AND ((r.session_id = s.session_id AND r.kind = 'forkParent') OR (r.related_id = s.session_id AND r.kind = 'subagent'))))
              AND t.idle_deadline_ms <= ?1 AND t.generation = a.generation AND a.enabled = 1
            ORDER BY t.idle_deadline_ms LIMIT 128")?;
        let branches = stmt
            .query_map([now_ms], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for branch in &branches {
            let (environment, agent, session): (String, String, String) =
                serde_json::from_str(branch)?;
            for table in ["native_file_delivery", "opencode_journal_delivery"] {
                tx.execute(
                    &format!(
                        "UPDATE {table} SET pending = 1, next_attempt_at_epoch = ?4
                    WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND blocked = 0
                      AND lease_expires_at_epoch IS NULL"
                    ),
                    params![environment, agent, session, now_ms / 1000],
                )?;
            }
        }
        tx.commit()?;
        Ok(branches.len())
    }
}
