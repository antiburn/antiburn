// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared fixtures: an in-memory consent record, a scoped `$HOME`, and a
//! one-call git repository builder.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::platform::git;
use crate::repositories::{ConsentGrants, LocatedRepo};

/// An in-memory [`ConsentGrants`], so grant handling is testable without any
/// persistence or real consent-controlled directory.
///
/// `home` is the root the recorded directory names hang off, mirroring how a
/// real implementation resolves `"Documents"` to `<home>/Documents`.
#[derive(Debug)]
pub struct FakeConsent {
    home: PathBuf,
    granted: Mutex<HashSet<String>>,
    /// Directory names [`ConsentGrants::discover_external_grants`] should report
    /// as newly granted.
    external: Mutex<HashSet<String>>,
    probes: Mutex<Vec<(String, String)>>,
}

impl FakeConsent {
    /// A record with `granted` already allowed under `home`.
    pub fn with_grants<I, S>(home: &Path, granted: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            home: home.to_path_buf(),
            granted: Mutex::new(granted.into_iter().map(Into::into).collect()),
            external: Mutex::new(HashSet::new()),
            probes: Mutex::new(Vec::new()),
        }
    }

    /// A record with nothing granted.
    pub fn empty(home: &Path) -> Self {
        Self::with_grants(home, Vec::<String>::new())
    }

    /// Stage `dir_name` as a grant made outside the application.
    // Consumed only by the macOS consent-promotion tests; other targets
    // never stage external grants.
    #[cfg(target_os = "macos")]
    pub fn stage_external_grant(&self, dir_name: &str) {
        self.external.lock().unwrap().insert(dir_name.to_string());
    }

    /// The grant names currently recorded.
    pub fn grants(&self) -> HashSet<String> {
        self.granted.lock().unwrap().clone()
    }

    /// Every `(target, outcome)` probe recorded so far.
    pub fn probes(&self) -> Vec<(String, String)> {
        self.probes.lock().unwrap().clone()
    }
}

#[async_trait]
impl ConsentGrants for FakeConsent {
    fn granted_dirs(&self) -> HashSet<String> {
        self.grants()
    }

    async fn discover_external_grants(&self, dir_names: &HashSet<String>) -> HashSet<String> {
        let staged = self.external.lock().unwrap().clone();
        let newly: HashSet<String> = dir_names.intersection(&staged).cloned().collect();
        self.granted.lock().unwrap().extend(newly.iter().cloned());
        newly
    }

    async fn revoke_grant_covering(&self, path: &Path) -> Option<String> {
        let mut granted = self.granted.lock().unwrap();
        let stale = granted
            .iter()
            .find(|name| path.starts_with(self.home.join(name)))?
            .clone();
        granted.remove(&stale);
        Some(stale)
    }

    fn record_probe(&self, target: &str, outcome: &str, _elapsed_ms: u64) {
        self.probes
            .lock()
            .unwrap()
            .push((target.to_string(), outcome.to_string()));
    }
}

/// Scoped environment override so a test's home directory is a temp dir.
pub struct EnvGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvGuard {
    pub fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: every test using this guard is `#[serial]`, so no other
        // thread reads or writes the environment concurrently.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: same `#[serial]` guarantee as `set`.
        match self.previous.take() {
            Some(previous) => unsafe { std::env::set_var(self.key, previous) },
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

/// Run a git command in `dir`, panicking on failure.
pub async fn run_git(dir: &Path, args: &[&str]) {
    let output = git::run_git_output_at(Some(dir), args, &[])
        .await
        .expect("git should be installed");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Create a repository on disk with the given remote. No commit is needed —
/// discovery resolves repositories by their remote URL.
pub async fn init_repo(dir: &Path, remote_url: &str) {
    tokio::fs::create_dir_all(dir).await.unwrap();
    run_git(dir, &["init", "-q"]).await;
    run_git(dir, &["remote", "add", "origin", remote_url]).await;
}

/// Make `dir` unreadable, run `f`, then restore permissions so the temp dir can
/// still be cleaned up. Mirrors a consent denial: `read_dir` fails with
/// `PermissionDenied` while `stat` still succeeds.
#[cfg(unix)]
pub async fn with_unreadable_dir<F, Fut, T>(dir: &Path, f: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    use std::os::unix::fs::PermissionsExt;
    let original = std::fs::metadata(dir).unwrap().permissions();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    let out = f().await;
    std::fs::set_permissions(dir, original).unwrap();
    out
}

/// A repository already located on disk, for the pure matching tests.
pub fn located(
    root: &str,
    remote: &str,
    accessible: bool,
    worktrees: u32,
    sessions: u32,
) -> LocatedRepo {
    LocatedRepo {
        remote_url: remote.to_string(),
        repo_root: PathBuf::from(root),
        dir_accessible: accessible,
        worktree_count: worktrees,
        session_count: sessions,
    }
}
