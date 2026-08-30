use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::analysis::evidence::{
    CacheEvidence, ChurnCounts, CompactionEvidence, ContextEvidence, ContextSourceEvidence,
    CoverageReason, EvidenceCoverage, EvidenceSource, EvidenceValue, LoadedSource,
    MAX_CONTEXT_SOURCES, MAX_EVIDENCE_EXAMPLES, MAX_SUBAGENT_CHILDREN, MAX_TOOL_NAMES,
    MAX_UNRECOGNIZED_TYPES, ModelEvidence, OrderingObservation, ParseDiagnostics,
    RelationConfidence, SessionEvidence, SessionEvidenceIdentity, SessionProvenance,
    SourceAcceptance, SourceCapabilities, SourceKind, SubagentChild, SubagentEvidence,
    SubagentExample, ToolClass, ToolEvidence, ToolUse, cap_string, insert_diagnostic_field,
    record_diagnostic_set_cap,
};
use crate::analysis::evidence_query::TurnFacts;
use crate::analysis::interface::{
    ContextSourceKind, EvidenceObservation, NormalizedRecord, RecordSink, SessionSummary,
    VisitOutcome,
};
use crate::analysis::metrics_sink::SessionMetricsAccumulator;
use crate::analysis::model::NormalizedEvent;
use crate::analysis::rows::TurnRowSink;
use crate::analysis::{
    ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION, SessionMetrics,
};

pub struct SessionEvidenceAccumulator {
    identity: SessionEvidenceIdentity,
    capabilities: SourceCapabilities,
    source_kind: SourceKind,
    source_acceptance: SourceAcceptance,
    ordering: OrderingObservation,
    diagnostics: ParseDiagnostics,
    record_loss_reason: Option<CoverageReason>,
    session_cap_exceeded: bool,
    /// The latest record timestamp seen so far, kept only to detect
    /// out-of-order records. The published time range comes from the
    /// row-derived [`TurnFacts`] instead.
    last_ts_ms: Option<i64>,
    tools: BTreeMap<String, ToolUse>,
    invoked_skills: BTreeSet<String>,
    tools_cap_exceeded: bool,
    skills: BTreeMap<String, LoadedSource>,
    mcp_servers: BTreeMap<String, LoadedSource>,
    context_sources_cap_exceeded: bool,
    subagent_spawn_count: u64,
    subagent_children: Vec<SubagentChild>,
    subagent_examples: Vec<SubagentExample>,
    subagents_cap_exceeded: bool,
    seen_thread_uuids: HashSet<String>,
    thread_parent_unresolved: bool,
    summary_observed: bool,
    /// The worst [`CoverageReason`] any streamed child reported, folded in
    /// by [`Self::observe_child_coverage`]. `None` when every streamed
    /// child (if any) reported none.
    child_loss_reason: Option<CoverageReason>,
}

impl SessionEvidenceAccumulator {
    pub fn new(source: EvidenceSource) -> Self {
        let mut diagnostics = ParseDiagnostics::new();
        let identity =
            SessionEvidenceIdentity::new(&source.agent, &source.session_id, &mut diagnostics);
        let session_cap_exceeded = !diagnostics.truncated_strings.is_empty();
        Self {
            identity,
            capabilities: source.capabilities,
            source_kind: source.kind,
            source_acceptance: SourceAcceptance::NotObserved,
            ordering: OrderingObservation::Monotonic,
            diagnostics,
            record_loss_reason: None,
            session_cap_exceeded,
            last_ts_ms: None,
            tools: BTreeMap::new(),
            invoked_skills: BTreeSet::new(),
            tools_cap_exceeded: false,
            skills: BTreeMap::new(),
            mcp_servers: BTreeMap::new(),
            context_sources_cap_exceeded: false,
            subagent_spawn_count: 0,
            subagent_children: Vec::new(),
            subagent_examples: Vec::new(),
            subagents_cap_exceeded: false,
            seen_thread_uuids: HashSet::new(),
            thread_parent_unresolved: false,
            summary_observed: false,
            child_loss_reason: None,
        }
    }

    /// Folds one record without taking it.
    pub fn observe(&mut self, record: &NormalizedRecord) {
        match record {
            NormalizedRecord::MetricsEvent(event) => {
                self.diagnostics.records_observed =
                    self.diagnostics.records_observed.saturating_add(1);
                self.observe_event(event);
            }
            NormalizedRecord::Observation(observation) => self.observe_observation(observation),
            NormalizedRecord::TurnContent(_) => {}
            NormalizedRecord::Unusable(reason) => {
                self.diagnostics.records_observed =
                    self.diagnostics.records_observed.saturating_add(1);
                let reason = CoverageReason::from(*reason);
                self.diagnostics.records_unusable =
                    self.diagnostics.records_unusable.saturating_add(1);
                let count = self.diagnostics.unusable_reasons.entry(reason).or_default();
                *count = count.saturating_add(1);
                self.set_record_loss_reason(reason);
            }
        }
    }

    fn observe_event(&mut self, event: &NormalizedEvent) {
        // Turn counts, context depth, per-model tokens, cache accounting,
        // and compaction boundaries all come from the row-derived
        // `TurnFacts` now. This only tracks ordering (from the timestamp)
        // and tool usage — neither of which a row query can give back,
        // since a tool call is not its own row and ordering must be seen
        // live to catch an out-of-order record.
        if let Some(timestamp) = event.ts_ms {
            if self.last_ts_ms.is_some_and(|last| timestamp < last) {
                self.ordering = OrderingObservation::OutOfOrder;
            }
            self.last_ts_ms = Some(timestamp);
        }

        for tool in &event.tools {
            let source_name = if tool.name.eq_ignore_ascii_case("skill") {
                tool.detail.as_deref().unwrap_or(&tool.name)
            } else {
                &tool.name
            };
            let name = cap_string("tools.by_name", source_name, &mut self.diagnostics);
            if name.len() != source_name.len() {
                self.tools_cap_exceeded = true;
            }
            if tool.name.eq_ignore_ascii_case("skill") {
                self.invoked_skills.insert(name.clone());
            }
            if let Some(entry) = self.tools.get_mut(&name) {
                entry.calls = entry.calls.saturating_add(1);
            } else if self.tools.len() == MAX_TOOL_NAMES {
                self.tools_cap_exceeded = true;
                self.note_collection_cap("tools.by_name");
            } else {
                self.tools.insert(
                    name,
                    ToolUse {
                        calls: 1,
                        class: ToolClass::Unclassified,
                    },
                );
            }
        }
    }

