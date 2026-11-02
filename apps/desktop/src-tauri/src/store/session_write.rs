//! Session write guards, source bindings, and indexed title fences.

use super::*;
use std::sync::atomic::AtomicBool;

/// Evidence captured before one source read. A later key delete, global clear,
/// newer attempt, or changed source binding rejects that attempt's commit.
#[derive(Debug)]
pub struct SessionWriteGuard {
    pub(super) key: SessionKey,
    pub(super) source_label: String,
    pub(super) physical_source: Option<String>,
    pub(super) stored_binding: Option<SessionSourceBinding>,
    pub(super) global_fence: u64,
    pub(super) key_fence: u64,
    pub(super) attempt: u64,
    pub(super) completed: AtomicBool,
    pub(super) state: Arc<Mutex<SessionWriteState>>,
}

impl SessionWriteGuard {
    /// Identify the exact attempt that produced a commit outcome.
    pub fn attempt_id(&self) -> u64 {
        self.attempt
    }
}

impl Drop for SessionWriteGuard {
    fn drop(&mut self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.attempts.get(&self.key).copied() == Some(self.attempt) {
            state.attempts.remove(&self.key);
            state.key_fences.remove(&self.key);
        }
    }
}

/// Why a guarded session write did not publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCommitRejection {
    GlobalFenceChanged,
    Deleted,
    Superseded,
    SourceChanged,
}

