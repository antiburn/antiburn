use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
};

use antiburn_local::paths::protected;

#[derive(Debug, Clone, Copy)]
pub(super) struct PathAdmission<'a> {
    home: &'a Path,
    granted_dirs: &'a HashSet<String>,
}

impl<'a> PathAdmission<'a> {
    pub fn new(home: &'a Path, granted_dirs: &'a HashSet<String>) -> Self {
        Self { home, granted_dirs }
    }

    pub fn check(self, path: &Path) -> Result<(), &'static str> {
        if !self.home.is_absolute() || has_parent_component(self.home) {
            return Err("home_unavailable");
        }
        #[cfg(windows)]
        if !windows_path_is_supported(self.home) {
            return Err("home_unavailable");
        }
        if !path.is_absolute() || has_parent_component(path) {
            return Err("path_outside_allowed_root");
        }
        #[cfg(windows)]
        if !windows_path_is_supported(path) {
            return Err("path_outside_allowed_root");
        }
        #[cfg(target_os = "macos")]
        let blocked = protected::protected_dir_names().iter().any(|name| {
            !self.granted_dirs.contains(*name)
                && may_alias_protected_prefix(path, &self.home.join(name))
        });
        #[cfg(not(target_os = "macos"))]
        let blocked = protected::cwd_resolution_blocked_in(self.home, path, self.granted_dirs);
        if blocked {
            return Err("protected_path_not_granted");
        }
        Ok(())
    }

    pub fn canonical_directory(self, path: &Path) -> Result<PathBuf, &'static str> {
        self.check(path)?;
        let mut current = PathBuf::new();
        let mut components = path.components().peekable();
        while let Some(component) = components.next() {
            current.push(component);
            // A verbatim drive prefix needs its root separator before lookup.
            if !current.is_absolute() || components.peek() == Some(&Component::RootDir) {
                continue;
            }
            self.check(&current)?;
            let metadata = fs::symlink_metadata(&current).map_err(|_| "directory_unavailable")?;
            if metadata.file_type().is_symlink() {
                return Err("symlink_not_read");
            }
            if !metadata.is_dir() {
                return Err("directory_unavailable");
            }
        }
        let canonical = path.canonicalize().map_err(|_| "directory_unavailable")?;
        self.check(&canonical)?;
        Ok(canonical)
    }

    pub fn configured_root(
        self,
        path: &Path,
        root: &Path,
    ) -> Result<Option<PathBuf>, &'static str> {
        self.check(path)?;
        self.check(root)?;
        if path.starts_with(root) {
            return Ok(Some(root.to_owned()));
        }
        #[cfg(windows)]
        // Normal and verbatim paths can resolve names differently. Check the root.
        if let Some(alias) = windows_root_alias(path, root)
            && self.canonical_directory(&alias)? == root
        {
            return Ok(Some(alias));
        }
        Ok(None)
    }
}

pub(super) fn has_parent_component(path: &Path) -> bool {
    path.as_os_str()
        .as_encoded_bytes()
        .split(|byte| *byte == b'/' || (cfg!(windows) && *byte == b'\\'))
        .any(|part| part == b"..")
}

#[cfg(windows)]
fn windows_path_is_supported(path: &Path) -> bool {
    use std::path::Prefix;

    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return false;
    };
    match prefix.kind() {
        Prefix::VerbatimDisk(_) | Prefix::VerbatimUNC(_, _) => true,
        Prefix::UNC(server, _)
            if server == std::ffi::OsStr::new("?") || server == std::ffi::OsStr::new(".") =>
        {
            false
        }
        // Windows can trim a final name but retain the same name inside a path.
        Prefix::Disk(_) | Prefix::UNC(_, _) => components.all(|component| match component {
            Component::Normal(name) => {
                let bytes = name.as_encoded_bytes();
                !bytes.ends_with(b" ") && !bytes.ends_with(b".")
            }
            _ => true,
        }),
        _ => false,
    }
}

#[cfg(windows)]
fn windows_root_alias(path: &Path, root: &Path) -> Option<PathBuf> {
    use std::path::Prefix;

    let mut actual = path.components();
    let mut expected = root.components();
    let (Some(Component::Prefix(prefix)), Some(Component::Prefix(root_prefix))) =
        (actual.next(), expected.next())
    else {
        return None;
    };
    let same_prefix = match (prefix.kind(), root_prefix.kind()) {
        (Prefix::Disk(a), Prefix::VerbatimDisk(b)) | (Prefix::VerbatimDisk(a), Prefix::Disk(b)) => {
            a == b
        }
        (Prefix::UNC(a, b), Prefix::VerbatimUNC(c, d))
        | (Prefix::VerbatimUNC(a, b), Prefix::UNC(c, d)) => a == c && b == d,
        _ => false,
    };
    if !same_prefix {
        return None;
    }
    let mut alias = PathBuf::from(prefix.as_os_str());
    for expected in expected {
        let component = actual.next()?;
        if component != expected {
            return None;
        }
        alias.push(component);
    }
    Some(alias)
}

#[cfg(target_os = "macos")]
fn may_alias_protected_prefix(path: &Path, prefix: &Path) -> bool {
    // Do not probe the volume for its case or Unicode rules. Match ASCII case
    // on all macOS volumes and treat non-ASCII components as possible aliases.
    let mut actual = path.components();
    prefix.components().all(|expected| {
        let Some(actual) = actual.next() else {
            return false;
        };
        match (actual, expected) {
            (Component::Normal(actual), Component::Normal(expected)) => {
                let actual = actual.as_encoded_bytes();
                let expected = expected.as_encoded_bytes();
                !actual.is_ascii() || !expected.is_ascii() || actual.eq_ignore_ascii_case(expected)
            }
            _ => actual == expected,
        }
    })
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn windows_root_aliases_keep_drives_shares_and_component_boundaries() {
        for (path, root, alias) in [
            (r"C:\safe\rules.md", r"\\?\C:\safe", r"C:\safe"),
            (r"\\?\C:\safe\rules.md", r"C:\safe", r"\\?\C:\safe"),
            (
                r"\\server\share\safe\rules.md",
                r"\\?\UNC\server\share\safe",
                r"\\server\share\safe",
            ),
            (
                r"\\?\UNC\server\share\safe\rules.md",
                r"\\server\share\safe",
                r"\\?\UNC\server\share\safe",
            ),
        ] {
            assert_eq!(
                windows_root_alias(Path::new(path), Path::new(root)),
                Some(PathBuf::from(alias))
            );
        }
        for (path, root) in [
            (r"C:\safe-copy\rules.md", r"\\?\C:\safe"),
            (r"D:\safe\rules.md", r"\\?\C:\safe"),
            (r"C:safe\rules.md", r"\\?\C:\safe"),
            (r"\\.\C:\safe\rules.md", r"\\?\C:\safe"),
            (r"C:\SAFE\rules.md", r"\\?\C:\safe"),
            (
                r"\\server\other\safe\rules.md",
                r"\\?\UNC\server\share\safe",
            ),
        ] {
            assert_eq!(windows_root_alias(Path::new(path), Path::new(root)), None);
        }
    }
}
