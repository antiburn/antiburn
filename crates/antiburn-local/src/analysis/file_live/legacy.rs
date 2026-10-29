//! Convert only the named file cursor revision 4 without replaying its progress.

use super::*;

mod main_64;

#[derive(Serialize, Deserialize)]
struct CursorV4 {
    revision: u32,
    agent: String,
    session_id: String,
    source_format: SourceFormat,
    anchor: NativeFileAnchor,
    baseline_offset: u64,
    start_offset: u64,
    next_record_index: u64,
    #[serde(with = "framer_codec")]
    framer: LiveJsonlFramer,
    adapter: AdapterV4,
    requests: VecDeque<NativeFileRequest>,
    request_ids: IdentityFilter,
    request_bytes: usize,
    history_complete: bool,
    pending_results: Option<PendingResults>,
}

#[derive(Serialize, Deserialize)]
enum AdapterV4 {
    Claude(Box<claude::live::StateV4>),
    Codex(Box<codex::live::StateV4>),
    Pi(Box<pi::live::StateV4>),
}

impl CursorV4 {
    fn convert(self) -> anyhow::Result<NativeFileCursor> {
        self.convert_revision(4)
    }

    fn convert_revision(self, revision: u32) -> anyhow::Result<NativeFileCursor> {
        anyhow::ensure!(
            self.revision == revision,
            NativeFileReadError::InvalidCursor
        );
        let evidence_baseline_offset = self.anchor.boundary;
        let admission_failure = self.pending_results.as_ref().and_then(|pending| {
            pending
                .observations_json
                .iter()
                .filter_map(|json| serde_json::from_str::<ToolResultObservation>(json).ok())
                .filter(|result| {
                    // A legacy non-main result can reflect missing history, not known ownership loss.
                    result.source_format == self.source_format
                        && result.session_id == self.session_id
                        && (result.completeness
                            != super::super::ToolResultCompleteness::UnsafeOwnership
                            || pending.main_session)
                })
                .find_map(|result| {
                    super::super::public_events::unsafe_reason(result.completeness).then(|| {
                        NativeAdmissionFailure {
                            source_format: self.source_format,
                            session_id: self.session_id.clone(),
                            position: super::super::ToolResultPosition::FileBytes {
                                record_start: pending.position.record_start,
                                record_end: pending.position.record_end,
                                item_index: u32::MAX,
                            },
                            reason: result.completeness,
                        }
                    })
                })
        });
        let mut cursor = NativeFileCursor {
            revision: NATIVE_FILE_CURSOR_REVISION,
            agent: self.agent,
            session_id: self.session_id,
            source_format: self.source_format,
            anchor: self.anchor,
            baseline_offset: self.baseline_offset,
            start_offset: self.start_offset,
            next_record_index: self.next_record_index,
            framer: self.framer,
            adapter: match self.adapter {
                AdapterV4::Claude(state) => {
                    Adapter::Claude(Box::new(state.convert(!self.history_complete)))
                }
                AdapterV4::Codex(state) => Adapter::Codex(Box::new(state.convert())),
                AdapterV4::Pi(state) => {
                    Adapter::Pi(Box::new(state.convert(!self.history_complete)))
                }
            },
            requests: self.requests,
            request_ids: self.request_ids,
            request_bytes: self.request_bytes,
            history_complete: self.history_complete,
            pending_results: self.pending_results,
            evidence_baseline_offset,
            pending_evidence: JsonState::default(),
            admission_failure: JsonState(admission_failure.clone()),
            main_64_public_baseline_pending: false,
        };
        if let Some(failure) = admission_failure {
            cursor.requests.clear();
            cursor.request_bytes = 0;
            cursor.pending_evidence.0.push_back(NativeEvidenceEvent {
                fact: NativeEvidenceFact::AdmissionFailure(failure),
                live_eligible: true,
            });
        }
        anyhow::ensure!(cursor.valid(), NativeFileReadError::InvalidCursor);
        cursor.migrate_pre_65_claude_requests()
    }
}

impl NativeFileCursor {
    /// Decode only the JSON body from main parser 64 and file cursor 1.
    /// The next read validates the source before it freezes new-event eligibility.
    pub fn convert_main_64_json(bytes: &[u8]) -> anyhow::Result<Self> {
        main_64::convert(bytes)
    }

    /// Retain the old read position and the validated page's conversion frontier.
    pub fn retry_json_after_page(mut self, page: &Self) -> anyhow::Result<String> {
        if self.main_64_public_baseline_pending {
            anyhow::ensure!(
                self.agent == page.agent
                    && self.session_id == page.session_id
                    && self.source_format == page.source_format
                    && self.anchor.identity == page.anchor.identity
                    && self.anchor.head == page.anchor.head
                    && self.offset() <= page.offset()
                    && !page.main_64_public_baseline_pending,
                NativeFileReadError::InvalidCursor
            );
            self.anchor.boundary = page.anchor.boundary;
            self.evidence_baseline_offset = page.evidence_baseline_offset;
            self.main_64_public_baseline_pending = false;
            anyhow::ensure!(self.valid(), NativeFileReadError::InvalidCursor);
        }
        Ok(serde_json::to_string(&self)?)
    }