    fn observe_observation(&mut self, observation: &EvidenceObservation) {
        match observation {
            EvidenceObservation::ContextSource {
                kind,
                name,
                description,
            } => self.observe_context_source(*kind, name, description.as_deref()),
            EvidenceObservation::SubagentSpawn {
                ts_ms,
                parent_model,
                provenance,
            } => self.observe_subagent_spawn(*ts_ms, parent_model.as_deref(), *provenance),
            // Delegated-turn counting and modeling now come entirely from
            // the row-derived `TurnFacts`; the accumulator does not fold
            // this observation.
            EvidenceObservation::DelegatedTurn { .. } => {}
            EvidenceObservation::ThreadLink { uuid, parent_uuid } => {
                // A parent link is verified only against identities this
                // source already declared. An unresolved link (a resumed
                // session pointing into another file, or a lost record)
                // degrades the linkage claim rather than fabricating it.
                if let Some(parent) = parent_uuid
                    && !self.seen_thread_uuids.contains(parent)
                {
                    self.thread_parent_unresolved = true;
                }
                if let Some(uuid) = uuid {
                    self.seen_thread_uuids.insert(uuid.clone());
                }
            }
            EvidenceObservation::RecordTimestamp { ts_ms } => {
                if self.last_ts_ms.is_some_and(|last| *ts_ms < last) {
                    self.ordering = OrderingObservation::OutOfOrder;
                }
                self.last_ts_ms = Some(*ts_ms);
            }
            EvidenceObservation::InheritedRecord => {
                self.diagnostics.records_observed =
                    self.diagnostics.records_observed.saturating_add(1);
            }
            EvidenceObservation::UnrecognizedType {
                discriminator,
                inert,
            } => {
                // The paired Unusable record counts an evidence-bearing unknown.
                if *inert {
                    self.diagnostics.records_observed =
                        self.diagnostics.records_observed.saturating_add(1);
                    self.diagnostics.records_unrecognized_inert = self
                        .diagnostics
                        .records_unrecognized_inert
                        .saturating_add(1);
                }
                let original_len = discriminator.len();
                let discriminator = cap_string(
                    "diagnostics.unrecognized_types",
                    discriminator,
                    &mut self.diagnostics,
                );
                if discriminator.len() != original_len {
                    self.session_cap_exceeded = true;
                    // A truncated discriminator no longer identifies the record format.
                    // Treat the truncation as loss so no supported group reports complete.
                    self.set_record_loss_reason(CoverageReason::CapExceeded);
                }
                if !self.diagnostics.unrecognized_types.contains(&discriminator) {
                    if self.diagnostics.unrecognized_types.len() == MAX_UNRECOGNIZED_TYPES {
                        self.session_cap_exceeded = true;
                        // A capped set means antiburn no longer understands the record format.
                        // Treat the cap as loss so no supported group reports complete.
                        self.set_record_loss_reason(CoverageReason::CapExceeded);
                        self.note_collection_cap("diagnostics.unrecognized_types");
                    } else {
                        self.diagnostics.unrecognized_types.insert(discriminator);
                    }
                }
            }
        }
    }

    fn observe_subagent_spawn(
        &mut self,
        ts_ms: Option<i64>,
        parent_model: Option<&str>,
        provenance: crate::analysis::interface::RelationProvenance,
    ) {
        self.subagent_spawn_count = self.subagent_spawn_count.saturating_add(1);
        let child_parent_model = parent_model.map(|model| {
            let capped = cap_string(
                "subagents.children.parent_model",
                model,
                &mut self.diagnostics,
            );
            if capped.len() != model.len() {
                self.subagents_cap_exceeded = true;
            }
            capped
        });
        if self.subagent_children.len() == MAX_SUBAGENT_CHILDREN {
            self.subagents_cap_exceeded = true;
            self.note_collection_cap("subagents.children");
        } else {
            self.subagent_children.push(SubagentChild {
                ordinal: u32::try_from(self.subagent_spawn_count).unwrap_or(u32::MAX),
                parent_model: child_parent_model,
                child_model: EvidenceValue::Unsupported,
                confidence: RelationConfidence::Observed,
                provenance,
            });
        }
        if let Some(ts_ms) = ts_ms {
            let example_parent_model = parent_model.map(|model| {
                let capped = cap_string(
                    "subagents.examples.parent_model",
                    model,
                    &mut self.diagnostics,
                );
                if capped.len() != model.len() {
                    self.subagents_cap_exceeded = true;
                }
                capped
            });
            if self.subagent_examples.len() == MAX_EVIDENCE_EXAMPLES {
                self.subagents_cap_exceeded = true;
                self.note_collection_cap("subagents.examples");
            } else {
                self.subagent_examples.push(SubagentExample {
                    ts_ms,
                    parent_model: example_parent_model,
                });
            }
        }
    }

    fn observe_context_source(
        &mut self,
        kind: ContextSourceKind,
        name: &str,
        description: Option<&str>,
    ) {
        let (field, description_field, map) = match kind {
            ContextSourceKind::Skill => (
                "context_sources.skills",
                Some("context_sources.skills.description"),
                &mut self.skills,
            ),
            // MCP "descriptions" came from server-injected instruction
            // blocks, which are broader than descriptions and read by
            // nothing downstream, so the sink refuses them outright
            // (#228, Option B).
            ContextSourceKind::McpServer => {
                ("context_sources.mcp_servers", None, &mut self.mcp_servers)
            }
        };
        let capped_name = cap_string(field, name, &mut self.diagnostics);
        if capped_name.len() != name.len() {
            self.context_sources_cap_exceeded = true;
        }
        let capped_description = description_field.and_then(|description_field| {
            description.map(|value| {
                let capped = cap_string(description_field, value, &mut self.diagnostics);
                if capped.len() != value.len() {
                    self.context_sources_cap_exceeded = true;
                }
                capped
            })
        });
        if let Some(existing) = map.get_mut(&capped_name) {
            if existing.description.is_none() {
                existing.description = capped_description;
            }
        } else if map.len() == MAX_CONTEXT_SOURCES {
            self.context_sources_cap_exceeded = true;
            self.note_collection_cap(field);
        } else {
            map.insert(
                capped_name,
                LoadedSource {
                    description: capped_description,
                    invoked: false,
                    origin: EvidenceValue::Unsupported,
                },
            );
        }
    }

    fn note_collection_cap(&mut self, field: &'static str) {
        if insert_diagnostic_field(&mut self.diagnostics.capped_collections, field) {
            self.session_cap_exceeded = true;
            record_diagnostic_set_cap(&mut self.diagnostics, "diagnostics.capped_collections");
        }
    }

    /// Folds one discovered child transcript that could not be read.
    pub fn observe_child_unreadable(&mut self) {
        self.diagnostics.children_discovered =
            self.diagnostics.children_discovered.saturating_add(1);
        self.diagnostics.children_unreadable =
            self.diagnostics.children_unreadable.saturating_add(1);
    }

    /// Folds one streamed child's residual into this (the parent's)
    /// residual. Folds the child's record loss, diagnostics counts, and
    /// out-of-order ordering. Does not fold the child's tools or context
    /// sources — those stay parent-only by design (deferred: a later
    /// change may decide a child's skill or MCP use belongs in the
    /// session's own coverage).
    pub fn observe_child_coverage(&mut self, child: &SessionEvidenceAccumulator) {
        self.diagnostics.children_discovered =
            self.diagnostics.children_discovered.saturating_add(1);
        if let Some(reason) = child.record_loss_reason {
            self.set_child_loss_reason(reason);
        }
        self.fold_child_diagnostics(&child.diagnostics);
        if child.ordering == OrderingObservation::OutOfOrder {
            self.ordering = OrderingObservation::OutOfOrder;
        }
    }

    /// A specific child loss reason replaces a cap reason. The cap is the
    /// weakest claim — the same rule [`Self::set_record_loss_reason`] uses.
    fn set_child_loss_reason(&mut self, reason: CoverageReason) {
        if self.child_loss_reason.is_none()
            || (self.child_loss_reason == Some(CoverageReason::CapExceeded)
                && reason != CoverageReason::CapExceeded)
        {
            self.child_loss_reason = Some(reason);
        }
    }

