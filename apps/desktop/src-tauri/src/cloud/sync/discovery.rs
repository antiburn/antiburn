//! Bounded session discovery and source version checks for Cloud sync.

use super::{
    repository_capture::{LookupDeferred, OriginProof, RepositoryLookups},
    repository_policy::saved_team_folder_id,
};
use crate::{
    cloud::{file_sources, projects},
    store::cloud_capture::{
        SourceKind, SourceSelection,
        sync::{Job, MAX_RECORDED_PROJECT_PATH_BYTES, Policy},
    },
};
use antiburn_cloud_capture::{
    Candidate, CaptureSourceKind, Limits, ProjectSelection, discover_claude_page,
    discover_opencode_page,
};
#[cfg(all(test, unix))]
use antiburn_cloud_capture::{discover_claude, discover_opencode};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const OPENCODE_DIGEST_SLICE: Duration = Duration::from_millis(250);

/// One admitted candidate with only native recorded project evidence.
pub(super) struct Selection {
    pub(super) candidate: Candidate,
    pub(super) scope: String,
    pub(super) project: String,
    pub(super) observation: String,
    pub(super) repository_origin: Option<String>,
    /// One saved folder rule proved by this discovery page. Never used for consent.
    pub(super) folder_group_id: Option<String>,
}

/// Memory-only position in the current policy's source sweep. A restart repeats
/// the sweep; durable source keys and accepted state prevent duplicate jobs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct DiscoveryCursor {
    pub(super) root: usize,
    pub(super) after: Option<String>,
    pub(super) repository_lookup: Option<RepositoryLookupCursor>,
}

/// Resume a large CWD list without repeating checks that did not grant
/// permission. Restart if the evidence changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RepositoryLookupCursor {
    source_id: String,
    paths: Vec<PathBuf>,
    next_path: usize,
    incomplete: bool,
}

