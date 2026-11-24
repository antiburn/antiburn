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
pub(super) fn roots() -> Vec<SourceSelection> {
    let Some(home) = home_dir() else {
        return Vec::new();
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
    let roots = roots.into_iter().chain(cursor_roots(&home)).collect();
    roots
}

/// List exact Cursor storage roots without probing session bodies or decoding project paths.
#[cfg(unix)]
fn cursor_roots(home: &Path) -> Vec<SourceSelection> {
    let paths = DISK_CURSOR.surface_paths(home);
    let limit = Limits::default().discovery_entries;
    let mut roots = Vec::new();
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
            let Ok(children) = std::fs::read_dir(&parent) else {
                continue;
            };
            for entry in children.take(limit).filter_map(std::result::Result::ok) {
                if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    continue;
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
    roots
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
                candidates: page.candidates,
                next_cursor: page.next_cursor,
                partial: false,
            });
        }
        SourceKind::CursorCliAgentJsonl => {
            let page = discover_cursor_cli_page(root, after, limits)?;
            return Ok(Page {
                candidates: page.candidates,
                next_cursor: page.next_cursor,
                partial: page.partial,
            });
        }
        _ => {}
    }
    let mut candidates = Vec::new();
    let mut cursor = None;
    let mut partial = false;
    for (id, path) in paths(kind, root, limits)?.into_iter().rev() {
        if after.is_some_and(|after| id.as_str() >= after) {
            continue;
        }
        if let Some(path) = path
            && let Ok(candidate) = open(kind, &path, &id, limits)
        {
            if candidates.len() == limits.discovery_sessions {
                return Ok(Page {
                    candidates,
                    next_cursor: cursor,
                    partial,
                });
            }
            candidates.push(candidate);
            cursor = Some(id);
        } else {
            partial = true;
        }
    }
    Ok(Page {
        candidates,
        next_cursor: None,
        partial,
    })
}

/// Finish a Cursor listing only within the setup candidate and work limits.
pub(super) fn discover_cursor_sources(
    kind: SourceKind,
    root: &Path,
    limits: &Limits,
) -> Result<Vec<Candidate>, Error> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut candidates = Vec::new();
    let mut after = None;
    for _ in 0..64 {
        if std::time::Instant::now() >= deadline || candidates.len() >= limits.discovery_sessions {
            return Err(Error::Limit);
        }
        let page = discover_page(
            kind,
            root,
            after.as_deref(),
            &Limits {
                discovery_sessions: limits.discovery_sessions - candidates.len(),
                ..limits.clone()
            },
        )?;
        if page.partial {
            return Err(Error::UnsupportedSource);
        }
        candidates.extend(page.candidates);
        let Some(next) = page.next_cursor else {
            return Ok(candidates);
        };
        after = Some(next);
    }
    Err(Error::Limit)
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
