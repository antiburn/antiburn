//! Compare Cloud source keys with current local bindings and accepted receipts.

use std::path::Path;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

use crate::checks::SessionIdentity;

/// A match must bind the exact local source, not just its provider ID.
pub(crate) struct ReceiptLookup<'a> {
    connection: &'a Connection,
    workspace_id: &'a str,
    origin: &'a str,
    opencode_path: bool,
}

impl<'a> ReceiptLookup<'a> {
    /// Older or local-only databases can have no receipts or source paths.
    pub(crate) fn new(
        connection: &'a Connection,
        workspace_id: &'a str,
        origin: &'a str,
    ) -> rusqlite::Result<Option<Self>> {
        let available = connection.query_row(
            "SELECT
                (SELECT count(*) FROM sqlite_schema WHERE type='table'
                    AND name IN ('cloud_sync_service_scope', 'cloud_sync_source')) = 2
                AND (SELECT count(*) FROM pragma_table_info('session')
                    WHERE name IN ('source_kind', 'source_label')) = 2,
                (SELECT count(*) FROM pragma_table_info('session')
                    WHERE name IN ('fingerprint_source_path', 'source_format')) = 2",
            [],
            |row| Ok((row.get::<_, bool>(0)?, row.get(1)?)),
        )?;
        Ok(available.0.then_some(Self {
            connection,
            workspace_id,
            origin,
            opencode_path: available.1,
        }))
    }

    /// Return a scoped key only for an exact source path with an accepted receipt.
    /// The receipt and scope must agree on the account, workspace and origin.
    pub(crate) fn source_key(&self, local: &SessionIdentity) -> rusqlite::Result<Option<String>> {
        if local.environment_key != "native" {
            return Ok(None);
        }
        let id = local.session_id.as_str();
        let source = match local.agent.as_str() {
            "claude-code" if safe_claude_id(id) => {
                self.source_path(local, "SELECT source_label FROM session
                    WHERE environment_key=?1 AND agent=?2 AND session_id=?3 AND source_kind='file'")?
                    .and_then(|label| claude_location(&label, id))
            }
            "opencode" if self.opencode_path && !id.is_empty() && id.len() <= 128
                && !id.chars().any(char::is_control) => self
                .source_path(local, "SELECT fingerprint_source_path FROM session
                    WHERE environment_key=?1 AND agent=?2 AND session_id=?3 AND source_kind='providerDb'
                      AND source_format IN ('\"open_code_sqlite_v2\"', '\"open_code_v2_sqlite\"')")?
                .and_then(|path| opencode_location(path, id)),
            _ => None,
        };
        let Some(source) = source else {
            return Ok(None);
        };
        self.accepted_key(source)
    }

    fn source_path(
        &self,
        identity: &SessionIdentity,
        sql: &str,
    ) -> rusqlite::Result<Option<String>> {
        use rusqlite::OptionalExtension;

        self.connection
            .query_row(
                sql,
                params![
                    identity.environment_key,
                    identity.agent,
                    identity.session_id
                ],
                |row| row.get(0),
            )
            .optional()
            .map(Option::flatten)
    }

    fn accepted_key(&self, source: SourceLocation) -> rusqlite::Result<Option<String>> {
        // Desktop keys accepted source identity by resource, not by its current grant.
        let mut query = self.connection.prepare_cached(
            "SELECT receipt.source_id
               FROM cloud_sync_service_scope AS scope
               JOIN cloud_sync_source AS receipt ON receipt.account_id=scope.account_id
                AND receipt.destination_id=scope.destination_id
                AND receipt.source_id=scope.source_scope_id || '/' || ?4
              WHERE scope.destination_id=?1 AND scope.resource_origin=?2
                AND scope.source_kind=?3 AND scope.root_path=?5
                AND json_extract(receipt.record_json, '$.resource_origin')=scope.resource_origin
                AND json_extract(receipt.record_json, '$.fingerprint.account_id')=scope.account_id
                AND json_extract(receipt.record_json, '$.fingerprint.destination_id')=scope.destination_id
                AND json_extract(receipt.record_json, '$.fingerprint.source_id')=receipt.source_id
                AND length(json_extract(receipt.record_json, '$.fingerprint.source_fingerprint'))=64
                AND json_type(receipt.record_json, '$.observation_key')='text'
                AND length(CAST(json_extract(receipt.record_json, '$.observation_key') AS BLOB))
                    BETWEEN 1 AND 4096
                AND json_extract(receipt.record_json, '$.suppressed')=0
              LIMIT 2",
        )?;
        let mut rows = query.query(params![
            self.workspace_id,
            self.origin,
            source.kind,
            source.native_key,
            source.root,
        ])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let source_id: String = row.get(0)?;
        if rows.next()?.is_some() {
            return Ok(None);
        }
        let digest = Sha256::digest(source_id.as_bytes());
        Ok(Some(
            digest.iter().map(|byte| format!("{byte:02x}")).collect(),
        ))
    }
}

struct SourceLocation {
    kind: &'static str,
    root: String,
    native_key: String,
}

fn claude_location(label: &str, id: &str) -> Option<SourceLocation> {
    let path = absolute_source_path(label)?;
    if path.file_stem()?.to_str()? != id || path.extension()?.to_str()? != "jsonl" {
        return None;
    }
    let parent = path.parent()?;
    let (root, native_key) =
        if parent.file_name().and_then(|name| name.to_str()) == Some("subagents") {
            let session_path = parent.parent()?;
            let session_id = session_path.file_name()?.to_str()?;
            if !safe_claude_id(session_id) || !id.starts_with("agent-") {
                return None;
            }
            (
                session_path.parent()?,
                format!("{session_id}/subagents/{id}"),
            )
        } else if id != "audit" {
            (parent, id.to_owned())
        } else {
            return None;
        };
    Some(SourceLocation {
        kind: "\"claude_jsonl\"",
        root: root.to_str()?.into(),
        native_key,
    })
}

fn opencode_location(root: String, id: &str) -> Option<SourceLocation> {
    let database = absolute_source_path(&root)?.file_name()?.to_str()?;
    if database.len() > 128 {
        return None;
    }
    let native_key = format!(
        "opencode/{}/{}",
        URL_SAFE_NO_PAD.encode(database),
        URL_SAFE_NO_PAD.encode(id)
    );
    Some(SourceLocation {
        kind: "\"opencode_sqlite_v2\"",
        root,
        native_key,
    })
}

fn absolute_source_path(value: &str) -> Option<&Path> {
    let path = Path::new(value);
    (value.len() <= 4096
        && !value.chars().any(char::is_control)
        && path.is_absolute()
        && !value
            .split('/')
            .any(|segment| matches!(segment, "." | "..")))
    .then_some(path)
}

fn safe_claude_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
