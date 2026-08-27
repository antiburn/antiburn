//! What repository discovery consumes and produces.
//!
//! Every type here describes a repository as it exists *on this machine*, or a
//! repository the embedding application already knows about and wants matched
//! against the machine. Nothing here carries an authentication, hosting, or
//! upload contract: [`RepositoryDescriptor`] is a plain name/URL record the
//! caller fills in from whatever source it has.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::platform::environment::{
    DiscoveryEnvironment, deserialize_wsl_distro, serialize_wsl_distro,
};

/// How a discovery run was performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryMode {
    /// A known-repository list was supplied; full gap analysis performed.
    Full,
    /// No known-repository list was supplied; only what is on disk was
    /// reported.
    SessionsOnly,
}

/// Access status for a single repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoAccessStatus {
    /// Found on disk and readable.
    Accessible,
    /// Found on disk, but the operating system blocked reading it.
    PermissionDenied,
    /// Known to the caller, but not present on this machine.
    NotCloned,
    /// Present, but the embedding application has excluded it.
    Disabled,
}

/// Serde default for [`LocalRepoStatus::enabled`]: selection is opt-out, so a
/// repository is enabled unless explicitly disabled. A record written without
/// the field therefore deserializes to enabled.
fn default_enabled() -> bool {
    true
}

/// Status of a single repository on the local machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRepoStatus {
    /// Execution environment the repository lives in, serialized as only the
    /// optional `wslDistro`; the default keeps legacy records native.
    #[serde(
        rename = "wslDistro",
        default,
        serialize_with = "serialize_wsl_distro",
        deserialize_with = "deserialize_wsl_distro"
    )]
    pub environment: DiscoveryEnvironment,
    /// Repository name without its owner (for example `widgets`).
    pub repo_name: String,
    /// Owner-qualified repository name (for example `avery/widgets`).
    pub full_name: String,
    /// Whether the repository is present and readable.
    pub status: RepoAccessStatus,
    /// Canonical main repository root, when it is readable.
    pub repo_root: Option<String>,
    /// Where the repository appears to live, when it is present but blocked.
    pub suspected_path: Option<String>,
    /// Linked worktrees attached to the main root.
    pub worktree_count: u32,
    /// Number of recent AI coding sessions whose working directory resolved to
    /// this repository (summed across agents, within the discovery window).
    /// `0` means the repository was found on disk but has no AI activity yet.
    #[serde(default)]
    pub session_count: u32,
    /// Whether this repository is selected by the embedding application.
    /// Selection is **opt-out**: a repository is enabled unless the caller
    /// explicitly disabled it (see [`RepositoryDescriptor::enabled`]).
    /// Repositories resolved without a known-repository list default to `true`
    /// (see [`default_enabled`]) so a caller that could not supply one does not
    /// silently disable everything.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

/// A directory that was skipped because it lives under an OS-protected
/// directory (for example macOS' consent-controlled `~/Documents`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredProtectedPath {
    /// The original path that could not be read.
    pub cwd: String,
    /// Human-readable name of the protected parent directory (for example
    /// `"Documents"`).
    pub protected_dir: String,
}

/// Complete result of a discovery run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoDiscoveryResult {
    /// Whether a known-repository list was available for this run.
    pub discovery_mode: DiscoveryMode,
    /// Every repository the run has something to say about.
    pub repos: Vec<LocalRepoStatus>,
    /// Paths under OS-protected directories that could not be scanned. When a
    /// known-repository list is available, only paths whose folder name matches
    /// a known repository are kept (unrelated local projects are dropped). When
    /// it is not, the list is left unfiltered so legitimate repositories are not
    /// hidden; a post-consent run filters once access is granted.
    #[serde(default)]
    pub deferred_protected: Vec<DeferredProtectedPath>,
}

/// A repository the caller already knows about, to be matched against what
/// discovery finds on disk.
///
/// This is deliberately a plain record: the engine never fetches it and reads
/// nothing but the fields below. Callers map whatever listing they have — a
/// hosting provider's API, a config file, a hard-coded set — onto this shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryDescriptor {
    /// Repository name without its owner (for example `widgets`).
    pub name: String,
    /// Owner-qualified repository name (for example `avery/widgets`).
    pub full_name: String,
    /// HTTPS clone URL, used to match a local clone by its git remote.
    pub clone_url: String,
    /// SSH clone URL, used to match a local clone by its git remote.
    pub ssh_url: String,
    /// Whether the caller has selected this repository, as a tri-state:
    /// `Some(true)` selected, `Some(false)` explicitly excluded, and `None`
    /// "the caller expressed no opinion". `None` is treated as selected, so a
    /// caller whose source predates the notion of selection does not disable
    /// every repository at once.
    pub enabled: Option<bool>,
}

impl RepositoryDescriptor {
    /// Whether this repository should be treated as selected, collapsing the
    /// tri-state [`enabled`](Self::enabled) with the opt-out default.
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
}

/// A repository located on disk — from session working directories and/or a
/// directory scan. `remote_url` is normalized for matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatedRepo {
    /// Normalized git remote URL (see
    /// [`normalize_remote_url`](super::normalize_remote_url)).
    pub remote_url: String,
    /// Canonical main repository root.
    pub repo_root: PathBuf,
    /// True when `read_dir()` on the repository root succeeds — not merely
    /// `stat()`, which some access-control systems allow on blocked paths.
    pub dir_accessible: bool,
    /// Linked worktrees attached to the main root.
    pub worktree_count: u32,
    /// Recent AI sessions whose working directory resolved to this repository.
    /// `0` for repositories found only by the filesystem scan.
    pub session_count: u32,
}
