use super::{DestinationBinding, Draft, Folder, auth::Session};
use crate::store::{
    Store,
    cloud_capture::{CONSENT_VERSION, CaptureRequest, SelectedSource},
};
use antiburn_cloud_capture::{CaptureIdentity, Limits, Spool, discover_claude};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Consent {
    version: String,
    account_id: String,
    destination_id: String,
    selected_roots: Vec<String>,
    full_content: bool,
    attachments: bool,
}

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

pub struct Host {
    directory: PathBuf,
    spool: Option<Spool>,
}

impl Host {
    pub fn new(directory: &Path) -> Self {
        Self {
            directory: directory.join("cloud-capture-v1"),
            spool: None,
        }
    }

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
            let limits = Limits {
                discovery_sessions: 512usize.saturating_sub(sources.len()),
                ..Limits::default()
            };
            if limits.discovery_sessions == 0 {
                return Err("Choose fewer native session sources.".into());
            }
            for candidate in discover_claude(Path::new(&root.path), &limits)
                .map_err(|_| "Could not list the selected native Claude session folder.")?
            {
                let candidate = candidate
                    .in_source_scope(&root.source_scope_id)
                    .map_err(|_| "Could not bind the selected native source.")?;
                let capture_id = URL_SAFE_NO_PAD.encode(Sha256::digest(
                    serde_json::to_vec(&(&request.request_id, candidate.source_id()))
                        .map_err(|_| "Could not bind the selected native source.")?,
                ));
                sources.push(SelectedSource {
                    identity: CaptureIdentity {
                        capture_id,
                        account_id: request.account_id.clone(),
                        destination_id: request.destination_id.clone(),
                        source_id: candidate.source_id().into(),
                    },
                    transcript_path: candidate
                        .transcript_path()
                        .to_str()
                        .ok_or("Choose native source paths with valid text.")?
                        .into(),
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

pub fn selected_roots(folders: &[Folder]) -> Vec<String> {
    let mut roots = folders
        .iter()
        .filter(|folder| folder.selected)
        .map(|folder| folder.path.clone())
        .collect::<Vec<_>>();
    roots.sort();
    roots
}

pub fn validate_selection(
    session: &Session,
    draft: &Draft,
    folders: &[Folder],
    consent: &Consent,
) -> Result<Vec<String>, String> {
    let binding = DestinationBinding {
        account_id: consent.account_id.clone(),
        destination_id: consent.destination_id.clone(),
    };
    let roots = selected_roots(folders);
    let mut consent_roots = consent.selected_roots.clone();
    consent_roots.sort();
    if !binding.matches(session)
        || consent.version != CONSENT_VERSION
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
        let path = Path::new(root);
        if !path.is_absolute()
            || root.len() > 4096
            || root.chars().any(char::is_control)
            || !path.is_dir()
        {
            return Err(
                "Choose an existing native Claude source folder without path aliases.".into(),
            );
        }
        let canonical = path
            .canonicalize()
            .map_err(|_| "Choose an existing native Claude source folder without path aliases.")?;
        if canonical.to_str() != Some(root.as_str()) {
            return Err(
                "Choose an existing native Claude source folder without path aliases.".into(),
            );
        }
    }
    Ok(roots)
}

pub fn valid_request_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(all(test, unix))]
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
                1,
            )
            .unwrap();
        let mut host = Host::new(&directory.path().canonicalize().unwrap());
        let selected = host.discover(&store, &request, || true).unwrap();
        assert!(selected.discovery_complete);
        assert_eq!(selected.sources.len(), 1);
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
        consent.selected_roots = selected_roots(&draft.folders);
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
}
