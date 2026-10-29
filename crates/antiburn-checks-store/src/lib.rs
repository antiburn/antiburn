use antiburn_local::analysis::{ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum CheckStatus {
    Failure,
    Success,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eligibility {
    Ready,
    Waiting,
    Unavailable,
    Unsupported,
    Paused,
    Unconfigured,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCheck {
    pub id: String,
    pub check_id: String,
    pub name: String,
    pub message: Option<String>,
    pub status: Option<CheckStatus>,
    pub assessed_at_epoch: Option<i64>,
    pub checking: bool,
    pub eligibility: Eligibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionAvailability {
    Available,
    Paused,
    Invalid,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeDefinition {
    pub check_id: String,
    pub activation_scope_key: String,
    pub generation: Option<i64>,
    pub definition_revision: String,
    pub name: String,
    pub availability: DefinitionAvailability,
}

const MAX_RUNTIME_DEFINITIONS: usize = 128;
const MAX_STATUS_MESSAGE_BYTES: usize = 4096;

/// Replace the last authoritative discovery snapshot in one transaction.
pub fn replace_runtime_definitions(
    connection: &mut Connection,
    definitions: &[RuntimeDefinition],
) -> Result<()> {
    ensure!(
        definitions.len() <= MAX_RUNTIME_DEFINITIONS,
        "runtime definition limit exceeded"
    );
    let mut expected = BTreeMap::new();
    for definition in definitions {
        expected.insert(
            (
                definition.check_id.as_str(),
                definition.activation_scope_key.as_str(),
            ),
            (
                definition.generation,
                definition.definition_revision.as_str(),
                definition.name.as_str(),
                definition.availability,
            ),
        );
    }
    let existing = connection
        .prepare(
            "SELECT check_id, activation_scope_key, generation, definition_revision, name, availability
               FROM burn_check_runtime_definition ORDER BY check_id, activation_scope_key",
        )?
        .query_map([], |row| {
            let availability: String = row.get(5)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                availability,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let current = existing
        .iter()
        .map(
            |(check_id, scope, generation, revision, name, availability)| {
                (
                    (check_id.as_str(), scope.as_str()),
                    (
                        *generation,
                        revision.as_str(),
                        name.as_str(),
                        match availability.as_str() {
                            "available" => DefinitionAvailability::Available,
                            "paused" => DefinitionAvailability::Paused,
                            "invalid" => DefinitionAvailability::Invalid,
                            _ => DefinitionAvailability::Unsupported,
                        },
                    ),
                )
            },
        )
        .collect::<BTreeMap<_, _>>();
    let initialized = runtime_snapshot_ready(connection)?;
    if current == expected && initialized {
        return Ok(());
    }
    let tx = connection.transaction()?;
    if current != expected {
        tx.execute("DELETE FROM burn_check_runtime_definition", [])?;
        for definition in definitions {
            tx.execute(
                "INSERT INTO burn_check_runtime_definition
                   (check_id, activation_scope_key, generation, definition_revision, name, availability)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    definition.check_id,
                    definition.activation_scope_key,
                    definition.generation,
                    definition.definition_revision,
                    definition.name,
                    match definition.availability {
                        DefinitionAvailability::Available => "available",
                        DefinitionAvailability::Paused => "paused",
                        DefinitionAvailability::Invalid => "invalid",
                        DefinitionAvailability::Unsupported => "unsupported",
                    },
                ],
            )?;
        }
    }
    tx.execute(
        "UPDATE burn_check_runtime_definition_state SET initialized = 1
         WHERE singleton = 1 AND protocol_version = 1",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

/// Report whether discovery has published its first authoritative snapshot.
pub fn runtime_snapshot_ready(connection: &Connection) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT initialized = 1 AND protocol_version = 1
         FROM burn_check_runtime_definition_state WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?)
}

fn public_id(scope: &str, check_id: &str, generation: i64, revision: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(scope.as_bytes());
    digest.update([0]);
    digest.update(check_id.as_bytes());
    digest.update([0]);
    digest.update(generation.to_le_bytes());
    digest.update([0]);
    digest.update(revision.as_bytes());
    let hash = digest.finalize();
    let mut id = String::from("yaml:");
    for byte in hash {
        use std::fmt::Write;
        write!(id, "{byte:02x}").expect("writing to a string cannot fail");
    }
    id
}

/// Read the active YAML-check projection for one persisted session.
///
/// The projection contains no evidence payloads and performs no writes.
pub fn read_session_checks(
    connection: &Connection,
    environment: &str,
    agent: &str,
    session_id: &str,
) -> Result<Vec<SessionCheck>> {
    if !runtime_snapshot_ready(connection)? {
        return Ok(Vec::new());
    }
    let session = connection
        .query_row(
            "SELECT cwd, source_kind, source_label FROM session
             WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
               AND wsl_distro IS NULL AND cwd IS NOT NULL AND cwd <> ''
               AND NOT EXISTS(SELECT 1 FROM session_relation r
                 WHERE r.environment_key = session.environment_key AND r.agent = session.agent
                   AND ((r.session_id = session.session_id AND r.kind = 'forkParent')
                     OR (r.related_id = session.session_id AND r.kind = 'subagent'))) ",
            params![environment, agent, session_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .context("read session identity")?;
    let Some((cwd, source_kind, source_label)) = session else {
        return Ok(Vec::new());
    };
    if environment != "native"
        || !(matches!(agent, "claude-code" | "codex" | "pi" | "opencode")
            || (agent == "cursor"
                && source_kind == "inline"
                && source_label.starts_with("cursor-desktop:")))
    {
        return Ok(Vec::new());
    }

    let mut statement = connection.prepare(
        "SELECT d.check_id, d.activation_scope_key, d.generation, a.enabled,
                EXISTS(SELECT 1 FROM burn_check_activation stale
                  WHERE stale.check_id = d.check_id
                    AND stale.activation_scope_key = d.activation_scope_key),
                d.definition_revision,
                d.name, d.availability,
                EXISTS(SELECT 1 FROM burn_check_candidate c
                  WHERE c.check_id = d.check_id AND c.activation_scope_key = d.activation_scope_key
                    AND c.generation = d.generation
                    AND a.enabled = 1
                    AND c.session_key_json = json_array(?1, ?2, ?3)
                    AND c.state IN ('pending', 'claimed', 'dispatching', 'answered')),
                st.status, h.actions_json, h.accepted_at_epoch, h.evidence_valid
         FROM burn_check_runtime_definition d
         LEFT JOIN burn_check_activation a
           ON a.check_id = d.check_id AND a.activation_scope_key = d.activation_scope_key
          AND a.generation = d.generation AND a.definition_revision = d.definition_revision
         LEFT JOIN burn_check_current_status st
           ON st.check_id = d.check_id AND st.activation_scope_key = d.activation_scope_key
          AND st.generation = d.generation AND st.branch_key = json_array(?1, ?2, ?3)
          AND st.evidence_valid = 1 AND a.enabled = 1
          LEFT JOIN burn_check_assessment_history h
            ON h.assessment_id = st.assessment_id AND h.generation = d.generation
           AND h.check_id = st.check_id AND h.activation_scope_key = st.activation_scope_key
           AND h.branch_key = st.branch_key AND h.trigger_position = st.position
           AND h.outcome = 'assessed'
         WHERE json_extract(d.activation_scope_key, '$.environment') = ?1
           AND json_extract(d.activation_scope_key, '$.session_cwd') IS ?4
           AND json_extract(d.activation_scope_key, '$.run') = 'Live'
          ORDER BY d.check_id, d.activation_scope_key LIMIT 129",
    )?;
    let rows = statement.query_map(params![environment, agent, session_id, cwd], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, Option<bool>>(3)?,
            row.get::<_, bool>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, bool>(8)?,
            row.get::<_, Option<String>>(9)?,
            row.get::<_, Option<String>>(10)?,
            row.get::<_, Option<i64>>(11)?,
            row.get::<_, Option<bool>>(12)?,
        ))
    })?;

    let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        rows.len() <= MAX_RUNTIME_DEFINITIONS,
        "session check projection limit exceeded"
    );
    if legacy_registry_source_schema(connection)? {
        return Ok(Vec::new());
    }
    let source_state = if rows
        .iter()
        .any(|row| row.7 == "available" && row.3 == Some(true))
    {
        Some((
            session_source_ready(connection, environment, agent, session_id)?,
            source_can_arrive(connection, environment, agent, session_id)?,
        ))
    } else {
        None
    };
    let mut checks = Vec::new();
    for row in rows {
        let (
            check_id,
            scope,
            generation,
            enabled,
            has_any_activation,
            definition_revision,
            name,
            availability,
            checking,
            status,
            actions,
            assessed_at,
            history_valid,
        ) = row;
        let generation_key = generation.unwrap_or_default();
        if availability != "available" {
            checks.push(SessionCheck {
                id: public_id(&scope, &check_id, generation_key, &definition_revision),
                name,
                check_id,
                message: None,
                status: None,
                assessed_at_epoch: None,
                checking: false,
                eligibility: match availability.as_str() {
                    "paused" => Eligibility::Paused,
                    "unsupported" => Eligibility::Unsupported,
                    _ if enabled == Some(false) => Eligibility::Paused,
                    _ => Eligibility::Unavailable,
                },
            });
            continue;
        }
        let Some(enabled) = enabled else {
            if has_any_activation {
                continue;
            }
            checks.push(SessionCheck {
                id: public_id(&scope, &check_id, generation_key, &definition_revision),
                name,
                check_id,
                message: None,
                status: None,
                assessed_at_epoch: None,
                checking: false,
                eligibility: Eligibility::Unconfigured,
            });
            continue;
        };
        if !enabled {
            checks.push(SessionCheck {
                id: public_id(&scope, &check_id, generation_key, &definition_revision),
                name,
                check_id,
                message: None,
                status: None,
                assessed_at_epoch: None,
                checking: false,
                eligibility: Eligibility::Paused,
            });
            continue;
        }

        let (ready, can_arrive) = source_state.expect("active definitions have source state");
        if status.is_some() {
            ensure!(
                history_valid.is_some(),
                "current status has no matching assessment"
            );
        }
        let status = if ready && history_valid == Some(true) {
            match status.as_deref() {
                Some("failure") => Some(CheckStatus::Failure),
                Some("success") => Some(CheckStatus::Success),
                None => None,
                _ => anyhow::bail!("unknown current check status"),
            }
        } else {
            None
        };
        let message = match status {
            Some(status) => status_message(
                Some(
                    actions
                        .as_deref()
                        .context("current status has no saved actions")?,
                ),
                match status {
                    CheckStatus::Failure => "failure",
                    CheckStatus::Success => "success",
                },
            )?,
            None => None,
        };
        ensure!(
            message
                .as_ref()
                .is_none_or(|message| message.len() <= MAX_STATUS_MESSAGE_BYTES),
            "check status message exceeds the projection limit"
        );
        checks.push(SessionCheck {
            id: public_id(&scope, &check_id, generation_key, &definition_revision),
            name,
            check_id,
            message,
            status,
            assessed_at_epoch: status.and(assessed_at),
            checking,
            eligibility: if ready && status.is_some() {
                Eligibility::Ready
            } else if checking || can_arrive {
                Eligibility::Waiting
            } else {
                Eligibility::Unavailable
            },
        });
    }
    Ok(checks)
}

fn legacy_registry_source_schema(connection: &Connection) -> Result<bool> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if !(81..=83).contains(&version) {
        return Ok(false);
    }
    // Registry-only branch builds precede capture and typed source metadata.
    // Withhold their projection rather than infer current source readiness.
    let legacy: bool = connection.query_row(
        "SELECT NOT EXISTS(SELECT 1 FROM sqlite_schema WHERE name IN
            ('opencode_v2_selection', 'opencode_v2_capture', 'opencode_v2_source_status',
             'session_relation_related'))
          AND (SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name IN
            ('native_file_delivery', 'opencode_journal_delivery',
             'native_tool_result_publication')) = 3
          AND NOT EXISTS(SELECT 1 FROM pragma_table_info('session') WHERE name = 'source_format')
          AND NOT EXISTS(SELECT 1 FROM pragma_table_info('native_tool_result_publication')
            WHERE name = 'gap_kind')",
        [],
        |row| row.get(0),
    )?;
    if !legacy {
        return Ok(false);
    }
    for sql in [
        "SELECT environment_key, agent, session_id, cwd, source_kind, source_generation,
            incarnation, source_label, wsl_distro FROM session LIMIT 0",
        "SELECT environment_key, agent, session_id, related_id, kind
         FROM session_relation LIMIT 0",
        "SELECT environment_key, agent, session_id, status, analyzed_generation,
            parser_revision, analyzer_revision, evidence_schema_revision
         FROM session_evidence LIMIT 0",
        "SELECT environment_key, agent, session_id, incarnation, source_label, cwd,
            parser_revision, cursor_json, blocked, last_error FROM native_file_delivery LIMIT 0",
        "SELECT environment_key, agent, session_id, incarnation, source_label, cwd,
            parser_revision, cursor_json, blocked, main_session, last_error
         FROM opencode_journal_delivery LIMIT 0",
        "SELECT publication_sequence, environment_key, agent, session_id, source_format,
            session_incarnation, project_cwd, outcome, gap_reason
         FROM native_tool_result_publication LIMIT 0",
    ] {
        let _statement = connection.prepare(sql)?;
    }
    Ok(true)
}

pub fn is_current_activation(
    connection: &Connection,
    check_id: &str,
    activation_scope_key: &str,
    generation: i64,
    definition_revision: &str,
    runtime_available: bool,
) -> rusqlite::Result<bool> {
    if !runtime_available {
        return Ok(false);
    }
    let snapshot_ready: bool = connection.query_row(
        "SELECT initialized = 1 AND protocol_version = 1
         FROM burn_check_runtime_definition_state WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    if !snapshot_ready {
        return Ok(false);
    }
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM burn_check_activation a
          JOIN burn_check_runtime_definition d USING(check_id, activation_scope_key)
          WHERE a.check_id = ?1 AND a.activation_scope_key = ?2 AND a.generation = ?3
            AND a.definition_revision = ?4 AND a.enabled = 1
            AND d.generation = a.generation AND d.definition_revision = a.definition_revision
            AND d.availability = 'available')",
        params![
            check_id,
            activation_scope_key,
            generation,
            definition_revision
        ],
        |row| row.get(0),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPlan {
    current_status: CheckStatus,
    outcomes: Vec<StoredAction>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAction {
    index: usize,
    outcome: StoredActionOutcome,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum StoredActionOutcome {
    Applied {
        result: CheckStatus,
        message: Option<String>,
    },
    SkippedConditionFalse,
    SkippedConditionUnavailable,
    SkippedInputUnavailable,
    Failed {
        code: String,
        message: String,
    },
}

pub fn status_message(actions: Option<&str>, status: &str) -> Result<Option<String>> {
    let Some(actions) = actions else {
        return Ok(None);
    };
    let expected = match status {
        "failure" => CheckStatus::Failure,
        "success" => CheckStatus::Success,
        _ => anyhow::bail!("unknown current check status"),
    };
    let actions: StoredPlan = serde_json::from_str(actions).context("decode status actions")?;
    ensure!(
        actions.current_status == expected,
        "stored plan does not match current status"
    );
    let mut last_write = None;
    for (index, action) in actions.outcomes.into_iter().enumerate() {
        ensure!(
            action.index == index,
            "stored status action order is invalid"
        );
        match action.outcome {
            StoredActionOutcome::Applied { result, message } => {
                ensure!(
                    message
                        .as_ref()
                        .is_none_or(|message| !message.trim().is_empty()),
                    "stored status message is blank"
                );
                last_write = Some((result, message));
            }
            StoredActionOutcome::Failed { code, message } => {
                ensure!(
                    !code.is_empty() && !message.is_empty(),
                    "stored action error is invalid"
                );
            }
            StoredActionOutcome::SkippedConditionFalse
            | StoredActionOutcome::SkippedConditionUnavailable
            | StoredActionOutcome::SkippedInputUnavailable => {}
        }
    }
    let (result, message) = last_write.context("current status has no applied action")?;
    ensure!(
        result == expected,
        "stored status action does not match current status"
    );
    ensure!(
        message
            .as_ref()
            .is_none_or(|message| message.len() <= MAX_STATUS_MESSAGE_BYTES),
        "check status message exceeds the projection limit"
    );
    Ok(message)
}

fn session_source_ready(
    connection: &Connection,
    environment: &str,
    agent: &str,
    session_id: &str,
) -> Result<bool> {
    let ready = source_ready_sql(1, 2, 3);
    Ok(connection.query_row(
        &format!(
            "SELECT COALESCE(({ready}), 0) FROM session s
             LEFT JOIN session_evidence e USING(environment_key, agent, session_id)
             WHERE s.environment_key = ?4 AND s.agent = ?5 AND s.session_id = ?6"
        ),
        params![
            PARSER_REVISION,
            ANALYZER_REVISION,
            EVIDENCE_SCHEMA_REVISION,
            environment,
            agent,
            session_id
        ],
        |row| row.get(0),
    )?)
}

fn source_can_arrive(
    connection: &Connection,
    environment: &str,
    agent: &str,
    session_id: &str,
) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT s.environment_key = 'native' AND s.wsl_distro IS NULL
             AND s.cwd IS NOT NULL AND s.cwd <> ''
             AND NOT EXISTS(SELECT 1 FROM session_relation r
               WHERE r.environment_key = s.environment_key AND r.agent = s.agent
                 AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
                   OR (r.related_id = s.session_id AND r.kind = 'subagent')))
         FROM session s WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
           AND ((s.agent = 'opencode' AND s.source_kind = 'providerDb')
             OR (s.agent IN ('claude-code', 'codex', 'pi') AND s.source_kind = 'file')
             OR (s.agent = 'cursor' AND s.source_kind = 'inline'
               AND s.source_label GLOB 'cursor-desktop:*'))",
            params![environment, agent, session_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false))
}

/// Check a native file receipt's supported layout and public identity fields.
///
/// Use delivery alias `d` and session alias `s`, joined by environment, agent,
/// and session ID. `parser` is a SQL parameter position. A NULL cursor permits
/// initialization only when its parser is NULL or known. A saved cursor needs a
/// known pair: 62/layout 4, main 64/layout 1, or current/63/64/65/66/layout 5.
///
/// This predicate does not grant readiness or pending work. It does not check
/// source incarnation, health, activity, or origin. Apply the native reset policy
/// first when that policy discards the old cursor. The reader still validates
/// the full cursor and file identity when it resumes a retained receipt.
/// Main at `143d4b05` needs its named converter before a layout-5 page can commit.
pub fn native_file_receipt_compatible_sql(parser: usize) -> String {
    format!(
        "CASE WHEN d.cursor_json IS NULL THEN
           d.parser_revision IS NULL OR d.parser_revision IN (?{parser}, 62, 63, 64, 65, 66)
         ELSE COALESCE(
            ((d.parser_revision = 62 AND json_extract(d.cursor_json, '$.revision') = 4)
             OR (d.parser_revision = 64 AND json_extract(d.cursor_json, '$.revision') = 1)
             OR (d.parser_revision IN (?{parser}, 63, 64, 65, 66)
               AND json_extract(d.cursor_json, '$.revision') = 5))
           AND json_extract(d.cursor_json, '$.session_id') = s.session_id
           AND json_extract(d.cursor_json, '$.agent') = CASE s.agent
             WHEN 'claude-code' THEN 'claude' ELSE s.agent END
           AND json_extract(d.cursor_json, '$.source_format') = CASE s.agent
             WHEN 'claude-code' THEN 'claude_jsonl'
             WHEN 'codex' THEN 'codex_rollout_jsonl' WHEN 'pi' THEN 'pi_v3_jsonl' END
           AND (s.source_format IS NULL OR s.source_format =
             json_quote(json_extract(d.cursor_json, '$.source_format'))), 0) END"
    )
}

/// Check a V1 journal receipt's supported live layout and public identity fields.
///
/// Use delivery alias `d` and session alias `s`, joined by environment, agent,
/// and session ID. `parser` is a SQL parameter position. NULL cursor/parser rules
/// match `native_file_receipt_compatible_sql`. Saved receipts need matching row
/// and cursor parsers: 62/live 4, main 64/live 2, or current/63/64/65/66/live 5.
/// Live 2 needs a non-negative integer baseline. Live 5 needs a non-negative
/// integer public baseline. These checks do not prove historical public fields.
///
/// This predicate does not check readiness, source binding, health, or activity.
/// Apply an approved cursor-discarding reset before this retained-receipt gate.
/// The reader still validates the full cursor, owner, sequences, and samples.
/// Main at `143d4b05` needs its named converter before a live-5 page can commit.
pub fn opencode_journal_receipt_compatible_sql(parser: usize) -> String {
    format!(
        "CASE WHEN d.cursor_json IS NULL THEN
           d.parser_revision IS NULL OR d.parser_revision IN (?{parser}, 62, 63, 64, 65, 66)
         ELSE COALESCE(
           json_extract(d.cursor_json, '$.session_id') = s.session_id
           AND json_extract(d.cursor_json, '$.parser_revision') = d.parser_revision
           AND ((d.parser_revision = 62 AND json_extract(d.cursor_json, '$.live.revision') = 4)
             OR (d.parser_revision = 64 AND json_extract(d.cursor_json, '$.live.revision') = 2
               AND json_type(d.cursor_json, '$.live.baseline') = 'integer'
               AND json_extract(d.cursor_json, '$.live.baseline') >= 0)
             OR (d.parser_revision IN (?{parser}, 63, 64, 65, 66)
               AND json_extract(d.cursor_json, '$.live.revision') = 5
               AND json_type(d.cursor_json, '$.live.public_baseline') = 'integer'
               AND json_extract(d.cursor_json, '$.live.public_baseline') >= 0)), 0) END"
    )
}

/// Builds source validity for databases with native OpenCode V2 capture.
pub fn source_ready_at_sql(parser: usize, analyzer: usize, evidence: usize, now: &str) -> String {
    // Named converters permit decoding, not readiness before a layout-5 page commits.
    let file_receipt = native_file_receipt_compatible_sql(parser);
    let journal_receipt = opencode_journal_receipt_compatible_sql(parser);
    format!(
        "CASE WHEN s.agent = 'opencode' AND s.source_kind = 'providerDb'
        AND s.source_format = '\"open_code_v2_sqlite\"' THEN
          s.environment_key = 'native' AND s.wsl_distro IS NULL
          AND s.cwd IS NOT NULL AND s.cwd <> ''
          AND NOT EXISTS(SELECT 1 FROM session_relation r
            WHERE r.environment_key = s.environment_key AND r.agent = s.agent
              AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
                OR (r.related_id = s.session_id AND r.kind = 'subagent')))
          AND EXISTS(SELECT 1 FROM opencode_v2_capture d
            WHERE d.environment_key = s.environment_key AND d.agent = s.agent
              AND d.session_id = s.session_id AND d.incarnation = s.incarnation
              AND d.source_label = s.source_label AND d.cwd = s.cwd
              AND d.capture_epoch <> '' AND d.claim_fence > 0
              AND d.lease_expires_at_epoch > ({now}))
        WHEN s.agent = 'opencode' AND s.source_kind = 'providerDb' THEN
          s.environment_key = 'native' AND s.wsl_distro IS NULL
          AND s.source_format = '\"open_code_sqlite_v2\"'
          AND s.cwd IS NOT NULL AND s.cwd <> ''
          AND NOT EXISTS(SELECT 1 FROM session_relation r
            WHERE r.environment_key = s.environment_key AND r.agent = s.agent
              AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
                OR (r.related_id = s.session_id AND r.kind = 'subagent')))
          AND EXISTS (
            SELECT 1 FROM opencode_journal_delivery d
             WHERE d.environment_key = s.environment_key AND d.agent = s.agent AND d.session_id = s.session_id
               AND d.incarnation = s.incarnation AND d.source_label = s.source_label AND d.cwd IS s.cwd
                  AND ({journal_receipt})
                  AND json_extract(d.cursor_json, '$.live.revision') = 5
                  AND json_type(d.cursor_json, '$.cold_replay') IS NULL
                 AND d.blocked = 0 AND d.main_session = 1
                AND (d.last_error IS NULL OR d.last_error IN ('journal-history-incomplete', 'journal-cancelled')))
            WHEN s.environment_key = 'native' AND s.agent IN ('claude-code', 'codex', 'pi')
              AND s.source_kind = 'file' THEN s.wsl_distro IS NULL
              AND s.cwd IS NOT NULL AND s.cwd <> '' AND EXISTS (
             SELECT 1 FROM native_file_delivery d
              WHERE d.environment_key = s.environment_key AND d.agent = s.agent AND d.session_id = s.session_id
                AND d.incarnation = s.incarnation AND d.source_label = s.source_label AND d.cwd IS s.cwd
                  AND ({file_receipt})
                  AND json_extract(d.cursor_json, '$.revision') = 5
                 AND d.blocked = 0 AND d.last_error IS NULL
                AND NOT EXISTS(SELECT 1 FROM session_relation r
                  WHERE r.environment_key = s.environment_key AND r.agent = s.agent
                    AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
                       OR (r.related_id = s.session_id AND r.kind = 'subagent')))
                 AND (s.agent <> 'claude-code' OR (d.parser_revision IN (?{parser}, 65, 66)
                   AND EXISTS(SELECT 1 FROM native_tool_result_publication origin
                     JOIN setting origin_binding ON origin_binding.key =
                       'internal:claudeRequestOriginParser65:' ||
                         json_array(s.environment_key, s.agent, s.session_id)
                     WHERE origin.environment_key = s.environment_key AND origin.agent = s.agent
                       AND origin.session_id = s.session_id AND origin.source_format = '\"claude_jsonl\"'
                       AND origin.session_incarnation = s.incarnation AND origin.project_cwd IS s.cwd
                       AND origin.outcome = 'coverage_gap' AND origin.gap_kind = 'segment_reset'
                       AND origin.gap_reason = 'claude-request-origin-parser-65'
                       AND json_extract(origin_binding.value, '$.publication_sequence') = origin.publication_sequence
                       AND json_extract(origin_binding.value, '$.source_label') = s.source_label
                       AND json_extract(origin_binding.value, '$.incarnation') = s.incarnation
                       AND json_extract(origin_binding.value, '$.cwd') IS s.cwd
                       AND json_extract(origin_binding.value, '$.source_format') = 'claude_jsonl'))))
            WHEN s.agent IN ('claude-code', 'codex', 'pi', 'opencode') THEN 0
            WHEN s.agent = 'cursor' THEN s.environment_key = 'native'
             AND s.source_kind = 'inline' AND s.source_label GLOB 'cursor-desktop:*'
             AND s.wsl_distro IS NULL
             AND NOT EXISTS(SELECT 1 FROM session_relation r
               WHERE r.environment_key = s.environment_key AND r.agent = s.agent
                 AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
                   OR (r.related_id = s.session_id AND r.kind = 'subagent')))
             AND COALESCE(e.status = 'ready' AND e.analyzed_generation = s.source_generation
               AND e.parser_revision = ?{parser} AND e.analyzer_revision = ?{analyzer}
               AND e.evidence_schema_revision = ?{evidence}, 0)
           ELSE COALESCE(e.status = 'ready' AND e.analyzed_generation = s.source_generation
              AND e.parser_revision = ?{parser} AND e.analyzer_revision = ?{analyzer}
               AND e.evidence_schema_revision = ?{evidence}, 0) END"
    )
}

