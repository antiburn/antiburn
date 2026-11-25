//! Count compressed bytes only for saved uploads allowed by the current policy.

use super::{Job, JobState, Policy};
use antiburn_cloud_capture::{CaptureState, QueueEntry, TransferProgress};
use serde::Serialize;

/// Confirmed compressed-byte counters for the current account's saved entries.
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Progress {
    negotiated: bool,
    total_compressed_bytes: u64,
    required_transfer_bytes: u64,
    reused_bytes: u64,
    confirmed_upload_bytes: u64,
}

impl Progress {
    fn add(&mut self, value: &TransferProgress) {
        self.negotiated &= value.negotiated;
        self.total_compressed_bytes += value.total_compressed_bytes;
        self.required_transfer_bytes += value.required_transfer_bytes;
        self.reused_bytes += value.reused_bytes;
        self.confirmed_upload_bytes += value.confirmed_upload_bytes;
    }
}

/// Required bytes and confirmed bytes for one identifiable queued upload.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UploadTransfer {
    sent_bytes: u64,
    total_bytes: u64,
}

impl UploadTransfer {
    /// Do not show raw source sizes, reused bytes, or estimated transfer progress.
    fn from_progress(value: &TransferProgress) -> Option<Self> {
        (value.negotiated && value.required_transfer_bytes > 0).then_some(Self {
            sent_bytes: value.confirmed_upload_bytes,
            total_bytes: value.required_transfer_bytes,
        })
    }
}

/// Return a per-upload counter only when one saved entry is queued to transfer.
/// A counter from a different workspace or an excluded job cannot contribute.
pub(super) fn saved_transfer_progress(
    policy: &Policy,
    jobs: &[Job],
    entries: &[QueueEntry],
) -> Result<(Progress, Option<UploadTransfer>), &'static str> {
    let mut aggregate = Progress {
        negotiated: true,
        ..Progress::default()
    };
    let mut has_entries = false;
    let mut queued = 0;
    let mut transfer = None;
    for entry in entries {
        if entry.identity.account_id != policy.account_id
            || entry.identity.destination_id != policy.destination_id
        {
            continue;
        }
        let Some(job) = jobs
            .iter()
            .find(|job| job.identity == entry.identity && job.allowed(policy))
        else {
            continue;
        };
        let value = entry
            .progress()
            .map_err(|_| "Could not read saved capture progress.")?;
        has_entries = true;
        aggregate.add(&value);
        if entry.state == CaptureState::Queued
            && matches!(job.state, JobState::Pending | JobState::Queued)
        {
            queued += 1;
            transfer = UploadTransfer::from_progress(&value);
        }
    }
    aggregate.negotiated &= has_entries;
    Ok((aggregate, if queued == 1 { transfer } else { None }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_upload_counts_only_confirmed_required_bytes() {
        let mut value = TransferProgress {
            total_compressed_bytes: 1_000,
            ..TransferProgress::default()
        };
        assert!(UploadTransfer::from_progress(&value).is_none());
        value.negotiated = true;
        value.reused_bytes = 1_000;
        assert!(UploadTransfer::from_progress(&value).is_none());
        value.reused_bytes = 200;
        value.required_transfer_bytes = 800;
        value.confirmed_upload_bytes = 300;
        assert_eq!(
            serde_json::to_value(UploadTransfer::from_progress(&value)).unwrap(),
            serde_json::json!({ "sentBytes": 300, "totalBytes": 800 })
        );
    }
}
