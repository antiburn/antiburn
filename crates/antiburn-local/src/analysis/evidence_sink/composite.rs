use super::*;

fn tool_result_revision_key(result: &ToolResultObservation) -> Option<String> {
    let identity = result
        .result_id
        .as_deref()
        .or(result.call_id.as_deref())
        .or(result.record_id.as_deref())?;
    serde_json::to_string(&(
        result.source_format,
        &result.session_id,
        identity,
        result.revision,
    ))
    .ok()
}

fn user_message_identity(request: &UserMessage) -> Option<String> {
    serde_json::to_string(&(
        request.source_format,
        &request.session_id,
        &request.message_id,
    ))
    .ok()
}

impl CompositeSink {
    pub fn new(metrics: SessionMetricsAccumulator, evidence: SessionEvidenceAccumulator) -> Self {
        Self {
            metrics,
            evidence,
            turn_rows: None,
            summary: None,
            pre_finish: None,
            tool_result_observations: Vec::new(),
            tool_result_observation_sizes: Vec::new(),
            native_tool_result_indices: HashMap::new(),
            native_tool_result_conflicts: BTreeSet::new(),
            tool_result_observations_bytes: 0,
            tool_result_observations_complete: true,
            user_messages: Vec::new(),
            user_message_indices: HashMap::new(),
            user_message_conflicts: BTreeSet::new(),
            user_messages_bytes: 0,
            user_messages_complete: true,
        }
    }

    /// Like [`Self::new`], with a [`TurnRowSink`] fanned out alongside
    /// metrics and evidence. Every recorded `MetricsEvent` becomes a turn
    /// row through it, in the same pass that builds metrics and evidence.
    pub fn with_turn_rows(
        metrics: SessionMetricsAccumulator,
        evidence: SessionEvidenceAccumulator,
        turn_rows: TurnRowSink,
    ) -> Self {
        Self {
            metrics,
            evidence,
            turn_rows: Some(turn_rows),
            summary: None,
            pre_finish: None,
            tool_result_observations: Vec::new(),
            tool_result_observation_sizes: Vec::new(),
            native_tool_result_indices: HashMap::new(),
            native_tool_result_conflicts: BTreeSet::new(),
            tool_result_observations_bytes: 0,
            tool_result_observations_complete: true,
            user_messages: Vec::new(),
            user_message_indices: HashMap::new(),
            user_message_conflicts: BTreeSet::new(),
            user_messages_bytes: 0,
            user_messages_complete: true,
        }
    }

    pub fn metrics(&self) -> Option<SessionMetrics> {
        self.evidence.can_publish().then(|| self.metrics.metrics())
    }

    /// Returns the bounded recent window of native tool-result observations.
    /// Exact replays appear once, and conflicting revisions do not appear.
    /// The collection can be incomplete; check
    /// [`Self::tool_result_observations_complete`] before treating it as exhaustive.
    pub fn tool_result_observations(&self) -> &[ToolResultObservation] {
        &self.tool_result_observations
    }

    /// Returns whether the sink retained every distinct observation without
    /// conflicting content or a count/byte limit overflow.
    pub fn tool_result_observations_complete(&self) -> bool {
        self.tool_result_observations_complete
    }

    /// Returns retained user-slot messages in first-seen source order.
    /// Exact replays appear once, and conflicting identities do not appear.
    /// Check [`Self::user_messages_complete`] before treating the
    /// collection as exhaustive.
    pub fn user_messages(&self) -> &[UserMessage] {
        &self.user_messages
    }

    /// Returns whether every distinct user-slot message was retained
    /// without unreadable facts, conflicts, or a count/byte limit overflow.
    pub fn user_messages_complete(&self) -> bool {
        self.user_messages_complete
    }