impl RepositoryLookupCursor {
    /// Keep the position and incomplete proof only for the same candidate and paths.
    fn resume(&self, source_id: &str, paths: &[PathBuf]) -> (usize, bool) {
        if self.source_id == source_id && self.paths == paths {
            (self.next_path, self.incomplete)
        } else {
            (0, false)
        }
    }
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
/// Yield between OpenCode exports so other Cloud work can use the worker.
pub(super) fn discover_policy_page(
    policy: &Policy,
    current: &dyn Fn() -> bool,
    cache: &mut projects::Cache,
    cursor: &DiscoveryCursor,
) -> Result<(Vec<Selection>, Option<DiscoveryCursor>, DiscoveryCoverage), String> {
    discover_policy_page_with_clock(policy, current, cache, cursor, Instant::now)
}

/// Read a sweep page. The clock measures the OpenCode digest budget for the turn.
pub(super) fn discover_policy_page_with_clock(
    policy: &Policy,
    current: &dyn Fn() -> bool,
    cache: &mut projects::Cache,
    cursor: &DiscoveryCursor,
    now: impl Fn() -> Instant,
) -> Result<(Vec<Selection>, Option<DiscoveryCursor>, DiscoveryCoverage), String> {
    cache.begin_scan();
    let mut repository_lookups = RepositoryLookups::default();
    let mut selected = Vec::new();
    let mut discovered = 0usize;
    let mut coverage = DiscoveryCoverage::default();
    let mut opencode_digest_deadline = None;
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
            if ProjectSelection::new(vec![folder.clone()]).is_ok() {
                verified_folders.push(folder);
            } else {
                coverage.blocked = true;
            }
        }
        let has_repository_rules = policy
            .repository_rules
            .iter()
            .any(|rule| policy.allows_repository(&root.source_scope_id, &rule.origin));
        if verified_folders.is_empty() && !has_repository_rules {
            // Report a missing saved root without listing its unapproved sessions.
            coverage.blocked |= Path::new(&root.path).symlink_metadata().is_err();
            continue;
        }
        let Ok(projects) = ProjectSelection::new(verified_folders.clone()) else {
            coverage.blocked = true;
            continue;
        };
        let mut last_visited_key = (index == cursor.root)
            .then(|| cursor.after.clone())
            .flatten();
        let pending_lookup = (index == cursor.root)
            .then_some(cursor.repository_lookup.as_ref())
            .flatten();
        let page = discover_root_page(
            root.source_kind,
            &root.path,
            last_visited_key.as_deref(),
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
                        after: last_visited_key,
                        repository_lookup: pending_lookup
                            .filter(|pending| pending.source_id == candidate.source_id())
                            .cloned(),
                    }),
                    coverage,
                ));
            }
            let previous_key = last_visited_key.replace(provider_cursor_key(&candidate).into());
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
            let (project, repository_origin, repository_projects) =
                match projects.matches(&evidence) {
                    Ok(true) => {
                        let Some(project) = evidence
                            .project_paths()
                            .iter()
                            .find(|path| {
                                verified_folders
                                    .iter()
                                    .any(|folder| path.starts_with(folder))
                                    && ProjectSelection::new(vec![(*path).clone()]).is_ok()
                            })
                            .and_then(|path| path.to_str())
                        else {
                            coverage.blocked = true;
                            continue;
                        };
                        if !policy.allows(&root.source_scope_id, project) {
                            continue;
                        }
                        (project, None, None)
                    }
                    Ok(false) if has_repository_rules && evidence.scan_complete() => {
                        let source_id = candidate.source_id();
                        let paths = evidence.project_paths();
                        let (start, mut incomplete) = pending_lookup
                            .map_or((0, false), |pending| pending.resume(source_id, paths));
                        let mut matched = None;
                        for (path_index, path) in paths.iter().enumerate().skip(start) {
                            if !current() {
                                return Err("Cloud upload controls changed.".into());
                            }
                            match repository_lookups.origin(path) {
                                Ok(OriginProof::Verified(origin))
                                    if policy.allows_repository(&root.source_scope_id, &origin) =>
                                {
                                    matched = Some((path, origin));
                                    break;
                                }
                                Ok(OriginProof::Incomplete) => incomplete = true,
                                Ok(OriginProof::Verified(_) | OriginProof::NoOrigin) => {}
                                Err(LookupDeferred) => {
                                    return Ok((
                                        selected,
                                        Some(DiscoveryCursor {
                                            root: index,
                                            after: previous_key,
                                            repository_lookup: Some(RepositoryLookupCursor {
                                                source_id: source_id.into(),
                                                paths: paths.to_vec(),
                                                next_path: path_index,
                                                incomplete,
                                            }),
                                        }),
                                        coverage,
                                    ));
                                }
                            }
                        }
                        let Some((path, origin)) = matched else {
                            coverage.partial |= incomplete;
                            continue;
                        };
                        let (Some(project), Ok(projects)) =
                            (path.to_str(), ProjectSelection::new(vec![path.clone()]))
                        else {
                            coverage.blocked = true;
                            continue;
                        };
                        (project, Some(origin), Some(projects))
                    }
                    Ok(false) => continue,
                    Err(_) => {
                        coverage.blocked = true;
                        continue;
                    }
                };
            if !current() {
                return Err("Cloud upload controls changed.".into());
            }
            if candidate.source_kind() == CaptureSourceKind::OpenCodeSqliteV2 {
                let now = now();
                if now >= *opencode_digest_deadline.get_or_insert(now + OPENCODE_DIGEST_SLICE) {
                    // One export can run past the budget. Keep the first unread
                    // source on the next page instead of reporting a gap.
                    coverage.opencode_slice_yielded = true;
                    return Ok((
                        selected,
                        Some(DiscoveryCursor {
                            root: index,
                            after: previous_key,
                            repository_lookup: pending_lookup
                                .filter(|pending| pending.source_id == candidate.source_id())
                                .cloned(),
                        }),
                        coverage,
                    ));
                }
            }
            let observation = match candidate_observation(
                &candidate,
                &evidence,
                repository_projects.as_ref().unwrap_or(&projects),
            ) {
                Ok(observation) => observation,
                Err(_) => {
                    coverage.blocked = true;
                    continue;
                }
            };
            let folder_group_id = if !evidence.scan_complete() {
                None
            } else if let Some(origin) = &repository_origin {
                // Admission stops at the first matching origin. Do not inspect
                // more checkouts to see if the session also matches another rule.
                (evidence.project_paths().len() == 1
                    || !policy.repository_rules.iter().any(|rule| {
                        rule.origin != *origin && rule.scopes.contains(&root.source_scope_id)
                    }))
                .then(|| projects::repository_group_id(origin))
            } else if has_repository_rules && evidence.project_paths().len() > 1 {
                // The exact route wins without checking the other CWDs for origins.
                None
            } else {
                exact_folder_group_id(&verified_folders, evidence.project_paths())
            };
            selected.push(Selection {
                candidate: candidate
                    .in_source_scope(&root.source_scope_id)
                    .map_err(|_| "Could not bind a session source.")?,
                scope: root.source_scope_id.clone(),
                project: project.to_owned(),
                observation,
                repository_origin,
                folder_group_id,
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
                    repository_lookup: None,
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
                    repository_lookup: None,
                }),
                coverage,
            ));
        }
    }
    Ok((selected, None, coverage))
}

