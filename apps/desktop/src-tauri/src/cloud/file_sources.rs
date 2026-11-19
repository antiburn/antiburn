//! Find Codex and Pi session files in the provider roots used by local discovery.
//! Filenames supply identity hints; the exporters prove the first owned header.

use crate::store::cloud_capture::{SourceKind, SourceSelection};
use antiburn_cloud_capture::{Candidate, Error, Limits, reopen_codex, reopen_pi};
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
    [
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
    .collect()
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

/// Find one page of fresh candidates. Skip malformed or in-progress files so
/// one bad header cannot hide another session. Recheck headers during capture.
pub(super) fn discover_page(
    kind: SourceKind,
    root: &Path,
    after: Option<&str>,
    limits: &Limits,
) -> Result<(Vec<Candidate>, Option<String>), Error> {
    let mut candidates = Vec::new();
    let mut cursor = None;
    for (id, path) in paths(kind, root, limits)?.into_iter().rev() {
        if after.is_some_and(|after| id.as_str() >= after) {
            continue;
        }
        let Some(path) = path else { continue };
        if let Ok(candidate) = open(kind, &path, &id, limits) {
            if candidates.len() == limits.discovery_sessions {
                return Ok((candidates, cursor));
            }
            candidates.push(candidate);
            cursor = Some(id);
        }
    }
    Ok((candidates, None))
}

/// Reopen the exact native ID after a proved pre-capture capacity refusal.
/// The scheduler must still compare its frozen generation and project binding.
pub(super) fn reopen(
    kind: SourceKind,
    root: &Path,
    key: &str,
    limits: &Limits,
) -> Result<Candidate, Error> {
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
