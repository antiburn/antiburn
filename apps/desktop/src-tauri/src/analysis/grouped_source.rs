use super::*;

/// Keeps each source's projections separate within one database read.
pub(super) struct SourceSink {
    parent_id: String,
    parent: CompositeSink,
    children: Vec<(String, CompositeSink)>,
    current_child: Option<usize>,
    capabilities: SourceCapabilities,
    agent: String,
    kind: SourceKind,
    roster: Vec<(String, String)>,
    has_source_summaries: bool,
    store: Option<Arc<dyn TurnRowStore>>,
}

pub(super) struct SourceParts {
    pub(super) parent: CompositeSink,
    pub(super) children: Vec<(String, CompositeSink)>,
    pub(super) roster: Vec<(String, String)>,
}

impl SourceSink {
    pub(super) fn new(
        parent_id: &str,
        parent: CompositeSink,
        capabilities: SourceCapabilities,
        agent: &str,
        kind: SourceKind,
        store: Option<Arc<dyn TurnRowStore>>,
    ) -> Self {
        Self {
            parent_id: parent_id.to_owned(),
            parent,
            children: Vec::new(),
            current_child: None,
            capabilities,
            agent: agent.to_owned(),
            kind,
            roster: Vec::new(),
            has_source_summaries: false,
            store,
        }
    }

    fn current(&mut self) -> &mut CompositeSink {
        match self.current_child {
            Some(index) => &mut self.children[index].1,
            None => &mut self.parent,
        }
    }

    pub(super) fn into_parts(self) -> SourceParts {
        SourceParts {
            parent: self.parent,
            children: self.children,
            roster: self.roster,
        }
    }
}

impl RecordSink for SourceSink {
    fn tool_results_requested(&self) -> bool {
        self.parent.tool_results_requested()
    }

    fn user_message(&mut self, message: antiburn_local::analysis::UserMessage) {
        self.current().user_message(message);
    }

    fn tool_result_gap(&mut self) {
        self.current().tool_result_gap();
    }

    fn tool_result_admission_gap(
        &mut self,
        admission: antiburn_local::analysis::ToolResultAdmission,
    ) {
        self.current().tool_result_admission_gap(admission);
    }

    fn source_member(&mut self, session_id: &str, label: &str) {
        if session_id != self.parent_id && !self.roster.iter().any(|(id, _)| id == session_id) {
            self.roster.push((session_id.to_owned(), label.to_owned()));
        }
    }

    fn begin_source(&mut self, session_id: &str) {
        if session_id == self.parent_id {
            self.current_child = None;
            return;
        }
        let sink = new_source_sink(
            &self.agent,
            session_id,
            self.kind,
            self.capabilities,
            self.store.as_ref(),
            Some(TurnScope::Delegated),
        );
        self.current_child = Some(self.children.len());
        self.children.push((session_id.to_owned(), sink));
    }

    fn source_summary(&mut self, _session_id: &str, summary: SessionSummary) {
        self.has_source_summaries = true;
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

    fn finish(&mut self, summary: SessionSummary) {
        if !self.has_source_summaries {
            self.parent.finish(summary);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sink() -> SourceSink {
        let capabilities = SourceCapabilities::generic();
        SourceSink::new(
            "parent",
            new_source_sink(
                "test",
                "parent",
                SourceKind::Sqlite,
                capabilities,
                None,
                None,
            ),
            capabilities,
            "test",
            SourceKind::Sqlite,
            None,
        )
    }

    #[test]
    fn ordinary_database_summaries_reach_the_parent() {
        let mut sink = sink();
        sink.finish(SessionSummary {
            model: Some("test-model".into()),
            ..Default::default()
        });
        let SourceParts {
            parent,
            children,
            roster,
        } = sink.into_parts();
        assert_eq!(
            parent.summary().unwrap().model.as_deref(),
            Some("test-model")
        );
        assert!(children.is_empty());
        assert!(roster.is_empty());
    }

    #[test]
    fn source_members_come_only_from_reader_metadata() {
        let mut sink = sink();
        sink.begin_source("unrelated-source");
        sink.source_member("child", "Review source");
        sink.source_member("child", "Duplicate");
        sink.source_member("parent", "Parent");
        let roster = sink.into_parts().roster;
        assert_eq!(roster, vec![("child".into(), "Review source".into())]);
    }
}
