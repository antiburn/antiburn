//! Find project-only session sources in the provider roots used by local discovery.
//! The capture library validates native ownership before reading payload.

use crate::store::cloud_capture::{SourceKind, SourceSelection};
use antiburn_cloud_capture::{Candidate, Error, Limits, reopen_codex, reopen_pi};
#[cfg(unix)]
use antiburn_cloud_capture::{
    discover_cursor_cli_page, discover_cursor_ide_page, reopen_cursor_cli, reopen_cursor_ide,
};
#[cfg(unix)]
use antiburn_local::discovery::agents::cursor::DISK_CURSOR;
use antiburn_local::discovery::{
    AgentExplorer,
    agents::{codex::CodexExplorer, omp::OmpExplorer, pi::PiExplorer},
    home_dir,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Reuse local provider root selection, including supported environment overrides.
/// An OMP root cannot be selected as Pi even if both adapters name it.
pub(super) fn roots() -> (Vec<SourceSelection>, bool) {
    let Some(home) = home_dir() else {
        return (Vec::new(), false);
    };
    let omp = OmpExplorer
        .watch_roots(&home)
        .into_iter()
        .filter_map(|root| root.path.canonicalize().ok())
        .collect::<Vec<_>>();
    let roots: Vec<_> = [
        (SourceKind::CodexJsonl, CodexExplorer.watch_roots(&home)),
        (SourceKind::PiJsonl, PiExplorer.watch_roots(&home)),
    ]
    .into_iter()
    .flat_map(|(source_kind, roots)| {
        roots.into_iter().filter_map({
            let omp = &omp;
            move |root| {
                let path = root.path;
                if !path.is_dir()
                    || path.canonicalize().ok().as_deref() != Some(&path)
                    || (source_kind == SourceKind::PiJsonl
                        && omp
                            .iter()
                            .any(|omp| path.starts_with(omp) || omp.starts_with(&path)))
                {
                    return None;
                }
                Some(SourceSelection {
                    source_kind,
                    path: path.to_str()?.into(),
                })
            }
        })
    })
    .collect();
    #[cfg(unix)]
    {
        let (cursor, partial) = cursor_roots(&home);
        (roots.into_iter().chain(cursor).collect(), partial)
    }
    #[cfg(not(unix))]
    (roots, false)
}

/// List exact Cursor storage roots without probing session bodies or decoding project paths.
#[cfg(unix)]
fn cursor_roots(home: &Path) -> (Vec<SourceSelection>, bool) {
    let paths = DISK_CURSOR.surface_paths(home);
    let limit = Limits::default().discovery_entries;
    let mut roots = Vec::new();
    let mut partial = false;
    for (source_kind, roots_for_kind) in [
        (SourceKind::CursorIdeComposer, paths.ide_desktop),
        (SourceKind::CursorCliAgentJsonl, paths.cli),
    ] {
        for parent in roots_for_kind {
            if source_kind == SourceKind::CursorCliAgentJsonl
                && parent.file_name().is_none_or(|name| name != "projects")
            {
                continue;
            }
            let children = match std::fs::read_dir(&parent) {
                Ok(children) => children,
                Err(error) => {
                    partial |= error.kind() != std::io::ErrorKind::NotFound;
                    continue;
                }
            };
            for (index, entry) in children.take(limit + 1).enumerate() {
                if index == limit {
                    partial = true;
                    break;
                }
                let Ok(entry) = entry else {
                    partial = true;
                    continue;
                };
                match entry.file_type() {
                    Ok(kind) if kind.is_dir() => {}
                    Ok(_) => continue,
                    Err(_) => {
                        partial = true;
                        continue;
                    }
                }
                let path = if source_kind == SourceKind::CursorCliAgentJsonl {
                    entry.path().join("agent-transcripts")
                } else {
                    let path = entry.path();
                    if !path.join("workspace.json").is_file() || !path.join("state.vscdb").is_file()
                    {
                        continue;
                    }
                    path
                };
                if path.is_dir()
                    && path.canonicalize().ok().as_deref() == Some(&path)
                    && let Some(path) = path.to_str()
                {
                    roots.push(SourceSelection {
                        source_kind,
                        path: path.into(),
                    });
                }
            }
        }
    }
    (roots, partial)
}

/// Return one native candidate per unambiguous filename ID, within the directory
/// budget. Do not follow links or guess at a source from a conflicting filename.
fn paths(
    kind: SourceKind,
    root: &Path,
    limits: &Limits,
) -> Result<BTreeMap<String, Option<PathBuf>>, Error> {
    if !root.is_absolute() || root.canonicalize().ok().as_deref() != Some(root) {
        return Err(Error::InvalidSource);
    }
    let (explorer, max_depth): (&dyn AgentExplorer, usize) = match kind {
        SourceKind::CodexJsonl => (&CodexExplorer, 3),
        SourceKind::PiJsonl => (&PiExplorer, 1),
        _ => return Err(Error::InvalidSource),
    };
    let mut directories = vec![(root.to_owned(), 0)];
    let mut entries = 0;
    let mut files = BTreeMap::new();
    while let Some((directory, depth)) = directories.pop() {
        for entry in std::fs::read_dir(directory).map_err(|_| Error::InvalidSource)? {
            if entries >= limits.discovery_entries {
                return Err(Error::Limit);
            }
            entries += 1;
            let entry = entry.map_err(|_| Error::InvalidSource)?;
            let file_type = entry.file_type().map_err(|_| Error::InvalidSource)?;
            let path = entry.path();
            if file_type.is_dir() && depth < max_depth {
                directories.push((path, depth + 1));
            } else if file_type.is_file()
                && path.extension().is_some_and(|ext| ext == "jsonl")
                && let Some(id) = explorer.recover_session_id_from_path(&path)
            {
                files
                    .entry(id)
                    .and_modify(|file| *file = None)
                    .or_insert(Some(path));
            }
        }
    }
    Ok(files)
}

fn open(kind: SourceKind, path: &Path, id: &str, limits: &Limits) -> Result<Candidate, Error> {
    match kind {
        SourceKind::CodexJsonl => reopen_codex(path, id, limits),
        SourceKind::PiJsonl => reopen_pi(path, id, limits),
        _ => Err(Error::InvalidSource),
    }
}

/// One provider page. Unknown project membership prevents a complete count.
pub(super) struct Page {
    pub(super) candidates: Vec<Candidate>,
    pub(super) next_cursor: Option<String>,
    pub(super) partial: bool,
    /// Budget used, including entries that did not produce candidates.
    pub(super) visited: usize,
}

/// Keep healthy candidates when another file cannot be opened or has a bad header.
/// Report the gap because the unreadable session may belong to a saved folder.
pub(super) fn discover_page(
    kind: SourceKind,
    root: &Path,
    after: Option<&str>,
    limits: &Limits,
) -> Result<Page, Error> {
    #[cfg(unix)]
    match kind {
        SourceKind::CursorIdeComposer => {
            let page = discover_cursor_ide_page(root, after, limits)?;
            return Ok(Page {
                // The provider does not report how many IDs it checked.
                visited: limits.discovery_sessions,
                candidates: page.candidates,
                next_cursor: page.next_cursor,
                partial: page.partial,
            });
        }
        SourceKind::CursorCliAgentJsonl => {
            let page = discover_cursor_cli_page(root, after, limits)?;
            return Ok(Page {
                candidates: page.candidates,
                next_cursor: page.next_cursor,
                partial: page.partial,
                visited: page.visited,
            });
        }
        _ => {}
    }
    let mut candidates = Vec::new();
    let mut cursor = None;
    let mut partial = false;
    let mut visited = 0;
    for (id, path) in paths(kind, root, limits)?.into_iter().rev() {
        if after.is_some_and(|after| id.as_str() >= after) {
            continue;
        }
        if visited == limits.discovery_sessions {
            return Ok(Page {
                visited,
                candidates,
                next_cursor: cursor,
                partial,
            });
        }
        visited += 1;
        if let Some(path) = path
            && let Ok(candidate) = open(kind, &path, &id, limits)
        {
            candidates.push(candidate);
        } else {
            partial = true;
        }
        cursor = Some(id);
    }
    Ok(Page {
        visited,
        candidates,
        next_cursor: None,
        partial,
    })
}

/// Follow Cursor continuations within setup limits. Keep candidates when the
/// listing is incomplete, but never use a partial listing as absence evidence.
#[cfg(all(test, unix))]
pub(super) fn discover_cursor_sources(
    kind: SourceKind,
    root: &Path,
    limits: &Limits,
    current: &dyn Fn() -> bool,
) -> Result<Page, Error> {
    let mut scan = CursorScan::new();
    let mut page = scan.read(kind, root, limits, current)?;
    page.partial |= page.next_cursor.is_some();
    Ok(page)
}

/// Keep the Cursor page and time limits when a folder scan splits its budget.
pub(super) struct CursorScan {
    deadline: std::time::Instant,
    pages_left: usize,
    after: Option<String>,
}

impl CursorScan {
    pub(super) fn new() -> Self {
        Self {
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(30),
            pages_left: 64,
            after: None,
        }
    }

    /// Return the continuation when only this read's entry budget runs out.
    pub(super) fn read(
        &mut self,
        kind: SourceKind,
        root: &Path,
        limits: &Limits,
        current: &dyn Fn() -> bool,
    ) -> Result<Page, Error> {
        let mut result = Page {
            candidates: Vec::new(),
            next_cursor: self.after.take(),
            partial: false,
            visited: 0,
        };
        while result.visited < limits.discovery_sessions {
            if !current() {
                return Err(Error::SourceChanged);
            }
            if self.pages_left == 0 || std::time::Instant::now() >= self.deadline {
                result.next_cursor = None;
                result.partial = true;
                break;
            }
            self.pages_left -= 1;
            let page = discover_page(
                kind,
                root,
                result.next_cursor.as_deref(),
                &Limits {
                    discovery_sessions: limits.discovery_sessions - result.visited,
                    ..limits.clone()
                },
            )?;
            result.partial |= page.partial;
            result.visited += page.visited;
            result.candidates.extend(page.candidates);
            result.next_cursor = page.next_cursor;
            if result.next_cursor.is_none() {
                return Ok(result);
            }
        }
        self.after.clone_from(&result.next_cursor);
        Ok(result)
    }
}

/// Reopen the exact native ID after a proved pre-capture capacity refusal.
/// The scheduler must still compare its frozen generation and project binding.
pub(super) fn reopen(
    kind: SourceKind,
    root: &Path,
    key: &str,
    limits: &Limits,
) -> Result<Candidate, Error> {
    #[cfg(unix)]
    match kind {
        SourceKind::CursorIdeComposer => {
            let id = key
                .strip_prefix("cursor/ide/")
                .ok_or(Error::InvalidIdentity)?;
            return reopen_cursor_ide(root, id, limits);
        }
        SourceKind::CursorCliAgentJsonl => {
            let id = key
                .strip_prefix("cursor/cli/")
                .ok_or(Error::InvalidIdentity)?;
            return reopen_cursor_cli(root, id, limits);
        }
        _ => {}
    }
    let id = key
        .strip_prefix(kind.upload_names().0)
        .and_then(|key| key.strip_prefix('/'))
        .ok_or(Error::InvalidIdentity)?;
    let path = paths(kind, root, limits)?
        .remove(id)
        .flatten()
        .ok_or(Error::SourceChanged)?;
    open(kind, &path, id, limits)
}
