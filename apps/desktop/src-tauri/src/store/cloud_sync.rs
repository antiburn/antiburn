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
const MAX_SOURCES: u64 = 16_384;
const MAX_JOBS: u64 = 32_768;

type PolicyBinding<'a> = (
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    Option<(&'a str, &'a str)>,
);

enum PolicyWrite<'a> {
    Create(&'a dyn Fn() -> bool),
    Reapprove(&'a Policy, &'a dyn Fn() -> bool),
    #[cfg(test)]
    ReplaceFixture,
}

fn device_upload_enabled(connection: &Connection) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT enabled FROM cloud_device_upload WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false))
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
            && policy.allows(&self.source_scope_id, &self.project_path)
            && self
                .project_folders
                .iter()
                .all(|folder| policy.allows(&self.source_scope_id, folder))
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
                && self.project_folders.len() <= 100
                && self
                    .project_folders
                    .iter()
                    .all(|path| valid_project_path(path))
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
        Ok(policy)
    })
    .transpose()
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

fn append_cloud_projects(
    transaction: &Transaction<'_>,
    policy: &mut Policy,
    origin: &str,
    projects: &BTreeSet<ProjectSelection>,
) -> Result<()> {
    ensure!(
        projects
            .iter()
            .all(|project| valid_project_path(&project.project_path)),
        "Choose recorded Cloud projects"
    );
    policy.projects.extend(projects.iter().cloned());
    policy.projects.sort();
    policy.projects.dedup();
    ensure!(
        policy.projects.len() <= 100,
        "Choose bounded Cloud projects"
    );
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
            ensure!(policy.roots.len() < 100, "Choose bounded Cloud sources");
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

/// The source observation key and whether the user asked to re-offer it.
pub(crate) enum SourceObservation<'a> {
    ChangedOnly(&'a str),
    Backfill(&'a str),
}

impl Store {
    /// Read the switch for this device profile. No saved preference means the
    /// user has not approved any folders or used the device switch.
    pub(crate) fn cloud_device_upload_enabled(&self) -> Result<bool> {
        device_upload_enabled(&self.lock())
    }

    /// Persist the device switch without changing a policy, job, or spool.
    pub(crate) fn set_cloud_device_upload_enabled(&self, enabled: bool) -> Result<()> {
        self.lock().execute(
            "INSERT INTO cloud_device_upload (id, enabled) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET enabled=excluded.enabled",
            [enabled],
        )?;
        Ok(())
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
                    && policy.projects.len() <= 100
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
    pub(crate) fn save_cloud_sync_policy_for_membership(
        &self,
        binding: PolicyBinding<'_>,
        roots: &[SourceSelection],
        projects: &[ProjectSelection],
        project_choices: &BTreeMap<String, bool>,
        current: impl Fn() -> bool,
    ) -> Result<Policy> {
        self.write_cloud_sync_policy(
            binding,
            roots,
            projects,
            project_choices,
            PolicyWrite::Create(&current),
        )
    }

    /// Confirm new folders only after the user turns off all folders under the
    /// exact old Team policy. The new membership must be current at commit.
    /// Old jobs, bytes, and service deletion markers stay under their old bindings.
    /// The `current` check runs under the store lock. It must not call the store.
    pub(crate) fn reapprove_cloud_sync_policy_for_membership(
        &self,
        previous: &Policy,
        binding: PolicyBinding<'_>,
        roots: &[SourceSelection],
        projects: &[ProjectSelection],
        project_choices: &BTreeMap<String, bool>,
        current: impl Fn() -> bool,
    ) -> Result<Policy> {
        self.write_cloud_sync_policy(
            binding,
            roots,
            projects,
            project_choices,
            PolicyWrite::Reapprove(previous, &current),
        )
    }

    /// Renew the exact saved Team folders with current membership proof. Add
    /// only the validated new selections. Keep the pause, old scopes and jobs.
    /// The caller verifies the Team and checks current local authority at commit.
    pub(crate) fn renew_cloud_team_sync_policy(
        &self,
        binding: PolicyBinding<'_>,
        expected_revision: &str,
        confirmed_paths: &BTreeSet<String>,
        additions: Option<(&BTreeSet<ProjectSelection>, &BTreeMap<String, bool>)>,
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
        ensure!(
            policy.consent_version == ONGOING_CONSENT_VERSION
                && policy.revision == expected_revision
                && policy.resource_origin.as_deref() == Some(resource)
                && policy.issuer_origin.as_deref() == Some(issuer)
                && valid_resource_origin(resource)
                && valid_resource_origin(issuer)
                && policy.needs_team_membership(generation)
                && !confirmed_paths.is_empty()
                && policy.projects.len() <= 100
                && policy
                    .projects
                    .iter()
                    .all(|project| valid_project_path(&project.project_path))
                && policy
                    .projects
                    .iter()
                    .map(|project| &project.project_path)
                    .collect::<BTreeSet<_>>()
                    == confirmed_paths.iter().collect(),
            "Saved Team folders or membership changed"
        );
        policy.team_grant_id = None;
        policy.membership_generation = Some(generation.to_owned());
        policy.workspace_name = Some(name.to_owned());
        if let Some((projects, choices)) = additions {
            append_cloud_projects(&transaction, &mut policy, resource, projects)?;
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
            projects,
            project_choices,
            PolicyWrite::ReplaceFixture,
        )
    }

    fn write_cloud_sync_policy(
        &self,
        binding: PolicyBinding<'_>,
        roots: &[SourceSelection],
        projects: &[ProjectSelection],
        project_choices: &BTreeMap<String, bool>,
        write: PolicyWrite<'_>,
    ) -> Result<Policy> {
        let (account, destination, resource_origin, issuer_origin, team) = binding;
        ensure!(
            valid_resource_origin(resource_origin)
                && valid_resource_origin(issuer_origin)
                && team.is_none_or(|(generation, name)| valid_team_membership(generation, name)),
            "Cloud service origin is invalid"
        );
        ensure!(
            !roots.is_empty() && roots.len() <= 100,
            "Choose bounded Cloud sources"
        );
        ensure!(
            !projects.is_empty() && projects.len() <= 100,
            "Choose bounded Cloud projects"
        );
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
        let policy = Policy {
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
            projects: projects.to_vec(),
            project_choices: project_choices.clone(),
            paused: false,
        };
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
            append_cloud_projects(&transaction, &mut saved, origin, projects)?;
            if clear_source_hold {
                saved.paused = false;
            }
        } else {
            saved.projects.retain(|project| !projects.contains(project));
            saved.roots.retain(|root| {
                let binds_root = |project: &ProjectSelection| {
                    project.source_kind == root.source_kind && project.source_path == root.path
                };
                !projects.iter().any(binds_root) || saved.projects.iter().any(binds_root)
            });
        }
        saved.project_choices.insert(group_id.to_owned(), selected);
        if saved != *policy {
            saved.revision = random_id()?;
            transaction.execute(
                "UPDATE cloud_sync_policy SET record_json=?3 WHERE account_id=?1 AND destination_id=?2",
                params![saved.account_id, saved.destination_id, encode(&saved, 524_288)?],
            )?;
        }
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
        let connection = self.lock();
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
            SourceObservation::ChangedOnly(observation_key),
            None,
        )
    }

    /// Freeze routing evidence and the upload identity in the same transaction
    /// that checks the current policy, existing active jobs, and suppression.
    pub(crate) fn enqueue_cloud_sync_with_paths(
        &self,
        policy: &Policy,
        source_scope_id: &str,
        project_path: &str,
        source_id: &str,
        observation: SourceObservation<'_>,
        recorded_project_paths: Option<&[String]>,
    ) -> Result<Option<Job>> {
        let (observation_key, changed_only) = match observation {
            SourceObservation::ChangedOnly(key) => (key, true),
            SourceObservation::Backfill(key) => (key, false),
        };
        ensure!(
            !policy.paused
                && policy.consent_version == ONGOING_CONSENT_VERSION
                && policy.allows(source_scope_id, project_path)
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
            if source.suppressed
                || (changed_only && source.observation_key.as_deref() == Some(observation_key))
            {
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
            project_folders: policy
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
                .collect(),
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
        self.save_cloud_sync_job_state(job, state, None)
    }

    /// Save only a proved capacity refusal. The host owns the refusal evidence;
    /// old empty or interrupted entries do not establish it.
    pub(crate) fn defer_cloud_sync_reservation(&self, job: &Job, retry_at: i64) -> Result<()> {
        self.save_cloud_sync_job_state(job, JobState::WaitingForCapacity, Some(retry_at))
    }

    /// Save a proved local-capacity refusal before any capture entry or request.
    pub(crate) fn defer_cloud_sync_local_capture(&self, job: &Job, retry_at: i64) -> Result<()> {
        self.save_cloud_sync_job_state(job, JobState::WaitingForLocalCapacity, Some(retry_at))
    }

    /// Compare the complete prior job before changing progress or retry permission.
    fn save_cloud_sync_job_state(
        &self,
        job: &Job,
        state: JobState,
        reservation_retry_at: Option<i64>,
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
        saved.validate()?;
        transaction.execute(
            "UPDATE cloud_sync_job SET record_json=?2 WHERE capture_id=?1",
            params![saved.identity.capture_id, encode(&saved, 16_384)?],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Finish an explicitly requested empty-capture recovery. The caller first
    /// saves `Recovering`, then uses the spool's checked empty cancellation.
    /// Keep the old ID terminal and leave every accepted fingerprint unchanged.
    pub(crate) fn abandon_cloud_sync_preparing(&self, job: &Job) -> Result<()> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let mut saved = job_in(&transaction, &job.identity.capture_id)?
            .ok_or_else(|| anyhow::anyhow!("Cloud job is unavailable"))?;
        ensure!(
            saved == *job && saved.state == JobState::Recovering,
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
                        manifest
                            .streams
                            .iter()
                            .find(|stream| stream.stream_id == "native-rows")
                    })
                    .map(|stream| format!("opencode-native-v1:{}", stream.expanded_sha256))
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
