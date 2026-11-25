//! Cached project suggestions and current local worktree grouping.
//! Grouping does not prove historical ownership or permit upload. Desktop capture
//! must obtain fresh recorded-CWD admission under separate saved consent.

use super::{
    Controller, DestinationBinding, Draft, Folder, now,
    repository_checkout::{CheckoutOrigin, CheckoutOriginCache},
    repository_origin::legacy_repository_origin_for_off_choice,
    sync,
    work::Priority,
};
use crate::store::{
    Store,
    cloud_capture::{
        SourceKind, SourceSelection,
        projects::{Cursor, Suggestion},
        sync::ProjectSelection,
    },
};
use antiburn_local::platform::process::{
    BoundedOutput, BoundedOutputOptions, BoundedOutputPolicy,
    bounded_headless_output_with_stdin_and_policy,
};
use serde::Serialize;
use std::time::{Duration, Instant};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::Ordering,
};
use tauri::Manager;

pub(super) use crate::store::cloud_capture::sync::repository_group_id;

#[path = "project_listing.rs"]
mod listing;
use listing::{ListingLookups, ListingProgress};

const CACHE_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_CONTEXT_ENTRIES: usize = 512;
const PROJECT_ORIGIN_LOOKUP_BUDGET: Duration = Duration::from_secs(2);
const PROJECT_LISTING_BUDGET: Duration = Duration::from_secs(3);
const PROJECT_DISCOVERY_INCOMPLETE: &str =
    "Project discovery is incomplete. Restore access or choose fewer sources and retry.";
const PROJECT_SCAN_CANDIDATES: usize = 4096;
const PROJECT_SCAN_PAGE_CANDIDATES: usize = 512;
const CLAUDE_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const CLAUDE_FIRST_SOURCE_BYTES: u64 = 128 * 1024 * 1024;

/// Read-only scope copied from the current Cloud snapshot. It cannot grant consent.
pub(super) struct ListingAuthority {
    pub(super) binding: DestinationBinding,
    pub(super) generation: u64,
    pub(super) expires_at: i64,
}

impl Clone for ListingAuthority {
    fn clone(&self) -> Self {
        Self {
            binding: DestinationBinding {
                account_id: self.binding.account_id.clone(),
                destination_id: self.binding.destination_id.clone(),
            },
            generation: self.generation,
            expires_at: self.expires_at,
        }
    }
}

impl ListingAuthority {
    fn matches(&self, binding: &DestinationBinding, generation: u64, checked_at: i64) -> bool {
        self.generation == generation
            && self.expires_at > checked_at
            && self.binding.account_id == binding.account_id
            && self.binding.destination_id == binding.destination_id
    }
}

struct CachedEvidence {
    observation: String,
    evidence: antiburn_cloud_capture::ProjectEvidence,
    bytes: usize,
    observed_at: Instant,
}

/// Bounded local metadata only. Cached evidence can suggest a capture, but new
/// captures still perform fresh admission and a full source check after reserve.
#[derive(Default)]
pub(super) struct Cache {
    evidence: BTreeMap<String, CachedEvidence>,
    listing_progress: Option<ListingProgress>,
    bytes: usize,
    scan_bytes: u64,
}

impl Cache {
    pub(super) fn begin_scan(&mut self) {
        self.scan_bytes = 0;
    }

    /// The first uncached Claude transcript can use the full per-source limit.
    /// After that read, only the remaining shared scan budget is available.
    pub(super) fn evidence(
        &mut self,
        account: &str,
        destination: &str,
        root: &SourceSelection,
        candidate: &antiburn_cloud_capture::Candidate,
        limits: &antiburn_cloud_capture::Limits,
    ) -> Result<Option<antiburn_cloud_capture::ProjectEvidence>, String> {
        use antiburn_cloud_capture::CaptureSourceKind::{CursorCliAgentJsonl, CursorIdeComposer};
        if matches!(
            candidate.source_kind(),
            CursorCliAgentJsonl | CursorIdeComposer
        ) {
            // Cursor binds proof across files. One file's generation cannot
            // prove that its companion or workspace metadata is unchanged.
            return candidate
                .project_evidence_with_limits(limits)
                .map(Some)
                .map_err(|_| "Could not read recorded project metadata.".into());
        }
        let key = group_id(
            account,
            &serde_json::json!([destination, root, candidate.source_id()]).to_string(),
        );
        let observation = sync::source_observation(candidate)?;
        if let Some(saved) = self.evidence.get(&key)
            && saved.observation == observation
            && saved.observed_at.elapsed() < Duration::from_secs(3600)
        {
            return Ok(Some(saved.evidence.clone()));
        }
        let claude_limits;
        let limits = if candidate.source_kind()
            == antiburn_cloud_capture::CaptureSourceKind::ClaudeJsonl
        {
            let bytes = std::fs::symlink_metadata(candidate.source_path())
                .map_err(|_| "Session source metadata is unavailable.")?
                .len();
            if bytes > limits.source_bytes.min(CLAUDE_FIRST_SOURCE_BYTES) {
                return Err("Could not read recorded project metadata.".into());
            }
            let bytes = bytes.max(1);
            if self.scan_bytes != 0 && bytes > CLAUDE_SCAN_BYTES.saturating_sub(self.scan_bytes) {
                return Ok(None);
            }
            self.scan_bytes += bytes;
            // The reader checks the opened file size before streaming. If the
            // file grows, it cannot read more than this reservation.
            claude_limits = antiburn_cloud_capture::Limits {
                source_bytes: bytes,
                ..limits.clone()
            };
            &claude_limits
        } else {
            limits
        };
        let evidence = candidate
            .project_evidence_with_limits(limits)
            .map_err(|_| "Could not read recorded project metadata.")?;
        if sync::source_observation(candidate)? != observation {
            return Err("Session source changed during project discovery.".into());
        }
        if let Some(previous) = self.evidence.remove(&key) {
            self.bytes -= previous.bytes;
        }
        let bytes = evidence
            .project_paths()
            .iter()
            .map(|path| path.as_os_str().len())
            .sum::<usize>()
            + std::mem::size_of_val(evidence.project_paths())
            + key.len()
            + observation.len()
            + 256;
        while !self.evidence.is_empty()
            && (self.evidence.len() >= 512 || self.bytes + bytes > CACHE_BYTES)
        {
            let key = self
                .evidence
                .first_key_value()
                .expect("nonempty metadata cache")
                .0
                .clone();
            self.bytes -= self
                .evidence
                .remove(&key)
                .expect("known metadata key")
                .bytes;
        }
        if bytes <= CACHE_BYTES {
            self.bytes += bytes;
            self.evidence.insert(
                key,
                CachedEvidence {
                    observation,
                    evidence: evidence.clone(),
                    bytes,
                    observed_at: Instant::now(),
                },
            );
        }
        Ok(Some(evidence))
    }