    /// Restores native tool-result state from a prior resume snapshot.
    /// Call this before recording new results for the resumed stream. Exact
    /// replays then deduplicate against the restored observations. Invalid
    /// JSON entries and entries that exceed a collection limit are not
    /// retained, and they make the collection incomplete. The snapshot's
    /// existing incomplete state and conflict identities are also restored.
    pub fn restore_tool_result_observations(&mut self, snapshot: &StreamSnapshot) {
        self.tool_result_observations.clear();
        self.tool_result_observation_sizes.clear();
        self.native_tool_result_indices.clear();
        self.native_tool_result_conflicts.clear();
        self.tool_result_observations_bytes = 0;
        self.tool_result_observations_complete = snapshot.tool_result_observations_complete;
        for key in &snapshot.native_tool_result_conflicts {
            if self.native_tool_result_conflicts.contains("") {
                self.tool_result_observations_complete = false;
                continue;
            }
            let next_bytes = self
                .tool_result_observations_bytes
                .saturating_add(key.len());
            if self.native_tool_result_conflicts.len() >= MAX_TOOL_RESULT_OBSERVATIONS
                || next_bytes > MAX_TOOL_RESULT_BYTES
            {
                self.tool_result_observations_complete = false;
                self.block_native_tool_results_after_conflict_overflow();
                continue;
            }
            self.native_tool_result_conflicts.insert(key.clone());
            self.tool_result_observations_bytes = next_bytes;
        }
        for encoded in &snapshot.tool_result_observations_json {
            let Ok(result) = serde_json::from_str::<ToolResultObservation>(encoded) else {
                self.tool_result_observations_complete = false;
                continue;
            };
            self.observe_native_tool_result(result);
        }
    }

    /// Restores user-message state from a prior resume snapshot.
    /// Invalid JSON entries, unbounded facts, and entries that exceed a
    /// collection limit are not retained and make the collection incomplete.
    /// Existing incompleteness and conflict identities are restored too.
    pub fn restore_user_messages(&mut self, snapshot: &StreamSnapshot) {
        self.user_messages.clear();
        self.user_message_indices.clear();
        self.user_message_conflicts.clear();
        self.user_messages_bytes = 0;
        self.user_messages_complete = snapshot.user_messages_complete;
        if !snapshot.user_message_conflicts.is_empty() {
            self.user_messages_complete = false;
        }
        for key in &snapshot.user_message_conflicts {
            if self.user_message_conflicts.contains("") {
                self.user_messages_complete = false;
                continue;
            }
            let next_bytes = self.user_messages_bytes.saturating_add(key.len());
            if self.user_message_conflicts.len() >= MAX_USER_MESSAGE_OBSERVATIONS
                || next_bytes > MAX_USER_MESSAGE_COLLECTION_BYTES
            {
                self.user_messages_complete = false;
                self.block_user_messages_after_conflict_overflow();
                continue;
            }
            self.user_message_conflicts.insert(key.clone());
            self.user_messages_bytes = next_bytes;
        }
        let conflict_overflowed = self.user_message_conflicts.remove("");
        for encoded in &snapshot.user_messages_json {
            let Ok(request) = serde_json::from_str::<UserMessage>(encoded) else {
                self.user_messages_complete = false;
                continue;
            };
            self.observe_user_message(request);
        }
        if conflict_overflowed {
            self.block_user_messages_after_conflict_overflow();
        }
    }

    pub(super) fn observe_user_message(&mut self, request: UserMessage) {
        if self.user_message_conflicts.contains("") {
            self.user_messages_complete = false;
            return;
        }
        if !request.is_bounded() {
            self.user_messages_complete = false;
            return;
        }
        let Some(key) = user_message_identity(&request) else {
            self.user_messages_complete = false;
            return;
        };
        if request.change == crate::analysis::UserMessageChange::Conflict {
            self.mark_user_message_conflict(key);
            return;
        }
        if self.user_message_conflicts.contains(&key) {
            self.user_messages_complete = false;
            return;
        }
        if let Some(index) = self.user_message_indices.get(&key).copied() {
            if self.user_messages[index] != request {
                self.mark_user_message_conflict(key);
            }
            return;
        }
        self.retain_user_message(request, key);
    }

