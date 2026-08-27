//! Incremental progress reporting for a discovery run.
//!
//! A run can take tens of seconds on a machine with many clones, so the engine
//! reports what it is doing as it goes. It does that through a plain callback
//! seam: the engine describes *what happened* ([`DiscoveryPhase`]) and *what it
//! knows so far* ([`DiscoveryProgress`]), and the embedding application decides
//! how — and whether — to surface it.
//!
//! The engine deliberately emits no user-facing prose. Phases are data, so the
//! application owns wording, localization, and transport (an event bus, a
//! channel, a progress bar, or nothing at all).

/// A repository located so far, as reported to a [`ProgressSink`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressRepo {
    /// Repository name without its owner.
    pub repo_name: String,
    /// Canonical main repository root.
    pub repo_root: String,
}

/// What the discovery run just did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryPhase {
    /// The run started and is asking agents for recent session working
    /// directories.
    SearchingSessions,
    /// One agent finished reporting its sessions.
    AgentScanned {
        /// Display name of the agent that finished.
        agent: String,
    },
    /// Partitioning paths by whether the operating system needs consent first.
    CheckingConsent,
    /// Resolving session working directories to repository roots.
    ResolvingRoots,
    /// Reading per-root git details (remote, worktrees).
    ReadingRepoDetails,
    /// A repository belonging to the requested owner was located.
    RepoFound {
        /// Name of the repository that was just located.
        repo_name: String,
    },
    /// The run finished.
    Done,
}

/// A progress snapshot: what just happened, plus everything known so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryProgress {
    /// What the run just did.
    pub phase: DiscoveryPhase,
    /// Whether the run is still working. `false` only for
    /// [`DiscoveryPhase::Done`].
    pub scanning: bool,
    /// Repositories located so far, sorted by name.
    pub repos: Vec<ProgressRepo>,
    /// Names of consent-protected directories the run had to skip.
    pub protected_dirs: Vec<String>,
    /// Working directories resolved so far. `None` outside the resolve phase.
    pub resolved: Option<usize>,
    /// Total working directories to resolve. `None` outside the resolve phase.
    pub total_paths: Option<usize>,
}

/// Where a discovery run sends its progress.
pub trait ProgressSink: Send + Sync {
    /// Receive one progress snapshot. Implementations must not block.
    fn report(&self, progress: DiscoveryProgress);
}

/// A [`ProgressSink`] that discards everything. Use it for background runs that
/// nobody is watching.
pub struct NoProgress;

impl ProgressSink for NoProgress {
    fn report(&self, _progress: DiscoveryProgress) {}
}

/// A [`ProgressSink`] built from a closure.
pub struct FnProgress<F>(pub F);

impl<F> ProgressSink for FnProgress<F>
where
    F: Fn(DiscoveryProgress) + Send + Sync,
{
    fn report(&self, progress: DiscoveryProgress) {
        (self.0)(progress)
    }
}