    #[cfg(test)]
    pub(super) fn expire_evidence(&mut self) {
        for entry in self.evidence.values_mut() {
            entry.observed_at = Instant::now() - Duration::from_secs(3601);
        }
    }
}

/// An exact folder group or a repository group with local anchor folders.
/// Discovery does not store selected state.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Group {
    id: String,
    /// A previous choice can keep this group off. This ID never grants access.
    #[serde(skip_serializing_if = "Option::is_none")]
    legacy_opt_out_id: Option<String>,
    environment: String,
    path: String,
    label: String,
    folder_paths: Vec<String>,
    worktree_count: u32,
    /// Repository groups list observed anchors, not every checkout.
    #[serde(skip_serializing_if = "Option::is_none")]
    repository_wide: Option<bool>,
    agents: Vec<String>,
    session_count: u64,
    capture_session_count: u32,
    capture_supported: bool,
    grouping_complete: bool,
    projects: Vec<ProjectSelection>,
}

/// A bounded page over all retained project suggestions plus qualified native
/// source choices. An incomplete native listing never becomes upload consent.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Discovery {
    groups: Vec<Group>,
    source_roots: Vec<SourceSelection>,
    next_cursor: Option<Cursor>,
    /// An inspection failed or exceeded a supported source, selection, or origin bound.
    partial: bool,
    /// A retry of this cursor can continue Git listing work. This is not consent.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    retry_same_page: bool,
}

#[derive(Clone)]
struct Context {
    main: String,
    folders: Vec<String>,
    origin: Option<String>,
    complete: bool,
    inspection_incomplete: bool,
}

impl Context {
    fn plain(path: &str) -> Self {
        Self {
            main: path.into(),
            folders: vec![path.into()],
            origin: None,
            complete: true,
            inspection_incomplete: false,
        }
    }

    fn unavailable(path: &str, inspection_incomplete: bool) -> Self {
        Self {
            main: path.into(),
            folders: Vec::new(),
            origin: None,
            complete: false,
            inspection_incomplete,
        }
    }

    fn group_key(&self, environment: &str) -> (String, bool, String) {
        match &self.origin {
            Some(origin) => (environment.into(), true, origin.clone()),
            None => (environment.into(), false, self.main.clone()),
        }
    }
}

/// Use fresh Git proof for each confirmation. A complete context proves each
/// exact member path, but does not prove any descendant or unlisted path.
#[derive(Default)]
struct ConfirmationContexts(BTreeMap<String, Rc<Context>>);

impl ConfirmationContexts {
    fn get(&mut self, path: &str) -> Rc<Context> {
        if let Some(saved) = self.0.get(path) {
            return Rc::clone(saved);
        }
        let checked = Rc::new(context("native", path));
        if self.0.len() < REQUEST_CONTEXT_ENTRIES {
            self.0.insert(path.into(), Rc::clone(&checked));
        }
        if checked.complete {
            for folder in &checked.folders {
                if self.0.len() == REQUEST_CONTEXT_ENTRIES {
                    break;
                }
                self.0
                    .entry(folder.clone())
                    .or_insert_with(|| Rc::clone(&checked));
            }
        }
        checked
    }
}

/// Read the current checkout's Git directory shared by all its worktrees.
async fn worktree_common_dir(
    environment: &antiburn_local::platform::environment::DiscoveryEnvironment,
    checkout: &Path,
    deadline: Option<tokio::time::Instant>,
) -> Option<PathBuf> {
    let output = exact_git(
        environment,
        checkout,
        &["rev-parse", "--git-common-dir"],
        deadline,
    )
    .await?;
    if !output.status.success() {
        return None;
    }
    require_worktree_time(deadline).ok()?;
    checkout
        .join(git_path_record(std::str::from_utf8(&output.stdout).ok()?)?)
        .canonicalize()
        .ok()
}

/// Browse commands share the origin deadline. Approval keeps its existing Git path.
async fn exact_git(
    environment: &antiburn_local::platform::environment::DiscoveryEnvironment,
    checkout: &Path,
    args: &[&str],
    deadline: Option<tokio::time::Instant>,
) -> Option<BoundedOutput> {
    let Some(deadline) = deadline else {
        let output = antiburn_local::platform::git::run_git_output_in_environment(
            environment,
            Some(checkout),
            args,
            &[],
        )
        .await
        .ok()?;
        return Some(BoundedOutput {
            status: output.status,
            stdout: output.stdout,
            truncated: false,
            stdout_truncated: false,
            stderr_truncated: false,
        });
    };
    let timeout = deadline
        .checked_duration_since(tokio::time::Instant::now())
        .filter(|remaining| !remaining.is_zero())?;
    let output = bounded_headless_output_with_stdin_and_policy(
        "git",
        args,
        None,
        BoundedOutputOptions {
            timeout,
            max_stdout_bytes: 512 * 1024,
            max_stderr_bytes: 4096,
            env: &[
                ("LC_ALL", "C"),
                ("GIT_CONFIG_NOSYSTEM", "1"),
                (
                    "GIT_CONFIG_GLOBAL",
                    if cfg!(windows) { "NUL" } else { "/dev/null" },
                ),
            ],
            env_remove: &[
                "GIT_DIR",
                "GIT_WORK_TREE",
                "GIT_COMMON_DIR",
                "GIT_OBJECT_DIRECTORY",
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                "GIT_CONFIG",
                "GIT_CONFIG_COUNT",
                "GIT_CONFIG_PARAMETERS",
            ],
            current_dir: Some(checkout),
        },
        BoundedOutputPolicy {
            fail_on_stdout_limit: true,
            kill_descendants_on_exit: true,
            ..BoundedOutputPolicy::default()
        },
    )
    .await
    .ok()?;
    (!output.truncated).then_some(output)
}

fn git_path_record(text: &str) -> Option<&Path> {
    let path = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))?;
    (!path.is_empty() && !path.chars().any(char::is_control)).then(|| Path::new(path))
}

/// Stop between filesystem calls. The listing worker keeps its permit while
/// an already started filesystem call is in progress.
fn require_worktree_time(deadline: Option<tokio::time::Instant>) -> std::io::Result<()> {
    if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
        return Err(std::io::ErrorKind::TimedOut.into());
    }
    Ok(())
}

