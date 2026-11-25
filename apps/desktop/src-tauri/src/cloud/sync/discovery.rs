//! Bounded session discovery and source version checks for Cloud sync.

use crate::{
    cloud::{file_sources, projects},
    store::cloud_capture::{
        SourceKind, SourceSelection,
        sync::{Job, MAX_RECORDED_PROJECT_PATH_BYTES, Policy},
    },
};
use antiburn_cloud_capture::{
    Candidate, Limits, discover_claude, discover_claude_page, discover_opencode,
    discover_opencode_page,
};
use std::path::Path;

/// One admitted candidate with only native recorded project evidence.
pub(super) struct Selection {
    pub(super) candidate: Candidate,
    pub(super) scope: String,
    pub(super) project: String,
    pub(super) observation: String,
}

/// Memory-only position in the current policy's source sweep. A restart repeats
/// the sweep; durable source keys and accepted state prevent duplicate jobs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct DiscoveryCursor {
    pub(super) root: usize,
    pub(super) after: Option<String>,
}

/// Inspect only admitted native source candidates. Project matching uses directory components;
/// paths in tool calls, attachments, and transcript text cannot grant access.
#[cfg(all(test, unix))]
pub(super) fn discover_policy(
    policy: &Policy,
    current: &dyn Fn() -> bool,
) -> Result<Vec<Selection>, String> {
    if !current() {
        return Ok(Vec::new());
    }
    discover_policy_page(
        policy,
        current,
        &mut projects::Cache::default(),
        &DiscoveryCursor::default(),
    )
    .map(|(candidates, _, _)| candidates)
}

/// Apply the desktop source size limit when reading project or version evidence.
pub(in crate::cloud) fn metadata_limits() -> Limits {
    Limits {
        source_bytes: 128 * 1024 * 1024,
        ..Limits::default()
    }
}

