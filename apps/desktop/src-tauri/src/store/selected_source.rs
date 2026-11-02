use super::*;
use antiburn_local::analysis::SourceFormat;

pub(crate) const OPENCODE_V1_SOURCE_SQL: &str =
    "s.environment_key = 'native' AND s.agent = 'opencode'
    AND s.source_kind = 'providerDb' AND s.source_format = '\"open_code_sqlite_v2\"'";
pub(crate) const OPENCODE_V2_SOURCE_SQL: &str =
    "s.environment_key = 'native' AND s.agent = 'opencode'
    AND s.source_kind = 'providerDb' AND s.source_format = '\"open_code_v2_sqlite\"'
    AND s.wsl_distro IS NULL AND s.cwd IS NOT NULL AND s.cwd <> ''
    AND NOT EXISTS(SELECT 1 FROM session_relation r
      WHERE r.environment_key = s.environment_key AND r.agent = s.agent
        AND r.session_id = s.session_id AND r.kind = 'forkParent')
    AND NOT EXISTS(SELECT 1 FROM session_relation r
      WHERE r.environment_key = s.environment_key AND r.agent = s.agent
        AND r.related_id = s.session_id AND r.kind = 'subagent')";

pub(crate) const ROOT_SESSION_SQL: &str = "NOT EXISTS(SELECT 1 FROM session_relation r
    WHERE r.environment_key = s.environment_key AND r.agent = s.agent
      AND r.session_id = s.session_id AND r.kind = 'forkParent')
    AND NOT EXISTS(SELECT 1 FROM session_relation r
      WHERE r.environment_key = s.environment_key AND r.agent = s.agent
        AND r.related_id = s.session_id AND r.kind = 'subagent')";

pub(crate) const LIVE_ACTIVATION_SQL: &str = "EXISTS(SELECT 1 FROM burn_check_activation a
    WHERE a.enabled = 1
      AND json_extract(a.activation_scope_key, '$.environment') = s.environment_key
      AND json_extract(a.activation_scope_key, '$.session_cwd') = s.cwd
      AND json_extract(a.activation_scope_key, '$.run') = 'Live')";

pub(crate) fn opencode_v2_source_label(session_id: &str) -> String {
    format!("opencode:{session_id}:open_code_v2_sqlite")
}

pub(crate) fn native_opencode_sqlite_format(
    key: &SessionKey,
    source_kind: &str,
    source_label: &str,
) -> Option<SourceFormat> {
    if key.environment_key != "native" || key.agent != "opencode" || source_kind != "providerDb" {
        return None;
    }
    if source_label == format!("opencode:{}", key.session_id) {
        Some(SourceFormat::OpenCodeSqliteV1)
    } else if source_label == opencode_v2_source_label(&key.session_id) {
        Some(SourceFormat::OpenCodeSqliteV2)
    } else {
        None
    }
}

pub(super) fn stored_native_opencode_format(
    key: &SessionKey,
    source_kind: &str,
    source_label: &str,
) -> Option<SourceFormat> {
    native_opencode_sqlite_format(key, source_kind, source_label).or_else(|| {
        (key.environment_key == "native"
            && key.agent == "opencode"
            && source_kind == "inline"
            && source_label == format!("opencode:{}", key.session_id))
        .then_some(SourceFormat::OpenCodeJsonl)
    })
}

fn source_is_allowed_in(
    connection: &Connection,
    key: &SessionKey,
    source_format: Option<SourceFormat>,
) -> Result<bool> {
    if key.environment_key != "native"
        || key.agent != "opencode"
        || !matches!(
            source_format,
            Some(SourceFormat::OpenCodeSqliteV1 | SourceFormat::OpenCodeJsonl)
        )
    {
        return Ok(true);
    }
    Ok(!connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM opencode_v2_selection
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3)",
        params![key.environment_key, key.agent, key.session_id],
        |row| row.get::<_, bool>(0),
    )?)
}

pub(super) fn persist_selection_in(connection: &Connection, record: &SessionRecord) -> Result<()> {
    anyhow::ensure!(
        source_is_allowed_in(
            connection,
            &record.key,
            stored_native_opencode_format(&record.key, &record.source_kind, &record.source_label)
        )?,
        "OpenCode V2 selection excludes the retained V1 source"
    );
    if native_opencode_sqlite_format(&record.key, &record.source_kind, &record.source_label)
        == Some(SourceFormat::OpenCodeSqliteV2)
    {
        connection.execute(
            "INSERT INTO opencode_v2_selection (environment_key, agent, session_id)
             VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id
            ],
        )?;
    }
    Ok(())
}

impl Store {
    pub(crate) fn session_source_is_allowed(
        &self,
        key: &SessionKey,
        source_format: SourceFormat,
    ) -> Result<bool> {
        source_is_allowed_in(&self.lock(), key, Some(source_format))
    }
}

#[cfg(test)]
#[path = "selected_source_tests.rs"]
mod tests;
