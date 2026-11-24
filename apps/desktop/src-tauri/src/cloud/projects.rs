//! Cached project suggestions and current local worktree grouping.
//! Grouping does not prove historical ownership or permit upload. Desktop capture
//! must obtain fresh recorded-CWD admission under separate saved consent.

use super::{
    Controller, DestinationBinding, Draft, Folder,
    engine::Engine,
    ensure_cloud_setup, now,
    repository_checkout::{CheckoutOrigin, CheckoutOriginCache},
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

const CACHE_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_CONTEXT_ENTRIES: usize = 512;
const PROJECT_ORIGIN_LOOKUP_BUDGET: Duration = Duration::from_secs(2);
const PROJECT_DISCOVERY_INCOMPLETE: &str =
    "Project discovery is incomplete. Restore access or choose fewer sources and retry.";
const PROJECT_SCAN_CANDIDATES: usize = 4096;
const PROJECT_SCAN_PAGE_CANDIDATES: usize = 512;
const CLAUDE_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const CLAUDE_FIRST_SOURCE_BYTES: u64 = 128 * 1024 * 1024;

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
    contexts: BTreeMap<(String, String), (Instant, Context)>,
    bytes: usize,
    scan_bytes: u64,
    context_bytes: usize,
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

    fn context(&mut self, environment: &str, cwd: &str, origins: &mut RequestOrigins) -> Context {
        if environment == "native" {
            match origins.get(cwd) {
                ProjectOrigin::Repository(checkout, origin) => {
                    return Context {
                        main: checkout.clone(),
                        folders: vec![checkout],
                        origin: Some(origin),
                        complete: true,
                        inspection_incomplete: false,
                    };
                }
                ProjectOrigin::Plain => return Context::plain(cwd),
                ProjectOrigin::Unavailable => return Context::unavailable(cwd, false),
                ProjectOrigin::Incomplete => return Context::unavailable(cwd, true),
                ProjectOrigin::ExactGit => {}
            }
        }
        let key = (environment.to_owned(), cwd.to_owned());
        if let Some((observed, context)) = self.contexts.get(&key)
            && observed.elapsed() < Duration::from_secs(if context.complete { 60 } else { 1 })
        {
            return context.clone();
        }
        let value = context(environment, cwd);
        let bytes = value.main.len()
            + value.folders.iter().map(String::len).sum::<usize>()
            + value.folders.len() * std::mem::size_of::<String>()
            + key.0.len()
            + key.1.len()
            + 128;
        if self.contexts.len() >= 512 || self.context_bytes + bytes > CACHE_BYTES {
            self.contexts.clear();
            self.context_bytes = 0;
        }
        self.context_bytes += bytes;
        self.contexts.insert(key, (Instant::now(), value.clone()));
        value
    }
}

/// An exact folder group or a repository group with local anchor folders.
/// Discovery does not store selected state.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Group {
    id: String,
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
) -> Option<PathBuf> {
    let output = antiburn_local::platform::git::run_git_output_in_environment(
        environment,
        Some(checkout),
        &["rev-parse", "--git-common-dir"],
        &[],
    )
    .await
    .ok()?;
    if !output.status.success() {
        return None;
    }
    checkout
        .join(git_path_record(std::str::from_utf8(&output.stdout).ok()?)?)
        .canonicalize()
        .ok()
}

fn git_path_record(text: &str) -> Option<&Path> {
    let path = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))?;
    (!path.is_empty() && !path.chars().any(char::is_control)).then(|| Path::new(path))
}

/// Bound reads of Git's worktree pointer files and reject special files.
fn read_worktree_pointer(path: &Path) -> Option<String> {
    use std::io::Read;
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
    let file = options.open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(8192).read_to_string(&mut text).ok()?;
    (text.len() < 8192).then_some(text)
}

/// A Git list entry can outlive its checkout. Check both pointers for linked
/// worktrees so a different repo at the same path cannot become a group member.
fn registered_linked_checkout(checkout: &Path, common: &Path) -> Option<()> {
    let dot_git = checkout.join(".git");
    let pointer = read_worktree_pointer(&dot_git)?;
    let git_dir = checkout
        .join(git_path_record(pointer.strip_prefix("gitdir: ")?)?)
        .canonicalize()
        .ok()?;
    if git_dir.parent()? != common.join("worktrees") {
        return None;
    }
    let registration = read_worktree_pointer(&git_dir.join("gitdir"))?;
    (git_dir
        .join(git_path_record(&registration)?)
        .canonicalize()
        .ok()?
        == dot_git)
        .then_some(())
}

