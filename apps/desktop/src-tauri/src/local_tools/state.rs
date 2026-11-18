//! Serialize setup work and fence observations from older setup revisions.

use super::{Phase, Reason, Snapshot, Status};

pub(super) struct State {
    snapshot: Snapshot,
    running: bool,
    queued: bool,
    worker: u64,
}

impl State {
    /// Start with an initial snapshot and a process-local revision counter.
    pub fn new(status: Status) -> Self {
        Self {
            snapshot: Snapshot {
                status,
                tested_sha256: None,
            },
            running: false,
            queued: false,
            worker: 0,
        }
    }

    /// Read a copy without permitting updates that bypass the revision fence.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.clone()
    }

    fn publish(&mut self, mut snapshot: Snapshot) -> Status {
        snapshot.status.revision = self.snapshot.status.revision + 1;
        self.snapshot = snapshot;
        self.snapshot.status.clone()
    }

    fn pending(&mut self) -> Status {
        let mut snapshot = self.snapshot.clone();
        snapshot.status.phase = Phase::Pending;
        snapshot.status.reason = None;
        self.publish(snapshot)
    }

    /// Queue one rerun or reserve the sole worker. Every request supersedes reads.
    pub fn begin_setup(&mut self) -> (Status, Option<u64>) {
        let status = self.pending();
        if self.running {
            self.queued = true;
            return (status, None);
        }
        self.running = true;
        self.worker = status.revision;
        (status, Some(self.worker))
    }

    /// Discard a result superseded by queued work and publish its next pending state.
    pub fn finish_setup(&mut self, worker: u64, snapshot: Snapshot) -> (Status, bool) {
        if !self.running || self.worker != worker {
            return (self.snapshot.status.clone(), false);
        }
        if self.queued {
            self.queued = false;
            return (self.pending(), true);
        }
        self.running = false;
        (self.publish(snapshot), false)
    }

    /// A failed or cancelled worker cannot leave setup permanently marked running.
    pub fn abort_setup(&mut self, worker: u64) -> Option<Status> {
        if !self.running || self.worker != worker {
            return None;
        }
        self.running = false;
        self.queued = false;
        Some(self.publish(Snapshot {
            status: Status::failed(Reason::SetupUnavailable, false, false),
            tested_sha256: None,
        }))
    }

    /// Apply inspection only to its original revision, even if bytes/digest match.
    pub fn apply_inspection(
        &mut self,
        revision: u64,
        result: Result<(), Reason>,
    ) -> (Status, bool) {
        if self.snapshot.status.revision != revision {
            return (self.snapshot.status.clone(), false);
        }
        let Err(reason) = result else {
            return (self.snapshot.status.clone(), false);
        };
        let mut snapshot = self.snapshot.clone();
        snapshot.status = Status::failed(
            reason,
            reason != Reason::CliUnavailable,
            !matches!(reason, Reason::CliUnavailable | Reason::CliTestFailed),
        );
        (self.publish(snapshot), true)
    }
}

#[cfg(test)]
#[path = "tests/state_tests.rs"]
mod tests;
