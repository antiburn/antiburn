// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local Git identity, worktree, and remote helpers.
//!
//! Everything here shells out to the user's own `git` through the bounded
//! command helpers in [`super::process`], so a repository inside a mounted WSL
//! distribution is queried with that distribution's `git` and its answers are
//! translated back to host-readable paths.
//!
//! The engine only *reads* local repository state. It runs no command that
//! contacts a network: no `fetch`, no `push`, no `clone`.

use anyhow::{Context, Result, bail};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::process::Output;

use super::environment::{self, DiscoveryEnvironment};
use crate::paths::protected;

/// Upper bound on a single logged text field, so a pathological command cannot
/// flood the log.
const MAX_LOGGED_TEXT_CHARS: usize = 4096;

/// Replace a leading home directory with `~` so logs do not carry the user's
/// account name, then bound the result.
fn sanitize_path(path: &Path) -> String {
    if let Some(home) = crate::paths::home_dir()
        && let Ok(stripped) = path.strip_prefix(home)
    {
        let stripped = stripped.display().to_string();
        if stripped.is_empty() {
            return "~".to_string();
        }
        return truncate_text(format!("~/{stripped}"), MAX_LOGGED_TEXT_CHARS);
    }

    truncate_text(path.display().to_string(), MAX_LOGGED_TEXT_CHARS)
}

fn truncate_text(value: impl AsRef<str>, max_chars: usize) -> String {
    let value = value.as_ref();
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...[truncated]");
    truncated
}

/// Run a git command and return its stdout as a trimmed `String`.
/// Returns an error if the command exits with a non-zero status.
#[cfg(test)]
async fn git_output(args: &[&str]) -> Result<String> {
    let output = run_git_output_at(None, args, &[]).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "git {} failed (exit {}): {}",
            args.join(" "),
            output.status,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8(output.stdout).context("git output was not valid UTF-8")?;
    Ok(stdout.trim().to_string())
}

async fn git_output_in(repo: &Path, args: &[&str]) -> Result<String> {
    let output = run_git_output_at(Some(repo), args, &[]).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }

    let stdout = String::from_utf8(output.stdout).context("git output was not valid UTF-8")?;
    Ok(stdout.trim().to_string())
}

/// Run git for `repo`, inferring the execution environment from its path.
pub async fn run_git_output_at(
    repo: Option<&Path>,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<Output> {
    let environment = repo
        .and_then(environment::environment_from_mounted_path)
        .unwrap_or_default();
    run_git_output_in_environment(&environment, repo, args, envs).await
}

/// Runs Git in the supplied native or WSL environment.
///
/// This helper configures process execution only. Callers that request paths
/// from Git must pass stdout through [`path_from_git_output`] before returning
/// those paths to host-side filesystem, cache, or environment-inference code.
pub async fn run_git_output_in_environment(
    environment: &DiscoveryEnvironment,
    repo: Option<&Path>,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<Output> {
    let mut cmd = environment::configure_command_for_environment(environment, "git", repo)?;
    let mut display_parts = vec!["git".to_string()];

    if let Some(repo) = repo {
        let repo_str = repo.to_string_lossy().to_string();
        display_parts.push("-C".to_string());
        display_parts.push(repo_str);
    }

    for (key, value) in envs {
        cmd.env(key, value);
    }

    let repo_for_log = repo.map(sanitize_path);
    let env_keys = envs
        .iter()
        .map(|(key, _)| (*key).to_string())
        .collect::<Vec<_>>();
    display_parts.extend(args.iter().map(|s| s.to_string()));
    ::tracing::trace!(
        event = "git_command_started",
        repo = repo_for_log.as_deref().unwrap_or(""),
        args = ?args,
        env_keys = ?env_keys
    );
    let output = cmd
        .args(args)
        .output()
        .await
        .context("failed to execute git")?;
    log_git_command_completed(repo, args, &output, false);
    Ok(output)
}

fn log_git_command_completed(repo: Option<&Path>, args: &[&str], output: &Output, verbose: bool) {
    let repo_for_log = repo.map(sanitize_path);
    let status = output.status.code();
    if verbose || !output.status.success() {
        ::tracing::trace!(
            event = "git_command_completed",
            repo = repo_for_log.as_deref().unwrap_or(""),
            args = ?args,
            status = ?status,
            stdout_bytes = output.stdout.len(),
            stderr_bytes = output.stderr.len(),
            stdout = truncate_text(String::from_utf8_lossy(&output.stdout), 2048),
            stderr = truncate_text(String::from_utf8_lossy(&output.stderr), 2048),
        );
        return;
    }
    ::tracing::trace!(
        event = "git_command_completed",
        repo = repo_for_log.as_deref().unwrap_or(""),
        args = ?args,
        status = ?status,
        stdout_bytes = output.stdout.len(),
        stderr_bytes = output.stderr.len(),
    );
}

/// Return the repository root (`git rev-parse --show-toplevel`).
#[cfg(test)]
async fn repo_root() -> Result<PathBuf> {
    let path = git_output(&["rev-parse", "--show-toplevel"]).await?;
    Ok(PathBuf::from(path))
}

/// Return the repository root for a given working directory.
///
/// Runs `git -C <dir> rev-parse --show-toplevel`. This handles the case
/// where `dir` is a subdirectory of the repo.
pub async fn repo_root_at(dir: &Path) -> Result<PathBuf> {
    let environment = environment::environment_from_mounted_path(dir).unwrap_or_default();
    let output = run_git_output_in_environment(
        &environment,
        Some(dir),
        &["rev-parse", "--show-toplevel"],
        &[],
    )
    .await
    .context("failed to execute git rev-parse --show-toplevel")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git rev-parse --show-toplevel failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8(output.stdout).context("git output was not valid UTF-8")?;
    path_from_git_output(&environment, stdout.trim())
}

/// Translates path-valued Git stdout back to the host-readable namespace while
/// preserving its originating WSL distribution identity.
fn path_from_git_output(environment: &DiscoveryEnvironment, path: &str) -> Result<PathBuf> {
    match environment {
        DiscoveryEnvironment::Native => Ok(PathBuf::from(path)),
        DiscoveryEnvironment::Wsl { distribution, .. } => {
            environment::wsl_to_windows_path(distribution, path)
        }
    }
}

/// Why and how a repository root was (or was not) resolved for a working
/// directory that no longer exists on disk.
#[derive(Debug, Clone, Default)]
pub struct RepoRootResolutionDiagnostics {
    /// The working directory resolution started from.
    pub requested_cwd: PathBuf,
    /// Whether that working directory still exists.
    pub cwd_exists: bool,
    /// Error from resolving the working directory directly.
    pub direct_error: Option<String>,
    /// Nearest ancestor of the working directory that still exists.
    pub nearest_existing_ancestor: Option<PathBuf>,
    /// Error from resolving that ancestor.
    pub ancestor_error: Option<String>,
    /// Repository names inferred from worktree-shaped path segments.
    pub candidate_repo_names: Vec<String>,
    /// Existing repositories that could own the missing worktree.
    pub candidate_owner_repo_roots: Vec<PathBuf>,
    /// The owning repository whose worktree list matched.
    pub matched_worktree_owner_repo_root: Option<PathBuf>,
    /// The worktree path that matched inside that repository.
    pub matched_worktree_path: Option<PathBuf>,
    /// Other roots (main repository, linked worktrees) for the resolved repo.
    pub alternative_repo_roots: Vec<PathBuf>,
    /// Which strategy produced the answer.
    pub resolved_via: Option<&'static str>,
}

/// A resolved repository root plus the diagnostics that produced it.
#[derive(Debug, Clone)]
pub struct RepoRootResolution {
    /// The resolved repository root.
    pub repo_root: PathBuf,
    /// How the root was found.
    pub diagnostics: RepoRootResolutionDiagnostics,
}

#[derive(Debug, Clone)]
struct WorktreeEntry {
    path: PathBuf,
}

/// Resolve the repository root for `cwd`, falling back to its nearest existing
/// ancestor and then to the repository that owns a since-deleted worktree.
///
/// `granted` is the set of consent-protected directory names the user has
/// already allowed (see [`crate::paths::protected`]); probing skips protected
/// directories that are not in it.
pub async fn resolve_repo_root_with_fallbacks(
    cwd: &Path,
    granted: &HashSet<String>,
) -> std::result::Result<RepoRootResolution, RepoRootResolutionDiagnostics> {
    let mut diagnostics = RepoRootResolutionDiagnostics {
        requested_cwd: cwd.to_path_buf(),
        cwd_exists: tokio::fs::try_exists(cwd).await.unwrap_or(false),
        ..RepoRootResolutionDiagnostics::default()
    };

    match repo_root_at(cwd).await {
        Ok(repo_root) => {
            diagnostics.resolved_via = Some("cwd");
            diagnostics.alternative_repo_roots = alternative_repo_roots_for_repo(&repo_root).await;
            return Ok(RepoRootResolution {
                repo_root,
                diagnostics,
            });
        }
        Err(err) => diagnostics.direct_error = Some(err.to_string()),
    }

    if let Some(existing_ancestor) = nearest_existing_ancestor(cwd).await {
        diagnostics.nearest_existing_ancestor = Some(existing_ancestor.clone());
        match repo_root_at(&existing_ancestor).await {
            Ok(repo_root) => {
                diagnostics.resolved_via = Some("existing_ancestor");
                diagnostics.alternative_repo_roots =
                    alternative_repo_roots_for_repo(&repo_root).await;
                return Ok(RepoRootResolution {
                    repo_root,
                    diagnostics,
                });
            }
            Err(err) => diagnostics.ancestor_error = Some(err.to_string()),
        }
    }

    let candidate_repo_names = worktree_repo_name_candidates(cwd);
    diagnostics.candidate_repo_names = candidate_repo_names.clone();
    let candidate_owner_repo_roots =
        candidate_owner_repo_roots(cwd, &candidate_repo_names, granted).await;
    diagnostics.candidate_owner_repo_roots = candidate_owner_repo_roots.clone();

    for candidate_repo_root in candidate_owner_repo_roots {
        let worktrees = match worktree_list_at(&candidate_repo_root).await {
            Ok(worktrees) => worktrees,
            Err(_) => continue,
        };
        if let Some(entry) = worktrees
            .into_iter()
            .find(|entry| cwd == entry.path || cwd.starts_with(&entry.path))
        {
            diagnostics.resolved_via = Some("worktree_owner_repo");
            diagnostics.matched_worktree_owner_repo_root = Some(candidate_repo_root.clone());
            diagnostics.matched_worktree_path = Some(entry.path);
            diagnostics.alternative_repo_roots =
                alternative_repo_roots_for_repo(&candidate_repo_root).await;
            return Ok(RepoRootResolution {
                repo_root: candidate_repo_root,
                diagnostics,
            });
        }
    }

    Err(diagnostics)
}

async fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if tokio::fs::try_exists(candidate).await.unwrap_or(false) {
            return Some(candidate.to_path_buf());
        }
        current = candidate.parent();
    }
    None
}