    fn mark_user_message_conflict(&mut self, key: String) {
        if self.user_message_conflicts.contains("") {
            self.user_messages_complete = false;
            return;
        }
        if let Some(index) = self.user_message_indices.get(&key).copied() {
            let removed = self.user_messages.remove(index);
            self.user_messages_bytes = self
                .user_messages_bytes
                .saturating_sub(serde_json::to_string(&removed).map_or(0, |s| s.len()));
            self.rebuild_user_message_indices();
        }
        if !self.user_message_conflicts.contains(&key) {
            let next_bytes = self.user_messages_bytes.saturating_add(key.len());
            if self.user_message_conflicts.len() < MAX_USER_MESSAGE_OBSERVATIONS
                && next_bytes <= MAX_USER_MESSAGE_COLLECTION_BYTES
            {
                self.user_message_conflicts.insert(key);
                self.user_messages_bytes = next_bytes;
            } else {
                self.block_user_messages_after_conflict_overflow();
            }
        }
        self.user_messages_complete = false;
    }

    fn retain_user_message(&mut self, request: UserMessage, key: String) {
        let Ok(encoded) = serde_json::to_string(&request) else {
            self.user_messages_complete = false;
            return;
        };
        while !self.user_messages.is_empty()
            && (self.user_messages.len() >= MAX_USER_MESSAGE_OBSERVATIONS
                || self.user_messages_bytes.saturating_add(encoded.len())
                    > MAX_USER_MESSAGE_COLLECTION_BYTES)
        {
            let removed = self.user_messages.remove(0);
            self.user_messages_bytes = self
                .user_messages_bytes
                .saturating_sub(serde_json::to_string(&removed).map_or(0, |value| value.len()));
            self.user_messages_complete = false;
            self.rebuild_user_message_indices();
        }
        let next_bytes = self.user_messages_bytes.saturating_add(encoded.len());
        if self.user_messages.len() >= MAX_USER_MESSAGE_OBSERVATIONS
            || next_bytes > MAX_USER_MESSAGE_COLLECTION_BYTES
        {
            self.user_messages_complete = false;
            return;
        }
        self.user_messages_bytes = next_bytes;
        let index = self.user_messages.len();
        self.user_message_indices.insert(key, index);
        self.user_messages.push(request);
    }

    fn rebuild_user_message_indices(&mut self) {
        self.user_message_indices.clear();
        for (index, request) in self.user_messages.iter().enumerate() {
            if let Some(key) = user_message_identity(request) {
                self.user_message_indices.insert(key, index);
            }
        }
    }

    fn block_user_messages_after_conflict_overflow(&mut self) {
        self.user_messages_bytes = self
            .user_messages_bytes
            .saturating_sub(self.user_message_conflicts.iter().map(String::len).sum());
        self.user_message_conflicts.clear();
        self.user_message_conflicts.insert(String::new());
        self.user_messages_complete = false;
    }

    pub(super) fn observe_native_tool_result(&mut self, result: ToolResultObservation) {
        if self.native_tool_result_conflicts.contains("") {
            self.tool_result_observations_complete = false;
            return;
        }
        let Some(key) = tool_result_revision_key(&result) else {
            self.retain_native_tool_result(result);
            return;
        };
        if self.native_tool_result_conflicts.contains(&key) {
            self.tool_result_observations_complete = false;
            return;
        }
        if let Some(index) = self.native_tool_result_indices.get(&key).copied() {
            if self.tool_result_observations[index] != result {
                let removed_size = self.tool_result_observation_sizes.remove(index);
                self.tool_result_observations.remove(index);
                self.tool_result_observations_bytes = self
                    .tool_result_observations_bytes
                    .saturating_sub(removed_size);
                let next_bytes = self
                    .tool_result_observations_bytes
                    .saturating_add(key.len());
                if self.native_tool_result_conflicts.len() < MAX_TOOL_RESULT_OBSERVATIONS
                    && next_bytes <= MAX_TOOL_RESULT_BYTES
                {
                    self.native_tool_result_conflicts.insert(key);
                    self.tool_result_observations_bytes = next_bytes;
                } else {
                    self.block_native_tool_results_after_conflict_overflow();
                }
                self.rebuild_native_tool_result_indices();
                self.tool_result_observations_complete = false;
            }
            return;
        }
        self.retain_native_tool_result(result);
    }