#[derive(Debug, Default)]
pub(super) struct SessionWriteState {
    pub(super) key_fences: HashMap<SessionKey, u64>,
    pub(super) attempts: HashMap<SessionKey, u64>,
    pub(super) next_attempt: u64,
    pub(super) physical_sources: HashMap<SessionKey, SessionPhysicalSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionPhysicalSource {
    pub(super) incarnation: Incarnation,
    pub(super) source_kind: String,
    pub(super) source_label: String,
    pub(super) physical_source: String,
    pub(super) attempt: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionSourceBinding {
    pub(super) incarnation: Incarnation,
    pub(super) source_kind: String,
    pub(super) source_label: String,
    pub(super) physical_source: Option<SessionPhysicalSource>,
}

/// Identity and write fences captured before an indexed title lookup.
#[derive(Debug, Clone)]
pub(crate) struct SessionTitleRefreshGuard {
    pub(crate) key: SessionKey,
    pub(super) binding: SessionSourceBinding,
    pub(super) global_fence: u64,
    pub(super) key_fence: u64,
}

impl Store {
    /// Capture the clear, deletion, and policy fence before discovery.
    pub fn capture_destructive_write_epoch(&self) -> u64 {
        let _connection = self.lock();
        self.destructive_write_epoch.load(Ordering::Acquire)
    }

    /// Begin a source read for one session and bind its result to the source
    /// label observed by discovery. A newer attempt for the same key supersedes it.
    pub fn begin_session_write(
        &self,
        key: &SessionKey,
        source_label: &str,
    ) -> Result<SessionWriteGuard> {
        self.begin_session_write_with_source(
            self.capture_destructive_write_epoch(),
            key,
            source_label,
            None,
        )
    }

    /// Begin a source read against a scan's destructive-write snapshot.
    pub fn begin_session_write_at(
        &self,
        global_fence: u64,
        key: &SessionKey,
        source_label: &str,
    ) -> Result<SessionWriteGuard> {
        self.begin_session_write_with_source(global_fence, key, source_label, None)
    }

    /// Bind one dispatch to its discovery epoch and physical source identity.
    /// The caller includes the source kind and full path, including database paths.
    /// Bindings last for this Store writer's lifetime; no attempts survive restart.
    pub fn begin_session_write_for_source_at(
        &self,
        epoch: u64,
        key: &SessionKey,
        source_label: &str,
        physical_source: &str,
    ) -> Result<SessionWriteGuard> {
        self.begin_session_write_with_source(epoch, key, source_label, Some(physical_source))
    }

    pub(super) fn begin_session_write_with_source(
        &self,
        epoch: u64,
        key: &SessionKey,
        source_label: &str,
        physical_source: Option<&str>,
    ) -> Result<SessionWriteGuard> {
        let connection = self.lock();
        let mut state = self
            .session_write_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let stored_binding = session_source_binding_in(&connection, &state, key)?;
        state.next_attempt = state
            .next_attempt
            .checked_add(1)
            .context("session write attempt sequence exhausted")?;
        let attempt = state.next_attempt;
        if self.destructive_write_epoch.load(Ordering::Acquire) == epoch {
            state.attempts.insert(key.clone(), attempt);
        }
        Ok(SessionWriteGuard {
            key: key.clone(),
            source_label: source_label.to_owned(),
            physical_source: physical_source.map(str::to_owned),
            stored_binding,
            global_fence: epoch,
            key_fence: state.key_fences.get(key).copied().unwrap_or(0),
            attempt,
            completed: AtomicBool::new(false),
            state: Arc::clone(&self.session_write_state),
        })
    }

    /// Publish one session only while its source read remains current.
    pub fn upsert_session_if_current(
        &self,
        guard: &SessionWriteGuard,
        record: &SessionRecord,
        evidence_agents: &[&str],
    ) -> Result<SessionCommitOutcome> {
        let mut connection = self.lock();
        if guard.key != record.key || guard.source_label != record.source_label {
            return Ok(SessionCommitOutcome::Rejected(
                SessionCommitRejection::SourceChanged,
            ));
        }
        if let Some(rejection) = self.session_write_rejection(&connection, guard)? {
            return Ok(SessionCommitOutcome::Rejected(rejection));
        }
        let result = upsert_sessions_locked(
            &mut connection,
            std::slice::from_ref(record),
            evidence_agents,
        )?;
        if let Some(physical_source) = &guard.physical_source {
            self.session_write_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .physical_sources
                .insert(
                    guard.key.clone(),
                    SessionPhysicalSource {
                        incarnation: result.0[0].1,
                        source_kind: record.source_kind.clone(),
                        source_label: record.source_label.clone(),
                        physical_source: physical_source.clone(),
                        attempt: guard.attempt,
                    },
                );
        }
        guard.completed.store(true, Ordering::Release);
        Ok(SessionCommitOutcome::Upserted(result))
    }

    /// Delete one session only while its source read remains current.
    pub fn delete_session_if_current(
        &self,
        guard: &SessionWriteGuard,
    ) -> Result<SessionCommitOutcome> {
        let mut connection = self.lock();
        if let Some(rejection) = self.session_write_rejection(&connection, guard)? {
            return Ok(SessionCommitOutcome::Rejected(rejection));
        }
        if guard.stored_binding.as_ref().is_some_and(|binding| {
            binding.source_label != guard.source_label
                || binding.physical_source.as_ref().is_some_and(|source| {
                    guard.physical_source.as_ref() != Some(&source.physical_source)
                })
        }) {
            return Ok(SessionCommitOutcome::Rejected(
                SessionCommitRejection::SourceChanged,
            ));
        }
        let tx = connection.transaction()?;
        let removed = delete_session_in(&tx, &guard.key)?;
        tx.commit()?;
        self.bump_session_key_fence(&guard.key);
        guard.completed.store(true, Ordering::Release);
        Ok(SessionCommitOutcome::Deleted(removed.map(|incarnation| {
            (incarnation, revision_of(&connection))
        })))
    }

    /// Accept an unchanged stored candidate once. Each dispatch needs a new guard.
    /// The first acceptance after restart binds its physical identity in memory.
    pub fn accept_session_if_current(&self, guard: &SessionWriteGuard) -> Result<bool> {
        let connection = self.lock();
        if self.session_write_rejection(&connection, guard)?.is_some() {
            return Ok(false);
        }
        let Some(binding) = &guard.stored_binding else {
            return Ok(false);
        };
        if binding.source_label != guard.source_label
            || binding.physical_source.as_ref().is_some_and(|source| {
                guard.physical_source.as_ref() != Some(&source.physical_source)
            })
        {
            return Ok(false);
        }
        if let Some(physical_source) = &guard.physical_source {
            self.session_write_state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .physical_sources
                .insert(
                    guard.key.clone(),
                    SessionPhysicalSource {
                        incarnation: binding.incarnation,
                        source_kind: binding.source_kind.clone(),
                        source_label: binding.source_label.clone(),
                        physical_source: physical_source.clone(),
                        attempt: guard.attempt,
                    },
                );
        }
        guard.completed.store(true, Ordering::Release);
        Ok(true)
    }

    pub(super) fn session_write_rejection(
        &self,
        connection: &Connection,
        guard: &SessionWriteGuard,
    ) -> Result<Option<SessionCommitRejection>> {
        if self.destructive_write_epoch.load(Ordering::Acquire) != guard.global_fence {
            return Ok(Some(SessionCommitRejection::GlobalFenceChanged));
        }
        let state = self
            .session_write_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.key_fences.get(&guard.key).copied().unwrap_or(0) != guard.key_fence {
            return Ok(Some(SessionCommitRejection::Deleted));
        }
        if !Arc::ptr_eq(&self.session_write_state, &guard.state)
            || guard.completed.load(Ordering::Acquire)
            || state.attempts.get(&guard.key).copied() != Some(guard.attempt)
        {
            return Ok(Some(SessionCommitRejection::Superseded));
        }
        if session_source_binding_in(connection, &state, &guard.key)? != guard.stored_binding {
            return Ok(Some(SessionCommitRejection::SourceChanged));
        }
        Ok(None)
    }

    /// Capture native session identities and write fences before resolving indexed titles.
    pub(crate) fn native_session_title_refresh_guards(
        &self,
        agent: &str,
    ) -> Result<Vec<SessionTitleRefreshGuard>> {
        let connection = self.lock();
        let state = self
            .session_write_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut statement = connection.prepare(
            "SELECT environment_key, agent, session_id, source_label, incarnation, source_kind
               FROM session
              WHERE environment_key = 'native' AND agent = ?1
              ORDER BY session_id",
        )?;
        let rows = statement.query_map(params![agent], |row| {
            let key = SessionKey::new(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            );
            let binding = bind_session_source(
                &state,
                &key,
                row.get(5)?,
                row.get(3)?,
                Incarnation(row.get(4)?),
            );
            Ok(SessionTitleRefreshGuard {
                binding,
                key_fence: state.key_fences.get(&key).copied().unwrap_or(0),
                key,
                global_fence: self.destructive_write_epoch.load(Ordering::Acquire),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Update only title fields while the captured session identity and write fences remain current.
    pub(crate) fn update_session_title_if_current(
        &self,
        guard: &SessionTitleRefreshGuard,
        title: Option<&str>,
        title_source: Option<&str>,
    ) -> Result<bool> {
        let connection = self.lock();
        if self.destructive_write_epoch.load(Ordering::Acquire) != guard.global_fence {
            return Ok(false);
        }
        let state = self
            .session_write_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.key_fences.get(&guard.key).copied().unwrap_or(0) != guard.key_fence {
            return Ok(false);
        }
        if session_source_binding_in(&connection, &state, &guard.key)?.as_ref()
            != Some(&guard.binding)
        {
            return Ok(false);
        }
        let changed = connection.execute(
            "UPDATE session SET title = ?4, title_source = ?5
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND incarnation = ?6 AND source_label = ?7",
            params![
                guard.key.environment_key,
                guard.key.agent,
                guard.key.session_id,
                title,
                title_source,
                guard.binding.incarnation.0,
                guard.binding.source_label,
            ],
        )?;
        Ok(changed > 0)
    }

    pub(super) fn bump_session_key_fence(&self, key: &SessionKey) {
        let mut state = self
            .session_write_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.physical_sources.remove(key);
        if !state.attempts.contains_key(key) {
            return;
        }
        let fence = state.key_fences.entry(key.clone()).or_default();
        *fence = fence.wrapping_add(1);
    }
}

fn session_source_binding_in(
    connection: &Connection,
    state: &SessionWriteState,
    key: &SessionKey,
) -> Result<Option<SessionSourceBinding>> {
    Ok(connection
        .query_row(
            "SELECT source_kind, source_label, incarnation FROM session
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
            |row| {
                Ok(bind_session_source(
                    state,
                    key,
                    row.get(0)?,
                    row.get(1)?,
                    Incarnation(row.get(2)?),
                ))
            },
        )
        .optional()?)
}

fn bind_session_source(
    state: &SessionWriteState,
    key: &SessionKey,
    source_kind: String,
    source_label: String,
    incarnation: Incarnation,
) -> SessionSourceBinding {
    let physical_source = state.physical_sources.get(key).filter(|source| {
        source.incarnation == incarnation
            && source.source_kind == source_kind
            && source.source_label == source_label
    });
    SessionSourceBinding {
        incarnation,
        source_kind,
        source_label,
        physical_source: physical_source.cloned(),
    }
}