async fn git_common_dir_at(repo: &Path) -> Result<Option<PathBuf>> {
    let environment = environment::environment_from_mounted_path(repo).unwrap_or_default();
    let output = run_git_output_in_environment(
        &environment,
        Some(repo),
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        &[],
    )
    .await
    .context("failed to execute git rev-parse --git-common-dir")?;
    if !output.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8(output.stdout).context("git output was not valid UTF-8")?;
    let common_dir = stdout.trim();
    if common_dir.is_empty() {
        return Ok(None);
    }
    Ok(Some(path_from_git_output(&environment, common_dir)?))
}

/// Resolve the canonical (main) repository root for a path that may be a
/// worktree. For a worktree `--show-toplevel` returns the worktree root,
/// but `--git-common-dir` returns the main repo's `.git` directory whose
/// parent is the true repo root. Returns `repo_root` unchanged when it is
/// already the main repo or on any error.
pub async fn canonical_main_repo_root(repo_root: &Path) -> PathBuf {
    if let Ok(Some(common_dir)) = git_common_dir_at(repo_root).await
        && common_dir.file_name().and_then(|name| name.to_str()) == Some(".git")
        && let Some(main_root) = common_dir.parent()
        && main_root != repo_root
    {
        return main_root.to_path_buf();
    }
    repo_root.to_path_buf()
}

/// Number of worktrees (including the main working tree).
/// Returns 1 for repos with no linked worktrees, 0 on error.
pub async fn worktree_count_at(repo: &Path) -> u32 {
    worktree_list_at(repo)
        .await
        .map(|v| v.len() as u32)
        .unwrap_or(0)
}

async fn alternative_repo_roots_for_repo(repo: &Path) -> Vec<PathBuf> {
    let mut roots = BTreeSet::new();
    roots.insert(repo.to_path_buf());

    if let Ok(Some(common_dir)) = git_common_dir_at(repo).await
        && common_dir.file_name().and_then(|name| name.to_str()) == Some(".git")
        && let Some(main_repo_root) = common_dir.parent()
    {
        roots.insert(main_repo_root.to_path_buf());
    }

    if let Ok(worktrees) = worktree_list_at(repo).await {
        for worktree in worktrees {
            roots.insert(worktree.path);
        }
    }

    roots.into_iter().filter(|path| path != repo).collect()
}

async fn worktree_list_at(repo: &Path) -> Result<Vec<WorktreeEntry>> {
    let environment = environment::environment_from_mounted_path(repo).unwrap_or_default();
    let output = run_git_output_in_environment(
        &environment,
        Some(repo),
        &["worktree", "list", "--porcelain"],
        &[],
    )
    .await
    .context("failed to execute git worktree list --porcelain")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git worktree list --porcelain failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8(output.stdout).context("git output was not valid UTF-8")?;
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(path) = current_path.take() {
                entries.push(WorktreeEntry { path });
            }
            current_path = Some(path_from_git_output(&environment, path.trim())?);
        } else if line.trim().is_empty()
            && let Some(path) = current_path.take()
        {
            entries.push(WorktreeEntry { path });
        }
    }
    if let Some(path) = current_path.take() {
        entries.push(WorktreeEntry { path });
    }
    Ok(entries)
}