/// Missing directories and aliases cannot qualify. Other I/O errors prevent
/// the caller from deciding whether an existing directory can qualify.
fn existing_project_path(path: &str) -> std::io::Result<Option<&Path>> {
    if !crate::store::cloud_capture::sync::valid_project_path(path) {
        return Ok(None);
    }
    let path = Path::new(path);
    let inspected = || -> std::io::Result<_> {
        if !path.symlink_metadata()?.is_dir() {
            return Ok(None);
        }
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
fn worktree_context(list: &str, separator: char, repo: &Path, common: &Path) -> Option<Context> {
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
        && existing_project_path(main)
            .ok()
            .flatten()
            .is_some_and(|path| {
                if bare {
                    return path == common;
                }
                // A copied .git pointer can refer to the parent without proving its path.
                let dot_git = path.join(".git");
                dot_git == common && dot_git.symlink_metadata().is_ok_and(|meta| meta.is_dir())
            });
    let linked: Vec<_> = entries.flatten().take(100).collect();
    let mut complete = linked.len() < 100;
    let mut folders = Vec::new();
    if verified_main && !bare {
        folders.push(main.to_owned());
    }
    for &(root, bare, prunable) in linked.iter().take(99) {
        if !bare && !prunable {
            match existing_project_path(root) {
                Ok(Some(path)) if registered_linked_checkout(path, common).is_some() => {
                    folders.push(root.to_owned());
                }
                Err(_) => complete = false,
                _ => {}
            }
        }
    }
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
    let path = if environment == "native" {
        existing_project_path(cwd)
    } else {
        Ok(None)
    };
    context_for_path(cwd, path)
}

fn context_for_path(cwd: &str, path: std::io::Result<Option<&Path>>) -> Context {
    let inspection_failed = || Context::unavailable(cwd, true);
    let path = match path {
        Ok(Some(path)) => path,
        Ok(None) => return Context::unavailable(cwd, false),
        Err(_) => return inspection_failed(),
    };
    use antiburn_local::platform::git;
    tauri::async_runtime::block_on(async {
        match git::repo_root_if_any_at(path).await {
            Ok(Some(repo)) => {
                let environment =
                    antiburn_local::platform::environment::DiscoveryEnvironment::Native;
                let Ok(mut output) = git::run_git_output_in_environment(
                    &environment,
                    Some(&repo),
                    &["worktree", "list", "--porcelain", "-z"],
                    &[],
                )
                .await
                else {
                    return inspection_failed();
                };
                let mut separator = '\0';
                if output.status.code() == Some(129) {
                    // Older Git versions do not accept `-z` for this command.
                    let Ok(legacy) = git::run_git_output_in_environment(
                        &environment,
                        Some(&repo),
                        &["worktree", "list", "--porcelain"],
                        &[],
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
                let Some(common) = worktree_common_dir(&environment, &repo).await else {
                    return inspection_failed();
                };
                worktree_context(list, separator, &repo, &common).unwrap_or_else(inspection_failed)
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
) -> Result<(Vec<SourceSelection>, bool), String> {
    let mut roots = BTreeSet::new();
    let mut after = String::new();
    let mut partial = false;
    loop {
        let paths = store
            .cloud_claude_source_paths_page(&after, 512)
            .map_err(|_| "Could not read cached session source metadata.")?;
        if paths.is_empty() {
            break;
        }
        after = paths.last().cloned().unwrap_or_default();
        for path in paths {
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
            let Some(root) = root.filter(|root| root.is_dir()) else {
                continue;
            };
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

/// Identify an origin group without changing legacy path-based IDs.
pub(super) fn repository_group_id(origin: &str) -> String {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    // A prefix that is not a JSON array separates this domain from path IDs.
    let mut hash = Sha256::new();
    hash.update(b"antiburn-repository-origin-v1\0");
    hash.update(origin.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash.finalize())
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
        Self {
            checkouts: CheckoutOriginCache::default(),
            paths: BTreeMap::new(),
            deadline: tokio::time::Instant::now() + PROJECT_ORIGIN_LOOKUP_BUDGET,
        }
    }
}

impl RequestOrigins {
    /// Use one deadline for all Git lookups in this request. Reuse completed
    /// proof for the same path, but never treat skipped Git as an exact folder.
    fn get(&mut self, path: &str) -> ProjectOrigin {
        if cfg!(not(unix)) {
            return ProjectOrigin::ExactGit;
        }
        if let Some(saved) = self.paths.get(path) {
            return saved.clone();
        }
        let checked = if tokio::time::Instant::now() < self.deadline {
            tauri::async_runtime::block_on(async {
                if tokio::time::Instant::now() >= self.deadline {
                    return None;
                }
                tokio::time::timeout_at(self.deadline, self.checkouts.lookup(Path::new(path)))
                    .await
                    .ok()
            })
        } else {
            None
        };
        let checked = match checked {
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
            None => ProjectOrigin::Incomplete,
            Some(CheckoutOrigin::UnverifiedGit) if tokio::time::Instant::now() >= self.deadline => {
                ProjectOrigin::Incomplete
            }
            Some(CheckoutOrigin::UnverifiedGit) => tauri::async_runtime::block_on(async {
                let path = Path::new(path);
                let probe =
                    tokio::time::timeout_at(self.deadline, tokio::fs::canonicalize(path)).await;
                match probe {
                    Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                        ProjectOrigin::Unavailable
                    }
                    Ok(Ok(canonical)) if canonical != path => ProjectOrigin::Unavailable,
                    _ => ProjectOrigin::Incomplete,
                }
            }),
        };
        if self.paths.len() < REQUEST_CONTEXT_ENTRIES {
            self.paths.insert(path.into(), checked.clone());
        }
        checked
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
            environment: suggestion.environment.clone(),
            path: context.main.clone(),
            label: Path::new(&context.main)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&context.main)
                .into(),
            worktree_count: context.folders.len().saturating_sub(1) as u32,
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

/// Build one discovery page using only the current account's saved provider
/// preferences. Suggestions and source access never create upload consent.
fn discover_projects(
    store: &Store,
    cache: &mut Cache,
    engine: &Engine,
    binding: &DestinationBinding,
    after: Option<&Cursor>,
    known: (Vec<SourceSelection>, bool),
    current: &dyn Fn() -> bool,
) -> Result<Discovery, String> {
    let session = engine.session().ok_or("Cloud account changed.")?;
    if !current() || !binding.matches(session) {
        return Err("Cloud account changed.".into());
    }
    let saved = store
        .cloud_draft(&session.user.id, &session.workspace.id)
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
    let cached_opencode_ids = store
        .cloud_opencode_session_ids(
            &suggestions
                .iter()
                .filter(|suggestion| suggestion.environment == "native")
                .map(|suggestion| suggestion.cwd.clone())
                .collect::<Vec<_>>(),
            PROJECT_SCAN_PAGE_CANDIDATES,
        )
        .map_err(|_| "Could not read cached session identities.")?;
    let mut groups = BTreeMap::new();
    let mut origins = RequestOrigins::default();
    cache.begin_scan();
    for suggestion in suggestions {
        if !current() {
            return Err("Cloud account changed.".into());
        }
        let context = cache.context(&suggestion.environment, &suggestion.cwd, &mut origins);
        partial |= context.inspection_incomplete;
        if suggestion.environment == "native" && !context.complete && !context.inspection_incomplete
        {
            continue;
        }
        add_suggestion(&mut groups, suggestion, &context);
    }
    let mut folder_path_bytes = groups
        .values()
        .flat_map(|group| &group.folder_paths)
        .map(String::len)
        .sum::<usize>();
    let mut qualified = vec![false; roots.len()];
    let mut project_choices = 0usize;
    let mut scans = ProjectScans::new(roots.iter().collect());
    while let Some((index, listing)) = scans.next(
        current,
        |_| true,
        |_| Ok(Cow::Borrowed(&cached_opencode_ids)),
    ) {
        let root = &roots[index];
        let listing = match listing {
            Ok(candidates) => candidates,
            Err(_) => {
                partial = true;
                continue;
            }
        };
        partial |= listing.partial;
        for candidate in listing.candidates {
            if !current() {
                return Err("Cloud account changed.".into());
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
                let Some(project) = path.to_str() else {
                    partial = true;
                    continue;
                };
                let context = cache.context("native", project, &mut origins);
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
        qualified[index] = true;
    }
    if !current() || engine.session().is_none() {
        return Err("Cloud account changed.".into());
    }
    partial |= !scans.pending.is_empty();
    Ok(Discovery {
        groups: groups.into_values().collect(),
        source_roots: roots
            .into_iter()
            .zip(qualified)
            .filter_map(|(root, qualified)| qualified.then_some(root))
            .collect(),
        next_cursor,
        partial,
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
    let work = app.state::<Controller>().blocking.clone();
    work.run(Priority::Normal, move || {
        let controller = app.state::<Controller>();
        let generation = controller.account_generation.load(Ordering::Acquire);
        let current = || controller.account_generation.load(Ordering::Acquire) == generation;
        let mut engine = controller
            .engine
            .lock()
            .map_err(|_| "Cloud is unavailable.")?;
        if engine.generation != generation {
            return Err("Cloud account changed.".into());
        }
        let refreshed = engine.refresh(now(), current);
        if let Err(error) = refreshed {
            engine.status = error.into();
        }
        super::emit_snapshot(&app, super::snapshot(&app, &engine));
        refreshed.map_err(|_| "Connect to Cloud before choosing projects.")?;
        if !engine
            .session()
            .is_some_and(|session| binding.matches(session))
        {
            return Err("Cloud account changed.".into());
        }
        let store = app.state::<Store>();
        ensure_cloud_setup(&controller.setup_ready, &store)?;
        let mut cache = controller
            .project_cache
            .lock()
            .map_err(|_| "Project discovery is unavailable.")?;
        let (mut known, partial) = known_sources(
            &store,
            antiburn_local::discovery::agents::opencode::native_database_paths(),
        )?;
        let (file_roots, file_partial) = super::file_sources::roots();
        known.extend(file_roots);
        discover_projects(
            &store,
            &mut cache,
            &engine,
            &binding,
            after.as_ref(),
            (known, partial || file_partial),
            &current,
        )
    })
    .await
}

#[cfg(all(test, unix))]
#[path = "tests/projects_integration_tests.rs"]
mod tests;