    /// Remove request text whose pre-65 Claude state does not retain origin proof.
    /// Call this only for a cursor saved before parser revision 65.
    pub fn migrate_pre_65_claude_requests(mut self) -> anyhow::Result<Self> {
        if self.source_format != SourceFormat::ClaudeJsonl {
            return Ok(self);
        }
        anyhow::ensure!(self.valid(), NativeFileReadError::InvalidCursor);
        if self.start_offset > 0
            && self.next_record_index == 0
            && self.admission_failure.0.is_none()
            && let Adapter::Claude(state) = &mut self.adapter
        {
            state.migrate_pre_65_unobserved_suffix();
        }
        self.requests.clear();
        self.request_bytes = 0;
        self.history_complete = false;
        if let Some(pending) = &mut self.pending_results {
            pending.requests.clear();
            pending.history_complete = false;
            for json in &mut pending.observations_json {
                let mut result: ToolResultObservation =
                    serde_json::from_str(json).map_err(|_| NativeFileReadError::InvalidCursor)?;
                strip_result_requests(&mut result);
                *json = serde_json::to_string(&result)?;
            }
        }
        self.pending_evidence
            .0
            .retain_mut(|event| match &mut event.fact {
                NativeEvidenceFact::Message(message) => strip_message_requests(message),
                NativeEvidenceFact::ResponseCycle(cycle) => {
                    cycle.context.user_messages.clear();
                    cycle.context.complete_through_event = false;
                    cycle.messages.retain_mut(strip_message_requests);
                    true
                }
                _ => true,
            });
        Ok(self)
    }

    pub fn convert_v4(bytes: &[u8]) -> anyhow::Result<Self> {
        anyhow::ensure!(
            bytes.len() <= 32 * 1024 * 1024,
            NativeFileReadError::InvalidCursor
        );
        let (legacy, remaining): (CursorV4, _) =
            postcard::take_from_bytes(bytes).map_err(|_| NativeFileReadError::InvalidCursor)?;
        anyhow::ensure!(remaining.is_empty(), NativeFileReadError::InvalidCursor);
        legacy.convert()
    }

    pub fn convert_v4_json(bytes: &[u8]) -> anyhow::Result<Self> {
        anyhow::ensure!(
            bytes.len() <= 64 * 1024 * 1024,
            NativeFileReadError::InvalidCursor
        );
        serde_json::from_slice::<CursorV4>(bytes)
            .map_err(|_| NativeFileReadError::InvalidCursor)?
            .convert()
    }
}

fn strip_result_requests(result: &mut ToolResultObservation) {
    if let Some(context) = &mut result.context {
        context.user_messages.clear();
        context.complete_through_event = false;
    }
}

fn strip_message_requests(message: &mut NativeMessageObservation) -> bool {
    match message {
        NativeMessageObservation::User(_) => false,
        NativeMessageObservation::Public(message) => {
            if let Some(context) = &mut message.context {
                context.user_messages.clear();
                context.complete_through_event = false;
            }
            true
        }
        NativeMessageObservation::ToolResult(result) => {
            strip_result_requests(result);
            true
        }
    }
}

#[cfg(test)]
pub(super) fn encode_v4(cursor: &NativeFileCursor) -> (Vec<u8>, Vec<u8>) {
    let old = CursorV4 {
        revision: 4,
        agent: cursor.agent.clone(),
        session_id: cursor.session_id.clone(),
        source_format: cursor.source_format,
        anchor: cursor.anchor.clone(),
        baseline_offset: cursor.baseline_offset,
        start_offset: cursor.start_offset,
        next_record_index: cursor.next_record_index,
        framer: cursor.framer.clone(),
        adapter: match &cursor.adapter {
            Adapter::Claude(state) => AdapterV4::Claude(Box::new(state.as_v4())),
            Adapter::Codex(state) => AdapterV4::Codex(Box::new(state.as_v4())),
            Adapter::Pi(state) => AdapterV4::Pi(Box::new(state.as_v4())),
        },
        requests: cursor.requests.clone(),
        request_ids: cursor.request_ids.clone(),
        request_bytes: cursor.request_bytes,
        history_complete: cursor.history_complete,
        pending_results: cursor.pending_results.clone(),
    };
    (
        postcard::to_allocvec(&old).unwrap(),
        serde_json::to_vec(&old).unwrap(),
    )
}
