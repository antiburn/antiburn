//! Count sessions and transfer bytes in one bounded upload scan.

use super::{Job, JobState, Policy, Selection, State, Status, Store};
use antiburn_cloud_capture::{CaptureState, QueueEntry, TransferProgress};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    found: Option<usize>,
    /// Counts of distinct sources keyed by saved folder rule ID.
    /// The scan may have gaps. Missing rules are unknown, not zero.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    folder_found: BTreeMap<String, usize>,
    blocked: usize,
    unsupported: usize,
    scan_incomplete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    transfer: Option<ScanTransfer>,
}

/// Required compressed bytes for this scan, including its accepted captures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanTransfer {
    sent_bytes: u64,
    total_bytes: Option<u64>,
}

/// Track one backfill with at most 4,096 sources and 4,096 capture counters.
/// No source identity leaves this tracker. A full tracker cannot prove a total.
pub(super) struct BackfillProgress {
    account: String,
    destination: String,
    resource: Option<String>,
    revision: String,
    grant_id: String,
    sources: BTreeMap<String, Source>,
    transfers: BTreeMap<String, CaptureTransfer>,
    decisions: BTreeMap<&'static str, usize>,
    last_report: Option<Instant>,
    reported_complete: bool,
    incomplete: bool,
    finished: bool,
    background: bool,
}

struct Source {
    observation: String,
    folder_group_id: Option<String>,
    uploaded: bool,
    problem: Option<SourceProblem>,
    refresh_needed: bool,
}