/// Bound reads of Git's worktree pointer files and reject special files.
fn read_worktree_pointer(path: &Path, deadline: Option<tokio::time::Instant>) -> Option<String> {
    use std::io::Read;
    require_worktree_time(deadline).ok()?;
    if !path.symlink_metadata().ok()?.is_file() {
        return None;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    require_worktree_time(deadline).ok()?;
    let mut file = options.open(path).ok()?;
    require_worktree_time(deadline).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = [0; 8192];
    let mut read = 0;
    while read < bytes.len() {
        require_worktree_time(deadline).ok()?;
        match file.read(&mut bytes[read..]) {
            Ok(0) => return std::str::from_utf8(&bytes[..read]).ok().map(str::to_owned),
            Ok(count) => read += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    None
}

/// A Git list entry can outlive its checkout. Check both pointers for linked
/// worktrees so a different repo at the same path cannot become a group member.
fn registered_linked_checkout(
    checkout: &Path,
    common: &Path,
    deadline: Option<tokio::time::Instant>,
) -> Option<()> {
    let dot_git = checkout.join(".git");
    let pointer = read_worktree_pointer(&dot_git, deadline)?;
    require_worktree_time(deadline).ok()?;
    let git_dir = checkout
        .join(git_path_record(pointer.strip_prefix("gitdir: ")?)?)
        .canonicalize()
        .ok()?;
    if git_dir.parent()? != common.join("worktrees") {
        return None;
    }
    let registration = read_worktree_pointer(&git_dir.join("gitdir"), deadline)?;
    require_worktree_time(deadline).ok()?;
    (git_dir
        .join(git_path_record(&registration)?)
        .canonicalize()
        .ok()?
        == dot_git)
        .then_some(())
}

/// Missing directories and aliases cannot qualify. Other I/O errors prevent
/// the caller from deciding whether an existing directory can qualify.
fn existing_project_path(
    path: &str,
    deadline: Option<tokio::time::Instant>,
) -> std::io::Result<Option<&Path>> {
    if !crate::store::cloud_capture::sync::valid_project_path(path) {
        return Ok(None);
    }
    let path = Path::new(path);
    let inspected = || -> std::io::Result<_> {
        require_worktree_time(deadline)?;
        if !path.symlink_metadata()?.is_dir() {
            return Ok(None);
        }
        require_worktree_time(deadline)?;
        Ok((path.canonicalize()? == path).then_some(path))
    };
    match inspected() {
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        #[cfg(unix)]
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => Ok(None),
        result => result,
    }
}

/// An unreadable marker also prevents the path from qualifying as plain.
fn has_git_marker(path: &Path) -> bool {
    path.ancestors().any(|ancestor| {
        !matches!(
            ancestor.join(".git").symlink_metadata(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        )
    })
}

/// Parse Git's porcelain records, then check the registered checkouts on disk.
/// Old Git can put raw newlines in paths. Parsed paths alone do not prove membership.
fn worktree_context(
    list: &str,
    separator: char,
    repo: &Path,
    common: &Path,
    deadline: Option<tokio::time::Instant>,
) -> Option<Context> {
    require_worktree_time(deadline).ok()?;
    let delimiter = if separator == '\0' { "\0\0" } else { "\n\n" };
    let mut entries = list.split(delimiter).map(|entry| {
        let mut fields = entry.split(separator);
        let root = fields.next()?.strip_prefix("worktree ")?;
        let (mut bare, mut prunable) = (false, false);
        for field in fields {
            bare |= field == "bare";
            prunable |= field == "prunable" || field.starts_with("prunable ");
        }
        Some((root, bare, prunable))
    });
    // Git lists the parent repo first, even when run in a linked checkout.
    let (main, bare, prunable) = entries.next()??;
    let verified_main = !prunable
        && existing_project_path(main, deadline)
            .ok()
            .flatten()
            .is_some_and(|path| {
                if bare {
                    return path == common;
                }
                // A copied .git pointer can refer to the parent without proving its path.
                let dot_git = path.join(".git");
                dot_git == common
                    && require_worktree_time(deadline).is_ok()
                    && dot_git.symlink_metadata().is_ok_and(|meta| meta.is_dir())
            });
    let linked: Vec<_> = entries.flatten().take(100).collect();
    let mut complete = linked.len() < 100;
    let mut folders = Vec::new();
    if verified_main && !bare {
        folders.push(main.to_owned());
    }
    for &(root, bare, prunable) in linked.iter().take(99) {
        require_worktree_time(deadline).ok()?;
        if !bare && !prunable {
            match existing_project_path(root, deadline) {
                Ok(Some(path)) if registered_linked_checkout(path, common, deadline).is_some() => {
                    folders.push(root.to_owned());
                }
                Err(_) => complete = false,
                _ => {}
            }
        }
    }
    require_worktree_time(deadline).ok()?;
    if !folders.iter().any(|folder| Path::new(folder) == repo) {
        return None;
    }
    if !verified_main {
        let current = repo.to_str()?.to_owned();
        return Some(Context {
            main: current.clone(),
            folders: vec![current],
            origin: None,
            complete: false,
            inspection_incomplete: true,
        });
    }
    folders.sort();
    folders.dedup();
    if folders.iter().map(String::len).sum::<usize>() > 64 * 1024 {
        folders.clear();
        complete = false;
    }
    Some(Context {
        main: main.into(),
        folders,
        origin: None,
        complete,
        inspection_incomplete: !complete,
    })
}

/// Use current local Git context only. Foreign environment paths are never
/// probed through the local filesystem, even when their strings match.
fn context(environment: &str, cwd: &str) -> Context {
    context_before(environment, cwd, None)
}

fn context_before(environment: &str, cwd: &str, deadline: Option<tokio::time::Instant>) -> Context {
    if environment != "native" {
        return Context::unavailable(cwd, false);
    }
    if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
        return Context::unavailable(cwd, true);
    }
    context_for_path(cwd, existing_project_path(cwd, deadline), deadline)
}

fn context_for_path(
    cwd: &str,
    path: std::io::Result<Option<&Path>>,
    deadline: Option<tokio::time::Instant>,
) -> Context {
    let inspection_failed = || Context::unavailable(cwd, true);
    let path = match path {
        Ok(Some(path)) => path,
        Ok(None) => return Context::unavailable(cwd, false),
        Err(_) => return inspection_failed(),
    };
    use antiburn_local::platform::git;
    tauri::async_runtime::block_on(async {
        let environment = antiburn_local::platform::environment::DiscoveryEnvironment::Native;
        let root = if deadline.is_some() {
            let Some(output) = exact_git(
                &environment,
                path,
                &["rev-parse", "--show-toplevel"],
                deadline,
            )
            .await
            else {
                return inspection_failed();
            };
            // Only the legacy proof path may classify a path as plain using Git's stderr.
            if !output.status.success() {
                return inspection_failed();
            }
            let Ok(repo) = std::str::from_utf8(&output.stdout) else {
                return inspection_failed();
            };
            Ok(Some(PathBuf::from(repo.trim())))
        } else {
            git::repo_root_if_any_at(path).await
        };
        match root {
            Ok(Some(repo)) => {
                let Some(mut output) = exact_git(
                    &environment,
                    &repo,
                    &["worktree", "list", "--porcelain", "-z"],
                    deadline,
                )
                .await
                else {
                    return inspection_failed();
                };
                let mut separator = '\0';
                if output.status.code() == Some(129) {
                    // Older Git versions do not accept `-z` for this command.
                    let Some(legacy) = exact_git(
                        &environment,
                        &repo,
                        &["worktree", "list", "--porcelain"],
                        deadline,
                    )
                    .await
                    else {
                        return inspection_failed();
                    };
                    output = legacy;
                    separator = '\n';
                }
                if !output.status.success() {
                    return inspection_failed();
                }
                let Ok(list) = std::str::from_utf8(&output.stdout) else {
                    return inspection_failed();
                };
                let Some(common) = worktree_common_dir(&environment, &repo, deadline).await else {
                    return inspection_failed();
                };
                let checked = worktree_context(list, separator, &repo, &common, deadline);
                if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                    return inspection_failed();
                }
                checked.unwrap_or_else(inspection_failed)
            }
            Ok(None) => {
                // Git can report no repository when a .git marker is damaged.
                if has_git_marker(path) {
                    return inspection_failed();
                }
                Context::plain(cwd)
            }
            Err(_) => inspection_failed(),
        }
    })
}

/// Derive provider roots from existing local metadata and the provider adapter.
/// This does not enumerate a second copy of provider directory layouts.
fn known_sources(
    store: &Store,
    opencode_databases: impl IntoIterator<Item = PathBuf>,
    scanning: &dyn Fn() -> bool,
) -> Result<(Vec<SourceSelection>, bool), String> {
    let mut roots = BTreeSet::new();
    let mut claude_parents = BTreeSet::new();
    let mut after = String::new();
    let mut partial = false;
    'claude: loop {
        if !scanning() {
            partial = true;
            break;
        }
        let paths = store
            .cloud_claude_source_paths_page(&after, 512)
            .map_err(|_| "Could not read cached session source metadata.")?;
        if paths.is_empty() {
            break;
        }
        after = paths.last().cloned().unwrap_or_default();
        for path in paths {
            if !scanning() {
                partial = true;
                break 'claude;
            }
            let path = PathBuf::from(path);
            let root = if path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                == Some("subagents")
            {
                path.parent().and_then(Path::parent).and_then(Path::parent)
            } else {
                path.parent()
            };
            let Some(root) = root else {
                continue;
            };
            if claude_parents.len() >= PROJECT_SCAN_PAGE_CANDIDATES {
                claude_parents.clear();
            }
            if !claude_parents.insert(root.to_path_buf()) || !root.is_dir() {
                continue;
            }
            if let Some(path) = root
                .canonicalize()
                .ok()
                .and_then(|path| path.to_str().map(str::to_owned))
            {
                roots.insert(SourceSelection {
                    source_kind: SourceKind::ClaudeJsonl,
                    path,
                });
            }
            if roots.len() > 100 {
                partial = true;
                break;
            }
        }
        if partial {
            break;
        }
    }
    for path in opencode_databases {
        if !scanning() {
            partial = true;
            break;
        }
        if path.is_file()
            && let Some(path) = path
                .canonicalize()
                .ok()
                .and_then(|path| path.to_str().map(str::to_owned))
        {
            roots.insert(SourceSelection {
                source_kind: SourceKind::OpenCodeSqliteV2,
                path,
            });
        }
    }
    if roots.len() > 100 {
        partial = true;
    }
    Ok((roots.into_iter().take(100).collect(), partial))
}

/// Saved provider opt-outs override discovery defaults. Explicit selections get
/// room first; paths and providers stay distinct and still need source admission.
fn merge_sources(
    known: Vec<SourceSelection>,
    folders: &[Folder],
) -> Result<(Vec<SourceSelection>, bool), String> {
    if folders.len() > 100 {
        return Err("Saved provider sources exceed the source limit.".into());
    }
    let canonical = |kind, value: &str| {
        let path = Path::new(value);
        if value.is_empty()
            || value.len() > 4096
            || value.chars().any(char::is_control)
            || !path.is_absolute()
        {
            return None;
        }
        path.canonicalize().ok().and_then(|path| {
            path.to_str().map(|path| SourceSelection {
                source_kind: kind,
                path: path.into(),
            })
        })
    };
    let mut partial = false;
    let mut excluded = BTreeSet::new();
    let mut selected = BTreeSet::new();
    for folder in folders {
        let key = canonical(folder.source_kind, &folder.path);
        if !folder.selected {
            excluded.insert(key.unwrap_or_else(|| SourceSelection {
                source_kind: folder.source_kind,
                path: folder.path.clone(),
            }));
        } else if let Some(key) = key {
            selected.insert(key);
        } else {
            partial = true;
        }
    }
    selected.retain(|source| !excluded.contains(source));
    let mut defaults = BTreeSet::new();
    for source in known {
        if let Some(source) = canonical(source.source_kind, &source.path) {
            if !excluded.contains(&source) && !selected.contains(&source) {
                defaults.insert(source);
            }
        } else {
            partial = true;
        }
    }
    partial |= selected.len() + defaults.len() > 100;
    Ok((
        selected.into_iter().chain(defaults).take(100).collect(),
        partial,
    ))
}

pub(super) fn group_id(environment: &str, path: &str) -> String {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&(environment, path)).expect("string tuple serialization");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(bytes))
}