/// Share a 512-entry budget across the remaining roots in one sweep page.
/// Use provider source keys, not file age or the desktop activity date range.
/// Keep bad sessions visible without discarding healthy selections or the cursor.
pub(super) fn discover_policy_page(
    policy: &Policy,
    current: &dyn Fn() -> bool,
    cache: &mut projects::Cache,
    cursor: &DiscoveryCursor,
) -> Result<(Vec<Selection>, Option<DiscoveryCursor>, DiscoveryCoverage), String> {
    cache.begin_scan();
    let mut selected = Vec::new();
    let mut discovered = 0usize;
    let mut coverage = DiscoveryCoverage::default();
    for (index, root) in policy.roots.iter().enumerate().skip(cursor.root) {
        if !current() {
            return Err("Cloud account changed.".into());
        }
        let mut verified_folders = Vec::new();
        for binding in &policy.projects {
            if binding.source_kind != root.source_kind || binding.source_path != root.path {
                continue;
            }
            let folder = Path::new(&binding.project_path).to_owned();
            if antiburn_cloud_capture::ProjectSelection::new(vec![folder.clone()]).is_ok() {
                verified_folders.push(folder);
            } else {
                coverage.blocked = true;
            }
        }
        if verified_folders.is_empty() {
            // Report a missing saved root without listing its unapproved sessions.
            coverage.blocked |= Path::new(&root.path).symlink_metadata().is_err();
            continue;
        }
        let Ok(projects) = antiburn_cloud_capture::ProjectSelection::new(verified_folders.clone())
        else {
            coverage.blocked = true;
            continue;
        };
        let page = discover_root_page(
            root.source_kind,
            &root.path,
            (index == cursor.root)
                .then_some(cursor.after.as_deref())
                .flatten(),
            512usize.saturating_sub(discovered),
        );
        let page = match page {
            Ok(page) => page,
            Err(_) => {
                coverage.blocked = true;
                tracing::debug!(
                    event = "cloud_sync_discovery_root_skipped",
                    source_kind = ?root.source_kind,
                    reason = "listing_unavailable"
                );
                continue;
            }
        };
        coverage.partial |= page.partial;
        discovered += page.visited;
        let mut last_claude_key = None;
        for candidate in page.candidates {
            if !current() {
                return Err("Cloud account changed.".into());
            }
            let root_selection = SourceSelection {
                source_kind: root.source_kind,
                path: root.path.clone(),
            };
            let evidence = cache.evidence(
                &policy.account_id,
                &policy.destination_id,
                &root_selection,
                &candidate,
                &metadata_limits(),
            );
            if !current() {
                return Err("Cloud upload controls changed.".into());
            }
            if matches!(evidence, Ok(None)) {
                // The Claude read budget renews next turn. Keep the last
                // visited key so the unread candidate starts the next page.
                return Ok((
                    selected,
                    Some(DiscoveryCursor {
                        root: index,
                        after: last_claude_key,
                    }),
                    coverage,
                ));
            }
            if root.source_kind == SourceKind::ClaudeJsonl {
                last_claude_key = Some(candidate.source_id().to_owned());
            }
            let Ok(Some(evidence)) = evidence else {
                coverage.blocked = true;
                continue;
            };
            coverage.partial |= !evidence.scan_complete();
            if evidence
                .project_paths()
                .iter()
                .map(|path| path.as_os_str().len())
                .sum::<usize>()
                > MAX_RECORDED_PROJECT_PATH_BYTES
            {
                coverage.blocked = true;
                continue;
            }
            match projects.matches(&evidence) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => {
                    coverage.blocked = true;
                    continue;
                }
            }
            let Some(project) = evidence
                .project_paths()
                .iter()
                .find(|path| {
                    verified_folders
                        .iter()
                        .any(|folder| path.starts_with(folder))
                        && antiburn_cloud_capture::ProjectSelection::new(vec![(*path).clone()])
                            .is_ok()
                })
                .map(|path| path.as_path())
                .and_then(Path::to_str)
            else {
                coverage.blocked = true;
                continue;
            };
            if !current() {
                return Err("Cloud upload controls changed.".into());
            }
            if !policy.allows(&root.source_scope_id, project) {
                continue;
            }
            let observation = match candidate_observation(&candidate, &evidence, &projects) {
                Ok(observation) => observation,
                Err(_) => {
                    coverage.blocked = true;
                    continue;
                }
            };
            selected.push(Selection {
                candidate: candidate
                    .in_source_scope(&root.source_scope_id)
                    .map_err(|_| "Could not bind a session source.")?,
                scope: root.source_scope_id.clone(),
                project: project.to_owned(),
                observation,
            });
        }
        if !current() {
            return Err("Cloud account changed.".into());
        }
        if let Some(after) = page.next_cursor {
            return Ok((
                selected,
                Some(DiscoveryCursor {
                    root: index,
                    after: Some(after),
                }),
                coverage,
            ));
        }
        if discovered == 512 && index + 1 < policy.roots.len() {
            return Ok((
                selected,
                Some(DiscoveryCursor {
                    root: index + 1,
                    after: None,
                }),
                coverage,
            ));
        }
    }
    Ok((selected, None, coverage))
}

/// Both flags prevent a complete count. Only `blocked` changes upload status.
#[derive(Default)]
pub(super) struct DiscoveryCoverage {
    pub(super) blocked: bool,
    pub(super) partial: bool,
}

/// Compare a fresh source generation and bounded metadata digest with the fixed
/// job hint. This hint never replaces full project admission or capture checks.
fn candidate_observation(
    candidate: &Candidate,
    evidence: &antiburn_cloud_capture::ProjectEvidence,
    projects: &antiburn_cloud_capture::ProjectSelection,
) -> Result<String, String> {
    if candidate.source_kind() == antiburn_cloud_capture::CaptureSourceKind::OpenCodeSqliteV2 {
        return candidate
            .opencode_content_digest(&metadata_limits(), projects)
            .map(|digest| format!("opencode-native-v1:{digest}"))
            .map_err(|_| "Could not check the approved session's native content.".into());
    }
    #[cfg(unix)]
    if candidate.source_kind() == antiburn_cloud_capture::CaptureSourceKind::CursorIdeComposer {
        return candidate
            .cursor_content_digest(&metadata_limits(), projects)
            .map(|digest| format!("cursor-ide-native-v1:{digest}"))
            .map_err(|_| "Could not check the approved Cursor composer's native content.".into());
    }
    if uses_file_generation(candidate) {
        #[cfg(unix)]
        return antiburn_cloud_capture::file_generation_key(
            &source_file_metadata(candidate)?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>(),
        )
        .map_err(|_| "Could not check the session file generation.".into());
        #[cfg(not(unix))]
        return Err("Session capture is not supported on this platform.".into());
    }
    legacy_file_observation(candidate, evidence)
}

