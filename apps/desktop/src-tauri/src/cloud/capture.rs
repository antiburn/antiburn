//! Own the private desktop capture spool and saved one-time source selections.
//! Ongoing uploads use separate explicit consent and the session scheduler.

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
/// V1 is Claude-only. V2 includes session-owned in-store blobs; neither permits
/// external attachment files or URL downloads.
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

/// Saved-request outcome, not payload or integrity acceptance proof.
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

/// Restore a saved request without resubmitting work.
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

/// Serialize spool access and freeze each completed selection. Logout does not
/// remove saved state. Explicit retries retain the original request ID.
pub struct Host {
    directory: PathBuf,
    spool: Option<Spool>,
    limits: Limits,
}

impl Host {
    /// Save the spool path without opening storage during UI setup.
    pub fn new(directory: &Path) -> Self {
        Self {
            directory: directory.join("cloud-capture-v1"),
            spool: None,
            limits: Limits::default(),
        }
    }

    /// Open one process-owned spool only inside admitted blocking work. Discovery
    /// and ongoing uploads share this owner and its single part budget.
    pub(super) fn spool(&mut self) -> Result<&mut Spool, String> {
        if self.spool.is_none() {
            self.spool = Some(
                Spool::open(&self.directory, self.limits.clone())
                    .map_err(|_| "Desktop capture storage is unavailable. Saved bytes are kept.")?,
            );
        }
        self.spool
            .as_mut()
            .ok_or_else(|| "Desktop capture storage is unavailable.".into())
    }

    /// Apply verified service bounds without deleting, rewriting, or recompressing
    /// saved entries. Restore the old bounds if pending entries need them.
    pub(super) fn configure_limits(&mut self, limits: Limits) -> Result<(), String> {
        if self.limits.source_bytes == limits.source_bytes
            && self.limits.capture_bytes == limits.capture_bytes
        {
            return Ok(());
        }
        self.spool = None;
        match Spool::open(&self.directory, limits.clone()) {
            Ok(spool) => {
                self.spool = Some(spool);
                self.limits = limits;
                Ok(())
            }
            Err(_) => {
                self.spool = Some(Spool::open(&self.directory, self.limits.clone()).map_err(
                    |_| "Desktop capture storage is unavailable. Saved bytes are kept.",
                )?);
                Err("Saved captures need their prior limits. Drain or preserve them before new capture.".into())
            }
        }
    }

    /// List names once under blocking admission. Freeze completed identities;
    /// a failed listing can retry with the same request. Account changes stop
    /// between roots; a running provider read finishes first.
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
                    .map_err(|_| "Desktop capture storage is unavailable.")?,
            );
        }
        self.spool
            .as_ref()
            .ok_or("Desktop capture storage is unavailable.")?
            .entries()
            .map_err(|_| "Could not read saved desktop captures. Your source request is kept.")?;
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
                    .map_err(|_| "Could not list the selected Claude session folder.")?,
                SourceKind::OpenCodeSqliteV2 => discover_opencode(Path::new(&root.path), &limits)
                    .map_err(|_| "Could not list the selected OpenCode V2 database. Check its access and supported schema.")?,
                SourceKind::CodexJsonl | SourceKind::PiJsonl => {
                    return Err("Use project sync for this session provider.".into());
                }
            };
            if candidates.len() > remaining {
                return Err("Choose fewer session sources.".into());
            }
            for candidate in candidates {
                let candidate = candidate
                    .in_source_scope(&root.source_scope_id)
                    .map_err(|_| "Could not bind the selected source.")?;
                let capture_id = URL_SAFE_NO_PAD.encode(Sha256::digest(
                    serde_json::to_vec(&(&request.request_id, candidate.source_id()))
                        .map_err(|_| "Could not bind the selected source.")?,
                ));
                if SourceKind::from(candidate.source_kind()) != root.source_kind {
                    return Err("The selected source provider changed.".into());
                }
                let base = if root.source_kind.is_directory() {
                    Path::new(&root.path)
                } else {
                    Path::new(&root.path)
                        .parent()
                        .ok_or("Choose an absolute database path.")?
                };
                let relative_path = candidate
                    .source_path()
                    .strip_prefix(base)
                    .ok()
                    .and_then(Path::to_str)
                    .ok_or("Could not bind the selected source path.")?;
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
            .map_err(|_| "Could not save the source selection. Your request is kept.".into())
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

/// Match personal authority, saved preferences, and explicit content consent.
/// Canonical filesystem checks require blocking admission.
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
        if root.source_kind.is_codex_or_pi() {
            return Err("Use project sync for this session provider.".into());
        }
        if !path.is_absolute()
            || root.path.len() > 4096
            || root.path.chars().any(char::is_control)
            || if root.source_kind.is_directory() {
                !path.is_dir()
            } else {
                !path.is_file()
            }
        {
            return Err(
                "Choose an existing source with the selected provider and no path aliases.".into(),
            );
        }
        let canonical = path
            .canonicalize()
            .map_err(|_| "Choose an existing source without path aliases.")?;
        if canonical.to_str() != Some(root.path.as_str()) {
            return Err("Choose an existing source without path aliases.".into());
        }
    }
    Ok(roots)
}

/// Admit bounded idempotency keys, never paths or arbitrary renderer text.
pub fn valid_request_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(all(test, unix))]
#[path = "tests/capture_integration_tests.rs"]
mod tests;
