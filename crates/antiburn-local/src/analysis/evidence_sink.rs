use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::mem::size_of;

use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::analysis::evidence::{
    CacheEvidence, ChurnCounts, CompactionEvidence, ContextEvidence, ContextSourceEvidence,
    CoverageReason, EVIDENCE_STRING_CAP, EvidenceCoverage, EvidenceSource, EvidenceValue,
    LoadedSource, MAX_CONTEXT_SOURCES, MAX_EVIDENCE_EXAMPLES, MAX_SUBAGENT_CHILDREN,
    MAX_TOOL_NAMES, MAX_UNRECOGNIZED_TYPES, ModelControlObservation, ModelEvidence,
    OrderingObservation, ParseDiagnostics, RelationConfidence, RepeatedContext,
    RepeatedContextAccounting, SessionCoverageRecord, SessionEvidence, SessionEvidenceIdentity,
    SessionProvenance, SourceAcceptance, SourceCapabilities, SourceKind, SubagentChild,
    SubagentEvidence, SubagentExample, ToolClass, ToolDefinition, ToolEvidence, ToolUse,
    cap_string, insert_diagnostic_field, record_diagnostic_set_cap,
};
use crate::analysis::evidence_query::TurnFacts;
use crate::analysis::initial_context::{InitialContextTokenSource, SourceOrigin};
use crate::analysis::interface::{
    ContextSourceKind, EvidenceObservation, NormalizedRecord, RecordSink, SessionSummary,
    VisitOutcome,
};
use crate::analysis::metrics_sink::SessionMetricsAccumulator;
use crate::analysis::model::NormalizedEvent;
use crate::analysis::resume::{AdapterResume, EvidenceSnapshot, StreamSnapshot};
use crate::analysis::rows::TurnRowSink;
use crate::analysis::tool_catalog::ToolCatalog;
use crate::analysis::{
    ANALYZER_REVISION, COVERAGE_SCHEMA_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION,
    RESUME_SNAPSHOT_REVISION, SessionMetrics,
};
use crate::model_catalog::{ModelCatalog, ReviewedModelCatalog, Support, model_control_target};

/// The most frequently observed full model id in `facts.by_model`, by
/// turn count, or `None` when the transcript never named one. A bare
/// alias (`sonnet`, `opus`, `haiku`) carries no hyphen and never resolves
/// against the built-in tool catalogue, so it is excluded — mirroring
/// `initial_context::ClaudeContextAccumulator::observe_model_id`.
fn resolved_model_id(facts: &TurnFacts) -> Option<&str> {
    facts
        .by_model
        .iter()
        .filter(|(model, _)| model.contains('-'))
        .max_by_key(|(_, tokens)| tokens.turns)
        .map(|(model, _)| model.as_str())
}

/// Tests enforce this ceiling for the accumulator's retained heap bytes.
/// The bound includes 16,384 thread identities with up to 256 bytes each.
pub const RETAINED_EVIDENCE_BYTES_BOUND: usize = 8 * 1_024 * 1_024;

/// A `BTreeMap` or `BTreeSet` has no queryable capacity: each insert grows
/// exactly one B-tree node. This estimates one entry's node overhead —
/// pointers and per-node slack — on top of its own key or value bytes.
const BTREE_ENTRY_OVERHEAD_BYTES: usize = 48;
const MAX_MODEL_CONTROL_OBSERVATIONS: usize = 128;
const MAX_TRACKED_THREAD_UUIDS: usize = 16_384;
const THREAD_UUIDS_DIAGNOSTIC: &str = "thread_link.seen_uuids";

/// The two fields [`SessionEvidenceAccumulator::coverage_record`] leaves
/// out because a closed pass's record already carries their final effect.
/// A resumed, still-open accumulator needs them back:
/// `last_ts_ms` to keep detecting out-of-order records across the resume
/// boundary, `seen_thread_uuids` so a later record's parent link resolves
/// against an identity an earlier, already-processed record declared.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvidenceResumeState {
    pub last_ts_ms: Option<i64>,
    #[serde(deserialize_with = "deserialize_thread_uuids")]
    pub seen_thread_uuids: HashSet<String>,
}

fn deserialize_thread_uuids<'de, D>(deserializer: D) -> Result<HashSet<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct ThreadUuidsVisitor;

    impl<'de> Visitor<'de> for ThreadUuidsVisitor {
        type Value = HashSet<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a bounded set of thread UUIDs")
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut uuids = HashSet::with_capacity(MAX_TRACKED_THREAD_UUIDS);
            while let Some(uuid) = sequence.next_element::<String>()? {
                if uuid.len() > EVIDENCE_STRING_CAP
                    || (uuids.len() == MAX_TRACKED_THREAD_UUIDS && !uuids.contains(&uuid))
                {
                    return Err(serde::de::Error::custom(
                        "thread UUID resume state exceeds its bound",
                    ));
                }
                uuids.insert(uuid);
            }
            Ok(uuids)
        }
    }

    deserializer.deserialize_seq(ThreadUuidsVisitor)
}

