//! Save consent-bound source requests and list native identities only. This host has no
//! reservation or upload transport and must not read native transcript content.

use super::{DestinationBinding, Draft, Folder, auth::Session};
use crate::store::{
    Store,
    cloud_capture::{
        CONSENT_VERSION, CaptureRequest, SOURCE_CONSENT_VERSION, SelectedSource, SourceKind,
        SourceLocator, SourceSelection,
    },
};
use antiburn_cloud_capture::{CaptureIdentity, Limits, Spool, discover_claude, discover_opencode};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Explicit consent for one exact account, personal destination, and root set.
/// V1 is Claude-only. V2 binds providers and includes session-owned in-store blobs;
/// both exclude external attachment files and URL downloads.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Consent {
    version: String,
    account_id: String,
    destination_id: String,
    #[serde(default)]
    selected_roots: Vec<String>,
    #[serde(default)]
    selected_sources: Vec<SourceSelection>,
    full_content: bool,
    attachments: bool,
}

impl Consent {
    /// Retain the exact confirmed version when saving or retrying a request.
    pub fn version(&self) -> &str {
        &self.version
    }
}

/// Saved-request outcome only. Upload unavailability is not payload or acceptance proof.
#[derive(Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SyncResult {
    UploadUnavailable,
    Cancelled,
}

impl From<&CaptureRequest> for SyncResult {
    fn from(request: &CaptureRequest) -> Self {
        if request.cancelled {
            Self::Cancelled
        } else {
            Self::UploadUnavailable
        }
    }
}

/// Restore the saved request ID and safe outcome without resubmitting work.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSync {
    pub request_id: String,
    pub result: SyncResult,
}

impl From<CaptureRequest> for PendingSync {
    fn from(request: CaptureRequest) -> Self {
        Self {
            result: SyncResult::from(&request),
            request_id: request.request_id,
        }
    }
}

/// Serialize access to the library spool and freeze each completed selection.
/// Explicit retries use the saved request ID; logout does not remove its state.
pub struct Host {
    directory: PathBuf,
    spool: Option<Spool>,
}

impl Host {
    /// Save the spool path in memory; do not open storage during UI setup.
    pub fn new(directory: &Path) -> Self {
        Self {
            directory: directory.join("cloud-capture-v1"),
            spool: None,
        }
    }

