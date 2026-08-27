//! The whole run, over real repositories on disk, driven through the three
//! seams: session working directories in, progress out, consent consulted.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use serial_test::serial;

use super::support::{EnvGuard, FakeConsent, init_repo};
use crate::repositories::{
    AgentProgress, DiscoveryMode, DiscoveryPhase, DiscoveryProgress, DiscoveryRequest, FnProgress,
    NoProgress, RepoAccessStatus, RepositoryDescriptor, SessionCwdSource, discover_repositories,
};

/// A [`SessionCwdSource`] that replays a fixed set of working directories, so
/// the pipeline can be exercised without any agent on the machine.
struct FakeSessions {
    counts: HashMap<String, u32>,
    agents: Vec<&'static str>,
}

impl FakeSessions {
    fn new(counts: &[(&Path, u32)]) -> Self {
        Self {
            counts: counts
                .iter()
                .map(|(path, count)| (path.to_string_lossy().to_string(), *count))
                .collect(),
            agents: vec!["Agent One", "Agent Two"],
        }
    }
}

#[async_trait]
impl SessionCwdSource for FakeSessions {
    async fn cwd_counts(
        &self,
        _now: i64,
        _since_secs: i64,
        on_agent_done: AgentProgress<'_>,
    ) -> HashMap<String, u32> {
        let total = self.agents.len();
        for (index, agent) in self.agents.iter().enumerate() {
            on_agent_done(agent, 1, index + 1, total);
        }
        self.counts.clone()
    }
}

fn descriptor(name: &str, owner: &str, enabled: Option<bool>) -> RepositoryDescriptor {
    RepositoryDescriptor {
        full_name: format!("{owner}/{name}"),
        name: name.to_string(),
        clone_url: format!("https://github.com/{owner}/{name}.git"),
        ssh_url: format!("git@github.com:{owner}/{name}.git"),
        enabled,
    }
}

fn request<'a>(
    owner: &'a str,
    known: Option<Vec<RepositoryDescriptor>>,
    state_dir: &'a Path,
) -> DiscoveryRequest<'a> {
    DiscoveryRequest {
        owner,
        known_repos: known,
        since_secs: 90 * 24 * 60 * 60,
        probe_protected: false,
        state_dir,
        concurrency: Some(2),
    }
}

/// A session working directory inside a clone resolves to that clone, the clone
/// matches the caller's known repository by remote, and the caller's selection
/// flags come through — including the "not on this machine" case.
#[tokio::test]
#[serial]
async fn full_run_matches_known_repos_and_carries_selection() {
    let home = tempfile::TempDir::new().unwrap();
    let _home = EnvGuard::set("HOME", home.path());
    let state_dir = home.path().join("state");
    tokio::fs::create_dir_all(&state_dir).await.unwrap();

    let widgets = home.path().join("dev").join("widgets");
    init_repo(&widgets, "git@github.com:acme/widgets.git").await;
    let session_cwd = widgets.join("src");
    tokio::fs::create_dir_all(&session_cwd).await.unwrap();

    let sessions = FakeSessions::new(&[(&session_cwd, 4)]);
    let consent = FakeConsent::empty(home.path());
    let known = vec![
        descriptor("widgets", "acme", Some(true)),
        descriptor("toolkit", "acme", Some(false)),
        descriptor("sprockets", "acme", None),
    ];

    let result = discover_repositories(
        request("acme", Some(known), &state_dir),
        &sessions,
        &consent,
        &NoProgress,
    )
    .await;

    assert_eq!(result.discovery_mode, DiscoveryMode::Full);
    let by_name: HashMap<&str, &crate::repositories::LocalRepoStatus> = result
        .repos
        .iter()
        .map(|repo| (repo.repo_name.as_str(), repo))
        .collect();

    let widgets_status = by_name["widgets"];
    assert_eq!(widgets_status.status, RepoAccessStatus::Accessible);
    assert_eq!(widgets_status.session_count, 4);
    assert!(widgets_status.enabled);

    assert_eq!(by_name["toolkit"].status, RepoAccessStatus::NotCloned);
    assert!(!by_name["toolkit"].enabled, "explicit exclusion survives");
    assert!(
        by_name["sprockets"].enabled,
        "an unset selection flag reads as enabled"
    );
}

