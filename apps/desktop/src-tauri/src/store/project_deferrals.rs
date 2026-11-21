use antiburn_local::repositories::{DeferredProtectedPath, repo_root_identity};
use serde::{Deserialize, Serialize};

use super::*;

const OWNED_DEFERRALS_KEY: &str = "internal:projectDeferralsV1";
const MAX_OBSERVATIONS: usize = 4096;

#[derive(Serialize, Deserialize)]
pub(crate) struct RepositoryDeferral {
    pub path: DeferredProtectedPath,
    pub source: RepositoryDeferralSource,
}

#[derive(Serialize, Deserialize)]
pub(crate) enum RepositoryDeferralSource {
    SessionCwd(String),
    ScanRoot,
}

#[derive(Serialize, Deserialize)]
struct SessionDeferral {
    key: SessionKey,
    incarnation: u64,
    path: DeferredProtectedPath,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct ProjectDeferrals {
    sessions: Vec<SessionDeferral>,
    repositories: Vec<RepositoryDeferral>,
}

impl ProjectDeferrals {
    pub(super) fn read(connection: &Connection) -> Result<Option<Self>> {
        Ok(connection
            .query_row(
                "SELECT value FROM setting WHERE key = ?1",
                [OWNED_DEFERRALS_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|value| serde_json::from_str(&value).ok()))
    }

    pub(super) fn write(&self, connection: &Connection) -> Result<()> {
        connection.execute(
            "INSERT INTO setting (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value
               WHERE setting.value <> excluded.value",
            params![OWNED_DEFERRALS_KEY, serde_json::to_string(self)?],
        )?;
        write_deferred_permission_dirs_in(connection, &self.summaries())?;
        Ok(())
    }

    pub(super) fn update_sessions(
        &mut self,
        changes: HashMap<SessionKey, (Incarnation, Option<DeferredProtectedPath>)>,
    ) -> bool {
        let previous_count = self.sessions.len();
        self.sessions.retain(|entry| {
            !changes
                .get(&entry.key)
                .is_some_and(|(incarnation, _)| incarnation.0 == entry.incarnation)
        });
        let mut additions: Vec<_> = changes
            .into_iter()
            .filter_map(|(key, (incarnation, path))| {
                let path = path?;
                (key.environment_key == "native").then_some(SessionDeferral {
                    key,
                    incarnation: incarnation.0,
                    path,
                })
            })
            .collect();
        additions.sort_by(|left, right| left.key.cmp(&right.key));
        let changed = !additions.is_empty() || self.sessions.len() != previous_count;
        self.sessions.extend(additions);
        if self.sessions.len() > MAX_OBSERVATIONS {
            self.sessions
                .drain(..self.sessions.len() - MAX_OBSERVATIONS);
        }
        changed
    }

    pub(super) fn replace_repositories(&mut self, observations: Vec<RepositoryDeferral>) {
        self.repositories = observations;
    }

    pub(super) fn reconcile(&mut self, connection: &Connection) -> Result<()> {
        if self.sessions.is_empty() && self.repositories.is_empty() {
            return Ok(());
        }
        let ignored = ignored_repository_paths_in(connection)?;
        let granted = read_granted_dirs_in(connection)?;
        let admitted = |path: &DeferredProtectedPath| {
            !granted.contains(&path.protected_dir)
                && !crate::repositories::project_path_is_ignored(&ignored, &path.cwd)
        };
        self.sessions.retain(|entry| admitted(&entry.path));
        self.repositories.retain(|entry| {
            admitted(&entry.path)
                && match &entry.source {
                    RepositoryDeferralSource::SessionCwd(cwd) => {
                        !crate::repositories::project_path_is_ignored(&ignored, cwd)
                    }
                    RepositoryDeferralSource::ScanRoot => true,
                }
        });

        if !self.sessions.is_empty() {
            let live: HashSet<usize> = connection
                .prepare(
                    "SELECT CAST(entry.key AS INTEGER), cwd FROM json_each(?1) entry
                       CROSS JOIN session ON environment_key = 'native'
                        AND agent = json_extract(entry.value, '$.key.agent')
                        AND session_id = json_extract(entry.value, '$.key.session_id')
                        AND incarnation = json_extract(entry.value, '$.incarnation')",
                )?
                .query_map([serde_json::to_string(&self.sessions)?], |row| {
                    Ok((row.get(0)?, row.get::<_, Option<String>>(1)?))
                })?
                .filter_map(|result| match result {
                    Ok((index, cwd))
                        if cwd.as_deref().is_none_or(|cwd| {
                            !crate::repositories::project_path_is_ignored(&ignored, cwd)
                        }) =>
                    {
                        Some(Ok(index))
                    }
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect::<rusqlite::Result<_>>()?;
            let mut index = 0;
            self.sessions.retain(|_| {
                let keep = live.contains(&index);
                index += 1;
                keep
            });
        }

        if !self.repositories.is_empty() {
            let mut seen = HashSet::new();
            self.repositories.retain(|entry| {
                let source = match &entry.source {
                    RepositoryDeferralSource::SessionCwd(cwd) => Some(cwd.as_str()),
                    RepositoryDeferralSource::ScanRoot => None,
                };
                seen.insert((
                    entry.path.protected_dir.clone(),
                    repo_root_identity(Path::new(&entry.path.cwd)),
                    source.map(str::to_owned),
                ))
            });
            self.repositories.truncate(MAX_OBSERVATIONS);
            let mut missing_cwds = HashSet::new();
            let mut missing_roots = HashSet::new();
            for entry in &self.repositories {
                match &entry.source {
                    RepositoryDeferralSource::SessionCwd(cwd) => {
                        missing_cwds.insert(cwd.clone());
                    }
                    RepositoryDeferralSource::ScanRoot => {
                        missing_roots.insert(entry.path.cwd.clone());
                    }
                }
            }
            if !missing_cwds.is_empty() {
                let mut statement = connection.prepare(
                    "SELECT cwd FROM session WHERE environment_key = 'native' AND cwd IS NOT NULL",
                )?;
                let mut rows = statement.query([])?;
                while !missing_cwds.is_empty() {
                    let Some(row) = rows.next()? else { break };
                    missing_cwds.remove(row.get_ref(0)?.as_str()?);
                }
            }
            if !missing_roots.is_empty() {
                let mut statement = connection.prepare("SELECT path FROM scan_root")?;
                let mut rows = statement.query([])?;
                while !missing_roots.is_empty() {
                    let Some(row) = rows.next()? else { break };
                    missing_roots.remove(row.get_ref(0)?.as_str()?);
                }
            }
            self.repositories.retain(|entry| match &entry.source {
                RepositoryDeferralSource::SessionCwd(cwd) => !missing_cwds.contains(cwd),
                RepositoryDeferralSource::ScanRoot => !missing_roots.contains(&entry.path.cwd),
            });
        }

        Ok(())
    }

    fn summaries(&self) -> Vec<DeferredPermissionDir> {
        crate::repositories::summarize_deferred(
            self.sessions
                .iter()
                .map(|entry| &entry.path)
                .chain(self.repositories.iter().map(|entry| &entry.path)),
        )
    }
}

pub(super) fn reconcile_stored(connection: &Connection) -> Result<()> {
    if let Some(mut observations) = ProjectDeferrals::read(connection)? {
        observations.reconcile(connection)?;
        observations.write(connection)?;
    } else {
        // Legacy counts do not identify the paths that caused the requests.
        connection.execute(
            "DELETE FROM setting WHERE key = ?1",
            [DEFERRED_PERMISSION_DIRS_KEY],
        )?;
    }
    Ok(())
}
