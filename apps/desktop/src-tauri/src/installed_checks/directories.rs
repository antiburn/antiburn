use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use antiburn_local::paths::protected;
use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri_plugin_opener::OpenerExt;

use crate::analytics::event::BurnCheckDirectoryOpenOutcome;
use crate::store::Store;

const MAX_DIRECTORIES: usize = 256;
const MAX_REPOSITORY_ROWS: usize = 4096;
const MAX_DIRECTORY_ENTRIES: usize = 4096;
const REPOSITORY_ROWS_QUERY: &str =
    "SELECT key, full_name, repo_root, wsl_distro, enabled FROM repository ORDER BY key LIMIT ?1";
const READ_ERROR: &str = "Could not read check folders. Refresh and try again.";
const ACCESS_MESSAGE: &str = "Allow access to this folder in Settings → Sources, then try again.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum DirectoryScope {
    Global,
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum DirectoryState {
    Ready,
    Missing,
    Paused,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckDirectory {
    reference: Option<String>,
    scope: DirectoryScope,
    label: String,
    path: String,
    state: DirectoryState,
    file_count: Option<usize>,
    message: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    path: PathBuf,
    scope: DirectoryScope,
    enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectoryBinding {
    candidate: Candidate,
    canonical: PathBuf,
}

type Bindings = BTreeMap<String, DirectoryBinding>;
type ReadResult = Result<Vec<CheckDirectory>, String>;

#[derive(Default)]
struct Snapshot {
    generation: u64,
    result: Option<ReadResult>,
    bindings: Bindings,
}

#[derive(Default)]
pub(crate) struct DirectoryInventory {
    read_lock: Arc<tokio::sync::Mutex<()>>,
    snapshot: RwLock<Snapshot>,
}

impl DirectoryInventory {
    fn generation(&self) -> u64 {
        self.snapshot
            .read()
            .expect("check directory snapshot")
            .generation
    }

    fn binding(&self, reference: &str) -> Option<DirectoryBinding> {
        self.snapshot
            .read()
            .expect("check directory snapshot")
            .bindings
            .get(reference)
            .cloned()
    }

    fn complete(&self, result: Result<(Vec<CheckDirectory>, Bindings), String>) -> ReadResult {
        let mut snapshot = self.snapshot.write().expect("check directory snapshot");
        let result = result.map(|(rows, bindings)| {
            snapshot.bindings = bindings;
            rows
        });
        snapshot.generation += 1;
        snapshot.result = Some(result.clone());
        result
    }
}

async fn read_inventory(
    inventory: Arc<DirectoryInventory>,
    read: impl FnOnce(&Bindings) -> Result<(Vec<CheckDirectory>, Bindings), String> + Send + 'static,
) -> ReadResult {
    let generation = inventory.generation();
    let guard = inventory.read_lock.clone().lock_owned().await;
    {
        let snapshot = inventory.snapshot.read().expect("check directory snapshot");
        if snapshot.generation != generation {
            return snapshot.result.clone().expect("completed directory read");
        }
    }
    tauri::async_runtime::spawn_blocking(move || {
        // Keep the lock in the worker if the calling window drops its request.
        let _guard = guard;
        let previous = inventory
            .snapshot
            .read()
            .expect("check directory snapshot")
            .bindings
            .clone();
        let result = read(&previous);
        inventory.complete(result)
    })
    .await
    .map_err(|_| READ_ERROR.to_owned())?
}

struct DirectoryContext {
    home: PathBuf,
    canonical_home: PathBuf,
    granted: HashSet<String>,
    candidates: Vec<Candidate>,
    remote: RemoteCache,
}

struct RemoteCache {
    path: PathBuf,
    canonical: PathBuf,
}

impl DirectoryContext {
    fn blocked(&self, path: &Path) -> bool {
        protected::cwd_resolution_blocked_in(&self.home, path, &self.granted)
            || protected::cwd_resolution_blocked_in(&self.canonical_home, path, &self.granted)
    }

    fn remote(&self, path: &Path) -> bool {
        let path = crate::commands::presentable(path.to_path_buf());
        path.starts_with(crate::commands::presentable(self.remote.path.clone()))
            || path.starts_with(crate::commands::presentable(self.remote.canonical.clone()))
    }
}

fn candidates(
    home: &Path,
    repositories: BTreeMap<PathBuf, bool>,
) -> Result<Vec<Candidate>, String> {
    if !home.is_absolute() {
        return Err(READ_ERROR.into());
    }
    if repositories.len() >= MAX_DIRECTORIES {
        return Err(
            "There are too many project folders to read. Check folder counts are unavailable."
                .into(),
        );
    }
    let global = home.join(".antiburn/checks");
    let mut candidates = vec![Candidate {
        path: global.clone(),
        scope: DirectoryScope::Global,
        enabled: true,
    }];
    candidates.extend(repositories.into_iter().filter_map(|(root, enabled)| {
        let path = root.join(".antiburn/checks");
        (path != global).then_some(Candidate {
            path,
            scope: DirectoryScope::Project,
            enabled,
        })
    }));
    Ok(candidates)
}

struct RepositoryDirectoryRow {
    key: String,
    full_name: String,
    repo_root: Option<String>,
    wsl_distro: Option<String>,
    enabled: bool,
}

fn known_native_repositories(store: &Store) -> Result<BTreeMap<PathBuf, bool>, String> {
    let mut bounded_rows = {
        let connection = store.lock();
        let mut statement = connection
            .prepare(REPOSITORY_ROWS_QUERY)
            .map_err(|_| READ_ERROR.to_owned())?;
        let rows = statement
            .query_map([MAX_REPOSITORY_ROWS + 1], |row| {
                Ok(RepositoryDirectoryRow {
                    key: row.get(0)?,
                    full_name: row.get(1)?,
                    repo_root: row.get(2)?,
                    wsl_distro: row.get(3)?,
                    enabled: row.get(4)?,
                })
            })
            .map_err(|_| READ_ERROR.to_owned())?;
        let mut bounded_rows = Vec::new();
        for (index, row) in rows.enumerate() {
            if index == MAX_REPOSITORY_ROWS {
                return Err("There are too many project records to read. Check folder counts are unavailable.".into());
            }
            bounded_rows.push(row.map_err(|_| READ_ERROR.to_owned())?);
        }
        bounded_rows
    };
    bounded_rows
        .sort_by(|left, right| (&left.full_name, &left.key).cmp(&(&right.full_name, &right.key)));
    let mut repositories = BTreeMap::new();
    for row in bounded_rows {
        if let Some((root, enabled)) =
            super::native_repository(row.repo_root, row.wsl_distro, row.enabled)
        {
            repositories.insert(root, enabled);
            if repositories.len() >= MAX_DIRECTORIES {
                return Err("There are too many project folders to read. Check folder counts are unavailable.".into());
            }
        }
    }
    Ok(repositories)
}

fn resolve_home(home: &Path) -> Result<PathBuf, String> {
    if !home.is_absolute() {
        return Err(READ_ERROR.into());
    }
    let canonical = std::fs::canonicalize(home).map_err(|_| READ_ERROR.to_owned())?;
    if !std::fs::metadata(&canonical)
        .map_err(|_| READ_ERROR.to_owned())?
        .is_dir()
    {
        return Err(READ_ERROR.into());
    }
    Ok(canonical)
}

fn context(store: &Store, home: PathBuf, remote: RemoteCache) -> Result<DirectoryContext, String> {
    let canonical_home = resolve_home(&home)?;
    let granted = store.granted_dirs().map_err(|_| READ_ERROR.to_owned())?;
    let candidates = candidates(&home, known_native_repositories(store)?)?;
    Ok(DirectoryContext {
        home,
        canonical_home,
        granted,
        candidates,
        remote,
    })
}

fn policy_is_current(store: &Store, context: &DirectoryContext) -> bool {
    if !resolve_home(&context.home).is_ok_and(|home| home == context.canonical_home) {
        return false;
    }
    let Ok(granted) = store.granted_dirs() else {
        return false;
    };
    let Ok(repositories) = known_native_repositories(store) else {
        return false;
    };
    granted == context.granted
        && candidates(&context.home, repositories)
            .is_ok_and(|candidates| candidates == context.candidates)
}

fn remote_cache(app: &tauri::AppHandle) -> Result<RemoteCache, String> {
    let data = app
        .path()
        .app_data_dir()
        .map_err(|_| READ_ERROR.to_owned())?;
    remote_cache_at(data)
}

fn remote_cache_at(data: PathBuf) -> Result<RemoteCache, String> {
    let root = data.join("remote-sessions");
    let canonical = match std::fs::canonicalize(&root) {
        Ok(root) => Ok(root),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Resolve the existing parent without creating remote storage on a read.
            std::fs::canonicalize(data)
                .map(|data| data.join("remote-sessions"))
                .map_err(|_| READ_ERROR.to_owned())
        }
        Err(_) => Err(READ_ERROR.to_owned()),
    }?;
    Ok(RemoteCache {
        path: root,
        canonical,
    })
}

fn count_files(
    entries: impl IntoIterator<Item = std::io::Result<std::fs::DirEntry>>,
) -> std::io::Result<usize> {
    let mut count = 0;
    for (index, entry) in entries.into_iter().enumerate() {
        if index == MAX_DIRECTORY_ENTRIES {
            return Err(std::io::Error::other("Check folder entry limit"));
        }
        let entry = entry?;
        if super::loader::has_check_extension(&entry.path()) && entry.file_type()?.is_file() {
            count += 1;
            if count > super::MAX_DEFINITIONS {
                return Err(std::io::Error::other("Check file limit"));
            }
        }
    }
    Ok(count)
}

fn inspect(
    candidate: &Candidate,
    context: &DirectoryContext,
) -> Result<(PathBuf, usize), DirectoryOpenError> {
    if context.blocked(&candidate.path) {
        return Err(DirectoryOpenError::unavailable(ACCESS_MESSAGE));
    }
    if context.remote(&candidate.path) {
        return Err(DirectoryOpenError::unavailable(
            "Cached remote folders are not local check folders.",
        ));
    }
    let canonical =
        std::fs::canonicalize(&candidate.path).map_err(DirectoryOpenError::inaccessible)?;
    if context.blocked(&canonical) {
        return Err(DirectoryOpenError::unavailable(ACCESS_MESSAGE));
    }
    if context.remote(&canonical) {
        return Err(DirectoryOpenError::unavailable(
            "Cached remote folders are not local check folders.",
        ));
    }
    if !std::fs::metadata(&canonical)
        .map_err(DirectoryOpenError::inaccessible)?
        .is_dir()
    {
        return Err(DirectoryOpenError::unavailable(
            "The check folder path is not a directory.",
        ));
    }
    let entries = std::fs::read_dir(&canonical).map_err(DirectoryOpenError::inaccessible)?;
    let count = count_files(entries).map_err(|_| {
        DirectoryOpenError::unavailable(
            "The check folder listing is incomplete. Check folder access and limits, then refresh.",
        )
    })?;
    if std::fs::canonicalize(&candidate.path).map_err(DirectoryOpenError::inaccessible)?
        != canonical
    {
        return Err(DirectoryOpenError::unavailable(
            "The check folder changed. Refresh and try again.",
        ));
    }
    Ok((canonical, count))
}

fn scan(
    context: &DirectoryContext,
    previous: &Bindings,
) -> Result<(Vec<CheckDirectory>, Bindings), String> {
    let mut bindings = BTreeMap::new();
    let mut rows = Vec::new();
    for candidate in &context.candidates {
        let mut row = CheckDirectory {
            reference: None,
            scope: candidate.scope,
            label: match candidate.scope {
                DirectoryScope::Global => "Global check files".into(),
                DirectoryScope::Project => candidate
                    .path
                    .parent()
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Project check files".into()),
            },
            path: crate::commands::presentable(candidate.path.clone())
                .to_string_lossy()
                .into_owned(),
            state: DirectoryState::Paused,
            file_count: None,
            message: Some(
                "This repository is disabled. Enable it in Settings → Sources to read its check files.",
            ),
        };
        if candidate.enabled {
            match inspect(candidate, context) {
                Ok((canonical, count)) => {
                    let binding = DirectoryBinding {
                        candidate: candidate.clone(),
                        canonical,
                    };
                    let reference =
                        match previous.iter().find(|(_, previous)| **previous == binding) {
                            Some((reference, _)) => reference.clone(),
                            None => {
                                let mut bytes = [0_u8; 16];
                                getrandom::fill(&mut bytes).map_err(|_| READ_ERROR.to_owned())?;
                                format!("check-directory:{}", super::hex(&bytes))
                            }
                        };
                    bindings.insert(reference.clone(), binding);
                    row.reference = Some(reference);
                    row.state = DirectoryState::Ready;
                    row.file_count = Some(count);
                    row.message = None;
                }
                Err(error) => {
                    row.state = if error.outcome == BurnCheckDirectoryOpenOutcome::Missing {
                        DirectoryState::Missing
                    } else {
                        DirectoryState::Unavailable
                    };
                    row.message = Some(error.message);
                }
            }
        }
        rows.push(row);
    }
    Ok((rows, bindings))
}

#[derive(Debug)]
struct DirectoryOpenError {
    outcome: BurnCheckDirectoryOpenOutcome,
    message: &'static str,
}

impl DirectoryOpenError {
    fn unavailable(message: &'static str) -> Self {
        Self {
            outcome: BurnCheckDirectoryOpenOutcome::Unavailable,
            message,
        }
    }

    fn missing() -> Self {
        Self {
            outcome: BurnCheckDirectoryOpenOutcome::Missing,
            message: "This check folder is no longer available. Refresh and try again.",
        }
    }

    fn inaccessible(error: std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::NotFound {
            Self::missing()
        } else {
            Self::unavailable(
                "The check folder cannot be accessed. Check folder access and permissions.",
            )
        }
    }

    fn open_failed() -> Self {
        Self {
            outcome: BurnCheckDirectoryOpenOutcome::OpenFailed,
            message: "The check folder could not be opened. Try again.",
        }
    }
}

fn open_directory(
    binding: Option<&DirectoryBinding>,
    context: &DirectoryContext,
    policy_current: impl FnOnce() -> bool,
    open: impl FnOnce(String) -> Result<(), ()>,
) -> Result<(), DirectoryOpenError> {
    let binding = binding.ok_or_else(DirectoryOpenError::missing)?;
    if !context
        .candidates
        .iter()
        .any(|candidate| candidate == &binding.candidate && candidate.enabled)
    {
        return Err(DirectoryOpenError::unavailable(
            "This repository is no longer available for check files. Refresh and try again.",
        ));
    }
    if context.blocked(&binding.candidate.path) {
        return Err(DirectoryOpenError::unavailable(ACCESS_MESSAGE));
    }
    if context.remote(&binding.candidate.path) {
        return Err(DirectoryOpenError::unavailable(
            "Cached remote folders are not local check folders.",
        ));
    }
    let canonical =
        std::fs::canonicalize(&binding.candidate.path).map_err(DirectoryOpenError::inaccessible)?;
    if context.blocked(&canonical) {
        return Err(DirectoryOpenError::unavailable(ACCESS_MESSAGE));
    }
    if canonical != binding.canonical || context.remote(&canonical) {
        return Err(DirectoryOpenError::unavailable(
            "The check folder changed or is not local. Refresh and try again.",
        ));
    }
    if !std::fs::metadata(&canonical)
        .map_err(DirectoryOpenError::inaccessible)?
        .is_dir()
    {
        return Err(DirectoryOpenError::unavailable(
            "The check folder path is not a directory.",
        ));
    }
    let path = open_path_argument(canonical)?;
    if !policy_current() {
        return Err(DirectoryOpenError::unavailable(
            "Folder access settings changed. Refresh and try again.",
        ));
    }
    open(path).map_err(|()| DirectoryOpenError::open_failed())
}

fn open_path_argument(path: PathBuf) -> Result<String, DirectoryOpenError> {
    crate::commands::presentable(path)
        .into_os_string()
        .into_string()
        .map_err(|_| {
            DirectoryOpenError::unavailable(
                "The check folder path cannot be opened on this system.",
            )
        })
}

fn complete_open(
    result: Result<(), DirectoryOpenError>,
    record: impl FnOnce(BurnCheckDirectoryOpenOutcome),
) -> Result<(), String> {
    record(match &result {
        Ok(()) => BurnCheckDirectoryOpenOutcome::Succeeded,
        Err(error) => error.outcome,
    });
    result.map_err(|error| error.message.to_owned())
}

fn check_window(label: &str) -> Result<(), DirectoryOpenError> {
    if matches!(label, crate::settings::LABEL | crate::main_window::LABEL) {
        Ok(())
    } else {
        Err(DirectoryOpenError::unavailable(
            "This action is available only in the main window or Settings.",
        ))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenArguments {
    directory_reference: String,
}

fn open_arguments(body: &tauri::ipc::InvokeBody) -> Result<OpenArguments, DirectoryOpenError> {
    let tauri::ipc::InvokeBody::Json(body) = body else {
        return Err(DirectoryOpenError::unavailable(
            "Use the check folder's Open action.",
        ));
    };
    serde_json::from_value(body.clone())
        .map_err(|_| DirectoryOpenError::unavailable("Use the check folder's Open action."))
}

#[tauri::command]
pub(crate) async fn list_installed_check_directories(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> ReadResult {
    check_window(window.label()).map_err(|error| error.message.to_owned())?;
    let inventory = Arc::clone(&app.state::<super::InstalledChecks>().directories);
    read_inventory(inventory, move |previous| {
        let home = antiburn_local::paths::home_dir().ok_or_else(|| READ_ERROR.to_owned())?;
        let context = context(&app.state::<Store>(), home, remote_cache(&app)?)?;
        let result = scan(&context, previous)?;
        if !policy_is_current(&app.state::<Store>(), &context) {
            return Err("Folder access settings changed. Refresh and try again.".into());
        }
        Ok(result)
    })
    .await
}

#[tauri::command]
pub(crate) async fn open_installed_check_directory(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    request: tauri::ipc::Request<'_>,
) -> Result<(), String> {
    let arguments = check_window(window.label()).and_then(|()| open_arguments(request.body()));
    let result = match arguments {
        Err(error) => Err(error),
        Ok(arguments) => {
            let worker_app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let binding = worker_app
                    .state::<super::InstalledChecks>()
                    .directories
                    .binding(&arguments.directory_reference);
                if binding.is_none() {
                    return Err(DirectoryOpenError::missing());
                }
                let home = antiburn_local::paths::home_dir()
                    .ok_or_else(|| DirectoryOpenError::unavailable(READ_ERROR))?;
                let remote = remote_cache(&worker_app)
                    .map_err(|_| DirectoryOpenError::unavailable(READ_ERROR))?;
                let context = context(&worker_app.state::<Store>(), home, remote)
                    .map_err(|_| DirectoryOpenError::unavailable(READ_ERROR))?;
                open_directory(
                    binding.as_ref(),
                    &context,
                    || policy_is_current(&worker_app.state::<Store>(), &context),
                    |path| {
                        worker_app
                            .opener()
                            .open_path(path, None::<&str>)
                            .map_err(|_| ())
                    },
                )
            })
            .await
            .unwrap_or_else(|_| Err(DirectoryOpenError::open_failed()))
        }
    };
    complete_open(result, |outcome| {
        crate::analytics::record_burn_check_directory_opened(&app, outcome)
    })
}

#[cfg(test)]
mod tests;
