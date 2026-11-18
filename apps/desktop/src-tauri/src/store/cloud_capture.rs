//! Checked transactions for account-bound requests and stable local root scopes.
//! Request IDs are idempotency keys. Cancellation retains records and does not
//! delete spool bytes or remote data. All methods require blocking admission.
//! The caller validates current authority and explicit consent before creation.

use super::Store;
use antiburn_cloud_capture::{CaptureIdentity, CaptureSourceKind};
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

#[path = "cloud_projects.rs"]
pub(crate) mod projects;
#[path = "cloud_sync.rs"]
pub(crate) mod sync;

/// Existing full-content consent format; saved records keep this exact version.
pub(crate) const CONSENT_VERSION: &str = "native-full-content-v1";
/// Typed source consent includes session-owned in-store blobs, not external reads.
pub(crate) const SOURCE_CONSENT_VERSION: &str = "native-full-content-v2";

/// Original provider source format, not an installed runtime or live permission.
/// Only absent legacy fields default to Claude; unknown values remain errors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceKind {
    #[default]
    ClaudeJsonl,
    #[serde(rename = "opencode_sqlite_v2")]
    OpenCodeSqliteV2,
}

impl SourceKind {
    /// Keep legacy Claude JSON unchanged when a request is saved again.
    pub(crate) fn is_claude(&self) -> bool {
        *self == Self::ClaudeJsonl
    }
}

impl From<CaptureSourceKind> for SourceKind {
    fn from(value: CaptureSourceKind) -> Self {
        match value {
            CaptureSourceKind::ClaudeJsonl => Self::ClaudeJsonl,
            CaptureSourceKind::OpenCodeSqliteV2 => Self::OpenCodeSqliteV2,
        }
    }
}

/// Exact approved source. Provider and path bind consent, not current access.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceSelection {
    pub source_kind: SourceKind,
    pub path: String,
}

/// Stable account/local-root identity, separate from replaceable access grants.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RootScope {
    pub path: String,
    pub source_scope_id: String,
    #[serde(default, skip_serializing_if = "SourceKind::is_claude")]
    pub source_kind: SourceKind,
}

/// Root-bound provider identity, not a source handle or permission to reopen it.
/// Claude paths are root-relative. OpenCode keeps only the approved database
/// filename relative to its parent, without access to other files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceLocator {
    pub source_kind: SourceKind,
    pub source_scope_id: String,
    pub relative_path: String,
    pub native_session_id: String,
    pub native_agent_id: Option<String>,
}

impl SourceLocator {
    /// Derive the scoped source key without I/O. Claude layout and native IDs
    /// must agree. This comparison never grants source access.
    fn source_id(&self) -> Result<String> {
        let native_key = match self.source_kind {
            SourceKind::ClaudeJsonl => {
                let valid_id = |value: &str| {
                    !value.is_empty()
                        && value.len() <= 128
                        && value
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                };
                ensure!(
                    valid_id(&self.native_session_id),
                    "Cloud Claude session ID is invalid"
                );
                let key = match &self.native_agent_id {
                    None => self.native_session_id.clone(),
                    Some(agent) => {
                        let filename_id = format!("agent-{agent}");
                        ensure!(
                            !agent.is_empty() && valid_id(&filename_id),
                            "Cloud Claude child ID is invalid"
                        );
                        format!("{}/subagents/{filename_id}", self.native_session_id)
                    }
                };
                ensure!(
                    self.relative_path == format!("{key}.jsonl"),
                    "Cloud Claude locator changed"
                );
                key
            }
            SourceKind::OpenCodeSqliteV2 => format!(
                "opencode/{}/{}",
                URL_SAFE_NO_PAD.encode(&self.relative_path),
                URL_SAFE_NO_PAD.encode(&self.native_session_id)
            ),
        };
        Ok(format!("{}/{native_key}", self.source_scope_id))
    }
}

/// Frozen selection, not pinned bytes or future authority. Legacy absolute
/// Claude locators retain their exact shape without path inspection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub(crate) enum SelectedSource {
    Located {
        identity: CaptureIdentity,
        locator: SourceLocator,
    },
    LegacyClaude {
        identity: CaptureIdentity,
        transcript_path: String,
    },
}

/// Durable consent-bound selection. Grant replacement and cancellation cannot
/// retarget frozen identities or remove the record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CaptureRequest {
    pub request_id: String,
    pub account_id: String,
    pub destination_id: String,
    pub consent_version: String,
    pub roots: Vec<RootScope>,
    pub sources: Vec<SelectedSource>,
    pub discovery_complete: bool,
    pub cancelled: bool,
}