#[derive(Clone)]
enum ProjectOrigin {
    Repository(String, String),
    ExactGit,
    Plain,
    Unavailable,
    Incomplete,
}

struct RequestOrigins {
    checkouts: CheckoutOriginCache,
    paths: BTreeMap<String, ProjectOrigin>,
    deadline: tokio::time::Instant,
}

impl Default for RequestOrigins {
    fn default() -> Self {
        Self::before(tokio::time::Instant::now() + PROJECT_ORIGIN_LOOKUP_BUDGET)
    }
}

impl RequestOrigins {
    fn before(deadline: tokio::time::Instant) -> Self {
        Self {
            checkouts: CheckoutOriginCache::default(),
            paths: BTreeMap::new(),
            deadline,
        }
    }

    /// Use one deadline for all Git lookups in this request. Reuse completed
    /// proof for the same path, but never treat skipped Git as an exact folder.
    fn get(&mut self, path: &str) -> ProjectOrigin {
        if let Some(saved) = self.paths.get(path) {
            return saved.clone();
        }
        let checked = inspect_origin(&mut self.checkouts, path, self.deadline, || {
            tauri::async_runtime::block_on(async {
                let probe = tokio::time::timeout_at(
                    self.deadline,
                    tokio::fs::canonicalize(Path::new(path)),
                )
                .await;
                classify_unverified_path(path, probe.ok())
            })
        });
        if self.paths.len() < REQUEST_CONTEXT_ENTRIES {
            self.paths.insert(path.into(), checked.clone());
        }
        checked
    }
}

