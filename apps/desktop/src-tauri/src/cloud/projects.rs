//! Cached project suggestions and current local worktree grouping.
//! Grouping does not prove historical ownership or permit upload. Desktop capture
//! must obtain fresh recorded-CWD admission under separate saved consent.

use super::{
    Controller, DestinationBinding, Draft, Folder, engine::Engine, ensure_cloud_setup, now, sync,
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
    sync::atomic::Ordering,
};
use tauri::Manager;

const CACHE_BYTES: usize = 4 * 1024 * 1024;
const PROJECT_SCAN_ENTRIES: usize = 512;

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
        if candidate.source_kind() == antiburn_cloud_capture::CaptureSourceKind::ClaudeJsonl {
            let bytes = std::fs::symlink_metadata(candidate.source_path())
                .map_err(|_| "Session source metadata is unavailable.")?
                .len();
            if self.scan_bytes >= 64 * 1024 * 1024 {
                return Ok(None);
            }
            self.scan_bytes = self
                .scan_bytes
                .saturating_add(bytes.min(limits.source_bytes));
        }
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

    fn context(&mut self, environment: &str, cwd: &str) -> Context {
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

/// One project and its currently verified local worktree folders. Initial
/// selected state belongs to the renderer and is not stored by discovery.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Group {
    id: String,
    environment: String,
    path: String,
    label: String,
    folder_paths: Vec<String>,
    worktree_count: u32,
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
    /// An inspection failed or exceeded a supported source/selection bound.
    /// Normal provider pages and per-request scan budgets do not set this flag.
    partial: bool,
}

#[derive(Clone)]
struct Context {
    main: String,
    folders: Vec<String>,
    complete: bool,
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
    let path = text.strip_suffix('\n')?;
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

fn existing_project_path(path: &str) -> Option<&Path> {
    if !crate::store::cloud_capture::sync::valid_project_path(path) {
        return None;
    }
    let path = Path::new(path);
    (path.is_dir() && path.canonicalize().ok().as_deref() == Some(path)).then_some(path)
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
        && existing_project_path(main).is_some_and(|path| {
            if bare {
                return path == common;
            }
            // A copied .git pointer can refer to the parent without proving its path.
            let dot_git = path.join(".git");
            dot_git == common && dot_git.symlink_metadata().is_ok_and(|meta| meta.is_dir())
        });
    let linked: Vec<_> = entries.flatten().take(100).collect();
    let mut folders = Vec::new();
    if verified_main && !bare {
        folders.push(main.to_owned());
    }
    for &(root, bare, prunable) in linked.iter().take(99) {
        if !bare
            && !prunable
            && let Some(path) = existing_project_path(root)
            && registered_linked_checkout(path, common).is_some()
        {
            folders.push(root.to_owned());
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
            complete: false,
        });
    }
    folders.sort();
    folders.dedup();
    let mut complete = linked.len() < 100;
    if folders.iter().map(String::len).sum::<usize>() > 64 * 1024 {
        folders.clear();
        complete = false;
    }
    Some(Context {
        main: main.into(),
        folders,
        complete,
    })
}

/// Use current local Git context only. Foreign environment paths are never
/// probed through the local filesystem, even when their strings match.
fn context(environment: &str, cwd: &str) -> Context {
    let fallback = || Context {
        main: cwd.into(),
        folders: Vec::new(),
        complete: false,
    };
    if environment != "native" {
        return fallback();
    }
    let Some(path) = existing_project_path(cwd) else {
        return fallback();
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
                    return fallback();
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
                        return fallback();
                    };
                    output = legacy;
                    separator = '\n';
                }
                if !output.status.success() {
                    return fallback();
                }
                let Ok(list) = std::str::from_utf8(&output.stdout) else {
                    return fallback();
                };
                let Some(common) = worktree_common_dir(&environment, &repo).await else {
                    return fallback();
                };
                worktree_context(list, separator, &repo, &common).unwrap_or_else(fallback)
            }
            Ok(None) => Context {
                main: cwd.into(),
                folders: vec![cwd.into()],
                complete: true,
            },
            Err(_) => fallback(),
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