    /// List source names under admitted blocking work and serialize library spool access.
    /// Freeze completed identities once; retry failed listing with the same request.
    /// The current-account predicate must be cheap and must not read source content.
    /// Account changes stop between roots; a running provider read finishes first.
    pub fn discover(
        &mut self,
        store: &Store,
        request: &CaptureRequest,
        current: impl Fn() -> bool,
    ) -> Result<CaptureRequest, String> {
        if request.discovery_complete || request.cancelled {
            return Ok(request.clone());
        }
        if !current() {
            return Err("Cloud account changed. Your saved request is kept.".into());
        }
        if self.spool.is_none() {
            self.spool = Some(
                Spool::open(&self.directory, Limits::default())
                    .map_err(|_| "Native capture storage is unavailable.")?,
            );
        }
        self.spool
            .as_ref()
            .ok_or("Native capture storage is unavailable.")?
            .entries()
            .map_err(|_| "Could not read saved native captures. Your source request is kept.")?;
        let mut sources = Vec::new();
        for root in &request.roots {
            if !current() {
                return Err("Cloud account changed. Your saved request is kept.".into());
            }
            let remaining = 512usize.saturating_sub(sources.len());
            let limits = Limits {
                // Discovery requires a positive limit, including for an empty root.
                discovery_sessions: remaining.max(1),
                ..Limits::default()
            };
            if request.consent_version != SOURCE_CONSENT_VERSION
                && (request.consent_version != CONSENT_VERSION || !root.source_kind.is_claude())
            {
                return Err("Confirm content consent for the selected source provider.".into());
            }
            let candidates = match root.source_kind {
                SourceKind::ClaudeJsonl => discover_claude(Path::new(&root.path), &limits)
                    .map_err(|_| "Could not list the selected native Claude session folder.")?,
                SourceKind::OpenCodeSqliteV2 => discover_opencode(Path::new(&root.path), &limits)
                    .map_err(|_| "Could not list the selected OpenCode V2 database. Check its access and supported schema.")?,
            };
            if candidates.len() > remaining {
                return Err("Choose fewer native session sources.".into());
            }
            for candidate in candidates {
                let candidate = candidate
                    .in_source_scope(&root.source_scope_id)
                    .map_err(|_| "Could not bind the selected native source.")?;
                let capture_id = URL_SAFE_NO_PAD.encode(Sha256::digest(
                    serde_json::to_vec(&(&request.request_id, candidate.source_id()))
                        .map_err(|_| "Could not bind the selected native source.")?,
                ));
                if SourceKind::from(candidate.source_kind()) != root.source_kind {
                    return Err("The selected source provider changed.".into());
                }
                let base = match root.source_kind {
                    SourceKind::ClaudeJsonl => Path::new(&root.path),
                    SourceKind::OpenCodeSqliteV2 => Path::new(&root.path)
                        .parent()
                        .ok_or("Choose an absolute native database path.")?,
                };
                let relative_path = candidate
                    .source_path()
                    .strip_prefix(base)
                    .ok()
                    .and_then(Path::to_str)
                    .ok_or("Could not bind the selected native source path.")?;
                sources.push(SelectedSource::Located {
                    identity: CaptureIdentity {
                        capture_id,
                        account_id: request.account_id.clone(),
                        destination_id: request.destination_id.clone(),
                        source_id: candidate.source_id().into(),
                    },
                    locator: SourceLocator {
                        source_kind: root.source_kind,
                        source_scope_id: root.source_scope_id.clone(),
                        relative_path: relative_path.into(),
                        native_session_id: candidate.native_session_id().into(),
                        native_agent_id: candidate.native_agent_id().map(str::to_owned),
                    },
                });
            }
        }
        if !current() {
            return Err("Cloud account changed. Your saved request is kept.".into());
        }
        store
            .finish_cloud_discovery(request, sources)
            .map_err(|_| "Could not save the native source selection. Your request is kept.".into())
    }
}

/// Sort consent-relevant roots without silently removing duplicate selections.
pub fn selected_roots(folders: &[Folder]) -> Vec<SourceSelection> {
    let mut roots = folders
        .iter()
        .filter(|folder| folder.selected)
        .map(|folder| SourceSelection {
            source_kind: folder.source_kind,
            path: folder.path.clone(),
        })
        .collect::<Vec<_>>();
    roots.sort();
    roots
}

/// Match current personal authority, saved preferences, and explicit content consent.
/// Canonical filesystem checks can block, so callers must hold worker admission.
pub fn validate_selection(
    session: &Session,
    draft: &Draft,
    folders: &[Folder],
    consent: &Consent,
) -> Result<Vec<SourceSelection>, String> {
    let binding = DestinationBinding {
        account_id: consent.account_id.clone(),
        destination_id: consent.destination_id.clone(),
    };
    let roots = selected_roots(folders);
    let mut consent_roots = match consent.version.as_str() {
        CONSENT_VERSION if consent.selected_sources.is_empty() => consent
            .selected_roots
            .iter()
            .map(|path| SourceSelection {
                source_kind: SourceKind::ClaudeJsonl,
                path: path.clone(),
            })
            .collect::<Vec<_>>(),
        SOURCE_CONSENT_VERSION if consent.selected_roots.is_empty() => {
            consent.selected_sources.clone()
        }
        _ => return Err("Confirm content consent for the selected source provider.".into()),
    };
    consent_roots.sort();
    if !binding.matches(session)
        || !consent.full_content
        || consent.attachments
        || roots.is_empty()
        || roots.len() > 100
        || roots != consent_roots
        || roots != selected_roots(&draft.folders)
        || roots.windows(2).any(|pair| pair[0] == pair[1])
    {
        return Err(
            "Confirm full-content capture for this account, destination, and selected folders."
                .into(),
        );
    }
    for root in &roots {
        let path = Path::new(&root.path);
        if !path.is_absolute()
            || root.path.len() > 4096
            || root.path.chars().any(char::is_control)
            || match root.source_kind {
                SourceKind::ClaudeJsonl => !path.is_dir(),
                SourceKind::OpenCodeSqliteV2 => !path.is_file(),
            }
        {
            return Err(
                "Choose an existing native source with the selected provider and no path aliases."
                    .into(),
            );
        }
        let canonical = path
            .canonicalize()
            .map_err(|_| "Choose an existing native source without path aliases.")?;
        if canonical.to_str() != Some(root.path.as_str()) {
            return Err("Choose an existing native source without path aliases.".into());
        }
    }
    Ok(roots)
}

