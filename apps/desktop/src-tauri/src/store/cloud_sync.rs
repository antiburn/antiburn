//! Durable ongoing consent, fixed capture jobs, and accepted source state.
//! Calls require bounded blocking admission. The desktop controller checks current
//! authority before calls. No record contains credentials or upload locations.

use super::{RootScope, SourceKind, SourceSelection};
use crate::store::Store;
use antiburn_cloud_capture::{AcceptedSourceFingerprint, CaptureIdentity, QueueEntry};
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
};

/// Full session content and ongoing capture require a separate explicit confirmation.
pub(crate) const ONGOING_CONSENT_VERSION: &str = "desktop-project-sync-v1";
/// Bound active jobs across all accounts, including fenced and interrupted work.
pub(crate) const MAX_ACTIVE_JOBS: usize = 128;
/// Keep frozen routing evidence within the existing job and discovery budgets.
pub(crate) const MAX_RECORDED_PROJECT_PATH_BYTES: usize = 8192;
/// Bound the provider roots in a saved policy.
pub(crate) const MAX_SYNC_SOURCE_ROOTS: usize = 100;
/// Match the capture limit for distinct exact project directories.
pub(crate) const MAX_SYNC_PROJECT_PATHS: usize = 100;
/// Bound exact provider-root and project-directory pairs in a saved policy.
pub(crate) const MAX_SYNC_PROJECT_BINDINGS: usize = 512;
const MAX_SOURCES: u64 = 16_384;
const MAX_JOBS: u64 = 32_768;

type PolicyBinding<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    Option<(&'a str, &'a str)>,
);

/// Confirmed project bindings, group choices, and verified repository anchors.
pub(crate) type TeamPolicyAdditions<'a> = (
    &'a BTreeSet<ProjectSelection>,
    &'a BTreeMap<String, bool>,
    &'a BTreeMap<String, String>,
);

enum PolicyWrite<'a> {
    Create(&'a dyn Fn() -> bool),
    Reapprove(&'a Policy, &'a dyn Fn() -> bool),
    #[cfg(test)]
    ReplaceFixture,
}

enum JobRouting<'a> {
    ExactFolder(Option<&'a [String]>),
    Repository {
        origin: &'a str,
        recorded_paths: &'a [String],
    },
}

/// Keep a missing preference distinct from OFF for first folder approval.
fn saved_device_upload_enabled(connection: &Connection) -> Result<Option<bool>> {
    Ok(connection
        .query_row(
            "SELECT enabled FROM cloud_device_upload WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

fn device_upload_enabled(connection: &Connection) -> Result<bool> {
    Ok(saved_device_upload_enabled(connection)?.unwrap_or(false))
}

fn write_device_upload_enabled(connection: &Connection, enabled: bool) -> Result<()> {
    connection.execute(
        "INSERT INTO cloud_device_upload (id, enabled) VALUES (1, ?1)
         ON CONFLICT(id) DO UPDATE SET enabled=excluded.enabled",
        [enabled],
    )?;
    Ok(())
}

/// Recorded project directory chosen by the user for one native source.
/// A touched file or a path mentioned in tool output cannot satisfy this rule.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProjectSelection {
    pub source_kind: SourceKind,
    pub source_path: String,
    pub project_path: String,
}

/// Repository consent for the listed source scopes. The anchor is display data.
/// Only native checkout proof can identify a matching origin.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RepositoryRule {
    pub origin: String,
    pub anchor_path: String,
    pub scopes: BTreeSet<String>,
}

/// Use the same repository group ID for discovery, saved status, and policy writes.
pub(crate) fn repository_group_id(origin: &str) -> String {
    // Exact folder IDs hash a JSON array. Keep repository IDs in a separate domain.
    let mut hash = Sha256::new();
    hash.update(b"antiburn-repository-origin-v1\0");
    hash.update(origin.as_bytes());
    URL_SAFE_NO_PAD.encode(hash.finalize())
}

/// Check the exact bindings and distinct paths without normalizing path aliases.
pub(crate) fn check_project_binding_limits<'a>(
    projects: impl ExactSizeIterator<Item = &'a ProjectSelection>,
) -> Result<()> {
    ensure!(
        projects.len() <= MAX_SYNC_PROJECT_BINDINGS,
        "Cloud project bindings exceed the policy limit"
    );
    let mut paths = BTreeSet::new();
    let mut roots = BTreeSet::new();
    for project in projects {
        paths.insert(&project.project_path);
        roots.insert((project.source_kind, &project.source_path));
    }
    ensure!(
        paths.len() <= MAX_SYNC_PROJECT_PATHS,
        "Cloud project directories exceed the policy limit"
    );
    ensure!(
        roots.len() <= MAX_SYNC_SOURCE_ROOTS,
        "Cloud source roots exceed the policy limit"
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/cloud_sync_integration_tests.rs"]
mod tests;

/// Current folder consent for one account, workspace, and service. Team consent
/// stays bound to one membership generation, not a device grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Policy {
    pub account_id: String,
    pub destination_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_origin: Option<String>,
    /// Legacy Team marker. It identifies saved consent for cleanup only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_grant_id: Option<String>,
    /// Present only after explicit approval for this Team membership.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membership_generation: Option<String>,
    /// Last verified team name. The name is display data, not grant authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_name: Option<String>,
    pub consent_version: String,
    pub revision: String,
    pub roots: Vec<RootScope>,
    pub projects: Vec<ProjectSelection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repository_rules: Vec<RepositoryRule>,
    /// Confirmed UI group choices, including exclusions. These are not permissions.
    #[serde(default)]
    pub project_choices: BTreeMap<String, bool>,
    pub paused: bool,
}

impl Policy {
    /// Include legacy Team consent so the user can remove or confirm its folders.
    pub(crate) fn is_team(&self) -> bool {
        self.membership_generation.is_some()
            || self.team_grant_id.is_some()
            || self.workspace_name.is_some()
    }

    /// Compare saved Team consent with current membership proof.
    pub(crate) fn needs_team_membership(&self, generation: &str) -> bool {
        self.is_team()
            && (self.team_grant_id.is_some()
                || self.membership_generation.as_deref() != Some(generation))
    }

    fn has_membership_authority(&self) -> bool {
        !self.is_team()
            || (self.team_grant_id.is_none()
                && self
                    .membership_generation
                    .as_deref()
                    .is_some_and(valid_membership_generation))
    }

    /// Match the exact source and a recorded project under a selected directory.
    /// Component matching excludes sibling checkouts outside that directory.
    /// Source roots do not imply
    /// permission for every project in a shared provider database.
    pub(crate) fn allows(&self, scope: &str, project: &str) -> bool {
        self.has_membership_authority()
            && self.resource_origin.is_some()
            && self.roots.iter().any(|root| {
                root.source_scope_id == scope
                    && self.projects.iter().any(|selected| {
                        selected.source_kind == root.source_kind
                            && selected.source_path == root.path
                            && valid_project_path(project)
                            && Path::new(project).starts_with(&selected.project_path)
                    })
            })
    }

    /// Match a verified origin only within a saved provider source scope.
    /// The caller must verify the checkout and current account authority.
    pub(crate) fn allows_repository(&self, scope: &str, origin: &str) -> bool {
        self.has_membership_authority()
            && self.resource_origin.is_some()
            && self.roots.iter().any(|root| root.source_scope_id == scope)
            && self
                .repository_rules
                .iter()
                .any(|rule| rule.origin == origin && rule.scopes.contains(scope))
    }

    /// Find a saved repository rule without reading the checkout.
    pub(crate) fn repository_rule_for_group(&self, group_id: &str) -> Option<&RepositoryRule> {
        self.repository_rules
            .iter()
            .find(|rule| repository_group_id(&rule.origin) == group_id)
    }

    /// List exact folders and repository anchors for status and confirmation.
    /// Do not use this list to authorize capture by path.
    pub(crate) fn projects_for_display(&self) -> Vec<ProjectSelection> {
        let mut projects = self.projects.clone();
        projects.extend(self.repository_rules.iter().flat_map(|rule| {
            self.roots
                .iter()
                .filter(|root| rule.scopes.contains(&root.source_scope_id))
                .map(|root| ProjectSelection {
                    source_kind: root.source_kind,
                    source_path: root.path.clone(),
                    project_path: rule.anchor_path.clone(),
                })
        }));
        projects.sort();
        projects.dedup();
        projects
    }
}

/// Host state is separate from the spool's lifecycle and never proves acceptance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JobState {
    Pending,
    /// A proved capacity refusal permits a same-ID reservation retry. Change to
    /// Pending before retry I/O so uncertain delivery cannot keep this permission.
    WaitingForCapacity,
    /// Local capacity refused capture before spool publication or reservation.
    /// Retry the same ID only after fresh source and project checks.
    WaitingForLocalCapacity,
    Queued,
    Interrupted,
    Fenced,
    Accepted,
    Suppressed,
    Recovering,
    Abandoned,
}