impl CaptureRequest {
    /// Compare immutable inputs without establishing live authority.
    fn matches_roots(&self, roots: &[SourceSelection]) -> bool {
        self.roots
            .iter()
            .map(|root| (root.source_kind, &root.path))
            .eq(roots.iter().map(|root| (root.source_kind, &root.path)))
    }

    /// Check typed bindings without opening, inferring, or repairing paths.
    fn validate(&self) -> Result<()> {
        ensure!(
            self.consent_version == SOURCE_CONSENT_VERSION
                || (self.consent_version == CONSENT_VERSION
                    && self.roots.iter().all(|root| root.source_kind.is_claude())),
            "Cloud request has unsupported consent"
        );
        for source in &self.sources {
            let (identity, locator) = match source {
                SelectedSource::Located { identity, locator } => (identity, Some(locator)),
                SelectedSource::LegacyClaude { identity, .. } => {
                    ensure!(
                        self.roots.iter().all(|root| root.source_kind.is_claude()),
                        "Legacy Cloud source has a changed provider"
                    );
                    (identity, None)
                }
            };
            ensure!(
                identity.account_id == self.account_id
                    && identity.destination_id == self.destination_id,
                "Cloud source binding changed"
            );
            if let Some(locator) = locator {
                let root = self
                    .roots
                    .iter()
                    .find(|root| root.source_scope_id == locator.source_scope_id)
                    .ok_or_else(|| anyhow::anyhow!("Cloud source root is unavailable"))?;
                let path = Path::new(&locator.relative_path);
                ensure!(
                    root.source_kind == locator.source_kind
                        && !locator.relative_path.is_empty()
                        && locator.relative_path.len() <= 4096
                        && !locator.relative_path.chars().any(char::is_control)
                        && !locator
                            .relative_path
                            .split('/')
                            .any(|part| part.is_empty() || part == "." || part == "..")
                        && path
                            .components()
                            .all(|part| matches!(part, Component::Normal(_)))
                        && !locator.native_session_id.is_empty()
                        && locator.native_session_id.len() <= 128
                        && !locator.native_session_id.chars().any(char::is_control),
                    "Cloud source locator is invalid"
                );
                if locator.source_kind == SourceKind::OpenCodeSqliteV2 {
                    ensure!(
                        locator.native_agent_id.is_none()
                            && Path::new(&root.path).file_name() == Some(path.as_os_str()),
                        "Cloud database locator changed"
                    );
                }
                ensure!(
                    identity.source_id == locator.source_id()?,
                    "Cloud source identity changed"
                );
            }
        }
        Ok(())
    }
}

/// Preserve absence versus corruption inside a caller's connection or transaction.
fn read_in(connection: &Connection, id: &str) -> Result<Option<CaptureRequest>> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT record_json FROM cloud_capture_request WHERE request_id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| {
        let request: CaptureRequest = serde_json::from_str(&raw)?;
        request.validate()?;
        Ok(request)
    })
    .transpose()
}

/// Apply the same record limit before every insert and update.
fn encoded(request: &CaptureRequest) -> Result<String> {
    request.validate()?;
    let raw = serde_json::to_string(request)?;
    ensure!(
        raw.len() <= 1_048_576,
        "Cloud request exceeds its size limit"
    );
    Ok(raw)
}

impl Store {
    /// Bind a new idempotency key and root scopes atomically after desktop
    /// consent validation. An insert failure rolls back newly created scopes.
    pub(crate) fn prepare_cloud_capture(
        &self,
        request_id: &str,
        account: &str,
        destination: &str,
        roots: &[SourceSelection],
        consent_version: &str,
        now: i64,
    ) -> Result<CaptureRequest> {
        ensure!(
            !roots.is_empty() && roots.len() <= 100,
            "Choose bounded Cloud sources"
        );
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        if let Some(saved) = read_in(&transaction, request_id)? {
            ensure!(
                saved.account_id == account
                    && saved.destination_id == destination
                    && saved.consent_version == consent_version
                    && saved.matches_roots(roots),
                "Cloud request is already bound to another selection"
            );
            return Ok(saved);
        }
        let count: u64 =
            transaction.query_row("SELECT count(*) FROM cloud_capture_request", [], |row| {
                row.get(0)
            })?;
        ensure!(count < 128, "Cloud request storage is full");
        let mut scopes = Vec::new();
        for root in roots {
            let path = &root.path;
            let saved: Option<String> = transaction.query_row(
                "SELECT source_scope_id FROM cloud_source_scope WHERE account_id = ?1 AND root_path = ?2",
                params![account, path], |row| row.get(0),
            ).optional()?;
            let source_scope_id = match saved {
                Some(saved) => saved,
                None => {
                    let count: u64 = transaction.query_row(
                        "SELECT count(*) FROM cloud_source_scope",
                        [],
                        |row| row.get(0),
                    )?;
                    ensure!(count < 1024, "Cloud source storage is full");
                    let mut bytes = [0; 32];
                    getrandom::fill(&mut bytes)
                        .map_err(|_| anyhow::anyhow!("Cloud source key is unavailable"))?;
                    let id = URL_SAFE_NO_PAD.encode(bytes);
                    transaction.execute(
                        "INSERT INTO cloud_source_scope(account_id, root_path, source_scope_id) VALUES (?1, ?2, ?3)",
                        params![account, path, id],
                    )?;
                    id
                }
            };
            scopes.push(RootScope {
                path: path.clone(),
                source_scope_id,
                source_kind: root.source_kind,
            });
        }
        let request = CaptureRequest {
            request_id: request_id.into(),
            account_id: account.into(),
            destination_id: destination.into(),
            consent_version: consent_version.into(),
            roots: scopes,
            sources: Vec::new(),
            discovery_complete: false,
            cancelled: false,
        };
        transaction.execute(
            "INSERT INTO cloud_capture_request(request_id, account_id, destination_id, created_at, record_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![request_id, account, destination, now, encoded(&request)?],
        )?;
        transaction.commit()?;
        Ok(request)
    }