/// Use the saved path, not its descendant CWD. Omit overlapping saved folders.
fn exact_folder_group_id(folders: &[PathBuf], recorded_paths: &[PathBuf]) -> Option<String> {
    let mut matches = folders
        .iter()
        .filter(|folder| recorded_paths.iter().any(|path| path.starts_with(folder)));
    let folder = matches.next()?;
    matches.next().is_none().then_some(())?;
    Some(saved_team_folder_id(folder.to_str()?))
}

/// Return the provider's exclusive pagination key for a completed candidate.
/// Claude includes the child filename in its key. Other providers page by ID.
fn provider_cursor_key(candidate: &Candidate) -> &str {
    match candidate.source_kind() {
        antiburn_cloud_capture::CaptureSourceKind::ClaudeJsonl => candidate.source_id(),
        _ => candidate.native_session_id(),
    }
}

#[cfg(test)]
mod repository_paging_tests {
    use super::*;

    #[test]
    fn repository_lookup_resumes_only_with_unchanged_recorded_paths() {
        let paths = vec![PathBuf::from("/first"), PathBuf::from("/second")];
        let pending = RepositoryLookupCursor {
            source_id: "session-a".into(),
            paths: paths.clone(),
            next_path: 1,
            incomplete: true,
        };

        assert_eq!(pending.resume("session-a", &paths), (1, true));
        assert_eq!(pending.resume("session-b", &paths), (0, false));
        assert_eq!(pending.resume("session-a", &paths[1..]), (0, false));
        assert_eq!(
            pending.resume("session-a", &[PathBuf::from("/new"), paths[1].clone()]),
            (0, false)
        );
    }
}

/// Both flags prevent a complete count. Only `blocked` changes upload status.
#[derive(Default)]
pub(super) struct DiscoveryCoverage {
    pub(super) blocked: bool,
    pub(super) partial: bool,
    pub(super) opencode_slice_yielded: bool,
}

/// Compare a fresh source generation and bounded metadata digest with the fixed
/// job hint. This hint never replaces full project admission or capture checks.
fn candidate_observation(
    candidate: &Candidate,
    evidence: &antiburn_cloud_capture::ProjectEvidence,
    projects: &antiburn_cloud_capture::ProjectSelection,
) -> Result<String, String> {
    if candidate.source_kind() == CaptureSourceKind::OpenCodeSqliteV2 {
        return candidate
            .opencode_content_digest(&metadata_limits(), projects)
            .map(|digest| format!("opencode-native-v1:{digest}"))
            .map_err(|_| "Could not check the approved session's native content.".into());
    }
    #[cfg(unix)]
    if candidate.source_kind() == CaptureSourceKind::CursorIdeComposer {
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

/// Compare paged scheduler discovery with a bounded root listing in tests.
#[cfg(all(test, unix))]
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