    /// Folds one child's record diagnostics into the parent's own,
    /// capping the merged collections the same way the parent caps its
    /// own records.
    fn fold_child_diagnostics(&mut self, child: &ParseDiagnostics) {
        self.diagnostics.records_observed = self
            .diagnostics
            .records_observed
            .saturating_add(child.records_observed);
        self.diagnostics.records_unusable = self
            .diagnostics
            .records_unusable
            .saturating_add(child.records_unusable);
        self.diagnostics.records_unrecognized_inert = self
            .diagnostics
            .records_unrecognized_inert
            .saturating_add(child.records_unrecognized_inert);
        for (reason, count) in &child.unusable_reasons {
            let entry = self
                .diagnostics
                .unusable_reasons
                .entry(*reason)
                .or_default();
            *entry = entry.saturating_add(*count);
        }
        for discriminator in &child.unrecognized_types {
            if self.diagnostics.unrecognized_types.contains(discriminator) {
                continue;
            }
            if self.diagnostics.unrecognized_types.len() == MAX_UNRECOGNIZED_TYPES {
                self.session_cap_exceeded = true;
                // A capped set means antiburn no longer understands the record format.
                // Treat the cap as loss so no supported group reports complete.
                self.set_record_loss_reason(CoverageReason::CapExceeded);
                self.note_collection_cap("diagnostics.unrecognized_types");
            } else {
                self.diagnostics
                    .unrecognized_types
                    .insert(discriminator.clone());
            }
        }
    }

    /// Folds the end-of-stream facts without taking them.
    pub fn observe_summary(&mut self, summary: &SessionSummary) {
        self.capabilities.cache_write_tokens = summary.cache_write_tokens_available;
        for reason in &summary.coverage_gaps {
            self.set_record_loss_reason(CoverageReason::from(*reason));
        }
        self.summary_observed = true;
    }

    /// Attaches the source outcome after the adapter returns.
    pub fn observe_source_outcome(&mut self, outcome: VisitOutcome) {
        if matches!(outcome, VisitOutcome::AcceptedPrefix { .. }) {
            self.set_record_loss_reason(CoverageReason::PinnedPrefix);
        }
        self.source_acceptance = SourceAcceptance::from(outcome);
    }

    /// Builds this session's [`SessionEvidence`] from the row-derived
    /// `facts` plus whatever this residual observed directly. Most groups
    /// come from `facts` outright; `tools`, `context_sources`, and the
    /// subagent relationship shape (`spawn_count`, `children`, `examples`)
    /// still come from this residual — a row query has no tool catalog and
    /// no context-source contract, and a child's spawn is only ever seen
    /// live, as an `EvidenceObservation`.
    pub fn evidence(&self, facts: &TurnFacts) -> SessionEvidence {
        // A discovered child that never streamed (or streamed but lost
        // records of its own) makes every group computed over the union of
        // rows — models, subagents, cache, context, eligibility, time
        // range, compactions — no better than that child's worst claim.
        // `record_loss_reason` (this source's own loss) still outranks it.
        // Neither degrades `tools` or `context_sources`: those are
        // parent-only, so a child's coverage cannot touch them. Neither
        // changes the session-level `coverage` — the child's own evidence
        // (when it streamed) already reports its own loss.
        let child_dependent_partial: Option<CoverageReason> = self.child_loss_reason.or({
            (self.diagnostics.children_unreadable > 0).then_some(CoverageReason::ReadFailed)
        });

        let mut diagnostics = self.diagnostics.clone();
        diagnostics.duplicate_turn_identities = facts.duplicate_turn_identities;
        for field in &facts.diagnostics.truncated_strings {
            if insert_diagnostic_field(&mut diagnostics.truncated_strings, field) {
                record_diagnostic_set_cap(&mut diagnostics, "diagnostics.truncated_strings");
            }
        }
        for field in &facts.diagnostics.capped_collections {
            if insert_diagnostic_field(&mut diagnostics.capped_collections, field) {
                record_diagnostic_set_cap(&mut diagnostics, "diagnostics.capped_collections");
            }
        }

        let context = ContextEvidence {
            max_request_context_tokens: facts.max_request_context_tokens,
            top_depth_examples: facts.top_depth_examples.clone(),
        };
        let eligibility = facts.eligibility.clone();
        let tools = self.classified_tools();
        let context_sources = self.context_sources();
        let models = ModelEvidence {
            by_model: facts.by_model.clone(),
            unattributed_turns: facts.unattributed_turns,
            effort_tiers: facts.effort_tiers.clone(),
            fast_modes: facts.fast_modes.clone(),
            service_tiers: EvidenceValue::Unsupported,
            effort_signal: facts.effort_signal,
            speed_signal: facts.speed_signal,
        };
        let models_cap_exceeded = facts.models_capped || facts.tiers_capped;
        let subagents = SubagentEvidence {
            spawn_count: self.subagent_spawn_count,
            delegated_turns: facts.delegated_turns,
            delegated_models: facts.delegated_models.clone(),
            children: self.subagent_children.clone(),
            examples: self.subagent_examples.clone(),
        };
        let subagents_cap_exceeded = self.subagents_cap_exceeded || facts.delegated_models_capped;
        // Verified previous-turn linkage: complete only when every counted
        // turn carried its own identity and every parent link resolved to an
        // identity this source declared earlier. `provider_eviction` stays
        // unsupported — no transcript record states an eviction.
        // `thread_identity_missing` feeds the record-identity claim here: a
        // row with no `uuid` is a record-identity gap, not a thread-identity
        // gap.
        let record_identity_gap = facts.thread_identity_missing || self.thread_parent_unresolved;
        let previous_turn = if !self.capabilities.record_identity {
            EvidenceValue::Unsupported
        } else if let Some(reason) = self.record_loss_reason {
            EvidenceValue::Partial {
                observed: (),
                reason,
            }
        } else if record_identity_gap {
            EvidenceValue::Partial {
                observed: (),
                reason: CoverageReason::AttributionIncomplete,
            }
        } else {
            EvidenceValue::Complete(())
        };
        let cache = CacheEvidence {
            cache_read_tokens: facts.cache_read_tokens,
            cache_creation_tokens: facts.cache_creation_tokens,
            fresh_input_tokens: facts.fresh_input_tokens,
            model_transitions: facts.model_transitions.clone(),
            longest_idle_gap_ms: facts.longest_idle_gap_ms,
            idle_gap_ms_total: facts.idle_gap_ms_total,
            user_controlled_churn: ChurnCounts {
                manual_compactions: facts.manual_compactions,
            },
            previous_turn,
            provider_eviction: EvidenceValue::Unsupported,
        };
        let compactions = CompactionEvidence {
            boundaries: facts.compaction_boundaries.clone(),
        };
        let diagnostic_cap_exceeded = diagnostics
            .capped_collections
            .contains("diagnostics.truncated_strings")
            || diagnostics
                .capped_collections
                .contains("diagnostics.capped_collections");
        let coverage_reason = self
            .record_loss_reason
            .or((self.session_cap_exceeded || diagnostic_cap_exceeded)
                .then_some(CoverageReason::CapExceeded));
        let coverage =
            coverage_reason.map_or(EvidenceCoverage::Complete, EvidenceCoverage::Partial);

        SessionEvidence {
            schema_revision: EVIDENCE_SCHEMA_REVISION,
            identity: self.identity.clone(),
            // No cap can make this group partial. The row query folds the
            // maximum over every row, and it keeps the deepest examples. A
            // dropped example is always less deep than the maximum, so it
            // hides nothing. Record loss and a lossy child still make the
            // group partial.
            context: self.supported_value(
                context,
                self.capabilities.request_context_tokens,
                child_dependent_partial,
                false,
            ),
            capabilities: self.capabilities,
            coverage,
            provenance: SessionProvenance {
                parser_revision: PARSER_REVISION,
                analyzer_revision: ANALYZER_REVISION,
                evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
                source_kind: self.source_kind,
                source_acceptance: self.source_acceptance,
                ordering: self.ordering,
                harness_version: EvidenceValue::Unsupported,
            },
            diagnostics,
            time_range: self.supported_value(
                facts.time_range.clone(),
                self.capabilities.timestamps_and_order,
                child_dependent_partial,
                false,
            ),
            eligibility: self.supported_value(eligibility, true, child_dependent_partial, false),
            tools: self.supported_value(
                ToolEvidence { by_name: tools },
                self.capabilities.tool_invocations,
                None,
                self.tools_cap_exceeded,
            ),
            context_sources: self.supported_value(
                context_sources,
                self.capabilities.skill_mcp_attribution,
                None,
                self.context_sources_cap_exceeded,
            ),
            models: if !self.capabilities.model_identity || !self.capabilities.token_classes {
                EvidenceValue::Unsupported
            } else if let Some(reason) = self.record_loss_reason {
                EvidenceValue::Partial {
                    observed: models,
                    reason,
                }
            } else if let Some(reason) = child_dependent_partial {
                EvidenceValue::Partial {
                    observed: models,
                    reason,
                }
            } else if models_cap_exceeded {
                EvidenceValue::Partial {
                    observed: models,
                    reason: CoverageReason::CapExceeded,
                }
            } else if facts.unattributed_turns > 0 || facts.duplicate_turn_identities > 0 {
                EvidenceValue::Partial {
                    observed: models,
                    reason: CoverageReason::AttributionIncomplete,
                }
            } else {
                EvidenceValue::Complete(models)
            },
            subagents: if !self.capabilities.subagent_relationships {
                EvidenceValue::Unsupported
            } else if let Some(reason) = self.record_loss_reason {
                EvidenceValue::Partial {
                    observed: subagents,
                    reason,
                }
            } else if let Some(reason) = child_dependent_partial {
                EvidenceValue::Partial {
                    observed: subagents,
                    reason,
                }
            } else if subagents_cap_exceeded {
                EvidenceValue::Partial {
                    observed: subagents,
                    reason: CoverageReason::CapExceeded,
                }
            } else if (self.capabilities.subagent_models && facts.delegated_model_missing)
                || facts.duplicate_turn_identities > 0
            {
                EvidenceValue::Partial {
                    observed: subagents,
                    reason: CoverageReason::AttributionIncomplete,
                }
            } else {
                EvidenceValue::Complete(subagents)
            },
            cache: if let Some(reason) = self.record_loss_reason {
                EvidenceValue::Partial {
                    observed: cache,
                    reason,
                }
            } else if let Some(reason) = child_dependent_partial {
                EvidenceValue::Partial {
                    observed: cache,
                    reason,
                }
            } else if facts.transitions_capped {
                EvidenceValue::Partial {
                    observed: cache,
                    reason: CoverageReason::CapExceeded,
                }
            } else if self.capabilities.record_identity && record_identity_gap {
                // The source promised per-record identity but a counted turn
                // lacked it (or a parent link did not resolve): the cache
                // group's linkage claim is incomplete, so the group degrades
                // and Cache Churn cannot read clean from it (no false clean).
                EvidenceValue::Partial {
                    observed: cache,
                    reason: CoverageReason::AttributionIncomplete,
                }
            } else {
                EvidenceValue::Complete(cache)
            },
            compactions: self.supported_value(
                compactions,
                self.capabilities.compaction_boundaries,
                child_dependent_partial,
                facts.compactions_capped,
            ),
            quota_incidents: EvidenceValue::Unsupported,
        }
    }

