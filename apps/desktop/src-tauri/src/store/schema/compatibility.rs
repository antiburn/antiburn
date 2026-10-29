use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension};

const UNTYPED_GAP_HEAD_KEY: &str = "internal:untypedNativeGapPublicationHeadV1";
const LEGACY_SOFT_GAPS: &str = include_str!("compatibility/legacy_soft_gaps.sql");

pub(in crate::store) fn prepare_source_metadata(connection: &Connection) -> Result<()> {
    // The boundary distinguishes old untyped rows from later explicit values.
    connection.execute(
        "INSERT INTO setting (key, value)
         SELECT ?1, CAST(COALESCE(MAX(publication_sequence), 0) AS TEXT)
           FROM native_tool_result_publication",
        [UNTYPED_GAP_HEAD_KEY],
    )?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct Profile {
    quota: bool,
    capture: bool,
    capture_fields: bool,
    accounting: bool,
    source_metadata: bool,
    runtime_definitions: bool,
    retired: bool,
    decisions: bool,
    evidence: bool,
    selection: bool,
    served_model: bool,
}

impl Profile {
    fn canonical(version: i64) -> Self {
        Self {
            quota: version >= 81,
            capture: version >= 82,
            capture_fields: version >= 83,
            accounting: version >= 84,
            source_metadata: version >= 85,
            runtime_definitions: version >= 86,
            retired: version >= 87,
            decisions: version >= 88,
            evidence: version >= 89,
            selection: version >= 90,
            served_model: version >= 91,
        }
    }

    fn branch(version: i64) -> Self {
        Self {
            quota: false,
            capture: true,
            capture_fields: version >= 82,
            accounting: version >= 83,
            source_metadata: false,
            runtime_definitions: false,
            retired: version >= 84,
            decisions: version >= 85,
            evidence: version >= 86,
            selection: version >= 87,
            served_model: false,
        }
    }

    fn pre_registry(version: i64) -> Self {
        Self {
            runtime_definitions: false,
            ..Self::canonical(version + 1)
        }
    }
}

pub(in crate::store) fn repair_branch_version(
    connection: &mut Connection,
    current: i64,
) -> Result<i64> {
    if !(81..=91).contains(&current) {
        return Ok(current);
    }
    let transaction = connection.transaction()?;
    let profile = read_profile(&transaction)
        .with_context(|| format!("invalid migration schema at version {current}"))?;
    if profile == Profile::canonical(current) {
        transaction.commit()?;
        return Ok(current);
    }
    let registry_only = Profile {
        quota: profile.quota,
        runtime_definitions: true,
        ..Profile::canonical(80)
    };
    if current <= 83 && profile == registry_only {
        transaction.execute_batch(super::V81)?;
        transaction.pragma_update(None, "user_version", 81)?;
        transaction.commit()?;
        return Ok(81);
    }
    let target = if (86..=90).contains(&current) && profile == Profile::pre_registry(current) {
        transaction.execute_batch(super::V86)?;
        current + 1
    } else {
        ensure!(
            current <= 87 && profile == Profile::branch(current),
            "unrecognized migration schema at version {current}: {profile:?}"
        );
        // Each branch step already ran. Add only the missing main schema.
        transaction.execute_batch(super::V81)?;
        if current <= 83 {
            current + 1
        } else {
            prepare_source_metadata(&transaction)?;
            transaction.execute_batch(super::V85)?;
            transaction.execute_batch(super::V86)?;
            if current == 87 {
                transaction.execute_batch(LEGACY_SOFT_GAPS)?;
            }
            current + 3
        }
    };
    ensure!(
        read_profile(&transaction)? == Profile::canonical(target),
        "incomplete migration repair from version {current} to {target}"
    );
    transaction.pragma_update(None, "user_version", target)?;
    transaction.commit()?;
    Ok(target)
}

fn read_profile(connection: &Connection) -> Result<Profile> {
    let quota = objects(
        connection,
        "quota index",
        "index",
        &["turn_quota_published"],
    )?;
    if quota {
        validate_index(
            connection,
            "turn_quota_published",
            &super::V81.replace("IF NOT EXISTS ", ""),
        )?;
    }
    let capture = objects(
        connection,
        "capture tables",
        "table",
        &[
            "opencode_v2_selection",
            "opencode_v2_capture",
            "opencode_v2_source_status",
        ],
    )?;
    ensure!(
        capture
            == objects(
                connection,
                "capture lease",
                "index",
                &["opencode_v2_capture_lease"]
            )?
            && capture
                == columns(
                    connection,
                    "capture identity",
                    &[
                        ("opencode_v2_capture", "capture_epoch"),
                        ("opencode_v2_capture", "baseline_sequence"),
                        ("opencode_v2_capture", "last_sequence"),
                        ("opencode_v2_capture", "history_gap"),
                        ("opencode_v2_capture", "lease_expires_at_epoch"),
                        ("opencode_v2_source_status", "code"),
                        ("opencode_v2_source_status", "observed_at_epoch"),
                    ]
                )?,
        "incomplete capture schema"
    );
    if capture {
        validate_capture_tables(connection)?;
        validate_index(
            connection,
            "opencode_v2_capture_lease",
            "CREATE INDEX opencode_v2_capture_lease ON opencode_v2_capture (lease_expires_at_epoch)",
        )?;
    }
    let capture_fields = columns(
        connection,
        "capture retirement",
        &[
            ("opencode_v2_capture", "admitted_source_json"),
            ("opencode_v2_capture", "retirement_reason"),
        ],
    )?;
    let accounting = columns(
        connection,
        "turn accounting",
        &[("turn", "accounting_json")],
    )?;
    let source_metadata = columns(
        connection,
        "source metadata",
        &[
            ("session", "source_format"),
            ("native_tool_result_publication", "gap_kind"),
        ],
    )?;
    ensure!(
        source_metadata
            == objects(
                connection,
                "related-session index",
                "index",
                &["session_relation_related"]
            )?,
        "incomplete source metadata schema"
    );
    if source_metadata {
        validate_index(
            connection,
            "session_relation_related",
            "CREATE INDEX session_relation_related ON session_relation(environment_key, agent, related_id, kind)",
        )?;
    }
    if let Some(head) = connection
        .query_row(
            "SELECT value FROM setting WHERE key = ?1",
            [UNTYPED_GAP_HEAD_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        ensure!(
            source_metadata && head.parse::<i64>().is_ok_and(|value| value >= 0),
            "invalid untyped-gap migration boundary"
        );
    }
    let retired = !objects(
        connection,
        "retired check tables",
        "table",
        &[
            "burn_check_assessment",
            "burn_check_dispatch_attempt",
            "burn_check_sampled_pair",
            "burn_check_sample_origin",
            "burn_check_instruction_epoch",
            "burn_check_work_answer",
            "burn_check_response_cache",
            "burn_check_request_outcome",
            "burn_check_usage_reservation",
        ],
    )?;
    let decisions = columns(
        connection,
        "named decisions",
        &[
            ("burn_check_candidate", "started_cooldown_until_epoch"),
            ("burn_check_evaluation_attempt", "decision_name"),
        ],
    )?;
    let attempt_key = if decisions {
        "candidate_id,decision_name,attempt_number"
    } else {
        "candidate_id,attempt_number"
    };
    let keys = connection
        .prepare(
            "SELECT (SELECT group_concat(name, ',') FROM
            (SELECT name FROM pragma_index_info(i.name) ORDER BY seqno)), i.partial
           FROM pragma_index_list('burn_check_evaluation_attempt') i WHERE i.\"unique\" = 1",
        )?
        .query_map([], |row| {
            Ok((row.get::<_, Option<String>>(0)?, row.get::<_, bool>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        keys.len() == 2
            && keys.iter().all(|(_, partial)| !partial)
            && keys
                .iter()
                .any(|(key, _)| key.as_deref() == Some("attempt_id"))
            && keys
                .iter()
                .any(|(key, _)| key.as_deref() == Some(attempt_key)),
        "invalid named-decision unique-key layout"
    );
    let evidence = columns(
        connection,
        "public evidence",
        &[
            ("native_tool_result_publication", "event_kind"),
            (
                "native_tool_result_publication",
                "first_eligible_publication",
            ),
            ("native_tool_result_publication", "owner_message_id"),
            ("native_tool_result_publication", "target_check_id"),
            ("native_tool_result_publication", "target_scope_key"),
            ("native_tool_result_publication", "target_generation"),
            ("native_tool_result_publication", "target_branch_key"),
            ("native_tool_result_publication", "target_incarnation"),
            ("native_tool_result_publication", "target_attachment_epoch"),
            ("native_tool_result_publication", "dependency_binding_json"),
            ("native_tool_result_current", "event_kind"),
            ("native_tool_result_current", "first_eligible_publication"),
            ("native_tool_result_current", "owner_message_id"),
            ("burn_check_observation", "event_kind"),
            ("burn_check_observation", "observer_binding_json"),
            ("burn_check_candidate_dependency", "event_kind"),
            ("burn_check_candidate_dependency", "dependency_binding_json"),
            ("burn_check_session_attachment", "source_label"),
            ("burn_check_session_attachment", "source_format"),
            ("burn_check_session_attachment", "source_receipt"),
            ("burn_check_session_attachment", "observer_epoch"),
            ("burn_check_session_attachment", "start_publication"),
            ("burn_check_session_attachment", "observation_started_at_ms"),
            ("burn_check_session_attachment", "last_activity_token"),
            ("burn_check_session_attachment", "last_activity_order_json"),
            ("burn_check_session_attachment", "last_activity_at_ms"),
            ("burn_check_session_attachment", "activity_revision"),
            ("burn_check_session_attachment", "idle_deadline_ms"),
            ("burn_check_session_attachment", "idle_emitted_revision"),
            ("burn_check_session_attachment", "idle_publication"),
            ("burn_check_session_attachment", "continuity_eligible"),
            ("burn_check_session_attachment", "session_selection_json"),
            ("native_file_delivery", "source_selection_json"),
            ("opencode_journal_delivery", "source_selection_json"),
            ("opencode_v2_capture", "source_selection_json"),
        ],
    )?;
    ensure!(
        evidence
            == objects(
                connection,
                "public evidence indexes",
                "index",
                &[
                    "native_evidence_publication_target",
                    "native_evidence_publication_unsafe",
                    "native_evidence_current_owner",
                    "burn_check_attachment_idle_due",
                ]
            )?,
        "incomplete public evidence indexes"
    );
    if evidence {
        validate_evidence_publication(connection)?;
        validate_evidence_indexes(connection)?;
    }
    ensure!(
        objects(
            connection,
            "publication guards",
            "trigger",
            &[
                "native_tool_result_publication_no_update",
                "native_tool_result_publication_no_delete",
            ]
        )?,
        "missing publication guards"
    );
    validate_publication_guards(connection)?;
    let selection = columns(
        connection,
        "publication selection",
        &[("native_tool_result_publication", "session_selection_json")],
    )?;
    let served_model = columns(
        connection,
        "usage served model",
        &[("burn_check_decision_usage", "served_model")],
    )?;
    if served_model {
        let sql: String = connection.query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'burn_check_decision_usage'",
            [], |row| row.get(0),
        )?;
        let actual = sql
            .split_once('(')
            .context("missing usage observation definition")?
            .1;
        let column = super::V91
            .split_once("ADD COLUMN ")
            .context("missing compiled usage observation column")?
            .1
            .trim()
            .trim_end_matches(';');
        let expected = format!(
            "attempt_id TEXT PRIMARY KEY NOT NULL,
            usage_known INTEGER NOT NULL CHECK (usage_known IN (0, 1)), {column}) STRICT"
        );
        ensure!(
            normalize_sql(actual) == normalize_sql(&expected),
            "invalid usage observation constraints"
        );
    }
    let runtime_definitions = objects(
        connection,
        "runtime definition registry",
        "table",
        &[
            "burn_check_runtime_definition",
            "burn_check_runtime_definition_state",
        ],
    )?;
    if runtime_definitions {
        validate_runtime_definitions(connection)?;
    }
    Ok(Profile {
        quota,
        capture,
        capture_fields,
        accounting,
        source_metadata,
        runtime_definitions,
        retired,
        decisions,
        evidence,
        selection,
        served_model,
    })
}

fn validate_evidence_publication(connection: &Connection) -> Result<()> {
    let sql: String = connection.query_row(
        "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'native_tool_result_publication'",
        [], |row| row.get(0),
    )?;
    let actual = normalize_sql(
        sql.split_once('(')
            .context("missing publication definition")?
            .1,
    );
    let mut actual = actual
        .strip_suffix(")strict")
        .context("invalid publication storage mode")?
        .to_owned();
    let expected = super::V89
        .split_once("CREATE TABLE native_tool_result_publication_v86 (")
        .context("missing compiled publication definition")?
        .1
        .split_once(") STRICT;")
        .context("missing compiled publication storage mode")?
        .0;
    let mut expected = normalize_sql(expected);
    // Only these metadata columns differ between the accepted evidence layouts.
    for column in [
        "gap_kind TEXT NOT NULL DEFAULT 'invalidated' CHECK (gap_kind IN ('invalidated', 'partial', 'segment_reset'))",
        "session_selection_json TEXT CHECK (session_selection_json IS NULL OR json_valid(session_selection_json))",
    ] {
        let column = format!(",{}", normalize_sql(column));
        actual = actual.replace(&column, "");
        expected = expected.replace(&column, "");
    }
    ensure!(
        actual == expected,
        "invalid public evidence publication constraints"
    );
    Ok(())
}

fn validate_evidence_indexes(connection: &Connection) -> Result<()> {
    for (name, sql) in [
        (
            "native_tool_result_publication_session",
            "CREATE INDEX native_tool_result_publication_session ON native_tool_result_publication (environment_key, agent, session_id, publication_sequence)",
        ),
        (
            "native_tool_result_publication_identity",
            "CREATE INDEX native_tool_result_publication_identity ON native_tool_result_publication (environment_key, agent, session_id, source_format, native_identity, session_incarnation, project_cwd, publication_sequence) WHERE outcome = 'update'",
        ),
        (
            "native_evidence_publication_target",
            "CREATE INDEX native_evidence_publication_target ON native_tool_result_publication (target_check_id, target_scope_key, target_generation, publication_sequence)",
        ),
        (
            "native_evidence_publication_unsafe",
            "CREATE INDEX native_evidence_publication_unsafe ON native_tool_result_publication (environment_key, agent, session_id, source_format, session_incarnation, project_cwd, publication_sequence) WHERE outcome = 'coverage_gap' AND gap_reason IN ('native-unsafe-identity', 'native-unsafe-order', 'native-unsafe-ownership')",
        ),
        (
            "native_tool_result_current_session",
            "CREATE INDEX native_tool_result_current_session ON native_tool_result_current (environment_key, agent, session_id, is_removed)",
        ),
        (
            "native_evidence_current_owner",
            "CREATE INDEX native_evidence_current_owner ON native_tool_result_current (environment_key, agent, session_id, source_format, owner_message_id, native_identity)",
        ),
        (
            "burn_check_attachment_idle_due",
            "CREATE INDEX burn_check_attachment_idle_due ON burn_check_session_attachment(idle_deadline_ms) WHERE observer_epoch IS NOT NULL AND continuity_eligible = 1 AND idle_emitted_revision IS NULL",
        ),
    ] {
        validate_index(connection, name, sql)?;
    }
    Ok(())
}

fn validate_publication_guards(connection: &Connection) -> Result<()> {
    for (name, expected) in [
        (
            "native_tool_result_publication_no_update",
            "CREATE TRIGGER native_tool_result_publication_no_update BEFORE UPDATE ON native_tool_result_publication BEGIN SELECT RAISE(ABORT, 'native tool-result publications are append-only'); END",
        ),
        (
            "native_tool_result_publication_no_delete",
            "CREATE TRIGGER native_tool_result_publication_no_delete BEFORE DELETE ON native_tool_result_publication WHEN EXISTS (SELECT 1 FROM session WHERE environment_key = OLD.environment_key AND agent = OLD.agent AND session_id = OLD.session_id) BEGIN SELECT RAISE(ABORT, 'native tool-result publications are append-only'); END",
        ),
    ] {
        let sql: String = connection.query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1",
            [name],
            |row| row.get(0),
        )?;
        ensure!(
            normalize_sql(&sql) == normalize_sql(expected),
            "invalid publication guard {name}"
        );
    }
    Ok(())
}

fn validate_capture_tables(connection: &Connection) -> Result<()> {
    for (table, expected) in [
        ("opencode_v2_selection", "environment_key,agent,session_id"),
        (
            "opencode_v2_capture",
            "environment_key,agent,session_id,incarnation,source_generation,source_label,cwd,capture_epoch,claim_fence,baseline_sequence,last_sequence,history_gap,lease_expires_at_epoch",
        ),
        (
            "opencode_v2_source_status",
            "environment_key,agent,session_id,incarnation,source_generation,source_label,cwd,code,observed_at_epoch",
        ),
    ] {
        let base_columns: String = connection.query_row(
            "SELECT group_concat(name, ',') FROM (SELECT name FROM pragma_table_info(?1)
                WHERE NOT (?1 = 'opencode_v2_capture' AND name IN
                    ('admitted_source_json', 'retirement_reason', 'source_selection_json')) ORDER BY cid)",
            [table], |row| row.get(0),
        )?;
        ensure!(
            base_columns == expected,
            "invalid capture columns in {table}"
        );
        let key: String = connection.query_row(
            "SELECT group_concat(name, ',') FROM
                (SELECT name FROM pragma_table_info(?1) WHERE pk > 0 ORDER BY pk)",
            [table],
            |row| row.get(0),
        )?;
        ensure!(
            key == "environment_key,agent,session_id",
            "invalid capture key in {table}"
        );
        if table != "opencode_v2_selection" {
            let foreign_key: (i64, i64) = connection.query_row(
                "SELECT COUNT(*), COUNT(DISTINCT id) FROM pragma_foreign_key_list(?1)
                 WHERE \"table\" = 'session' AND \"from\" = \"to\"
                   AND \"from\" IN ('environment_key', 'agent', 'session_id') AND on_delete = 'CASCADE'",
                [table], |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            ensure!(
                foreign_key == (3, 1),
                "invalid capture foreign key in {table}"
            );
        }
    }
    Ok(())
}

fn validate_runtime_definitions(connection: &Connection) -> Result<()> {
    for table in [
        "burn_check_runtime_definition",
        "burn_check_runtime_definition_state",
    ] {
        let sql: String = connection.query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )?;
        let actual = sql
            .split_once('(')
            .context("missing runtime registry definition")?
            .1;
        let expected = super::V86
            .split_once(&format!("CREATE TABLE IF NOT EXISTS {table} ("))
            .context("missing compiled runtime registry definition")?
            .1
            .split_once(';')
            .context("missing runtime registry statement")?
            .0;
        ensure!(
            normalize_sql(actual) == normalize_sql(expected),
            "invalid runtime registry constraints in {table}"
        );
    }
    let state: (i64, bool) = connection.query_row(
        "SELECT COUNT(*), COALESCE(MIN(singleton = 1 AND protocol_version = 1
            AND initialized IN (0, 1)), 0) FROM burn_check_runtime_definition_state",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    ensure!(state == (1, true), "invalid runtime registry state");
    Ok(())
}