fn worktree_repo_name_candidates(cwd: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let normalized = cwd.to_string_lossy().replace('\\', "/");
    let segments = normalized
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    for (idx, segment) in segments.iter().enumerate() {
        if !segment.to_ascii_lowercase().contains("worktree") {
            continue;
        }
        let after = segments.get(idx + 1).cloned();
        let after_after = segments.get(idx + 2).cloned();
        if let Some(candidate) = after.clone().filter(|value| is_repo_name_candidate(value)) {
            names.push(candidate);
        } else if let Some(candidate) = after_after.filter(|value| is_repo_name_candidate(value)) {
            names.push(candidate);
        }
    }
    let mut seen = BTreeSet::new();
    names
        .into_iter()
        .filter(|name| seen.insert(name.to_ascii_lowercase()))
        .collect()
}

fn is_repo_name_candidate(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let lowercase = value.to_ascii_lowercase();
    if lowercase == "worktrees" || lowercase.ends_with("worktrees") {
        return false;
    }
    if value.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }
    true
}

async fn candidate_owner_repo_roots(
    cwd: &Path,
    repo_names: &[String],
    granted: &HashSet<String>,
) -> Vec<PathBuf> {
    let Some(home) = crate::paths::home_dir() else {
        return Vec::new();
    };
    candidate_owner_repo_roots_in(&home, cwd, repo_names, granted).await
}

// Skip probing inside consent-protected directories (e.g. ~/Documents on macOS)
// the user has not granted access to — a bare `try_exists` against those paths
// can trip the native consent dialog on modern macOS and, if the file does
// exist, the follow-up `git` invocation with `cwd` inside the protected
// directory definitely will.
//
// The `None` arm falls through to `true` because `is_access_protected` and
// `protected_dir_name` share one home-directory lookup and one protected-name
// list. If one says "protected" but the other cannot map the path back to a
// name, that is a list mismatch (e.g. a test environment where the home
// directory shifts between calls) rather than a real protected path, so failing
// open is safe.
#[must_use]
fn is_probe_allowed(path: &Path, granted: &HashSet<String>) -> bool {
    if !protected::is_access_protected(path) {
        return true;
    }
    match protected::protected_dir_name(path) {
        Some(name) => granted.contains(&name),
        None => true,
    }
}

/// Existing repositories under `home` that could own a since-deleted worktree
/// at `cwd`, plus any `.git` found by walking up from the nearest existing
/// ancestor of `cwd`.
///
/// `granted` names the consent-protected directories the user already allowed;
/// candidates under any other protected directory are skipped without touching
/// the filesystem.
pub async fn candidate_owner_repo_roots_in(
    home: &Path,
    cwd: &Path,
    repo_names: &[String],
    granted: &HashSet<String>,
) -> Vec<PathBuf> {
    let mut candidates = BTreeSet::new();

    for repo_name in repo_names {
        for parent in [
            home.join("dev"),
            home.join("Documents").join("GitHub"),
            home.join("src"),
            home.join("code"),
            home.join("Projects"),
            home.join("workspaces"),
        ] {
            let candidate = parent.join(repo_name);
            if !is_probe_allowed(&candidate, granted) {
                continue;
            }
            if tokio::fs::try_exists(candidate.join(".git"))
                .await
                .unwrap_or(false)
            {
                candidates.insert(candidate);
            }
        }
    }

    if let Some(existing_ancestor) = nearest_existing_ancestor(cwd).await {
        let mut current = Some(existing_ancestor.as_path());
        while let Some(candidate) = current {
            if is_probe_allowed(candidate, granted)
                && tokio::fs::try_exists(candidate.join(".git"))
                    .await
                    .unwrap_or(false)
            {
                candidates.insert(candidate.to_path_buf());
            }
            current = candidate.parent();
        }
    }

    candidates.into_iter().collect()
}

/// Return the current branch name for a repo, if HEAD is attached.
pub async fn current_branch_at(repo: &Path) -> Result<Option<String>> {
    let output = run_git_output_at(
        Some(repo),
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        &[],
    )
    .await
    .context("failed to execute git symbolic-ref")?;
    if !output.status.success() {
        return Ok(None);
    }
    let branch = String::from_utf8(output.stdout).context("branch output was not valid UTF-8")?;
    let branch = branch.trim();
    if branch.is_empty() {
        Ok(None)
    } else {
        Ok(Some(branch.to_string()))
    }
}

/// Return the current HEAD commit SHA for a repo.
pub async fn head_sha_at(repo: &Path) -> Result<Option<String>> {
    let output = run_git_output_at(Some(repo), &["rev-parse", "HEAD"], &[])
        .await
        .context("failed to execute git rev-parse HEAD")?;

    if !output.status.success() {
        return Ok(None);
    }

    let sha = String::from_utf8(output.stdout).context("head sha output was not valid UTF-8")?;
    let sha = sha.trim();
    if sha.is_empty() {
        Ok(None)
    } else {
        Ok(Some(sha.to_string()))
    }
}

/// Check whether the repository has at least one configured remote.
///
/// Returns `Ok(false)` if `git remote` fails (e.g., not in a git repository)
/// rather than propagating the error, so it is safe to call defensively.
#[cfg(test)]
async fn has_upstream() -> Result<bool> {
    match git_output(&["remote"]).await {
        Ok(remotes) => Ok(!remotes.is_empty()),
        Err(_) => Ok(false),
    }
}

/// Resolve the push remote for the current repository.
///
/// Resolution order:
/// 1) branch.<name>.pushRemote
/// 2) remote.pushDefault
/// 3) branch.<name>.remote
/// 4) if exactly one remote exists, use it
/// 5) otherwise return None
///
/// Returns `Ok(None)` when HEAD is detached or the remote is "."/empty.
#[cfg(test)]
async fn resolve_push_remote() -> Result<Option<String>> {
    let output = run_git_output_at(None, &["symbolic-ref", "--quiet", "--short", "HEAD"], &[])
        .await
        .context("failed to execute git symbolic-ref")?;

    if !output.status.success() {
        return Ok(None);
    }

    let branch =
        String::from_utf8(output.stdout).context("git symbolic-ref output was not valid UTF-8")?;
    let branch = branch.trim();
    if branch.is_empty() {
        return Ok(None);
    }

    let push_remote_key = format!("branch.{branch}.pushRemote");
    if let Ok(Some(remote)) = config_get(&push_remote_key).await
        && !remote.is_empty()
        && remote != "."
    {
        return Ok(Some(remote));
    }

    if let Ok(Some(remote)) = config_get("remote.pushDefault").await
        && !remote.is_empty()
        && remote != "."
    {
        return Ok(Some(remote));
    }

    let branch_remote_key = format!("branch.{branch}.remote");
    if let Ok(Some(remote)) = config_get(&branch_remote_key).await
        && !remote.is_empty()
        && remote != "."
    {
        return Ok(Some(remote));
    }

    let remotes = match git_output(&["remote"]).await {
        Ok(list) => list,
        Err(_) => return Ok(None),
    };
    let mut names = remotes.lines().filter(|r| !r.is_empty());
    let first = names.next();
    if first.is_none() || names.next().is_some() {
        return Ok(None);
    }

    Ok(first.map(|s| s.to_string()))
}