    fn classified_tools(&self) -> BTreeMap<String, ToolUse> {
        self.tools
            .iter()
            .map(|(name, tool)| {
                let class = if self.invoked_skills.contains(name) || self.skills.contains_key(name)
                {
                    ToolClass::Skill
                } else if self
                    .mcp_servers
                    .keys()
                    .any(|server| name == server || name.contains(server))
                {
                    ToolClass::Mcp
                } else {
                    ToolClass::Unclassified
                };
                (
                    name.clone(),
                    ToolUse {
                        calls: tool.calls,
                        class,
                    },
                )
            })
            .collect()
    }

    fn context_sources(&self) -> ContextSourceEvidence {
        let mut skills = self.skills.clone();
        let mut mcp_servers = self.mcp_servers.clone();
        for (name, source) in &mut skills {
            source.invoked = self.invoked_skills.contains(name) || self.tools.contains_key(name);
        }
        for (name, source) in &mut mcp_servers {
            source.invoked = self
                .tools
                .keys()
                .any(|tool| tool == name || tool.contains(name));
        }
        ContextSourceEvidence {
            skills,
            mcp_servers,
            tool_definitions: EvidenceValue::Unsupported,
        }
    }

    /// `child_dependent_reason` degrades a group that is computed over the
    /// union of rows (parent and child); pass `None` for a parent-only
    /// group such as `tools` or `context_sources`.
    fn supported_value<T>(
        &self,
        observed: T,
        supported: bool,
        child_dependent_reason: Option<CoverageReason>,
        cap_exceeded: bool,
    ) -> EvidenceValue<T> {
        if !supported {
            EvidenceValue::Unsupported
        } else if let Some(reason) = self.record_loss_reason {
            EvidenceValue::Partial { observed, reason }
        } else if let Some(reason) = child_dependent_reason {
            EvidenceValue::Partial { observed, reason }
        } else if cap_exceeded {
            EvidenceValue::Partial {
                observed,
                reason: CoverageReason::CapExceeded,
            }
        } else {
            EvidenceValue::Complete(observed)
        }
    }

    fn set_record_loss_reason(&mut self, reason: CoverageReason) {
        // A specific loss reason replaces a cap reason. The cap is the weakest claim.
        if self.record_loss_reason.is_none()
            || (self.record_loss_reason == Some(CoverageReason::CapExceeded)
                && reason != CoverageReason::CapExceeded)
        {
            self.record_loss_reason = Some(reason);
        }
    }

    fn can_publish(&self) -> bool {
        self.summary_observed
            && !matches!(
                self.source_acceptance,
                SourceAcceptance::NotObserved | SourceAcceptance::SourceChanged
            )
    }
}

impl RecordSink for SessionEvidenceAccumulator {
    fn record(&mut self, record: NormalizedRecord) {
        self.observe(&record);
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.observe_summary(&summary);
    }
}

pub struct CompositeSink {
    metrics: SessionMetricsAccumulator,
    evidence: SessionEvidenceAccumulator,
    turn_rows: Option<TurnRowSink>,
    /// Set once [`Self::evidence`] queries the row store and the query
    /// fails. `Cell` because the query happens from `evidence(&self)` —
    /// see that method's doc comment for why `&self` is enough.
    turn_row_query_failed: Cell<bool>,
}

impl CompositeSink {
    pub fn new(metrics: SessionMetricsAccumulator, evidence: SessionEvidenceAccumulator) -> Self {
        Self {
            metrics,
            evidence,
            turn_rows: None,
            turn_row_query_failed: Cell::new(false),
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
            turn_row_query_failed: Cell::new(false),
        }
    }

    pub fn metrics(&self) -> Option<SessionMetrics> {
        self.evidence.can_publish().then(|| self.metrics.metrics())
    }