impl JobState {
    /// Terminal jobs keep their IDs after the spool releases its receipt.
    pub(crate) fn terminal(self) -> bool {
        matches!(self, Self::Accepted | Self::Suppressed | Self::Abandoned)
    }
}

/// Immutable local selection and source observation for one unique capture ID.
/// The observation is a scheduling hint, not an accepted content fingerprint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Job {
    pub identity: CaptureIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_origin: Option<String>,
    /// Legacy Team marker. It does not authorize recovery or upload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_grant_id: Option<String>,
    /// Freeze the Team generation at admission. Retry must not change it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub membership_generation: Option<String>,
    pub source_scope_id: String,
    pub project_path: String,
    #[serde(default)]
    pub project_folders: Vec<String>,
    /// Verified origin used for repository admission. Exact-folder jobs omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_origin: Option<String>,
    /// Exact source CWD evidence used to check a saved capture's source, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_project_paths: Option<Vec<String>>,
    pub observation_key: String,
    pub state: JobState,
    /// Earliest capacity retry in Unix seconds. Absent outside the proved
    /// waiting state; ordinary state changes clear this permission and deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reservation_retry_at: Option<i64>,
}

impl Job {
    /// Match the job's account, workspace, origins, and membership. Call before
    /// recovery as well as upload. A legacy Team marker can never match.
    pub(crate) fn matches_membership(&self, policy: &Policy) -> bool {
        self.resource_origin.is_some()
            && self.resource_origin == policy.resource_origin
            && self.issuer_origin.is_some()
            && self.issuer_origin == policy.issuer_origin
            && self.identity.account_id == policy.account_id
            && self.identity.destination_id == policy.destination_id
            && self.same_membership_generation(policy)
    }

    fn same_membership_generation(&self, policy: &Policy) -> bool {
        self.team_grant_id.is_none()
            && policy.has_membership_authority()
            && self.membership_generation == policy.membership_generation
    }

    /// Keep every approved folder bound to saved bytes. Narrowing the current
    /// policy fences the whole capture instead of removing part of its content.
    pub(crate) fn allowed(&self, policy: &Policy) -> bool {
        self.matches_membership(policy)
            && match &self.repository_origin {
                Some(origin) => policy.allows_repository(&self.source_scope_id, origin),
                None => {
                    policy.allows(&self.source_scope_id, &self.project_path)
                        && self
                            .project_folders
                            .iter()
                            .all(|folder| policy.allows(&self.source_scope_id, folder))
                }
            }
    }

    /// Reject changed scope, owner, and project bindings before spool access.
    fn validate(&self) -> Result<()> {
        ensure!(
            safe_id(&self.identity.capture_id)
                && self
                    .resource_origin
                    .as_deref()
                    .is_none_or(valid_resource_origin)
                && self
                    .issuer_origin
                    .as_deref()
                    .is_none_or(valid_resource_origin)
                && self
                    .membership_generation
                    .as_deref()
                    .is_none_or(valid_membership_generation)
                && safe_id(&self.identity.account_id)
                && safe_id(&self.identity.destination_id)
                && safe_id(&self.source_scope_id)
                && self
                    .identity
                    .source_id
                    .starts_with(&format!("{}/", self.source_scope_id))
                && self.identity.source_id.len() <= 512
                && !self.identity.source_id.chars().any(char::is_control)
                && valid_project_path(&self.project_path)
                && self.project_folders.len() <= MAX_SYNC_PROJECT_PATHS
                && self
                    .project_folders
                    .iter()
                    .all(|path| valid_project_path(path))
                && self.repository_origin.as_deref().is_none_or(|origin| {
                    valid_repository_origin_label(origin)
                        && self.project_folders.is_empty()
                        && self.recorded_project_paths.is_some()
                })
                && self.recorded_project_paths.as_ref().is_none_or(|paths| {
                    !paths.is_empty()
                        && paths.len() <= 256
                        && paths.iter().map(String::len).sum::<usize>()
                            <= MAX_RECORDED_PROJECT_PATH_BYTES
                        && paths.iter().all(|path| valid_project_path(path))
                        && paths.contains(&self.project_path)
                })
                && !self.observation_key.is_empty()
                && self.observation_key.len() <= 4096,
            "Cloud job binding changed"
        );
        ensure!(
            matches!(
                self.state,
                JobState::WaitingForCapacity | JobState::WaitingForLocalCapacity
            ) == self.reservation_retry_at.is_some()
                && self.reservation_retry_at.is_none_or(|at| at > 0),
            "Cloud capacity retry state changed"
        );
        Ok(())
    }
}

/// Retained service outcome. Suppression blocks automatic recapture of the
/// source after restart. An accepted fingerprint alone cannot clear suppression.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_origin: Option<String>,
    pub fingerprint: Option<AcceptedSourceFingerprint>,
    pub observation_key: Option<String>,
    pub suppressed: bool,
}

/// Accept only canonical service origins, never paths, proof, or signed URLs.
fn valid_resource_origin(value: &str) -> bool {
    value.len() <= 4096
        && url::Url::parse(value).is_ok_and(|url| {
            matches!(url.scheme(), "https" | "http")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
                && url.origin().ascii_serialization() == value
        })
}

/// Bound and validate native repository approvals against the saved source roots.
fn check_repository_rules(rules: &[RepositoryRule], roots: &[RootScope]) -> Result<()> {
    if rules.is_empty() {
        return Ok(());
    }
    ensure!(
        rules.len() <= MAX_SYNC_PROJECT_PATHS,
        "Cloud repository rules exceed the policy limit"
    );
    ensure!(
        rules.windows(2).all(|pair| pair[0].origin < pair[1].origin),
        "Cloud repository origins must be unique and sorted"
    );
    let scopes: BTreeSet<_> = roots
        .iter()
        .filter(|root| valid_project_path(&root.path) && safe_id(&root.source_scope_id))
        .map(|root| root.source_scope_id.as_str())
        .collect();
    ensure!(
        rules.iter().all(|rule| {
            valid_repository_origin_label(&rule.origin)
                && valid_project_path(&rule.anchor_path)
                && !rule.scopes.is_empty()
                && rule.scopes.len() <= MAX_SYNC_SOURCE_ROOTS
                && rule
                    .scopes
                    .iter()
                    .all(|scope| scopes.contains(scope.as_str()))
        }),
        "Choose verified Cloud repositories and saved source scopes"
    );
    Ok(())
}

/// Check the shared limits without treating repository anchors as exact grants.
fn check_policy_projects(policy: &Policy) -> Result<()> {
    check_repository_rules(&policy.repository_rules, &policy.roots)?;
    let displayed = policy.projects_for_display();
    let expected_bindings = policy.projects.len()
        + policy
            .repository_rules
            .iter()
            .map(|rule| rule.scopes.len())
            .sum::<usize>();
    ensure!(
        displayed.len() == expected_bindings,
        "Cloud repository anchors must not replace saved exact folders"
    );
    check_project_binding_limits(displayed.iter())
}

/// Resolve checked anchor paths to the source scopes allocated in this transaction.
fn add_cloud_repository_anchors<'a>(
    policy: &mut Policy,
    projects: impl Iterator<Item = &'a ProjectSelection> + Clone,
    anchors: &BTreeMap<String, String>,
) -> Result<()> {
    for (anchor_path, origin) in anchors {
        let scopes = projects
            .clone()
            .filter(|project| project.project_path == *anchor_path)
            .map(|project| {
                policy
                    .roots
                    .iter()
                    .find(|root| {
                        root.source_kind == project.source_kind && root.path == project.source_path
                    })
                    .map(|root| root.source_scope_id.clone())
                    .ok_or_else(|| anyhow::anyhow!("Cloud repository scope is missing"))
            })
            .collect::<Result<BTreeSet<_>>>()?;
        ensure!(
            !scopes.is_empty(),
            "Cloud repository anchor is not selected"
        );
        match policy
            .repository_rules
            .binary_search_by(|rule| rule.origin.cmp(origin))
        {
            Ok(index) => policy.repository_rules[index].scopes.extend(scopes),
            Err(index) => policy.repository_rules.insert(
                index,
                RepositoryRule {
                    origin: origin.clone(),
                    anchor_path: anchor_path.clone(),
                    scopes,
                },
            ),
        }
    }
    Ok(())
}

/// Validate a historical recorded path without opening any project files.
pub(crate) fn valid_project_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && !value.chars().any(char::is_control)
        && Path::new(value).is_absolute()
        && (value == "/" || !value.ends_with('/'))
        && !value.contains("//")
        && !value.split('/').any(|part| part == "." || part == "..")
        && Path::new(value)
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
}

