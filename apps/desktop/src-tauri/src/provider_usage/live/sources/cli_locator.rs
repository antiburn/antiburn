//! Find a coding CLI when the app runs with a thin `PATH`.
//!
//! macOS gives an app that starts from Finder, the Dock, or a login item only
//! `/usr/bin:/bin:/usr/sbin:/sbin`. It does not read the reader's shell
//! profile. The native Claude installer (`~/.local/bin`), Homebrew, npm,
//! volta, bun, and nvm all put their binaries outside that `PATH`. A lookup
//! through the process `PATH` alone therefore reports "not installed" for a
//! CLI that the reader runs every day. A development build that starts from a
//! terminal inherits the full shell `PATH` and does not show the problem.
//!
//! This module searches the process `PATH` first, then the usual install
//! locations. It only reads file metadata. It never runs a binary.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

/// The home-relative directories that install tools put on `PATH` through a
/// shell profile.
const HOME_BIN_DIRS: [&str; 4] = [".volta/bin", ".bun/bin", ".npm-global/bin", ".local/bin"];

/// The fixed prefixes that Homebrew (Apple silicon and Intel) and manual
/// installs use.
const FIXED_BIN_DIRS: [&str; 2] = ["/usr/local/bin", "/opt/homebrew/bin"];

/// A limit on how many nvm version directories to search. A reader with more
/// node versions gets the newest ones. The limit prevents a very large
/// directory from slowing every search.
const MAX_NVM_VERSIONS: usize = 16;

/// The home-relative directories that only one CLI installs into. The
/// legacy Claude Code "local" install keeps its launcher in `~/.claude/local`.
fn own_home_dirs(binary: &str) -> &'static [&'static str] {
    match binary {
        "claude" => &[".claude/local"],
        _ => &[],
    }
}

/// Where `binary` is installed, searched through [`search_dirs`] and the
/// CLI's own install directories.
pub(super) fn locate(binary: &str) -> Option<(PathBuf, Vec<PathBuf>)> {
    let dirs = search_dirs(own_home_dirs(binary));
    locate_in(binary, &dirs).map(|path| (path, dirs))
}

/// Every directory to search, in order: the process `PATH`, the
/// home-relative install directories, `extra_home_dirs`, the nvm
/// per-version `bin` directories (newest first), and the fixed prefixes.
///
/// nvm has no shim. Each node version is its own prefix, so its `bin`
/// directories are listed directly.
pub(super) fn search_dirs(extra_home_dirs: &[&str]) -> Vec<PathBuf> {
    search_dirs_in(
        std::env::var_os("PATH"),
        antiburn_local::paths::home_dir().as_deref(),
        extra_home_dirs,
    )
}

fn search_dirs_in(
    path: Option<OsString>,
    home: Option<&Path>,
    extra_home_dirs: &[&str],
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = path
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = home {
        for dir in HOME_BIN_DIRS.iter().chain(extra_home_dirs) {
            dirs.push(home.join(dir));
        }
        dirs.extend(nvm_bin_dirs(&home.join(".nvm/versions/node")));
    }
    dirs.extend(FIXED_BIN_DIRS.iter().map(PathBuf::from));
    dirs.retain(|dir| !dir.as_os_str().is_empty());
    dirs
}

