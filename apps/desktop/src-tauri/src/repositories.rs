//! Which repositories on this machine antiburn watches.
//!
//! # Why this is assembled here rather than taken whole from the engine
//!
//! The engine's merged pipeline (`repositories::discover_repositories`) is
//! **owner-scoped**: the forward scan keeps a clone only when its git remote
//! names a specific owner, and the session pass that would supply that owner is
//! crate-private. antiburn has no account and therefore no owner to supply, so
//! it assembles the same two passes from the engine's public parts:
//!
//! 1. **From sessions.** Every working directory the store has seen resolves to
//!    a canonical repository root through the engine's git helpers. This is the
//!    authoritative list — it finds clones wherever they live and yields the
//!    per-repository session count.
//! 2. **Forward, over the reader's own scan roots.** The engine's
//!    [`scan_roots_for_repos`] walk *is* owner-scoped, so it runs once per owner
//!    the first pass observed (capped at [`MAX_SCAN_OWNERS`]). That is what
//!    surfaces a sibling clone with no agent sessions yet.
//!
//! Both passes are bounded. Session working directories already under a known
//! root are resolved from the store rather than by shelling out to git again,
//! so the steady state costs one git call per genuinely new repository.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use antiburn_local::paths::{home_dir, ignored_paths, protected};
use antiburn_local::platform::git;
use antiburn_local::repositories::{
    ConsentGrants, DeferredProtectedPath, LocatedRepo, dedup_deferred_by_cwd, merge_located_repos,
    normalize_remote_url, parse_repo_name_from_url, partition_cwds_by_grants, repo_root_identity,
    scan_roots_for_repos, verify_dir_access,
};
use tauri::{AppHandle, Manager};

use crate::consent::StoreConsentGrants;
use crate::dto::{DeferredPermissionDir, RepositoryItem};
use crate::scan::IGNORE_SCOPE;
use crate::store::{RepositoryRecord, Store};

/// Owners the forward scan runs for, most sessions first. A machine that works
/// across many owners still gets its session-located repositories in full; the
/// cap only bounds the extra walk that finds clones with no sessions.
const MAX_SCAN_OWNERS: usize = 4;

/// Session working directories resolved through git in one pass. Directories
/// already under a known repository root do not count against this.
const MAX_NEW_ROOTS_PER_PASS: usize = 64;

/// Sessions considered when counting per-repository activity.
const MAX_SESSIONS_CONSIDERED: usize = 2_000;

/// Re-derive the repository list and persist it.
pub async fn refresh(app: &AppHandle) -> anyhow::Result<()> {
    let store = app.state::<Store>();
    let ignored = ignored_paths::load_ignored(store.state_dir(), IGNORE_SCOPE);

    // Repository counts use the newest bounded slice of everything indexed,
    // with no age-based expiry. The cap protects an always-running utility
    // from loading an unbounded history into memory at once.
    let sessions = store.recent_sessions(0, MAX_SESSIONS_CONSIDERED)?;
    let known: Vec<RepositoryRecord> = store.repositories()?;
    let scan_roots: Vec<PathBuf> = store.scan_roots()?.into_iter().map(PathBuf::from).collect();

    // What the user has already allowed. Read, never probed: confirming a grant
    // means reading the directory, which is the very thing that prompts.
    let consent = StoreConsentGrants::new(&store);
    let granted = consent.granted_dirs();

    let cwds: Vec<String> = distinct_cwds(&sessions);
    // `deferred` names the protected directories this pass deliberately left
    // untouched, so the interface can offer the user a way to grant them.
    let (located, deferred) = locate_from_cwds(&cwds, &known, &granted, &consent).await;
    let counted = count_sessions(located, &sessions);

    let owners = top_owners(&counted);
    let (scanned, scan_deferred) = forward_scan(&owners, &scan_roots, &consent).await;
    let mut deferred = merge_deferred(deferred, scan_deferred);

    // A read this pass may have observed that a recorded grant is stale and
    // dropped it. The deferred list was derived before that happened, so it
    // still describes a world where the folder was reachable. Re-derive it
    // against what is true now — partitioning is pure, so this costs nothing
    // and saves the reader a pass spent wondering where their repositories
    // went.
    let granted_now = consent.granted_dirs();
    if granted_now != granted
        && let Some(home) = home_dir()
    {
        let (_, still_blocked) = partition_cwds_by_grants(&home, &cwds, &granted_now);
        deferred = merge_deferred(deferred, still_blocked);
    }

    let merged = merge_located_repos(counted.into_iter().map(|(repo, _)| repo).collect(), scanned);
    let mut records = Vec::with_capacity(merged.len());
    for repo in &merged {
        records.push(to_record(repo, &sessions, &known, &ignored).await);
    }
    retain_disabled(&mut records, known);
    surface_orphaned_opt_outs(&mut records, store.state_dir(), &granted).await;
    store.replace_repositories(&records)?;
    store.set_deferred_permission_dirs(&summarize_deferred(&deferred))?;
    Ok(())
}