/// Resolve checkout evidence before the caller's deadline. The caller owns
/// filesystem admission for the fallback that distinguishes missing paths.
fn inspect_origin(
    checkouts: &mut CheckoutOriginCache,
    path: &str,
    deadline: tokio::time::Instant,
    probe_unverified: impl FnOnce() -> ProjectOrigin,
) -> ProjectOrigin {
    if cfg!(not(unix)) {
        return ProjectOrigin::ExactGit;
    }
    let checked = tauri::async_runtime::block_on(async {
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::timeout_at(deadline, checkouts.lookup(Path::new(path)))
            .await
            .ok()
    });
    match checked {
        Some(CheckoutOrigin::Plain) => ProjectOrigin::Plain,
        Some(CheckoutOrigin::Verified { origin: None, .. }) => ProjectOrigin::ExactGit,
        Some(CheckoutOrigin::Verified {
            checkout,
            origin: Some(origin),
        }) => checkout
            .to_str()
            .map_or(ProjectOrigin::Incomplete, |checkout| {
                ProjectOrigin::Repository(checkout.into(), origin.as_label().into())
            }),
        Some(CheckoutOrigin::UnverifiedGit) if tokio::time::Instant::now() < deadline => {
            probe_unverified()
        }
        _ => ProjectOrigin::Incomplete,
    }
}

fn classify_unverified_path(path: &str, probe: Option<std::io::Result<PathBuf>>) -> ProjectOrigin {
    match probe {
        Some(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            ProjectOrigin::Unavailable
        }
        Some(Ok(canonical)) if canonical != Path::new(path) => ProjectOrigin::Unavailable,
        _ => ProjectOrigin::Incomplete,
    }
}

#[derive(Default)]
struct ConfirmationTargets {
    paths: BTreeSet<String>,
    exact_groups: BTreeSet<String>,
    repository_anchors: BTreeMap<String, String>,
}

impl ConfirmationTargets {
    fn is_confirmed(&self) -> bool {
        self.exact_groups.is_empty() && self.repository_anchors.is_empty()
    }
}

/// Confirm choices for a caller that only needs the allowed source roots.
#[cfg(test)]
pub(super) fn confirm_project_choices(
    store: &Store,
    cache: &mut Cache,
    binding: &DestinationBinding,
    choices: &BTreeMap<String, bool>,
    projects: &BTreeSet<ProjectSelection>,
    current: &dyn Fn() -> bool,
) -> Result<Vec<SourceSelection>, String> {
    confirm_project_choices_with_origins(store, cache, binding, choices, projects, current)
        .map(|(roots, _)| roots)
}

/// Confirm each source and group against current session evidence. Return the
/// allowed sources and the exact checkout roots explicitly approved by origin.
pub(super) fn confirm_project_choices_with_origins(
    store: &Store,
    cache: &mut Cache,
    binding: &DestinationBinding,
    choices: &BTreeMap<String, bool>,
    projects: &BTreeSet<ProjectSelection>,
    current: &dyn Fn() -> bool,
) -> Result<(Vec<SourceSelection>, BTreeMap<String, String>), String> {
    let saved = store
        .cloud_draft(&binding.account_id, &binding.destination_id)
        .map_err(|_| "Could not read saved provider sources.")?
        .map(|raw| serde_json::from_str::<Draft>(&raw))
        .transpose()
        .map_err(|_| "Could not read saved provider sources.")?
        .unwrap_or_default();
    let (mut known, _) = known_sources(
        store,
        antiburn_local::discovery::agents::opencode::native_database_paths(),
        &|| true,
    )?;
    known.extend(super::file_sources::roots().0);
    let (roots, _) = merge_sources(known, &saved.folders)?;
    let mut by_source = BTreeMap::<&SourceSelection, ConfirmationTargets>::new();
    let mut contexts = ConfirmationContexts::default();
    let mut origins = RequestOrigins::default();
    let mut approved_origins = BTreeMap::new();
    for project in projects {
        if !current() {
            return Err("Cloud account or upload controls changed.".into());
        }
        let root = roots
            .iter()
            .find(|root| {
                root.source_kind == project.source_kind && root.path == project.source_path
            })
            .ok_or("A session source changed. Reload projects and confirm again.")?;
        let targets = by_source.entry(root).or_default();
        targets.paths.insert(project.project_path.clone());
        let group = match origins.get(&project.project_path) {
            ProjectOrigin::Repository(checkout, origin)
                if checkout == project.project_path
                    && choices.get(&repository_group_id(&origin)) == Some(&true) =>
            {
                targets
                    .repository_anchors
                    .insert(checkout.clone(), origin.clone());
                approved_origins.insert(checkout, origin);
                continue;
            }
            ProjectOrigin::Repository(..) | ProjectOrigin::ExactGit => {
                let member = contexts.get(&project.project_path);
                if !member.complete || !member.folders.contains(&project.project_path) {
                    return Err(
                        "The project folders changed. Reload projects and confirm again.".into(),
                    );
                }
                group_id("native", &member.main)
            }
            ProjectOrigin::Plain => group_id("native", &project.project_path),
            ProjectOrigin::Unavailable => {
                return Err(
                    "The project folders changed. Reload projects and confirm again.".into(),
                );
            }
            ProjectOrigin::Incomplete => return Err(PROJECT_DISCOVERY_INCOMPLETE.into()),
        };
        if choices.get(&group) != Some(&true) {
            return Err("The project folders changed. Reload projects and confirm again.".into());
        }
        targets.exact_groups.insert(group);
    }
    cache.begin_scan();
    let (selected_roots, mut targets): (Vec<_>, Vec<_>) = by_source.into_iter().unzip();
    let mut scans = ProjectScans::new(selected_roots.clone());
    let mut partial = vec![false; selected_roots.len()];
    while let Some((index, listing)) = scans.next(
        current,
        |index| !targets[index].is_confirmed(),
        |index| {
            let paths = targets[index].paths.iter().cloned().collect::<Vec<_>>();
            store
                .cloud_opencode_session_ids(&paths, PROJECT_SCAN_PAGE_CANDIDATES)
                .map(Cow::Owned)
                .map_err(|_| "Could not read cached session identities.".into())
        },
    ) {
        let root = selected_roots[index];
        let target = &mut targets[index];
        let listing = listing?;
        partial[index] |= listing.partial;
        for candidate in listing.candidates {
            if !current() {
                return Err("Cloud account or upload controls changed.".into());
            }
            let evidence = match cache.evidence(
                &binding.account_id,
                &binding.destination_id,
                root,
                &candidate,
                &sync::metadata_limits(),
            ) {
                Ok(Some(evidence)) if evidence.scan_complete() => evidence,
                _ => {
                    partial[index] = true;
                    continue;
                }
            };
            for path in evidence
                .project_paths()
                .iter()
                .filter_map(|path| path.to_str())
            {
                let proof = origins.get(path);
                if let ProjectOrigin::Repository(checkout, origin) = &proof
                    && target.repository_anchors.get(checkout) == Some(origin)
                {
                    target.repository_anchors.remove(checkout);
                }
                if matches!(proof, ProjectOrigin::Incomplete) {
                    partial[index] = true;
                } else if !target.exact_groups.is_empty() {
                    match proof {
                        ProjectOrigin::Plain => {
                            target.exact_groups.remove(&group_id("native", path));
                        }
                        ProjectOrigin::ExactGit | ProjectOrigin::Repository(..) => {
                            let member = contexts.get(path);
                            if member.complete {
                                target
                                    .exact_groups
                                    .remove(&group_id("native", &member.main));
                            } else {
                                partial[index] |= member.inspection_incomplete;
                            }
                        }
                        ProjectOrigin::Unavailable | ProjectOrigin::Incomplete => {}
                    }
                }
                if target.is_confirmed() {
                    break;
                }
            }
            if target.is_confirmed() {
                break;
            }
        }
    }
    for (index, target) in targets.into_iter().enumerate() {
        if !target.is_confirmed() {
            if partial[index] || scans.root_incomplete(index) {
                return Err(PROJECT_DISCOVERY_INCOMPLETE.into());
            }
            return Err("Could not confirm a supported session for this project. Reload projects and try again.".into());
        }
    }
    Ok((roots, approved_origins))
}