/// A fresh file capture must match the same local version at the spool pin.
fn uses_file_generation(candidate: &Candidate) -> bool {
    use antiburn_cloud_capture::CaptureSourceKind::{
        ClaudeJsonl, CodexJsonl, CursorCliAgentJsonl, PiJsonl,
    };
    matches!(
        candidate.source_kind(),
        ClaudeJsonl | CodexJsonl | CursorCliAgentJsonl | PiJsonl
    )
}

/// Migrate a legacy capacity wait only after its old observation still matches.
/// Read the new generation on both sides of that comparison. The spool checks
/// the same generation again before it retries the reservation.
pub(super) fn check_retry_observation(
    candidate: &Candidate,
    evidence: &antiburn_cloud_capture::ProjectEvidence,
    projects: &antiburn_cloud_capture::ProjectSelection,
    saved: &str,
) -> Result<String, String> {
    let observed = candidate_observation(candidate, evidence, projects)?;
    if observed == saved
        || (uses_file_generation(candidate)
            && !saved.starts_with(antiburn_cloud_capture::FILE_GENERATION_PREFIX)
            && legacy_file_observation(candidate, evidence)? == saved
            && candidate_observation(candidate, evidence, projects)? == observed)
    {
        return Ok(observed);
    }
    Err("The capture source changed. Saved state is kept.".into())
}

/// Keep the original observation format for saved capacity waits.
pub(super) fn legacy_file_observation(
    candidate: &Candidate,
    evidence: &antiburn_cloud_capture::ProjectEvidence,
) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let observation = source_observation(candidate)?;
    let bytes = serde_json::to_vec(&(observation, evidence.source_digest()))
        .map_err(|_| "Could not record a source observation.")?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Read one bounded provider page using stable native keys and fresh source checks.
pub(in crate::cloud) fn discover_root_page(
    kind: SourceKind,
    path: &str,
    after: Option<&str>,
    remaining: usize,
) -> Result<file_sources::Page, String> {
    if remaining == 0 {
        return Err("Session discovery is still loading.".into());
    }
    match kind {
        SourceKind::ClaudeJsonl => discover_claude_page(
            Path::new(path),
            after,
            &Limits {
                discovery_sessions: remaining,
                ..Limits::default()
            },
        )
        .map(|page| file_sources::Page {
            visited: page.candidates.len(),
            candidates: page.candidates,
            next_cursor: page.next_cursor,
            partial: false,
        })
        .map_err(|_| "Could not list the selected Claude source.".into()),
        SourceKind::OpenCodeSqliteV2 => discover_opencode_page(
            Path::new(path),
            after,
            &Limits {
                discovery_sessions: remaining,
                ..Limits::default()
            },
        )
        .map(|page| file_sources::Page {
            visited: page.candidates.len(),
            candidates: page.candidates,
            next_cursor: page.next_cursor,
            partial: false,
        })
        .map_err(|_| {
            "Could not list the selected session source. Check its access and supported format."
                .into()
        }),
        SourceKind::CodexJsonl
        | SourceKind::PiJsonl
        | SourceKind::CursorIdeComposer
        | SourceKind::CursorCliAgentJsonl => file_sources::discover_page(
            kind,
            Path::new(path),
            after,
            &Limits {
                discovery_sessions: remaining,
                ..metadata_limits()
            },
        )
        .map_err(|_| {
            "Could not list the selected session source. Check access and source limits.".into()
        }),
    }
}

/// List one source root for project selection. Reject results beyond the budget.
pub(super) fn discover_root(
    kind: SourceKind,
    path: &str,
    remaining: usize,
) -> Result<Vec<Candidate>, String> {
    let limits = Limits {
        discovery_sessions: remaining.max(1),
        ..Limits::default()
    };
    let candidates = match kind {
        SourceKind::ClaudeJsonl => discover_claude(Path::new(path), &limits),
        SourceKind::OpenCodeSqliteV2 => discover_opencode(Path::new(path), &limits),
        SourceKind::CursorIdeComposer | SourceKind::CursorCliAgentJsonl => {
            file_sources::discover_cursor_sources(kind, Path::new(path), &limits, &|| true)
                .and_then(|page| {
                    if page.partial {
                        Err(antiburn_cloud_capture::Error::UnsupportedSource)
                    } else {
                        Ok(page.candidates)
                    }
                })
        }
        SourceKind::CodexJsonl | SourceKind::PiJsonl => {
            file_sources::discover_page(kind, Path::new(path), None, &limits).and_then(|page| {
                if page.next_cursor.is_some() {
                    Err(antiburn_cloud_capture::Error::Limit)
                } else {
                    Ok(page.candidates)
                }
            })
        }
    }
    .map_err(
        |_| "Could not list the selected session source. Check its access and supported format.",
    )?;
    if candidates.len() > remaining {
        return Err("Choose fewer session sources.".into());
    }
    Ok(candidates)
}

