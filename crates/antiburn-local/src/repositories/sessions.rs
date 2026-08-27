//! Locating repositories from the working directories of recent agent sessions.
//!
//! This is the supplementary path to [`scan`](super::scan): it turns "where did
//! the user actually work" into repository roots, which finds clones outside the
//! scanned directories and yields the per-repository session count.
//!
//! The pipeline runs in two bounded stages so a machine with hundreds of
//! working directories stays responsive without letting concurrency reorder
//! anything observable:
//!
//! - **Stage A** resolves each working directory to a canonical repository root.
//!   Git work runs concurrently; the fold that aggregates counts and recovers
//!   from stale consent grants is ordered.
//! - **Stage B** reads side-effect-free git details per distinct root
//!   concurrently, then verifies directory access **serially** — access
//!   verification can drop a stale consent grant, and that state change must not
//!   interleave.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::{FuturesUnordered, StreamExt as _};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::discovery::Explorers;
use crate::paths::protected::protected_dir_name;
use crate::platform::environment::DiscoveryEnvironment;
use crate::platform::git;

use super::access::{verify_cwd_admitted_on_grant, verify_dir_access};
use super::consent::ConsentGrants;
use super::identity::{normalize_remote_url, parse_repo_name_from_url, repo_root_identity};
use super::model::{DeferredProtectedPath, LocalRepoStatus, LocatedRepo, RepoAccessStatus};
use super::platform::{self, PlatformDiscovery as _};
use super::progress::{DiscoveryPhase, DiscoveryProgress, ProgressRepo, ProgressSink};
use super::scan::{SCAN_DEPTH, scan_dir_for_repos};

const CONCURRENCY_FALLBACK: usize = 4;
const CONCURRENCY_MIN: usize = 2;
const CONCURRENCY_MAX: usize = 8;

/// Callback a [`SessionCwdSource`] invokes as each agent finishes, with
/// `(agent, sessions_found, completed, total)`.
///
/// Spelled as an alias with an explicit higher-ranked bound so the agent name
/// borrows for the call only, not for the lifetime of the whole run.
pub type AgentProgress<'a> =
    &'a (dyn for<'agent> Fn(&'agent str, usize, usize, usize) + Send + Sync);

/// Where a discovery run gets the working directories of recent agent sessions.
///
/// The engine ships [`ExplorerCwdSource`], which fans out over an adapter set.
/// Applications that have to prepare something before the fan-out (refresh a
/// mirror, warm a cache) implement this instead and call the adapter set
/// themselves.
#[async_trait]
pub trait SessionCwdSource: Send + Sync {
    /// Per-working-directory count of recent sessions — one increment per
    /// session log — for sessions within `since_secs` of `now`.
    ///
    /// `on_agent_done` is called once per agent with
    /// `(agent, sessions_found, completed, total)`.
    async fn cwd_counts(
        &self,
        now: i64,
        since_secs: i64,
        on_agent_done: AgentProgress<'_>,
    ) -> HashMap<String, u32>;
}

/// A [`SessionCwdSource`] backed directly by an adapter set.
pub struct ExplorerCwdSource(pub Explorers);

#[async_trait]
impl SessionCwdSource for ExplorerCwdSource {
    async fn cwd_counts(
        &self,
        now: i64,
        since_secs: i64,
        on_agent_done: AgentProgress<'_>,
    ) -> HashMap<String, u32> {
        self.0
            .discover_cwd_counts_with_progress(
                now,
                since_secs,
                |agent: &str, found: usize, done: usize, total: usize| {
                    on_agent_done(agent, found, done, total)
                },
            )
            .await
    }
}

/// Resolve how many repository probes may run at once.
///
/// `configured` is an application-supplied override (typically an environment
/// variable), which wins when it parses to a positive number. Otherwise the
/// host's available parallelism is clamped into a range that keeps a laptop
/// responsive without starving a workstation, falling back to a fixed default
/// when parallelism is unknown.
pub fn concurrency_from(configured: Option<&str>, available_parallelism: Option<usize>) -> usize {
    configured
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.min(Semaphore::MAX_PERMITS))
        .unwrap_or_else(|| {
            available_parallelism
                .map(|value| value.clamp(CONCURRENCY_MIN, CONCURRENCY_MAX))
                .unwrap_or(CONCURRENCY_FALLBACK)
        })
}