/// Combine two deferred lists, keeping each working directory once.
fn merge_deferred(
    mut first: Vec<DeferredProtectedPath>,
    second: Vec<DeferredProtectedPath>,
) -> Vec<DeferredProtectedPath> {
    first.extend(second);
    dedup_deferred_by_cwd(&mut first);
    first
}

/// Reduce deferred paths to one entry per protected directory, since that is
/// the granularity the operating system grants at and therefore the only
/// granularity worth asking the user about.
fn summarize_deferred(deferred: &[DeferredProtectedPath]) -> Vec<DeferredPermissionDir> {
    let mut order: Vec<String> = Vec::new();
    let mut counts: HashMap<String, u32> = HashMap::new();
    for entry in deferred {
        if !counts.contains_key(&entry.protected_dir) {
            order.push(entry.protected_dir.clone());
        }
        *counts.entry(entry.protected_dir.clone()).or_insert(0) += 1;
    }
    order
        .into_iter()
        .map(|dir| DeferredPermissionDir {
            path_count: counts.get(&dir).copied().unwrap_or(0),
            dir,
        })
        .collect()
}

/// Give every opted-out root a Settings row, even when its record is gone.
///
/// An opted-out repository produces no session evidence, so nothing re-derives
/// its row — and a row that vanished (an earlier build dropped it) leaves the
/// opt-out active with no toggle to undo it. Any ignored path that exists on
/// disk and matches no listed record is surfaced as a disabled row: name from
/// its git remote when one answers, its folder name otherwise.
async fn surface_orphaned_opt_outs(
    records: &mut Vec<RepositoryRecord>,
    state_dir: &Path,
    granted: &HashSet<String>,
) {
    let listed: HashSet<String> = records
        .iter()
        .flat_map(|record| {
            record
                .repo_root
                .iter()
                .chain(record.suspected_path.iter())
                .map(|path| repo_root_identity(Path::new(path)))
        })
        .collect();

    for path in ignored_paths::list_ignored(state_dir, IGNORE_SCOPE) {
        let root = Path::new(&path);
        if listed.contains(&repo_root_identity(root)) || !root.is_dir() {
            continue;
        }
        let folder_name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("repository")
            .to_string();
        // An opted-out repository inside an ungranted protected directory keeps
        // its row, named after its folder: reading its remote would mean running
        // git in there, and the row exists precisely so the user can act on a
        // repository antiburn is not otherwise touching.
        let remote_name = if protected::cwd_resolution_blocked(root, granted) {
            None
        } else {
            git::first_remote_url_at(root)
                .await
                .ok()
                .flatten()
                .and_then(|url| parse_repo_name_from_url(&normalize_remote_url(&url)))
        };
        let (repo_name, full_name) = match remote_name {
            Some(name) => (name.rsplit('/').next().unwrap_or(&name).to_string(), name),
            None => (folder_name.clone(), folder_name),
        };
        records.push(RepositoryRecord {
            key: repo_root_identity(root),
            repo_name,
            full_name,
            status: "accessible".to_string(),
            repo_root: Some(path),
            suspected_path: None,
            worktree_count: 0,
            session_count: 0,
            wsl_distro: None,
            enabled: false,
        });
    }
}

/// Carry disabled repositories forward through a rebuild.
///
/// The list is re-derived from session evidence, and a disabled repository has
/// none: its rows are purged on opt-out and the scan indexes nothing new. But
/// the Settings list is where the reader turns it back ON — losing the row
/// would make the opt-out irreversible from the UI, so the stored record rides
/// along until it is re-enabled and re-located normally.
fn retain_disabled(records: &mut Vec<RepositoryRecord>, known: Vec<RepositoryRecord>) {
    let listed: HashSet<String> = records.iter().map(|record| record.key.clone()).collect();
    for record in known {
        if !record.enabled && !listed.contains(&record.key) {
            records.push(record);
        }
    }
}

