use super::*;

/// Keeps each native session's projections separate within one SQLite read.
pub(super) struct SourceSink {
    parent_id: String,
    parent: CompositeSink,
    children: Vec<(String, CompositeSink)>,
    current_child: Option<usize>,
    capabilities: SourceCapabilities,
    store: Option<Arc<dyn TurnRowStore>>,
}

impl SourceSink {
    pub(super) fn new(
        parent_id: &str,
        parent: CompositeSink,
        capabilities: SourceCapabilities,
        store: Option<Arc<dyn TurnRowStore>>,
    ) -> Self {
        Self {
            parent_id: parent_id.to_owned(),
            parent,
            children: Vec::new(),
            current_child: None,
            capabilities,
            store,
        }
    }

    fn current(&mut self) -> &mut CompositeSink {
        match self.current_child {
            Some(index) => &mut self.children[index].1,
            None => &mut self.parent,
        }
    }

    pub(super) fn into_parts(self) -> (CompositeSink, Vec<(String, CompositeSink)>) {
        (self.parent, self.children)
    }
}

impl RecordSink for SourceSink {
    fn begin_source(&mut self, session_id: &str) {
        if session_id == self.parent_id {
            self.current_child = None;
            return;
        }
        let metrics = SessionMetricsAccumulator::new("opencode", session_id);
        let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "opencode".to_owned(),
            session_id: session_id.to_owned(),
            kind: SourceKind::Sqlite,
            capabilities: self.capabilities,
        });
        let sink = match &self.store {
            Some(store) => CompositeSink::with_turn_rows(
                metrics,
                evidence,
                TurnRowSink::new(Arc::clone(store), session_id, Some(TurnScope::Delegated)),
            ),
            None => CompositeSink::new(metrics, evidence),
        };
        self.current_child = Some(self.children.len());
        self.children.push((session_id.to_owned(), sink));
    }

    fn source_summary(&mut self, _session_id: &str, summary: SessionSummary) {
        self.current().tool_result_gap();
        self.current().finish(summary);
    }

    fn record(&mut self, mut record: NormalizedRecord) {
        if self.current_child.is_some()
            && let NormalizedRecord::MetricsEvent(event) = &mut record
        {
            // Each child's accumulator measures its own context before the inclusive merge.
            event.source = antiburn_local::analysis::EventSource::Parent;
        }
        self.current().record(record);
    }

    fn finish(&mut self, _summary: SessionSummary) {}
}