/// [`concurrency_from`] against the current host, with no application override.
pub fn default_concurrency() -> usize {
    concurrency_from(
        None,
        std::thread::available_parallelism()
            .ok()
            .map(|value| value.get()),
    )
}

/// Result of scanning session working directories.
pub(super) struct SessionDiscoveryResult {
    pub(super) repos: Vec<LocatedRepo>,
    pub(super) deferred: Vec<DeferredProtectedPath>,
    /// Working directories that did not resolve to a repository — typically a
    /// *parent folder above the repositories*. Used to derive parent scan roots
    /// so the child repositories beneath them are discovered.
    pub(super) unresolved_cwds: Vec<String>,
}

/// One task's outcome, tagged with its input position so concurrent completion
/// can be folded back into input order.
#[derive(Debug)]
pub(super) struct IndexedTaskResult<I, O> {
    pub(super) index: usize,
    pub(super) input: I,
    pub(super) outcome: Result<O, String>,
}

/// A repository root reached by at least one working directory, with the stable
/// identity used to dedup worktrees and path spellings onto it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DistinctRepoRoot {
    pub(super) root_key: String,
    pub(super) canonical_root: PathBuf,
}

/// The ordered result of folding Stage A.
#[derive(Debug)]
pub(super) struct StageAFold {
    pub(super) root_session_counts: HashMap<String, u32>,
    pub(super) distinct_roots: Vec<DistinctRepoRoot>,
    pub(super) unresolved_cwds: Vec<String>,
    /// Working directories admitted on a recorded consent grant that then failed
    /// to resolve — the grant is the prime suspect.
    pub(super) failed_granted_cwds: Vec<String>,
}

/// Side-effect-free git details for one repository root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RootGitDetails {
    pub(super) normalized_url: String,
    pub(super) repo_name: String,
    pub(super) repo_owner: String,
    pub(super) worktree_count: u32,
}

/// A root whose details resolved and whose access was verified.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct VerifiedRootDetails {
    pub(super) root: DistinctRepoRoot,
    pub(super) details: RootGitDetails,
    pub(super) dir_accessible: bool,
}

/// Canonical root identity → (normalized remote, root, accessible, worktrees).
pub(super) type SeenRepos = HashMap<String, (String, PathBuf, bool, u32)>;

/// Run independent async operations concurrently while retaining a permit for
/// each operation's complete lifetime. Results are restored to input order.
pub(super) async fn run_indexed_bounded<I, O, F, Fut>(
    inputs: Vec<I>,
    concurrency: usize,
    operation: F,
) -> Vec<IndexedTaskResult<I, O>>
where
    I: Clone + Send + 'static,
    O: Send + 'static,
    F: Fn(I) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = O> + Send + 'static,
{
    run_indexed_bounded_with_progress(inputs, concurrency, operation, |_| {}).await
}

/// [`run_indexed_bounded`] that reports the running completion count.
pub(super) async fn run_indexed_bounded_with_progress<I, O, F, Fut, P>(
    inputs: Vec<I>,
    concurrency: usize,
    operation: F,
    mut on_completion: P,
) -> Vec<IndexedTaskResult<I, O>>
where
    I: Clone + Send + 'static,
    O: Send + 'static,
    F: Fn(I) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = O> + Send + 'static,
    P: FnMut(usize),
{
    let permit_count = concurrency
        .clamp(1, Semaphore::MAX_PERMITS)
        .min(inputs.len().max(1));
    let semaphore = Arc::new(Semaphore::new(permit_count));
    let mut tasks = FuturesUnordered::new();

    for (index, input) in inputs.into_iter().enumerate() {
        let task_input = input.clone();
        let task_semaphore = Arc::clone(&semaphore);
        let task_operation = operation.clone();
        let handle = tokio::spawn(async move {
            let _permit: OwnedSemaphorePermit = task_semaphore
                .acquire_owned()
                .await
                .expect("repository discovery semaphore cannot close while tasks retain it");
            task_operation(task_input).await
        });
        tasks.push(async move {
            IndexedTaskResult {
                index,
                input,
                outcome: handle.await.map_err(|error| error.to_string()),
            }
        });
    }

    let mut results = Vec::with_capacity(tasks.len());
    while let Some(result) = tasks.next().await {
        results.push(result);
        on_completion(results.len());
    }
    results.sort_by_key(|result| result.index);
    results
}