/// Return the URL for a named remote in a specific repository (if any).
pub async fn remote_url_at(repo: &Path, remote: &str) -> Result<Option<String>> {
    let output = run_git_output_at(Some(repo), &["remote", "get-url", remote], &[])
        .await
        .context("failed to execute git remote get-url")?;

    if !output.status.success() {
        return Ok(None);
    }

    let url = String::from_utf8(output.stdout)
        .context("git remote get-url output was not valid UTF-8")?;
    Ok(Some(url.trim().to_string()))
}

/// Return all configured remote URLs for a repository.
pub async fn remote_urls_at(repo: &Path) -> Result<Vec<String>> {
    let output = run_git_output_at(Some(repo), &["remote"], &[])
        .await
        .context("failed to execute git remote")?;
    if !output.status.success() {
        return Ok(Vec::new());
    }

    let remotes =
        String::from_utf8(output.stdout).context("git remote output was not valid UTF-8")?;
    let mut urls = BTreeSet::new();
    for remote in remotes.lines().filter(|name| !name.trim().is_empty()) {
        if let Some(url) = remote_url_at(repo, remote).await?
            && !url.trim().is_empty()
        {
            urls.insert(url);
        }
    }
    Ok(urls.into_iter().collect())
}

/// Return the canonical repo root plus linked worktree roots for a repository.
pub async fn repo_and_worktree_roots_at(repo: &Path) -> Vec<String> {
    let mut roots = Vec::with_capacity(1);
    roots.push(repo.to_string_lossy().to_string());
    roots.extend(
        alternative_repo_roots_for_repo(repo)
            .await
            .into_iter()
            .map(|path| path.to_string_lossy().to_string()),
    );
    roots.sort();
    roots.dedup();
    roots
}

/// Resolve the push remote for a specific repository.
///
/// Resolution order:
/// 1) branch.<name>.pushRemote
/// 2) remote.pushDefault
/// 3) branch.<name>.remote
/// 4) if exactly one remote exists, use it
/// 5) otherwise return None
pub async fn resolve_push_remote_at(repo: &Path) -> Result<Option<String>> {
    let output = run_git_output_at(
        Some(repo),
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        &[],
    )
    .await
    .context("failed to execute git symbolic-ref")?;

    if !output.status.success() {
        return Ok(None);
    }

    let branch =
        String::from_utf8(output.stdout).context("git symbolic-ref output was not valid UTF-8")?;
    let branch = branch.trim();
    if branch.is_empty() {
        return Ok(None);
    }

    let push_remote_key = format!("branch.{branch}.pushRemote");
    if let Ok(Some(remote)) = config_get_at(repo, &push_remote_key).await
        && !remote.is_empty()
        && remote != "."
    {
        return Ok(Some(remote));
    }

    if let Ok(Some(remote)) = config_get_at(repo, "remote.pushDefault").await
        && !remote.is_empty()
        && remote != "."
    {
        return Ok(Some(remote));
    }

    let branch_remote_key = format!("branch.{branch}.remote");
    if let Ok(Some(remote)) = config_get_at(repo, &branch_remote_key).await
        && !remote.is_empty()
        && remote != "."
    {
        return Ok(Some(remote));
    }

    let remotes = match git_output_in(repo, &["remote"]).await {
        Ok(list) => list,
        Err(_) => return Ok(None),
    };
    let mut names = remotes.lines().filter(|r| !r.is_empty());
    let first = names.next();
    if first.is_none() || names.next().is_some() {
        return Ok(None);
    }

    Ok(first.map(|s| s.to_string()))
}

/// Return the URL of the first configured remote for the repo at `repo`.
///
/// Returns `None` if no remotes are configured. Returns an error only if
/// the git commands themselves fail unexpectedly.
pub async fn first_remote_url_at(repo: &Path) -> Result<Option<String>> {
    let output = run_git_output_at(Some(repo), &["remote"], &[])
        .await
        .context("failed to execute git remote")?;

    if !output.status.success() {
        return Ok(None);
    }

    let remotes =
        String::from_utf8(output.stdout).context("git remote output was not valid UTF-8")?;
    let first = match remotes.lines().next() {
        Some(name) if !name.is_empty() => name,
        _ => return Ok(None),
    };

    let url_output = run_git_output_at(Some(repo), &["remote", "get-url", first], &[])
        .await
        .context("failed to execute git remote get-url")?;

    if !url_output.status.success() {
        return Ok(None);
    }

    let url = String::from_utf8(url_output.stdout)
        .context("git remote get-url output was not valid UTF-8")?;
    Ok(Some(url.trim().to_string()))
}

/// Resolve the best available remote URL for a repository.
///
/// Prefers the configured push remote, then falls back to `origin`, then to the
/// first configured remote URL. This keeps repository identity working in repos
/// where the current branch has not been wired to a remote yet.
pub async fn preferred_remote_url_at(repo: &Path) -> Result<Option<String>> {
    if let Some(remote) = resolve_push_remote_at(repo).await?
        && let Some(url) = remote_url_at(repo, &remote).await?
        && !url.trim().is_empty()
    {
        return Ok(Some(url));
    }

    if let Some(url) = remote_url_at(repo, "origin").await?
        && !url.trim().is_empty()
    {
        return Ok(Some(url));
    }

    Ok(first_remote_url_at(repo)
        .await?
        .filter(|url| !url.trim().is_empty()))
}

/// Extract the owner segment from ALL remote URLs of the current repository.
#[cfg(test)]
async fn remote_owners() -> Result<Vec<String>> {
    let remotes = git_output(&["remote"]).await?;
    let mut owners = Vec::new();

    for remote_name in remotes.lines() {
        if remote_name.is_empty() {
            continue;
        }
        if let Ok(url) = git_output(&["remote", "get-url", remote_name]).await
            && let Some(owner) = parse_owner_from_url(&url)
            && !owners
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(&owner))
        {
            owners.push(owner);
        }
    }

    Ok(owners)
}

/// Extract the owner segment from ALL remote URLs in a specific repository.
///
/// Returns a deduplicated list of owner names extracted from all configured
/// remotes. Deduplication is case-insensitive (e.g. `My-Team` and `my-team`
/// from different remotes are the same owner; only the first encountered
/// variant is kept). Returns an empty `Vec` if no remotes are configured or no
/// URLs can be parsed.
pub async fn remote_owners_at(repo: &Path) -> Result<Vec<String>> {
    let remotes = git_output_in(repo, &["remote"]).await?;
    let mut owners = Vec::new();

    for remote_name in remotes.lines() {
        if remote_name.is_empty() {
            continue;
        }
        if let Ok(url) = git_output_in(repo, &["remote", "get-url", remote_name]).await
            && let Some(owner) = parse_owner_from_url(&url)
            && !owners
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(&owner))
        {
            owners.push(owner);
        }
    }

    Ok(owners)
}

