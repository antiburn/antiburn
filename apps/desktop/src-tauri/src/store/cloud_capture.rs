//! Checked transactions for account-bound requests and stable local root scopes.
//! Request IDs are idempotency keys. Cancellation keeps the record and does not
//! delete spool bytes or remote data.
//! All methods are synchronous and require admitted blocking work. The caller
//! validates current authority. Creating a request also requires explicit consent.

use super::Store;
use antiburn_cloud_capture::{CaptureIdentity, CaptureSourceKind};
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

/// Existing full-content consent format; saved records keep their original version.
pub(crate) const CONSENT_VERSION: &str = "native-full-content-v1";
/// Typed source consent includes proved session-owned in-store blobs, not external reads.
pub(crate) const SOURCE_CONSENT_VERSION: &str = "native-full-content-v2";

/// Native source contract, independent of an installed runtime or live permission.
/// Only absent legacy fields default to Claude; unknown saved values are errors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceKind {
    #[default]
    ClaudeJsonl,
    #[serde(rename = "opencode_sqlite_v2")]
    OpenCodeSqliteV2,
}

impl SourceKind {
    /// Keep legacy Claude JSON shapes unchanged when a request is saved again.
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

/// Exact user-approved source: a Claude project directory or one OpenCode database.
/// Provider and path together bind preferences and consent, not current access.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SourceSelection {
    pub source_kind: SourceKind,
    pub path: String,
}

/// Stable account/local-root source identity, independent of a replaceable access grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RootScope {
    pub path: String,
    pub source_scope_id: String,
    #[serde(default, skip_serializing_if = "SourceKind::is_claude")]
    pub source_kind: SourceKind,
}

/// Root-bound native identity, not a source handle or permission to reopen content.
/// Claude paths are relative to the approved directory. OpenCode paths are the
/// approved database filename relative to its parent, not permission for other files.
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
    /// Derive the exact scoped key used by native candidates without source I/O.
    /// Claude filename layout must agree with its session and optional child ID.
    /// Callers compare this key with the frozen identity; it does not grant access.
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
                URL_SAFE_NO_PAD.encode(&self.native_session_id),
            ),
        };
        Ok(format!("{}/{native_key}", self.source_scope_id))
    }
}

/// Frozen selection identity, not pinned source bytes or future capture authority.
/// Old absolute Claude locators retain their exact shape without path inspection.
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

/// Durable consent-bound selection. Frozen identities do not freeze source bytes;
/// cancellation and grant replacement must not retarget or remove its record.
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
    /// Compare immutable selection inputs. This check does not establish live authority.
    fn matches_roots(&self, roots: &[SourceSelection]) -> bool {
        self.roots
            .iter()
            .map(|root| (root.source_kind, &root.path))
            .eq(roots.iter().map(|root| (root.source_kind, &root.path)))
    }

    /// Check saved typed bindings without opening, inferring, or repairing source paths.
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

/// Read inside the caller's connection or transaction, preserving absence versus corruption.
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

/// Apply the same serialized record limit before every request insert or update.
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
    /// Atomically bind a new idempotency key and stable account/root scopes.
    /// Repeated IDs must match their old selection; failure rolls back new scopes.
    /// Call only after native consent validation, with blocking admission.
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

    /// Read the original account/destination's request without fallback to another owner.
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

    /// Find the latest saved selection matching exact roots, without reading source files.
    /// The caller supplies current verified account context and a sorted root set.
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

    /// Commit frozen candidates only if the original binding still matches.
    /// Completed or cancelled selections stay unchanged on repeated discovery.
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

    /// Persist cancellation for the original owner while retaining all request/source identities.
    /// This checked transaction does not own spool deletion or remote cleanup.
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
mod tests {
    use super::*;

    fn claude_root(path: &str) -> SourceSelection {
        SourceSelection {
            source_kind: SourceKind::ClaudeJsonl,
            path: path.into(),
        }
    }

    #[test]
    fn cloud_provider_wire_values_match_native_candidate_kinds() {
        for (kind, wire) in [
            (CaptureSourceKind::ClaudeJsonl, "\"claude_jsonl\""),
            (
                CaptureSourceKind::OpenCodeSqliteV2,
                "\"opencode_sqlite_v2\"",
            ),
        ] {
            let native = SourceKind::from(kind);
            assert_eq!(serde_json::to_string(&native).unwrap(), wire);
            assert_eq!(serde_json::from_str::<SourceKind>(wire).unwrap(), native);
        }
        assert!(serde_json::from_str::<SourceKind>("\"unknown\"").is_err());
    }