/// Build the same source-validity predicate used by the desktop report.
pub fn source_ready_sql(parser: usize, analyzer: usize, evidence: usize) -> String {
    source_ready_at_sql(parser, analyzer, evidence, "unixepoch()")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!("../tests/fixtures/session.sql"))
            .unwrap();
        connection
            .execute(
                "UPDATE native_file_delivery SET parser_revision = ?1",
                [PARSER_REVISION],
            )
            .unwrap();
        connection
    }

    fn seed_activation(connection: &Connection, enabled: bool) {
        let scope = r#"{"environment":"native","session_cwd":"/repo","run":"Live"}"#;
        connection
            .execute(
                "INSERT INTO burn_check_activation VALUES ('retry-loop',?1,2,?2,'revision')",
                params![scope, enabled],
            )
            .unwrap();
        connection.execute(
            "INSERT INTO burn_check_runtime_definition VALUES ('retry-loop',?1,2,'revision','retry-loop','available')",
            [scope],
        ).unwrap();
    }

    #[test]
    fn projects_current_status_and_authored_message_without_evidence() {
        let connection = database();
        seed_activation(&connection, true);
        let scope = r#"{"environment":"native","session_cwd":"/repo","run":"Live"}"#;
        connection.execute(
            "INSERT INTO burn_check_current_status VALUES ('retry-loop',?1,2,'[\"native\",\"claude-code\",\"s1\"]','failure','a1',1,0)",
            [scope],
        ).unwrap();
        connection.execute(
            "INSERT INTO burn_check_assessment_history VALUES ('a1',2,'assessed',1,?1,123,'retry-loop',?2,'[\"native\",\"claude-code\",\"s1\"]',0)",
            params![r#"{"current_status":"Failure","outcomes":[{"index":0,"outcome":{"Applied":{"result":"Failure","message":"Repeated retries detected."}}}]}"#, scope],
        ).unwrap();
        let before: i64 = connection
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();

        let checks = read_session_checks(&connection, "native", "claude-code", "s1").unwrap();

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check_id, "retry-loop");
        assert_eq!(checks[0].name, "retry-loop");
        assert_eq!(checks[0].status, Some(CheckStatus::Failure));
        assert_eq!(
            checks[0].message.as_deref(),
            Some("Repeated retries detected.")
        );
        assert_eq!(checks[0].assessed_at_epoch, Some(123));
        assert_eq!(checks[0].eligibility, Eligibility::Ready);
        let after: i64 = connection
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn status_message_keeps_applied_without_message_distinct_from_missing_action() {
        let actions = r#"{"current_status":"Failure","outcomes":[{"index":0,"outcome":{"Applied":{"result":"Failure","message":null}}}]}"#;
        assert_eq!(status_message(Some(actions), "failure").unwrap(), None);
        assert!(
            status_message(
                Some(r#"{"current_status":"Failure","outcomes":[]}"#),
                "failure"
            )
            .is_err()
        );
    }

    #[test]
    fn first_empty_discovery_marks_the_runtime_snapshot_initialized() {
        let mut connection = database();
        connection
            .execute(
                "UPDATE burn_check_runtime_definition_state SET initialized = 0",
                [],
            )
            .unwrap();
        assert!(!runtime_snapshot_ready(&connection).unwrap());
        assert!(
            read_session_checks(&connection, "native", "claude-code", "s1")
                .unwrap()
                .is_empty()
        );

        replace_runtime_definitions(&mut connection, &[]).unwrap();
        assert!(runtime_snapshot_ready(&connection).unwrap());
    }

    #[test]
    fn marks_disabled_activation_paused_and_excludes_subagents() {
        let connection = database();
        seed_activation(&connection, false);
        let checks = read_session_checks(&connection, "native", "claude-code", "s1").unwrap();
        assert_eq!(checks[0].eligibility, Eligibility::Paused);
        connection.execute(
            "INSERT INTO session_relation VALUES ('native','claude-code','child','s1','subagent')",
            [],
        ).unwrap();
        assert!(
            read_session_checks(&connection, "native", "claude-code", "s1")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn runtime_snapshot_replacement_invalidates_stale_definitions() {
        let mut connection = database();
        seed_activation(&connection, true);
        assert!(
            is_current_activation(
                &connection,
                "retry-loop",
                r#"{"environment":"native","session_cwd":"/repo","run":"Live"}"#,
                2,
                "revision",
                true,
            )
            .unwrap()
        );
        connection
            .execute(
                "UPDATE burn_check_runtime_definition SET generation = 9 WHERE check_id = 'retry-loop'",
                [],
            )
            .unwrap();
        assert!(
            read_session_checks(&connection, "native", "claude-code", "s1")
                .unwrap()
                .is_empty()
        );
        connection
            .execute(
                "UPDATE burn_check_runtime_definition SET generation = 2, definition_revision = 'old' WHERE check_id = 'retry-loop'",
                [],
            )
            .unwrap();
        assert!(
            read_session_checks(&connection, "native", "claude-code", "s1")
                .unwrap()
                .is_empty()
        );
        connection
            .execute(
                "UPDATE burn_check_runtime_definition SET definition_revision = 'revision' WHERE check_id = 'retry-loop'",
                [],
            )
            .unwrap();
        replace_runtime_definitions(&mut connection, &[]).unwrap();
        assert!(
            !is_current_activation(
                &connection,
                "retry-loop",
                r#"{"environment":"native","session_cwd":"/repo","run":"Live"}"#,
                2,
                "revision",
                true,
            )
            .unwrap()
        );
        assert!(
            read_session_checks(&connection, "native", "claude-code", "s1")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn marks_active_check_waiting_when_source_has_not_arrived() {
        let connection = database();
        connection
            .execute("UPDATE native_file_delivery SET cursor_json = NULL", [])
            .unwrap();
        seed_activation(&connection, true);
        let checks = read_session_checks(&connection, "native", "claude-code", "s1").unwrap();
        assert_eq!(checks[0].eligibility, Eligibility::Waiting);
    }

    #[test]
    fn source_ready_without_assessment_remains_waiting() {
        let connection = database();
        seed_activation(&connection, true);
        let checks = read_session_checks(&connection, "native", "claude-code", "s1").unwrap();
        assert_eq!(checks[0].status, None);
        assert_eq!(checks[0].eligibility, Eligibility::Waiting);
    }

    #[test]
    fn excludes_wsl_fork_and_unsupported_sessions() {
        let connection = database();
        seed_activation(&connection, true);
        connection
            .execute("UPDATE session SET wsl_distro = 'synthetic'", [])
            .unwrap();
        assert!(
            read_session_checks(&connection, "native", "claude-code", "s1")
                .unwrap()
                .is_empty()
        );
        connection
            .execute("UPDATE session SET wsl_distro = NULL", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO session_relation VALUES ('native','claude-code','s1',NULL,'forkParent')",
                [],
            )
            .unwrap();
        assert!(
            read_session_checks(&connection, "native", "claude-code", "s1")
                .unwrap()
                .is_empty()
        );
        connection
            .execute("DELETE FROM session_relation", [])
            .unwrap();
        connection
            .execute("UPDATE session SET agent = 'cursor'", [])
            .unwrap();
        assert!(
            read_session_checks(&connection, "native", "cursor", "s1")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cursor_desktop_projection_requires_native_inline_source() {
        let connection = database();
        seed_activation(&connection, true);
        connection.execute("UPDATE session SET agent = 'cursor', source_kind = 'inline', source_label = 'cursor-desktop:workspace:s1'", []).unwrap();
        let checks = read_session_checks(&connection, "native", "cursor", "s1").unwrap();
        assert_eq!(checks.len(), 1);
        connection
            .execute("UPDATE session SET source_label = 'cursor-cli:s1'", [])
            .unwrap();
        assert!(
            read_session_checks(&connection, "native", "cursor", "s1")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn reports_unsupported_registry_definition_without_activation() {
        let connection = database();
        seed_activation(&connection, true);
        connection
            .execute("DELETE FROM burn_check_activation", [])
            .unwrap();
        connection
            .execute(
                "UPDATE burn_check_runtime_definition
                 SET generation = NULL, availability = 'unsupported'",
                [],
            )
            .unwrap();
        let checks = read_session_checks(&connection, "native", "claude-code", "s1").unwrap();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].eligibility, Eligibility::Unsupported);
        assert_eq!(checks[0].status, None);
    }

    #[test]
    fn invalidation_hides_old_result_and_recovery_restores_it() {
        let connection = database();
        seed_activation(&connection, true);
        let scope = r#"{"environment":"native","session_cwd":"/repo","run":"Live"}"#;
        connection.execute(
            "INSERT INTO burn_check_current_status VALUES ('retry-loop',?1,2,'[\"native\",\"claude-code\",\"s1\"]','failure','a1',1,0)",
            [scope],
        ).unwrap();
        connection.execute(
            "INSERT INTO burn_check_assessment_history VALUES ('a1',2,'assessed',1,?1,123,'retry-loop',?2,'[\"native\",\"claude-code\",\"s1\"]',0)",
            params![r#"{"current_status":"Failure","outcomes":[{"index":0,"outcome":{"Applied":{"result":"Failure","message":"Repeated retries detected."}}}]}"#, scope],
        ).unwrap();

        connection
            .execute(
                "UPDATE native_file_delivery SET cursor_json = NULL WHERE session_id = 's1'",
                [],
            )
            .unwrap();
        let invalidated = read_session_checks(&connection, "native", "claude-code", "s1").unwrap();
        assert_eq!(invalidated[0].eligibility, Eligibility::Waiting);
        assert_eq!(invalidated[0].status, None);
        assert_eq!(invalidated[0].message, None);
        assert_eq!(invalidated[0].assessed_at_epoch, None);

        connection
            .execute(
                "UPDATE native_file_delivery SET cursor_json =
                 '{\"revision\":5,\"agent\":\"claude\",\"session_id\":\"s1\",\"source_format\":\"claude_jsonl\"}'
                 WHERE session_id = 's1'",
                [],
            )
            .unwrap();
        let recovered = read_session_checks(&connection, "native", "claude-code", "s1").unwrap();
        assert_eq!(recovered[0].eligibility, Eligibility::Ready);
        assert_eq!(recovered[0].status, Some(CheckStatus::Failure));
        assert_eq!(
            recovered[0].message.as_deref(),
            Some("Repeated retries detected.")
        );
    }
    #[test]
    fn v2_source_readiness_uses_the_current_capture_lease() {
        let connection = database();
        connection
            .execute_batch(
                "UPDATE session SET agent = 'opencode', source_kind = 'providerDb',
                source_format = '\"open_code_v2_sqlite\"';
             INSERT INTO opencode_v2_capture VALUES
                ('native','opencode','s1',7,'source','/repo','epoch',1,unixepoch()+60);",
            )
            .unwrap();
        assert!(session_source_ready(&connection, "native", "opencode", "s1").unwrap());
        connection
            .execute("UPDATE opencode_v2_capture SET incarnation = 8", [])
            .unwrap();
        assert!(!session_source_ready(&connection, "native", "opencode", "s1").unwrap());
        connection
            .execute(
                "UPDATE opencode_v2_capture SET incarnation = 7, lease_expires_at_epoch = 0",
                [],
            )
            .unwrap();
        assert!(!session_source_ready(&connection, "native", "opencode", "s1").unwrap());
        connection
            .execute(
                "UPDATE opencode_v2_capture SET lease_expires_at_epoch = unixepoch()+60",
                [],
            )
            .unwrap();
        connection.execute("INSERT INTO session_relation VALUES ('native','opencode','s1','parent','forkParent')", []).unwrap();
        assert!(!session_source_ready(&connection, "native", "opencode", "s1").unwrap());
    }
}
