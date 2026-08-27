//! Stable identity for a local repository and its remote.
//!
//! Two clones of the same repository, a worktree and its main root, and the
//! same path spelled two different ways on Windows all have to collapse to one
//! key. These helpers are pure string/path work: no filesystem, no git.

use std::path::Path;

/// Normalize a path for a cross-form string prefix comparison: drop the Windows
/// `\\?\` / `\\?\UNC\` verbatim prefix that `fs::canonicalize` adds (git
/// repository roots are non-verbatim), unify separators to `/`, and trim a
/// trailing separator.
///
/// This lets a `fs::canonicalize`d directory and a git-canonicalized repository
/// root be compared as plain strings on every platform — `Path::starts_with`
/// cannot, because a verbatim path never shares a prefix component with a
/// non-verbatim one.
pub fn normalize_for_prefix(path: &Path) -> String {
    let lossy = path.to_string_lossy();
    let raw: &str = lossy.as_ref();
    let stripped = if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw.to_string()
    };
    stripped
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string()
}

/// Stable local-repository identity, independent of Windows separator spelling
/// and drive-letter/path casing.
///
/// WSL paths retain Linux component casing because the filesystem behind the
/// UNC namespace is case-sensitive.
pub fn repo_root_identity(path: &Path) -> String {
    repo_root_identity_for_platform(path, cfg!(target_os = "windows"))
}

/// [`repo_root_identity`] against an explicit platform, so the Windows folding
/// rules stay testable from any host.
pub fn repo_root_identity_for_platform(path: &Path, is_windows: bool) -> String {
    let normalized = normalize_for_prefix(path);
    if is_windows {
        let lower = normalized.to_ascii_lowercase();
        if lower.starts_with("//wsl.localhost/") || lower.starts_with("//wsl$/") {
            normalized
        } else {
            lower
        }
    } else {
        normalized
    }
}

/// Normalize a git remote URL for comparison.
///
/// Strips a trailing `.git`, lowercases, and strips trailing slashes so that
/// `git@github.com:Owner/Repo.git` and `https://github.com/owner/repo` compare
/// as equal.
pub fn normalize_remote_url(url: &str) -> String {
    let trimmed = url.trim();
    let stripped = trimmed
        .strip_suffix(".git")
        .unwrap_or(trimmed)
        .trim_end_matches('/');
    stripped.to_ascii_lowercase()
}

/// Extract the repository name from a remote URL.
///
/// Handles the SSH (`git@host:owner/repo.git`) and HTTP(S)
/// (`https://host/owner/repo.git`) spellings. Returns `None` for anything else,
/// including an owner-only remote with no repository segment.
pub fn parse_repo_name_from_url(url: &str) -> Option<String> {
    let normalized = url.trim();

    // SSH: git@host:owner/repo.git
    if let Some(path) = normalized
        .strip_prefix("git@")
        .and_then(|s| s.split_once(':').map(|(_, path)| path))
    {
        let name = path.split('/').nth(1).unwrap_or(path);
        let name = name.strip_suffix(".git").unwrap_or(name);
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }

    // HTTPS: https://host/owner/repo.git
    if normalized.starts_with("https://") || normalized.starts_with("http://") {
        let parts: Vec<&str> = normalized.split('/').collect();
        if parts.len() >= 5 {
            let name = parts[4];
            let name = name.strip_suffix(".git").unwrap_or(name);
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }

    None
}