/// Find grants for the group even when current provider discovery cannot list them.
/// If a remaining grant can still cover the group, do not report that it is off.
pub(super) fn group_projects_to_remove(
    id: &str,
    folder_paths: &[String],
    saved: &[ProjectSelection],
) -> Result<BTreeSet<ProjectSelection>, String> {
    let mut folders: BTreeSet<_> = folder_paths.iter().cloned().collect();
    let contexts: BTreeMap<_, _> = saved
        .iter()
        .map(|project| &project.project_path)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|path| (path, context("native", path)))
        .collect();
    for (path, member) in &contexts {
        if group_id("native", path) == id || group_id("native", &member.main) == id {
            folders.insert((*path).clone());
            folders.insert(member.main.clone());
            folders.extend(member.folders.iter().cloned());
        }
    }
    let mut removed = BTreeSet::new();
    for project in saved {
        if folders.contains(&project.project_path) {
            removed.insert(project.clone());
            continue;
        }
        let path = Path::new(&project.project_path);
        if folders
            .iter()
            .any(|folder| Path::new(folder).starts_with(path) || path.starts_with(folder))
        {
            return Err(
                "Another saved project overlaps these folders. Pause uploads to stop syncing them."
                    .into(),
            );
        }
        if !contexts[&project.project_path].complete {
            return Err(
                "Could not verify all saved project folders. Pause uploads to stop syncing them."
                    .into(),
            );
        }
    }
    Ok(removed)
}

fn add_suggestion(
    groups: &mut BTreeMap<(String, bool, String), Group>,
    suggestion: Suggestion,
    context: &Context,
) {
    let group = groups
        .entry(context.group_key(&suggestion.environment))
        .or_insert_with(|| Group {
            id: context.origin.as_deref().map_or_else(
                || group_id(&suggestion.environment, &context.main),
                repository_group_id,
            ),
            legacy_opt_out_id: context
                .origin
                .as_deref()
                .and_then(legacy_repository_origin_for_off_choice)
                .map(|origin| repository_group_id(&origin)),
            environment: suggestion.environment.clone(),
            path: context.main.clone(),
            label: Path::new(&context.main)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&context.main)
                .into(),
            worktree_count: context
                .folders
                .iter()
                .filter(|folder| *folder != &context.main)
                .count() as u32,
            repository_wide: context.origin.as_ref().map(|_| true),
            folder_paths: context.folders.clone(),
            agents: Vec::new(),
            session_count: 0,
            capture_session_count: 0,
            capture_supported: false,
            grouping_complete: context.complete,
            projects: Vec::new(),
        });
    group.session_count += suggestion.session_count;
    group.agents.extend(suggestion.agents);
    group.agents.sort();
    group.agents.dedup();
}

/// Keep provider continuations only for this call. Give each root a first turn
/// before spending the rest of the same budget on continuations.
struct ProjectScans<'a> {
    roots: Vec<&'a SourceSelection>,
    first: usize,
    pending: VecDeque<(usize, ProjectRootScan)>,
    remaining: usize,
}

impl<'a> ProjectScans<'a> {
    fn new(roots: Vec<&'a SourceSelection>) -> Self {
        Self {
            roots,
            first: 0,
            pending: VecDeque::new(),
            remaining: PROJECT_SCAN_CANDIDATES,
        }
    }

    /// Charge a failed read its full share. Only the first read uses cached IDs.
    fn next<'h>(
        &mut self,
        current: &dyn Fn() -> bool,
        needs_more: impl Fn(usize) -> bool,
        hints: impl FnOnce(usize) -> Result<Cow<'h, [String]>, String>,
    ) -> Option<(usize, Result<super::file_sources::Page, String>)> {
        if self.remaining == 0 {
            return None;
        }
        let (index, mut scan, mut share, first) = if self.first < self.roots.len() {
            let index = self.first;
            let share = (self.remaining / (self.roots.len() - index))
                .clamp(1, PROJECT_SCAN_PAGE_CANDIDATES);
            self.first += 1;
            (index, ProjectRootScan::new(self.roots[index]), share, true)
        } else {
            let (index, scan) = loop {
                let (index, scan) = self.pending.pop_front()?;
                if needs_more(index) {
                    break (index, scan);
                }
            };
            (
                index,
                scan,
                self.remaining.min(PROJECT_SCAN_PAGE_CANDIDATES),
                false,
            )
        };
        self.remaining -= share;
        let result = (|| {
            if !current() {
                return Err("Cloud account changed.".into());
            }
            let ids: Cow<'_, [String]> =
                if first && self.roots[index].source_kind == SourceKind::OpenCodeSqliteV2 {
                    hints(index)?
                } else {
                    Cow::Borrowed(&[])
                };
            let page = scan.read(self.roots[index], &ids, &mut share, current)?;
            if page.next_cursor.is_some() && (page.visited == 0 || page.next_cursor == scan.after) {
                return Err("Session discovery did not advance.".into());
            }
            scan.after.clone_from(&page.next_cursor);
            Ok(page)
        })();
        if let Ok(page) = &result {
            self.remaining += share;
            if page.next_cursor.is_some() {
                self.pending.push_back((index, scan));
            }
        }
        Some((index, result))
    }

    fn root_incomplete(&self, index: usize) -> bool {
        self.pending.iter().any(|(pending, _)| *pending == index)
    }

    fn has_remaining(&self) -> bool {
        self.remaining > 0 && (self.first < self.roots.len() || !self.pending.is_empty())
    }
}

struct ProjectRootScan {
    after: Option<String>,
    cursor: Option<super::file_sources::CursorScan>,
    hinted_ids: BTreeSet<String>,
}