    fn retain_native_tool_result(&mut self, result: ToolResultObservation) {
        let Ok(encoded) = serde_json::to_string(&result) else {
            self.tool_result_observations_complete = false;
            return;
        };
        if encoded.len() > MAX_TOOL_RESULT_BYTES {
            self.tool_result_observations_complete = false;
            return;
        }
        let mut evict = 0;
        let mut retained_bytes = self.tool_result_observations_bytes;
        while evict < self.tool_result_observations.len()
            && (self.tool_result_observations.len() - evict >= MAX_TOOL_RESULT_OBSERVATIONS
                || retained_bytes.saturating_add(encoded.len()) > MAX_TOOL_RESULT_BYTES)
        {
            retained_bytes =
                retained_bytes.saturating_sub(self.tool_result_observation_sizes[evict]);
            evict += 1;
        }
        if evict > 0 {
            self.tool_result_observations.drain(..evict);
            self.tool_result_observation_sizes.drain(..evict);
            self.tool_result_observations_bytes = retained_bytes;
            self.tool_result_observations_complete = false;
            self.rebuild_native_tool_result_indices();
        }
        let next_bytes = self
            .tool_result_observations_bytes
            .saturating_add(encoded.len());
        if self.tool_result_observations.len() >= MAX_TOOL_RESULT_OBSERVATIONS
            || next_bytes > MAX_TOOL_RESULT_BYTES
        {
            self.tool_result_observations_complete = false;
            return;
        }
        self.tool_result_observations_bytes = next_bytes;
        self.tool_result_observation_sizes.push(encoded.len());
        let index = self.tool_result_observations.len();
        if let Some(key) = tool_result_revision_key(&result) {
            self.native_tool_result_indices.insert(key, index);
        }
        self.tool_result_observations.push(result);
    }

    fn rebuild_native_tool_result_indices(&mut self) {
        self.native_tool_result_indices.clear();
        for (index, result) in self.tool_result_observations.iter().enumerate() {
            if let Some(key) = tool_result_revision_key(result) {
                self.native_tool_result_indices.insert(key, index);
            }
        }
    }

    fn block_native_tool_results_after_conflict_overflow(&mut self) {
        self.tool_result_observations_bytes = self.tool_result_observations_bytes.saturating_sub(
            self.native_tool_result_conflicts
                .iter()
                .map(String::len)
                .sum(),
        );
        self.native_tool_result_conflicts.clear();
        self.native_tool_result_conflicts.insert(String::new());
        self.tool_result_observations_complete = false;
    }

    /// This source's own `SessionSummary`, once [`RecordSink::finish`] has
    /// run. `None` before `finish`, and for a source `finish` never runs
    /// for (an unreadable source the caller skipped).
    pub fn summary(&self) -> Option<&SessionSummary> {
        self.summary.as_ref()
    }

    /// `None` when there is no fanned-out [`TurnRowSink`] — a pass without a
    /// row store publishes no evidence — or when the finished residual
    /// cannot publish yet. Otherwise reads the row-derived facts back out
    /// of the store and builds evidence from them. A query error makes
    /// this return `None`.
    ///
    /// `&self`, not `&mut self`: [`RecordSink::finish`] already flushes the
    /// row sink's buffer, so by the time a caller asks for evidence the
    /// buffer is empty and the store's own query sees every row.
    pub fn evidence(&self) -> Option<SessionEvidence> {
        if !self.evidence.can_publish() {
            return None;
        }
        let turn_rows = self.turn_rows.as_ref()?;
        match turn_rows.query_turn_facts() {
            Ok(facts) => Some(self.evidence.evidence(&facts)),
            Err(_) => None,
        }
    }

