//! Project admission. Stored grants describe policy, not operating-system permission.
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectAccessDecision {
    Allowed,
    DeferredProtected(String),
    DeferredUncertain,
}
impl ProjectAccessDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }
}
fn components(path: &Path) -> Option<Vec<String>> {
    if !path.is_absolute() {
        return None;
    }
    let text = path.to_str()?;
    if text.contains('\0')
        || text
            .split(|c| c == '/' || (cfg!(windows) && c == '\\'))
            .any(|part| part == "." || part == "..")
    {
        return None;
    }
    #[cfg(windows)]
    if !windows_project_path_is_supported(path) {
        return None;
    }
    path.components()
        .map(|part| match part {
            Component::ParentDir | Component::CurDir => None,
            _ => part.as_os_str().to_str().map(|s| {
                if cfg!(target_os = "macos") {
                    s.to_lowercase()
                } else {
                    s.to_string()
                }
            }),
        })
        .collect()
}

fn policy_components(path: &Path) -> Option<Vec<String>> {
    let mut parts = components(path)?;
    if cfg!(target_os = "macos")
        && parts
            .get(1)
            .is_some_and(|part| matches!(part.as_str(), "var" | "tmp" | "etc"))
    {
        parts.insert(1, "private".to_string());
    }
    Some(parts)
}

#[cfg(windows)]
fn windows_project_path_is_supported(path: &Path) -> bool {
    use std::path::Prefix;

    let mut parts = path.components();
    let Some(Component::Prefix(prefix)) = parts.next() else {
        return false;
    };
    matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
        && parts.next() == Some(Component::RootDir)
        && parts.all(|part| match part {
            Component::Normal(name) => {
                let Some(name) = name.to_str() else {
                    return false;
                };
                !name.contains([':', '/'])
                    && !windows_device_name(name)
                    && (prefix.kind().is_verbatim() || !name.ends_with([' ', '.']))
            }
            _ => false,
        })
}

#[cfg(windows)]
fn windows_device_name(name: &str) -> bool {
    let basename = name
        .split_once('.')
        .map_or(name, |(base, _)| base)
        .trim_end_matches(' ');
    let upper = basename.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON" | "CONIN$" | "CONOUT$" | "PRN" | "AUX" | "NUL"
    ) || upper
        .strip_prefix("COM")
        .or_else(|| upper.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            let mut digits = suffix.chars();
            matches!(digits.next(), Some('1'..='9' | '¹' | '²' | '³')) && digits.next().is_none()
        })
}
/// Classify a path without filesystem or process access.
pub fn project_access_decision(path: &Path, granted: &HashSet<String>) -> ProjectAccessDecision {
    project_access_decision_in(super::home_dir().as_deref(), path, granted)
}
/// Classify a path with an explicit home directory and no filesystem access.
pub fn project_access_decision_in(
    home: Option<&Path>,
    path: &Path,
    granted: &HashSet<String>,
) -> ProjectAccessDecision {
    use ProjectAccessDecision::*;
    let Some(parts) = policy_components(path) else {
        return DeferredUncertain;
    };
    if !cfg!(target_os = "macos") {
        return Allowed;
    }
    let Some(home) = home.and_then(policy_components) else {
        return DeferredUncertain;
    };
    let possible_protected_child = parts.get(home.len()).is_some_and(|name| {
        !name.is_ascii()
            || super::protected::protected_dir_names()
                .iter()
                .any(|protected| name.eq_ignore_ascii_case(protected))
    });
    if possible_protected_child && !parts.starts_with(&home) && home.len() > 1 {
        let parent = &home[..home.len() - 1];
        if parts.starts_with(parent)
            && (home.last().is_some_and(|name| !name.is_ascii())
                || parts.get(parent.len()).is_some_and(|name| !name.is_ascii()))
        {
            return DeferredUncertain;
        }
    }
    if possible_protected_child
        && !parts.starts_with(&home)
        && parts.len() >= home.len()
        && parts.iter().zip(&home).all(|(candidate, expected)| {
            candidate == expected || !candidate.is_ascii() || !expected.is_ascii()
        })
    {
        return DeferredUncertain;
    }
    // These domains have no grant category in Settings.
    if (parts.get(1).is_some_and(|p| p == "system") && parts.get(2).is_some_and(|p| p == "volumes"))
        || parts.iter().any(|p| {
            p == "cloudstorage"
                || p == "mobile documents"
                || p == "google drive"
                || p.starts_with("googledrive")
                || p == "dropbox"
                || p.starts_with("onedrive")
        })
        || parts
            .get(1)
            .is_some_and(|p| p == "volumes" || p == "network" || p == "net")
    {
        return DeferredUncertain;
    }
    if parts.starts_with(&home)
        && let Some(segment) = parts.get(home.len())
    {
        if !segment.is_ascii() {
            return DeferredUncertain;
        }
        for name in super::protected::protected_dir_names() {
            if segment.eq_ignore_ascii_case(name) && !granted.contains(*name) {
                return DeferredProtected((*name).to_string());
            }
        }
    }
    Allowed
}
/// Reject symlinks before following any project path component.
/// Missing suffixes remain admitted so callers can handle absent files.
pub fn project_path_access(path: &Path, granted: &HashSet<String>) -> ProjectAccessDecision {
    project_path_access_in(super::home_dir().as_deref(), path, granted)
}

