//! Count sessions in one explicit backfill without using historical job totals.

use super::{Job, Policy, Selection, State, Status, Store};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

const MAX_TRACKED_SOURCES: usize = 4096;

/// Renderer progress for the saved folders in one destination's backfill.
/// A missing total means the scan is still running or discovery had gaps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UploadProgress {
    uploaded: usize,
    total: Option<usize>,
    blocked: usize,
    unsupported: usize,
    scan_incomplete: bool,
}

/// Track only observed sources for the current backfill. No source identity
/// leaves this tracker. A full tracker cannot prove a completed total.
pub(super) struct BackfillProgress {
    account: String,
    destination: String,
    resource: Option<String>,
    revision: String,
    grant_id: String,
    sources: BTreeMap<String, Source>,
    accepted_during_scan: BTreeSet<String>,
    decisions: BTreeMap<&'static str, usize>,
    last_report: Option<Instant>,
    reported_complete: bool,
    incomplete: bool,
    finished: bool,
}

struct Source {
    observation: String,
    uploaded: bool,
    problem: Option<SourceProblem>,
    refresh_needed: bool,
}

/// Why a discovered source cannot upload on its current turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SourceProblem {
    Blocked,
    Unsupported,
}

impl BackfillProgress {
    pub(super) fn new(policy: &Policy, grant_id: &str) -> Self {
        Self {
            account: policy.account_id.clone(),
            destination: policy.destination_id.clone(),
            resource: policy.resource_origin.clone(),
            revision: policy.revision.clone(),
            grant_id: grant_id.into(),
            sources: BTreeMap::new(),
            accepted_during_scan: BTreeSet::new(),
            decisions: BTreeMap::new(),
            last_report: None,
            reported_complete: false,
            incomplete: false,
            finished: false,
        }
    }

    pub(super) fn matches(&self, policy: &Policy) -> bool {
        self.account == policy.account_id
            && self.destination == policy.destination_id
            && self.resource == policy.resource_origin
            && self.revision == policy.revision
    }

    pub(super) fn matches_grant(&self, grant_id: &str) -> bool {
        self.grant_id == grant_id
    }