/// Use file generations only as scheduling hints. A periodic full reconcile
/// bounds stale hints; capture still checks admitted handles and exact bytes.
pub(in crate::cloud) fn source_observation(candidate: &Candidate) -> Result<String, String> {
    #[cfg(unix)]
    {
        use sha2::{Digest, Sha256};
        use std::os::unix::fs::MetadataExt;
        let generations: Vec<_> = source_file_metadata(candidate)?
            .into_iter()
            .map(|metadata| {
                metadata.map(|metadata| {
                    (
                        metadata.dev(),
                        metadata.ino(),
                        metadata.len(),
                        metadata.mtime(),
                        metadata.mtime_nsec(),
                        metadata.ctime(),
                        metadata.ctime_nsec(),
                    )
                })
            })
            .collect();
        let bytes = serde_json::to_vec(&generations)
            .map_err(|_| "Could not record a source observation.")?;
        Ok(Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
    #[cfg(not(unix))]
    {
        let _ = candidate;
        Err("Session capture is not supported on this platform.".into())
    }
}

/// Keep optional companion and WAL positions stable for older cache keys.
#[cfg(unix)]
fn source_file_metadata(candidate: &Candidate) -> Result<Vec<Option<std::fs::Metadata>>, String> {
    let mut paths = vec![candidate.source_path().to_owned()];
    match candidate.source_kind() {
        antiburn_cloud_capture::CaptureSourceKind::ClaudeJsonl
            if candidate.native_agent_id().is_some() =>
        {
            paths.push(candidate.source_path().with_extension("meta.json"));
        }
        antiburn_cloud_capture::CaptureSourceKind::OpenCodeSqliteV2 => {
            let mut wal = candidate.source_path().as_os_str().to_owned();
            wal.push("-wal");
            paths.push(wal.into());
        }
        _ => {}
    }
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() && !metadata.is_symlink() => Ok(Some(metadata)),
            Err(error) if index > 0 && error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            _ => Err("Session source metadata is unavailable.".into()),
        })
        .collect()
}

/// Reopen a saved locator only under its current approved source root.
pub(super) fn reopen_waiting_source(
    policy: &Policy,
    job: &Job,
) -> antiburn_cloud_capture::Result<Candidate> {
    use antiburn_cloud_capture::{Error, reopen_claude, reopen_opencode};
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

    let root = policy
        .roots
        .iter()
        .find(|root| root.source_scope_id == job.source_scope_id)
        .ok_or(Error::InvalidIdentity)?;
    let (scope, key) = job
        .identity
        .source_id
        .split_once('/')
        .ok_or(Error::InvalidIdentity)?;
    if scope != root.source_scope_id {
        return Err(Error::InvalidIdentity);
    }
    let path = Path::new(&root.path);
    let limits = metadata_limits();
    let candidate = match root.source_kind {
        SourceKind::ClaudeJsonl => {
            let (session, agent) = match key.split_once("/subagents/agent-") {
                Some((session, agent)) => (session, Some(agent)),
                None => (key, None),
            };
            reopen_claude(path, session, agent, &limits)?
        }
        SourceKind::OpenCodeSqliteV2 => {
            let (_, encoded_id) = key
                .strip_prefix("opencode/")
                .and_then(|key| key.split_once('/'))
                .ok_or(Error::InvalidIdentity)?;
            let id = URL_SAFE_NO_PAD
                .decode(encoded_id)
                .map_err(|_| Error::InvalidIdentity)?;
            let id = std::str::from_utf8(&id).map_err(|_| Error::InvalidIdentity)?;
            reopen_opencode(path, id, &limits)?
        }
        SourceKind::CodexJsonl
        | SourceKind::PiJsonl
        | SourceKind::CursorIdeComposer
        | SourceKind::CursorCliAgentJsonl => {
            file_sources::reopen(root.source_kind, path, key, &limits)?
        }
    }
    .in_source_scope(scope)?;
    if candidate.source_id() != job.identity.source_id {
        return Err(Error::SourceChanged);
    }
    Ok(candidate)
}