pub fn project_path_access_in(
    home: Option<&Path>,
    path: &Path,
    granted: &HashSet<String>,
) -> ProjectAccessDecision {
    path_access_with(home, path, granted, &mut |p| {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_ATTRIBUTE_REPARSE_POINT, GetDriveTypeW,
            };
            use windows_sys::Win32::System::WindowsProgramming::{
                DRIVE_NO_ROOT_DIR, DRIVE_REMOTE, DRIVE_UNKNOWN,
            };

            if p.parent().is_none()
                && let Some(Component::Prefix(prefix)) = p.components().next()
                && let std::path::Prefix::Disk(drive) | std::path::Prefix::VerbatimDisk(drive) =
                    prefix.kind()
            {
                let root = [u16::from(drive), b':' as u16, b'\\' as u16, 0];
                // SAFETY: The drive root is a null-terminated UTF-16 string.
                if matches!(
                    unsafe { GetDriveTypeW(root.as_ptr()) },
                    DRIVE_NO_ROOT_DIR | DRIVE_REMOTE | DRIVE_UNKNOWN
                ) {
                    return Ok(true);
                }
            }
            std::fs::symlink_metadata(p)
                .map(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
        }
        #[cfg(not(windows))]
        std::fs::symlink_metadata(p).map(|m| m.file_type().is_symlink())
    })
}
fn path_access_with(
    home: Option<&Path>,
    path: &Path,
    granted: &HashSet<String>,
    metadata: &mut impl FnMut(&Path) -> std::io::Result<bool>,
) -> ProjectAccessDecision {
    let decision = project_access_decision_in(home, path, granted);
    if !decision.is_allowed() {
        return decision;
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component.as_os_str());
        // Windows can report a verbatim drive prefix as absolute.
        if matches!(component, Component::Prefix(_)) || !prefix.is_absolute() {
            continue;
        }
        let decision = project_access_decision_in(home, &prefix, granted);
        if !decision.is_allowed() {
            return decision;
        }
        match metadata(&prefix) {
            Ok(false) => {}
            // macOS owns these fixed aliases. Reject other aliases.
            Ok(true)
                if cfg!(target_os = "macos")
                    && matches!(prefix.to_str(), Some("/var" | "/tmp" | "/etc")) =>
            {
                let Ok(suffix) = prefix.strip_prefix("/") else {
                    return ProjectAccessDecision::DeferredUncertain;
                };
                let expected = Path::new("/private").join(suffix);
                if std::fs::read_link(&prefix)
                    .ok()
                    .map(|target| {
                        if target.is_absolute() {
                            target
                        } else {
                            Path::new("/").join(target)
                        }
                    })
                    .as_deref()
                    != Some(expected.as_path())
                {
                    return ProjectAccessDecision::DeferredUncertain;
                }
                let decision = project_access_decision_in(home, &expected, granted);
                if !decision.is_allowed() {
                    return decision;
                }
                prefix = expected;
            }
            Ok(true) => return ProjectAccessDecision::DeferredUncertain,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && prefix.parent().is_some() => {
                return ProjectAccessDecision::Allowed;
            }
            Err(_) => return ProjectAccessDecision::DeferredUncertain,
        }
    }
    ProjectAccessDecision::Allowed
}
fn resolve_git_reference(base: &Path, value: &str, granted: &HashSet<String>) -> Option<PathBuf> {
    let raw = Path::new(value);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        base.join(raw)
    };
    let mut normalized = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::ParentDir => {
                if !project_path_access(&normalized, granted).is_allowed() {
                    return None;
                }
                if !normalized.pop() {
                    return None;
                }
            }
            Component::CurDir => {}
            part => normalized.push(part.as_os_str()),
        }
    }
    Some(normalized)
}
fn bounded_text(path: &Path) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut text = String::new();
    file.take(65_537).read_to_string(&mut text).ok()?;
    (text.len() <= 65_536).then_some(text)
}