impl ProjectRootScan {
    fn new(root: &SourceSelection) -> Self {
        Self {
            after: None,
            cursor: matches!(
                root.source_kind,
                SourceKind::CursorCliAgentJsonl | SourceKind::CursorIdeComposer
            )
            .then(super::file_sources::CursorScan::new),
            hinted_ids: BTreeSet::new(),
        }
    }

    /// Reopen cached IDs to prove source membership. A failed source proof does
    /// not select a fallback reader. Provider pages can repeat successful hints.
    fn read(
        &mut self,
        root: &SourceSelection,
        cached_opencode_ids: &[String],
        remaining: &mut usize,
        current: &dyn Fn() -> bool,
    ) -> Result<super::file_sources::Page, String> {
        if let Some(cursor) = &mut self.cursor {
            let page = cursor
                .read(
                    root.source_kind,
                    Path::new(&root.path),
                    &antiburn_cloud_capture::Limits {
                        discovery_sessions: *remaining,
                        ..sync::metadata_limits()
                    },
                    current,
                )
                .map_err(|_| "Could not qualify the selected Cursor source.")?;
            *remaining = remaining
                .checked_sub(page.visited)
                .ok_or("Session discovery exceeded its page limit.")?;
            return Ok(page);
        }
        let mut candidates = Vec::new();
        for id in cached_opencode_ids {
            if !current() {
                return Err("Cloud account changed.".into());
            }
            // Keep room for fresh page admission, even when every hint is stale.
            if *remaining <= 1 {
                break;
            }
            *remaining -= 1;
            match antiburn_cloud_capture::reopen_opencode(
                Path::new(&root.path),
                id,
                &sync::metadata_limits(),
            ) {
                Ok(candidate) => {
                    if self.hinted_ids.insert(candidate.source_id().to_owned()) {
                        candidates.push(candidate);
                    }
                }
                Err(antiburn_cloud_capture::Error::SourceChanged) => {
                    // A missing cached ID can belong to another database.
                    // Fresh source-page admission below must still succeed.
                }
                Err(_) => return Err("Could not qualify the selected session source.".into()),
            }
        }
        if !current() {
            return Err("Cloud account changed.".into());
        }
        let mut page = sync::discover_root_page(
            root.source_kind,
            &root.path,
            self.after.as_deref(),
            *remaining,
        )?;
        *remaining = remaining
            .checked_sub(page.visited)
            .ok_or("Session discovery exceeded its page limit.")?;
        candidates.extend(
            std::mem::take(&mut page.candidates)
                .into_iter()
                .filter(|candidate| !self.hinted_ids.remove(candidate.source_id())),
        );
        page.candidates = candidates;
        Ok(page)
    }
}

struct ListingWindow<'a> {
    deadline: tokio::time::Instant,
    now: &'a dyn Fn() -> tokio::time::Instant,
}

impl ListingWindow<'_> {
    fn new(deadline: tokio::time::Instant) -> Self {
        Self {
            deadline,
            now: &tokio::time::Instant::now,
        }
    }
}