/// The repository list as the settings pane renders it.
pub fn list(store: &Store) -> anyhow::Result<Vec<RepositoryItem>> {
    Ok(store
        .repositories()?
        .into_iter()
        .map(|record| RepositoryItem {
            key: record.key,
            repo_name: record.repo_name,
            full_name: record.full_name,
            status: if record.enabled {
                record.status
            } else {
                // The engine's own vocabulary: a present-but-excluded repository
                // is `disabled`, not "accessible but off".
                "disabled".to_string()
            },
            repo_root: record.repo_root,
            suspected_path: record.suspected_path,
            worktree_count: record.worktree_count,
            session_count: record.session_count,
            wsl_distro: record.wsl_distro,
            enabled: record.enabled,
        })
        .collect())
}

/// Include or ignore one repository.
///
/// The choice is written twice on purpose: the store's flag is the record the
/// settings pane reads back, and the engine's ignored-path store is what the
/// *next scan* enforces, so ignoring a repository also stops its sessions from
/// being collected.
pub async fn set_enabled(store: &Store, key: &str, enabled: bool) -> anyhow::Result<bool> {
    let Some(record) = store
        .repositories()?
        .into_iter()
        .find(|record| record.key == key)
    else {
        return Ok(false);
    };
    if let Some(path) = record
        .repo_root
        .as_deref()
        .or(record.suspected_path.as_deref())
    {
        if enabled {
            ignored_paths::unignore_path(store.state_dir(), IGNORE_SCOPE, path).await?;
        } else {
            ignored_paths::ignore_path(store.state_dir(), IGNORE_SCOPE, path).await?;
            // Excluded means excluded: rows already indexed for this
            // repository leave the store now rather than remaining stale.
            // Re-checked against the full opt-out set as the scan
            // does, so normalization matches exactly — which also sweeps any
            // stragglers from earlier opt-outs.
            purge_ignored_sessions(store)?;
        }
    }
    store.set_repository_enabled(key, enabled)
}