/// Without a known-repository list the run reports exactly what is on disk for
/// the owner, and nothing belonging to anyone else.
#[tokio::test]
#[serial]
async fn run_without_known_repos_reports_only_the_owner_clones() {
    let home = tempfile::TempDir::new().unwrap();
    let _home = EnvGuard::set("HOME", home.path());
    let state_dir = home.path().join("state");
    tokio::fs::create_dir_all(&state_dir).await.unwrap();

    let mine = home.path().join("dev").join("widgets");
    let theirs = home.path().join("dev").join("foreign");
    init_repo(&mine, "git@github.com:acme/widgets.git").await;
    init_repo(&theirs, "git@github.com:otherowner/foreign.git").await;

    let sessions = FakeSessions::new(&[]);
    let consent = FakeConsent::empty(home.path());

    let result = discover_repositories(
        request("acme", None, &state_dir),
        &sessions,
        &consent,
        &NoProgress,
    )
    .await;

    assert_eq!(result.discovery_mode, DiscoveryMode::SessionsOnly);
    assert_eq!(result.repos.len(), 1);
    assert_eq!(result.repos[0].full_name, "acme/widgets");
    assert_eq!(
        result.repos[0].session_count, 0,
        "found by the directory scan, with no session activity"
    );
}

/// The progress seam reports each phase in order, carries the running resolve
/// counts, and finishes with a single non-scanning `Done` that lists what was
/// found.
#[tokio::test]
#[serial]
async fn progress_seam_reports_phases_in_order() {
    let home = tempfile::TempDir::new().unwrap();
    let _home = EnvGuard::set("HOME", home.path());
    let state_dir = home.path().join("state");
    tokio::fs::create_dir_all(&state_dir).await.unwrap();

    let widgets = home.path().join("dev").join("widgets");
    init_repo(&widgets, "git@github.com:acme/widgets.git").await;

    let sessions = FakeSessions::new(&[(&widgets, 1)]);
    let consent = FakeConsent::empty(home.path());
    let seen: Mutex<Vec<DiscoveryProgress>> = Mutex::new(Vec::new());

    let result = discover_repositories(
        request("acme", None, &state_dir),
        &sessions,
        &consent,
        &FnProgress(|progress: DiscoveryProgress| seen.lock().unwrap().push(progress)),
    )
    .await;
    assert_eq!(result.repos.len(), 1);

    let seen = seen.into_inner().unwrap();
    let phases: Vec<&DiscoveryPhase> = seen.iter().map(|p| &p.phase).collect();

    assert_eq!(phases.first(), Some(&&DiscoveryPhase::SearchingSessions));
    assert!(
        phases.contains(&&DiscoveryPhase::AgentScanned {
            agent: "Agent Two".to_string()
        }),
        "each agent reports as it finishes"
    );
    assert!(phases.contains(&&DiscoveryPhase::CheckingConsent));
    assert!(phases.contains(&&DiscoveryPhase::ResolvingRoots));
    assert!(phases.contains(&&DiscoveryPhase::ReadingRepoDetails));
    assert!(phases.contains(&&DiscoveryPhase::RepoFound {
        repo_name: "widgets".to_string()
    }));

    // Only the final snapshot stops scanning, and it carries the result.
    let done = seen.last().expect("a run always reports Done");
    assert_eq!(done.phase, DiscoveryPhase::Done);
    assert!(!done.scanning);
    assert_eq!(done.resolved, Some(1));
    assert_eq!(done.total_paths, Some(1));
    assert_eq!(done.repos.len(), 1);
    assert_eq!(done.repos[0].repo_name, "widgets");
    assert_eq!(
        seen.iter().filter(|p| !p.scanning).count(),
        1,
        "exactly one terminal snapshot"
    );

    // The resolve phase counts up to the total.
    let resolve_counts: Vec<Option<usize>> = seen
        .iter()
        .filter(|p| p.phase == DiscoveryPhase::ResolvingRoots)
        .map(|p| p.resolved)
        .collect();
    assert_eq!(resolve_counts, vec![Some(0), Some(1)]);
}