/// Stage A: resolve every working directory to its canonical repository root.
pub(super) async fn resolve_cwd_roots_bounded_with_progress<P>(
    cwds: Vec<String>,
    concurrency: usize,
    granted_dirs: HashSet<String>,
    on_completion: P,
) -> Vec<IndexedTaskResult<String, Option<PathBuf>>>
where
    P: FnMut(usize),
{
    let granted = Arc::new(granted_dirs);
    run_indexed_bounded_with_progress(
        cwds,
        concurrency,
        move |cwd| {
            let granted = Arc::clone(&granted);
            async move {
                match git::resolve_repo_root_with_fallbacks(Path::new(&cwd), &granted).await {
                    Ok(resolution) => {
                        Some(git::canonical_main_repo_root(&resolution.repo_root).await)
                    }
                    Err(_) => None,
                }
            }
        },
        on_completion,
    )
    .await
}

/// [`resolve_cwd_roots_bounded_with_progress`] without progress reporting.
#[cfg(test)]
pub(super) async fn resolve_cwd_roots_bounded(
    cwds: Vec<String>,
    concurrency: usize,
) -> Vec<IndexedTaskResult<String, Option<PathBuf>>> {
    resolve_cwd_roots_bounded_with_progress(cwds, concurrency, HashSet::new(), |_| {}).await
}

/// Run `operation` over distinct roots with bounded concurrency, in input order.
pub(super) async fn resolve_distinct_roots_bounded_with<O, F, Fut>(
    roots: Vec<DistinctRepoRoot>,
    concurrency: usize,
    operation: F,
) -> Vec<IndexedTaskResult<DistinctRepoRoot, O>>
where
    O: Send + 'static,
    F: Fn(DistinctRepoRoot) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = O> + Send + 'static,
{
    run_indexed_bounded(roots, concurrency, operation).await
}

/// Stage B: read the git details of each distinct root. Deliberately
/// side-effect free, so it is safe to run concurrently.
pub(super) async fn resolve_root_details_bounded(
    roots: Vec<DistinctRepoRoot>,
    concurrency: usize,
) -> Vec<IndexedTaskResult<DistinctRepoRoot, Option<RootGitDetails>>> {
    resolve_distinct_roots_bounded_with(roots, concurrency, |root| async move {
        let url = match git::preferred_remote_url_at(&root.canonical_root).await {
            Ok(Some(url)) => url,
            Ok(None) | Err(_) => return None,
        };
        let repo_owner = git::parse_owner_from_url(&url).unwrap_or_default();
        let repo_name = parse_repo_name_from_url(&url).unwrap_or_else(|| {
            root.canonical_root
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default()
        });
        let worktree_count = git::worktree_count_at(&root.canonical_root).await;

        Some(RootGitDetails {
            normalized_url: normalize_remote_url(&url),
            repo_name,
            repo_owner,
            worktree_count,
        })
    })
    .await
}