/// Keep only identities, observations, and counters after accepted bytes leave the spool.
struct CaptureTransfer {
    source_id: String,
    observation: String,
    sent_bytes: u64,
    total_bytes: Option<u64>,
    accepted: bool,
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
            transfers: BTreeMap::new(),
            decisions: BTreeMap::new(),
            last_report: None,
            reported_complete: false,
            incomplete: false,
            finished: false,
            background: false,
        }
    }

    /// Use the normal discovery sweep without requesting or restarting source work.
    pub(super) fn new_background(policy: &Policy, grant_id: &str) -> Self {
        Self {
            background: true,
            ..Self::new(policy, grant_id)
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
                    self.transfers
                        .retain(|_, transfer| transfer.source_id != id);
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
                            folder_group_id: selection.folder_group_id.clone(),
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

    /// Replace active counters with the current spool snapshot. Accepted counters
    /// remain after acknowledgement. Missing or expired parts cannot keep old credit.
    pub(super) fn observe_transfers(
        &mut self,
        policy: &Policy,
        jobs: &[Job],
        entries: &[QueueEntry],
    ) -> Result<(), &'static str> {
        self.transfers.retain(|_, transfer| transfer.accepted);
        for entry in entries {
            let Some(job) = jobs
                .iter()
                .find(|job| job.identity == entry.identity && job.allowed(policy))
            else {
                continue;
            };
            if entry.state != CaptureState::Cancelled {
                self.observe_transfer(
                    job,
                    &entry
                        .progress()
                        .map_err(|_| "Could not read saved capture progress.")?,
                    entry.state == CaptureState::Accepted,
                );
            }
        }
        Ok(())
    }

    /// Save validated counters for the exact job and capture snapshot. Call before
    /// acknowledging acceptance, including when discovery has not found the source.
    /// An older terminal job cannot add historical bytes to a new scan.
    pub(super) fn observe_transfer(
        &mut self,
        job: &Job,
        progress: &TransferProgress,
        accepted: bool,
    ) {
        if !self.matches_job(job)
            || (job.state == JobState::Accepted
                && !self.transfers.contains_key(&job.identity.capture_id))
            || (self.finished
                && self
                    .sources
                    .get(&job.identity.source_id)
                    .is_none_or(|source| {
                        source.uploaded
                            && !source.refresh_needed
                            && source.observation != job.observation_key
                    }))
        {
            return;
        }
        if self.transfers.len() == MAX_TRACKED_SOURCES
            && !self.transfers.contains_key(&job.identity.capture_id)
        {
            self.mark_incomplete();
            return;
        }
        self.transfers.insert(
            job.identity.capture_id.clone(),
            CaptureTransfer {
                source_id: job.identity.source_id.clone(),
                observation: job.observation_key.clone(),
                sent_bytes: progress.confirmed_upload_bytes,
                total_bytes: progress
                    .negotiated
                    .then_some(progress.required_transfer_bytes),
                accepted,
            },
        );
    }

    /// Record one closed reason for release diagnostics without logging a source ID.
    pub(super) fn record_decision(&mut self, reason: &'static str) {
        *self.decisions.entry(reason).or_default() += 1;
    }

    /// Do not present a pre-reservation file change as a manual recovery case.
    /// Keep the source in the scan and match its fresh version on a normal page.
    pub(super) fn refresh_changed_source(&mut self, source_id: &str) -> bool {
        if let Some(source) = self.sources.get_mut(source_id) {
            source.uploaded = false;
            source.problem = None;
            source.refresh_needed = true;
            self.record_decision("file_changed_before_reservation");
            return true;
        }
        false
    }

    /// Request a normal discovery turn for sources that changed during backfill.
    pub(super) fn needs_refresh(&self) -> bool {
        self.sources.values().any(|source| source.refresh_needed)
    }

    /// Close unresolved observations after a full refresh. Absence alone does
    /// not prove that a source moved outside the approved folders.
    pub(super) fn finish_refresh(&mut self) {
        for source in self
            .sources
            .values_mut()
            .filter(|source| source.refresh_needed)
        {
            source.refresh_needed = false;
            source.problem = Some(SourceProblem::Blocked);
            self.incomplete = true;
        }
    }

    /// Update the problem for a source already counted in this scan.
    pub(super) fn observe_problem(&mut self, source_id: &str, problem: Option<SourceProblem>) {
        if let Some(source) = self.sources.get_mut(source_id) {
            source.problem = problem;
        }
    }

    /// Reconcile acceptance for a counted source. The saved receipt can describe
    /// a different observation than the scan or the original job.
    pub(super) fn observe_acceptance(&mut self, store: &Store, job: &Job) -> bool {
        if !self.matches_job(job) {
            return false;
        }
        let id = &job.identity.source_id;
        let Some(resource) = self.resource.as_deref() else {
            return false;
        };
        let Some(source) = self.sources.get_mut(id) else {
            return false;
        };
        match store.cloud_sync_source(&self.account, &self.destination, resource, id) {
            Ok(Some(receipt)) if receipt.suppressed => {
                self.sources.remove(id);
                self.transfers
                    .retain(|_, transfer| &transfer.source_id != id);
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
                source.refresh_needed = false;
                tracing::debug!(
                    event = "cloud_backfill_receipt_outcome",
                    outcome = "matched"
                );
            }
            Ok(Some(receipt))
                if receipt.fingerprint.is_some() && receipt.observation_key.is_some() =>
            {
                // The new receipt does not prove which version is still on disk.
                source.uploaded = false;
                source.problem = None;
                source.refresh_needed = true;
                tracing::debug!(
                    event = "cloud_backfill_receipt_outcome",
                    outcome = "refresh"
                );
                return true;
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
        false
    }

    pub(super) fn observe_suppression(&mut self, job: &Job) {
        if !self.matches_job(job) {
            return;
        }
        self.transfers
            .retain(|_, transfer| transfer.source_id != job.identity.source_id);
        if self.sources.remove(&job.identity.source_id).is_some() {
            tracing::debug!(
                event = "cloud_backfill_receipt_outcome",
                outcome = "suppressed"
            );
        }
    }

    /// Remove a source whose current recorded folders are all outside consent.
    pub(super) fn observe_exclusion(&mut self, source_id: &str) {
        self.transfers
            .retain(|_, transfer| transfer.source_id != source_id);
        if self.sources.remove(source_id).is_some() {
            tracing::debug!(
                event = "cloud_backfill_receipt_outcome",
                outcome = "excluded"
            );
        }
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
        self.transfers
            .retain(|_, transfer| self.sources.contains_key(&transfer.source_id));
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
            background = self.background,
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

    /// A final total needs complete discovery and a current plan for every source
    /// without a matching receipt. Earlier accepted versions retain only their work.
    fn transfer_status(&self, scan_active: bool) -> Option<ScanTransfer> {
        let mut sent_bytes = 0_u64;
        let mut total_bytes = Some(0_u64);
        let mut planned_sources = BTreeSet::new();
        for transfer in self.transfers.values() {
            let Some(source) = self.sources.get(&transfer.source_id) else {
                continue;
            };
            sent_bytes = sent_bytes.checked_add(transfer.sent_bytes)?;
            total_bytes = total_bytes
                .zip(transfer.total_bytes)
                .and_then(|(total, bytes)| total.checked_add(bytes));
            if !transfer.accepted
                && transfer.total_bytes.is_some()
                && transfer.observation == source.observation
            {
                planned_sources.insert(transfer.source_id.as_str());
            }
        }
        let plans_known = self.sources.iter().all(|(id, source)| {
            source.uploaded || (!source.refresh_needed && planned_sources.contains(id.as_str()))
        });
        Some(ScanTransfer {
            sent_bytes,
            total_bytes: total_bytes
                .filter(|_| self.finished && !self.incomplete && !scan_active && plans_known),
        })
    }

    /// Only share counts for the exact saved policy and a current storage read.
    pub(super) fn status(&self, status: &Status, scan_active: bool) -> Option<UploadProgress> {
        (status.storage_known
            && !status.paused
            && !matches!(status.status, State::ReauthRequired | State::Offline)
            && status.account_id.as_ref() == Some(&self.account)
            && status.destination_id.as_ref() == Some(&self.destination)
            && status.policy_revision.as_ref() == Some(&self.revision))
        .then(|| {
            let (uploaded, blocked, unsupported) = self.counts();
            let mut folder_found = BTreeMap::new();
            for group_id in self
                .sources
                .values()
                .filter_map(|source| source.folder_group_id.as_ref())
            {
                *folder_found.entry(group_id).or_insert(0) += 1;
            }
            UploadProgress {
                uploaded,
                total: (self.finished && !self.incomplete && !scan_active)
                    .then_some(self.sources.len()),
                found: (scan_active && !self.sources.is_empty()).then_some(self.sources.len()),
                folder_found: folder_found
                    .into_iter()
                    .map(|(id, count)| (id.clone(), count))
                    .collect(),
                blocked,
                unsupported,
                scan_incomplete: self.incomplete,
                transfer: self.transfer_status(scan_active),
            }
        })
    }
}

#[cfg(test)]
#[path = "tests/backfill_progress_tests.rs"]
mod tests;