/// The first executable file named `binary` in `dirs`. On Windows, the usual
/// executable extensions are also tried.
pub(super) fn locate_in(binary: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().find_map(|dir| {
        #[cfg(target_os = "windows")]
        for extension in ["exe", "cmd", "bat", "ps1"] {
            let candidate = dir.join(format!("{binary}.{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        let candidate = dir.join(binary);
        candidate.is_file().then_some(candidate)
    })
}

/// A `PATH` for a child process that runs `binary`. The binary's own
/// directory comes first, then every searched directory. An npm-installed
/// CLI starts with `#!/usr/bin/env node`, so the child must be able to find
/// `node` too. The node binary is normally in the same directory (nvm,
/// volta, Homebrew) or in one of the searched directories.
pub(super) fn child_path(binary: &Path, dirs: &[PathBuf]) -> Option<OsString> {
    let own_dir = binary.parent().map(Path::to_path_buf);
    let mut ordered: Vec<PathBuf> = own_dir.into_iter().collect();
    for dir in dirs {
        if !ordered.contains(dir) {
            ordered.push(dir.clone());
        }
    }
    std::env::join_paths(ordered).ok()
}

/// The per-version `bin` directories under an nvm-style root, newest version
/// first. The sort is numeric, because a text sort puts `v9` after `v20`.
pub(super) fn nvm_bin_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut versions: Vec<(Vec<u64>, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let key = version_key(&name.to_string_lossy())?;
            Some((key, entry.path().join("bin")))
        })
        .collect();
    versions.sort_by(|left, right| right.0.cmp(&left.0));
    versions.truncate(MAX_NVM_VERSIONS);
    versions.into_iter().map(|(_, dir)| dir).collect()
}

/// The numeric sort key of a `vMAJOR.MINOR.PATCH` directory name, or `None`
/// for other names, such as nvm's `node` alias symlink.
fn version_key(name: &str) -> Option<Vec<u64>> {
    let digits = name.strip_prefix('v')?;
    digits
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_executable(path: &Path) {
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, b"#!/bin/sh\n").expect("write");
    }

    #[test]
    fn a_thin_gui_path_still_finds_a_native_install_under_home() {
        // The PATH that macOS gives an app started from Finder.
        let home = tempfile::tempdir().expect("tempdir");
        touch_executable(&home.path().join(".local/bin/claude"));

        let dirs = search_dirs_in(
            Some(OsString::from("/usr/bin:/bin:/usr/sbin:/sbin")),
            Some(home.path()),
            &[],
        );

        assert_eq!(
            locate_in("claude", &dirs),
            Some(home.path().join(".local/bin/claude"))
        );
    }

    #[test]
    fn the_process_path_wins_over_the_fallback_directories() {
        let home = tempfile::tempdir().expect("tempdir");
        let custom = tempfile::tempdir().expect("tempdir");
        touch_executable(&home.path().join(".local/bin/claude"));
        touch_executable(&custom.path().join("claude"));

        let dirs = search_dirs_in(
            Some(custom.path().as_os_str().to_owned()),
            Some(home.path()),
            &[],
        );

        assert_eq!(
            locate_in("claude", &dirs),
            Some(custom.path().join("claude"))
        );
    }

    #[test]
    fn extra_home_directories_are_searched() {
        let home = tempfile::tempdir().expect("tempdir");
        touch_executable(&home.path().join(".claude/local/claude"));

        let dirs = search_dirs_in(None, Some(home.path()), &[".claude/local"]);

        assert_eq!(
            locate_in("claude", &dirs),
            Some(home.path().join(".claude/local/claude"))
        );
    }

    #[test]
    fn nvm_versions_are_searched_newest_first() {
        let home = tempfile::tempdir().expect("tempdir");
        let nvm = home.path().join(".nvm/versions/node");
        touch_executable(&nvm.join("v18.20.3/bin/claude"));
        touch_executable(&nvm.join("v20.11.1/bin/claude"));

        let dirs = search_dirs_in(None, Some(home.path()), &[]);

        assert_eq!(
            locate_in("claude", &dirs),
            Some(nvm.join("v20.11.1/bin/claude"))
        );
    }

    #[test]
    fn a_directory_with_the_binary_name_is_not_a_binary() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("claude")).expect("mkdir");

        assert_eq!(locate_in("claude", &[dir.path().to_path_buf()]), None);
    }

    #[test]
    fn an_absent_binary_is_not_found() {
        let dirs = search_dirs(&[]);
        assert_eq!(locate_in("antiburn-nonexistent-touch-binary", &dirs), None);
    }

    #[test]
    fn the_child_path_starts_with_the_binary_directory_without_duplicates() {
        let bin = PathBuf::from("/opt/tool/bin/claude");
        let dirs = vec![
            PathBuf::from("/usr/bin"),
            PathBuf::from("/opt/tool/bin"),
            PathBuf::from("/bin"),
        ];

        let path = child_path(&bin, &dirs).expect("joined");

        let parts: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(
            parts,
            [
                PathBuf::from("/opt/tool/bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
            ]
        );
    }

    #[test]
    fn nvm_version_directories_sort_numerically_newest_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        for version in ["v9.1.0", "v20.11.1", "v18.20.3"] {
            fs::create_dir_all(dir.path().join(version).join("bin")).expect("mkdir");
        }
        // nvm keeps other entries next to the versions. They are skipped.
        fs::create_dir_all(dir.path().join("alias")).expect("mkdir");

        let names: Vec<String> = nvm_bin_dirs(dir.path())
            .iter()
            .map(|bin| {
                bin.parent()
                    .and_then(Path::file_name)
                    .expect("version dir")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();

        assert_eq!(names, ["v20.11.1", "v18.20.3", "v9.1.0"]);
    }

    #[test]
    fn a_missing_nvm_root_contributes_no_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(nvm_bin_dirs(&dir.path().join("not-there")).is_empty());
    }
}