/// Verify directory access one root at a time, in input order.
///
/// Access verification can drop a stale consent grant, so it stays out of the
/// concurrent stage: two roots under the same dead grant must observe the
/// eviction in a defined order.
pub(super) async fn verify_root_details_serially<F, Fut>(
    mut results: Vec<IndexedTaskResult<DistinctRepoRoot, Option<RootGitDetails>>>,
    verify_access: F,
) -> Vec<VerifiedRootDetails>
where
    F: Fn(PathBuf) -> Fut,
    Fut: Future<Output = bool>,
{
    results.sort_by_key(|result| result.index);
    let mut verified = Vec::with_capacity(results.len());
    for result in results {
        let Ok(Some(details)) = result.outcome else {
            continue;
        };
        let dir_accessible = verify_access(result.input.canonical_root.clone()).await;
        verified.push(VerifiedRootDetails {
            root: result.input,
            details,
            dir_accessible,
        });
    }
    verified
}

/// Fold Stage A into distinct roots and per-root session counts, in input order.
pub(super) fn fold_cwd_resolutions(
    mut resolutions: Vec<IndexedTaskResult<String, Option<PathBuf>>>,
    cwd_counts: &HashMap<String, u32>,
    admitted_cwds: &HashSet<String>,
) -> StageAFold {
    resolutions.sort_by_key(|result| result.index);

    let mut root_session_counts = HashMap::new();
    let mut distinct_roots = Vec::new();
    let mut seen_root_keys = HashSet::new();
    let mut unresolved_cwds = Vec::new();
    let mut failed_granted_cwds = Vec::new();

    for result in resolutions {
        let cwd = result.input;
        let canonical_root = match result.outcome {
            Ok(Some(root)) => root,
            Ok(None) | Err(_) => {
                if admitted_cwds.contains(&cwd) {
                    failed_granted_cwds.push(cwd.clone());
                }
                unresolved_cwds.push(cwd);
                continue;
            }
        };

        let root_key = repo_root_identity(&canonical_root);
        *root_session_counts.entry(root_key.clone()).or_insert(0) +=
            cwd_counts.get(&cwd).copied().unwrap_or(0);
        if seen_root_keys.insert(root_key.clone()) {
            distinct_roots.push(DistinctRepoRoot {
                root_key,
                canonical_root,
            });
        }
    }

    StageAFold {
        root_session_counts,
        distinct_roots,
        unresolved_cwds,
        failed_granted_cwds,
    }
}

/// Re-check every working directory admitted on a recorded grant that then
/// failed to resolve, dropping the grant when the read really is denied.
pub(super) async fn verify_failed_granted_cwds(
    consent: &dyn ConsentGrants,
    failed_granted_cwds: &[String],
    admitted_cwds: &HashSet<String>,
) {
    for cwd in failed_granted_cwds {
        verify_cwd_admitted_on_grant(consent, cwd, admitted_cwds).await;
    }
}

fn log_cwd_resolution_failures(resolutions: &[IndexedTaskResult<String, Option<PathBuf>>]) {
    for result in resolutions {
        if let Err(error) = &result.outcome {
            ::tracing::warn!(
                event = "repo_discovery_resolve_task_failed",
                cwd = result.input.as_str(),
                error,
            );
        }
    }
}

fn log_root_detail_failures(
    details: &[IndexedTaskResult<DistinctRepoRoot, Option<RootGitDetails>>],
) {
    for result in details {
        if let Err(error) = &result.outcome {
            ::tracing::warn!(
                event = "repo_discovery_root_detail_task_failed",
                root = result.input.canonical_root.to_string_lossy().as_ref(),
                error,
            );
        }
    }
}

/// Turn the deduped root map into located repositories, attaching each root's
/// summed session count.
pub(super) fn located_repos_from_seen(
    seen_repos: SeenRepos,
    root_session_counts: &HashMap<String, u32>,
) -> Vec<LocatedRepo> {
    seen_repos
        .into_iter()
        .map(
            |(root_key, (remote_url, repo_root, dir_accessible, worktree_count))| LocatedRepo {
                remote_url,
                repo_root,
                dir_accessible,
                worktree_count,
                session_count: root_session_counts.get(&root_key).copied().unwrap_or(0),
            },
        )
        .collect()
}

