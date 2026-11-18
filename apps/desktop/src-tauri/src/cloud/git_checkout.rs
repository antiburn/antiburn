//! Optional local Git context for a new desktop capture.

use antiburn_cloud_capture::{
    Candidate, MetadataEvidence, MetadataObservation, MetadataProducer, MetadataSection,
    MetadataSections,
};
use antiburn_local::platform::process::{
    BoundedOutputOptions, BoundedOutputPolicy, bounded_headless_output_with_stdin_and_policy,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

const SECTION_KEY: &str = "antiburn.git-checkout";

#[derive(Serialize)]
struct Checkout {
    commit: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repository: Option<Repository>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Repository {
    owner: String,
    name: String,
}

/// Use the first admitted recorded CWD. Call only on the bounded capture worker
/// under current content consent, before writing new capture bytes, never on
/// upload resume. Failure to observe Git does not block capture.
pub(super) fn capture_sections(
    candidate: &Candidate,
    current: &dyn Fn() -> bool,
    allowed_checkout: &dyn Fn(&Path) -> bool,
) -> MetadataSections {
    let Some(cwd) = candidate.admitted_project_path().filter(|_| current()) else {
        return MetadataSections::new();
    };
    if !verified_directory(cwd) || !current() {
        return MetadataSections::new();
    }
    let checkout = tauri::async_runtime::block_on(observe(cwd, allowed_checkout));
    if !current() || !verified_directory(cwd) {
        return MetadataSections::new();
    }
    checkout
        .and_then(section)
        .map(|section| MetadataSections::from([(SECTION_KEY.into(), section)]))
        .unwrap_or_default()
}

fn verified_directory(cwd: &Path) -> bool {
    cwd.is_absolute() && cwd.is_dir() && cwd.canonicalize().ok().as_deref() == Some(cwd)
}

async fn observe(cwd: &Path, allowed_checkout: &dyn Fn(&Path) -> bool) -> Option<Checkout> {
    const HEAD: &[&str] = &[
        "rev-parse",
        "--path-format=absolute",
        "--show-toplevel",
        "--absolute-git-dir",
        "--git-common-dir",
        "--is-inside-work-tree",
        "--verify",
        "HEAD^{commit}",
    ];
    let output = git(cwd, HEAD).await?;
    let [root, git_dir, common_dir, "true", commit] = output.lines().collect::<Vec<_>>()[..] else {
        return None;
    };
    let root = Path::new(root);
    if !verified_directory(root)
        || !cwd.starts_with(root)
        || !allowed_checkout(root)
        || !Path::new(git_dir).is_absolute()
        || !Path::new(common_dir).is_absolute()
        || !valid_commit(commit)
    {
        return None;
    }

    let (branch, remote) = tokio::join!(
        git(cwd, &["symbolic-ref", "--quiet", "HEAD"]),
        git(
            cwd,
            &[
                "config",
                "--local",
                "--no-includes",
                "--null",
                "--get-all",
                "remote.origin.url",
            ],
        ),
    );
    if git(cwd, HEAD).await.as_deref() != Some(output.as_str()) {
        return None;
    }
    Some(Checkout {
        commit: commit.into(),
        kind: if git_dir == common_dir {
            "checkout"
        } else {
            "worktree"
        },
        branch: branch.as_deref().and_then(branch_name).map(str::to_owned),
        repository: remote
            .as_deref()
            .and_then(|remote| remote.strip_suffix('\0'))
            .and_then(github_repository),
    })
}

fn valid_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64)
        && commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn branch_name(output: &str) -> Option<&str> {
    let branch = output.strip_suffix('\n')?.strip_prefix("refs/heads/")?;
    (!branch.is_empty() && branch.len() <= 255 && !branch.chars().any(char::is_control))
        .then_some(branch)
}

/// Accept only direct GitHub clone addresses. Do not return the address, user
/// info, or other path segments. An unsupported address leaves identity absent.
fn github_repository(remote: &str) -> Option<Repository> {
    let (authority, path) = if let Some(url) = remote.strip_prefix("https://") {
        url.split_once('/')?
    } else if let Some(url) = remote.strip_prefix("ssh://git@") {
        url.split_once('/')?
    } else {
        remote.strip_prefix("git@")?.split_once(':')?
    };
    if !authority.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let (owner, name) = path.split_once('/')?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    if owner.is_empty()
        || owner.len() > 39
        || owner.starts_with('-')
        || owner.ends_with('-')
        || !owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || name.is_empty()
        || name.len() > 100
        || matches!(name, "." | "..")
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return None;
    }
    Some(Repository {
        owner: owner.into(),
        name: name.into(),
    })
}

/// Run fixed, read-only commands without ambient Git settings, output logs, or
/// transport access. An oversized result does not supply partial metadata.
async fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = bounded_headless_output_with_stdin_and_policy(
        "git",
        args,
        None,
        BoundedOutputOptions {
            timeout: Duration::from_secs(1),
            max_stdout_bytes: 16 * 1024,
            max_stderr_bytes: 1024,
            env: &[
                ("GIT_CONFIG_NOSYSTEM", "1"),
                (
                    "GIT_CONFIG_GLOBAL",
                    if cfg!(windows) { "NUL" } else { "/dev/null" },
                ),
                ("GIT_ALLOW_PROTOCOL", ""),
                ("GIT_NO_LAZY_FETCH", "1"),
                ("GIT_NO_REPLACE_OBJECTS", "1"),
            ],
            env_remove: &[],
            current_dir: Some(cwd),
        },
        BoundedOutputPolicy {
            clear_env: true,
            fail_on_stdout_limit: true,
            kill_descendants_on_exit: true,
        },
    )
    .await
    .ok()?;
    if !output.status.success() || output.truncated {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn section(checkout: Checkout) -> Option<MetadataSection> {
    let unix_millis =
        u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000).ok()?;
    if !(1..=253_402_300_799_999).contains(&unix_millis) {
        return None;
    }
    let data = serde_json::to_value(checkout).ok()?;
    let compact = serde_json::to_vec(&data).ok()?;
    Some(MetadataSection {
        schema: "antiburn.git-checkout.v1".into(),
        producer: MetadataProducer {
            name: "antiburn-desktop".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            unknown_fields: Default::default(),
        },
        observation: MetadataObservation::ObservedAt {
            unix_millis,
            unknown_fields: Default::default(),
        },
        evidence: vec![MetadataEvidence::LocalObservation {
            method: "git-checkout".into(),
            data_sha256: Sha256::digest(compact)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            unknown_fields: Default::default(),
        }],
        data,
        unknown_fields: Default::default(),
    })
}

#[cfg(test)]
#[path = "tests/git_checkout_tests.rs"]
mod tests;
