//! Describe scheduled work without changing discovery, retry, or upload admission.

use super::{
    BackfillProgress, Engine, Job, JobState, Policy, QueueEntry, Runner, State, Status, Store,
    capture, status_for_destination, upload,
};
use antiburn_cloud_capture::CaptureState;

impl Runner {
    /// Bind scan counters and activity to the current grant and readable storage.
    pub(super) fn backfill_status(&self, mut status: Status, grant_id: Option<&str>) -> Status {
        let activity_available = status.storage_known
            && !status.paused
            && !matches!(
                status.status,
                State::ConsentRequired
                    | State::Paused
                    | State::ReauthRequired
                    | State::StorageUnavailable
            )
            && (status.status != State::Offline
                || status.full_sync_in_progress
                || status.background_sync_in_progress);
        status.full_sync_in_progress &= activity_available;
        status.background_sync_in_progress &= activity_available;
        let progress = self
            .backfill_progress
            .as_ref()
            .filter(|progress| grant_id.is_some_and(|grant| progress.matches_grant(grant)));
        status.backfill_in_progress = activity_available
            && progress.is_some()
            && self.backfill_revision.is_some()
            && self.backfill_revision == status.policy_revision;
        status.full_sync_in_progress |= status.storage_known
            && !status.paused
            && matches!(status.status, State::Ready | State::Syncing)
            && status.backfill_in_progress;
        status.upload_progress =
            progress.and_then(|progress| progress.status(&status, status.backfill_in_progress));
        status.background_progress = self
            .background_progress
            .as_ref()
            .filter(|progress| grant_id.is_some_and(|grant| progress.matches_grant(grant)))
            .and_then(|progress| progress.status(&status, self.background_page));
        status
    }

    /// Finish each scan only after its final discovery page drains.
    pub(super) fn finish_backfill_scan(&mut self) {
        if self.backfill_page && self.candidates.is_empty() && self.discovery_cursor.is_none() {
            if let Some(progress) = &mut self.backfill_progress {
                progress.finish();
                if progress.needs_refresh() {
                    self.next_reconcile = None;
                }
            }
            self.backfill_revision = None;
            self.backfill_page = false;
        }
        self.finish_background_scan();
    }

    /// Recheck a source when its accepted version differs from the scan.
    pub(super) fn observe_backfill_acceptance(&mut self, store: &Store, job: &Job) {
        if self.full_sync_pending
            && let Some(progress) = &mut self.backfill_progress
            && progress.observe_acceptance(store, job)
        {
            self.request_backfill_refresh();
        }
        if let Some(progress) = &mut self.background_progress {
            // The next scheduled sweep checks changed versions. Progress must
            // not add discovery work or restart the normal cursor.
            progress.observe_acceptance(store, job);
            if !self.background_page {
                progress.finish_refresh();
            }
        }
    }

    /// Retain final byte counters before either acceptance path removes the entry.
    pub(super) fn observe_accepted_transfer(
        &mut self,
        job: &Job,
        entry: &QueueEntry,
    ) -> Result<(), String> {
        for progress in self.progress_trackers() {
            let transfer = entry
                .progress()
                .map_err(|_| "Could not read accepted capture progress.")?;
            progress.observe_transfer(job, &transfer, true);
        }
        Ok(())
    }

    /// Publish a closed failure without rereading failed storage. Retained
    /// counters are last-known values for the same account and service only.
    /// `storage_known: false` makes their current values explicitly unknown.
    pub(super) fn report_status(
        &mut self,
        store: &Store,
        host: &mut capture::Host,
        engine: &Engine,
        failed: bool,
        account_current: &dyn Fn() -> bool,
        publish: impl FnOnce(Status),
    ) -> Status {
        if failed {
            self.backfill_revision = None;
            self.backfill_page = false;
            self.backfill_progress = None;
            self.full_sync_pending = false;
            self.background_progress = None;
            self.background_page = false;
        } else {
            self.finish_backfill_scan();
        }
        let read = if failed {
            Err("Capture storage is unavailable.".to_owned())
        } else if !account_current() {
            Ok(Status {
                status: State::ReauthRequired,
                ..Status::default()
            })
        } else {
            let team = self.team.as_ref().map(|(id, _)| id.clone());
            status_for_destination(store, host, engine, self.state, team.as_deref(), Some(self))
        };
        let mut result = match read {
            Ok(value) => {
                let grant_id = engine.session().map(|session| session.grant_id.as_str());
                for progress in self
                    .backfill_progress
                    .iter_mut()
                    .chain(self.background_progress.iter_mut())
                {
                    if grant_id.is_some_and(|grant| progress.matches_grant(grant)) {
                        progress.log_progress();
                    }
                }
                let value = self.backfill_status(value, grant_id);
                self.last_status = Some(value.clone());
                self.last_status_origin = engine.resource_origin().map(str::to_owned);
                value
            }
            Err(_) => {
                self.state = State::StorageUnavailable;
                self.backfill_revision = None;
                self.backfill_page = false;
                self.backfill_progress = None;
                self.full_sync_pending = false;
                self.background_progress = None;
                self.background_page = false;
                let previous = self.last_status.as_ref().filter(|previous| {
                    account_current()
                        && self.last_status_origin.as_deref() == engine.resource_origin()
                        && engine.session().is_some_and(|session| {
                            previous.account_id.as_deref() == Some(&session.user.id)
                                && previous.destination_id.as_deref()
                                    == Some(
                                        self.team
                                            .as_ref()
                                            .map_or(session.workspace.id.as_str(), |(id, _)| id),
                                    )
                        })
                });
                let mut value = previous.cloned().unwrap_or_default();
                if account_current()
                    && let Some(session) = engine.session()
                {
                    value.account_id = Some(session.user.id.clone());
                    value.destination_id = Some(
                        self.team
                            .as_ref()
                            .map_or(&session.workspace.id, |(id, _)| id)
                            .clone(),
                    );
                }
                value.status = State::StorageUnavailable;
                value.storage_known = false;
                value.automatic_recovery_count = None;
                value.retry_available = false;
                value.backfill_in_progress = false;
                value.full_sync_in_progress = false;
                value.background_sync_in_progress = false;
                value.upload_progress = None;
                value.background_progress = None;
                value.upload_transfer = None;
                value
            }
        };
        if !account_current() {
            result = Status {
                status: if self.state == State::StorageUnavailable {
                    State::StorageUnavailable
                } else {
                    State::ReauthRequired
                },
                ..Status::default()
            };
        }
        publish(result.clone());
        result
    }