/// Parse the owner segment from a git remote URL.
///
/// This is a pure function extracted for testability.
pub fn parse_owner_from_url(url: &str) -> Option<String> {
    // SSH: git@example.com:owner/repo.git
    if let Some(after_colon) = url.strip_prefix("git@").and_then(|s| {
        // Find the colon that separates host from path
        s.split_once(':').map(|(_, path)| path)
    }) {
        let owner = after_colon.split('/').next()?;
        if owner.is_empty() {
            return None;
        }
        return Some(owner.to_string());
    }

    // HTTPS: https://example.com/owner/repo.git
    if url.starts_with("https://") || url.starts_with("http://") {
        // Split on '/' and take the path segments after the host
        let without_scheme = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))?;
        let mut segments = without_scheme.split('/');
        let _host = segments.next(); // e.g. "example.com"
        let owner = segments.next()?;
        if owner.is_empty() {
            return None;
        }
        return Some(owner.to_string());
    }

    None
}

/// Read a git config value. Returns `Ok(None)` if the key is not set.
///
/// Distinguishes exit code 1 (key not set) from other exit codes (e.g., 2 for
/// invalid config file) to avoid silently swallowing genuine errors.
#[cfg(test)]
async fn config_get(key: &str) -> Result<Option<String>> {
    let output = run_git_output_at(None, &["config", "--get", key], &[])
        .await
        .context("failed to execute git config --get")?;

    if !output.status.success() {
        // Exit code 1 means the key is not set — that is not an error.
        // Any other non-zero exit (e.g., code 2 for corrupt config) is a real error.
        let code = output.status.code().unwrap_or(-1);
        if code != 1 {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "git config --get {:?} failed (exit {}): {}",
                key,
                code,
                stderr.trim()
            );
        }
        return Ok(None);
    }

    let value =
        String::from_utf8(output.stdout).context("git config output was not valid UTF-8")?;
    Ok(Some(value.trim().to_string()))
}

/// Read a git config value from a specific repo. Returns `Ok(None)` if unset.
pub async fn config_get_at(repo: &Path, key: &str) -> Result<Option<String>> {
    let output = run_git_output_at(Some(repo), &["config", "--get", key], &[])
        .await
        .context("failed to execute git config --get")?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        if code != 1 {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "git config --get {:?} failed (exit {}): {}",
                key,
                code,
                stderr.trim()
            );
        }
        return Ok(None);
    }

    let value =
        String::from_utf8(output.stdout).context("git config output was not valid UTF-8")?;
    Ok(Some(value.trim().to_string()))
}

/// Read a git config value from global scope. Returns `Ok(None)` if the key is
/// not set.
///
/// Uses `--global` to read only the global config, not repo-local.
pub async fn config_get_global(key: &str) -> Result<Option<String>> {
    let output = run_git_output_at(None, &["config", "--global", "--get", key], &[])
        .await
        .context("failed to execute git config --global --get")?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        if code != 1 {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "git config --global --get {:?} failed (exit {}): {}",
                key,
                code,
                stderr.trim()
            );
        }
        return Ok(None);
    }

    let value =
        String::from_utf8(output.stdout).context("git config output was not valid UTF-8")?;
    Ok(Some(value.trim().to_string()))
}