    /// This residual's [`SessionCoverageRecord`] snapshot, once it can
    /// publish — `None` under the same rule [`Self::evidence`] returns
    /// `None`. A caller that persists a coverage record alongside turn rows
    /// calls this instead of [`Self::evidence`], since evidence itself now
    /// comes from replaying rows against the persisted record.
    pub fn coverage_record(&self) -> Option<SessionCoverageRecord> {
        self.evidence
            .can_publish()
            .then(|| self.evidence.coverage_record())
    }

    pub fn observe_source_outcome(&mut self, outcome: VisitOutcome) {
        self.evidence.observe_source_outcome(outcome);
    }

    /// True once the fanned-out [`TurnRowSink`] (if any) has hit a write
    /// error. The caller must not publish this pass's metrics or evidence
    /// when this is true — rows and projections would disagree.
    pub fn turn_row_write_failed(&self) -> bool {
        self.turn_rows.as_ref().is_some_and(TurnRowSink::has_error)
    }

    /// The fanned-out [`TurnRowSink`]'s next `turn_index`, `None` when
    /// there is none. A caller building a resume snapshot needs this
    /// before [`Self::into_parts`] drops the row sink.
    pub fn turn_rows_next_index(&self) -> Option<u64> {
        self.turn_rows.as_ref().map(TurnRowSink::next_index)
    }

    pub fn into_parts(self) -> Option<(SessionMetricsAccumulator, SessionEvidenceAccumulator)> {
        self.evidence
            .can_publish()
            .then_some((self.metrics, self.evidence))
    }

    /// Assembles this source's [`StreamSnapshot`] for the next resume,
    /// combining `resume` (the adapter's own half, from
    /// [`crate::analysis::interface::ResumedVisit::resume`]) with this
    /// sink's own metrics, evidence, and row-index state.
    ///
    /// Uses state captured just *before* the most recent `finish` mutated
    /// it, not after. `finish` can commit a one-time decision from an
    /// incomplete view of the session — `ProgressSlots::flip_to_active`
    /// (`metrics_sink/slots.rs`) fixes its chart-bucket position scale on
    /// the first `finish` that drains a record, using only what that
    /// `finish` has seen so far — and a resumed pass calls `finish` at
    /// every step, not only the last one. Restoring the pre-`finish` state
    /// and running one more `finish` after every future record this
    /// session ever sees — whether resumed again or not — reproduces
    /// exactly what a single continuous pass computes, because a
    /// continuous pass also calls `finish` exactly once, after the same
    /// records. See `crates/antiburn-local/tests/resume_parity.rs`.
    ///
    /// `None` before the first `finish`, or under the same rule
    /// [`Self::evidence`] returns `None` (no fanned-out [`TurnRowSink`], or
    /// the residual cannot publish yet).
    pub fn snapshot(&self, resume: AdapterResume) -> Option<StreamSnapshot> {
        if !self.evidence.can_publish() {
            return None;
        }
        let pre_finish = self.pre_finish.as_ref()?;
        let next_turn_index = self.turn_rows_next_index()?;
        Some(StreamSnapshot {
            revision: RESUME_SNAPSHOT_REVISION,
            resume: resume.point,
            adapter: resume.adapter,
            metrics: pre_finish.metrics.clone(),
            evidence: pre_finish.evidence.clone(),
            next_turn_index,
            tool_result_observations_json: self
                .tool_result_observations
                .iter()
                .filter_map(|result| serde_json::to_string(result).ok())
                .collect(),
            native_tool_result_conflicts: self
                .native_tool_result_conflicts
                .iter()
                .cloned()
                .collect(),
            tool_result_observations_complete: self.tool_result_observations_complete,
            user_messages_json: self
                .user_messages
                .iter()
                .map(|request| {
                    serde_json::to_string(request).expect("a bounded user message always encodes")
                })
                .collect(),
            user_message_conflicts: self.user_message_conflicts.iter().cloned().collect(),
            user_messages_complete: self.user_messages_complete,
        })
    }
}