/// Admit bounded opaque idempotency keys, never filesystem paths or arbitrary renderer text.
pub fn valid_request_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture() -> (
        tempfile::TempDir,
        PathBuf,
        Store,
        super::super::auth::Session,
        Draft,
        Consent,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory
            .path()
            .canonicalize()
            .unwrap()
            .join("native-project");
        fs::create_dir(&root).unwrap();
        fs::write(
            root.join("session-one.jsonl"),
            b"synthetic bytes, deliberately not parsed\n",
        )
        .unwrap();
        let store = Store::open(directory.path()).unwrap();
        store.initialize_cloud_setup().unwrap();
        let session = super::super::tests::session();
        let path = root.to_str().unwrap().to_owned();
        let draft = Draft {
            folders: vec![Folder {
                path: path.clone(),
                selected: true,
                ..Folder::default()
            }],
            ..Draft::default()
        };
        let consent = Consent {
            version: "native-full-content-v1".into(),
            account_id: session.user.id.clone(),
            destination_id: session.workspace.id.clone(),
            selected_roots: vec![path],
            selected_sources: Vec::new(),
            full_content: true,
            attachments: false,
        };
        (directory, root, store, session, draft, consent)
    }

    #[test]
    fn cloud_selected_sources_stay_durable_without_capture_bytes_or_upload_authority() {
        let (directory, root, store, session, draft, consent) = fixture();
        let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
        let request = store
            .prepare_cloud_capture(
                "request",
                &session.user.id,
                &session.workspace.id,
                &roots,
                consent.version(),
                1,
            )
            .unwrap();
        let mut host = Host::new(&directory.path().canonicalize().unwrap());
        let selected = host.discover(&store, &request, || true).unwrap();
        assert!(selected.discovery_complete);
        assert_eq!(selected.sources.len(), 1);
        let SelectedSource::Located { identity, locator } = &selected.sources[0] else {
            panic!("new selections need typed locators");
        };
        assert_eq!(locator.source_kind, SourceKind::ClaudeJsonl);
        assert_eq!(locator.relative_path, "session-one.jsonl");
        assert_eq!(locator.native_session_id, "session-one");
        let source_id = format!("{}/session-one", request.roots[0].source_scope_id);
        assert_eq!(identity.source_id, source_id);
        assert_eq!(
            identity.capture_id,
            URL_SAFE_NO_PAD.encode(Sha256::digest(
                serde_json::to_vec(&("request", &source_id)).unwrap()
            ))
        );
        assert!(matches!(
            SyncResult::from(&selected),
            SyncResult::UploadUnavailable
        ));
        assert!(host.spool.as_ref().unwrap().entries().unwrap().is_empty());
        assert_eq!(
            fs::read(root.join("session-one.jsonl")).unwrap(),
            b"synthetic bytes, deliberately not parsed\n"
        );
        fs::write(
            root.join("later-session.jsonl"),
            b"another synthetic source",
        )
        .unwrap();
        let repeated = host.discover(&store, &selected, || true).unwrap();
        assert_eq!(repeated, selected);
        drop(host);
        drop(store);
        let store = Store::open(directory.path()).unwrap();
        let saved = store
            .cloud_capture_request("request", &session.user.id, &session.workspace.id)
            .unwrap();
        assert_eq!(saved, selected);
    }

    #[test]
    fn cloud_source_consent_rejects_changed_roots_accounts_destinations_and_attachments() {
        let (_directory, _, _store, session, draft, mut consent) = fixture();
        validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
        consent.selected_roots.clear();
        assert!(validate_selection(&session, &draft, &draft.folders, &consent).is_err());
        consent.selected_roots = selected_roots(&draft.folders)
            .into_iter()
            .map(|root| root.path)
            .collect();
        consent.account_id = "another".into();
        assert!(validate_selection(&session, &draft, &draft.folders, &consent).is_err());
        consent.account_id = session.user.id.clone();
        consent.destination_id = "team".into();
        assert!(validate_selection(&session, &draft, &draft.folders, &consent).is_err());
        consent.destination_id = session.workspace.id.clone();
        consent.attachments = true;
        assert!(validate_selection(&session, &draft, &draft.folders, &consent).is_err());
    }

    #[test]
    fn cloud_source_roots_reject_exact_path_aliases_before_scope_creation() {
        let (_directory, root, _store, session, mut draft, mut consent) = fixture();
        for alias in [
            format!("{}/", root.display()),
            format!("{}/.", root.display()),
            format!("{}//native-project", root.parent().unwrap().display()),
        ] {
            draft.folders[0].path = alias.clone();
            consent.selected_roots = vec![alias];
            assert!(validate_selection(&session, &draft, &draft.folders, &consent).is_err());
        }
    }

    #[test]
    fn cloud_spool_has_one_owner_and_account_loss_keeps_saved_requests() {
        let (directory, _, store, session, draft, consent) = fixture();
        let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
        let request = store
            .prepare_cloud_capture(
                "request",
                &session.user.id,
                &session.workspace.id,
                &roots,
                consent.version(),
                1,
            )
            .unwrap();
        let mut host = Host::new(&directory.path().canonicalize().unwrap());
        assert!(host.discover(&store, &request, || false).is_err());
        assert!(
            !store
                .cloud_capture_request("request", &session.user.id, &session.workspace.id)
                .unwrap()
                .discovery_complete
        );
        host.discover(&store, &request, || true).unwrap();
        let next = store
            .prepare_cloud_capture(
                "next-request",
                &session.user.id,
                &session.workspace.id,
                &roots,
                consent.version(),
                2,
            )
            .unwrap();
        let mut second = Host::new(&directory.path().canonicalize().unwrap());
        assert!(second.discover(&store, &next, || true).is_err());
        assert!(
            !store
                .cloud_capture_request("next-request", &session.user.id, &session.workspace.id)
                .unwrap()
                .discovery_complete
        );
    }

    fn authorize_database(draft: &mut Draft, consent: &mut Consent, database: &Path) {
        draft.folders.push(Folder {
            source_kind: SourceKind::OpenCodeSqliteV2,
            path: database.to_str().unwrap().into(),
            selected: true,
            ..Folder::default()
        });
        consent.version = SOURCE_CONSENT_VERSION.into();
        consent.selected_roots.clear();
        consent.selected_sources = selected_roots(&draft.folders);
    }

    #[test]
    fn cloud_claude_child_locators_keep_root_relative_paths_and_independent_identities() {
        let (directory, root, store, session, draft, consent) = fixture();
        let children = root.join("session-one/subagents");
        fs::create_dir_all(&children).unwrap();
        fs::write(
            children.join("agent-child.jsonl"),
            b"synthetic child source",
        )
        .unwrap();
        let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
        let request = store
            .prepare_cloud_capture(
                "children",
                &session.user.id,
                &session.workspace.id,
                &roots,
                consent.version(),
                1,
            )
            .unwrap();
        let mut host = Host::new(&directory.path().canonicalize().unwrap());
        let selected = host.discover(&store, &request, || true).unwrap();
        assert_eq!(selected.sources.len(), 2);
        let SelectedSource::Located { identity, locator } = &selected.sources[1] else {
            panic!("typed child locator expected");
        };
        assert_eq!(
            locator.relative_path,
            "session-one/subagents/agent-child.jsonl"
        );
        assert_eq!(locator.native_session_id, "session-one");
        assert_eq!(locator.native_agent_id.as_deref(), Some("child"));
        assert_eq!(
            identity.source_id,
            format!(
                "{}/session-one/subagents/agent-child",
                request.roots[0].source_scope_id
            )
        );
        assert_eq!(
            fs::read(children.join("agent-child.jsonl")).unwrap(),
            b"synthetic child source"
        );
        assert!(host.spool.as_ref().unwrap().entries().unwrap().is_empty());
    }

    #[test]
    fn cloud_opencode_selection_freezes_each_database_session_without_payload_capture() {
        let (directory, _, store, session, mut draft, mut consent) = fixture();
        let root = directory.path().canonicalize().unwrap();
        let sql = include_str!("../../../../../fixtures/cloud/opencode/current.sql");
        let mut before = Vec::new();
        for name in ["first", "second"] {
            let parent = root.join(name);
            fs::create_dir(&parent).unwrap();
            let database = parent.join("opencode.db");
            rusqlite::Connection::open(&database)
                .unwrap()
                .execute_batch(sql)
                .unwrap();
            before.push((database.clone(), fs::read(&database).unwrap()));
            authorize_database(&mut draft, &mut consent, &database);
        }
        let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
        let request = store
            .prepare_cloud_capture(
                "mixed",
                &session.user.id,
                &session.workspace.id,
                &roots,
                consent.version(),
                1,
            )
            .unwrap();
        let mut host = Host::new(&root);
        let selected = host.discover(&store, &request, || true).unwrap();
        assert_eq!(selected.sources.len(), 9);
        let mut identities = std::collections::BTreeSet::new();
        let mut sessions = std::collections::BTreeMap::<String, usize>::new();
        for source in &selected.sources {
            let SelectedSource::Located { identity, locator } = source else {
                panic!("typed source expected")
            };
            assert!(identities.insert(identity.capture_id.clone()));
            if locator.source_kind == SourceKind::OpenCodeSqliteV2 {
                assert_eq!(locator.relative_path, "opencode.db");
                assert!(locator.native_agent_id.is_none());
                *sessions
                    .entry(locator.native_session_id.clone())
                    .or_default() += 1;
            }
        }
        assert_eq!(
            sessions,
            ["ses_main", "ses_child", "ses_detached", "ses_fork"]
                .into_iter()
                .map(|id| (id.into(), 2))
                .collect()
        );
        assert!(host.spool.as_ref().unwrap().entries().unwrap().is_empty());
        assert!(matches!(
            SyncResult::from(&selected),
            SyncResult::UploadUnavailable
        ));
        for (database, bytes) in before {
            assert_eq!(fs::read(&database).unwrap(), bytes);
            fs::remove_file(database).unwrap();
        }
        assert_eq!(host.discover(&store, &selected, || true).unwrap(), selected);
        drop(host);
        drop(store);
        let store = Store::open(directory.path()).unwrap();
        assert_eq!(
            store
                .cloud_capture_request("mixed", &session.user.id, &session.workspace.id)
                .unwrap(),
            selected
        );
    }

    #[test]
    fn cloud_opencode_dispatch_requires_typed_consent_and_never_falls_back() {
        let (directory, _, store, session, mut draft, mut consent) = fixture();
        let root = directory.path().canonicalize().unwrap();
        let database = root.join("opencode.db");
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY);")
            .unwrap();
        authorize_database(&mut draft, &mut consent, &database);
        consent.version = CONSENT_VERSION.into();
        consent.selected_sources.clear();
        consent.selected_roots = draft
            .folders
            .iter()
            .map(|folder| folder.path.clone())
            .collect();
        assert!(validate_selection(&session, &draft, &draft.folders, &consent).is_err());
        consent.version = SOURCE_CONSENT_VERSION.into();
        consent.selected_roots.clear();
        consent.selected_sources = selected_roots(&draft.folders);
        let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
        let request = store
            .prepare_cloud_capture(
                "typed",
                &session.user.id,
                &session.workspace.id,
                &roots,
                consent.version(),
                1,
            )
            .unwrap();
        let mut host = Host::new(&root);
        assert!(
            host.discover(&store, &request, || true)
                .unwrap_err()
                .contains("OpenCode V2")
        );
        let saved = store
            .cloud_capture_request("typed", &session.user.id, &session.workspace.id)
            .unwrap();
        assert_eq!(saved, request);
        connection
            .execute_batch(include_str!(
                "../../../../../fixtures/cloud/opencode/earlier.sql"
            ))
            .unwrap();
        drop(connection);
        let selected = host.discover(&store, &saved, || true).unwrap();
        assert_eq!(selected.sources.len(), 2);
        assert_eq!(selected.consent_version, SOURCE_CONSENT_VERSION);
    }

    #[test]
    fn cloud_discovery_at_the_source_limit_accepts_empty_roots_but_rejects_more_sources() {
        let (directory, root, store, session, mut draft, mut consent) = fixture();
        for index in 1..512 {
            fs::write(root.join(format!("session-{index}.jsonl")), b"").unwrap();
        }
        let empty = directory.path().canonicalize().unwrap().join("zz-empty");
        fs::create_dir(&empty).unwrap();
        draft.folders.push(Folder {
            path: empty.to_str().unwrap().into(),
            selected: true,
            ..Folder::default()
        });
        consent.selected_roots = selected_roots(&draft.folders)
            .into_iter()
            .map(|root| root.path)
            .collect();
        let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
        let mut host = Host::new(&directory.path().canonicalize().unwrap());
        let request = store
            .prepare_cloud_capture(
                "at-limit",
                &session.user.id,
                &session.workspace.id,
                &roots,
                consent.version(),
                1,
            )
            .unwrap();
        let selected = host.discover(&store, &request, || true).unwrap();
        assert!(selected.discovery_complete);
        assert_eq!(selected.sources.len(), 512);

        fs::write(empty.join("one-more.jsonl"), b"").unwrap();
        let overflow = store
            .prepare_cloud_capture(
                "over-limit",
                &session.user.id,
                &session.workspace.id,
                &roots,
                consent.version(),
                2,
            )
            .unwrap();
        assert!(
            host.discover(&store, &overflow, || true)
                .unwrap_err()
                .contains("Choose fewer")
        );
        assert_eq!(
            store
                .cloud_capture_request("over-limit", &session.user.id, &session.workspace.id)
                .unwrap(),
            overflow
        );
        assert!(host.spool.as_ref().unwrap().entries().unwrap().is_empty());
    }

    #[test]
    fn cloud_mixed_discovery_limit_keeps_the_original_request_without_partial_selection() {
        let (directory, _, store, session, mut draft, mut consent) = fixture();
        let root = directory.path().canonicalize().unwrap();
        let database = root.join("opencode.db");
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch(include_str!(
                "../../../../../fixtures/cloud/opencode/earlier.sql"
            ))
            .unwrap();
        connection
            .execute_batch(
                "WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n < 511)
             INSERT INTO session_v2(id) SELECT 'ses_' || n FROM ids;",
            )
            .unwrap();
        drop(connection);
        authorize_database(&mut draft, &mut consent, &database);
        let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
        let request = store
            .prepare_cloud_capture(
                "bounded",
                &session.user.id,
                &session.workspace.id,
                &roots,
                consent.version(),
                1,
            )
            .unwrap();
        let mut host = Host::new(&root);
        assert!(host.discover(&store, &request, || true).is_err());
        assert_eq!(
            store
                .cloud_capture_request("bounded", &session.user.id, &session.workspace.id)
                .unwrap(),
            request
        );
        assert!(host.spool.as_ref().unwrap().entries().unwrap().is_empty());
    }

    #[test]
    fn cloud_account_invalidation_stops_discovery_before_the_next_provider_root() {
        let (directory, _, store, session, mut draft, mut consent) = fixture();
        let root = directory.path().canonicalize().unwrap();
        let database = root.join("opencode.db");
        fs::write(&database, b"not a database; this provider must not run").unwrap();
        authorize_database(&mut draft, &mut consent, &database);
        let roots = validate_selection(&session, &draft, &draft.folders, &consent).unwrap();
        let request = store
            .prepare_cloud_capture(
                "fenced",
                &session.user.id,
                &session.workspace.id,
                &roots,
                consent.version(),
                1,
            )
            .unwrap();
        let checks = std::sync::atomic::AtomicUsize::new(0);
        let mut host = Host::new(&root);
        let error = host
            .discover(&store, &request, || {
                checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 2
            })
            .unwrap_err();
        assert!(error.contains("Cloud account changed"));
        assert_eq!(checks.load(std::sync::atomic::Ordering::Relaxed), 3);
        assert_eq!(
            store
                .cloud_capture_request("fenced", &session.user.id, &session.workspace.id)
                .unwrap(),
            request
        );
    }
}