/// Confirm the selected folders against current source choices and group evidence.
/// Return the allowed roots so a caller can check bindings held after source changes.
pub(super) fn confirm_project_choices(
    store: &Store,
    cache: &mut Cache,
    binding: &DestinationBinding,
    choices: &BTreeMap<String, bool>,
    projects: &BTreeSet<ProjectSelection>,
    current: &dyn Fn() -> bool,
) -> Result<Vec<SourceSelection>, String> {
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
    let mut by_source = BTreeMap::<&SourceSelection, (BTreeSet<String>, BTreeSet<String>)>::new();
    let mut contexts = BTreeMap::new();
    for project in projects {
        let root = roots
            .iter()
            .find(|root| {
                root.source_kind == project.source_kind && root.path == project.source_path
            })
            .ok_or("A session source changed. Reload projects and confirm again.")?;
        let member = contexts
            .entry(&project.project_path)
            .or_insert_with(|| context("native", &project.project_path));
        let group = group_id("native", &member.main);
        if !member.complete
            || choices.get(&group) != Some(&true)
            || !member.folders.contains(&project.project_path)
        {
            return Err("The project folders changed. Reload projects and confirm again.".into());
        }
        let (paths, groups) = by_source.entry(root).or_default();
        paths.insert(project.project_path.clone());
        groups.insert(group);
    }
    cache.begin_scan();
    let (selected_roots, mut targets): (Vec<_>, Vec<_>) = by_source.into_iter().unzip();
    let mut scans = ProjectScans::new(selected_roots.clone());
    let mut partial = vec![false; selected_roots.len()];
    while let Some((index, listing)) = scans.next(
        current,
        |index| !targets[index].1.is_empty(),
        |index| {
            let paths = targets[index].0.iter().cloned().collect::<Vec<_>>();
            store
                .cloud_opencode_session_ids(&paths, PROJECT_SCAN_ENTRIES)
                .map(Cow::Owned)
                .map_err(|_| "Could not read cached session identities.".into())
        },
    ) {
        let root = selected_roots[index];
        let groups = &mut targets[index].1;
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
                groups.remove(&group_id("native", &cache.context("native", path).main));
            }
            if groups.is_empty() {
                break;
            }
        }
    }
    for (index, (_, groups)) in targets.into_iter().enumerate() {
        if !groups.is_empty() {
            if partial[index] || scans.cursor_incomplete(index) {
                return Err("Project discovery is incomplete. Restore access or choose fewer sources and retry.".into());
            }
            return Err("Could not confirm a supported session for this project. Reload projects and try again.".into());
        }
    }
    Ok(roots)
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
    cache: &mut Cache,
    groups: &mut BTreeMap<(String, String), Group>,
    suggestion: Suggestion,
) {
    let context = cache.context(&suggestion.environment, &suggestion.cwd);
    let key = (suggestion.environment.clone(), context.main.clone());
    let group = groups.entry(key).or_insert_with(|| Group {
        id: group_id(&suggestion.environment, &context.main),
        environment: suggestion.environment.clone(),
        path: context.main.clone(),
        label: Path::new(&context.main)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&context.main)
            .into(),
        worktree_count: context.folders.len().saturating_sub(1) as u32,
        folder_paths: context.folders,
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
            remaining: PROJECT_SCAN_ENTRIES,
        }
    }

    /// Charge a failed read its full share. Only the first read uses cached IDs.
    fn next<'h>(
        &mut self,
        current: &dyn Fn() -> bool,
        needs_more: impl Fn(usize) -> bool,
        hints: impl FnOnce(usize) -> Result<Cow<'h, [String]>, String>,
    ) -> Option<(usize, Result<super::file_sources::Page, String>)> {
        let (index, mut scan, mut share, first) = if self.first < self.roots.len() {
            let index = self.first;
            let share = self.remaining / (self.roots.len() - index);
            self.first += 1;
            (index, ProjectRootScan::new(self.roots[index]), share, true)
        } else {
            if self.remaining == 0 {
                return None;
            }
            let (index, scan) = loop {
                let (index, scan) = self.pending.pop_front()?;
                if needs_more(index) {
                    break (index, scan);
                }
            };
            (index, scan, self.remaining, false)
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
            scan.read(self.roots[index], &ids, &mut share, current)
        })();
        if let Ok(page) = &result {
            self.remaining += share;
            if page.next_cursor.is_some() {
                self.pending.push_back((index, scan));
            }
        }
        Some((index, result))
    }

    fn cursor_incomplete(&self, index: usize) -> bool {
        self.pending
            .iter()
            .any(|(pending, scan)| *pending == index && scan.cursor.is_some())
    }
}

struct ProjectRootScan {
    after: Option<String>,
    cursor: Option<super::file_sources::CursorScan>,
    seen: BTreeSet<String>,
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
            seen: BTreeSet::new(),
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
            *remaining -= page.visited;
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
                    if self.seen.insert(candidate.source_id().to_owned()) {
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
        *remaining -= page.visited;
        self.after.clone_from(&page.next_cursor);
        candidates.extend(
            std::mem::take(&mut page.candidates)
                .into_iter()
                .filter(|candidate| self.seen.insert(candidate.source_id().to_owned())),
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
            PROJECT_SCAN_ENTRIES,
        )
        .map_err(|_| "Could not read cached session identities.")?;
    let mut groups = BTreeMap::new();
    cache.begin_scan();
    for suggestion in suggestions {
        if !current() {
            return Err("Cloud account changed.".into());
        }
        add_suggestion(cache, &mut groups, suggestion);
    }
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
                Ok(None) => continue,
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
                let context = cache.context("native", project);
                if !groups.contains_key(&("native".into(), context.main.clone())) {
                    if after.is_some() {
                        continue;
                    }
                    if groups.len() >= 256
                        || groups
                            .values()
                            .map(|group| group.folder_paths.iter().map(String::len).sum::<usize>())
                            .sum::<usize>()
                            + context.folders.iter().map(String::len).sum::<usize>()
                            > 4 * 1024 * 1024
                    {
                        partial = true;
                        continue;
                    }
                    add_suggestion(
                        cache,
                        &mut groups,
                        Suggestion {
                            environment: "native".into(),
                            cwd: project.to_owned(),
                            agents: vec![root.source_kind.upload_names().0.into()],
                            session_count: 0,
                        },
                    );
                }
                if let Some(group) = groups.get_mut(&("native".into(), context.main)) {
                    if matched.insert(group.id.clone()) {
                        group.capture_session_count += 1;
                    }
                    if !evidence.scan_complete()
                        || !group.grouping_complete
                        || group.folder_paths.is_empty()
                    {
                        continue;
                    }
                    group.capture_supported = true;
                    for folder in &group.folder_paths {
                        if group.projects.iter().any(|project| {
                            project.source_kind == root.source_kind
                                && project.source_path == root.path
                                && project.project_path == *folder
                        }) {
                            continue;
                        }
                        if project_choices >= 512 {
                            partial = true;
                            group.capture_supported = false;
                            continue;
                        }
                        group.projects.push(ProjectSelection {
                            source_kind: root.source_kind,
                            source_path: root.path.clone(),
                            project_path: folder.clone(),
                        });
                        project_choices += 1;
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
    partial |= (0..roots.len()).any(|index| scans.cursor_incomplete(index));
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