fn random_id() -> Result<String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| anyhow::anyhow!("Cloud key is unavailable"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn valid_membership_generation(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn valid_team_membership(generation: &str, name: &str) -> bool {
    valid_membership_generation(generation)
        && !name.is_empty()
        && name.len() <= 256
        && !name.chars().any(char::is_control)
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Generate a fresh UUIDv4 capture ID without a stored device or account secret.
fn capture_id() -> Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| anyhow::anyhow!("Cloud capture key is unavailable"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

fn encode(value: &impl Serialize, limit: usize) -> Result<String> {
    let raw = serde_json::to_string(value)?;
    ensure!(raw.len() <= limit, "Cloud record exceeds its limit");
    Ok(raw)
}

fn saved_policy(
    connection: &Connection,
    account: &str,
    destination: &str,
) -> Result<Option<Policy>> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT record_json FROM cloud_sync_policy WHERE account_id=?1 AND destination_id=?2",
            params![account, destination],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| {
        let policy: Policy = serde_json::from_str(&raw)?;
        ensure!(
            policy.account_id == account && policy.destination_id == destination,
            "Cloud policy binding changed"
        );
        check_repository_rules(&policy.repository_rules, &policy.roots)?;
        Ok(policy)
    })
    .transpose()
}

/// Refresh the revision only when a confirmed policy change affects saved state.
fn write_cloud_policy_change(
    connection: &Connection,
    previous: &Policy,
    saved: &mut Policy,
) -> Result<()> {
    if *saved != *previous {
        saved.revision = random_id()?;
        connection.execute(
            "UPDATE cloud_sync_policy SET record_json=?3 WHERE account_id=?1 AND destination_id=?2",
            params![
                saved.account_id,
                saved.destination_id,
                encode(saved, 524_288)?
            ],
        )?;
    }
    Ok(())
}

fn source_in(
    connection: &Connection,
    account: &str,
    destination: &str,
    source_id: &str,
) -> Result<Option<SourceState>> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT record_json FROM cloud_sync_source
             WHERE account_id=?1 AND destination_id=?2 AND source_id=?3",
            params![account, destination, source_id],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| {
        let value: SourceState = serde_json::from_str(&raw)?;
        ensure!(
            value
                .resource_origin
                .as_deref()
                .is_none_or(valid_resource_origin),
            "Cloud source service binding changed"
        );
        ensure!(
            value.fingerprint.is_some() == value.observation_key.is_some(),
            "Cloud accepted observation changed"
        );
        if let Some(fingerprint) = &value.fingerprint {
            ensure!(
                fingerprint.account_id == account
                    && fingerprint.destination_id == destination
                    && fingerprint.source_id == source_id,
                "Cloud accepted source binding changed"
            );
        }
        Ok(value)
    })
    .transpose()
}

fn job_in(connection: &Connection, capture_id: &str) -> Result<Option<Job>> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT record_json FROM cloud_sync_job WHERE capture_id=?1",
            [capture_id],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| {
        let job: Job = serde_json::from_str(&raw)?;
        job.validate()?;
        ensure!(
            job.identity.capture_id == capture_id,
            "Cloud job binding changed"
        );
        Ok(job)
    })
    .transpose()
}

/// Load active jobs for one destination without opening a second transaction.
fn active_cloud_sync_jobs(
    connection: &Connection,
    account: &str,
    destination: &str,
    resource_origin: &str,
) -> Result<Vec<Job>> {
    let mut statement = connection.prepare(
        "SELECT record_json FROM cloud_sync_job WHERE account_id=?1 AND destination_id=?2
         AND json_extract(record_json, '$.resource_origin')=?3
         AND json_extract(record_json, '$.issuer_origin') IS NOT NULL
         AND json_extract(record_json, '$.state') NOT IN ('accepted', 'suppressed', 'abandoned')
         ORDER BY rowid LIMIT 129",
    )?;
    let mut jobs = Vec::new();
    for raw in statement.query_map(params![account, destination, resource_origin], |row| {
        row.get::<_, String>(0)
    })? {
        let job: Job = serde_json::from_str(&raw?)?;
        job.validate()?;
        ensure!(
            job.identity.account_id == account && job.identity.destination_id == destination,
            "Cloud job binding changed"
        );
        if !job.state.terminal() {
            ensure!(
                jobs.len() < MAX_ACTIVE_JOBS,
                "Cloud queue exceeds its limit"
            );
            jobs.push(job);
        }
    }
    Ok(jobs)
}

/// Keep source identity at the resource destination. A new issuer or grant
/// requires fresh consent but must not bypass prior acceptance or suppression.
fn source_scope(
    transaction: &Transaction<'_>,
    account: &str,
    destination: &str,
    resource_origin: &str,
    root: &SourceSelection,
) -> Result<RootScope> {
    let kind = serde_json::to_string(&root.source_kind)?;
    let saved: Option<String> = transaction
        .query_row(
            "SELECT source_scope_id FROM cloud_sync_service_scope
             WHERE account_id=?1 AND destination_id=?2 AND resource_origin=?3 AND source_kind=?4 AND root_path=?5",
            params![account, destination, resource_origin, kind, root.path],
            |row| row.get(0),
        )
        .optional()?;
    let source_scope_id = match saved {
        Some(saved) => saved,
        None => {
            let count: u64 = transaction.query_row(
                "SELECT count(*) FROM cloud_sync_service_scope",
                [],
                |row| row.get(0),
            )?;
            ensure!(count < 4096, "Cloud source scope storage is full");
            let scope = random_id()?;
            transaction.execute(
                "INSERT INTO cloud_sync_service_scope VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    account,
                    destination,
                    resource_origin,
                    kind,
                    root.path,
                    scope
                ],
            )?;
            scope
        }
    };
    Ok(RootScope {
        path: root.path.clone(),
        source_kind: root.source_kind,
        source_scope_id,
    })
}

/// Add confirmed folders and repository anchors after allocating their scopes.
fn append_cloud_projects(
    transaction: &Transaction<'_>,
    policy: &mut Policy,
    origin: &str,
    projects: &BTreeSet<ProjectSelection>,
    repository_anchors: &BTreeMap<String, String>,
) -> Result<()> {
    check_project_binding_limits(projects.iter())?;
    ensure!(
        projects
            .iter()
            .all(|project| valid_project_path(&project.project_path)),
        "Choose recorded Cloud projects"
    );
    append_cloud_roots(transaction, policy, origin, projects)?;
    policy.projects.extend(
        projects
            .iter()
            .filter(|project| !repository_anchors.contains_key(&project.project_path))
            .cloned(),
    );
    policy.projects.sort();
    policy.projects.dedup();
    add_cloud_repository_anchors(policy, projects.iter(), repository_anchors)?;
    check_policy_projects(policy)
}

fn append_cloud_roots(
    transaction: &Transaction<'_>,
    policy: &mut Policy,
    origin: &str,
    projects: &BTreeSet<ProjectSelection>,
) -> Result<()> {
    let roots: BTreeSet<_> = projects
        .iter()
        .map(|project| SourceSelection {
            source_kind: project.source_kind,
            path: project.source_path.clone(),
        })
        .collect();
    for root in roots {
        if !policy
            .roots
            .iter()
            .any(|existing| existing.source_kind == root.source_kind && existing.path == root.path)
        {
            ensure!(
                policy.roots.len() < MAX_SYNC_SOURCE_ROOTS,
                "Cloud source roots exceed the policy limit"
            );
            policy.roots.push(source_scope(
                transaction,
                &policy.account_id,
                &policy.destination_id,
                origin,
                &root,
            )?);
        }
    }
    policy
        .roots
        .sort_by(|a, b| (a.source_kind, &a.path).cmp(&(b.source_kind, &b.path)));
    Ok(())
}

impl Store {
    /// Read the switch for this device profile. No saved preference means the
    /// user has not approved any folders or used the device switch.
    pub(crate) fn cloud_device_upload_enabled(&self) -> Result<bool> {
        device_upload_enabled(&self.lock())
    }

    /// Persist the device switch without changing a policy, job, or spool.
    pub(crate) fn set_cloud_device_upload_enabled(&self, enabled: bool) -> Result<()> {
        write_device_upload_enabled(&self.lock(), enabled)
    }

