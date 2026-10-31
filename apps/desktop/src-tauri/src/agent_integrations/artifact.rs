use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use super::Vendor;
use super::transaction::{self, Result, digest};

pub(super) fn vendor_directory(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::Claude => "claude",
        Vendor::Codex => "codex",
        Vendor::Cursor => "cursor",
        Vendor::OpenCode => "opencode",
        Vendor::Pi => "pi",
    }
}

fn connection_paths() -> impl Iterator<Item = PathBuf> {
    Vendor::ALL
        .into_iter()
        .map(|vendor| PathBuf::from(vendor_directory(vendor)).join("lib/connection.json"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    owner: String,
    schema: u32,
    files: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: String,
    sha256: String,
}

pub struct Artifact {
    pub hash: String,
    pub files: Vec<(PathBuf, Vec<u8>)>,
}

impl Artifact {
    pub fn read(root: &Path) -> Result<Self> {
        let bytes = read_resource(root, Path::new("manifest.json"))?;
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| "artifact_invalid")?;
        if manifest.schema != 1
            || manifest.owner != "antiburn-desktop-agent-integrations-v1"
            || manifest.files.is_empty()
            || manifest.files.len() > 64
        {
            return Err("artifact_invalid");
        }
        let mut files = Vec::new();
        for entry in manifest.files {
            let path = PathBuf::from(entry.path);
            if path
                .components()
                .any(|p| !matches!(p, Component::Normal(_)))
                || files.iter().any(|(name, _)| name == &path)
            {
                return Err("artifact_invalid");
            }
            let contents = read_resource(root, &path)?;
            if digest(&contents) != entry.sha256 {
                return Err("artifact_hash_mismatch");
            }
            files.push((path, contents));
        }
        Ok(Self {
            hash: digest(&bytes),
            files,
        })
    }

    pub fn stage(&self, state: &Path, connection: &serde_json::Value) -> Result<PathBuf> {
        let encoded = serde_json::to_vec(connection).map_err(|_| "invalid_cli_connection")?;
        let identity = digest(&encoded);
        let destination = state.join(format!("{}-{}", &self.hash[..16], &identity[..16]));
        if destination.exists() {
            self.verify_staged(&destination, &encoded)?;
            return Ok(destination);
        }
        if fs::read_dir(state)
            .map_err(|_| "state_unavailable")?
            .take(17)
            .count()
            > 16
        {
            return Err("artifact_retention_limit");
        }
        let pending = state.join(format!(".{}-{}.pending", &self.hash[..16], &identity[..16]));
        transaction::private_directory(&pending)?;
        for (path, bytes) in &self.files {
            let target = pending.join(path);
            ensure_parents(&pending, target.parent().ok_or("artifact_invalid")?)?;
            match transaction::read(&target)? {
                None => transaction::publish(&target, None, bytes)?,
                Some(current) if current == *bytes => (),
                Some(_) => return Err("owned_artifact_changed"),
            }
        }
        for relative in connection_paths() {
            let path = pending.join(relative);
            match transaction::read(&path)? {
                None => transaction::publish(&path, None, &encoded)?,
                Some(current) if current == encoded => (),
                Some(_) => return Err("owned_artifact_changed"),
            }
        }
        self.verify_staged(&pending, &encoded)?;
        fs::rename(&pending, &destination).map_err(|_| "artifact_publish_failed")?;
        Ok(destination)
    }

    fn verify_staged(&self, destination: &Path, connection: &[u8]) -> Result<()> {
        let mut expected = self
            .files
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        expected.extend(connection_paths());
        let mut actual = Vec::new();
        inventory(destination, Path::new(""), &mut actual, &mut 128, 0)?;
        actual.sort();
        expected.sort();
        if actual != expected {
            return Err("owned_artifact_changed");
        }
        for (path, bytes) in &self.files {
            if transaction::read(&destination.join(path))?.as_deref() != Some(bytes) {
                return Err("owned_artifact_changed");
            }
        }
        for path in connection_paths() {
            if transaction::read(&destination.join(path))?.as_deref() != Some(connection) {
                return Err("owned_artifact_changed");
            }
        }
        Ok(())
    }
}

fn inventory(
    root: &Path,
    relative: &Path,
    files: &mut Vec<PathBuf>,
    remaining: &mut usize,
    depth: usize,
) -> Result<()> {
    if depth > 4 {
        return Err("artifact_invalid");
    }
    transaction::checked_directory(&root.join(relative))?;
    for entry in fs::read_dir(root.join(relative)).map_err(|_| "artifact_unreadable")? {
        *remaining = remaining.checked_sub(1).ok_or("artifact_invalid")?;
        let entry = entry.map_err(|_| "artifact_unreadable")?;
        let path = relative.join(entry.file_name());
        let kind = entry.file_type().map_err(|_| "artifact_unreadable")?;
        if kind.is_symlink() {
            return Err("artifact_symlink");
        }
        if kind.is_dir() {
            inventory(root, &path, files, remaining, depth + 1)?;
        } else if kind.is_file() {
            files.push(path);
        } else {
            return Err("artifact_invalid");
        }
        if files.len() > 128 {
            return Err("artifact_invalid");
        }
    }
    Ok(())
}

fn read_resource(root: &Path, relative: &Path) -> Result<Vec<u8>> {
    if !root.is_absolute()
        || relative
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err("artifact_invalid");
    }
    let mut target = root.to_path_buf();
    for component in relative.components() {
        target.push(component);
        if fs::symlink_metadata(&target)
            .map_err(|_| "artifact_missing")?
            .file_type()
            .is_symlink()
        {
            return Err("artifact_symlink");
        }
    }
    let metadata = fs::metadata(&target).map_err(|_| "artifact_missing")?;
    if !metadata.is_file() || metadata.len() > transaction::MAX_BYTES {
        return Err("artifact_invalid");
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut bytes = Vec::new();
    options
        .open(target)
        .map_err(|_| "artifact_missing")?
        .take(transaction::MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "artifact_unreadable")?;
    if bytes.len() as u64 > transaction::MAX_BYTES {
        return Err("artifact_invalid");
    }
    Ok(bytes)
}

fn ensure_parents(root: &Path, parent: &Path) -> Result<()> {
    let relative = parent.strip_prefix(root).map_err(|_| "unsafe_path")?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err("unsafe_path");
        }
        current.push(component);
        transaction::private_directory(&current)?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn staging_reuses_verified_files_and_refuses_extra_content() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let state = root.join("artifacts");
        transaction::private_directory(&state).unwrap();
        let artifact = Artifact {
            hash: digest(b"fixture"),
            files: Vendor::ALL
                .into_iter()
                .map(|vendor| {
                    (
                        PathBuf::from(vendor_directory(vendor)).join("lib/core.mjs"),
                        b"export {}".to_vec(),
                    )
                })
                .collect(),
        };
        let connection = serde_json::json!({"schema": 1});
        let destination = artifact.stage(&state, &connection).unwrap();
        assert_eq!(artifact.stage(&state, &connection).unwrap(), destination);
        let encoded = serde_json::to_vec(&connection).unwrap();
        assert_eq!(connection_paths().count(), Vendor::ALL.len());
        for path in connection_paths() {
            assert_eq!(
                fs::read(destination.join(path))
                    .expect("read the staged vendor connection fixture"),
                encoded
            );
        }
        let pending = state.join(format!(
            ".{}-{}.pending",
            &artifact.hash[..16],
            &digest(&encoded)[..16]
        ));
        fs::rename(&destination, &pending).unwrap();
        assert_eq!(artifact.stage(&state, &connection).unwrap(), destination);
        fs::write(destination.join("extra"), b"unexpected").unwrap();
        assert_eq!(
            artifact.stage(&state, &connection),
            Err("owned_artifact_changed")
        );
    }
}