    /// Update active runs only. Later background work cannot change a settled
    /// explicit result, including a source that failed during that scan.
    pub(super) fn progress_trackers(&mut self) -> impl Iterator<Item = &mut BackfillProgress> {
        let full_sync_pending = self.full_sync_pending;
        self.backfill_progress
            .iter_mut()
            .filter(move |_| full_sync_pending)
            .chain(self.background_progress.iter_mut())
    }

    /// Start counters only where the normal scheduler starts a fresh sweep.
    /// A resumed cursor cannot prove a complete total for a new tracker.
    pub(super) fn start_background_scan(&mut self, policy: &Policy) {
        if self.full_sync_pending {
            return;
        }
        if let Some(grant) = &self.retry_grant {
            self.background_progress =
                Some(BackfillProgress::new_background(policy, &grant.grant_id));
        }
    }

    /// Finish counters after the final page drains. Changed sources remain gaps
    /// until the scheduler's next sweep; counters never request extra source reads.
    pub(super) fn finish_background_scan(&mut self) {
        if self.background_page && self.candidates.is_empty() && self.discovery_cursor.is_none() {
            if let Some(progress) = &mut self.background_progress {
                progress.finish_refresh();
                progress.finish();
            }
            self.background_page = false;
        }
    }

    /// Keep activity independent of measurements. Saved work can resume before
    /// this process reaches a natural discovery boundary and starts counters.
    pub(super) fn update_runtime_progress(
        &mut self,
        policy: &Policy,
        grant_id: &str,
        pending_upload: bool,
        status: &mut Status,
    ) {
        let discovery_scope =
            self.binding
                .as_ref()
                .is_some_and(|(account, destination, resource, revision)| {
                    account == &policy.account_id
                        && destination == &policy.destination_id
                        && resource == &policy.resource_origin
                        && revision == &policy.revision
                })
                && self.retry_grant.as_ref().is_some_and(|grant| {
                    grant.account_id == policy.account_id
                        && grant.destination_id == policy.destination_id
                        && grant.grant_id == grant_id
                });
        let discovery_pending =
            discovery_scope && (!self.candidates.is_empty() || self.discovery_cursor.is_some());
        let full_scope = self
            .backfill_progress
            .as_ref()
            .is_some_and(|progress| progress.matches(policy) && progress.matches_grant(grant_id));
        let refresh_pending = full_scope
            && self.full_sync_pending
            && self
                .backfill_progress
                .as_ref()
                .is_some_and(BackfillProgress::needs_refresh);
        if !full_scope
            || (self.backfill_revision.is_none()
                && !discovery_pending
                && !pending_upload
                && !refresh_pending)
        {
            self.full_sync_pending = false;
        }
        let can_discover = !self.discovery_unavailable && status.status != State::Offline;
        let backfill_requested =
            full_scope && self.backfill_revision.as_deref() == Some(&policy.revision);
        let active = status.storage_known
            && !status.paused
            && !matches!(
                status.status,
                State::ConsentRequired
                    | State::Paused
                    | State::ReauthRequired
                    | State::StorageUnavailable
            )
            && (pending_upload
                || (can_discover
                    && (discovery_pending
                        || backfill_requested
                        || (discovery_scope && self.background_page)
                        || refresh_pending)));
        status.full_sync_in_progress = active && self.full_sync_pending;
        status.background_sync_in_progress = active && !self.full_sync_pending;
    }

    /// Count saved work only when the scheduler can resume it without user action.
    pub(super) fn has_scheduled_upload(
        &self,
        job: &Job,
        entry: Option<&QueueEntry>,
        state: State,
    ) -> bool {
        // One unsupported format does not stop other saved uploads.
        let recovery_state = if state == State::UploadUnavailable && !self.discovery_unavailable {
            State::Ready
        } else {
            state
        };
        self.will_recover_automatically(job, entry, recovery_state)
            || (!matches!(
                recovery_state,
                State::RouteDenied | State::UploadUnavailable
            ) && matches!(job.state, JobState::Pending | JobState::Queued)
                && entry.is_some_and(|entry| entry.state == CaptureState::Queued)
                && self.has_saved_probe_budget(&job.identity.capture_id)
                && !self
                    .unsupported_saved_uploads
                    .contains(&job.identity.capture_id)
                && !self
                    .retry_deadlines
                    .get(&job.identity.capture_id)
                    .is_some_and(|(_, reason)| {
                        matches!(
                            reason,
                            upload::Failure::Unavailable
                                | upload::Failure::Unauthorized
                                | upload::Failure::Rejected
                        )
                    }))
    }
}