/// Remove every stored session whose working directory the reader has opted
/// out of, using the same containment test the scan gate applies on the way
/// in. Returns how many rows were removed.
pub fn purge_ignored_sessions(store: &Store) -> anyhow::Result<usize> {
    let ignored = ignored_paths::load_ignored(store.state_dir(), IGNORE_SCOPE);
    if ignored.is_empty() {
        return Ok(0);
    }
    let mut removed = 0;
    for session in store.recent_sessions(0, usize::MAX)? {
        let Some(cwd) = session.cwd.as_deref() else {
            continue;
        };
        if ignored_paths::set_contains(&ignored, cwd) && store.delete_session(&session.key)? {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Distinct, non-empty working directories across the considered sessions.
fn distinct_cwds(sessions: &[crate::store::SessionRecord]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut cwds = Vec::new();
    for session in sessions {
        let Some(cwd) = session.cwd.as_deref() else {
            continue;
        };
        if cwd.is_empty() {
            continue;
        }
        if seen.insert(cwd.to_string()) {
            cwds.push(cwd.to_string());
        }
    }
    cwds
}

/// What may be done with a repository root an earlier pass already resolved.
#[derive(Debug, PartialEq, Eq)]
enum KnownRootAccess {
    /// Outside every protected directory. Readable, nothing to ask.
    Open,
    /// Inside a directory the user granted. Read it — and let a denial, if the
    /// grant has since been revoked, be what drops the grant.
    Verify,
    /// Inside a directory the user has *not* granted. Do not touch it: a record
    /// can outlive the grant it was found under, and reading it then is exactly
    /// the unannounced dialog this whole design exists to prevent.
    Blocked(String),
}

/// Decide which of the three applies. Pure, and home-explicit so it behaves the
/// same in a test on any platform.
fn classify_known_root(home: &Path, root: &Path, granted: &HashSet<String>) -> KnownRootAccess {
    match protected::protected_dir_name_in(home, root) {
        None => KnownRootAccess::Open,
        Some(dir) if granted.contains(&dir) => KnownRootAccess::Verify,
        Some(dir) => KnownRootAccess::Blocked(dir),
    }
}

/// Resolve working directories to canonical repository roots, reusing roots the
/// last pass already established.
///
/// `granted` names the consent-protected directories the user has allowed.
/// Working directories under any other protected directory are returned as
/// deferred rather than resolved: this pass runs in the background, and both a
/// directory read and a `git` process whose working directory sits inside one
/// would raise a permission dialog with nothing on screen to explain it.
async fn locate_from_cwds(
    cwds: &[String],
    known: &[RepositoryRecord],
    granted: &HashSet<String>,
    consent: &dyn ConsentGrants,
) -> (Vec<LocatedRepo>, Vec<DeferredProtectedPath>) {
    let mut by_key: HashMap<String, LocatedRepo> = HashMap::new();
    let mut deferred: Vec<DeferredProtectedPath> = Vec::new();
    let mut resolved = 0usize;
    let mut unresolved: Vec<&str> = Vec::new();
    // Without a home directory nothing can be classified as protected, which is
    // how the engine's own guards fail open.
    let home = home_dir();

    for cwd in cwds {
        // Already under a repository the last pass found: answered from the
        // store, so no filesystem access and no consent question to ask.
        if let Some(record) = known.iter().find(|record| {
            record
                .repo_root
                .as_deref()
                .is_some_and(|root| under(cwd, root))
        }) {
            let Some(root) = record.repo_root.as_deref() else {
                continue;
            };
            if by_key.contains_key(&record.key) {
                continue;
            }
            // A protected root is the one case the store cannot answer for:
            // the grant it was found under can be revoked without telling us,
            // and assuming otherwise keeps reporting a repository as readable
            // long after the system stopped allowing it.
            let root_path = PathBuf::from(root);
            let access = match home.as_deref() {
                Some(home) => classify_known_root(home, &root_path, granted),
                None => KnownRootAccess::Open,
            };
            let dir_accessible = match access {
                KnownRootAccess::Open => true,
                KnownRootAccess::Verify => verify_dir_access(consent, &root_path).await,
                KnownRootAccess::Blocked(protected_dir) => {
                    deferred.push(DeferredProtectedPath {
                        cwd: root_path.to_string_lossy().to_string(),
                        protected_dir,
                    });
                    false
                }
            };
            by_key.insert(
                record.key.clone(),
                LocatedRepo {
                    remote_url: String::new(),
                    repo_root: root_path,
                    dir_accessible,
                    worktree_count: record.worktree_count,
                    session_count: 0,
                },
            );
            continue;
        }

        unresolved.push(cwd);
    }

    // Everything past this point shells out to git, so consent is settled first.
    let (admitted, blocked) = match home.as_deref() {
        Some(home) => partition_cwds_by_grants(home, unresolved, granted),
        None => (
            unresolved.into_iter().map(str::to_string).collect(),
            Vec::new(),
        ),
    };
    deferred.extend(blocked);

    for cwd in &admitted {
        if resolved >= MAX_NEW_ROOTS_PER_PASS {
            continue;
        }
        resolved += 1;

        let Ok(root) = git::repo_root_at(Path::new(cwd)).await else {
            continue;
        };
        let root = git::canonical_main_repo_root(&root).await;
        // A worktree outside the protected directories can still be owned by a
        // main repository inside one, so the root gets its own check before the
        // reads below.
        if protected::cwd_resolution_blocked(&root, granted) {
            if let Some(protected_dir) = protected::protected_dir_name(&root) {
                deferred.push(DeferredProtectedPath {
                    cwd: root.to_string_lossy().to_string(),
                    protected_dir,
                });
            }
            continue;
        }
        let key = repo_root_identity(&root);
        if by_key.contains_key(&key) {
            continue;
        }
        let remote = git::preferred_remote_url_at(&root)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        by_key.insert(
            key,
            LocatedRepo {
                remote_url: normalize_remote_url(&remote),
                repo_root: root.clone(),
                dir_accessible: verify_dir_access(consent, &root).await,
                worktree_count: git::worktree_count_at(&root).await,
                session_count: 0,
            },
        );
    }

    dedup_deferred_by_cwd(&mut deferred);
    (by_key.into_values().collect(), deferred)
}

/// Attach the per-repository session count, keeping each root's own sessions.
fn count_sessions(
    located: Vec<LocatedRepo>,
    sessions: &[crate::store::SessionRecord],
) -> Vec<(LocatedRepo, String)> {
    located
        .into_iter()
        .map(|mut repo| {
            let root = repo.repo_root.to_string_lossy().to_string();
            repo.session_count = sessions
                .iter()
                .filter(|session| {
                    session
                        .cwd
                        .as_deref()
                        .is_some_and(|cwd| under(cwd, root.as_str()))
                })
                .count() as u32;
            (repo, root)
        })
        .collect()
}

/// The owners worth running the forward scan for, busiest first.
fn top_owners(located: &[(LocatedRepo, String)]) -> Vec<String> {
    let mut weight: HashMap<String, u32> = HashMap::new();
    for (repo, _) in located {
        let Some(owner) = git::parse_owner_from_url(&repo.remote_url) else {
            continue;
        };
        *weight.entry(owner).or_insert(0) += repo.session_count.max(1);
    }
    let mut owners: Vec<(String, u32)> = weight.into_iter().collect();
    owners.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    owners
        .into_iter()
        .take(MAX_SCAN_OWNERS)
        .map(|(owner, _)| owner)
        .collect()
}

/// The engine's owner-scoped forward walk, over the common code directories and
/// the reader's declared scan roots.
async fn forward_scan(
    owners: &[String],
    scan_roots: &[PathBuf],
    consent: &StoreConsentGrants<'_>,
) -> (Vec<LocatedRepo>, Vec<DeferredProtectedPath>) {
    let Some(home) = home_dir() else {
        return (Vec::new(), Vec::new());
    };
    let mut located = Vec::new();
    let mut deferred = Vec::new();
    for owner in owners {
        // `probe_protected: false` — a background pass must never be able to
        // raise the operating system's consent dialog. Roots the user has
        // already granted are walked normally; the rest come back deferred.
        let (repos, skipped) = scan_roots_for_repos(&home, owner, false, scan_roots, consent).await;
        located.extend(repos);
        deferred.extend(skipped);
    }
    (located, deferred)
}

/// Shape one located repository into the record the store keeps.
async fn to_record(
    repo: &LocatedRepo,
    sessions: &[crate::store::SessionRecord],
    known: &[RepositoryRecord],
    ignored: &HashSet<String>,
) -> RepositoryRecord {
    let root = repo.repo_root.to_string_lossy().to_string();
    let key = repo_root_identity(&repo.repo_root);
    let name = parse_repo_name_from_url(&repo.remote_url).unwrap_or_else(|| {
        repo.repo_root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| root.clone())
    });
    let full_name = match git::parse_owner_from_url(&repo.remote_url) {
        Some(owner) => format!("{owner}/{name}"),
        // A clone with no usable remote is still a real repository on this
        // machine; it just has no owner-qualified name to show.
        None => name.clone(),
    };
    let session_count = if repo.session_count > 0 {
        repo.session_count
    } else {
        sessions
            .iter()
            .filter(|session| {
                session
                    .cwd
                    .as_deref()
                    .is_some_and(|cwd| under(cwd, root.as_str()))
            })
            .count() as u32
    };

    // Selection is the reader's, so a rescan inherits it; a repository seen for
    // the first time starts from the engine's opt-out list.
    let enabled = known
        .iter()
        .find(|record| record.key == key)
        .map(|record| record.enabled)
        .unwrap_or_else(|| !ignored_paths::set_contains(ignored, &root));

    let wsl_distro =
        antiburn_local::platform::environment::environment_from_mounted_path(&repo.repo_root)
            .and_then(|environment| environment.wsl_distro().map(str::to_string));

    RepositoryRecord {
        key,
        repo_name: name,
        full_name,
        status: if repo.dir_accessible {
            "accessible".to_string()
        } else {
            "permission_denied".to_string()
        },
        repo_root: repo.dir_accessible.then(|| root.clone()),
        suspected_path: (!repo.dir_accessible).then_some(root),
        worktree_count: repo.worktree_count,
        session_count,
        wsl_distro,
        enabled,
    }
}

/// True when `path` is `root` or sits beneath it, comparing with `/` on every
/// platform so a Windows working directory still matches its root.
fn under(path: &str, root: &str) -> bool {
    let path = ignored_paths::normalize(path).replace('\\', "/");
    let root = ignored_paths::normalize(root).replace('\\', "/");
    path == root || path.starts_with(&format!("{root}/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn located(remote: &str, root: &str, sessions: u32) -> (LocatedRepo, String) {
        (
            LocatedRepo {
                remote_url: normalize_remote_url(remote),
                repo_root: PathBuf::from(root),
                dir_accessible: true,
                worktree_count: 1,
                session_count: sessions,
            },
            root.to_string(),
        )
    }

    fn repo_record(key: &str, enabled: bool) -> RepositoryRecord {
        RepositoryRecord {
            key: key.to_string(),
            repo_name: key.to_string(),
            full_name: format!("avery/{key}"),
            status: "accessible".to_string(),
            repo_root: Some(format!("/home/avery/code/{key}")),
            suspected_path: None,
            worktree_count: 1,
            session_count: 0,
            wsl_distro: None,
            enabled,
        }
    }

    /// An opted-out path whose record is gone entirely (an earlier build
    /// dropped it) is resurrected as a disabled row from the opt-out store,
    /// so the exclusion is always visible and reversible.
    /// The rule a background pass must never break: a repository record can
    /// outlive the grant it was found under, and reading it then is the
    /// unannounced dialog this design exists to prevent.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_known_root_is_only_read_while_its_grant_still_stands() {
        let home = Path::new("/Users/avery");
        let documents = home.join("Documents/GitHub/repo");
        let plain = home.join("dev/repo");

        // Outside the protected directories the store's answer stands.
        assert_eq!(
            classify_known_root(home, &plain, &HashSet::new()),
            KnownRootAccess::Open
        );

        // Granted: read it, so a revoked grant is observed and dropped.
        assert_eq!(
            classify_known_root(home, &documents, &HashSet::from(["Documents".to_string()])),
            KnownRootAccess::Verify
        );

        // Not granted: never touched, however stale the record.
        assert_eq!(
            classify_known_root(home, &documents, &HashSet::new()),
            KnownRootAccess::Blocked("Documents".to_string())
        );

        // A grant covers one directory, not every protected one.
        assert_eq!(
            classify_known_root(
                home,
                &home.join("Desktop/repo"),
                &HashSet::from(["Documents".to_string()])
            ),
            KnownRootAccess::Blocked("Desktop".to_string())
        );
    }

    #[tokio::test]
    async fn an_orphaned_opt_out_surfaces_as_a_disabled_row() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().join("ingot");
        std::fs::create_dir_all(&repo).unwrap();
        ignored_paths::ignore_path(dir.path(), IGNORE_SCOPE, &repo.to_string_lossy())
            .await
            .unwrap();

        let mut records = vec![repo_record("widgets", true)];
        surface_orphaned_opt_outs(&mut records, dir.path(), &HashSet::new()).await;

        assert_eq!(records.len(), 2);
        let orphan = &records[1];
        assert_eq!(orphan.repo_name, "ingot");
        assert!(!orphan.enabled);
        assert_eq!(orphan.repo_root.as_deref(), Some(&*repo.to_string_lossy()));

        // Idempotent: a second pass sees the row listed and adds nothing.
        let mut again = records.clone();
        surface_orphaned_opt_outs(&mut again, dir.path(), &HashSet::new()).await;
        assert_eq!(again.len(), 2);
    }

    /// A disabled repository has no session evidence left (its rows were
    /// purged), but its Settings row must survive a rebuild — it is the only
    /// place the reader can turn it back on.
    #[test]
    fn a_disabled_repository_survives_a_list_rebuild() {
        let mut records = vec![repo_record("widgets", true)];
        retain_disabled(
            &mut records,
            vec![
                repo_record("widgets", true),   // re-located normally
                repo_record("gadgets", false),  // disabled, no evidence left
                repo_record("sprockets", true), // enabled but gone from disk
            ],
        );

        let keys: Vec<&str> = records.iter().map(|record| record.key.as_str()).collect();
        assert_eq!(keys, ["widgets", "gadgets"]);
        // An enabled repository that stopped being discoverable is genuinely
        // gone and must not be resurrected from stale records.
        assert!(!keys.contains(&"sprockets"));
    }

    /// Disabling a repository removes its already-indexed rows immediately;
    /// sessions from other repositories stay.
    #[tokio::test]
    async fn purging_removes_stored_sessions_under_the_opted_out_root() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = crate::store::Store::open_in_memory(dir.path()).unwrap();

        let session = |id: &str, cwd: &str| crate::store::SessionRecord {
            key: crate::store::SessionKey::new("native", "claude-code", id),
            source_kind: "file".to_string(),
            source_label: format!("file:{id}"),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: Some(cwd.to_string()),
            surface: "cli".to_string(),
            updated_at_epoch: Some(1_800_000_000),
            activity_cursor: String::new(),
            activity_source: "mtime".to_string(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: None,
        };
        store
            .upsert_sessions(
                &[
                    session("in-widgets", "/home/avery/code/widgets"),
                    session("in-widgets-sub", "/home/avery/code/widgets/src"),
                    session("in-gadgets", "/home/avery/code/gadgets"),
                ],
                &[],
            )
            .unwrap();

        ignored_paths::ignore_path(store.state_dir(), IGNORE_SCOPE, "/home/avery/code/widgets")
            .await
            .unwrap();
        let removed = purge_ignored_sessions(&store).unwrap();

        assert_eq!(removed, 2);
        let remaining = store.recent_sessions(0, 10).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].key.session_id, "in-gadgets");
    }

    #[test]
    fn a_working_directory_matches_its_root_and_its_descendants_only() {
        assert!(under(
            "/home/avery/code/widgets",
            "/home/avery/code/widgets"
        ));
        assert!(under(
            "/home/avery/code/widgets/src/api",
            "/home/avery/code/widgets"
        ));
        assert!(under(
            "/home/avery/code/widgets/",
            "/home/avery/code/widgets"
        ));
        // A sibling whose name merely starts the same must not match.
        assert!(!under(
            "/home/avery/code/widgets-legacy",
            "/home/avery/code/widgets"
        ));
        assert!(!under("/home/avery/code", "/home/avery/code/widgets"));
    }

    #[test]
    fn windows_working_directories_match_their_root() {
        assert!(under(r"C:\work\widgets\src", r"C:\work\widgets"));
        assert!(!under(r"C:\work\widgets-legacy", r"C:\work\widgets"));
    }

    #[test]
    fn owners_are_ranked_by_activity_and_capped() {
        let repos = vec![
            located("https://github.com/avery/widgets.git", "/a", 9),
            located("https://github.com/avery/gadgets.git", "/b", 1),
            located("https://github.com/bailey/sprockets.git", "/c", 4),
            located("https://github.com/casey/cogs.git", "/d", 3),
            located("https://github.com/devon/bolts.git", "/e", 2),
            located("https://github.com/emery/nuts.git", "/f", 1),
        ];
        let owners = top_owners(&repos);
        assert_eq!(owners.len(), MAX_SCAN_OWNERS);
        assert_eq!(owners[0], "avery", "10 sessions across two clones");
        assert_eq!(owners[1], "bailey");
    }

    #[test]
    fn a_clone_with_no_remote_contributes_no_owner() {
        let repos = vec![located("", "/a", 3)];
        assert!(top_owners(&repos).is_empty());
    }

    #[test]
    fn session_counts_are_attributed_to_the_root_that_contains_them() {
        let session = |cwd: &str| crate::store::SessionRecord {
            key: crate::store::SessionKey::new("native", "claude-code", cwd),
            source_kind: "file".into(),
            source_label: "/tmp/x.jsonl".into(),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: Some(cwd.to_string()),
            surface: "cli".into(),
            updated_at_epoch: Some(1),
            activity_cursor: String::new(),
            activity_source: "mtime".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: None,
        };
        let sessions = vec![
            session("/home/avery/code/widgets"),
            session("/home/avery/code/widgets/src"),
            session("/home/avery/code/gadgets"),
        ];
        let counted = count_sessions(
            vec![LocatedRepo {
                remote_url: String::new(),
                repo_root: PathBuf::from("/home/avery/code/widgets"),
                dir_accessible: true,
                worktree_count: 1,
                session_count: 0,
            }],
            &sessions,
        );
        assert_eq!(counted[0].0.session_count, 2);
    }
}