/// Locate repositories from the working directories of recent agent sessions.
#[allow(clippy::too_many_arguments)]
pub(super) async fn discover_repos_from_sessions(
    owner: &str,
    since_secs: i64,
    probe_protected: bool,
    concurrency: usize,
    sessions: &dyn SessionCwdSource,
    consent: &dyn ConsentGrants,
    progress: &dyn ProgressSink,
) -> SessionDiscoveryResult {
    let t0 = std::time::Instant::now();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let owner_lower = owner.to_ascii_lowercase();

    let report = |phase: DiscoveryPhase,
                  scanning: bool,
                  repos: &[ProgressRepo],
                  protected_dirs: &[String],
                  resolved: Option<usize>,
                  total_paths: Option<usize>| {
        progress.report(DiscoveryProgress {
            phase,
            scanning,
            repos: repos.to_vec(),
            protected_dirs: protected_dirs.to_vec(),
            resolved,
            total_paths,
        });
    };

    report(
        DiscoveryPhase::SearchingSessions,
        true,
        &[],
        &[],
        Some(0),
        None,
    );

    // Per-working-directory session counts (one increment per session log) so
    // each repository can report how many sessions ran in it. The map keys
    // double as the unique working-directory set resolved below.
    let cwd_counts = sessions
        .cwd_counts(now, since_secs, &|agent, _found, completed, total| {
            report(
                DiscoveryPhase::AgentScanned {
                    agent: agent.to_string(),
                },
                true,
                &[],
                &[],
                Some(completed),
                Some(total),
            );
        })
        .await;

    ::tracing::info!(
        event = "repo_discovery_cwds_discovered",
        unique_cwds = cwd_counts.len(),
        window_days = since_secs / 86400,
        elapsed_ms = t0.elapsed().as_millis() as u64,
    );

    // Partition working directories: skip consent-protected paths so the native
    // permission dialog is never raised mid-scan. They are returned as
    // `deferred` so the caller can ask the user first.
    //
    // The recorded grant set is trusted as-is and never probed here. This runs
    // on background paths too, where a probe after an external consent reset
    // (state: *no decision*) would raise the dialog at a random moment. A stale
    // grant is instead dropped lazily by `verify_dir_access` once a read under
    // it actually comes back denied.
    report(DiscoveryPhase::CheckingConsent, true, &[], &[], None, None);
    let plat = platform::platform();
    let granted_names = consent.granted_dirs();
    let accessible_dirs: HashSet<PathBuf> = if let Some(home) = crate::paths::home_dir() {
        granted_names.iter().map(|name| home.join(name)).collect()
    } else {
        HashSet::new()
    };

    // Sort for deterministic order.
    let mut all_cwds: Vec<String> = cwd_counts.keys().cloned().collect();
    all_cwds.sort();
    let mut cwds: Vec<String> = Vec::new();
    let mut deferred: Vec<DeferredProtectedPath> = Vec::new();
    // Protected working directories let through on the strength of a recorded
    // grant. If one of these fails to resolve below, the grant is the prime
    // suspect and gets verified (see the resolution-failure branch).
    let mut admitted_cwds: HashSet<String> = HashSet::new();

    for cwd in all_cwds {
        if plat.is_access_protected(Path::new(&cwd)) {
            let already_granted = accessible_dirs
                .iter()
                .any(|dir| Path::new(&cwd).starts_with(dir));
            if already_granted {
                admitted_cwds.insert(cwd.clone());
                cwds.push(cwd);
            } else if let Some(dir_name) = protected_dir_name(Path::new(&cwd)) {
                ::tracing::debug!(
                    event = "repo_discovery_cwd_deferred",
                    cwd = cwd.as_str(),
                    protected_dir = dir_name.as_str(),
                );
                deferred.push(DeferredProtectedPath {
                    cwd,
                    protected_dir: dir_name,
                });
            } else {
                cwds.push(cwd);
            }
        } else {
            cwds.push(cwd);
        }
    }

    // When the caller asked for it (an explicit user action), re-probe deferred
    // directories to pick up grants made outside the application.
    if probe_protected {
        let deferred_names: HashSet<String> = deferred
            .iter()
            .map(|entry| entry.protected_dir.clone())
            .collect();
        let newly_granted = consent.discover_external_grants(&deferred_names).await;
        if !newly_granted.is_empty() {
            let (promoted, remaining): (Vec<_>, Vec<_>) = deferred
                .into_iter()
                .partition(|entry| newly_granted.contains(&entry.protected_dir));
            cwds.extend(promoted.into_iter().map(|entry| entry.cwd));
            deferred = remaining;
        }
    }
    cwds.sort();

    // Re-read the grants for Stage A: the probe above may have just recorded
    // one, and a promoted working directory must be resolvable with it. Within
    // Stage A nothing mutates the record — eviction only happens in the ordered
    // folds afterwards — so one snapshot is enough for the whole stage.
    let granted_for_resolution = consent.granted_dirs();

    let mut seen_repos = SeenRepos::new();
    let mut progress_repos: Vec<ProgressRepo> = Vec::new();

    // Deduplicate protected directory names for the progress snapshot.
    let protected_dir_names: Vec<String> = {
        let mut names: Vec<String> = deferred
            .iter()
            .map(|entry| entry.protected_dir.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        names.sort();
        names
    };

    let total_paths = cwds.len();
    report(
        DiscoveryPhase::ResolvingRoots,
        true,
        &[],
        &protected_dir_names,
        Some(0),
        Some(total_paths),
    );

    ::tracing::debug!(
        event = "repo_discovery_resolving_cwds",
        total = total_paths,
        concurrency,
    );
    for (index, cwd) in cwds.iter().enumerate() {
        ::tracing::debug!(
            event = "repo_discovery_resolving_cwd",
            cwd = cwd.as_str(),
            index,
            total = total_paths,
        );
    }

    // Stage A: resolve each sorted working directory to its canonical
    // repository root. Git work runs concurrently, but the stateful permission
    // recovery and result aggregation below remain an ordered fold.
    let cwd_resolutions = resolve_cwd_roots_bounded_with_progress(
        cwds,
        concurrency,
        granted_for_resolution,
        |completed| {
            report(
                DiscoveryPhase::ResolvingRoots,
                true,
                &progress_repos,
                &protected_dir_names,
                Some(completed),
                Some(total_paths),
            );
        },
    )
    .await;
    log_cwd_resolution_failures(&cwd_resolutions);

    let stage_a = fold_cwd_resolutions(cwd_resolutions, &cwd_counts, &admitted_cwds);
    for cwd in &stage_a.unresolved_cwds {
        ::tracing::debug!(event = "repo_discovery_cwd_no_repo", cwd = cwd.as_str());
    }
    verify_failed_granted_cwds(consent, &stage_a.failed_granted_cwds, &admitted_cwds).await;

    // Stage B: resolve side-effect-free per-root git details concurrently.
    // Access verification stays out of the tasks because it may drop a stale
    // consent grant; that stateful fold runs serially below.
    report(
        DiscoveryPhase::ReadingRepoDetails,
        true,
        &progress_repos,
        &protected_dir_names,
        None,
        None,
    );
    ::tracing::debug!(
        event = "repo_discovery_resolving_root_details",
        total = stage_a.distinct_roots.len(),
        concurrency,
    );
    let root_details = resolve_root_details_bounded(stage_a.distinct_roots, concurrency).await;
    log_root_detail_failures(&root_details);
    let verified_roots = verify_root_details_serially(root_details, |root| async move {
        verify_dir_access(consent, &root).await
    })
    .await;

    for verified in verified_roots {
        let RootGitDetails {
            normalized_url,
            repo_name,
            repo_owner,
            worktree_count,
        } = verified.details;
        let root = verified.root;
        let dir_accessible = verified.dir_accessible;
        let matches_owner = repo_owner.to_ascii_lowercase() == owner_lower;
        ::tracing::info!(
            event = "repo_discovery_found_repo",
            repo = repo_name.as_str(),
            owner = repo_owner.as_str(),
            matches_owner,
            root = root.canonical_root.to_string_lossy().as_ref(),
        );

        if !dir_accessible {
            ::tracing::warn!(
                event = "repo_discovery_access_false_positive",
                repo = repo_name.as_str(),
                root = root.canonical_root.to_string_lossy().as_ref(),
                "metadata() succeeded but read_dir() blocked -- likely an OS consent control",
            );
        }

        if matches_owner {
            progress_repos.push(ProgressRepo {
                repo_name: repo_name.clone(),
                repo_root: root.canonical_root.to_string_lossy().to_string(),
            });
            progress_repos.sort_by(|a, b| a.repo_name.cmp(&b.repo_name));
            report(
                DiscoveryPhase::RepoFound {
                    repo_name: repo_name.clone(),
                },
                true,
                &progress_repos,
                &protected_dir_names,
                None,
                None,
            );
        }

        seen_repos.insert(
            root.root_key,
            (
                normalized_url,
                root.canonical_root,
                dir_accessible,
                worktree_count,
            ),
        );
    }

    report(
        DiscoveryPhase::Done,
        false,
        &progress_repos,
        &protected_dir_names,
        Some(total_paths),
        Some(total_paths),
    );

    ::tracing::info!(
        event = "repo_discovery_session_repos_done",
        total_repos = seen_repos.len(),
        owner_matching = progress_repos.len(),
        elapsed_ms = t0.elapsed().as_millis() as u64,
    );

    let repos = located_repos_from_seen(seen_repos, &stage_a.root_session_counts);

    SessionDiscoveryResult {
        repos,
        deferred,
        unresolved_cwds: stage_a.unresolved_cwds,
    }
}

/// Resolve paths the user has just granted access to into repositories.
///
/// Each path is first resolved UP to its enclosing repository (a protected
/// *session working directory*); a path that is not itself a repository — a
/// protected *parent scan root* the user just granted — is scanned DOWN for the
/// repositories inside it.
pub async fn resolve_granted_repos(
    cwds: Vec<String>,
    owner: &str,
    consent: &dyn ConsentGrants,
) -> Vec<LocalRepoStatus> {
    let owner_lower = owner.to_ascii_lowercase();
    let mut results = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    // Canonical root → count of supplied working directories resolving to it
    // (approximate session count; the next full run reconciles the real number).
    let mut root_cwd_counts: HashMap<String, u32> = HashMap::new();
    // Granted paths that are not a repository themselves (protected parent scan
    // roots); scanned downward for child repositories below.
    let mut scan_down_dirs: Vec<PathBuf> = Vec::new();

    for cwd in &cwds {
        let granted = consent.granted_dirs();
        let repo_root = match git::resolve_repo_root_with_fallbacks(Path::new(cwd), &granted).await
        {
            Ok(resolution) => resolution.repo_root,
            Err(_) => {
                scan_down_dirs.push(PathBuf::from(cwd));
                continue;
            }
        };

        let canonical_root = git::canonical_main_repo_root(&repo_root).await;

        let url = match git::preferred_remote_url_at(&canonical_root).await {
            Ok(Some(url)) => url,
            _ => continue,
        };

        let repo_owner = git::parse_owner_from_url(&url).unwrap_or_default();
        if repo_owner.to_ascii_lowercase() != owner_lower {
            continue;
        }

        let repo_name = parse_repo_name_from_url(&url).unwrap_or_else(|| {
            canonical_root
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default()
        });

        let root_key = repo_root_identity(&canonical_root);
        *root_cwd_counts.entry(root_key.clone()).or_insert(0) += 1;

        // Dedup by canonical root so worktrees collapse but different clones of
        // the same repository are reported separately.
        if seen.contains(&root_key) {
            continue;
        }
        seen.insert(root_key);
        let full_name = format!("{repo_owner}/{repo_name}");
        let root_str = canonical_root.to_string_lossy().to_string();
        let worktree_count = git::worktree_count_at(&canonical_root).await;

        // Resolved without a known-repository list. Opt-out: enabled by default.
        if verify_dir_access(consent, &canonical_root).await {
            ::tracing::info!(
                event = "deferred_repo_accessible",
                repo = repo_name.as_str(),
                root = root_str.as_str(),
            );
            results.push(LocalRepoStatus {
                environment: DiscoveryEnvironment::from_mounted_path(&canonical_root),
                repo_name,
                full_name,
                status: RepoAccessStatus::Accessible,
                repo_root: Some(root_str),
                suspected_path: None,
                worktree_count,
                session_count: 0,
                enabled: true,
            });
        } else {
            ::tracing::warn!(
                event = "deferred_repo_still_denied",
                repo = repo_name.as_str(),
                root = root_str.as_str(),
            );
            results.push(LocalRepoStatus {
                environment: DiscoveryEnvironment::from_mounted_path(&canonical_root),
                repo_name,
                full_name,
                status: RepoAccessStatus::PermissionDenied,
                repo_root: None,
                suspected_path: Some(root_str),
                worktree_count,
                session_count: 0,
                enabled: true,
            });
        }
    }

    // Backfill the approximate session count onto each session-resolved
    // repository (>= 1, since each came from a session working directory). Run
    // before the scanned-down repositories are appended so their real (often
    // zero) counts are not clobbered to 1.
    for status in &mut results {
        let key = status
            .repo_root
            .clone()
            .or_else(|| status.suspected_path.clone());
        if let Some(key) = key {
            status.session_count = root_cwd_counts
                .get(&repo_root_identity(Path::new(&key)))
                .copied()
                .unwrap_or(1);
        }
    }

    // Scan granted parent directories downward for child repositories — a grant
    // on a protected common-directory scan root resolves to no repository
    // itself. `scan_dir_for_repos` already filters by owner and dedups by
    // canonical root; `seen` drops anything a working directory already
    // surfaced above.
    if !scan_down_dirs.is_empty() {
        let mut by_root: HashMap<String, LocatedRepo> = HashMap::new();
        for dir in &scan_down_dirs {
            scan_dir_for_repos(dir, &owner_lower, SCAN_DEPTH, consent, &mut by_root).await;
        }
        for located in by_root.into_values() {
            // The identity key deduplicates; it is not a path. On Windows it
            // folds case and separators, so the reported root/suspected path
            // below carries the canonical path instead — matching the
            // session-resolved arm above and `LocalRepoStatus`'s contract.
            let root_key = repo_root_identity(&located.repo_root);
            if !seen.insert(root_key) {
                continue;
            }
            let root_str = located.repo_root.to_string_lossy().to_string();
            let repo_name = parse_repo_name_from_url(&located.remote_url).unwrap_or_else(|| {
                located
                    .repo_root
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default()
            });
            let repo_owner = git::parse_owner_from_url(&located.remote_url).unwrap_or_default();
            let full_name = format!("{repo_owner}/{repo_name}");
            if located.dir_accessible {
                results.push(LocalRepoStatus {
                    environment: DiscoveryEnvironment::from_mounted_path(&located.repo_root),
                    repo_name,
                    full_name,
                    status: RepoAccessStatus::Accessible,
                    repo_root: Some(root_str),
                    suspected_path: None,
                    worktree_count: located.worktree_count,
                    session_count: located.session_count,
                    // Scanned without a known-repository list; opt-out → enabled
                    // by default, same as above.
                    enabled: true,
                });
            } else {
                results.push(LocalRepoStatus {
                    environment: DiscoveryEnvironment::from_mounted_path(&located.repo_root),
                    repo_name,
                    full_name,
                    status: RepoAccessStatus::PermissionDenied,
                    repo_root: None,
                    suspected_path: Some(root_str),
                    worktree_count: located.worktree_count,
                    session_count: located.session_count,
                    enabled: true,
                });
            }
        }
    }

    results
}