/// [`Clone`] lets a caller keep one input's residual as its own
/// (for a future resume snapshot) while folding a copy into the
/// parent's coverage record. See `stream_vendor_with_hooks`.
#[derive(Clone)]
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
    model_control_observations: Vec<ModelControlObservation>,
    subagent_spawn_count: u64,
    subagent_children: Vec<SubagentChild>,
    subagent_examples: Vec<SubagentExample>,
    subagents_cap_exceeded: bool,
    subagent_linkage_incomplete: bool,
    seen_thread_uuids: HashSet<String>,
    thread_parent_unresolved: bool,
    /// The harness's own version, from a `HarnessVersion` observation.
    /// First-seen value wins.
    harness_version: Option<String>,
    /// Tool names named in a `DeferredTool` observation this session.
    deferred_tools: BTreeSet<String>,
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
            model_control_observations: Vec::new(),
            subagent_spawn_count: 0,
            subagent_children: Vec::new(),
            subagent_examples: Vec::new(),
            subagents_cap_exceeded: false,
            subagent_linkage_incomplete: false,
            seen_thread_uuids: HashSet::new(),
            thread_parent_unresolved: false,
            harness_version: None,
            deferred_tools: BTreeSet::new(),
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
            let tracked = if let Some(entry) = self.tools.get_mut(&name) {
                entry.calls = entry.calls.saturating_add(1);
                true
            } else if self.tools.len() == MAX_TOOL_NAMES {
                self.tools_cap_exceeded = true;
                self.note_collection_cap("tools.by_name");
                false
            } else {
                self.tools.insert(
                    name.clone(),
                    ToolUse {
                        calls: 1,
                        class: ToolClass::Unclassified,
                    },
                );
                true
            };
            if tracked && tool.name.eq_ignore_ascii_case("skill") {
                self.invoked_skills.insert(name);
            }
        }
        self.observe_model_control(event);
    }

    fn observe_model_control(&mut self, event: &NormalizedEvent) {
        if !matches!(event.role, crate::analysis::Role::Assistant) {
            return;
        }
        let Some(model) = event.model.as_deref() else {
            return;
        };
        let reviewed_route = if event.thinking_mode.is_none() && event.speed.is_none() {
            let target = model_control_target(
                &self.identity.agent,
                event.provider.as_deref(),
                event.api.as_deref(),
                model,
            );
            matches!(
                ReviewedModelCatalog::default().resolve(&target),
                Support::Supported(_)
            )
            .then_some((target.provider, target.api))
        } else {
            None
        };
        if event.thinking_mode.is_none() && event.speed.is_none() && reviewed_route.is_none() {
            return;
        }
        let truncated_before = self.diagnostics.truncated_strings.len();
        let model = cap_string(
            "models.control_observations.model",
            model,
            &mut self.diagnostics,
        );
        let provider = reviewed_route
            .as_ref()
            .map(|(provider, _)| provider.as_str())
            .or(event.provider.as_deref())
            .map(|value| {
                cap_string(
                    "models.control_observations.provider",
                    value,
                    &mut self.diagnostics,
                )
            });
        let api = reviewed_route
            .as_ref()
            .map(|(_, api)| api.as_str())
            .or(event.api.as_deref())
            .map(|value| {
                cap_string(
                    "models.control_observations.api",
                    value,
                    &mut self.diagnostics,
                )
            });
        let effort = event.thinking_mode.as_deref().map(|value| {
            cap_string(
                "models.control_observations.effort",
                value,
                &mut self.diagnostics,
            )
        });
        let speed = event.speed.as_deref().map(|value| {
            cap_string(
                "models.control_observations.speed",
                value,
                &mut self.diagnostics,
            )
        });
        if self.diagnostics.truncated_strings.len() > truncated_before {
            self.session_cap_exceeded = true;
        }
        if let Some(observation) = self
            .model_control_observations
            .iter_mut()
            .find(|observation| {
                observation.provider == provider
                    && observation.api == api
                    && observation.model == model
                    && observation.effort == effort
                    && observation.speed == speed
            })
        {
            let count = match event.source {
                crate::analysis::EventSource::Parent => &mut observation.turns.main_loop,
                crate::analysis::EventSource::Subagent => &mut observation.turns.delegated,
            };
            *count = count.saturating_add(1);
            observation.last_ts_ms = observation.last_ts_ms.max(event.ts_ms.unwrap_or_default());
            return;
        }
        if self.model_control_observations.len() == MAX_MODEL_CONTROL_OBSERVATIONS {
            self.session_cap_exceeded = true;
            self.note_collection_cap("models.control_observations");
            return;
        }
        let mut turns = crate::analysis::TurnCounts::default();
        match event.source {
            crate::analysis::EventSource::Parent => turns.main_loop = 1,
            crate::analysis::EventSource::Subagent => turns.delegated = 1,
        }
        self.model_control_observations
            .push(ModelControlObservation {
                provider,
                api,
                model,
                effort,
                speed,
                last_ts_ms: event.ts_ms.unwrap_or_default(),
                turns,
            });
    }

    fn observe_observation(&mut self, observation: &EvidenceObservation) {
        match observation {
            EvidenceObservation::ContextSource {
                kind,
                name,
                description,
            } => self.observe_context_source(
                *kind,
                name,
                description.as_deref(),
                *kind == ContextSourceKind::McpServer,
                false,
            ),
            EvidenceObservation::SkillInjection { name, invoked } => {
                self.observe_context_source(ContextSourceKind::Skill, name, None, true, *invoked);
            }
            EvidenceObservation::SubagentSpawn {
                ts_ms,
                parent_model,
                parent_call_id,
                child_model,
                provenance,
            } => self.observe_subagent_spawn(
                *ts_ms,
                parent_model.as_deref(),
                parent_call_id.as_deref(),
                child_model.as_deref(),
                *provenance,
            ),
            EvidenceObservation::SubagentModel {
                parent_call_id,
                model,
            } => {
                self.observe_child_models(Some(parent_call_id), std::iter::once(model.as_str()));
            }
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
                    self.observe_thread_uuid(uuid);
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
            EvidenceObservation::ReplayedRecord => {
                self.diagnostics.records_replayed =
                    self.diagnostics.records_replayed.saturating_add(1);
            }
            EvidenceObservation::HarnessVersion { version } => {
                if self.harness_version.is_none() {
                    self.harness_version = Some(version.clone());
                }
            }
            EvidenceObservation::DeferredTool { name } => {
                self.deferred_tools.insert(name.clone());
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

    fn observe_thread_uuid(&mut self, uuid: &str) {
        if uuid.len() > EVIDENCE_STRING_CAP
            || (self.seen_thread_uuids.len() == MAX_TRACKED_THREAD_UUIDS
                && !self.seen_thread_uuids.contains(uuid))
        {
            self.session_cap_exceeded = true;
            self.thread_parent_unresolved = true;
            self.note_collection_cap(THREAD_UUIDS_DIAGNOSTIC);
            return;
        }
        self.seen_thread_uuids.insert(uuid.to_owned());
    }

    fn observe_subagent_spawn(
        &mut self,
        ts_ms: Option<i64>,
        parent_model: Option<&str>,
        parent_call_id: Option<&str>,
        child_model: Option<&str>,
        provenance: crate::analysis::interface::RelationProvenance,
    ) {
        self.subagent_spawn_count = self.subagent_spawn_count.saturating_add(1);
        let child_parent_model = parent_model.and_then(|model| {
            let capped = cap_string(
                "subagents.children.parent_model",
                model,
                &mut self.diagnostics,
            );
            if capped.len() != model.len() {
                self.subagents_cap_exceeded = true;
            }
            (capped.len() == model.len()).then_some(capped)
        });
        let parent_call_id = parent_call_id
            .filter(|id| !id.trim().is_empty())
            .and_then(|id| {
                let capped = cap_string(
                    "subagents.children.parent_call_id",
                    id,
                    &mut self.diagnostics,
                );
                if capped.len() != id.len() {
                    self.subagents_cap_exceeded = true;
                    return None;
                }
                Some(capped)
            });
        if self.subagent_children.len() == MAX_SUBAGENT_CHILDREN {
            self.subagents_cap_exceeded = true;
            self.note_collection_cap("subagents.children");
        } else {
            let index = self.subagent_children.len();
            self.subagent_children.push(SubagentChild {
                ordinal: u32::try_from(self.subagent_spawn_count).unwrap_or(u32::MAX),
                parent_model: child_parent_model,
                parent_call_id,
                observed_child_models: BTreeSet::new(),
                child_model: EvidenceValue::Unsupported,
                confidence: RelationConfidence::Observed,
                provenance,
            });
            if let Some(model) = child_model {
                self.insert_child_model(index, model);
            }
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

    /// Adds models only when exactly one native parent call matches the child metadata.
    pub fn observe_child_models<'a>(
        &mut self,
        parent_call_id: Option<&str>,
        models: impl Iterator<Item = &'a str>,
    ) {
        let Some(id) = parent_call_id.filter(|id| !id.trim().is_empty()) else {
            self.subagent_linkage_incomplete = true;
            return;
        };
        let mut matches = self
            .subagent_children
            .iter()
            .enumerate()
            .filter(|(_, child)| child.parent_call_id.as_deref() == Some(id));
        let Some((index, _)) = matches.next() else {
            self.subagent_linkage_incomplete = true;
            return;
        };
        if matches.next().is_some() {
            self.subagent_linkage_incomplete = true;
            return;
        }
        let mut observed = false;
        for model in models {
            observed = true;
            self.insert_child_model(index, model);
        }
        self.subagent_linkage_incomplete |= !observed;
    }

    fn insert_child_model(&mut self, index: usize, model: &str) {
        let capped = cap_string(
            "subagents.children.observed_child_models",
            model,
            &mut self.diagnostics,
        );
        if capped.len() != model.len() {
            self.subagents_cap_exceeded = true;
            return;
        }
        if model.trim().is_empty() || model == "<synthetic>" {
            self.subagent_linkage_incomplete = true;
            return;
        }
        if self.subagent_children[index]
            .observed_child_models
            .contains(model)
        {
            return;
        }
        // The cap applies across all children, not separately to each child.
        if self
            .subagent_children
            .iter()
            .map(|child| child.observed_child_models.len())
            .sum::<usize>()
            >= crate::analysis::evidence::MAX_SUBAGENT_MODELS
        {
            self.subagents_cap_exceeded = true;
            self.note_collection_cap("subagents.children.observed_child_models");
            return;
        }
        let child = &mut self.subagent_children[index];
        self.subagent_linkage_incomplete |= child
            .parent_model
            .as_deref()
            .is_none_or(|model| model.trim().is_empty());
        child.observed_child_models.insert(capped);
        child.child_model = EvidenceValue::Complete(());
    }

    fn observe_context_source(
        &mut self,
        kind: ContextSourceKind,
        name: &str,
        description: Option<&str>,
        injected: bool,
        invoked: bool,
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
                // A description is display-only. Truncation does not lose
                // evidence about which context source loaded or ran, so it
                // does not degrade the context_sources group.
                cap_string(description_field, value, &mut self.diagnostics)
            })
        });
        if let Some(existing) = map.get_mut(&capped_name) {
            existing.injected |= injected;
            existing.invoked |= invoked;
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
                    configured: false,
                    available: true,
                    injected,
                    invoked,
                    token_count: None,
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

    /// Folds a child's record loss, diagnostics, ordering, and model controls into the parent.
    /// Counts child model controls as delegated work. Keeps tools and context sources parent-only.
    pub fn observe_child_coverage(&mut self, child: &SessionEvidenceAccumulator) {
        self.diagnostics.children_discovered =
            self.diagnostics.children_discovered.saturating_add(1);
        if let Some(reason) = child.record_loss_reason {
            self.set_child_loss_reason(reason);
        }
        self.fold_child_diagnostics(&child.diagnostics);
        for observation in &child.model_control_observations {
            let delegated = observation
                .turns
                .main_loop
                .saturating_add(observation.turns.delegated);
            if let Some(existing) = self.model_control_observations.iter_mut().find(|existing| {
                existing.provider == observation.provider
                    && existing.api == observation.api
                    && existing.model == observation.model
                    && existing.effort == observation.effort
                    && existing.speed == observation.speed
            }) {
                existing.turns.delegated = existing.turns.delegated.saturating_add(delegated);
                existing.last_ts_ms = existing.last_ts_ms.max(observation.last_ts_ms);
            } else if self.model_control_observations.len() == MAX_MODEL_CONTROL_OBSERVATIONS {
                self.session_cap_exceeded = true;
                self.note_collection_cap("models.control_observations");
            } else {
                let mut observation = observation.clone();
                observation.turns.main_loop = 0;
                observation.turns.delegated = delegated;
                self.model_control_observations.push(observation);
            }
        }
        if child
            .diagnostics
            .capped_collections
            .contains("models.control_observations")
        {
            self.note_collection_cap("models.control_observations");
        }
        for field in &child.diagnostics.truncated_strings {
            if field.starts_with("models.control_observations.")
                && insert_diagnostic_field(&mut self.diagnostics.truncated_strings, field)
            {
                self.session_cap_exceeded = true;
                record_diagnostic_set_cap(&mut self.diagnostics, "diagnostics.truncated_strings");
            }
        }
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
        self.diagnostics.records_replayed = self
            .diagnostics
            .records_replayed
            .saturating_add(child.records_replayed);
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
        self.observe_initial_context(summary);
        for reason in &summary.coverage_gaps {
            self.set_record_loss_reason(CoverageReason::from(*reason));
        }
        self.summary_observed = true;
    }

    fn observe_initial_context(&mut self, summary: &SessionSummary) {
        let Some(context) = &summary.initial_context else {
            return;
        };
        for row in &context.sources {
            let Some(name) = row.source_name.as_deref() else {
                continue;
            };
            let kind = if row.source == InitialContextTokenSource::Skill.as_str() {
                ContextSourceKind::Skill
            } else if row.source == InitialContextTokenSource::Mcp.as_str() {
                ContextSourceKind::McpServer
            } else {
                continue;
            };
            self.observe_context_source(kind, name, None, false, false);
            let map = match kind {
                ContextSourceKind::Skill => &mut self.skills,
                ContextSourceKind::McpServer => &mut self.mcp_servers,
            };
            if let Some(source) = map.get_mut(name) {
                if kind == ContextSourceKind::McpServer {
                    source.injected |= row.token_count > 0;
                }
                source.invoked |= row.use_count > 0;
                source.token_count = Some(row.token_count);
                source.origin = match row.origin {
                    SourceOrigin::Unknown => EvidenceValue::Unsupported,
                    origin => EvidenceValue::Complete(origin),
                };
            }
        }
    }

    /// Attaches the source outcome after the adapter returns.
    pub fn observe_source_outcome(&mut self, outcome: VisitOutcome) {
        if matches!(outcome, VisitOutcome::AcceptedPrefix { .. }) {
            self.set_record_loss_reason(CoverageReason::PinnedPrefix);
        }
        self.source_acceptance = SourceAcceptance::from(outcome);
    }

    /// Snapshots every field [`Self::evidence`] reads from `self` (every
    /// field but the two transient ones, `last_ts_ms` and
    /// `seen_thread_uuids`, that exist only to compute `ordering` and
    /// `thread_parent_unresolved` live) into a [`SessionCoverageRecord`].
    /// [`crate::analysis::evidence_replay::evidence_from_facts`] combines the
    /// record this returns with row-derived [`TurnFacts`] to rebuild
    /// [`SessionEvidence`] without a live fold.
    pub fn coverage_record(&self) -> SessionCoverageRecord {
        SessionCoverageRecord {
            coverage_schema_revision: COVERAGE_SCHEMA_REVISION,
            identity: self.identity.clone(),
            capabilities: self.capabilities,
            source_kind: self.source_kind,
            source_acceptance: self.source_acceptance,
            ordering: self.ordering,
            diagnostics: self.diagnostics.clone(),
            record_loss_reason: self.record_loss_reason,
            session_cap_exceeded: self.session_cap_exceeded,
            tools: self.tools.clone(),
            invoked_skills: self.invoked_skills.clone(),
            tools_cap_exceeded: self.tools_cap_exceeded,
            skills: self.skills.clone(),
            mcp_servers: self.mcp_servers.clone(),
            context_sources_cap_exceeded: self.context_sources_cap_exceeded,
            model_control_observations: self.model_control_observations.clone(),
            subagent_spawn_count: self.subagent_spawn_count,
            subagent_children: self.subagent_children.clone(),
            subagent_examples: self.subagent_examples.clone(),
            subagents_cap_exceeded: self.subagents_cap_exceeded,
            subagent_linkage_incomplete: self.subagent_linkage_incomplete,
            thread_parent_unresolved: self.thread_parent_unresolved,
            harness_version: self.harness_version.clone(),
            deferred_tools: self.deferred_tools.clone(),
            summary_observed: self.summary_observed,
            child_loss_reason: self.child_loss_reason,
        }
    }

    /// Rebuilds an accumulator from a [`SessionCoverageRecord`] a prior pass
    /// produced. The two transient fields [`Self::coverage_record`] does not
    /// carry (`last_ts_ms`, `seen_thread_uuids`) start empty: [`Self::evidence`]
    /// never reads them directly, only the `ordering` and
    /// `thread_parent_unresolved` results already folded into the record.
    ///
    /// This is the row-replay shape: [`crate::analysis::evidence_from_facts`]
    /// rebuilds [`SessionEvidence`] for a *closed* pass, where the record
    /// already carries the final effect of those two fields. A resumed
    /// accumulator that will keep observing more records instead needs
    /// [`Self::from_coverage_record_with_resume`].
    pub fn from_coverage_record(record: SessionCoverageRecord) -> Self {
        Self::from_coverage_record_with_resume(record, EvidenceResumeState::default())
    }

    /// Like [`Self::from_coverage_record`], but restores the two transient
    /// fields from `resume` instead of starting them empty. Use this to
    /// resume a still-open accumulator: an unresumed
    /// `seen_thread_uuids` would make a later record's parent link look
    /// unresolved even when an earlier, already-processed record declared
    /// it.
    pub fn from_coverage_record_with_resume(
        record: SessionCoverageRecord,
        mut resume: EvidenceResumeState,
    ) -> Self {
        let resume_overflowed = resume.seen_thread_uuids.len() > MAX_TRACKED_THREAD_UUIDS
            || resume
                .seen_thread_uuids
                .iter()
                .any(|uuid| uuid.len() > EVIDENCE_STRING_CAP);
        if resume_overflowed {
            resume
                .seen_thread_uuids
                .retain(|uuid| uuid.len() <= EVIDENCE_STRING_CAP);
            resume.seen_thread_uuids = resume
                .seen_thread_uuids
                .into_iter()
                .take(MAX_TRACKED_THREAD_UUIDS)
                .collect();
        }
        let mut diagnostics = record.diagnostics;
        if resume_overflowed
            && insert_diagnostic_field(&mut diagnostics.capped_collections, THREAD_UUIDS_DIAGNOSTIC)
        {
            record_diagnostic_set_cap(&mut diagnostics, "diagnostics.capped_collections");
        }
        Self {
            identity: record.identity,
            capabilities: record.capabilities,
            source_kind: record.source_kind,
            source_acceptance: record.source_acceptance,
            ordering: record.ordering,
            diagnostics,
            record_loss_reason: record.record_loss_reason,
            session_cap_exceeded: record.session_cap_exceeded || resume_overflowed,
            last_ts_ms: resume.last_ts_ms,
            tools: record.tools,
            invoked_skills: record.invoked_skills,
            tools_cap_exceeded: record.tools_cap_exceeded,
            skills: record.skills,
            mcp_servers: record.mcp_servers,
            context_sources_cap_exceeded: record.context_sources_cap_exceeded,
            model_control_observations: record.model_control_observations,
            subagent_spawn_count: record.subagent_spawn_count,
            subagent_children: record.subagent_children,
            subagent_examples: record.subagent_examples,
            subagents_cap_exceeded: record.subagents_cap_exceeded,
            subagent_linkage_incomplete: record.subagent_linkage_incomplete,
            seen_thread_uuids: resume.seen_thread_uuids,
            thread_parent_unresolved: record.thread_parent_unresolved || resume_overflowed,
            harness_version: record.harness_version,
            deferred_tools: record.deferred_tools,
            summary_observed: record.summary_observed,
            child_loss_reason: record.child_loss_reason,
        }
    }

    /// The two transient fields [`Self::coverage_record`] leaves out,
    /// needed to resume a still-open accumulator. See
    /// [`Self::from_coverage_record_with_resume`].
    pub fn resume_state(&self) -> EvidenceResumeState {
        EvidenceResumeState {
            last_ts_ms: self.last_ts_ms,
            seen_thread_uuids: self.seen_thread_uuids.clone(),
        }
    }

    /// Builds this session's [`SessionEvidence`] against the embedded
    /// production tool catalogue. See [`Self::evidence_with_catalog`].
    pub fn evidence(&self, facts: &TurnFacts) -> SessionEvidence {
        self.evidence_with_catalog(facts, crate::analysis::tool_catalog::embedded())
    }

    /// Builds this session's [`SessionEvidence`] from the row-derived
    /// `facts` plus whatever this residual observed directly. Most groups
    /// come from `facts` outright; `tools`, `context_sources`, and the
    /// subagent relationship shape (`spawn_count`, `children`, `examples`)
    /// still come from this residual — a row query has no tool catalog and
    /// no context-source contract, and a child's spawn is only ever seen
    /// live, as an `EvidenceObservation`. `catalog` resolves built-in tool
    /// definitions; a test can supply a fixture catalogue in place of the
    /// real embedded one (see [`Self::evidence`]).
    pub fn evidence_with_catalog(
        &self,
        facts: &TurnFacts,
        catalog: &ToolCatalog,
    ) -> SessionEvidence {
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
        let (context_sources, skill_attribution_incomplete) =
            self.context_sources(catalog, resolved_model_id(facts));
        let models = ModelEvidence {
            by_model: facts.by_model.clone(),
            unattributed_turns: facts.unattributed_turns,
            effort_tiers: facts.effort_tiers.clone(),
            fast_modes: facts.fast_modes.clone(),
            effort_tiers_by_model: facts.effort_tiers_by_model.clone(),
            fast_modes_by_model: facts.fast_modes_by_model.clone(),
            control_observations: self.model_control_observations.clone(),
            service_tiers: EvidenceValue::Unsupported,
            effort_signal: facts.effort_signal,
            speed_signal: facts.speed_signal,
            dominant_main_model: facts.dominant_main_model.clone(),
        };
        let models_cap_exceeded = facts.models_capped
            || facts.tiers_capped
            || diagnostics
                .capped_collections
                .contains("models.control_observations")
            || diagnostics
                .truncated_strings
                .iter()
                .any(|field| field.starts_with("models.control_observations."));
        let subagents = SubagentEvidence {
            spawn_count: self.subagent_spawn_count,
            delegated_turns: facts.delegated_turns,
            delegated_models: facts.delegated_models.clone(),
            children: self.subagent_children.clone(),
            examples: self.subagent_examples.clone(),
        };
        let subagents_cap_exceeded = self.subagents_cap_exceeded || facts.delegated_models_capped;
        // Verified previous-turn linkage, attested through either of two
        // routes. The id route (`record_identity`): complete only when
        // every counted turn carried its own identity and every parent link
        // resolved to an identity this source declared earlier.
        // `thread_identity_missing` feeds the record-identity claim here: a
        // row with no `uuid` is a record-identity gap, not a thread-identity
        // gap. The order route (`linear_record_order`): a source with no
        // per-record id but one thread per append-only stream needs none —
        // a counted turn's predecessor is always the counted record
        // immediately before it, so linkage is complete whenever this
        // source lost no record. The id-gap concept does not apply here: an
        // id-less source never emits `ThreadLink`, so `record_identity_gap`
        // is gated to the id route and never degrades this one.
        // `provider_eviction` stays unsupported — no transcript record
        // states an eviction.
        let record_identity_gap = facts.thread_identity_missing || self.thread_parent_unresolved;
        let previous_turn = if self.capabilities.record_identity {
            if let Some(reason) = self.record_loss_reason {
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
            }
        } else if self.capabilities.linear_record_order {
            match self.record_loss_reason {
                Some(reason) => EvidenceValue::Partial {
                    observed: (),
                    reason,
                },
                None => EvidenceValue::Complete(()),
            }
        } else {
            EvidenceValue::Unsupported
        };
        // The same precedence the `cache` group's own `EvidenceValue` below
        // resolves to: this source's own record loss outranks a lossy
        // child, which outranks the transitions cap, which outranks a
        // record-identity gap. `repeated_context` degrades on the same
        // reason, because it is computed from the same rows.
        let cache_partial_reason: Option<CoverageReason> = self
            .record_loss_reason
            .or(child_dependent_partial)
            .or(facts
                .transitions_capped
                .then_some(CoverageReason::CapExceeded))
            .or((self.capabilities.record_identity && record_identity_gap)
                .then_some(CoverageReason::AttributionIncomplete));
        let supports_repeated_context =
            self.capabilities.token_classes && self.capabilities.request_context_tokens;
        let cache_write_supported = (facts.repeated_context_cache_write_seen
            || matches!(self.identity.agent.as_str(), "claude" | "claude-code"))
            && supports_repeated_context
            && (self.capabilities.cache_write_tokens || self.identity.agent == "pi");
        let uncached_input_supported = (facts.repeated_context_uncached_input_seen
            || self.identity.agent == "codex")
            && supports_repeated_context;
        let unsupported_segment = (facts.repeated_context_cache_write_seen
            && !cache_write_supported)
            || (facts.repeated_context_uncached_input_seen && !uncached_input_supported);
        let repeated_context = if !cache_write_supported && !uncached_input_supported {
            EvidenceValue::Unsupported
        } else {
            let mut segments = Vec::new();
            if cache_write_supported {
                segments.push(crate::analysis::RepeatedContextSegment {
                    accounting: RepeatedContextAccounting::CacheWrite,
                    repeated_tokens: facts.repeated_context_cache_write_tokens,
                    paid_tokens: facts.repeated_context_cache_write_paid_tokens,
                    pairs_considered: facts.repeated_context_cache_write_pairs_considered,
                    pairs_skipped: facts.repeated_context_cache_write_pairs_skipped,
                });
            }
            if uncached_input_supported {
                segments.push(crate::analysis::RepeatedContextSegment {
                    accounting: RepeatedContextAccounting::UncachedInput,
                    repeated_tokens: facts.repeated_context_uncached_input_tokens,
                    paid_tokens: facts.repeated_context_uncached_input_paid_tokens,
                    pairs_considered: facts.repeated_context_uncached_input_pairs_considered,
                    pairs_skipped: facts.repeated_context_uncached_input_pairs_skipped,
                });
            }
            let primary = segments.first().expect("at least one supported segment");
            let observed = RepeatedContext {
                accounting: primary.accounting,
                repeated_tokens: primary.repeated_tokens,
                paid_tokens: primary.paid_tokens,
                pairs_considered: facts.repeated_context_pairs_considered,
                pairs_skipped: facts.repeated_context_pairs_skipped,
                segments,
            };
            if let Some(reason) = cache_partial_reason {
                EvidenceValue::Partial { observed, reason }
            } else if facts.repeated_context_incomplete || unsupported_segment {
                EvidenceValue::Partial {
                    observed,
                    reason: CoverageReason::AttributionIncomplete,
                }
            } else {
                EvidenceValue::Complete(observed)
            }
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
            repeated_context,
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
                // `self.harness_version` is folded only from a
                // `HarnessVersion` observation, which only the Claude
                // adapter emits (`records::evidence_observations`), so
                // every other source stays `Unsupported` here without a
                // separate capability gate. The version string itself is
                // not carried here — `harness_version` is a bare marker,
                // like the other `EvidenceValue<()>` fields.
                harness_version: if self.harness_version.is_some() {
                    EvidenceValue::Complete(())
                } else {
                    EvidenceValue::Unsupported
                },
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
            context_sources: if !(self.capabilities.skill_inventory
                || self.capabilities.mcp_inventory
                || self.capabilities.tool_definitions
                || !context_sources.skills.is_empty()
                || !context_sources.mcp_servers.is_empty())
            {
                EvidenceValue::Unsupported
            } else if self.context_sources_cap_exceeded {
                EvidenceValue::Partial {
                    observed: context_sources,
                    reason: CoverageReason::CapExceeded,
                }
            } else if skill_attribution_incomplete {
                EvidenceValue::Partial {
                    observed: context_sources,
                    reason: CoverageReason::AttributionIncomplete,
                }
            } else {
                EvidenceValue::Complete(context_sources)
            },
            models: if !self.capabilities.model_identity {
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
            subagents: if !self.capabilities.subagent_relationships && subagents.spawn_count == 0 {
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
            } else if !self.capabilities.subagent_relationships
                || self.subagent_linkage_incomplete
                || facts.delegated_model_missing
                || facts.duplicate_turn_identities > 0
            {
                EvidenceValue::Partial {
                    observed: subagents,
                    reason: CoverageReason::AttributionIncomplete,
                }
            } else {
                EvidenceValue::Complete(subagents)
            },
            // The source promised per-record identity but a counted turn
            // lacked it (or a parent link did not resolve): the cache
            // group's linkage claim is incomplete, so the group degrades
            // and Cache Churn cannot read clean from it (no false clean).
            // `cache_partial_reason` computes this same precedence above,
            // shared with `repeated_context`.
            cache: match cache_partial_reason {
                Some(reason) => EvidenceValue::Partial {
                    observed: cache,
                    reason,
                },
                None => EvidenceValue::Complete(cache),
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

    fn resolved_skill_invocations(&self) -> (BTreeSet<String>, bool) {
        let mut resolved = BTreeSet::new();
        let mut ambiguous = false;
        for invocation in &self.invoked_skills {
            if let Some(exact) = self
                .skills
                .keys()
                .find(|name| name.eq_ignore_ascii_case(invocation))
            {
                resolved.insert(exact.clone());
                continue;
            }
            if invocation.contains(':') {
                continue;
            }
            let matches = self
                .skills
                .keys()
                .filter(|name| {
                    name.rsplit(':')
                        .next()
                        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(invocation))
                })
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [name] => {
                    resolved.insert((*name).clone());
                }
                [_, _, ..] => ambiguous = true,
                _ => {}
            }
        }
        (resolved, ambiguous)
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
                    .any(|server| tool_belongs_to_mcp_server(name, server))
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

    fn context_sources(
        &self,
        catalog: &ToolCatalog,
        model: Option<&str>,
    ) -> (ContextSourceEvidence, bool) {
        let mut skills = self.skills.clone();
        let mut mcp_servers = self.mcp_servers.clone();
        let (invoked_skills, mut skill_attribution_incomplete) = self.resolved_skill_invocations();
        for (name, source) in &mut skills {
            source.invoked |= invoked_skills.contains(name);
        }
        // A directory basename cannot identify which namespaced skill document loaded.
        skill_attribution_incomplete |= skills.iter().any(|(name, source)| {
            source.injected
                && !source.invoked
                && !name.contains(':')
                && skills.keys().any(|candidate| {
                    candidate
                        .rsplit_once(':')
                        .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case(name))
                })
        });
        for (name, source) in &mut mcp_servers {
            source.invoked |= self
                .tools
                .keys()
                .any(|tool| tool_belongs_to_mcp_server(tool, name));
        }
        (
            ContextSourceEvidence {
                skill_coverage: self.resource_coverage(
                    !skills.is_empty(),
                    "context_sources.skills",
                    skill_attribution_incomplete,
                ),
                mcp_coverage: self.resource_coverage(
                    !mcp_servers.is_empty(),
                    "context_sources.mcp_servers",
                    false,
                ),
                skills,
                mcp_servers,
                tool_definitions: self.tool_definitions(catalog, model),
            },
            skill_attribution_incomplete,
        )
    }

    fn resource_coverage(
        &self,
        observed: bool,
        field: &str,
        attribution_incomplete: bool,
    ) -> EvidenceValue<()> {
        // Coverage applies only to observed resources, not the full historical inventory.
        self.supported_value(
            (),
            observed,
            attribution_incomplete.then_some(CoverageReason::AttributionIncomplete),
            self.diagnostics.capped_collections.contains(field)
                || self.diagnostics.truncated_strings.contains(field),
        )
    }

    /// Resolves this session's built-in tool definitions against
    /// `catalog`, for `self.identity.agent` at `self.harness_version` and
    /// `model`. `Unsupported` — never a partial map — when the source
    /// declares no `tool_definitions` capability, the version was never
    /// observed, `model` is `None`, or the catalogue cannot resolve the
    /// version or model for this agent.
    fn tool_definitions(
        &self,
        catalog: &ToolCatalog,
        model: Option<&str>,
    ) -> EvidenceValue<BTreeMap<String, ToolDefinition>> {
        if !self.capabilities.tool_definitions {
            return EvidenceValue::Unsupported;
        }
        let (Some(version), Some(model)) = (self.harness_version.as_deref(), model) else {
            return EvidenceValue::Unsupported;
        };
        let Some(tools) = catalog.lookup_exact(&self.identity.agent, version, model) else {
            return EvidenceValue::Unsupported;
        };
        let mut definitions = BTreeMap::new();
        for tool in tools {
            let invoked = tool.match_names().iter().any(|match_name| {
                self.tools
                    .keys()
                    .any(|name| name.eq_ignore_ascii_case(match_name))
            });
            let deferred = tool.is_deferred(&self.deferred_tools);
            definitions.insert(
                tool.display_name(),
                ToolDefinition {
                    tokens: tool.tokens,
                    invoked,
                    deferred,
                },
            );
        }
        self.supported_value(definitions, true, None, self.tools_cap_exceeded)
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

    /// Estimates this accumulator's retained heap bytes: every capped
    /// collection's held strings and entries, plus each interned string's
    /// own capacity. Mirrors [`crate::analysis::metrics_sink::
    /// SessionMetricsAccumulator::retained_bytes`]'s approach, adapted for
    /// `BTreeMap`/`BTreeSet` fields, which carry no queryable capacity.
    pub fn retained_bytes(&self) -> usize {
        self.identity
            .agent
            .capacity()
            .saturating_add(self.identity.session_id.capacity())
            .saturating_add(diagnostics_retained_bytes(&self.diagnostics))
            .saturating_add(tools_retained_bytes(&self.tools))
            .saturating_add(string_set_retained_bytes(&self.invoked_skills))
            .saturating_add(context_sources_retained_bytes(&self.skills))
            .saturating_add(context_sources_retained_bytes(&self.mcp_servers))
            .saturating_add(model_controls_retained_bytes(
                &self.model_control_observations,
                self.model_control_observations.capacity(),
            ))
            .saturating_add(subagent_children_retained_bytes(
                &self.subagent_children,
                self.subagent_children.capacity(),
            ))
            .saturating_add(subagent_examples_retained_bytes(
                &self.subagent_examples,
                self.subagent_examples.capacity(),
            ))
            .saturating_add(hash_string_set_retained_bytes(&self.seen_thread_uuids))
    }
}

fn model_controls_retained_bytes(
    observations: &[ModelControlObservation],
    capacity: usize,
) -> usize {
    capacity
        .saturating_mul(size_of::<ModelControlObservation>())
        .saturating_add(observations.iter().fold(0usize, |bytes, observation| {
            bytes
                .saturating_add(observation.model.capacity())
                .saturating_add(observation.provider.as_ref().map_or(0, String::capacity))
                .saturating_add(observation.api.as_ref().map_or(0, String::capacity))
                .saturating_add(observation.effort.as_ref().map_or(0, String::capacity))
                .saturating_add(observation.speed.as_ref().map_or(0, String::capacity))
        }))
}

fn tool_belongs_to_mcp_server(tool: &str, server: &str) -> bool {
    tool == server
        || tool
            .strip_prefix("mcp__")
            .and_then(|name| name.split_once("__"))
            .is_some_and(|(candidate, _)| candidate == server)
}

/// One `BTreeSet<String>` entry's retained bytes: its string capacity plus
/// the tree's own per-entry overhead.
fn string_set_retained_bytes(set: &BTreeSet<String>) -> usize {
    set.iter()
        .map(|value| value.capacity().saturating_add(BTREE_ENTRY_OVERHEAD_BYTES))
        .sum()
}

fn diagnostics_retained_bytes(diagnostics: &ParseDiagnostics) -> usize {
    diagnostics
        .unusable_reasons
        .len()
        .saturating_mul(
            size_of::<(CoverageReason, u64)>().saturating_add(BTREE_ENTRY_OVERHEAD_BYTES),
        )
        .saturating_add(string_set_retained_bytes(&diagnostics.unrecognized_types))
        .saturating_add(string_set_retained_bytes(&diagnostics.truncated_strings))
        .saturating_add(string_set_retained_bytes(&diagnostics.capped_collections))
}

fn tools_retained_bytes(tools: &BTreeMap<String, ToolUse>) -> usize {
    tools
        .keys()
        .map(|name| {
            name.capacity()
                .saturating_add(size_of::<ToolUse>())
                .saturating_add(BTREE_ENTRY_OVERHEAD_BYTES)
        })
        .sum()
}

/// One `context_sources` map's (`skills` or `mcp_servers`) retained bytes:
/// each name's capacity, each entry's own size, and each held description's
/// capacity.
fn context_sources_retained_bytes(sources: &BTreeMap<String, LoadedSource>) -> usize {
    sources
        .iter()
        .map(|(name, source)| {
            name.capacity()
                .saturating_add(size_of::<LoadedSource>())
                .saturating_add(source.description.as_ref().map_or(0, String::capacity))
                .saturating_add(BTREE_ENTRY_OVERHEAD_BYTES)
        })
        .sum()
}

fn subagent_children_retained_bytes(children: &[SubagentChild], capacity: usize) -> usize {
    capacity
        .saturating_mul(size_of::<SubagentChild>())
        .saturating_add(
            children
                .iter()
                .map(|child| {
                    child.parent_model.as_ref().map_or(0, String::capacity)
                        + child.parent_call_id.as_ref().map_or(0, String::capacity)
                        + child
                            .observed_child_models
                            .iter()
                            .map(|model| model.capacity() + BTREE_ENTRY_OVERHEAD_BYTES)
                            .sum::<usize>()
                })
                .sum::<usize>(),
        )
}

fn subagent_examples_retained_bytes(examples: &[SubagentExample], capacity: usize) -> usize {
    capacity
        .saturating_mul(size_of::<SubagentExample>())
        .saturating_add(
            examples
                .iter()
                .map(|example| example.parent_model.as_ref().map_or(0, String::capacity))
                .sum::<usize>(),
        )
}

fn hash_string_set_retained_bytes(set: &HashSet<String>) -> usize {
    set.capacity()
        .saturating_mul(size_of::<String>())
        .saturating_add(set.iter().map(String::capacity).sum::<usize>())
}

impl RecordSink for SessionEvidenceAccumulator {
    fn record(&mut self, record: NormalizedRecord) {
        self.observe(&record);
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.observe_summary(&summary);
    }
}

/// Metrics and evidence state as they stood immediately before the most
/// recent [`RecordSink::finish`] mutated them. See [`CompositeSink::snapshot`].
struct PreFinishState {
    metrics: SessionMetricsAccumulator,
    evidence: EvidenceSnapshot,
}

pub struct CompositeSink {
    metrics: SessionMetricsAccumulator,
    evidence: SessionEvidenceAccumulator,
    turn_rows: Option<TurnRowSink>,
    /// The `SessionSummary` [`RecordSink::finish`] was called with, kept
    /// for [`Self::summary`] — a caller that fans this source's summary out
    /// to a `BTreeMap<source_key, SessionSummary>` (a worker pass, for the
    /// desktop app's drilldown replay) needs it after `finish` runs, and
    /// `finish` itself only borrows the summary before moving it on to
    /// `self.metrics`.
    summary: Option<SessionSummary>,
    /// Set by [`RecordSink::finish`], `None` before the first call. See
    /// [`Self::snapshot`].
    pre_finish: Option<PreFinishState>,
}

impl CompositeSink {
    pub fn new(metrics: SessionMetricsAccumulator, evidence: SessionEvidenceAccumulator) -> Self {
        Self {
            metrics,
            evidence,
            turn_rows: None,
            summary: None,
            pre_finish: None,
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
        }
    }

    pub fn metrics(&self) -> Option<SessionMetrics> {
        self.evidence.can_publish().then(|| self.metrics.metrics())
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
        })
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
        // `Self::snapshot` needs the state from before `finish` mutates it.
        // Only a sink with turn rows can snapshot, so a sink without them
        // does not pay for the clone.
        if self.turn_rows.is_some() {
            self.pre_finish = Some(PreFinishState {
                metrics: self.metrics.snapshot(),
                evidence: EvidenceSnapshot {
                    record: self.evidence.coverage_record(),
                    resume: self.evidence.resume_state(),
                },
            });
        }
        self.evidence.observe_summary(&summary);
        if let Some(turn_rows) = &mut self.turn_rows {
            turn_rows.flush();
        }
        self.summary = Some(summary.clone());
        self.metrics.finish(summary);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::analysis::evidence::ModelTokens;
    use crate::analysis::model::{Role, ToolCall};
    use crate::analysis::rows::{MemoryTurnRowStore, TurnRowStore};
    use crate::analysis::{EVIDENCE_STRING_CAP, PartialReason, RawSource, SessionReader};

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
            fork_parent_session_id: None,
        };
        let mut composite = composite_with_rows("claude", "attachment");
        let outcome = crate::analysis::ClaudeSessionReader
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
            fork_parent_session_id: None,
        };
        let mut composite = composite_with_rows("claude", "unknown");
        let outcome = crate::analysis::ClaudeSessionReader
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
        let facts = TurnFacts::default();
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
    fn child_model_controls_reach_detectors_through_reader_rows_and_replay() {
        use crate::analysis::{ClaudeSessionReader, SessionInput, TurnScope};
        use crate::insights::{
            DetectorId, DetectorStatus, EfficiencyReportAccumulator, ReportContext, ReportWindow,
        };

        for (parent_speed, child_effort, child_speed, finding) in [
            ("standard", "max", "fast", true),
            ("fast", "low", "standard", false),
        ] {
            let store = MemoryTurnRowStore::new("claude", "parent");
            let mut residuals = Vec::new();
            for (id, effort, speed, scope) in [
                ("parent", "low", parent_speed, None),
                (
                    "child",
                    child_effort,
                    child_speed,
                    Some(TurnScope::Delegated),
                ),
            ] {
                let input = SessionInput {
                    agent: "claude".to_owned(),
                    session_id: id.to_owned(),
                    source: RawSource::Jsonl(
                        serde_json::json!({
                            "type": "assistant", "uuid": id,
                            "timestamp": "2026-09-01T12:00:00Z", "effort": effort,
                            "message": {"role": "assistant", "model": "claude-opus-4-6",
                                "usage": {"input_tokens": 10, "output_tokens": 5, "speed": speed},
                                "content": [{"type": "text", "text": "Synthetic response."}]}
                        })
                        .to_string(),
                    ),
                    fork_parent_session_id: None,
                };
                let mut sink = CompositeSink::with_turn_rows(
                    SessionMetricsAccumulator::new("claude", id),
                    accumulator(true),
                    TurnRowSink::new(
                        Arc::clone(&store) as Arc<dyn TurnRowStore>,
                        id.to_owned(),
                        scope,
                    ),
                );
                let outcome = ClaudeSessionReader
                    .visit(&input, &mut sink)
                    .expect("parse transcript");
                sink.observe_source_outcome(outcome);
                residuals.push(sink.into_parts().expect("published residual").1);
            }
            let child = residuals.pop().unwrap();
            let mut parent = residuals.pop().unwrap();
            parent.observe_child_coverage(&child);
            let facts = store
                .query_turn_facts()
                .expect("query parent and child rows");
            let evidence = parent.evidence(&facts);
            let EvidenceValue::Complete(models) = &evidence.models else {
                panic!("model evidence must be complete");
            };
            assert_eq!(models.control_observations.len(), 2);
            assert_eq!(models.control_observations[0].turns.main_loop, 1);
            assert_eq!(models.control_observations[0].turns.delegated, 0);
            assert_eq!(models.control_observations[1].turns.main_loop, 0);
            assert_eq!(models.control_observations[1].turns.delegated, 1);
            let encoded = serde_json::to_vec(&parent.coverage_record()).unwrap();
            let restored = SessionEvidenceAccumulator::from_coverage_record(
                serde_json::from_slice(&encoded).unwrap(),
            );
            assert_eq!(evidence, restored.evidence(&facts));

            let mut report = EfficiencyReportAccumulator::new();
            report.observe_session(restored.evidence(&facts));
            let report = report.finish(ReportContext {
                environment_key: "native".to_owned(),
                window: ReportWindow {
                    start_epoch: 0,
                    end_epoch: i64::MAX,
                },
                computed_at_epoch: 0,
                parser_revision: PARSER_REVISION,
                analyzer_revision: ANALYZER_REVISION,
                evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
                coverage: Default::default(),
            });
            for detector in [DetectorId::ModelOverthinking, DetectorId::OveruseOfFastMode] {
                let status = &report.detector_statuses[detector.index()];
                if finding {
                    assert!(
                        matches!(status, DetectorStatus::Findings(_)),
                        "{detector:?}: {status:?}"
                    );
                } else {
                    assert_eq!(status, &DetectorStatus::Clean, "{detector:?}");
                }
            }
        }
    }

    #[test]
    fn child_model_controls_merge_at_capacity_with_saturating_delegated_counts() {
        let mut parent = accumulator(true);
        for index in 0..MAX_MODEL_CONTROL_OBSERVATIONS {
            let mut event = assistant_event(index);
            event.thinking_mode = Some("low".to_owned());
            parent.observe_event(&event);
        }
        let mut child = accumulator(true);
        let mut event = assistant_event(MAX_MODEL_CONTROL_OBSERVATIONS);
        event.thinking_mode = Some("low".to_owned());
        child.observe_event(&event);
        event.model = Some("model-0".to_owned());
        child.observe_event(&event);
        child.model_control_observations[1].turns.main_loop = u64::MAX;
        child.model_control_observations[1].turns.delegated = 1;
        parent.model_control_observations[0].turns.delegated = 1;
        parent.observe_child_coverage(&child);
        let evidence = parent.evidence(&TurnFacts::default());
        assert_capped_collection(&evidence, "models.control_observations");
        let models = assert_cap_partial(evidence.models);
        assert_eq!(
            models.control_observations.len(),
            MAX_MODEL_CONTROL_OBSERVATIONS
        );
        assert_eq!(models.control_observations[0].turns.main_loop, 1);
        assert_eq!(models.control_observations[0].turns.delegated, u64::MAX);
        assert!(matches!(evidence.tools, EvidenceValue::Complete(_)));
    }

    #[test]
    fn child_model_control_caps_and_truncation_keep_parent_models_partial() {
        for truncate in [false, true] {
            let mut child = accumulator(true);
            for index in 0..=MAX_MODEL_CONTROL_OBSERVATIONS {
                let mut event = assistant_event(index);
                event.thinking_mode = Some("low".to_owned());
                if truncate {
                    event.model = Some(long_string());
                }
                child.observe_event(&event);
            }
            let mut parent = accumulator(true);
            parent.observe_child_coverage(&child);
            let evidence = parent.evidence(&TurnFacts::default());
            if truncate {
                assert_truncated_string(&evidence, "models.control_observations.model");
            } else {
                assert_capped_collection(&evidence, "models.control_observations");
            }
            assert_cap_partial(evidence.models);
            assert!(matches!(evidence.tools, EvidenceValue::Complete(_)));
            assert!(matches!(evidence.context, EvidenceValue::Complete(_)));
        }
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
    fn a_childs_tool_call_never_marks_a_parent_definition_invoked() {
        let mut parent = accumulator(true);
        parent.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::HarnessVersion {
                version: "2.1.233".to_owned(),
            },
        )));
        let mut child = accumulator(true);
        let mut bash_call = assistant_event(0);
        bash_call.tools.push(ToolCall::new("Bash"));
        child.record(NormalizedRecord::MetricsEvent(Box::new(bash_call)));

        // Tool definitions are parent-only, the same as `tools` and
        // `context_sources`: folding a child's coverage never reaches
        // them (see `Self::observe_child_coverage`'s doc comment).
        parent.observe_child_coverage(&child);

        let facts = TurnFacts {
            by_model: BTreeMap::from([(
                "claude-opus-4-6".to_owned(),
                ModelTokens {
                    turns: 1,
                    ..ModelTokens::default()
                },
            )]),
            ..TurnFacts::default()
        };
        let evidence = parent.evidence(&facts);
        let EvidenceValue::Complete(sources) = evidence.context_sources else {
            panic!("context sources must be complete");
        };
        let EvidenceValue::Complete(definitions) = sources.tool_definitions else {
            panic!("tool definitions must resolve with a known version and model");
        };
        assert!(
            definitions
                .get("Bash")
                .is_some_and(|definition| !definition.invoked),
            "a child's own Bash call must never mark the parent's Bash definition invoked"
        );
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

    #[test]
    fn resource_coverage_is_independent_and_survives_replay() {
        for capped_kind in [ContextSourceKind::Skill, ContextSourceKind::McpServer] {
            let mut sink = accumulator(true);
            sink.observe_context_source(ContextSourceKind::Skill, "skill", None, true, false);
            sink.observe_context_source(ContextSourceKind::McpServer, "mcp", None, true, false);
            for index in 0..MAX_CONTEXT_SOURCES {
                sink.observe_context_source(
                    capped_kind,
                    &format!("source-{index}"),
                    None,
                    true,
                    false,
                );
            }
            let replay = SessionEvidenceAccumulator::from_coverage_record(sink.coverage_record());
            let evidence = replay.evidence(&TurnFacts::default());
            let EvidenceValue::Partial {
                observed: sources, ..
            } = evidence.context_sources
            else {
                panic!("the coarse group must retain its cap");
            };
            let (capped, complete) = match capped_kind {
                ContextSourceKind::Skill => (sources.skill_coverage, sources.mcp_coverage),
                ContextSourceKind::McpServer => (sources.mcp_coverage, sources.skill_coverage),
            };
            assert_eq!(complete, EvidenceValue::Complete(()));
            assert_eq!(
                capped,
                EvidenceValue::Partial {
                    observed: (),
                    reason: CoverageReason::CapExceeded
                }
            );
        }
    }

    #[test]
    fn missing_resource_lifecycle_does_not_prove_empty_inventory() {
        let sink = accumulator(true);
        let EvidenceValue::Complete(sources) = sink.evidence(&TurnFacts::default()).context_sources
        else {
            panic!("the wrapper remains available for built-in definitions");
        };
        assert_eq!(sources.skill_coverage, EvidenceValue::Unsupported);
        assert_eq!(sources.mcp_coverage, EvidenceValue::Unsupported);
    }

    #[test]
    fn record_loss_degrades_each_observed_resource() {
        let mut sink = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "codex".to_owned(),
            session_id: "resource-record-loss".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::codex(),
        });
        sink.observe_context_source(ContextSourceKind::Skill, "skill", None, true, false);
        sink.observe_context_source(ContextSourceKind::McpServer, "mcp", None, true, false);
        sink.set_record_loss_reason(CoverageReason::MalformedRecord);
        let sink = SessionEvidenceAccumulator::from_coverage_record(sink.coverage_record());
        let EvidenceValue::Complete(sources) = sink.evidence(&TurnFacts::default()).context_sources
        else {
            unreachable!()
        };
        for coverage in [sources.skill_coverage, sources.mcp_coverage] {
            assert_eq!(
                coverage,
                EvidenceValue::Partial {
                    observed: (),
                    reason: CoverageReason::MalformedRecord
                }
            );
        }
    }

    #[test]
    fn a_skill_suffix_does_not_match_a_namespaced_identity() {
        let mut accumulator = accumulator(true);
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::ContextSource {
                kind: ContextSourceKind::Skill,
                name: "plugin:deploy".to_owned(),
                description: None,
            },
        )));
        let mut event = assistant_event(0);
        event.tools.push(crate::analysis::ToolCall::new("deploy"));
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));

        let evidence = accumulator.evidence(&TurnFacts::default());

        let EvidenceValue::Complete(sources) = &evidence.context_sources else {
            panic!("context_sources must be complete");
        };
        let skill = sources
            .skills
            .get("plugin:deploy")
            .expect("plugin:deploy must be recorded as a loaded skill");
        assert!(!skill.invoked);

        let EvidenceValue::Complete(tools) = &evidence.tools else {
            panic!("tools must be complete");
        };
        assert_eq!(
            tools.by_name.get("deploy").map(|tool| tool.class),
            Some(ToolClass::Unclassified)
        );
    }

    #[test]
    fn an_explicit_skill_alias_matches_one_namespaced_identity() {
        let mut accumulator = accumulator(true);
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::ContextSource {
                kind: ContextSourceKind::Skill,
                name: "plugin:deploy".to_owned(),
                description: None,
            },
        )));
        let mut event = assistant_event(0);
        let mut skill = crate::analysis::ToolCall::new("Skill");
        skill.detail = Some("deploy".to_owned());
        event.tools.push(skill);
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));

        let evidence = accumulator.evidence(&TurnFacts::default());

        let EvidenceValue::Complete(sources) = &evidence.context_sources else {
            panic!("context_sources must be complete");
        };
        assert!(sources.skills["plugin:deploy"].invoked);
    }

    #[test]
    fn an_ambiguous_skill_alias_makes_attribution_partial() {
        let mut accumulator = accumulator(true);
        for name in ["a:deploy", "b:deploy"] {
            accumulator.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::ContextSource {
                    kind: ContextSourceKind::Skill,
                    name: name.to_owned(),
                    description: None,
                },
            )));
        }
        let mut event = assistant_event(0);
        let mut skill = crate::analysis::ToolCall::new("Skill");
        skill.detail = Some("deploy".to_owned());
        event.tools.push(skill);
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));

        let evidence = accumulator.evidence(&TurnFacts::default());

        assert!(matches!(
            evidence.context_sources,
            EvidenceValue::Partial {
                reason: CoverageReason::AttributionIncomplete,
                ..
            }
        ));
    }

    fn subagents_overflow() -> SessionEvidence {
        let mut accumulator = accumulator(true);
        for index in 0..(MAX_SUBAGENT_CHILDREN * 2) {
            accumulator.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::SubagentSpawn {
                    ts_ms: Some(i64::try_from(index).unwrap()),
                    parent_model: Some("model".to_owned()),
                    parent_call_id: None,
                    child_model: None,
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
    fn context_sources_skill_description_overflow_stays_complete() {
        let evidence = source_string_overflow(ContextSourceKind::Skill, true);
        assert_truncated_string(&evidence, "context_sources.skills.description");
        assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
        let EvidenceValue::Complete(sources) = evidence.context_sources else {
            panic!("a truncated description must not degrade context_sources");
        };
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

    #[test]
    fn native_child_models_require_one_exact_call_and_reject_truncated_identities() {
        for (ids, joined_id) in [
            (vec!["call-1".to_owned()], None),
            (vec!["call-1".to_owned()], Some("other".to_owned())),
            (
                vec!["call-1".to_owned(), "call-1".to_owned()],
                Some("call-1".to_owned()),
            ),
            (vec![long_string()], Some(long_string())),
        ] {
            let mut accumulator = accumulator(true);
            for id in ids {
                accumulator.observe_subagent_spawn(
                    None,
                    Some("claude-opus-4-6"),
                    Some(&id),
                    None,
                    crate::analysis::RelationProvenance::TaskToolUse,
                );
            }
            accumulator
                .observe_child_models(joined_id.as_deref(), std::iter::once("claude-opus-4-6"));
            let record = accumulator.coverage_record();
            assert!(
                record
                    .subagent_children
                    .iter()
                    .all(|child| child.observed_child_models.is_empty())
            );
            assert!(matches!(
                accumulator.evidence(&TurnFacts::default()).subagents,
                EvidenceValue::Partial { .. }
            ));
        }
    }

    #[test]
    fn native_child_model_pairs_are_bounded_and_survive_json_and_binary_replay() {
        let mut accumulator = accumulator(true);
        accumulator.observe_subagent_spawn(
            None,
            Some("claude-opus-4-6"),
            Some("call-1"),
            None,
            crate::analysis::RelationProvenance::TaskToolUse,
        );
        accumulator.observe_child_models(Some("call-1"), std::iter::once(long_string().as_str()));
        assert!(
            accumulator.coverage_record().subagent_children[0]
                .observed_child_models
                .is_empty()
        );
        for index in 0..=crate::analysis::evidence::MAX_SUBAGENT_MODELS {
            accumulator.observe_child_models(
                Some("call-1"),
                std::iter::once(format!("model-{index}").as_str()),
            );
        }
        let record = accumulator.coverage_record();
        assert_eq!(
            record.subagent_children[0].observed_child_models.len(),
            crate::analysis::evidence::MAX_SUBAGENT_MODELS
        );
        assert!(record.subagents_cap_exceeded);
        assert!(accumulator.retained_bytes() < RETAINED_EVIDENCE_BYTES_BOUND);
        let decoded: SessionCoverageRecord =
            postcard::from_bytes(&postcard::to_allocvec(&record).unwrap()).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(
            SessionEvidenceAccumulator::from_coverage_record(decoded)
                .evidence(&TurnFacts::default()),
            accumulator.evidence(&TurnFacts::default())
        );
        let mut legacy = serde_json::to_value(&record).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("subagentLinkageIncomplete");
        for child in legacy["subagentChildren"].as_array_mut().unwrap() {
            child.as_object_mut().unwrap().remove("parentCallId");
            child.as_object_mut().unwrap().remove("observedChildModels");
        }
        let decoded: SessionCoverageRecord = serde_json::from_value(legacy).unwrap();
        assert!(decoded.subagent_children[0].parent_call_id.is_none());
        assert!(
            decoded.subagent_children[0]
                .observed_child_models
                .is_empty()
        );
    }

    fn subagent_string_overflow() -> SessionEvidence {
        let mut accumulator = accumulator(true);
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::SubagentSpawn {
                ts_ms: Some(1),
                parent_model: Some(long_string()),
                parent_call_id: None,
                child_model: None,
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
        assert!(subagents.children[0].parent_model.is_none());
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
    fn long_thread_identity_tracking_preserves_complete_coverage() {
        let mut accumulator = accumulator(true);
        for index in 0..MAX_TRACKED_THREAD_UUIDS {
            accumulator.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::ThreadLink {
                    uuid: Some(format!("{index:0256}")),
                    parent_uuid: (index > 0).then(|| format!("{:0256}", index - 1)),
                },
            )));
        }
        let encoded = serde_json::to_vec(&accumulator.resume_state()).unwrap();
        let resume = serde_json::from_slice(&encoded).unwrap();
        let restored = SessionEvidenceAccumulator::from_coverage_record_with_resume(
            accumulator.coverage_record(),
            resume,
        );
        let evidence = restored.evidence(&TurnFacts::default());
        assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
        assert!(
            !evidence
                .diagnostics
                .capped_collections
                .contains(THREAD_UUIDS_DIAGNOSTIC)
        );
        assert!(restored.retained_bytes() < RETAINED_EVIDENCE_BYTES_BOUND);
    }

    #[test]
    fn thread_identity_tracking_is_bounded_and_degrades_coverage() {
        let mut accumulator = accumulator(true);
        for index in 0..=MAX_TRACKED_THREAD_UUIDS {
            accumulator.record(NormalizedRecord::Observation(Box::new(
                EvidenceObservation::ThreadLink {
                    uuid: Some(format!("u-{index}")),
                    parent_uuid: None,
                },
            )));
        }

        assert_eq!(
            accumulator.resume_state().seen_thread_uuids.len(),
            MAX_TRACKED_THREAD_UUIDS
        );
        let evidence = accumulator.evidence(&TurnFacts::default());
        assert_eq!(
            evidence.coverage,
            EvidenceCoverage::Partial(CoverageReason::CapExceeded)
        );
        assert_eq!(
            evidence_reason(&evidence.cache),
            Some(CoverageReason::AttributionIncomplete)
        );
        assert_capped_collection(&evidence, THREAD_UUIDS_DIAGNOSTIC);
        assert!(accumulator.retained_bytes() < RETAINED_EVIDENCE_BYTES_BOUND);
    }

    #[test]
    fn oversized_resume_thread_state_is_bounded_and_degrades_coverage() {
        let accumulator = accumulator(true);
        let record = accumulator.coverage_record();
        let resume = EvidenceResumeState {
            last_ts_ms: Some(10),
            seen_thread_uuids: (0..=MAX_TRACKED_THREAD_UUIDS)
                .map(|index| format!("u-{index}"))
                .collect(),
        };

        let restored = SessionEvidenceAccumulator::from_coverage_record_with_resume(record, resume);
        assert_eq!(
            restored.resume_state().seen_thread_uuids.len(),
            MAX_TRACKED_THREAD_UUIDS
        );
        let evidence = restored.evidence(&TurnFacts::default());
        assert_eq!(
            evidence.coverage,
            EvidenceCoverage::Partial(CoverageReason::CapExceeded)
        );
        assert_eq!(
            evidence_reason(&evidence.cache),
            Some(CoverageReason::AttributionIncomplete)
        );
    }

    #[test]
    fn serialized_resume_thread_state_rejects_overflow() {
        let resume = EvidenceResumeState {
            last_ts_ms: None,
            seen_thread_uuids: (0..=MAX_TRACKED_THREAD_UUIDS)
                .map(|index| format!("u-{index}"))
                .collect(),
        };
        let encoded = serde_json::to_vec(&resume).unwrap();

        assert!(serde_json::from_slice::<EvidenceResumeState>(&encoded).is_err());
    }

    #[test]
    fn a_compaction_boundarys_null_parent_uuid_keeps_the_link_verified() {
        // `records::evidence_observations` reads a compaction boundary's
        // `ThreadLink.parent_uuid` from `parentUuid` only, with no fallback
        // to `logicalParentUuid`. The boundary's real `parentUuid` is
        // always null, so its `ThreadLink` carries no parent to verify: the
        // boundary's own `uuid` still registers, and the next turn's link
        // to it resolves normally.
        let mut accumulator = accumulator(true);
        for record in thread_record(Some("u-1"), None, 0) {
            accumulator.record(record);
        }
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::ThreadLink {
                uuid: Some("boundary".to_owned()),
                parent_uuid: None,
            },
        )));
        for record in thread_record(Some("u-2"), Some("boundary"), 1) {
            accumulator.record(record);
        }
        let facts = TurnFacts::default();
        let EvidenceValue::Complete(cache) = accumulator.evidence(&facts).cache else {
            panic!("a null parentUuid on a boundary record must keep the cache group complete");
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

    /// A source with `linear_record_order` but no `record_identity`
    /// (Codex's shape) attests linkage from line order alone: no counted
    /// record was lost, so every turn's predecessor is the counted record
    /// immediately before it.
    #[test]
    fn a_linear_order_source_with_no_loss_completes_previous_turn() {
        let mut accumulator = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "codex".to_owned(),
            session_id: "s1".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::codex(),
        });
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(assistant_event(1))));
        let EvidenceValue::Complete(cache) = accumulator.evidence(&TurnFacts::default()).cache
        else {
            panic!("cache must be complete: no record loss and no child");
        };
        assert_eq!(cache.previous_turn, EvidenceValue::Complete(()));
    }

    /// The same linear-order source, but a record was lost: the order
    /// claim can no longer prove line N-1 is the predecessor of line N, so
    /// linkage degrades to the same reason the loss itself carries.
    #[test]
    fn a_linear_order_source_with_a_record_loss_degrades_previous_turn() {
        let mut accumulator = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "codex".to_owned(),
            session_id: "s1".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::codex(),
        });
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(assistant_event(1))));
        accumulator.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
        let cache = match accumulator.evidence(&TurnFacts::default()).cache {
            EvidenceValue::Complete(cache)
            | EvidenceValue::Partial {
                observed: cache, ..
            } => cache,
            EvidenceValue::Unsupported => panic!("cache group must stay supported"),
        };
        assert_eq!(
            cache.previous_turn,
            EvidenceValue::Partial {
                observed: (),
                reason: CoverageReason::MalformedRecord,
            }
        );
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