fn git_config_has_external_path(text: &str) -> bool {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut core_section = false;
    let is_worktree_key = |line: &str| {
        line.trim_start()
            .split(|c: char| c == '=' || c.is_ascii_whitespace())
            .next()
            .is_some_and(|key| key.eq_ignore_ascii_case("worktree"))
    };
    for line in text.lines().map(str::trim_start) {
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if line
            .as_bytes()
            .iter()
            .rev()
            .take_while(|byte| **byte == b'\\')
            .count()
            % 2
            == 1
        {
            return true;
        }
        if let Some(section) = line.strip_prefix('[') {
            let Some((section, remainder)) = section.split_once(']') else {
                return true;
            };
            let section = section.trim();
            let section_name = section
                .split(|c: char| c == '.' || c.is_ascii_whitespace())
                .next()
                .unwrap_or(section);
            if section_name.eq_ignore_ascii_case("include")
                || section_name.eq_ignore_ascii_case("includeif")
            {
                return true;
            }
            core_section = section.eq_ignore_ascii_case("core");
            if core_section && is_worktree_key(remainder) {
                return true;
            }
            continue;
        }
        if core_section && is_worktree_key(line) {
            return true;
        }
    }
    false
}

/// Admit Git's directory references before a project command starts.
pub fn git_project_access(path: &Path, granted: &HashSet<String>) -> ProjectAccessDecision {
    let decision = project_path_access(path, granted);
    if !decision.is_allowed() {
        return decision;
    }
    for ancestor in path.ancestors() {
        let marker = ancestor.join(".git");
        let decision = project_path_access(&marker, granted);
        if !decision.is_allowed() {
            return decision;
        }
        match std::fs::symlink_metadata(&marker) {
            Ok(metadata) => {
                let git_dir = if metadata.is_dir() {
                    marker
                } else if metadata.is_file() {
                    let Some(text) = bounded_text(&marker) else {
                        return ProjectAccessDecision::DeferredUncertain;
                    };
                    let Some(target) = text
                        .trim()
                        .strip_prefix("gitdir: ")
                        .and_then(|v| resolve_git_reference(ancestor, v, granted))
                    else {
                        return ProjectAccessDecision::DeferredUncertain;
                    };
                    target
                } else {
                    return ProjectAccessDecision::DeferredUncertain;
                };
                return git_directory_access(&git_dir, granted);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return ProjectAccessDecision::DeferredUncertain,
        }
        let head = ancestor.join("HEAD");
        let decision = project_path_access(&head, granted);
        if !decision.is_allowed() {
            return decision;
        }
        if head.is_file() {
            let objects = ancestor.join("objects");
            let refs = ancestor.join("refs");
            for candidate in [&objects, &refs] {
                let decision = project_path_access(candidate, granted);
                if !decision.is_allowed() {
                    return decision;
                }
            }
            if objects.is_dir() && refs.is_dir() {
                return git_directory_access(ancestor, granted);
            }
        }
    }
    ProjectAccessDecision::Allowed
}
fn git_directory_access(git_dir: &Path, granted: &HashSet<String>) -> ProjectAccessDecision {
    let decision = project_path_access(git_dir, granted);
    if !decision.is_allowed() {
        return decision;
    }
    let common_file = git_dir.join("commondir");
    let decision = project_path_access(&common_file, granted);
    if !decision.is_allowed() {
        return decision;
    }
    let common = if common_file.is_file() {
        let Some(common) = bounded_text(&common_file)
            .and_then(|v| resolve_git_reference(git_dir, v.trim(), granted))
        else {
            return ProjectAccessDecision::DeferredUncertain;
        };
        common
    } else {
        git_dir.to_path_buf()
    };
    for dir in [git_dir, common.as_path()] {
        for name in [
            "config",
            "config.worktree",
            "HEAD",
            "objects",
            "refs",
            "worktrees",
            "packed-refs",
            "objects/info/alternates",
        ] {
            let candidate = dir.join(name);
            let decision = project_path_access(&candidate, granted);
            if !decision.is_allowed() {
                return decision;
            }
            if name == "objects/info/alternates" && candidate.is_file() {
                return ProjectAccessDecision::DeferredUncertain;
            }
            if matches!(name, "refs" | "worktrees") {
                let decision = git_metadata_tree_access(&candidate, granted, name == "worktrees");
                if !decision.is_allowed() {
                    return decision;
                }
            }
            if name.starts_with("config") && candidate.is_file() {
                let Some(text) = bounded_text(&candidate) else {
                    return ProjectAccessDecision::DeferredUncertain;
                };
                if git_config_has_external_path(&text) {
                    return ProjectAccessDecision::DeferredUncertain;
                }
            }
        }
    }
    ProjectAccessDecision::Allowed
}

// Inspect at most this many entries in each refs or worktrees tree.
// Defer access when entries remain unchecked.
const MAX_GIT_METADATA_TREE_ENTRIES: usize = 4096;

fn git_metadata_tree_access(
    root: &Path,
    granted: &HashSet<String>,
    worktrees: bool,
) -> ProjectAccessDecision {
    let mut stack = vec![root.to_path_buf()];
    let mut remaining = MAX_GIT_METADATA_TREE_ENTRIES;
    while let Some(dir) = stack.pop() {
        let decision = project_path_access(&dir, granted);
        if !decision.is_allowed() {
            return decision;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return ProjectAccessDecision::DeferredUncertain,
        };
        for entry in entries {
            if remaining == 0 {
                return ProjectAccessDecision::DeferredUncertain;
            }
            remaining -= 1;
            let Ok(entry) = entry else {
                return ProjectAccessDecision::DeferredUncertain;
            };
            let path = entry.path();
            let decision = project_path_access(&path, granted);
            if !decision.is_allowed() {
                return decision;
            }
            let Ok(kind) = entry.file_type() else {
                return ProjectAccessDecision::DeferredUncertain;
            };
            if worktrees && entry.file_name() == "gitdir" {
                if !kind.is_file() {
                    return ProjectAccessDecision::DeferredUncertain;
                }
                let Some(target) = bounded_text(&path).map(|text| PathBuf::from(text.trim()))
                else {
                    return ProjectAccessDecision::DeferredUncertain;
                };
                let decision = project_path_access(&target, granted);
                if !decision.is_allowed() {
                    return decision;
                }
            } else if kind.is_dir() {
                stack.push(path);
            }
        }
    }
    ProjectAccessDecision::Allowed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_paths_never_reach_filesystem() {
        for path in ["relative/repo", "/safe/../Documents/repo", "/safe/./repo"] {
            let mut calls = 0;
            let decision = path_access_with(
                Some(Path::new("/Users/synthetic")),
                Path::new(path),
                &HashSet::new(),
                &mut |_| {
                    calls += 1;
                    Ok(false)
                },
            );
            assert_eq!(decision, ProjectAccessDecision::DeferredUncertain);
            assert_eq!(calls, 0);
        }
    }

    #[test]
    #[cfg(windows)]
    fn windows_drive_roots_are_probed_with_the_root_separator() {
        for (root, project) in [
            (r"C:\", r"C:\synthetic\repo"),
            (r"\\?\C:\", r"\\?\C:\synthetic\repo"),
        ] {
            let mut visited = Vec::new();
            let decision = path_access_with(None, Path::new(project), &HashSet::new(), &mut |p| {
                visited.push(p.to_path_buf());
                Ok(false)
            });
            assert_eq!(decision, ProjectAccessDecision::Allowed);
            assert_eq!(
                visited,
                [
                    PathBuf::from(root),
                    Path::new(root).join("synthetic"),
                    PathBuf::from(project),
                ]
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn unsupported_windows_paths_never_reach_filesystem() {
        for path in [
            r"C:repo",
            r"\\?\C:",
            r"\repo",
            r"C:\synthetic\.\repo",
            r"C:\synthetic\..\repo",
            r"C:\synthetic.\repo",
            r"C:\synthetic\repo:stream",
            r"C:\synthetic\nul.txt",
            r"C:\synthetic\COM1",
            r"C:\synthetic\Lpt².config",
            r"C:\synthetic\COM³ .config",
            r"\\?\C:\synthetic\CONOUT$",
            r"\\?\C:\synthetic/repo",
            r"\\server\share\repo",
            r"\\?\UNC\server\share\repo",
            r"\\.\C:\synthetic\repo",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\repo",
        ] {
            let decision = path_access_with(None, Path::new(path), &HashSet::new(), &mut |_| {
                panic!("unexpected filesystem probe for {path}")
            });
            assert_eq!(decision, ProjectAccessDecision::DeferredUncertain, "{path}");
        }
    }

    #[test]
    #[cfg(windows)]
    fn a_missing_drive_root_defers_but_a_missing_suffix_is_admitted() {
        let root = Path::new(r"C:\");
        let path = root.join("synthetic").join("repo");
        for (missing, expected) in [
            (root, ProjectAccessDecision::DeferredUncertain),
            (&path, ProjectAccessDecision::Allowed),
        ] {
            let mut visited = Vec::new();
            let decision = path_access_with(None, &path, &HashSet::new(), &mut |p| {
                visited.push(p.to_path_buf());
                if p == missing {
                    return Err(std::io::ErrorKind::NotFound.into());
                }
                Ok(false)
            });
            assert_eq!(decision, expected);
            assert_eq!(visited.last().unwrap(), missing);
        }
    }

    #[test]
    #[cfg(windows)]
    fn canonical_windows_paths_remain_admitted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let path = root.join("config.json");
        std::fs::write(&path, "{}").unwrap();
        assert!(project_path_access(&path, &HashSet::new()).is_allowed());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn protected_and_unsupported_paths_never_reach_filesystem() {
        let home = Path::new("/Users/synthetic");
        for path in [
            "/Users/synthetic/Documents/repo",
            "/users/SYNTHETIC/dOcUmEnTs/repo",
            "/Users/synthetic/Library/CloudStorage/provider/repo",
            "/Users/synthetic/Library/Mobile Documents/repo",
            "/Users/synthetic/Google Drive/repo",
            "/Volumes/external/repo",
        ] {
            let mut calls = 0;
            let decision =
                path_access_with(Some(home), Path::new(path), &HashSet::new(), &mut |_| {
                    calls += 1;
                    Ok(false)
                });
            assert!(!decision.is_allowed(), "{path}");
            assert_eq!(calls, 0);
        }
        let mut calls = 0;
        assert_eq!(
            path_access_with(None, Path::new("/safe/repo"), &HashSet::new(), &mut |_| {
                calls += 1;
                Ok(false)
            }),
            ProjectAccessDecision::DeferredUncertain
        );
        assert_eq!(calls, 0);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn grant_does_not_authorize_a_different_child_domain() {
        let home = Path::new("/Users/synthetic");
        let granted = HashSet::from(["Documents".to_owned()]);
        assert!(
            project_access_decision_in(Some(home), &home.join("Documents/repo"), &granted)
                .is_allowed()
        );
        assert!(
            !project_access_decision_in(
                Some(home),
                &home.join("Documents/Google Drive/repo"),
                &granted
            )
            .is_allowed()
        );
        assert!(
            !project_access_decision_in(Some(home), &home.join("Desktop/repo"), &granted)
                .is_allowed()
        );
        assert!(
            project_access_decision_in(
                Some(home),
                &home.join("Documents-other/repo"),
                &HashSet::new()
            )
            .is_allowed()
        );
    }

    #[test]
    fn symlink_boundary_stops_before_target_or_descendant() {
        let mut visited = Vec::new();
        let root = Path::new(if cfg!(windows) { r"C:\" } else { "/" });
        let alias = root.join("synthetic").join("alias");
        let path = alias.join("secret").join("repo");
        let decision = path_access_with(
            Some(Path::new("/home/synthetic")),
            &path,
            &HashSet::new(),
            &mut |p| {
                visited.push(p.to_path_buf());
                Ok(p == alias)
            },
        );
        assert_eq!(decision, ProjectAccessDecision::DeferredUncertain);
        assert_eq!(visited.last().unwrap(), &alias);
    }

    #[test]
    fn accessible_and_missing_paths_remain_admitted() {
        let dir = tempfile::tempdir().unwrap();
        assert!(project_path_access(dir.path(), &HashSet::new()).is_allowed());
        assert!(
            project_path_access(&dir.path().join("missing/checks"), &HashSet::new()).is_allowed()
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn git_indirections_defer_without_touching_protected_targets() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(".git");
        std::fs::write(
            &marker,
            "gitdir: /Volumes/synthetic-never-probed/repo/.git\n",
        )
        .unwrap();
        assert_eq!(
            git_project_access(dir.path(), &HashSet::new()),
            ProjectAccessDecision::DeferredUncertain
        );
        std::fs::remove_file(&marker).unwrap();
        std::fs::create_dir(&marker).unwrap();
        std::fs::write(
            marker.join("commondir"),
            "/Volumes/synthetic-never-probed/repo/.git\n",
        )
        .unwrap();
        assert_eq!(
            git_project_access(dir.path(), &HashSet::new()),
            ProjectAccessDecision::DeferredUncertain
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn data_volume_alias_is_deferred_without_metadata() {
        let mut calls = 0;
        let decision = path_access_with(
            Some(Path::new("/Users/synthetic")),
            Path::new("/System/Volumes/Data/Users/synthetic/Documents/repo"),
            &HashSet::new(),
            &mut |_| {
                calls += 1;
                Ok(false)
            },
        );
        assert_eq!(decision, ProjectAccessDecision::DeferredUncertain);
        assert_eq!(calls, 0);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn system_alias_spellings_cannot_bypass_a_protected_home_child() {
        for (home, path) in [
            ("/private/var/synthetic", "/var/synthetic/Documents/repo"),
            ("/var/synthetic", "/private/var/synthetic/Documents/repo"),
        ] {
            let mut calls = 0;
            let decision = path_access_with(
                Some(Path::new(home)),
                Path::new(path),
                &HashSet::new(),
                &mut |_| {
                    calls += 1;
                    Ok(false)
                },
            );
            assert_eq!(
                decision,
                ProjectAccessDecision::DeferredProtected("Documents".to_string())
            );
            assert_eq!(calls, 0);
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn unprotected_system_alias_still_requires_physical_verification() {
        let mut visited = Vec::new();
        let decision = path_access_with(
            Some(Path::new("/private/var/synthetic")),
            Path::new("/var/synthetic/repo"),
            &HashSet::new(),
            &mut |path| {
                visited.push(path.to_path_buf());
                Ok(path == Path::new("/var"))
            },
        );
        assert_eq!(decision, ProjectAccessDecision::Allowed);
        assert!(visited.contains(&PathBuf::from("/var")));
        assert!(visited.contains(&PathBuf::from("/private/var/synthetic")));
    }

    #[test]
    fn bare_git_config_indirections_are_deferred() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("objects")).unwrap();
        std::fs::create_dir(dir.path().join("refs")).unwrap();
        std::fs::write(dir.path().join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            dir.path().join("config"),
            "\u{feff}[include]\npath = /never-probed\n",
        )
        .unwrap();
        assert_eq!(
            git_project_access(dir.path(), &HashSet::new()),
            ProjectAccessDecision::DeferredUncertain
        );
        let child = dir.path().join("child");
        std::fs::create_dir(&child).unwrap();
        assert_eq!(
            git_project_access(&child, &HashSet::new()),
            ProjectAccessDecision::DeferredUncertain
        );
        std::fs::write(
            dir.path().join("config"),
            "[extensions]\nworktreeConfig = true\n",
        )
        .unwrap();
        assert!(git_project_access(&child, &HashSet::new()).is_allowed());
        std::fs::write(
            dir.path().join("config"),
            "[core]\nname = \"first\\\n[other]\\\nlast\"\nworktree = /never-probed\n",
        )
        .unwrap();
        assert_eq!(
            git_project_access(&child, &HashSet::new()),
            ProjectAccessDecision::DeferredUncertain
        );
        std::fs::write(
            dir.path().join("config"),
            "[core] worktree = /never-probed\n",
        )
        .unwrap();
        assert_eq!(
            git_project_access(&child, &HashSet::new()),
            ProjectAccessDecision::DeferredUncertain
        );
    }

    #[test]
    fn git_metadata_tree_defers_when_the_entry_limit_is_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let refs = dir.path().join("refs");
        std::fs::create_dir(&refs).unwrap();
        for index in 0..MAX_GIT_METADATA_TREE_ENTRIES {
            std::fs::File::create(refs.join(format!("branch-{index}"))).unwrap();
        }
        let grants = HashSet::new();
        for worktrees in [false, true] {
            assert_eq!(
                git_metadata_tree_access(&refs, &grants, worktrees),
                ProjectAccessDecision::Allowed
            );
        }

        std::fs::File::create(refs.join("extra-branch")).unwrap();
        for worktrees in [false, true] {
            assert_eq!(
                git_metadata_tree_access(&refs, &grants, worktrees),
                ProjectAccessDecision::DeferredUncertain
            );
        }
    }

    #[test]
    fn worktree_gitdir_entry_must_be_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let worktrees = dir.path().join("worktrees");
        std::fs::create_dir_all(worktrees.join("synthetic/gitdir")).unwrap();
        assert_eq!(
            git_metadata_tree_access(&worktrees, &HashSet::new(), true),
            ProjectAccessDecision::DeferredUncertain
        );
    }

    #[test]
    fn git_safe_relative_common_directory_remains_admitted() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let linked = dir.path().join("linked");
        std::fs::create_dir_all(repo.join(".git/worktrees/linked")).unwrap();
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(
            linked.join(".git"),
            "gitdir: ../repo/.git/worktrees/linked\n",
        )
        .unwrap();
        std::fs::write(repo.join(".git/worktrees/linked/commondir"), "../..\n").unwrap();
        assert!(git_project_access(&linked, &HashSet::new()).is_allowed());
        std::fs::write(
            repo.join(".git/config"),
            "[include]\npath = /never-probed\n",
        )
        .unwrap();
        assert_eq!(
            git_project_access(&linked, &HashSet::new()),
            ProjectAccessDecision::DeferredUncertain
        );
    }
}
