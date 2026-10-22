use super::*;
use antiburn_local::analysis::SourceFormat;

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
    } else if source_label == format!("opencode:{}:open_code_v2_sqlite", key.session_id) {
        Some(SourceFormat::OpenCodeSqliteV2)
    } else {
        None
    }
}

fn stored_native_opencode_format(
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