    #[test]
    fn cloud_request_ids_and_account_root_scopes_survive_restart_and_grant_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        let roots = vec![claude_root("/Example/Native")];
        let first = store
            .prepare_cloud_capture(
                "request-one",
                "account-one",
                "personal-one",
                &roots,
                CONSENT_VERSION,
                1,
            )
            .unwrap();
        let next = store
            .prepare_cloud_capture(
                "request-two",
                "account-one",
                "personal-one",
                &roots,
                CONSENT_VERSION,
                2,
            )
            .unwrap();
        assert_eq!(first.roots, next.roots);
        let another = store
            .prepare_cloud_capture(
                "request-three",
                "account-two",
                "personal-two",
                &roots,
                CONSENT_VERSION,
                3,
            )
            .unwrap();
        assert_ne!(first.roots, another.roots);
        assert!(
            store
                .prepare_cloud_capture(
                    "request-one",
                    "account-two",
                    "personal-two",
                    &roots,
                    CONSENT_VERSION,
                    4
                )
                .is_err()
        );
        assert!(
            store
                .prepare_cloud_capture(
                    "request-one",
                    "account-one",
                    "another-destination",
                    &roots,
                    CONSENT_VERSION,
                    4
                )
                .is_err()
        );
        assert!(
            store
                .prepare_cloud_capture(
                    "request-one",
                    "account-one",
                    "personal-one",
                    &[claude_root("/Example/Other")],
                    CONSENT_VERSION,
                    4
                )
                .is_err()
        );
        store.cancel_cloud_capture_request(&first).unwrap();
        drop(store);
        let store = Store::open(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        let repeated = store
            .prepare_cloud_capture(
                "request-one",
                "account-one",
                "personal-one",
                &roots,
                CONSENT_VERSION,
                10,
            )
            .unwrap();
        assert!(repeated.cancelled);
        assert_eq!(repeated.roots, first.roots);
        assert!(
            store
                .cloud_capture_request("request-one", "account-two", "personal-two")
                .is_err()
        );
    }

    #[test]
    fn cloud_request_insert_failure_rolls_back_new_scope_keys() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        store
            .lock()
            .execute_batch(
                "CREATE TRIGGER reject_cloud_request BEFORE INSERT ON cloud_capture_request
             BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
            )
            .unwrap();
        assert!(
            store
                .prepare_cloud_capture(
                    "request",
                    "account",
                    "personal",
                    &[claude_root("/Example/Native")],
                    CONSENT_VERSION,
                    1
                )
                .is_err()
        );
        let scopes: u64 = store
            .lock()
            .query_row("SELECT count(*) FROM cloud_source_scope", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(scopes, 0);
    }

    #[test]
    fn cloud_legacy_saved_requests_keep_exact_json_and_identities_without_source_access() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        let raw = r#"{"request_id":"legacy","account_id":"account","destination_id":"personal","consent_version":"native-full-content-v1","roots":[{"path":"/Example/missing","source_scope_id":"old-scope"}],"sources":[{"identity":{"capture_id":"old-capture","account_id":"account","destination_id":"personal","source_id":"old-scope/session"},"transcript_path":"/Example/missing/session.jsonl"}],"discovery_complete":true,"cancelled":false}"#;
        store
            .lock()
            .execute(
                "INSERT INTO cloud_capture_request VALUES ('legacy', 'account', 'personal', 1, ?1)",
                [raw],
            )
            .unwrap();
        let request = store
            .cloud_capture_request("legacy", "account", "personal")
            .unwrap();
        assert_eq!(serde_json::to_string(&request).unwrap(), raw);
        let roots = [claude_root("/Example/missing")];
        assert_eq!(
            store
                .prepare_cloud_capture("legacy", "account", "personal", &roots, CONSENT_VERSION, 2)
                .unwrap(),
            request
        );
        assert_eq!(
            store
                .current_cloud_capture_request("account", "personal", &roots)
                .unwrap(),
            Some(request.clone())
        );
        assert_eq!(
            store.finish_cloud_discovery(&request, Vec::new()).unwrap(),
            request
        );
        let saved: String = store
            .lock()
            .query_row(
                "SELECT record_json FROM cloud_capture_request WHERE request_id = 'legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(saved, raw);
    }