fn columns(connection: &Connection, group: &str, fields: &[(&str, &str)]) -> Result<bool> {
    let mut statement =
        connection.prepare("SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)")?;
    let mut count = 0;
    for (table, column) in fields {
        count += usize::from(statement.query_row([table, column], |row| row.get::<_, bool>(0))?);
    }
    ensure!(
        count == 0 || count == fields.len(),
        "incomplete {group} columns"
    );
    Ok(count > 0)
}

fn objects(connection: &Connection, group: &str, kind: &str, names: &[&str]) -> Result<bool> {
    let mut statement = connection
        .prepare("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = ?1 AND name = ?2)")?;
    let mut count = 0;
    for name in names {
        count += usize::from(statement.query_row([kind, name], |row| row.get::<_, bool>(0))?);
    }
    ensure!(count == 0 || count == names.len(), "incomplete {group}");
    Ok(count > 0)
}

fn validate_index(connection: &Connection, name: &str, expected: &str) -> Result<()> {
    let sql: String = connection.query_row(
        "SELECT sql FROM sqlite_schema WHERE type = 'index' AND name = ?1",
        [name],
        |row| row.get(0),
    )?;
    ensure!(
        normalize_sql(&sql) == normalize_sql(expected),
        "invalid migration index {name}"
    );
    Ok(())
}

fn normalize_sql(sql: &str) -> String {
    let mut normalized = String::new();
    let mut quoted = false;
    for character in sql.chars() {
        if character == '\'' {
            quoted = !quoted;
            normalized.push(character);
        } else if quoted {
            normalized.push(character);
        } else if !character.is_whitespace() && character != ';' {
            normalized.extend(character.to_lowercase());
        }
    }
    normalized
}

#[cfg(test)]
mod tests;
