use std::cell::Cell;

use anyhow::Result;
use rusqlite::Connection;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct InvalidationEffects {
    pub accepted_evidence_revoked: usize,
    pub current_status_revoked: usize,
    pub unfinished_stale: usize,
}

impl std::ops::AddAssign for InvalidationEffects {
    fn add_assign(&mut self, other: Self) {
        self.accepted_evidence_revoked += other.accepted_evidence_revoked;
        self.current_status_revoked += other.current_status_revoked;
        self.unfinished_stale += other.unfinished_stale;
    }
}

impl InvalidationEffects {
    pub fn record(self, sink: Option<&Cell<Self>>) {
        if let Some(sink) = sink {
            let mut effects = sink.get();
            effects += self;
            sink.set(effects);
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PublicationEffects {
    pub after_cursor: i64,
    pub through_cursor: i64,
    pub updates: usize,
    pub removals: usize,
    pub gaps: usize,
    pub restored: usize,
    pub invalidation: InvalidationEffects,
}

impl PublicationEffects {
    pub fn head(connection: &Connection) -> Result<i64> {
        Ok(connection.query_row(
            "SELECT COALESCE(MAX(publication_sequence), 0) FROM native_tool_result_publication",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn collect(
        connection: &Connection,
        after_cursor: i64,
        invalidation: InvalidationEffects,
    ) -> Result<Self> {
        Ok(connection.query_row(
            "SELECT COALESCE(MAX(publication_sequence), ?1),
                COUNT(*) FILTER (WHERE outcome = 'update'),
                COUNT(*) FILTER (WHERE outcome = 'removal'),
                COUNT(*) FILTER (WHERE outcome = 'coverage_gap'),
                COUNT(*) FILTER (WHERE outcome = 'coverage_complete')
               FROM native_tool_result_publication WHERE publication_sequence > ?1",
            [after_cursor],
            |row| {
                Ok(Self {
                    after_cursor,
                    through_cursor: row.get(0)?,
                    updates: row.get(1)?,
                    removals: row.get(2)?,
                    gaps: row.get(3)?,
                    restored: row.get(4)?,
                    invalidation,
                })
            },
        )?)
    }

    pub fn trace(
        self,
        origin: &'static str,
        reason: &'static str,
        capture_claim_fence: Option<i64>,
        capture_source_generation: Option<i64>,
    ) {
        if self.after_cursor == self.through_cursor && self.invalidation == Default::default() {
            return;
        }
        ::tracing::debug!(
            event = "native_evidence_transition_committed",
            publication_origin = origin,
            reason,
            publication_after_cursor = self.after_cursor,
            publication_through_cursor = self.through_cursor,
            capture_claim_fence,
            capture_source_generation,
            updates = self.updates,
            removals = self.removals,
            gaps = self.gaps,
            restored = self.restored,
            accepted_evidence_revoked = self.invalidation.accepted_evidence_revoked,
            current_status_revoked = self.invalidation.current_status_revoked,
            unfinished_stale = self.invalidation.unfinished_stale,
        );
    }
}