    #[test]
    fn cloud_request_ids_cannot_change_provider_or_consent_version() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        let roots = [claude_root("/Example/source")];
        let original = store
            .prepare_cloud_capture("request", "account", "personal", &roots, CONSENT_VERSION, 1)
            .unwrap();
        assert!(
            store
                .prepare_cloud_capture(
                    "request",
                    "account",
                    "personal",
                    &roots,
                    SOURCE_CONSENT_VERSION,
                    2
                )
                .is_err()
        );
        let changed = [SourceSelection {
            source_kind: SourceKind::OpenCodeSqliteV2,
            path: roots[0].path.clone(),
        }];
        assert!(
            store
                .prepare_cloud_capture(
                    "request",
                    "account",
                    "personal",
                    &changed,
                    SOURCE_CONSENT_VERSION,
                    2
                )
                .is_err()
        );
        assert!(
            store
                .current_cloud_capture_request("account", "personal", &changed)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .cloud_capture_request("request", "account", "personal")
                .unwrap(),
            original
        );
    }

    #[test]
    fn cloud_unknown_or_retargeted_saved_locators_fail_without_rewriting_records() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        let roots = [SourceSelection {
            source_kind: SourceKind::OpenCodeSqliteV2,
            path: "/Example/opencode.db".into(),
        }];
        let request = store
            .prepare_cloud_capture(
                "request",
                "account",
                "personal",
                &roots,
                SOURCE_CONSENT_VERSION,
                1,
            )
            .unwrap();
        let source = SelectedSource::Located {
            identity: CaptureIdentity {
                capture_id: "capture".into(),
                account_id: "account".into(),
                destination_id: "personal".into(),
                source_id: format!(
                    "{}/opencode/b3BlbmNvZGUuZGI/c2VzX21haW4",
                    request.roots[0].source_scope_id
                ),
            },
            locator: SourceLocator {
                source_kind: SourceKind::OpenCodeSqliteV2,
                source_scope_id: request.roots[0].source_scope_id.clone(),
                relative_path: "opencode.db".into(),
                native_session_id: "ses_main".into(),
                native_agent_id: None,
            },
        };
        let saved = store
            .finish_cloud_discovery(&request, vec![source])
            .unwrap();
        assert_eq!(
            store
                .cloud_capture_request("request", "account", "personal")
                .unwrap(),
            saved
        );
        let raw = serde_json::to_string(&saved).unwrap();
        for changed in [
            raw.replace(
                "\"native_session_id\":\"ses_main\"",
                "\"native_session_id\":\"ses_child\"",
            ),
            raw.replace("opencode_sqlite_v2", "unknown_source"),
            raw.replace(
                "\"relative_path\":\"opencode.db\"",
                "\"relative_path\":\"other.db\"",
            ),
            raw.replace(
                "\"relative_path\":\"opencode.db\"",
                "\"relative_path\":\"../opencode.db\"",
            ),
            raw.replace(
                "\"native_agent_id\":null",
                "\"native_agent_id\":\"persona\"",
            ),
        ] {
            assert_rejected_without_rewrite(&store, &saved, &roots, &changed);
        }
    }

    fn assert_rejected_without_rewrite(
        store: &Store,
        saved: &CaptureRequest,
        roots: &[SourceSelection],
        changed: &str,
    ) {
        store
            .lock()
            .execute(
                "UPDATE cloud_capture_request SET record_json = ?1 WHERE request_id = ?2",
                params![changed, saved.request_id],
            )
            .unwrap();
        assert!(
            store
                .cloud_capture_request(&saved.request_id, &saved.account_id, &saved.destination_id)
                .is_err()
        );
        assert!(
            store
                .current_cloud_capture_request(&saved.account_id, &saved.destination_id, roots)
                .is_err()
        );
        assert!(store.cancel_cloud_capture_request(saved).is_err());
        let unchanged: String = store
            .lock()
            .query_row(
                "SELECT record_json FROM cloud_capture_request WHERE request_id = ?1",
                [&saved.request_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unchanged, changed);
    }

    #[test]
    fn cloud_typed_claude_locators_reject_changed_paths_sessions_and_child_ids_without_rewrite() {
        for (agent, relative_path, native_key) in [
            (None, "session-main.jsonl", "session-main"),
            (
                Some("child"),
                "session-main/subagents/agent-child.jsonl",
                "session-main/subagents/agent-child",
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let store = Store::open_in_memory(directory.path()).unwrap();
            store.initialize_cloud_setup().unwrap();
            let roots = [claude_root("/Example/claude")];
            let request = store
                .prepare_cloud_capture("request", "account", "personal", &roots, CONSENT_VERSION, 1)
                .unwrap();
            let source = SelectedSource::Located {
                identity: CaptureIdentity {
                    capture_id: "frozen-capture".into(),
                    account_id: "account".into(),
                    destination_id: "personal".into(),
                    source_id: format!("{}/{native_key}", request.roots[0].source_scope_id),
                },
                locator: SourceLocator {
                    source_kind: SourceKind::ClaudeJsonl,
                    source_scope_id: request.roots[0].source_scope_id.clone(),
                    relative_path: relative_path.into(),
                    native_session_id: "session-main".into(),
                    native_agent_id: agent.map(str::to_owned),
                },
            };
            let saved = store
                .finish_cloud_discovery(&request, vec![source])
                .unwrap();
            assert_eq!(
                store
                    .cloud_capture_request("request", "account", "personal")
                    .unwrap(),
                saved
            );
            assert_eq!(
                store
                    .current_cloud_capture_request("account", "personal", &roots)
                    .unwrap(),
                Some(saved.clone())
            );
            let raw = serde_json::to_string(&saved).unwrap();
            for changed in [
                raw.replace(
                    &format!("\"relative_path\":\"{relative_path}\""),
                    "\"relative_path\":\"session-other.jsonl\"",
                ),
                raw.replace(
                    "\"native_session_id\":\"session-main\"",
                    "\"native_session_id\":\"session-other\"",
                ),
                raw.replace(
                    &format!(
                        "\"native_agent_id\":{}",
                        serde_json::to_string(&agent).unwrap()
                    ),
                    "\"native_agent_id\":\"other\"",
                ),
            ] {
                assert_ne!(changed, raw);
                assert_rejected_without_rewrite(&store, &saved, &roots, &changed);
            }
        }
    }
}
