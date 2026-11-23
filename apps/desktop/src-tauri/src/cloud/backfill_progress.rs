//! Count sessions in one explicit backfill without using historical job totals.

use super::{Job, Policy, Selection, State, Status, Store};
use serde::Serialize;
use std::collections::BTreeMap;

const MAX_TRACKED_SOURCES: usize = 4096;

/// Renderer progress for the saved folders in one destination's backfill.
/// A missing total means the scan is still running or discovery had gaps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UploadProgress {
    uploaded: usize,
    total: Option<usize>,
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
    incomplete: bool,
    finished: bool,
}

struct Source {
    observation: String,
    uploaded: bool,
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
    /// Read local receipts only. Failure here does not change upload admission.
    pub(super) fn observe_page<'a>(
        &mut self,
        store: &Store,
        candidates: impl IntoIterator<Item = &'a Selection>,
    ) {
        let Some(resource) = self.resource.as_deref() else {
            self.mark_incomplete();
            return;
        };
        for selection in candidates {
            let id = selection.candidate.source_id();
            if self.sources.len() == MAX_TRACKED_SOURCES && !self.sources.contains_key(id) {
                self.incomplete = true;
                break;
            }
            match store.cloud_sync_source(&self.account, &self.destination, resource, id) {
                Ok(Some(receipt)) if receipt.suppressed => {
                    self.sources.remove(id);
                }
                Ok(receipt) => {
                    let uploaded = receipt.is_some_and(|receipt| {
                        receipt.fingerprint.is_some()
                            && receipt.observation_key.as_ref() == Some(&selection.observation)
                    });
                    self.sources.insert(
                        id.to_owned(),
                        Source {
                            observation: selection.observation.clone(),
                            uploaded,
                        },
                    );
                }
                Err(_) => self.incomplete = true,
            }
        }
    }

    /// Reconcile the outcome only for a source that this scan actually counted.
    /// Use the saved observation, which can differ from the job's first observation.
    pub(super) fn observe_outcome(&mut self, store: &Store, job: &Job) {
        let id = &job.identity.source_id;
        let Some(resource) = self.resource.as_deref() else {
            return;
        };
        if job.identity.account_id != self.account
            || job.identity.destination_id != self.destination
            || job.resource_origin.as_deref() != Some(resource)
        {
            return;
        }
        let Some(source) = self.sources.get_mut(id) else {
            return;
        };
        match store.cloud_sync_source(&self.account, &self.destination, resource, id) {
            Ok(Some(receipt)) if receipt.suppressed => {
                self.sources.remove(id);
            }
            Ok(Some(receipt))
                if receipt.fingerprint.is_some()
                    && receipt.observation_key.as_ref() == Some(&source.observation) =>
            {
                source.uploaded = true;
            }
            Ok(_) => {}
            Err(_) => self.incomplete = true,
        }
    }

    pub(super) fn mark_incomplete(&mut self) {
        self.incomplete = true;
    }

    pub(super) fn finish(&mut self) {
        self.finished = true;
    }

    /// Only share counts for the exact saved policy and a current storage read.
    pub(super) fn status(&self, status: &Status) -> Option<UploadProgress> {
        (status.storage_known
            && !status.paused
            && !matches!(status.status, State::ReauthRequired | State::Offline)
            && status.account_id.as_ref() == Some(&self.account)
            && status.destination_id.as_ref() == Some(&self.destination)
            && status.policy_revision.as_ref() == Some(&self.revision))
        .then(|| UploadProgress {
            uploaded: self
                .sources
                .values()
                .filter(|source| source.uploaded)
                .count(),
            total: (self.finished && !self.incomplete && !status.backfill_in_progress)
                .then_some(self.sources.len()),
        })
    }
}