    /// Add discovered sources that match the saved folders and are not suppressed.
    /// A normal page only updates sources that changed before their capture pin.
    pub(super) fn observe_page<'a>(
        &mut self,
        store: &Store,
        candidates: impl IntoIterator<Item = &'a Selection>,
        refresh_only: bool,
    ) {
        let Some(resource) = self.resource.as_deref() else {
            self.mark_incomplete();
            return;
        };
        let (mut matched, mut changed, mut missing, mut suppressed, mut unreadable) =
            (0, 0, 0, 0, 0);
        for selection in candidates {
            let id = selection.candidate.source_id();
            if refresh_only
                && !self
                    .sources
                    .get(id)
                    .is_some_and(|source| source.refresh_needed)
            {
                continue;
            }
            if self.sources.len() == MAX_TRACKED_SOURCES && !self.sources.contains_key(id) {
                self.incomplete = true;
                break;
            }
            match store.cloud_sync_source(&self.account, &self.destination, resource, id) {
                Ok(Some(receipt)) if receipt.suppressed => {
                    self.sources.remove(id);
                    suppressed += 1;
                }
                Ok(receipt) => {
                    let uploaded = match receipt.filter(|receipt| receipt.fingerprint.is_some()) {
                        Some(receipt)
                            if receipt.observation_key.as_ref() == Some(&selection.observation) =>
                        {
                            matched += 1;
                            true
                        }
                        Some(_) => {
                            changed += 1;
                            false
                        }
                        None => {
                            missing += 1;
                            false
                        }
                    };
                    self.sources.insert(
                        id.to_owned(),
                        Source {
                            observation: selection.observation.clone(),
                            uploaded,
                            problem: None,
                            refresh_needed: false,
                        },
                    );
                }
                Err(_) => {
                    self.incomplete = true;
                    unreadable += 1;
                }
            }
        }
        for (reason, count) in [
            ("receipt_matched", matched),
            ("receipt_changed", changed),
            ("receipt_missing", missing),
            ("receipt_suppressed", suppressed),
            ("receipt_unreadable", unreadable),
        ] {
            if count > 0 {
                *self.decisions.entry(reason).or_default() += count;
            }
        }
    }

    /// Record one closed reason for release diagnostics without logging a source ID.
    pub(super) fn record_decision(&mut self, reason: &'static str) {
        *self.decisions.entry(reason).or_default() += 1;
    }

    /// Do not present a pre-reservation file change as a manual recovery case.
    /// Keep the source in the scan and match its fresh version on a normal page.
    pub(super) fn refresh_changed_source(&mut self, source_id: &str) {
        if let Some(source) = self.sources.get_mut(source_id) {
            source.uploaded = false;
            source.problem = None;
            source.refresh_needed = true;
            self.record_decision("file_changed_before_reservation");
        }
    }

    /// Request a normal discovery turn for sources that changed during backfill.
    pub(super) fn needs_refresh(&self) -> bool {
        self.sources.values().any(|source| source.refresh_needed)
    }

    /// Update a source already counted in this scan. An unchanged accepted
    /// receipt still counts even if an explicit re-offer is blocked.
    pub(super) fn observe_problem(&mut self, source_id: &str, problem: Option<SourceProblem>) {
        if let Some(source) = self.sources.get_mut(source_id) {
            source.problem = problem;
        }
    }

    /// Reconcile exact acceptance. A saved job can finish before the scan
    /// reaches its source. In that case do not re-offer the same version.
    /// Use the saved observation, which can differ from the job's first observation.
    pub(super) fn observe_acceptance(&mut self, store: &Store, job: &Job) {
        if !self.matches_job(job) {
            return;
        }
        let id = &job.identity.source_id;
        let Some(resource) = self.resource.as_deref() else {
            return;
        };
        if !self.finished {
            if self.accepted_during_scan.len() < MAX_TRACKED_SOURCES {
                self.accepted_during_scan.insert(id.clone());
            } else if !self.accepted_during_scan.contains(id) {
                self.incomplete = true;
            }
        }
        let Some(source) = self.sources.get_mut(id) else {
            return;
        };
        match store.cloud_sync_source(&self.account, &self.destination, resource, id) {
            Ok(Some(receipt)) if receipt.suppressed => {
                self.sources.remove(id);
                tracing::debug!(
                    event = "cloud_backfill_receipt_outcome",
                    outcome = "suppressed"
                );
            }
            Ok(Some(receipt))
                if receipt.fingerprint.is_some()
                    && receipt.observation_key.as_ref() == Some(&source.observation) =>
            {
                source.uploaded = true;
                source.problem = None;
                tracing::debug!(
                    event = "cloud_backfill_receipt_outcome",
                    outcome = "matched"
                );
            }
            Ok(_) => {
                if !source.uploaded {
                    source.problem = Some(SourceProblem::Blocked);
                }
                tracing::debug!(
                    event = "cloud_backfill_receipt_outcome",
                    outcome = "unmatched"
                );
            }
            Err(_) => {
                self.incomplete = true;
                tracing::warn!(
                    event = "cloud_backfill_receipt_outcome",
                    outcome = "unreadable"
                );
            }
        }
    }

    pub(super) fn observe_suppression(&mut self, job: &Job) {
        if self.matches_job(job) && self.sources.remove(&job.identity.source_id).is_some() {
            tracing::debug!(
                event = "cloud_backfill_receipt_outcome",
                outcome = "suppressed"
            );
        }
    }

    /// Remove a source whose current recorded folders are all outside consent.
    pub(super) fn observe_exclusion(&mut self, source_id: &str) {
        if self.sources.remove(source_id).is_some() {
            tracing::debug!(
                event = "cloud_backfill_receipt_outcome",
                outcome = "excluded"
            );
        }
    }

    /// Skip a second capture only when the caller also matches the saved receipt.
    pub(super) fn accepted_during_scan(&self, source_id: &str) -> bool {
        self.accepted_during_scan.contains(source_id)
    }

    fn matches_job(&self, job: &Job) -> bool {
        job.identity.account_id == self.account
            && job.identity.destination_id == self.destination
            && job.resource_origin == self.resource
    }

    pub(super) fn mark_incomplete(&mut self) {
        self.incomplete = true;
    }

    pub(super) fn finish(&mut self) {
        self.finished = true;
        self.log_scan_outcome("finished");
    }

    /// Record a scan that cannot finish with its current inputs.
    pub(super) fn abort(&mut self, reason: &'static str) {
        self.mark_incomplete();
        self.finished = true;
        self.log_scan_outcome(reason);
    }

    /// Report progress at most once per minute and report full acceptance once.
    pub(super) fn log_progress(&mut self) {
        let (uploaded, blocked, unsupported) = self.counts();
        let known = self.sources.len();
        if self.finished && !self.incomplete && known > 0 && uploaded == known {
            if !self.reported_complete {
                self.reported_complete = true;
                self.log_scan_outcome("all_accepted");
            }
        } else if (!self.finished || (!self.incomplete && uploaded + blocked + unsupported < known))
            && self
                .last_report
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(60))
        {
            self.last_report = Some(Instant::now());
            self.log_scan_outcome("in_progress");
        }
    }

    fn log_scan_outcome(&self, outcome: &'static str) {
        let (uploaded, blocked, unsupported) = self.counts();
        tracing::info!(
            event = "cloud_backfill_scan_summary",
            outcome,
            discovered = self.sources.len(),
            uploaded,
            blocked,
            unsupported,
            incomplete = self.incomplete,
            decisions = ?self.decisions,
        );
    }

    fn counts(&self) -> (usize, usize, usize) {
        let (mut uploaded, mut blocked, mut unsupported) = (0, 0, 0);
        for source in self.sources.values() {
            if source.uploaded {
                uploaded += 1;
            } else if let Some(problem) = source.problem {
                match problem {
                    SourceProblem::Blocked => blocked += 1,
                    SourceProblem::Unsupported => unsupported += 1,
                }
            }
        }
        (uploaded, blocked, unsupported)
    }

    /// Only share counts for the exact saved policy and a current storage read.
    pub(super) fn status(&self, status: &Status) -> Option<UploadProgress> {
        (status.storage_known
            && !status.paused
            && !matches!(status.status, State::ReauthRequired | State::Offline)
            && status.account_id.as_ref() == Some(&self.account)
            && status.destination_id.as_ref() == Some(&self.destination)
            && status.policy_revision.as_ref() == Some(&self.revision))
        .then(|| {
            let (uploaded, blocked, unsupported) = self.counts();
            UploadProgress {
                uploaded,
                total: (self.finished && !self.incomplete && !status.backfill_in_progress)
                    .then_some(self.sources.len()),
                blocked,
                unsupported,
                scan_incomplete: self.incomplete,
            }
        })
    }
}