    /// Read the original owner's request without another-account fallback.
    pub(crate) fn cloud_capture_request(
        &self,
        id: &str,
        account: &str,
        destination: &str,
    ) -> Result<CaptureRequest> {
        let request = read_in(&self.lock(), id)?
            .ok_or_else(|| anyhow::anyhow!("Cloud request is unavailable"))?;
        ensure!(
            request.account_id == account && request.destination_id == destination,
            "Cloud request is unavailable"
        );
        Ok(request)
    }

    /// Find the latest exact selection without source reads. The caller supplies
    /// verified account context and sorted roots.
    pub(crate) fn current_cloud_capture_request(
        &self,
        account: &str,
        destination: &str,
        roots: &[SourceSelection],
    ) -> Result<Option<CaptureRequest>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT record_json FROM cloud_capture_request WHERE account_id = ?1 AND destination_id = ?2
             ORDER BY created_at DESC, request_id DESC LIMIT 128",
        )?;
        for row in
            statement.query_map(params![account, destination], |row| row.get::<_, String>(0))?
        {
            let request: CaptureRequest = serde_json::from_str(&row?)?;
            request.validate()?;
            if request.matches_roots(roots) {
                return Ok(Some(request));
            }
        }
        Ok(None)
    }

    /// Freeze candidates only while the original binding matches. Repeated
    /// discovery cannot change a completed or cancelled selection.
    pub(crate) fn finish_cloud_discovery(
        &self,
        request: &CaptureRequest,
        sources: Vec<SelectedSource>,
    ) -> Result<CaptureRequest> {
        ensure!(
            sources.len() <= 512,
            "Cloud source selection exceeds its limit"
        );
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut saved = read_in(&transaction, &request.request_id)?
            .ok_or_else(|| anyhow::anyhow!("Cloud request is unavailable"))?;
        ensure!(
            saved.account_id == request.account_id
                && saved.destination_id == request.destination_id
                && saved.consent_version == request.consent_version
                && saved.roots == request.roots,
            "Cloud request changed"
        );
        if saved.discovery_complete || saved.cancelled {
            return Ok(saved);
        }
        saved.sources = sources;
        saved.discovery_complete = true;
        transaction.execute(
            "UPDATE cloud_capture_request SET record_json = ?2 WHERE request_id = ?1",
            params![saved.request_id, encoded(&saved)?],
        )?;
        transaction.commit()?;
        Ok(saved)
    }

    /// Persist cancellation while retaining identities. This transaction does
    /// not own spool deletion or remote cleanup.
    pub(crate) fn cancel_cloud_capture_request(
        &self,
        request: &CaptureRequest,
    ) -> Result<CaptureRequest> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut saved = read_in(&transaction, &request.request_id)?
            .ok_or_else(|| anyhow::anyhow!("Cloud request is unavailable"))?;
        ensure!(
            saved.account_id == request.account_id
                && saved.destination_id == request.destination_id,
            "Cloud request is unavailable"
        );
        saved.cancelled = true;
        transaction.execute(
            "UPDATE cloud_capture_request SET record_json = ?2 WHERE request_id = ?1",
            params![saved.request_id, encoded(&saved)?],
        )?;
        transaction.commit()?;
        Ok(saved)
    }
}

#[cfg(test)]
#[path = "tests/cloud_capture_integration_tests.rs"]
mod tests;