/// Write a git config value (repo-local scope).
#[cfg(test)]
async fn config_set(key: &str, value: &str) -> Result<()> {
    let output = run_git_output_at(None, &["config", key, value], &[])
        .await
        .context("failed to execute git config set")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git config set failed: {}", stderr.trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    #[test]
    fn path_valued_wsl_git_output_preserves_distribution_namespace() {
        let environment = DiscoveryEnvironment::Wsl {
            distribution: "Ubuntu".into(),
            user: "avery".into(),
        };

        assert_eq!(
            path_from_git_output(&environment, "/home/avery/repo").unwrap(),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\avery\repo")
        );
        assert_eq!(
            path_from_git_output(&environment, "/mnt/c/src/repo").unwrap(),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\mnt\c\src\repo")
        );
    }

    #[test]
    fn sanitize_path_bounds_and_anonymizes() {
        assert_eq!(
            sanitize_path(Path::new("/definitely/not/a/home/repo")),
            "/definitely/not/a/home/repo"
        );
        assert_eq!(truncate_text("abcdef", 3), "abc...[truncated]");
        assert_eq!(truncate_text("abc", 3), "abc");
    }

    struct EnvGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn new(key: &'static str) -> Self {
            Self {
                key,
                original: std::env::var_os(key),
            }
        }

        fn set_path(&self, value: &Path) {
            // SAFETY: every test using this guard is `#[serial]`.
            unsafe { std::env::set_var(self.key, value) };
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: same `#[serial]` guarantee as `set_path`.
            unsafe {
                match &self.original {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    fn portable_test_path(path: &Path) -> PathBuf {
        #[cfg(windows)]
        {
            let raw = path.as_os_str().to_string_lossy();
            if let Some(stripped) = raw.strip_prefix(r"\\?\") {
                return PathBuf::from(stripped);
            }
        }

        path.to_path_buf()
    }

    async fn canonical_test_path(path: &Path) -> PathBuf {
        let canonical = tokio::fs::canonicalize(path)
            .await
            .unwrap_or_else(|_| path.to_path_buf());
        portable_test_path(&canonical)
    }

    /// Create a temporary git repo with one commit.
    ///
    /// The process-wide working directory cannot be changed safely in parallel
    /// tests, so every git command here uses `-C <path>`.
    async fn init_temp_repo() -> TempDir {
        let dir = TempDir::new().expect("failed to create temp dir");
        let path = dir.path();

        run_git(path, &["init"]).await;
        // Set required user config for commits
        run_git(path, &["config", "user.email", "test@example.com"]).await;
        run_git(path, &["config", "user.name", "Test User"]).await;
        // Override hooksPath so a global hook cannot fire during tests.
        run_git(path, &["config", "core.hooksPath", "/dev/null"]).await;
        tokio::fs::write(path.join("README.md"), "hello")
            .await
            .unwrap();
        run_git(path, &["add", "README.md"]).await;
        run_git(path, &["commit", "-m", "initial commit"]).await;

        dir
    }

    /// Run a git command inside the given directory, panicking on failure.
    async fn run_git(dir: &Path, args: &[&str]) -> String {
        let output = run_git_output_at(Some(dir), args, &[])
            .await
            .expect("failed to run git");
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!("git {args:?} failed: {stderr}");
        }
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// Create a temp repo and chdir into it. Returns the `TempDir` (which must
    /// be kept alive to prevent cleanup) and the original cwd for restoration.
    async fn enter_temp_repo() -> (TempDir, PathBuf) {
        let original_cwd = std::env::current_dir().expect("failed to get cwd");
        let dir = init_temp_repo().await;
        std::env::set_current_dir(dir.path()).expect("failed to chdir into temp repo");
        (dir, original_cwd)
    }

    #[cfg(target_os = "macos")]
    async fn make_git_dir(path: &Path) {
        tokio::fs::create_dir_all(path.join(".git")).await.unwrap();
    }

    async fn init_repo_with_branch(repo_root: &Path, name: &str) {
        tokio::fs::create_dir_all(repo_root.parent().expect("repo parent"))
            .await
            .expect("create repo parent");
        run_git(repo_root.parent().expect("repo parent"), &["init", name]).await;
        run_git(repo_root, &["config", "user.email", "test@example.com"]).await;
        run_git(repo_root, &["config", "user.name", "Test User"]).await;
        run_git(repo_root, &["config", "core.hooksPath", "/dev/null"]).await;
        tokio::fs::write(repo_root.join("README.md"), "hello")
            .await
            .expect("write readme");
        run_git(repo_root, &["add", "README.md"]).await;
        run_git(repo_root, &["commit", "-m", "initial commit"]).await;
        run_git(repo_root, &["branch", "feature"]).await;
    }

    #[tokio::test]
    async fn repo_root_at_finds_the_worktree_root() {
        let dir = init_temp_repo().await;
        let root = repo_root_at(dir.path()).await.unwrap();
        assert!(root.exists());
        assert!(root.join("README.md").exists());
    }

    #[tokio::test]
    async fn resolve_repo_root_with_fallbacks_uses_existing_ancestor() {
        let dir = init_temp_repo().await;
        let missing = dir.path().join("missing").join("nested");

        let resolution = resolve_repo_root_with_fallbacks(&missing, &HashSet::new())
            .await
            .expect("resolve repo root with fallbacks");

        assert_eq!(
            resolution
                .repo_root
                .canonicalize()
                .expect("canonical repo root"),
            dir.path().canonicalize().expect("canonical temp repo")
        );
        assert_eq!(
            resolution.diagnostics.resolved_via,
            Some("existing_ancestor")
        );
        assert_eq!(
            resolution.diagnostics.nearest_existing_ancestor,
            Some(dir.path().to_path_buf())
        );
    }

    #[tokio::test]
    #[serial]
    async fn resolve_repo_root_with_fallbacks_uses_worktree_owner_repo() {
        let home = TempDir::new().expect("home tempdir");
        let canonical_home = canonical_test_path(home.path()).await;
        let home_guard = EnvGuard::new("HOME");
        home_guard.set_path(&canonical_home);

        let repo_root = canonical_home.join("dev").join("example-cli");
        init_repo_with_branch(&repo_root, "example-cli").await;

        let worktree_path = canonical_home
            .join(".claude-worktrees")
            .join("example-cli")
            .join("vigorous-engelbart");
        tokio::fs::create_dir_all(worktree_path.parent().expect("worktree parent"))
            .await
            .expect("create worktree parent");
        run_git(
            &repo_root,
            &[
                "worktree",
                "add",
                worktree_path.to_str().expect("worktree path utf8"),
                "feature",
            ],
        )
        .await;

        tokio::fs::remove_dir_all(&worktree_path)
            .await
            .expect("remove worktree directory");

        let resolution = resolve_repo_root_with_fallbacks(&worktree_path, &HashSet::new())
            .await
            .expect("resolve repo root via worktree owner");

        assert_eq!(
            resolution
                .repo_root
                .canonicalize()
                .expect("canonical repo root"),
            repo_root.canonicalize().expect("canonical main repo")
        );
        assert_eq!(
            resolution.diagnostics.resolved_via,
            Some("worktree_owner_repo")
        );
        assert_eq!(
            resolution.diagnostics.candidate_repo_names,
            vec!["example-cli".to_string()]
        );
        assert_eq!(
            resolution.diagnostics.matched_worktree_owner_repo_root,
            Some(repo_root.clone())
        );
        assert_eq!(
            resolution.diagnostics.matched_worktree_path,
            Some(worktree_path.clone())
        );
    }

    #[tokio::test]
    #[serial]
    async fn resolve_repo_root_with_fallbacks_uses_worktree_owner_repo_for_missing_subdir() {
        let home = TempDir::new().expect("home tempdir");
        let canonical_home = canonical_test_path(home.path()).await;
        let home_guard = EnvGuard::new("HOME");
        home_guard.set_path(&canonical_home);

        let repo_root = canonical_home.join("dev").join("example-cli");
        init_repo_with_branch(&repo_root, "example-cli").await;

        let worktree_path = canonical_home
            .join(".claude-worktrees")
            .join("example-cli")
            .join("vigorous-engelbart");
        tokio::fs::create_dir_all(worktree_path.parent().expect("worktree parent"))
            .await
            .expect("create worktree parent");
        run_git(
            &repo_root,
            &[
                "worktree",
                "add",
                worktree_path.to_str().expect("worktree path utf8"),
                "feature",
            ],
        )
        .await;

        tokio::fs::remove_dir_all(&worktree_path)
            .await
            .expect("remove worktree directory");

        let missing_subdir = worktree_path.join("src").join("nested");
        let resolution = resolve_repo_root_with_fallbacks(&missing_subdir, &HashSet::new())
            .await
            .expect("resolve repo root via worktree owner");

        assert_eq!(
            resolution
                .repo_root
                .canonicalize()
                .expect("canonical repo root"),
            repo_root.canonicalize().expect("canonical main repo")
        );
        assert_eq!(
            resolution.diagnostics.resolved_via,
            Some("worktree_owner_repo")
        );
        assert_eq!(
            resolution.diagnostics.matched_worktree_owner_repo_root,
            Some(repo_root.clone())
        );
        assert_eq!(
            resolution.diagnostics.matched_worktree_path,
            Some(worktree_path.clone())
        );
    }

    #[tokio::test]
    #[serial]
    async fn alternative_repo_roots_include_linked_worktrees() {
        let dir = init_temp_repo().await;
        run_git(dir.path(), &["branch", "feature"]).await;
        let worktree_root = TempDir::new().expect("worktree tempdir");
        let canonical_worktree_root = canonical_test_path(worktree_root.path()).await;
        let worktree_path = canonical_worktree_root.join("feature-worktree");
        run_git(
            dir.path(),
            &[
                "worktree",
                "add",
                worktree_path.to_str().expect("worktree path utf8"),
                "feature",
            ],
        )
        .await;

        let alternatives = alternative_repo_roots_for_repo(dir.path()).await;
        let canonical_worktree_path = tokio::fs::canonicalize(&worktree_path)
            .await
            .expect("canonicalize linked worktree");

        assert!(alternatives.into_iter().any(|candidate| {
            std::fs::canonicalize(candidate)
                .map(|path| path == canonical_worktree_path)
                .unwrap_or(false)
        }));
    }

    #[tokio::test]
    #[serial]
    async fn canonical_main_repo_root_resolves_a_linked_worktree_to_its_owner() {
        let dir = init_temp_repo().await;
        run_git(dir.path(), &["branch", "feature"]).await;
        let worktree_root = TempDir::new().expect("worktree tempdir");
        let worktree_path = canonical_test_path(worktree_root.path())
            .await
            .join("feature-worktree");
        run_git(
            dir.path(),
            &[
                "worktree",
                "add",
                worktree_path.to_str().expect("worktree path utf8"),
                "feature",
            ],
        )
        .await;

        assert_eq!(
            canonical_main_repo_root(&worktree_path)
                .await
                .canonicalize()
                .expect("canonical main repo root"),
            dir.path().canonicalize().expect("canonical temp repo")
        );
        assert_eq!(worktree_count_at(dir.path()).await, 2);
    }

    #[test]
    fn worktree_repo_name_candidates_cover_common_agent_layouts() {
        let claude = worktree_repo_name_candidates(Path::new(
            "/Users/avery/.claude-worktrees/example-cli/vigorous-engelbart",
        ));
        assert_eq!(claude, vec!["example-cli".to_string()]);

        let codex =
            worktree_repo_name_candidates(Path::new("/Users/avery/.codex/worktrees/3139/example"));
        assert_eq!(codex, vec!["example".to_string()]);

        let windows_codex = worktree_repo_name_candidates(Path::new(
            r"C:\Users\avery\.codex\worktrees\3139\example-app",
        ));
        assert_eq!(windows_codex, vec!["example-app".to_string()]);

        let unc_claude = worktree_repo_name_candidates(Path::new(
            r"\\server\share\.claude-worktrees\example-cli\branch-one",
        ));
        assert_eq!(unc_claude, vec!["example-cli".to_string()]);
    }

    #[tokio::test]
    async fn head_sha_and_branch_are_readable() {
        let dir = init_temp_repo().await;
        let sha = head_sha_at(dir.path()).await.unwrap().expect("head sha");
        assert_eq!(sha.len(), 40);
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(current_branch_at(dir.path()).await.unwrap().is_some());

        run_git(dir.path(), &["checkout", "--detach", "HEAD"]).await;
        // A detached HEAD still has a SHA but no branch.
        assert_eq!(head_sha_at(dir.path()).await.unwrap(), Some(sha));
        assert_eq!(current_branch_at(dir.path()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn head_sha_changes_after_a_new_commit() {
        let dir = init_temp_repo().await;
        let path = dir.path();

        let first = head_sha_at(path).await.unwrap().expect("first sha");

        tokio::fs::write(path.join("file2.txt"), "content")
            .await
            .unwrap();
        run_git(path, &["add", "file2.txt"]).await;
        run_git(path, &["commit", "-m", "second commit"]).await;

        let second = head_sha_at(path).await.unwrap().expect("second sha");
        assert_ne!(first, second);
        assert_eq!(second.len(), 40);
    }

    #[tokio::test]
    async fn remote_urls_are_empty_without_remotes() {
        let dir = init_temp_repo().await;
        assert!(remote_urls_at(dir.path()).await.unwrap().is_empty());
        assert_eq!(first_remote_url_at(dir.path()).await.unwrap(), None);
        assert_eq!(preferred_remote_url_at(dir.path()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn remote_owners_at_collects_owners() {
        let dir = init_temp_repo().await;
        let path = dir.path();

        run_git(
            path,
            &[
                "remote",
                "add",
                "origin",
                "git@example.com:owner-one/repo.git",
            ],
        )
        .await;
        run_git(
            path,
            &[
                "remote",
                "add",
                "upstream",
                "https://example.com/owner-two/repo.git",
            ],
        )
        .await;

        let owners = remote_owners_at(path).await.expect("remote_owners_at");
        assert_eq!(owners.len(), 2);
        assert!(owners.contains(&"owner-one".to_string()));
        assert!(owners.contains(&"owner-two".to_string()));
        assert_eq!(remote_urls_at(path).await.unwrap().len(), 2);
        // A repository with no linked worktrees reports itself as a root.
        let roots = repo_and_worktree_roots_at(path).await;
        assert!(roots.contains(&path.to_string_lossy().to_string()));
    }

    #[test]
    fn parse_owner_from_url_handles_supported_shapes() {
        assert_eq!(
            parse_owner_from_url("git@example.com:my-owner/my-repo.git"),
            Some("my-owner".to_string())
        );
        assert_eq!(
            parse_owner_from_url("https://example.com/other-owner/some-repo.git"),
            Some("other-owner".to_string())
        );
        assert_eq!(
            parse_owner_from_url("http://example.com/http-owner/repo.git"),
            Some("http-owner".to_string())
        );
        // SSH with nested paths — the owner is the first segment after the colon.
        assert_eq!(
            parse_owner_from_url("git@example.com:owner/sub/repo.git"),
            Some("owner".to_string())
        );
        // "example.com:443" is treated as the host, "owner" is the owner.
        assert_eq!(
            parse_owner_from_url("https://example.com:443/owner/repo.git"),
            Some("owner".to_string())
        );
        // Embedded credentials are part of the host segment.
        assert_eq!(
            parse_owner_from_url("https://user:pass@example.com/owner/repo.git"),
            Some("owner".to_string())
        );
        // Trailing slash after the owner still parses.
        assert_eq!(
            parse_owner_from_url("https://example.com/owner/"),
            Some("owner".to_string())
        );

        assert_eq!(parse_owner_from_url("svn://example.com/repo"), None);
        assert_eq!(parse_owner_from_url(""), None);
        assert_eq!(parse_owner_from_url("https://example.com/"), None);
        assert_eq!(parse_owner_from_url("https://example.com"), None);
    }

    #[tokio::test]
    async fn config_get_at_reads_set_values_and_reports_missing_keys() {
        let dir = init_temp_repo().await;
        let path = dir.path();

        assert_eq!(
            config_get_at(path, "ai.example.nonexistent").await.unwrap(),
            None
        );

        run_git(path, &["config", "ai.example.enabled", "true"]).await;
        assert_eq!(
            config_get_at(path, "ai.example.enabled").await.unwrap(),
            Some("true".to_string())
        );

        run_git(path, &["config", "ai.example.enabled", "false"]).await;
        assert_eq!(
            config_get_at(path, "ai.example.enabled").await.unwrap(),
            Some("false".to_string())
        );
    }

    #[tokio::test]
    #[serial]
    async fn cwd_repo_root_and_upstream_helpers_read_the_current_repository() {
        let (_dir, original_cwd) = enter_temp_repo().await;

        let root = repo_root().await.expect("repo_root");
        assert!(root.join("README.md").exists());
        assert!(!has_upstream().await.expect("has_upstream"));
        assert!(remote_owners().await.expect("remote_owners").is_empty());

        std::env::set_current_dir(original_cwd).unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn cwd_config_helpers_round_trip() {
        let (_dir, original_cwd) = enter_temp_repo().await;

        assert_eq!(config_get("ai.example.nonexistent").await.unwrap(), None);
        config_set("ai.example.enabled", "true")
            .await
            .expect("config_set");
        assert_eq!(
            config_get("ai.example.enabled").await.unwrap(),
            Some("true".to_string())
        );

        std::env::set_current_dir(original_cwd).unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn has_upstream_is_true_once_a_remote_exists() {
        let (dir, original_cwd) = enter_temp_repo().await;
        run_git(
            dir.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://example.com/test-owner/test-repo.git",
            ],
        )
        .await;

        assert!(has_upstream().await.expect("has_upstream"));

        std::env::set_current_dir(original_cwd).unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn resolve_push_remote_prefers_branch_push_remote() {
        let (dir, original_cwd) = enter_temp_repo().await;
        let branch = run_git(dir.path(), &["symbolic-ref", "--short", "HEAD"]).await;

        run_git(
            dir.path(),
            &[
                "remote",
                "add",
                "upstream",
                "https://example.com/example/upstream.git",
            ],
        )
        .await;
        run_git(
            dir.path(),
            &["config", &format!("branch.{branch}.pushRemote"), "upstream"],
        )
        .await;

        assert_eq!(
            resolve_push_remote().await.expect("resolve_push_remote"),
            Some("upstream".to_string())
        );

        std::env::set_current_dir(original_cwd).unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn resolve_push_remote_uses_push_default() {
        let (dir, original_cwd) = enter_temp_repo().await;

        run_git(
            dir.path(),
            &[
                "remote",
                "add",
                "fork",
                "https://example.com/example/fork.git",
            ],
        )
        .await;
        run_git(dir.path(), &["config", "remote.pushDefault", "fork"]).await;

        assert_eq!(
            resolve_push_remote().await.expect("resolve_push_remote"),
            Some("fork".to_string())
        );

        std::env::set_current_dir(original_cwd).unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn resolve_push_remote_uses_branch_remote() {
        let (dir, original_cwd) = enter_temp_repo().await;
        let branch = run_git(dir.path(), &["symbolic-ref", "--short", "HEAD"]).await;

        run_git(
            dir.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://example.com/example/origin.git",
            ],
        )
        .await;
        run_git(
            dir.path(),
            &["config", &format!("branch.{branch}.remote"), "origin"],
        )
        .await;

        assert_eq!(
            resolve_push_remote().await.expect("resolve_push_remote"),
            Some("origin".to_string())
        );

        std::env::set_current_dir(original_cwd).unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn resolve_push_remote_uses_single_remote_fallback() {
        let (dir, original_cwd) = enter_temp_repo().await;

        run_git(
            dir.path(),
            &[
                "remote",
                "add",
                "solo",
                "https://example.com/example/solo.git",
            ],
        )
        .await;

        assert_eq!(
            resolve_push_remote().await.expect("resolve_push_remote"),
            Some("solo".to_string())
        );

        std::env::set_current_dir(original_cwd).unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn resolve_push_remote_detached_head_returns_none() {
        let (dir, original_cwd) = enter_temp_repo().await;
        run_git(dir.path(), &["checkout", "--detach", "HEAD"]).await;

        assert_eq!(
            resolve_push_remote().await.expect("resolve_push_remote"),
            None
        );

        std::env::set_current_dir(original_cwd).unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn preferred_remote_url_uses_origin_when_branch_remote_is_unset() {
        let (dir, original_cwd) = enter_temp_repo().await;

        run_git(
            dir.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://example.com/example/origin.git",
            ],
        )
        .await;
        run_git(
            dir.path(),
            &[
                "remote",
                "add",
                "upstream",
                "https://example.com/example/upstream.git",
            ],
        )
        .await;

        assert_eq!(
            preferred_remote_url_at(dir.path())
                .await
                .expect("preferred_remote_url_at"),
            Some("https://example.com/example/origin.git".to_string())
        );

        std::env::set_current_dir(original_cwd).unwrap();
    }

    #[tokio::test]
    #[serial]
    #[cfg(target_os = "macos")]
    async fn candidate_owner_repo_roots_skips_protected_dirs_when_not_granted() {
        let home_dir = TempDir::new().unwrap();
        let home = home_dir.path();
        let guard = EnvGuard::new("HOME");
        guard.set_path(home);

        let dev_repo = home.join("dev").join("myrepo");
        let docs_repo = home.join("Documents").join("GitHub").join("myrepo");
        make_git_dir(&dev_repo).await;
        make_git_dir(&docs_repo).await;

        let cwd = home.join("nonexistent");
        let names = vec!["myrepo".to_string()];

        let result = candidate_owner_repo_roots_in(home, &cwd, &names, &HashSet::new()).await;

        assert!(result.contains(&dev_repo), "dev candidate should be probed");
        assert!(
            !result.contains(&docs_repo),
            "Documents candidate must be skipped when not granted; got {result:?}"
        );
    }

    #[tokio::test]
    #[serial]
    #[cfg(target_os = "macos")]
    async fn candidate_owner_repo_roots_probes_protected_dirs_when_granted() {
        let home_dir = TempDir::new().unwrap();
        let home = home_dir.path();
        let guard = EnvGuard::new("HOME");
        guard.set_path(home);

        let docs_repo = home.join("Documents").join("GitHub").join("myrepo");
        make_git_dir(&docs_repo).await;

        let cwd = home.join("nonexistent");
        let names = vec!["myrepo".to_string()];

        let granted: HashSet<String> = HashSet::from(["Documents".to_string()]);
        let result = candidate_owner_repo_roots_in(home, &cwd, &names, &granted).await;

        assert!(
            result.contains(&docs_repo),
            "Documents candidate should be returned once granted; got {result:?}"
        );
    }

    #[tokio::test]
    #[serial]
    #[cfg(target_os = "macos")]
    async fn candidate_owner_repo_roots_ancestor_walk_skips_protected_but_continues() {
        // The ancestor walk should skip `try_exists` for nodes inside ~/Documents
        // when not granted, but keep walking upward so that a `.git` at the home
        // root (above the protected boundary) is still discovered.
        let home_dir = TempDir::new().unwrap();
        let home = home_dir.path();
        let guard = EnvGuard::new("HOME");
        guard.set_path(home);

        // .git at the home root (outside Documents).
        make_git_dir(home).await;
        // .git inside ~/Documents/GitHub/repo — must be skipped.
        let docs_repo = home.join("Documents").join("GitHub").join("repo");
        make_git_dir(&docs_repo).await;

        // cwd lives under ~/Documents so the walk starts inside protected territory.
        let cwd = docs_repo.join("subdir");
        let result = candidate_owner_repo_roots_in(home, &cwd, &[], &HashSet::new()).await;

        assert!(
            !result.contains(&docs_repo),
            "ancestor inside Documents must be skipped; got {result:?}"
        );
        assert!(
            result.iter().any(|p| p == home),
            "ancestor walk must continue past protected dirs to reach unprotected ancestors; got {result:?}"
        );
    }

    #[tokio::test]
    #[serial]
    #[cfg(target_os = "macos")]
    async fn is_probe_allowed_unprotected_path_always_allowed() {
        let home_dir = TempDir::new().unwrap();
        let guard = EnvGuard::new("HOME");
        guard.set_path(home_dir.path());

        let unprotected = home_dir.path().join("dev").join("myrepo");
        assert!(is_probe_allowed(&unprotected, &HashSet::new()));
    }

    #[tokio::test]
    #[cfg(not(target_os = "macos"))]
    async fn every_path_is_probeable_without_consent_controls() {
        assert!(is_probe_allowed(
            Path::new("/home/avery/Documents/repo"),
            &HashSet::new()
        ));
    }
}
