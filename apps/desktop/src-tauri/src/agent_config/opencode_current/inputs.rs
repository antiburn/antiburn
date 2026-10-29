use super::{
    ConfigContext, ConfigEntry, ConfigUnavailableReason, Dependency, OpenCodeConfigSubject,
    OperationSelector, hash,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(super) const MAX_DEPENDENCIES: usize = 4096;
const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Snapshot {
    pub(super) dependencies: Vec<Dependency>,
    contents: BTreeMap<PathBuf, Vec<u8>>,
    directories: BTreeMap<PathBuf, Vec<PathBuf>>,
    documents: BTreeMap<PathBuf, Value>,
    bytes: usize,
    #[cfg(test)]
    pub(super) file_reads: usize,
    #[cfg(test)]
    pub(super) directory_reads: usize,
}

impl Snapshot {
    fn reserve_bytes(&mut self, bytes: usize) -> Result<(), ConfigUnavailableReason> {
        let retained = self.bytes.saturating_add(bytes);
        if retained > MAX_INPUT_BYTES {
            return Err(ConfigUnavailableReason::FileTooLarge);
        }
        self.bytes = retained;
        Ok(())
    }

    fn retain_directory(
        &mut self,
        path: &Path,
        entries: Vec<PathBuf>,
    ) -> Result<(), ConfigUnavailableReason> {
        let bytes = dependency_retained_bytes(path)
            .saturating_add(map_key_retained_bytes::<Vec<PathBuf>>(path))
            .saturating_add(
                entries
                    .capacity()
                    .saturating_mul(std::mem::size_of::<PathBuf>()),
            )
            .saturating_add(
                entries
                    .iter()
                    .map(|path| path.capacity().saturating_mul(2))
                    .sum::<usize>(),
            );
        self.reserve_bytes(bytes)?;
        self.directories.insert(path.into(), entries);
        Ok(())
    }
}

fn dependency_retained_bytes(path: &Path) -> usize {
    path.as_os_str()
        .len()
        .saturating_mul(2)
        .saturating_add(2 * std::mem::size_of::<Dependency>())
}

fn map_key_retained_bytes<T>(path: &Path) -> usize {
    path.as_os_str()
        .len()
        .saturating_mul(2)
        .saturating_add(std::mem::size_of::<(PathBuf, T)>())
        .saturating_add(4 * std::mem::size_of::<usize>())
}

pub(super) fn track(path: &Path, snapshot: &mut Snapshot) -> Result<(), ConfigUnavailableReason> {
    if snapshot.dependencies.iter().any(|old| old.path == path) {
        return Ok(());
    }
    if snapshot.dependencies.len() >= MAX_DEPENDENCIES {
        return Err(ConfigUnavailableReason::FileTooLarge);
    }
    let mut bytes = 0;
    let digest = if super::path_entry_exists(path)? {
        if std::fs::symlink_metadata(path)
            .map_err(|_| ConfigUnavailableReason::UnsafePath)?
            .is_dir()
        {
            let entries = directory_entries(path)?;
            let digest = super::directory_digest(&entries)?;
            snapshot.retain_directory(path, entries)?;
            #[cfg(test)]
            {
                snapshot.directory_reads += 1;
            }
            Some(digest)
        } else {
            let file = super::read_checked(
                path,
                path.parent().ok_or(ConfigUnavailableReason::UnsafePath)?,
            )?;
            bytes = file.bytes.len();
            snapshot.reserve_bytes(
                dependency_retained_bytes(path)
                    .saturating_add(map_key_retained_bytes::<Vec<u8>>(path))
                    .saturating_add(file.bytes.capacity()),
            )?;
            let digest = hash(&file.bytes);
            snapshot.contents.insert(path.into(), file.bytes);
            #[cfg(test)]
            {
                snapshot.file_reads += 1;
            }
            Some(digest)
        }
    } else {
        snapshot.reserve_bytes(dependency_retained_bytes(path))?;
        None
    };
    snapshot.dependencies.push(Dependency {
        path: path.into(),
        digest,
        bytes,
    });
    Ok(())
}

pub(super) fn read_dependency(
    path: &Path,
    snapshot: &mut Snapshot,
) -> Result<Vec<u8>, ConfigUnavailableReason> {
    track(path, snapshot)?;
    snapshot
        .contents
        .get(path)
        .cloned()
        .ok_or(ConfigUnavailableReason::MissingTarget)
}

pub(super) fn document(
    path: &Path,
    snapshot: &mut Snapshot,
) -> Result<Value, ConfigUnavailableReason> {
    if let Some(document) = snapshot.documents.get(path) {
        return Ok(document.clone());
    }
    let document = super::super::vendors::json::parse(&read_dependency(path, snapshot)?)?;
    snapshot.reserve_bytes(
        map_key_retained_bytes::<Value>(path)
            .saturating_add(document.to_string().len().saturating_mul(64)),
    )?;
    snapshot.documents.insert(path.into(), document.clone());
    Ok(document)
}

pub(super) fn dependencies(
    context: &ConfigContext,
    entries: &[ConfigEntry],
) -> Result<Snapshot, ConfigUnavailableReason> {
    let cwd = context
        .workspace_cwd
        .as_deref()
        .ok_or(ConfigUnavailableReason::UnsafePath)?;
    let mut candidates = BTreeSet::new();
    for directory in cwd.ancestors() {
        if candidates.len() > MAX_DEPENDENCIES {
            return Err(ConfigUnavailableReason::FileTooLarge);
        }
        for file in ["opencode.json", "opencode.jsonc"] {
            candidates.insert(directory.join(file));
            candidates.insert(directory.join(".opencode").join(file));
        }
        candidates.insert(directory.join(".opencode"));
    }
    for entry in entries {
        if let Some(path) = &entry.path {
            candidates.insert(path.clone());
            if entry.info.is_none() {
                for file in ["opencode.json", "opencode.jsonc"] {
                    candidates.insert(path.join(file));
                }
            }
        }
    }
    let documents: BTreeSet<_> = entries
        .iter()
        .filter(|entry| entry.info.is_some())
        .filter_map(|entry| entry.path.as_ref())
        .collect();
    let mut snapshot = Snapshot::default();
    for path in candidates {
        track(&path, &mut snapshot)?;
        if path
            .file_name()
            .is_some_and(|name| name == "opencode.json" || name == "opencode.jsonc")
            && super::path_entry_exists(&path)?
            && !documents.contains(&path)
        {
            return Err(ConfigUnavailableReason::InvalidPrecedence);
        }
    }
    Ok(snapshot)
}

pub(super) fn directory_entries(path: &Path) -> Result<Vec<PathBuf>, ConfigUnavailableReason> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ConfigUnavailableReason::SymlinkTarget);
    }
    let mut paths = Vec::new();
    for (index, entry) in std::fs::read_dir(path)
        .map_err(|_| ConfigUnavailableReason::PermissionDenied)?
        .enumerate()
    {
        if index >= 256 {
            return Err(ConfigUnavailableReason::FileTooLarge);
        }
        let entry = entry.map_err(|_| ConfigUnavailableReason::UnsafePath)?;
        let path = entry.path();
        if path.to_str().is_none() {
            return Err(ConfigUnavailableReason::UnsafePath);
        }
        let kind = entry
            .file_type()
            .map_err(|_| ConfigUnavailableReason::UnsafePath)?;
        if kind.is_symlink() {
            return Err(ConfigUnavailableReason::SymlinkTarget);
        }
        if kind.is_dir()
            || path.extension().is_some_and(|extension| extension == "md")
            || path
                .file_name()
                .is_some_and(|name| name == "opencode.json" || name == "opencode.jsonc")
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

pub(super) fn markdown(
    bytes: &[u8],
) -> Result<(serde_yaml_ng::Value, &str), ConfigUnavailableReason> {
    let text = std::str::from_utf8(bytes).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    let (header, body) = text
        .strip_prefix("---\n")
        .and_then(|text| text.split_once("\n---\n"))
        .ok_or(ConfigUnavailableReason::MalformedConfig)?;
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(header).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    if !value.is_mapping() {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    Ok((value, body))
}

#[cfg(not(windows))]
pub(super) fn markdown_edit(
    bytes: &[u8],
    update: impl FnOnce(&mut serde_yaml_ng::Value) -> Result<(), ConfigUnavailableReason>,
) -> Result<Vec<u8>, ConfigUnavailableReason> {
    let (mut value, body) = markdown(bytes)?;
    update(&mut value)?;
    let header =
        serde_yaml_ng::to_string(&value).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    Ok(format!("---\n{header}---\n{body}").into_bytes())
}

pub(super) fn files(
    directory: &Path,
    depth: usize,
    snapshot: &mut Snapshot,
) -> Result<Vec<PathBuf>, ConfigUnavailableReason> {
    track(directory, snapshot)?;
    let Some(entries) = snapshot.directories.get(directory).cloned() else {
        if snapshot.contents.contains_key(directory) {
            return Err(ConfigUnavailableReason::SymlinkTarget);
        }
        return Ok(Vec::new());
    };
    if depth > 16 {
        return Err(ConfigUnavailableReason::FileTooLarge);
    }
    let mut files = Vec::new();
    for path in entries {
        if std::fs::symlink_metadata(&path)
            .map_err(|_| ConfigUnavailableReason::UnsafePath)?
            .is_dir()
        {
            files.extend(self::files(&path, depth + 1, snapshot)?);
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
        if files.len() > MAX_DEPENDENCIES {
            return Err(ConfigUnavailableReason::FileTooLarge);
        }
    }
    files.sort();
    Ok(files)
}

pub(super) fn agent_documents(
    directory: &Path,
    subject: &OpenCodeConfigSubject,
    snapshot: &mut Snapshot,
    winner: &mut Option<(PathBuf, OperationSelector, Value)>,
) -> Result<(), ConfigUnavailableReason> {
    let OpenCodeConfigSubject::AgentModel(id) = subject else {
        return Ok(());
    };
    for field in ["agent", "agents", "mode", "modes"] {
        for path in files(&directory.join(field), 0, snapshot)? {
            let relative = path
                .strip_prefix(directory.join(field))
                .map_err(|_| ConfigUnavailableReason::UnsafePath)?
                .with_extension("");
            if matches!(field, "mode" | "modes") && relative.components().count() != 1 {
                continue;
            }
            if relative.to_string_lossy().replace('\\', "/") != *id {
                continue;
            }
            let bytes = read_dependency(&path, snapshot)?;
            let (header, _) = markdown(&bytes)?;
            let normalized = match super::normalize::markdown_agent_model(&header) {
                Ok(value) => value,
                Err(ConfigUnavailableReason::MissingTarget) => {
                    *winner = None;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if let Some(normalized) = normalized {
                *winner = Some((
                    path,
                    OperationSelector::OpenCodeMarkdownAgentModel(id.clone()),
                    normalized,
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn noisy_non_markdown_entries_do_not_enter_the_snapshot_path_cache() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        for directory in 0..4 {
            let path = root.join(format!("directory-{directory}-{}", "d".repeat(160)));
            fs::create_dir(&path).unwrap();
            for file in 0..256 {
                fs::write(
                    path.join(format!("noise-{file}-{}.bin", "n".repeat(180))),
                    b"",
                )
                .unwrap();
            }
        }
        let mut snapshot = Snapshot::default();
        assert!(files(&root, 0, &mut snapshot).unwrap().is_empty());
        assert_eq!(snapshot.directories.len(), 5);
        assert_eq!(
            snapshot.directories.values().map(Vec::len).sum::<usize>(),
            4
        );
        assert_eq!(snapshot.file_reads, 0);
        assert!(snapshot.contents.is_empty());
        assert!(snapshot.bytes > 0);
        assert!(snapshot.bytes < 128 * 1024);

        let directory = root.join(format!("directory-0-{}", "d".repeat(160)));
        fs::write(directory.join("one-more.bin"), b"").unwrap();
        assert_eq!(
            directory_entries(&directory),
            Err(ConfigUnavailableReason::FileTooLarge)
        );
    }

    #[test]
    fn long_directory_path_listings_exhaust_the_byte_budget_before_retention() {
        let mut snapshot = Snapshot::default();
        let mut rejected = false;
        for index in 0..MAX_DEPENDENCIES {
            let directory = PathBuf::from(format!("source-{index}-{}", "d".repeat(180)));
            let entries = (0..256)
                .map(|entry| directory.join(format!("non-markdown-{entry}-{}", "p".repeat(180))))
                .collect::<Vec<_>>();
            let before = (snapshot.bytes, snapshot.directories.len());
            match snapshot.retain_directory(&directory, entries) {
                Ok(()) => assert!(snapshot.bytes <= MAX_INPUT_BYTES),
                Err(error) => {
                    assert_eq!(error, ConfigUnavailableReason::FileTooLarge);
                    assert_eq!((snapshot.bytes, snapshot.directories.len()), before);
                    assert!(!snapshot.directories.contains_key(&directory));
                    rejected = true;
                    break;
                }
            }
        }
        assert!(rejected);
        assert!(snapshot.directories.len() < MAX_DEPENDENCIES);
    }

    #[test]
    fn missing_dependency_paths_also_use_the_snapshot_byte_budget() {
        let temporary = tempfile::tempdir().unwrap();
        let mut snapshot = Snapshot {
            bytes: MAX_INPUT_BYTES - 1,
            ..Default::default()
        };
        assert_eq!(
            track(&temporary.path().join("missing"), &mut snapshot),
            Err(ConfigUnavailableReason::FileTooLarge)
        );
        assert!(snapshot.dependencies.is_empty());
        assert!(snapshot.directories.is_empty());
        assert!(snapshot.contents.is_empty());
        assert_eq!(snapshot.bytes, MAX_INPUT_BYTES - 1);
    }
}