    /// `None` when there is no fanned-out [`TurnRowSink`] — a pass without a
    /// row store publishes no evidence — or when the finished residual
    /// cannot publish yet. Otherwise reads the row-derived facts back out
    /// of the store and builds evidence from them. A query error is kept
    /// (see [`Self::turn_row_query_failed`]) and this returns `None`.
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
            Err(_) => {
                self.turn_row_query_failed.set(true);
                None
            }
        }
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

    /// True once a call to [`Self::evidence`] queried the row store and the
    /// query failed. The caller treats this like a write failure: the pass
    /// must not publish metrics whose rows it could not read back.
    pub fn turn_row_query_failed(&self) -> bool {
        self.turn_row_query_failed.get()
    }

    pub fn into_parts(self) -> Option<(SessionMetricsAccumulator, SessionEvidenceAccumulator)> {
        self.evidence
            .can_publish()
            .then_some((self.metrics, self.evidence))
    }
}

impl RecordSink for CompositeSink {
    fn record(&mut self, record: NormalizedRecord) {
        self.evidence.observe(&record);
        if let Some(turn_rows) = &mut self.turn_rows {
            turn_rows.observe(&record);
        }
        self.metrics.record(record);
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.evidence.observe_summary(&summary);
        if let Some(turn_rows) = &mut self.turn_rows {
            turn_rows.flush();
        }
        self.metrics.finish(summary);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::analysis::model::Role;
    use crate::analysis::rows::{MemoryTurnRowStore, TurnRowStore};
    use crate::analysis::{EVIDENCE_STRING_CAP, PartialReason, RawSource, VendorAdapter};

    fn accumulator(request_context_tokens: bool) -> SessionEvidenceAccumulator {
        let mut capabilities = SourceCapabilities::claude();
        capabilities.request_context_tokens = request_context_tokens;
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: "s1".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities,
        })
    }

    /// A `CompositeSink` with a fresh in-memory row store attached, so
    /// `composite.evidence()` has facts to build from.
    fn composite_with_rows(agent: &str, session_id: &str) -> CompositeSink {
        let store = MemoryTurnRowStore::new(agent, session_id);
        let turn_rows = crate::analysis::rows::TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            session_id.to_owned(),
            None,
        );
        CompositeSink::with_turn_rows(
            SessionMetricsAccumulator::new(agent, session_id),
            SessionEvidenceAccumulator::new(EvidenceSource {
                agent: agent.to_owned(),
                session_id: session_id.to_owned(),
                kind: SourceKind::Jsonl,
                capabilities: SourceCapabilities::claude(),
            }),
            turn_rows,
        )
    }

    #[test]
    fn a_recognized_eventless_record_keeps_coverage_complete() {
        let input = crate::analysis::SessionInput {
            agent: "claude".to_owned(),
            session_id: "attachment".to_owned(),
            source: RawSource::Jsonl(
                r#"{"type":"attachment","attachment":{"type":"skill_listing","content":"- orbit: Synthetic source."}}"#.to_owned(),
            ),
        };
        let mut composite = composite_with_rows("claude", "attachment");
        let outcome = crate::analysis::ClaudeAdapter
            .visit(&input, &mut composite)
            .expect("attachment must parse");
        composite.observe_source_outcome(outcome);
        assert_eq!(
            composite.evidence().expect("evidence").coverage,
            EvidenceCoverage::Complete
        );
    }

    #[test]
    fn an_inert_unmodelled_type_keeps_coverage_and_records_its_discriminator() {
        let input = crate::analysis::SessionInput {
            agent: "claude".to_owned(),
            session_id: "unknown".to_owned(),
            source: RawSource::Jsonl(r#"{"type":"telemetry_ping","payload":"private"}"#.to_owned()),
        };
        let mut composite = composite_with_rows("claude", "unknown");
        let outcome = crate::analysis::ClaudeAdapter
            .visit(&input, &mut composite)
            .expect("unknown record must be skipped");
        composite.observe_source_outcome(outcome);
        let evidence = composite.evidence().expect("evidence");
        assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
        assert_eq!(evidence.diagnostics.records_unrecognized_inert, 1);
        assert_eq!(
            evidence.diagnostics.unrecognized_types,
            BTreeSet::from(["telemetry_ping".to_owned()])
        );
    }

    fn identity_string_overflow(long_agent: bool) -> SessionEvidence {
        let source = EvidenceSource {
            agent: if long_agent {
                long_string()
            } else {
                "claude".to_owned()
            },
            session_id: if long_agent {
                "s1".to_owned()
            } else {
                long_string()
            },
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        };
        SessionEvidenceAccumulator::new(source).evidence(&TurnFacts::default())
    }

    #[test]
    fn identity_agent_overflows_to_partial() {
        let evidence = identity_string_overflow(true);
        assert_eq!(evidence.identity.agent.len(), EVIDENCE_STRING_CAP);
        assert_eq!(
            evidence.coverage,
            EvidenceCoverage::Partial(CoverageReason::CapExceeded)
        );
        assert_truncated_string(&evidence, "identity.agent");
    }

    #[test]
    fn a_truncated_identity_string_degrades_session_coverage() {
        let evidence = identity_string_overflow(false);
        assert_eq!(evidence.identity.session_id.len(), EVIDENCE_STRING_CAP);
        assert_eq!(
            evidence.coverage,
            EvidenceCoverage::Partial(CoverageReason::CapExceeded)
        );
        assert_truncated_string(&evidence, "identity.session_id");
    }

    #[test]
    fn a_record_loss_reason_outranks_a_cap_reason_in_coverage() {
        let source = EvidenceSource {
            agent: "a".repeat(512),
            session_id: "s1".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        };
        let mut accumulator = SessionEvidenceAccumulator::new(source);
        accumulator.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
        assert_eq!(
            accumulator.evidence(&TurnFacts::default()).coverage,
            EvidenceCoverage::Partial(CoverageReason::MalformedRecord)
        );
    }

    fn assistant_event(index: usize) -> NormalizedEvent {
        let mut event = NormalizedEvent::new(Role::Assistant);
        event.ts_ms = Some(i64::try_from(index).unwrap());
        event.model = Some(format!("model-{index}"));
        event.usage.input_tokens = u64::try_from(index + 1).unwrap();
        event
    }

    fn assert_cap_partial<T>(value: EvidenceValue<T>) -> T {
        let EvidenceValue::Partial {
            observed,
            reason: CoverageReason::CapExceeded,
        } = value
        else {
            panic!("cap owner must be partial");
        };
        observed
    }

    fn assert_capped_collection(evidence: &SessionEvidence, field: &str) {
        assert!(
            evidence.diagnostics.capped_collections.contains(field),
            "missing capped collection diagnostic for {field}"
        );
    }

    fn assert_truncated_string(evidence: &SessionEvidence, field: &str) {
        assert!(
            evidence.diagnostics.truncated_strings.contains(field),
            "missing truncated string diagnostic for {field}"
        );
    }

    /// The row query bounds `top_depth_examples`, not the sink — the sink's
    /// own rule is that no cap can make `context` partial. `models.byModel`,
    /// `models.effortTiers`, `models.fastModes`, `cache.modelTransitions`,
    /// `compactions.boundaries`, and `subagents.delegatedModels` caps have
    /// moved with their fields to `query_turn_facts`'s own tests in
    /// `evidence_query.rs`.
    #[test]
    fn context_top_depth_examples_cap_keeps_the_group_complete() {
        let accumulator = accumulator(true);
        let facts = TurnFacts {
            depth_examples_capped: true,
            ..TurnFacts::default()
        };
        let evidence = accumulator.evidence(&facts);
        assert!(
            matches!(evidence.context, EvidenceValue::Complete(_)),
            "an example cap must not make the context group partial"
        );
    }

    #[test]
    fn models_cap_from_facts_overflows_models_to_partial() {
        let accumulator = accumulator(true);
        let facts = TurnFacts {
            models_capped: true,
            ..TurnFacts::default()
        };
        let evidence = accumulator.evidence(&facts);
        assert_cap_partial(evidence.models);
    }

    #[test]
    fn duplicate_turn_identities_degrade_models_and_subagents_to_attribution_incomplete() {
        let accumulator = accumulator(true);
        let facts = TurnFacts {
            duplicate_turn_identities: 1,
            ..TurnFacts::default()
        };
        let evidence = accumulator.evidence(&facts);
        assert!(matches!(
            evidence.models,
            EvidenceValue::Partial {
                reason: CoverageReason::AttributionIncomplete,
                ..
            }
        ));
        assert!(matches!(
            evidence.subagents,
            EvidenceValue::Partial {
                reason: CoverageReason::AttributionIncomplete,
                ..
            }
        ));
        assert_eq!(evidence.diagnostics.duplicate_turn_identities, 1);
    }

    #[test]
    fn observe_child_coverage_with_a_lossy_child_degrades_child_dependent_groups_but_not_tools() {
        let mut parent = accumulator(true);
        let mut child = accumulator(true);
        child.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));

        parent.observe_child_coverage(&child);
        let evidence = parent.evidence(&TurnFacts::default());

        for reason in [
            evidence_reason(&evidence.models),
            evidence_reason(&evidence.subagents),
            evidence_reason(&evidence.cache),
            evidence_reason(&evidence.context),
            evidence_reason(&evidence.eligibility),
            evidence_reason(&evidence.time_range),
            evidence_reason(&evidence.compactions),
        ] {
            assert_eq!(reason, Some(CoverageReason::MalformedRecord));
        }
        // Tools stay parent-only: a child's loss does not reach them.
        assert!(matches!(evidence.tools, EvidenceValue::Complete(_)));
        // A child's loss does not change the session-level coverage either.
        assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    }

    #[test]
    fn observe_child_unreadable_degrades_child_dependent_groups_with_read_failed() {
        let mut parent = accumulator(true);
        parent.observe_child_unreadable();
        let evidence = parent.evidence(&TurnFacts::default());

        assert_eq!(
            evidence_reason(&evidence.models),
            Some(CoverageReason::ReadFailed)
        );
        assert_eq!(
            evidence_reason(&evidence.subagents),
            Some(CoverageReason::ReadFailed)
        );
        assert!(matches!(evidence.tools, EvidenceValue::Complete(_)));
        assert_eq!(evidence.diagnostics.children_discovered, 1);
        assert_eq!(evidence.diagnostics.children_unreadable, 1);
        assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    }

    /// The `CoverageReason` an `EvidenceValue` carries, or `None` for
    /// `Complete`. Panics on `Unsupported` — every group this helper checks
    /// is supported by the Claude capability set `accumulator` uses.
    fn evidence_reason<T>(value: &EvidenceValue<T>) -> Option<CoverageReason> {
        match value {
            EvidenceValue::Complete(_) => None,
            EvidenceValue::Partial { reason, .. } => Some(*reason),
            EvidenceValue::Unsupported => panic!("group must be supported"),
        }
    }

    #[test]
    fn tools_by_name_overflows_to_partial() {
        let mut accumulator = accumulator(true);
        for index in 0..(MAX_TOOL_NAMES * 2) {
            let mut event = assistant_event(index);
            event.model = Some("model".to_owned());
            event
                .tools
                .push(crate::analysis::ToolCall::new(format!("tool-{index}")));
            accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));
        }
        let evidence = accumulator.evidence(&TurnFacts::default());
        assert_capped_collection(&evidence, "tools.by_name");
        assert_eq!(
            assert_cap_partial(evidence.tools).by_name.len(),
            MAX_TOOL_NAMES
        );
    }

    fn context_sources_overflow(kind: ContextSourceKind) -> SessionEvidence {
        let mut accumulator = accumulator(true);
        for index in 0..(MAX_CONTEXT_SOURCES * 2) {
            accumulator.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::ContextSource {
                    kind,
                    name: format!("source-{index}"),
                    description: Some("Synthetic description.".to_owned()),
                },
            )));
        }
        accumulator.evidence(&TurnFacts::default())
    }

    #[test]
    fn context_sources_skills_overflows_to_partial() {
        let evidence = context_sources_overflow(ContextSourceKind::Skill);
        assert_capped_collection(&evidence, "context_sources.skills");
        assert_eq!(
            assert_cap_partial(evidence.context_sources).skills.len(),
            MAX_CONTEXT_SOURCES
        );
    }

    #[test]
    fn context_sources_mcp_servers_overflows_to_partial() {
        let evidence = context_sources_overflow(ContextSourceKind::McpServer);
        assert_capped_collection(&evidence, "context_sources.mcp_servers");
        assert_eq!(
            assert_cap_partial(evidence.context_sources)
                .mcp_servers
                .len(),
            MAX_CONTEXT_SOURCES
        );
    }

    fn subagents_overflow() -> SessionEvidence {
        let mut accumulator = accumulator(true);
        for index in 0..(MAX_SUBAGENT_CHILDREN * 2) {
            accumulator.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::SubagentSpawn {
                    ts_ms: Some(i64::try_from(index).unwrap()),
                    parent_model: Some("model".to_owned()),
                    provenance: crate::analysis::RelationProvenance::TaskToolUse,
                },
            )));
        }
        accumulator.evidence(&TurnFacts::default())
    }

    #[test]
    fn subagents_children_overflows_to_partial() {
        let evidence = subagents_overflow();
        assert_capped_collection(&evidence, "subagents.children");
        assert_eq!(
            assert_cap_partial(evidence.subagents).children.len(),
            MAX_SUBAGENT_CHILDREN
        );
    }

    #[test]
    fn subagents_examples_overflows_to_partial() {
        let evidence = subagents_overflow();
        assert_capped_collection(&evidence, "subagents.examples");
        assert_eq!(
            assert_cap_partial(evidence.subagents).examples.len(),
            MAX_EVIDENCE_EXAMPLES
        );
    }

    #[test]
    fn an_inert_unrecognized_record_keeps_complete_coverage() {
        let mut accumulator = accumulator(true);
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::UnrecognizedType {
                discriminator: "telemetry_ping".to_owned(),
                inert: true,
            },
        )));

        let evidence = accumulator.evidence(&TurnFacts::default());
        assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
        assert_eq!(evidence.diagnostics.records_observed, 1);
        assert_eq!(evidence.diagnostics.records_unrecognized_inert, 1);
        assert_eq!(evidence.diagnostics.records_unusable, 0);
        assert!(evidence.diagnostics.unusable_reasons.is_empty());
        assert!(
            evidence
                .diagnostics
                .unrecognized_types
                .contains("telemetry_ping")
        );
    }

    #[test]
    fn an_evidence_bearing_unrecognized_record_still_fails_closed() {
        let mut accumulator = accumulator(true);
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::UnrecognizedType {
                discriminator: "telemetry_ping".to_owned(),
                inert: false,
            },
        )));
        accumulator.record(NormalizedRecord::Unusable(
            crate::analysis::framing::PartialReason::UnrecognizedRecordType,
        ));

        let evidence = accumulator.evidence(&TurnFacts::default());
        assert_eq!(
            evidence.coverage,
            EvidenceCoverage::Partial(CoverageReason::UnrecognizedRecordType)
        );
        assert_eq!(evidence.diagnostics.records_observed, 1);
        assert_eq!(evidence.diagnostics.records_unrecognized_inert, 0);
        assert_eq!(evidence.diagnostics.records_unusable, 1);
        assert!(matches!(
            evidence.context,
            EvidenceValue::Partial {
                reason: CoverageReason::UnrecognizedRecordType,
                ..
            }
        ));
        assert!(matches!(
            evidence.time_range,
            EvidenceValue::Partial {
                reason: CoverageReason::UnrecognizedRecordType,
                ..
            }
        ));
        assert!(matches!(
            evidence.eligibility,
            EvidenceValue::Partial {
                reason: CoverageReason::UnrecognizedRecordType,
                ..
            }
        ));
    }

    #[test]
    fn diagnostics_unrecognized_types_overflows_to_partial() {
        let mut accumulator = accumulator(true);
        for index in 0..(MAX_UNRECOGNIZED_TYPES * 2) {
            accumulator.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::UnrecognizedType {
                    discriminator: format!("type-{index}"),
                    inert: true,
                },
            )));
        }
        let evidence = accumulator.evidence(&TurnFacts::default());
        assert_eq!(
            evidence.diagnostics.unrecognized_types.len(),
            MAX_UNRECOGNIZED_TYPES
        );
        assert_eq!(
            evidence.coverage,
            EvidenceCoverage::Partial(CoverageReason::CapExceeded)
        );
        assert_capped_collection(&evidence, "diagnostics.unrecognized_types");
        assert_eq!(
            evidence.diagnostics.records_unrecognized_inert,
            (MAX_UNRECOGNIZED_TYPES * 2) as u64
        );
    }

    #[test]
    fn a_capped_inert_session_and_an_evidence_bearing_record_report_the_loss_reason() {
        let mut accumulator = accumulator(true);
        for index in 0..=MAX_UNRECOGNIZED_TYPES {
            accumulator.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::UnrecognizedType {
                    discriminator: format!("type-{index}"),
                    inert: true,
                },
            )));
        }
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::UnrecognizedType {
                discriminator: "bearing".to_owned(),
                inert: false,
            },
        )));
        accumulator.record(NormalizedRecord::Unusable(
            crate::analysis::framing::PartialReason::UnrecognizedRecordType,
        ));

        assert_eq!(
            accumulator.evidence(&TurnFacts::default()).coverage,
            EvidenceCoverage::Partial(CoverageReason::UnrecognizedRecordType)
        );
    }

    #[test]
    fn diagnostics_capped_collections_overflows_to_partial() {
        let mut accumulator = accumulator(true);
        for field in [
            "f00", "f01", "f02", "f03", "f04", "f05", "f06", "f07", "f08", "f09", "f10", "f11",
            "f12", "f13", "f14", "f15", "f16",
        ] {
            accumulator.note_collection_cap(field);
        }
        let evidence = accumulator.evidence(&TurnFacts::default());
        assert_eq!(
            evidence.diagnostics.capped_collections.len(),
            crate::analysis::evidence::MAX_DIAGNOSTIC_FIELDS
        );
        assert_eq!(
            evidence.coverage,
            EvidenceCoverage::Partial(CoverageReason::CapExceeded)
        );
        assert_capped_collection(&evidence, "diagnostics.capped_collections");
    }

    #[test]
    fn diagnostics_truncated_strings_overflows_to_partial() {
        let mut accumulator = accumulator(true);
        let long = "x".repeat(EVIDENCE_STRING_CAP * 2);
        for field in [
            "f00", "f01", "f02", "f03", "f04", "f05", "f06", "f07", "f08", "f09", "f10", "f11",
            "f12", "f13", "f14", "f15", "f16",
        ] {
            cap_string(field, &long, &mut accumulator.diagnostics);
        }
        let evidence = accumulator.evidence(&TurnFacts::default());
        assert_eq!(
            evidence.diagnostics.truncated_strings.len(),
            crate::analysis::evidence::MAX_DIAGNOSTIC_FIELDS
        );
        assert_eq!(
            evidence.coverage,
            EvidenceCoverage::Partial(CoverageReason::CapExceeded)
        );
        assert_capped_collection(&evidence, "diagnostics.truncated_strings");
    }

    fn long_string() -> String {
        "x".repeat(EVIDENCE_STRING_CAP * 2)
    }

    #[test]
    fn tools_by_name_key_overflows_to_partial() {
        let mut accumulator = accumulator(true);
        let mut event = assistant_event(1);
        event.model = Some("model".to_owned());
        event
            .tools
            .push(crate::analysis::ToolCall::new(long_string()));
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));
        let evidence = accumulator.evidence(&TurnFacts::default());
        assert_truncated_string(&evidence, "tools.by_name");
        let tools = assert_cap_partial(evidence.tools);
        assert_eq!(
            tools.by_name.keys().next().unwrap().len(),
            EVIDENCE_STRING_CAP
        );
    }

    fn source_string_overflow(kind: ContextSourceKind, long_description: bool) -> SessionEvidence {
        let mut accumulator = accumulator(true);
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::ContextSource {
                kind,
                name: if long_description {
                    "source".to_owned()
                } else {
                    long_string()
                },
                description: Some(if long_description {
                    long_string()
                } else {
                    "Synthetic description.".to_owned()
                }),
            },
        )));
        accumulator.evidence(&TurnFacts::default())
    }

    #[test]
    fn context_sources_skill_name_overflows_to_partial() {
        let evidence = source_string_overflow(ContextSourceKind::Skill, false);
        assert_truncated_string(&evidence, "context_sources.skills");
        let sources = assert_cap_partial(evidence.context_sources);
        assert_eq!(
            sources.skills.keys().next().unwrap().len(),
            EVIDENCE_STRING_CAP
        );
    }

    #[test]
    fn context_sources_skill_description_overflows_to_partial() {
        let evidence = source_string_overflow(ContextSourceKind::Skill, true);
        assert_truncated_string(&evidence, "context_sources.skills.description");
        let sources = assert_cap_partial(evidence.context_sources);
        assert_eq!(
            sources
                .skills
                .values()
                .next()
                .unwrap()
                .description
                .as_ref()
                .unwrap()
                .len(),
            EVIDENCE_STRING_CAP
        );
    }

    #[test]
    fn context_sources_mcp_server_name_overflows_to_partial() {
        let evidence = source_string_overflow(ContextSourceKind::McpServer, false);
        assert_truncated_string(&evidence, "context_sources.mcp_servers");
        let sources = assert_cap_partial(evidence.context_sources);
        assert_eq!(
            sources.mcp_servers.keys().next().unwrap().len(),
            EVIDENCE_STRING_CAP
        );
    }

    #[test]
    fn context_sources_mcp_server_description_is_dropped_not_capped() {
        let evidence = source_string_overflow(ContextSourceKind::McpServer, true);
        assert!(
            !evidence
                .diagnostics
                .truncated_strings
                .contains("context_sources.mcp_servers.description"),
            "a dropped description must not record a truncation diagnostic"
        );
        let EvidenceValue::Complete(sources) = evidence.context_sources else {
            panic!("dropping the MCP description must not degrade coverage");
        };
        let source = sources
            .mcp_servers
            .get("source")
            .expect("name must persist");
        assert_eq!(source.description, None);
    }

    fn subagent_string_overflow() -> SessionEvidence {
        let mut accumulator = accumulator(true);
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::SubagentSpawn {
                ts_ms: Some(1),
                parent_model: Some(long_string()),
                provenance: crate::analysis::RelationProvenance::TaskToolUse,
            },
        )));
        accumulator.evidence(&TurnFacts::default())
    }

    #[test]
    fn subagents_child_parent_model_overflows_to_partial() {
        let evidence = subagent_string_overflow();
        assert_truncated_string(&evidence, "subagents.children.parent_model");
        let subagents = assert_cap_partial(evidence.subagents);
        assert_eq!(
            subagents.children[0].parent_model.as_ref().unwrap().len(),
            EVIDENCE_STRING_CAP
        );
    }

    #[test]
    fn subagents_example_parent_model_overflows_to_partial() {
        let evidence = subagent_string_overflow();
        assert_truncated_string(&evidence, "subagents.examples.parent_model");
        let subagents = assert_cap_partial(evidence.subagents);
        assert_eq!(
            subagents.examples[0].parent_model.as_ref().unwrap().len(),
            EVIDENCE_STRING_CAP
        );
    }

    #[test]
    fn diagnostics_unrecognized_type_string_overflows_to_partial() {
        let mut accumulator = accumulator(true);
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::UnrecognizedType {
                discriminator: long_string(),
                inert: true,
            },
        )));
        let evidence = accumulator.evidence(&TurnFacts::default());
        assert_eq!(
            evidence
                .diagnostics
                .unrecognized_types
                .iter()
                .next()
                .unwrap()
                .len(),
            EVIDENCE_STRING_CAP
        );
        assert_eq!(
            evidence.coverage,
            EvidenceCoverage::Partial(CoverageReason::CapExceeded)
        );
        assert_truncated_string(&evidence, "diagnostics.unrecognized_types");
    }

    fn thread_record(
        uuid: Option<&str>,
        parent_uuid: Option<&str>,
        index: usize,
    ) -> Vec<NormalizedRecord> {
        let mut records = Vec::new();
        if uuid.is_some() || parent_uuid.is_some() {
            records.push(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::ThreadLink {
                    uuid: uuid.map(str::to_owned),
                    parent_uuid: parent_uuid.map(str::to_owned),
                },
            )));
        }
        let mut event = assistant_event(index);
        event.model = Some("model".to_owned());
        event.uuid = uuid.map(str::to_owned);
        event.parent_uuid = parent_uuid.map(str::to_owned);
        records.push(NormalizedRecord::MetricsEvent(Box::new(event)));
        records
    }

    /// `thread_identity_missing` is row-derived now (`TurnFacts`), not
    /// something the accumulator sees turn by turn. This mirrors what the
    /// row query would report for the same chain: missing whenever any
    /// counted turn in it carries no `uuid`.
    fn previous_turn_for(chain: &[(Option<&str>, Option<&str>)]) -> EvidenceValue<()> {
        let mut accumulator = accumulator(true);
        let mut thread_identity_missing = false;
        for (index, (uuid, parent_uuid)) in chain.iter().enumerate() {
            if uuid.is_none() {
                thread_identity_missing = true;
            }
            for record in thread_record(*uuid, *parent_uuid, index) {
                accumulator.record(record);
            }
        }
        let facts = TurnFacts {
            thread_identity_missing,
            ..TurnFacts::default()
        };
        match accumulator.evidence(&facts).cache {
            EvidenceValue::Complete(cache)
            | EvidenceValue::Partial {
                observed: cache, ..
            } => cache.previous_turn,
            EvidenceValue::Unsupported => panic!("Claude cache evidence must be supported"),
        }
    }

    #[test]
    fn a_resolved_uuid_chain_completes_previous_turn() {
        assert_eq!(
            previous_turn_for(&[
                (Some("u-1"), None),
                (Some("u-2"), Some("u-1")),
                (Some("u-3"), Some("u-2")),
            ]),
            EvidenceValue::Complete(())
        );
    }

    #[test]
    fn an_unresolved_parent_link_degrades_previous_turn() {
        // A parent pointing outside this source (a resumed session) is
        // unresolved, not fabricated: the linkage claim degrades.
        assert_eq!(
            previous_turn_for(&[(Some("u-1"), Some("u-absent")), (Some("u-2"), Some("u-1")),]),
            EvidenceValue::Partial {
                observed: (),
                reason: CoverageReason::AttributionIncomplete,
            }
        );
    }

    #[test]
    fn a_compaction_boundarys_logical_parent_resolves_the_link() {
        // `records::evidence_observations` reports a compaction boundary's
        // `ThreadLink.parent_uuid` as `parentUuid` falling back to
        // `logicalParentUuid`, so this accumulator never sees the boundary's
        // real (null) `parentUuid` directly — only the resolved fallback.
        let mut accumulator = accumulator(true);
        for record in thread_record(Some("u-1"), None, 0) {
            accumulator.record(record);
        }
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::ThreadLink {
                uuid: Some("boundary".to_owned()),
                parent_uuid: Some("u-1".to_owned()),
            },
        )));
        for record in thread_record(Some("u-2"), Some("boundary"), 1) {
            accumulator.record(record);
        }
        let facts = TurnFacts::default();
        let EvidenceValue::Complete(cache) = accumulator.evidence(&facts).cache else {
            panic!("a resolved logical parent must keep the cache group complete");
        };
        assert_eq!(cache.previous_turn, EvidenceValue::Complete(()));
    }

    #[test]
    fn a_counted_turn_without_a_uuid_degrades_previous_turn() {
        assert_eq!(
            previous_turn_for(&[(Some("u-1"), None), (None, None)]),
            EvidenceValue::Partial {
                observed: (),
                reason: CoverageReason::AttributionIncomplete,
            }
        );
    }

    /// A source with thread identity but no record identity (Codex's
    /// shape) never claimed per-record linkage, so a counted turn without
    /// a `uuid` is not a gap: `previous_turn` stays unsupported and the
    /// cache group stays complete.
    #[test]
    fn a_source_without_record_identity_keeps_previous_turn_unsupported() {
        let mut capabilities = SourceCapabilities::claude();
        capabilities.thread_identity = true;
        capabilities.record_identity = false;
        let mut accumulator = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: "s1".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities,
        });
        for record in thread_record(None, None, 1) {
            accumulator.record(record);
        }
        let facts = TurnFacts {
            thread_identity_missing: true,
            ..TurnFacts::default()
        };
        let EvidenceValue::Complete(cache) = accumulator.evidence(&facts).cache else {
            panic!("cache must stay complete: this source never claimed record identity");
        };
        assert_eq!(cache.previous_turn, EvidenceValue::Unsupported);
    }

    #[test]
    fn an_unclassified_tool_is_not_called_built_in() {
        let mut accumulator = accumulator(true);
        let mut event = assistant_event(1);
        event.tools.push(crate::analysis::ToolCall::new("Mystery"));
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));
        let EvidenceValue::Complete(tools) = accumulator.evidence(&TurnFacts::default()).tools
        else {
            panic!("tools must be complete");
        };
        assert_eq!(tools.by_name["Mystery"].class, ToolClass::Unclassified);
        assert!(!serde_json::to_string(&tools).unwrap().contains("built_in"));
    }
}