    /// Save ON only after the controller accepts it inside the transaction.
    /// Callbacks must not access the store. A rejected request rolls back.
    pub(crate) fn resume_cloud_device_uploads(
        &self,
        current: impl FnOnce() -> bool,
        accept_commit: impl FnOnce() -> bool,
    ) -> Result<bool> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        if saved_device_upload_enabled(&transaction)? != Some(true) {
            write_device_upload_enabled(&transaction, true)?;
        }
        if !current() || !accept_commit() {
            transaction.rollback()?;
            return Ok(false);
        }
        transaction.commit()?;
        Ok(true)
    }

    /// Read only this account and workspace's explicit ongoing policy.
    /// An old policy without a proved issuer cannot authorize upload.
    pub(crate) fn cloud_sync_policy_for_origins(
        &self,
        account: &str,
        destination: &str,
        resource_origin: &str,
        issuer_origin: &str,
    ) -> Result<Option<Policy>> {
        ensure!(
            valid_resource_origin(resource_origin) && valid_resource_origin(issuer_origin),
            "Cloud service origin is invalid"
        );
        let policy = saved_policy(&self.lock(), account, destination)?;
        Ok(policy.filter(|policy| {
            policy.consent_version == ONGOING_CONSENT_VERSION
                && policy.resource_origin.as_deref() == Some(resource_origin)
                && policy.issuer_origin.as_deref() == Some(issuer_origin)
        }))
    }

    /// Read saved Team consent for the verified account and both origins.
    /// Missing or revoked membership does not hide the local off controls.
    pub(crate) fn saved_team_sync_policies(
        &self,
        account: &str,
        personal_workspace: &str,
        resource: &str,
        issuer: &str,
    ) -> Result<Vec<Policy>> {
        ensure!(
            valid_resource_origin(resource) && valid_resource_origin(issuer),
            "Cloud service origins are invalid"
        );
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT destination_id, record_json FROM cloud_sync_policy
             WHERE account_id=?1 AND destination_id<>?2
               AND json_extract(record_json, '$.resourceOrigin')=?3
               AND json_extract(record_json, '$.issuerOrigin')=?4
               AND json_extract(record_json, '$.consentVersion')=?5
                AND (json_type(record_json, '$.membershipGeneration')='text'
                     OR json_type(record_json, '$.teamGrantId')='text'
                     OR json_type(record_json, '$.workspaceName')='text')
             ORDER BY destination_id LIMIT 101",
        )?;
        let mut policies = Vec::new();
        for row in statement.query_map(
            params![
                account,
                personal_workspace,
                resource,
                issuer,
                ONGOING_CONSENT_VERSION
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )? {
            let (destination, raw) = row?;
            let policy: Policy = serde_json::from_str(&raw)?;
            ensure!(
                policies.len() < 100
                    && policy.account_id == account
                    && policy.destination_id == destination
                    && policy.resource_origin.as_deref() == Some(resource)
                    && policy.issuer_origin.as_deref() == Some(issuer)
                    && policy.is_team()
                    && policy
                        .membership_generation
                        .as_deref()
                        .is_none_or(valid_membership_generation)
                    && policy.team_grant_id.as_deref().is_none_or(safe_id)
                    && policy.roots.len() <= MAX_SYNC_SOURCE_ROOTS
                    && check_repository_rules(&policy.repository_rules, &policy.roots).is_ok()
                    && check_project_binding_limits(policy.projects.iter()).is_ok()
                    && policy
                        .projects
                        .iter()
                        .all(|project| valid_project_path(&project.project_path)),
                "Invalid saved team consent"
            );
            policies.push(policy);
        }
        Ok(policies)
    }

    #[cfg(test)]
    pub(crate) fn cloud_sync_policy(
        &self,
        account: &str,
        destination: &str,
        origin: &str,
    ) -> Result<Option<Policy>> {
        self.cloud_sync_policy_for_origins(account, destination, origin, origin)
    }

    /// Save a newly confirmed allowlist and its stable account/workspace/root keys.
    /// Choices are the complete UI snapshot, including false and unrelated group
    /// entries. They never grant source access. Save them in the policy transaction
    /// without retargeting jobs or clearing service suppression.
    #[cfg(test)]
    pub(crate) fn save_cloud_sync_policy(
        &self,
        account: &str,
        destination: &str,
        resource_origin: &str,
        roots: &[SourceSelection],
        projects: &[ProjectSelection],
        project_choices: &BTreeMap<String, bool>,
    ) -> Result<Policy> {
        self.save_cloud_sync_policy_for_grant(
            (account, destination, resource_origin, resource_origin, None),
            roots,
            projects,
            project_choices,
        )
    }

    /// Create a policy for explicitly confirmed folders. The caller verifies
    /// current authority and passes the exact Team generation and workspace name.
    /// An existing row must never be replaced by a new approval.
    #[cfg(test)]
    pub(crate) fn save_cloud_sync_policy_for_membership(
        &self,
        binding: PolicyBinding<'_>,
        roots: &[SourceSelection],
        projects: &[ProjectSelection],
        project_choices: &BTreeMap<String, bool>,
        current: impl Fn() -> bool,
    ) -> Result<Policy> {
        self.save_cloud_sync_policy_with_repositories(
            binding,
            roots,
            projects,
            project_choices,
            &BTreeMap::new(),
            current,
        )
    }

    /// Save the selected exact folders and verified repository anchors together.
    /// An anchor becomes a repository rule, not an exact-folder grant.
    pub(crate) fn save_cloud_sync_policy_with_repositories(
        &self,
        binding: PolicyBinding<'_>,
        roots: &[SourceSelection],
        projects: &[ProjectSelection],
        project_choices: &BTreeMap<String, bool>,
        repository_anchors: &BTreeMap<String, String>,
        current: impl Fn() -> bool,
    ) -> Result<Policy> {
        self.write_cloud_sync_policy(
            binding,
            roots,
            (projects, repository_anchors),
            project_choices,
            PolicyWrite::Create(&current),
        )
    }

    /// Confirm new folders only after the user turns off all folders under the
    /// exact old Team policy. The new membership must be current at commit.
    /// Old jobs, bytes, and service deletion markers stay under their old bindings.
    /// The `current` check runs under the store lock. It must not call the store.
    #[cfg(test)]
    pub(crate) fn reapprove_cloud_sync_policy_for_membership(
        &self,
        previous: &Policy,
        binding: PolicyBinding<'_>,
        roots: &[SourceSelection],
        projects: &[ProjectSelection],
        project_choices: &BTreeMap<String, bool>,
        current: impl Fn() -> bool,
    ) -> Result<Policy> {
        self.reapprove_cloud_sync_policy_with_repositories(
            previous,
            binding,
            roots,
            (projects, &BTreeMap::new()),
            project_choices,
            current,
        )
    }

    /// Approve new exact folders and repository anchors after all prior Team
    /// grants have been turned off. Keep saved jobs bound to the old generation.
    pub(crate) fn reapprove_cloud_sync_policy_with_repositories(
        &self,
        previous: &Policy,
        binding: PolicyBinding<'_>,
        roots: &[SourceSelection],
        projects_and_repositories: (&[ProjectSelection], &BTreeMap<String, String>),
        project_choices: &BTreeMap<String, bool>,
        current: impl Fn() -> bool,
    ) -> Result<Policy> {
        self.write_cloud_sync_policy(
            binding,
            roots,
            projects_and_repositories,
            project_choices,
            PolicyWrite::Reapprove(previous, &current),
        )
    }

    /// Renew the exact saved Team folders with current membership proof. Add
    /// only the validated new selections. Keep the pause, old scopes and jobs.
    /// The caller verifies the Team and checks current local authority at commit.
    #[cfg(test)]
    pub(crate) fn renew_cloud_team_sync_policy(
        &self,
        binding: PolicyBinding<'_>,
        expected_revision: &str,
        confirmed_paths: &BTreeSet<String>,
        additions: Option<(&BTreeSet<ProjectSelection>, &BTreeMap<String, bool>)>,
        current: impl Fn() -> bool,
    ) -> Result<Policy> {
        let no_repository_anchors = BTreeMap::new();
        self.renew_cloud_team_sync_policy_with_repositories(
            binding,
            expected_revision,
            confirmed_paths,
            additions.map(|(projects, choices)| (projects, choices, &no_repository_anchors)),
            current,
        )
    }

    /// Renew reviewed exact folders and repository rules in one transaction.
    /// `confirmed_paths` includes the saved display anchors, even if they are
    /// missing on disk. Only new repository grants need fresh checkout proof.
    pub(crate) fn renew_cloud_team_sync_policy_with_repositories(
        &self,
        binding: PolicyBinding<'_>,
        expected_revision: &str,
        confirmed_paths: &BTreeSet<String>,
        additions: Option<TeamPolicyAdditions<'_>>,
        current: impl Fn() -> bool,
    ) -> Result<Policy> {
        let (account, destination, resource, issuer, team) = binding;
        let (generation, name) = team
            .filter(|(generation, name)| valid_team_membership(generation, name))
            .ok_or_else(|| anyhow::anyhow!("Current Team membership is required"))?;
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut policy = saved_policy(&transaction, account, destination)?
            .ok_or_else(|| anyhow::anyhow!("No saved Team folders"))?;
        let reviewed_projects = policy.projects_for_display();
        ensure!(
            policy.consent_version == ONGOING_CONSENT_VERSION
                && policy.revision == expected_revision
                && policy.resource_origin.as_deref() == Some(resource)
                && policy.issuer_origin.as_deref() == Some(issuer)
                && valid_resource_origin(resource)
                && valid_resource_origin(issuer)
                && policy.needs_team_membership(generation)
                && !confirmed_paths.is_empty()
                && policy.roots.len() <= MAX_SYNC_SOURCE_ROOTS
                && check_policy_projects(&policy).is_ok()
                && reviewed_projects
                    .iter()
                    .all(|project| valid_project_path(&project.project_path))
                && reviewed_projects
                    .iter()
                    .map(|project| &project.project_path)
                    .collect::<BTreeSet<_>>()
                    == confirmed_paths.iter().collect(),
            "Saved Team folders or membership changed"
        );
        policy.team_grant_id = None;
        policy.membership_generation = Some(generation.to_owned());
        policy.workspace_name = Some(name.to_owned());
        if let Some((projects, choices, repository_anchors)) = additions {
            append_cloud_projects(
                &transaction,
                &mut policy,
                resource,
                projects,
                repository_anchors,
            )?;
            policy.project_choices.extend(choices.clone());
        }
        policy.revision = random_id()?;
        transaction.execute(
            "UPDATE cloud_sync_policy SET record_json=?3 WHERE account_id=?1 AND destination_id=?2",
            params![account, destination, encode(&policy, 524_288)?],
        )?;
        ensure!(current(), "Cloud account or upload controls changed");
        transaction.commit()?;
        Ok(policy)
    }

    /// Replace a policy only when a test needs to simulate an older client.
    #[cfg(test)]
    pub(crate) fn save_cloud_sync_policy_for_grant(
        &self,
        binding: PolicyBinding<'_>,
        roots: &[SourceSelection],
        projects: &[ProjectSelection],
        project_choices: &BTreeMap<String, bool>,
    ) -> Result<Policy> {
        self.write_cloud_sync_policy(
            binding,
            roots,
            (projects, &BTreeMap::new()),
            project_choices,
            PolicyWrite::ReplaceFixture,
        )
    }

    fn write_cloud_sync_policy(
        &self,
        binding: PolicyBinding<'_>,
        roots: &[SourceSelection],
        projects_and_repositories: (&[ProjectSelection], &BTreeMap<String, String>),
        project_choices: &BTreeMap<String, bool>,
        write: PolicyWrite<'_>,
    ) -> Result<Policy> {
        let (projects, repository_anchors) = projects_and_repositories;
        let (account, destination, resource_origin, issuer_origin, team) = binding;
        ensure!(
            valid_resource_origin(resource_origin)
                && valid_resource_origin(issuer_origin)
                && team.is_none_or(|(generation, name)| valid_team_membership(generation, name)),
            "Cloud service origin is invalid"
        );
        ensure!(!roots.is_empty(), "Choose bounded Cloud sources");
        ensure!(!projects.is_empty(), "Choose bounded Cloud projects");
        ensure!(
            roots.len() <= MAX_SYNC_SOURCE_ROOTS,
            "Cloud source roots exceed the policy limit"
        );
        check_project_binding_limits(projects.iter())?;
        ensure!(
            roots.windows(2).all(|pair| pair[0] < pair[1]),
            "Cloud sources must be unique and sorted"
        );
        ensure!(
            projects.windows(2).all(|pair| pair[0] < pair[1]),
            "Cloud projects must be unique and sorted"
        );
        ensure!(
            projects.iter().all(|project| {
                valid_project_path(&project.project_path)
                    && roots.iter().any(|root| {
                        root.source_kind == project.source_kind && root.path == project.source_path
                    })
            }),
            "Choose recorded Cloud projects"
        );
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        match &write {
            PolicyWrite::Create(_) => ensure!(
                saved_policy(&transaction, account, destination)?.is_none_or(|saved| {
                    saved.issuer_origin.is_none()
                        && saved
                            .resource_origin
                            .as_deref()
                            .is_none_or(|origin| origin == resource_origin)
                }),
                "A Cloud folder policy is already saved for this workspace"
            ),
            PolicyWrite::Reapprove(previous, _) => ensure!(
                previous.is_team()
                    && previous.consent_version == ONGOING_CONSENT_VERSION
                    && previous.projects.is_empty()
                    && previous.repository_rules.is_empty()
                    && previous.resource_origin.as_deref() == Some(resource_origin)
                    && previous.issuer_origin.as_deref() == Some(issuer_origin)
                    && team
                        .is_some_and(|(generation, _)| previous.needs_team_membership(generation))
                    && saved_policy(&transaction, account, destination)?.as_ref()
                        == Some(*previous),
                "Confirm the saved Team folders before adding more."
            ),
            #[cfg(test)]
            PolicyWrite::ReplaceFixture => {}
        }
        let mut scoped = Vec::new();
        for root in roots {
            scoped.push(source_scope(
                &transaction,
                account,
                destination,
                resource_origin,
                root,
            )?);
        }
        let (team_grant_id, membership_generation) = match &write {
            #[cfg(test)]
            PolicyWrite::ReplaceFixture => (team.map(|(grant, _)| grant.to_owned()), None),
            _ => (None, team.map(|(generation, _)| generation.to_owned())),
        };
        let mut policy = Policy {
            account_id: account.into(),
            destination_id: destination.into(),
            resource_origin: Some(resource_origin.into()),
            issuer_origin: Some(issuer_origin.into()),
            team_grant_id,
            membership_generation,
            workspace_name: team.map(|(_, name)| name.to_owned()),
            consent_version: ONGOING_CONSENT_VERSION.into(),
            revision: random_id()?,
            roots: scoped,
            projects: projects
                .iter()
                .filter(|project| !repository_anchors.contains_key(&project.project_path))
                .cloned()
                .collect(),
            repository_rules: Vec::new(),
            project_choices: project_choices.clone(),
            paused: false,
        };
        add_cloud_repository_anchors(&mut policy, projects.iter(), repository_anchors)?;
        check_policy_projects(&policy)?;
        transaction.execute(
            "INSERT INTO cloud_sync_policy VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id, destination_id) DO UPDATE SET record_json=excluded.record_json",
            params![account, destination, encode(&policy, 524_288)?],
        )?;
        // The first explicit folder approval can start uploads. Later approvals
        // must not override the saved device preference.
        transaction.execute(
            "INSERT OR IGNORE INTO cloud_device_upload VALUES (1, 1)",
            [],
        )?;
        match write {
            PolicyWrite::Reapprove(_, current) | PolicyWrite::Create(current) => {
                ensure!(current(), "Cloud account or upload controls changed");
            }
            #[cfg(test)]
            PolicyWrite::ReplaceFixture => {}
        }
        transaction.commit()?;
        Ok(policy)
    }

    /// Replace folder and repository grants without changing source scopes, pause,
    /// choices, or jobs. The caller verifies repository and source evidence.
    /// Do not fence an active exact-folder job through this conversion. The user
    /// can still revoke exact consent through the folder control.
    /// The account check runs under the store lock and must not call the store.
    pub(crate) fn replace_cloud_repository_rules(
        &self,
        policy: &Policy,
        rules: Vec<RepositoryRule>,
        projects: Vec<ProjectSelection>,
        current: impl Fn() -> bool,
    ) -> Result<Policy> {
        let resource = policy
            .resource_origin
            .as_deref()
            .filter(|origin| valid_resource_origin(origin))
            .ok_or_else(|| anyhow::anyhow!("Cloud service origin is invalid"))?;
        ensure!(
            policy.consent_version == ONGOING_CONSENT_VERSION
                && policy.has_membership_authority()
                && policy
                    .issuer_origin
                    .as_deref()
                    .is_some_and(valid_resource_origin),
            "Cloud consent is required"
        );
        ensure!(
            !projects.is_empty() || !rules.is_empty(),
            "Choose Cloud projects or repositories"
        );
        ensure!(
            policy.roots.len() <= MAX_SYNC_SOURCE_ROOTS,
            "Cloud source roots exceed the policy limit"
        );
        check_project_binding_limits(projects.iter())?;
        ensure!(
            projects.windows(2).all(|pair| pair[0] < pair[1]),
            "Cloud projects must be unique and sorted"
        );
        ensure!(
            projects.iter().all(|project| {
                valid_project_path(&project.project_path)
                    && policy.roots.iter().any(|root| {
                        root.source_kind == project.source_kind
                            && root.path == project.source_path
                            && valid_project_path(&root.path)
                            && safe_id(&root.source_scope_id)
                    })
            }),
            "Choose recorded Cloud projects"
        );
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        ensure!(
            saved_policy(&transaction, &policy.account_id, &policy.destination_id)?.as_ref()
                == Some(policy),
            "Cloud policy changed"
        );
        let mut saved = policy.clone();
        saved.projects = projects;
        saved.repository_rules = rules;
        check_policy_projects(&saved)?;
        let new_origins: BTreeSet<_> = saved
            .repository_rules
            .iter()
            .filter(|rule| {
                !policy
                    .repository_rules
                    .iter()
                    .any(|old| old.origin == rule.origin && rule.scopes.is_subset(&old.scopes))
            })
            .map(|rule| &rule.origin)
            .collect();
        if !new_origins.is_empty() {
            ensure!(
                policy.project_choices.values().all(|selected| *selected),
                "Cloud folder exclusions need review"
            );
            let mut other = transaction.prepare(
                "SELECT record_json FROM cloud_sync_policy WHERE account_id = ?1 AND destination_id != ?2",
            )?;
            let rows = other
                .query_map(params![policy.account_id, policy.destination_id], |row| {
                    row.get::<_, String>(0)
                })?;
            for row in rows {
                let other: Policy = serde_json::from_str(&row?)?;
                if other.resource_origin != policy.resource_origin
                    || other.issuer_origin != policy.issuer_origin
                {
                    continue;
                }
                ensure!(
                    other.projects.is_empty()
                        && !other
                            .repository_rules
                            .iter()
                            .any(|rule| new_origins.contains(&rule.origin)),
                    "Cloud repository approval conflicts with another workspace"
                );
            }
        }
        if saved.projects != policy.projects {
            ensure!(
                active_cloud_sync_jobs(
                    &transaction,
                    &policy.account_id,
                    &policy.destination_id,
                    resource,
                )?
                .iter()
                .all(|job| job.repository_origin.is_some()
                    || !job.allowed(policy)
                    || job.allowed(&saved)),
                "Cloud exact folders still authorize saved uploads"
            );
        }
        write_cloud_policy_change(&transaction, policy, &mut saved)?;
        ensure!(current(), "Cloud account or upload controls changed");
        transaction.commit()?;
        Ok(saved)
    }

    /// Convert exact approved checkout roots after explicit repository approval.
    /// The caller verifies the anchors and current authority. The replacement
    /// rejects the conversion if it would fence an active exact-folder job.
    pub(crate) fn migrate_cloud_repository_anchors(
        &self,
        policy: &Policy,
        anchors: &BTreeMap<String, String>,
        current: impl Fn() -> bool,
    ) -> Result<Policy> {
        let mut migrated = policy.clone();
        add_cloud_repository_anchors(&mut migrated, policy.projects.iter(), anchors)?;
        migrated
            .projects
            .retain(|project| !anchors.contains_key(&project.project_path));
        self.replace_cloud_repository_rules(
            policy,
            migrated.repository_rules,
            migrated.projects,
            current,
        )
    }

    /// Remove one repository grant by stable group ID. This does not need current
    /// Team membership or checkout access. The caller checks the current account.
    pub(crate) fn revoke_cloud_repository(
        &self,
        policy: &Policy,
        group_id: &str,
        origin: &str,
        current: impl Fn() -> bool,
    ) -> Result<Policy> {
        let rule = policy
            .repository_rule_for_group(group_id)
            .filter(|rule| rule.origin == origin)
            .ok_or_else(|| anyhow::anyhow!("Cloud repository selection changed"))?;
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        ensure!(
            saved_policy(&transaction, &policy.account_id, &policy.destination_id)?.as_ref()
                == Some(policy),
            "Cloud policy changed"
        );
        let mut saved = policy.clone();
        saved.repository_rules.retain(|rule| rule.origin != origin);
        saved.roots.retain(|root| {
            !rule.scopes.contains(&root.source_scope_id)
                || saved.projects.iter().any(|project| {
                    root.source_kind == project.source_kind && root.path == project.source_path
                })
                || saved
                    .repository_rules
                    .iter()
                    .any(|rule| rule.scopes.contains(&root.source_scope_id))
        });
        saved.project_choices.insert(group_id.to_owned(), false);
        write_cloud_policy_change(&transaction, policy, &mut saved)?;
        ensure!(current(), "Cloud account or upload controls changed");
        transaction.commit()?;
        Ok(saved)
    }

    /// Apply a checked group change to the exact current policy in one transaction.
    /// The caller checks group membership and admits any new source before this call.
    /// An OFF change can only remove grants. The scope table retains IDs for re-enable.
    pub(crate) fn set_cloud_sync_project(
        &self,
        policy: &Policy,
        group_id: &str,
        selected: bool,
        projects: &BTreeSet<ProjectSelection>,
        clear_source_hold: bool,
        current: impl Fn() -> bool,
    ) -> Result<Policy> {
        self.set_cloud_sync_project_with_repositories(
            policy,
            group_id,
            selected,
            (projects, &BTreeMap::new()),
            clear_source_hold,
            current,
        )
    }

    /// Add exact folders or one verified repository group. Resolve all new
    /// source scopes before committing the group choice and its grants.
    pub(crate) fn set_cloud_sync_project_with_repositories(
        &self,
        policy: &Policy,
        group_id: &str,
        selected: bool,
        projects_and_repositories: (&BTreeSet<ProjectSelection>, &BTreeMap<String, String>),
        clear_source_hold: bool,
        current: impl Fn() -> bool,
    ) -> Result<Policy> {
        let (projects, repository_anchors) = projects_and_repositories;
        ensure!(
            repository_anchors.is_empty()
                || (selected
                    && repository_anchors
                        .values()
                        .all(|origin| repository_group_id(origin) == group_id)),
            "Cloud repository selection changed"
        );
        let origin = policy
            .resource_origin
            .as_deref()
            .filter(|origin| valid_resource_origin(origin))
            .ok_or_else(|| anyhow::anyhow!("Cloud service origin is invalid"))?;
        policy
            .issuer_origin
            .as_deref()
            .filter(|origin| valid_resource_origin(origin))
            .ok_or_else(|| anyhow::anyhow!("Cloud issuer is invalid"))?;
        ensure!(
            policy.consent_version == ONGOING_CONSENT_VERSION,
            "Cloud consent is required"
        );
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let raw: String = transaction.query_row(
            "SELECT record_json FROM cloud_sync_policy WHERE account_id=?1 AND destination_id=?2",
            params![policy.account_id, policy.destination_id],
            |row| row.get(0),
        )?;
        ensure!(
            serde_json::from_str::<Policy>(&raw)? == *policy,
            "Cloud policy changed"
        );
        let mut saved = policy.clone();
        if selected {
            ensure!(
                policy.has_membership_authority() && !projects.is_empty(),
                "Choose recorded Cloud projects"
            );
            append_cloud_projects(
                &transaction,
                &mut saved,
                origin,
                projects,
                repository_anchors,
            )?;
            if clear_source_hold {
                saved.paused = false;
            }
        } else {
            saved.projects.retain(|project| !projects.contains(project));
            saved.roots.retain(|root| {
                let binds_root = |project: &ProjectSelection| {
                    project.source_kind == root.source_kind && project.source_path == root.path
                };
                !projects.iter().any(binds_root)
                    || saved.projects.iter().any(binds_root)
                    || saved
                        .repository_rules
                        .iter()
                        .any(|rule| rule.scopes.contains(&root.source_scope_id))
            });
        }
        saved.project_choices.insert(group_id.to_owned(), selected);
        write_cloud_policy_change(&transaction, policy, &mut saved)?;
        ensure!(current(), "Cloud account or upload controls changed");
        transaction.commit()?;
        Ok(saved)
    }

    /// Persist pause or explicit retry without changing consent or frozen jobs.
    pub(crate) fn set_cloud_sync_paused(&self, policy: &Policy, paused: bool) -> Result<Policy> {
        let mut saved = self
            .cloud_sync_policy_for_origins(
                &policy.account_id,
                &policy.destination_id,
                policy
                    .resource_origin
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("Confirm Cloud sync for this service"))?,
                policy
                    .issuer_origin
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("Confirm Cloud sync for this issuer"))?,
            )?
            .ok_or_else(|| anyhow::anyhow!("Cloud consent is required"))?;
        ensure!(saved == *policy, "Cloud policy changed");
        saved.paused = paused;
        let connection = self.lock();
        let previous: String = connection.query_row(
            "SELECT record_json FROM cloud_sync_policy WHERE account_id=?1 AND destination_id=?2",
            params![policy.account_id, policy.destination_id],
            |row| row.get(0),
        )?;
        ensure!(
            serde_json::from_str::<Policy>(&previous)? == *policy,
            "Cloud policy changed"
        );
        let changed = connection.execute(
            "UPDATE cloud_sync_policy SET record_json=?3
             WHERE account_id=?1 AND destination_id=?2 AND record_json=?4",
            params![
                saved.account_id,
                saved.destination_id,
                encode(&saved, 524_288)?,
                previous
            ],
        )?;
        ensure!(changed == 1, "Cloud policy changed");
        Ok(saved)
    }

    /// Return bounded active jobs for the original account. Terminal jobs and
    /// jobs with no issuer stay on disk but do not consume active queue admission.
    pub(crate) fn cloud_sync_jobs(
        &self,
        account: &str,
        destination: &str,
        resource_origin: &str,
    ) -> Result<Vec<Job>> {
        active_cloud_sync_jobs(&self.lock(), account, destination, resource_origin)
    }

    /// Keep the accepted fingerprint or service suppression after restart.
    pub(crate) fn cloud_sync_source(
        &self,
        account: &str,
        destination: &str,
        resource_origin: &str,
        source_id: &str,
    ) -> Result<Option<SourceState>> {
        let source = source_in(&self.lock(), account, destination, source_id)?;
        ensure!(
            source
                .as_ref()
                .is_none_or(|source| source.resource_origin.as_deref() == Some(resource_origin)),
            "Cloud accepted source belongs to another service"
        );
        Ok(source)
    }

    /// Find a fixed job for spool recovery without needing a current grant.
    /// The caller must compare the complete identity before changing anything.
    pub(crate) fn cloud_sync_job(&self, capture_id: &str) -> Result<Option<Job>> {
        job_in(&self.lock(), capture_id)
    }

    /// Freeze a new capture job before reservation. At most one active job may own
    /// a source; unchanged and service-suppressed sources do not create jobs.
    #[cfg(test)]
    pub(crate) fn enqueue_cloud_sync(
        &self,
        policy: &Policy,
        source_scope_id: &str,
        project_path: &str,
        source_id: &str,
        observation_key: &str,
    ) -> Result<Option<Job>> {
        self.enqueue_cloud_sync_with_paths(
            policy,
            source_scope_id,
            project_path,
            source_id,
            observation_key,
            None,
        )
    }

    /// Freeze routing evidence and the upload identity in the same transaction
    /// that checks the current policy, active jobs, receipts, and deletion markers.
    pub(crate) fn enqueue_cloud_sync_with_paths(
        &self,
        policy: &Policy,
        source_scope_id: &str,
        project_path: &str,
        source_id: &str,
        observation_key: &str,
        recorded_project_paths: Option<&[String]>,
    ) -> Result<Option<Job>> {
        self.enqueue_cloud_sync_job(
            policy,
            source_scope_id,
            project_path,
            source_id,
            observation_key,
            JobRouting::ExactFolder(recorded_project_paths),
        )
    }

    /// Freeze the verified origin and recorded CWDs for a repository-authorized
    /// job. The caller proves the origin from the recorded paths before admission.
    pub(crate) fn enqueue_cloud_sync_for_repository(
        &self,
        policy: &Policy,
        source_scope_id: &str,
        project_path: &str,
        source_id: &str,
        observation_key: &str,
        repository: (&str, &[String]),
    ) -> Result<Option<Job>> {
        self.enqueue_cloud_sync_job(
            policy,
            source_scope_id,
            project_path,
            source_id,
            observation_key,
            JobRouting::Repository {
                origin: repository.0,
                recorded_paths: repository.1,
            },
        )
    }

    fn enqueue_cloud_sync_job(
        &self,
        policy: &Policy,
        source_scope_id: &str,
        project_path: &str,
        source_id: &str,
        observation_key: &str,
        routing: JobRouting<'_>,
    ) -> Result<Option<Job>> {
        let (repository_origin, recorded_project_paths, allowed) = match routing {
            JobRouting::ExactFolder(paths) => {
                (None, paths, policy.allows(source_scope_id, project_path))
            }
            JobRouting::Repository {
                origin,
                recorded_paths,
            } => (
                Some(origin),
                Some(recorded_paths),
                policy.allows_repository(source_scope_id, origin),
            ),
        };
        ensure!(
            !policy.paused
                && policy.consent_version == ONGOING_CONSENT_VERSION
                && allowed
                && policy
                    .resource_origin
                    .as_deref()
                    .is_some_and(valid_resource_origin),
            "Cloud project is not allowed"
        );
        ensure!(
            policy
                .issuer_origin
                .as_deref()
                .is_some_and(valid_resource_origin),
            "Cloud project is not allowed"
        );
        ensure!(
            source_id.starts_with(&format!("{source_scope_id}/"))
                && source_id.len() <= 512
                && !source_id.chars().any(char::is_control),
            "Cloud source scope changed"
        );
        ensure!(
            !observation_key.is_empty() && observation_key.len() <= 4096,
            "Cloud source observation is invalid"
        );
        let identity = CaptureIdentity {
            capture_id: capture_id()?,
            account_id: policy.account_id.clone(),
            destination_id: policy.destination_id.clone(),
            source_id: source_id.into(),
        };
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        ensure!(
            device_upload_enabled(&transaction)?,
            "Cloud uploads are paused on this device"
        );
        ensure!(
            saved_policy(&transaction, &policy.account_id, &policy.destination_id)?.as_ref()
                == Some(policy),
            "Cloud policy changed"
        );
        if let Some(source) = source_in(
            &transaction,
            &identity.account_id,
            &identity.destination_id,
            &identity.source_id,
        )? {
            ensure!(
                source.resource_origin == policy.resource_origin,
                "Cloud source service changed"
            );
            if source.suppressed || source.observation_key.as_deref() == Some(observation_key) {
                return Ok(None);
            }
        }
        let mut active = 0;
        {
            // Filter before LIMIT so legacy rows cannot hide jobs that can still
            // resume under their original issuer. Do not rewrite legacy records.
            let mut statement = transaction.prepare(
                "SELECT record_json FROM cloud_sync_job
                 WHERE json_extract(record_json, '$.issuer_origin') IS NOT NULL
                 AND json_extract(record_json, '$.state') NOT IN ('accepted', 'suppressed', 'abandoned')
                 LIMIT 129"
            )?;
            for raw in statement.query_map([], |row| row.get::<_, String>(0))? {
                let job: Job = serde_json::from_str(&raw?)?;
                job.validate()?;
                if !job.state.terminal() {
                    active += 1;
                    if job.identity.account_id == identity.account_id
                        && job.identity.destination_id == identity.destination_id
                        && job.identity.source_id == identity.source_id
                        && job.same_membership_generation(policy)
                    {
                        return Ok(None);
                    }
                }
            }
        }
        ensure!(active < MAX_ACTIVE_JOBS, "Cloud queue is full");
        let count: u64 =
            transaction.query_row("SELECT count(*) FROM cloud_sync_job", [], |row| row.get(0))?;
        ensure!(count < MAX_JOBS, "Cloud job storage is full");
        let job = Job {
            identity,
            resource_origin: policy.resource_origin.clone(),
            issuer_origin: policy.issuer_origin.clone(),
            team_grant_id: None,
            membership_generation: policy.membership_generation.clone(),
            source_scope_id: source_scope_id.into(),
            project_path: project_path.into(),
            project_folders: if repository_origin.is_some() {
                Vec::new()
            } else {
                policy
                    .projects
                    .iter()
                    .filter(|project| {
                        Path::new(project_path).starts_with(&project.project_path)
                            && policy.roots.iter().any(|root| {
                                root.source_scope_id == source_scope_id
                                    && root.source_kind == project.source_kind
                                    && root.path == project.source_path
                            })
                    })
                    .map(|project| project.project_path.clone())
                    .collect()
            },
            repository_origin: repository_origin.map(str::to_owned),
            recorded_project_paths: recorded_project_paths.map(<[String]>::to_vec),
            observation_key: observation_key.into(),
            state: JobState::Pending,
            reservation_retry_at: None,
        };
        job.validate()?;
        transaction.execute(
            "INSERT INTO cloud_sync_job VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                job.identity.capture_id,
                job.identity.account_id,
                job.identity.destination_id,
                job.identity.source_id,
                encode(&job, 16_384)?
            ],
        )?;
        transaction.commit()?;
        Ok(Some(job))
    }

    /// Update host progress only under the job's fixed selection.
    pub(crate) fn set_cloud_sync_job_state(&self, job: &Job, state: JobState) -> Result<()> {
        self.save_cloud_sync_job_state(job, state, None, None)
    }

    /// Clear capacity-only retry permission and save the verified observation
    /// before reservation. The caller must first check this exact saved source.
    pub(crate) fn begin_cloud_sync_reservation_retry(
        &self,
        job: &Job,
        observation: &str,
    ) -> Result<()> {
        ensure!(
            matches!(
                job.state,
                JobState::WaitingForCapacity | JobState::WaitingForLocalCapacity
            ),
            "Cloud capture is not waiting for capacity"
        );
        self.save_cloud_sync_job_state(job, JobState::Pending, None, Some(observation))
    }

    /// Save only a proved capacity refusal. The host owns the refusal evidence;
    /// old empty or interrupted entries do not establish it.
    pub(crate) fn defer_cloud_sync_reservation(&self, job: &Job, retry_at: i64) -> Result<()> {
        self.save_cloud_sync_job_state(job, JobState::WaitingForCapacity, Some(retry_at), None)
    }

    /// Save a proved local-capacity refusal before any capture entry or request.
    pub(crate) fn defer_cloud_sync_local_capture(&self, job: &Job, retry_at: i64) -> Result<()> {
        self.save_cloud_sync_job_state(job, JobState::WaitingForLocalCapacity, Some(retry_at), None)
    }

    /// Compare the complete prior job before changing progress or retry permission.
    fn save_cloud_sync_job_state(
        &self,
        job: &Job,
        state: JobState,
        reservation_retry_at: Option<i64>,
        observation: Option<&str>,
    ) -> Result<()> {
        ensure!(!state.terminal(), "Use the terminal outcome transaction");
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut saved = job_in(&transaction, &job.identity.capture_id)?
            .ok_or_else(|| anyhow::anyhow!("Cloud job is unavailable"))?;
        ensure!(
            saved == *job && !saved.state.terminal(),
            "Cloud job changed"
        );
        saved.state = state;
        saved.reservation_retry_at = reservation_retry_at;
        if let Some(observation) = observation {
            saved.observation_key = observation.into();
        }
        saved.validate()?;
        transaction.execute(
            "UPDATE cloud_sync_job SET record_json=?2 WHERE capture_id=?1",
            params![saved.identity.capture_id, encode(&saved, 16_384)?],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Finish a saved empty-capture recovery decision. The caller first saves
    /// `Recovering`, then uses the spool's checked empty cancellation.
    /// Keep the old ID terminal and leave every accepted fingerprint unchanged.
    pub(crate) fn abandon_cloud_sync_preparing(&self, job: &Job) -> Result<()> {
        self.abandon_cloud_sync_job(job, JobState::Recovering)
    }

    /// Retire a fresh job only after the spool proves a pin mismatch before it
    /// created an entry or reserved storage. Do not use for reservation retries.
    pub(crate) fn abandon_cloud_sync_unreserved(&self, job: &Job) -> Result<()> {
        self.abandon_cloud_sync_job(job, JobState::Pending)
    }

    fn abandon_cloud_sync_job(&self, job: &Job, expected: JobState) -> Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut saved = job_in(&transaction, &job.identity.capture_id)?
            .ok_or_else(|| anyhow::anyhow!("Cloud job is unavailable"))?;
        ensure!(
            saved == *job && saved.state == expected,
            "Cloud recovery state changed"
        );
        saved.state = JobState::Abandoned;
        saved.reservation_retry_at = None;
        transaction.execute(
            "UPDATE cloud_sync_job SET record_json=?2 WHERE capture_id=?1",
            params![saved.identity.capture_id, encode(&saved, 16_384)?],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Persist a verified service suppression before host progress can change.
    /// Keep the spool and any accepted fingerprint. Only the fixed job's service
    /// can establish this outcome; generic conflicts cannot call this method.
    pub(crate) fn suppress_cloud_sync(&self, job: &Job) -> Result<()> {
        ensure!(
            job.resource_origin
                .as_deref()
                .is_some_and(valid_resource_origin),
            "Cloud service binding is required"
        );
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut saved = job_in(&transaction, &job.identity.capture_id)?
            .ok_or_else(|| anyhow::anyhow!("Cloud job is unavailable"))?;
        ensure!(
            saved == *job && !saved.state.terminal(),
            "Cloud job changed"
        );
        let previous = source_in(
            &transaction,
            &job.identity.account_id,
            &job.identity.destination_id,
            &job.identity.source_id,
        )?;
        ensure!(
            previous
                .as_ref()
                .is_none_or(|source| source.resource_origin == job.resource_origin),
            "Cloud source service changed"
        );
        let count: u64 =
            transaction.query_row("SELECT count(*) FROM cloud_sync_source", [], |row| {
                row.get(0)
            })?;
        ensure!(
            previous.is_some() || count < MAX_SOURCES,
            "Cloud source storage is full"
        );
        let mut source = previous.unwrap_or(SourceState {
            resource_origin: job.resource_origin.clone(),
            fingerprint: None,
            observation_key: None,
            suppressed: false,
        });
        source.suppressed = true;
        saved.state = JobState::Suppressed;
        saved.reservation_retry_at = None;
        transaction.execute(
            "INSERT INTO cloud_sync_source VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, destination_id, source_id) DO UPDATE SET record_json=excluded.record_json",
            params![job.identity.account_id, job.identity.destination_id, job.identity.source_id, encode(&source, 16_384)?],
        )?;
        transaction.execute(
            "UPDATE cloud_sync_job SET record_json=?2 WHERE capture_id=?1",
            params![saved.identity.capture_id, encode(&saved, 16_384)?],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Save the accepted fingerprint and terminal job in the same transaction.
    /// Only after this succeeds may the caller acknowledge the spool receipt.
    /// This local commit needs no live grant; verified acceptance survives logout.
    pub(crate) fn accept_cloud_sync(&self, entry: &QueueEntry) -> Result<()> {
        let fingerprint = entry
            .accepted_source_fingerprint()
            .ok_or_else(|| anyhow::anyhow!("Cloud integrity acceptance is required"))?;
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut job = job_in(&transaction, &entry.identity.capture_id)?
            .ok_or_else(|| anyhow::anyhow!("Cloud job is unavailable"))?;
        ensure!(
            job.identity == entry.identity
                && !matches!(job.state, JobState::Suppressed | JobState::Abandoned),
            "Cloud job binding changed"
        );
        if job.state == JobState::Accepted {
            ensure!(
                source_in(
                    &transaction,
                    &entry.identity.account_id,
                    &entry.identity.destination_id,
                    &entry.identity.source_id
                )?
                .is_some_and(|source| source.resource_origin == job.resource_origin
                    && source.fingerprint.as_ref() == Some(&fingerprint)),
                "Cloud accepted source changed"
            );
            return Ok(());
        }
        let previous = source_in(
            &transaction,
            &entry.identity.account_id,
            &entry.identity.destination_id,
            &entry.identity.source_id,
        )?;
        ensure!(
            previous
                .as_ref()
                .is_none_or(|source| source.resource_origin == job.resource_origin),
            "Cloud accepted source service changed"
        );
        let count: u64 =
            transaction.query_row("SELECT count(*) FROM cloud_sync_source", [], |row| {
                row.get(0)
            })?;
        ensure!(
            previous.is_some() || count < MAX_SOURCES,
            "Cloud source storage is full"
        );
        let source = SourceState {
            resource_origin: job.resource_origin.clone(),
            fingerprint: Some(fingerprint),
            observation_key: Some(
                entry
                    .manifest
                    .as_ref()
                    .and_then(|manifest| {
                        // SQLite capture can read a later snapshot than discovery.
                        manifest.streams.iter().find_map(|stream| {
                            let prefix = match stream.stream_id.as_str() {
                                "native-rows" => "opencode-native-v1",
                                "cursor-ide-rows" => "cursor-ide-native-v1",
                                _ => return None,
                            };
                            Some(format!("{prefix}:{}", stream.expanded_sha256))
                        })
                    })
                    .unwrap_or_else(|| job.observation_key.clone()),
            ),
            suppressed: previous.is_some_and(|source| source.suppressed),
        };
        transaction.execute(
            "INSERT INTO cloud_sync_source VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, destination_id, source_id) DO UPDATE SET record_json=excluded.record_json",
            params![entry.identity.account_id, entry.identity.destination_id, entry.identity.source_id, encode(&source, 16_384)?],
        )?;
        job.state = JobState::Accepted;
        job.reservation_retry_at = None;
        transaction.execute(
            "UPDATE cloud_sync_job SET record_json=?2 WHERE capture_id=?1",
            params![job.identity.capture_id, encode(&job, 16_384)?],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Count durable terminal outcomes without exposing native source identities.
    pub(crate) fn cloud_sync_outcome_counts(
        &self,
        account: &str,
        destination: &str,
        resource_origin: &str,
    ) -> Result<(u32, u32)> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT record_json FROM cloud_sync_source WHERE account_id=?1 AND destination_id=?2
             AND json_extract(record_json, '$.resource_origin')=?3",
        )?;
        let (mut accepted, mut suppressed) = (0, 0);
        for raw in statement.query_map(params![account, destination, resource_origin], |row| {
            row.get::<_, String>(0)
        })? {
            let source: SourceState = serde_json::from_str(&raw?)?;
            accepted += u32::from(source.fingerprint.is_some());
            suppressed += u32::from(source.suppressed);
        }
        Ok((accepted, suppressed))
    }
}
use crate::cloud::repository_origin::valid_repository_origin_label;