/// The consent seam is consulted before anything is walked, and a protected
/// directory with no grant is deferred rather than read.
#[cfg(target_os = "macos")]
#[tokio::test]
#[serial]
async fn ungranted_protected_session_cwd_is_deferred_not_read() {
    let home = tempfile::TempDir::new().unwrap();
    let _home = EnvGuard::set("HOME", home.path());
    let state_dir = home.path().join("state");
    tokio::fs::create_dir_all(&state_dir).await.unwrap();

    let protected = home.path().join("Documents").join("widgets");
    init_repo(&protected, "git@github.com:acme/widgets.git").await;

    let sessions = FakeSessions::new(&[(&protected, 2)]);
    let consent = FakeConsent::empty(home.path());

    let result = discover_repositories(
        request(
            "acme",
            Some(vec![descriptor("widgets", "acme", Some(true))]),
            &state_dir,
        ),
        &sessions,
        &consent,
        &NoProgress,
    )
    .await;

    assert!(
        result
            .deferred_protected
            .iter()
            .any(|entry| entry.protected_dir == "Documents" && Path::new(&entry.cwd) == protected),
        "the protected working directory is deferred: {:?}",
        result.deferred_protected
    );
    assert_eq!(
        result
            .repos
            .iter()
            .find(|repo| repo.repo_name == "widgets")
            .map(|repo| repo.status.clone()),
        Some(RepoAccessStatus::NotCloned),
        "a deferred clone is never read, so it is not located"
    );
    assert!(
        consent.probes().is_empty(),
        "nothing under an ungranted directory is probed"
    );
}

/// A directory the user granted outside the application, picked up by the opt-in
/// probe, is promoted out of the deferred set and resolved in the same run
/// rather than waiting for the next one.
///
/// Git resolution consults the grant record too (for the deleted-worktree
/// fallback), which is why the record it reads is re-taken after the probe.
#[cfg(target_os = "macos")]
#[tokio::test]
#[serial]
async fn externally_granted_protected_cwd_resolves_in_the_same_run() {
    let home = tempfile::TempDir::new().unwrap();
    let _home = EnvGuard::set("HOME", home.path());
    let state_dir = home.path().join("state");
    tokio::fs::create_dir_all(&state_dir).await.unwrap();

    let protected = home.path().join("Documents").join("widgets");
    init_repo(&protected, "git@github.com:acme/widgets.git").await;

    let sessions = FakeSessions::new(&[(&protected, 2)]);
    let consent = FakeConsent::empty(home.path());
    consent.stage_external_grant("Documents");

    let result = discover_repositories(
        DiscoveryRequest {
            // The opt-in probe: only ever set in response to an explicit user
            // action, because probing can raise the consent dialog.
            probe_protected: true,
            ..request(
                "acme",
                Some(vec![descriptor("widgets", "acme", Some(true))]),
                &state_dir,
            )
        },
        &sessions,
        &consent,
        &NoProgress,
    )
    .await;

    assert!(
        result.deferred_protected.is_empty(),
        "the freshly granted directory is no longer deferred: {:?}",
        result.deferred_protected
    );
    let widgets = result
        .repos
        .iter()
        .find(|repo| repo.repo_name == "widgets")
        .expect("the known repository is reported");
    assert_eq!(
        widgets.status,
        RepoAccessStatus::Accessible,
        "the promoted working directory resolves to its clone"
    );
    assert_eq!(widgets.session_count, 2);
}