/// Build one discovery page using only the current account's saved provider
/// preferences. Suggestions and source access never create upload consent.
fn discover_projects(
    store: &Store,
    cache: &mut Cache,
    scope: (&ListingAuthority, u64),
    after: Option<&Cursor>,
    known: (Vec<SourceSelection>, bool),
    current: &dyn Fn() -> bool,
    window: ListingWindow<'_>,
) -> Result<Discovery, String> {
    if !current() {
        return Err("Cloud account changed.".into());
    }
    let (authority, sync_generation) = scope;
    let binding = &authority.binding;
    let within_budget = || (window.now)() < window.deadline;
    let scanning = || current() && within_budget();
    let saved = store
        .cloud_draft(&binding.account_id, &binding.destination_id)
        .map_err(|_| "Could not read saved provider sources.")?
        .map(|raw| serde_json::from_str::<Draft>(&raw))
        .transpose()
        .map_err(|_| "Could not read saved provider sources.")?
        .unwrap_or_default();
    let (roots, saved_partial) = merge_sources(known.0, &saved.folders)?;
    let mut partial = known.1 || saved_partial;
    let suggestions = store
        .cloud_project_suggestions_page(after, 32)
        .map_err(|_| "Could not read cached project metadata.")?;
    let next_cursor = (suggestions.len() == 32).then(|| {
        let last = suggestions.last().expect("full project page");
        Cursor {
            environment: last.environment.clone(),
            cwd: last.cwd.clone(),
        }
    });
    let mut groups = BTreeMap::new();
    let progress_key =
        ListingProgress::key(authority, sync_generation, after, &saved.folders, &roots);
    let mut lookups = ListingLookups::new(
        cache.listing_progress.take(),
        progress_key,
        window
            .deadline
            .min(tokio::time::Instant::now() + PROJECT_ORIGIN_LOOKUP_BUDGET),
    );
    let mut suggested_native_paths = Vec::new();
    cache.begin_scan();
    for suggestion in suggestions {
        if !current() {
            return Err("Cloud account changed.".into());
        }
        if suggestion.environment == "native" {
            suggested_native_paths.push(suggestion.cwd.clone());
        }
        let context = lookups.context(&suggestion.environment, &suggestion.cwd);
        partial |= context.inspection_incomplete;
        if suggestion.environment == "native" && !context.complete && !context.inspection_incomplete
        {
            continue;
        }
        add_suggestion(&mut groups, suggestion, &context);
    }
    let cached_opencode_ids = if scanning()
        && roots
            .iter()
            .any(|root| root.source_kind == SourceKind::OpenCodeSqliteV2)
    {
        store
            .cloud_opencode_session_ids(&suggested_native_paths, PROJECT_SCAN_PAGE_CANDIDATES)
            .map_err(|_| "Could not read cached session identities.")?
    } else {
        Vec::new()
    };
    let mut folder_path_bytes = groups
        .values()
        .flat_map(|group| &group.folder_paths)
        .map(String::len)
        .sum::<usize>();
    let mut qualified = vec![false; roots.len()];
    let mut project_choices = 0usize;
    let mut scans = ProjectScans::new(roots.iter().collect());
    'providers: while scanning() {
        let Some((index, listing)) = scans.next(
            &scanning,
            |_| true,
            |_| Ok(Cow::Borrowed(&cached_opencode_ids)),
        ) else {
            break;
        };
        let root = &roots[index];
        let listing = match listing {
            Ok(candidates) => candidates,
            Err(_) => {
                partial = true;
                if !within_budget() {
                    lookups.defer_provider_work();
                }
                continue;
            }
        };
        partial |= listing.partial;
        qualified[index] = true;
        for candidate in listing.candidates {
            if !current() {
                break 'providers;
            }
            if !within_budget() {
                lookups.defer_provider_work();
                break 'providers;
            }
            let evidence = match cache.evidence(
                &binding.account_id,
                &binding.destination_id,
                root,
                &candidate,
                &sync::metadata_limits(),
            ) {
                Ok(Some(evidence)) => evidence,
                Ok(None) => {
                    partial = true;
                    continue;
                }
                Err(_) => {
                    partial = true;
                    continue;
                }
            };
            partial |= !evidence.scan_complete();
            let mut matched = BTreeSet::new();
            for path in evidence.project_paths() {
                if !current() {
                    break 'providers;
                }
                let Some(project) = path.to_str().filter(|project| {
                    crate::store::cloud_capture::sync::valid_project_path(project)
                }) else {
                    partial = true;
                    continue;
                };
                if !within_budget() {
                    lookups.defer_provider_work();
                    break 'providers;
                }
                let context = lookups.context("native", project);
                partial |= context.inspection_incomplete;
                if !context.complete && !context.inspection_incomplete {
                    continue;
                }
                let key = context.group_key("native");
                if !groups.contains_key(&key) {
                    if after.is_some() {
                        continue;
                    }
                    let added_bytes = context.folders.iter().map(String::len).sum::<usize>();
                    if groups.len() >= 256 || folder_path_bytes + added_bytes > CACHE_BYTES {
                        partial = true;
                        continue;
                    }
                    folder_path_bytes += added_bytes;
                    add_suggestion(
                        &mut groups,
                        Suggestion {
                            environment: "native".into(),
                            cwd: project.to_owned(),
                            agents: vec![root.source_kind.upload_names().0.into()],
                            session_count: 0,
                        },
                        &context,
                    );
                }
                if let Some(group) = groups.get_mut(&key) {
                    if matched.insert(group.id.clone()) {
                        group.capture_session_count += 1;
                    }
                    if !evidence.scan_complete()
                        || !context.complete
                        || !group.grouping_complete
                        || group.folder_paths.is_empty()
                    {
                        continue;
                    }
                    group.capture_supported = true;
                    let (folders, repository) = if context.origin.is_some() {
                        (&context.folders, true)
                    } else {
                        (&group.folder_paths, false)
                    };
                    let mut added_anchor = None;
                    for folder in folders {
                        if group.projects.iter().any(|project| {
                            project.source_kind == root.source_kind
                                && project.source_path == root.path
                                && (repository || project.project_path == *folder)
                        }) {
                            continue;
                        }
                        if project_choices >= 512 {
                            partial = true;
                            group.capture_supported = false;
                            continue;
                        }
                        if repository && !group.folder_paths.contains(folder) {
                            if folder_path_bytes + folder.len() > CACHE_BYTES {
                                partial = true;
                                group.capture_supported = false;
                                continue;
                            }
                            folder_path_bytes += folder.len();
                            added_anchor = Some(folder.clone());
                        }
                        group.projects.push(ProjectSelection {
                            source_kind: root.source_kind,
                            source_path: root.path.clone(),
                            project_path: folder.clone(),
                        });
                        project_choices += 1;
                    }
                    if let Some(anchor) = added_anchor {
                        group.folder_paths.push(anchor);
                        group.folder_paths.sort();
                    }
                    group.projects.sort();
                    group.projects.dedup();
                }
            }
        }
    }
    if !current() {
        return Err("Cloud account changed.".into());
    }
    if !within_budget() {
        partial = true;
        if scans.has_remaining() {
            lookups.defer_provider_work();
        }
    }
    partial |= !scans.pending.is_empty();
    let (progress, retry_same_page) = lookups.finish();
    cache.listing_progress = progress;
    Ok(Discovery {
        groups: groups.into_values().collect(),
        source_roots: roots
            .into_iter()
            .zip(qualified)
            .filter_map(|(root, qualified)| qualified.then_some(root))
            .collect(),
        next_cursor,
        partial,
        retry_same_page,
    })
}

/// Page cached suggestions without saving initial ON rows or changing consent.
/// Only native exporter evidence can qualify a folder for capture.
#[tauri::command]
pub(crate) async fn cloud_project_groups(
    app: tauri::AppHandle,
    binding: DestinationBinding,
    after: Option<Cursor>,
) -> Result<Discovery, String> {
    let controller = app.state::<Controller>();
    let work = controller.project_listing.clone();
    let generation = controller.account_generation.load(Ordering::Acquire);
    let sync_generation = controller.sync.generation.load(Ordering::Acquire);
    let later_page = after.is_some();
    work.run(Priority::Normal, move || {
        let controller = app.state::<Controller>();
        let authority = controller
            .project_listing_authority
            .lock()
            .map_err(|_| "Cloud is unavailable.")?
            .clone()
            .filter(|authority| authority.matches(&binding, generation, now()))
            .ok_or("Cloud account changed.")?;
        let account_current = || {
            controller.account_generation.load(Ordering::Acquire) == generation
                && authority.expires_at > now()
        };
        let scope_current =
            || controller.sync.generation.load(Ordering::Acquire) == sync_generation;
        let current = || account_current() && scope_current();
        if !account_current() {
            return Err("Cloud account changed.".into());
        }
        if !scope_current() {
            return Err(super::work::BUSY.into());
        }
        if !controller.setup_ready.load(Ordering::Acquire) {
            return Err("Cloud folder settings are unavailable.".into());
        }
        let deadline = tokio::time::Instant::now() + PROJECT_LISTING_BUDGET;
        let scanning = || current() && tokio::time::Instant::now() < deadline;
        let store = app
            .state::<Store>()
            .open_reader(Duration::from_millis(100))
            .map_err(|_| "Could not read cached folder metadata.")?;
        let mut cache = controller
            .project_listing_cache
            .lock()
            .map_err(|_| "Project discovery is unavailable.")?;
        let (mut known, partial) = known_sources(
            &store,
            antiburn_local::discovery::agents::opencode::native_database_paths(),
            &scanning,
        )?;
        let (file_roots, file_partial) = if scanning() {
            super::file_sources::roots()
        } else {
            (Vec::new(), true)
        };
        known.extend(file_roots);
        let discovery = discover_projects(
            &store,
            &mut cache,
            (&authority, sync_generation),
            after.as_ref(),
            (known, partial || file_partial),
            &current,
            ListingWindow::new(deadline),
        );
        let active = controller
            .project_listing_authority
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if !account_current()
            || !active
                .as_ref()
                .is_some_and(|authority| authority.matches(&binding, generation, now()))
        {
            return Err("Cloud account changed.".into());
        }
        if !scope_current() {
            return Err(super::work::BUSY.into());
        }
        discovery
    })
    .await
    .inspect_err(|reason| {
        tracing::debug!(later_page, reason = %reason, "Cloud folder page read failed");
    })
}

#[cfg(all(test, unix))]
#[path = "tests/projects_integration_tests.rs"]
mod tests;
