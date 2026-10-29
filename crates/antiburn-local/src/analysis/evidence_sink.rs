use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::mem::size_of;

use serde::de::{SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::analysis::evidence::{
    CacheEvidence, ChurnCounts, CompactionEvidence, ContextEvidence, ContextSourceEvidence,
    CoverageReason, EVIDENCE_STRING_CAP, EvidenceCoverage, EvidenceSource, EvidenceValue,
    LoadedSource, MAX_CONTEXT_SOURCES, MAX_EVIDENCE_EXAMPLES, MAX_PROVIDER_INCIDENTS,
    MAX_QUOTA_INCIDENTS, MAX_SUBAGENT_CHILDREN, MAX_TOOL_NAMES, MAX_UNRECOGNIZED_TYPES,
    ModelControlObservation, ModelEvidence, OrderingObservation, ParseDiagnostics,
    ProviderIncident, QuotaIncident, RelationConfidence, RepeatedContext,
    RepeatedContextAccounting, SessionCoverageRecord, SessionEvidence, SessionEvidenceIdentity,
    SessionProvenance, SessionProviderEvidence, SessionQuotaEvidence, SourceAcceptance,
    SourceCapabilities, SourceKind, SubagentChild, SubagentConfigBinding, SubagentEvidence,
    SubagentExample, ToolClass, ToolDefinition, ToolEvidence, ToolUse, cap_string,
    insert_diagnostic_field, record_diagnostic_set_cap,
};
use crate::analysis::evidence_query::TurnFacts;
use crate::analysis::initial_context::{InitialContextTokenSource, SourceOrigin};
use crate::analysis::interface::{
    ContextSourceKind, EvidenceObservation, NormalizedRecord, RecordSink, SessionSummary,
    ToolResultObservation, UserMessage, VisitOutcome,
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

mod composite;

/// Maximum number of native tool-result observations and conflict identities
/// retained per composite sink and resume snapshot.
pub const MAX_TOOL_RESULT_OBSERVATIONS: usize = 4096;

/// Maximum total JSON-text bytes retained for native tool-result observations
/// and conflict identities in a composite sink and resume snapshot.
pub const MAX_TOOL_RESULT_BYTES: usize = 4 * 1024 * 1024;

/// Maximum events retained in the separate generic native evidence collection.
pub const MAX_NATIVE_EVIDENCE_OBSERVATIONS: usize = 4096;

/// Maximum JSON bytes retained in the separate generic native evidence collection.
pub const MAX_NATIVE_EVIDENCE_BYTES: usize = 4 * 1024 * 1024;

/// Maximum number of user-message observations and conflict identities
/// retained per composite sink and resume snapshot.
pub const MAX_USER_MESSAGE_OBSERVATIONS: usize = 4096;

/// Maximum total JSON-text bytes retained for user-message observations
/// and conflict identities in a composite sink and resume snapshot.
pub const MAX_USER_MESSAGE_COLLECTION_BYTES: usize = 4 * 1024 * 1024;

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
    /// Transcript-observed quota incidents, from a `QuotaIncident`
    /// observation. Bounded by [`MAX_QUOTA_INCIDENTS`].
    quota_incidents: Vec<QuotaIncident>,
    /// True when a `QuotaIncident` observation overflowed
    /// [`MAX_QUOTA_INCIDENTS`].
    quota_incidents_capped: bool,
    /// Transcript-observed provider incidents, from a `ProviderIncident`
    /// observation. Bounded by [`MAX_PROVIDER_INCIDENTS`].
    provider_incidents: Vec<ProviderIncident>,
    /// True when a `ProviderIncident` observation overflowed
    /// [`MAX_PROVIDER_INCIDENTS`].
    provider_incidents_capped: bool,
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
            quota_incidents: Vec::new(),
            quota_incidents_capped: false,
            provider_incidents: Vec::new(),
            provider_incidents_capped: false,
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
            NormalizedRecord::ToolResult(_)
            | NormalizedRecord::PublicMessage(_)
            | NormalizedRecord::ResponseCycle(_) => {}
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
                config_binding,
            } => self.observe_subagent_spawn(
                *ts_ms,
                parent_model.as_deref(),
                parent_call_id.as_deref(),
                child_model.as_deref(),
                *provenance,
                config_binding.as_ref(),
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
            EvidenceObservation::QuotaIncident(incident) => {
                if self.quota_incidents.len() < MAX_QUOTA_INCIDENTS {
                    self.quota_incidents.push(incident.clone());
                } else {
                    self.quota_incidents_capped = true;
                    self.note_collection_cap("quota_incidents.incidents");
                }
            }
            EvidenceObservation::ProviderIncident(incident) => {
                if self.provider_incidents.len() < MAX_PROVIDER_INCIDENTS {
                    self.provider_incidents.push(incident.clone());
                } else {
                    self.provider_incidents_capped = true;
                    self.note_collection_cap("provider_incidents.incidents");
                }
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
        config_binding: Option<&SubagentConfigBinding>,
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
        let mut duplicate_call = false;
        if let Some(id) = &parent_call_id {
            for child in &mut self.subagent_children {
                if child.parent_call_id.as_ref() == Some(id) {
                    child.config_binding = None;
                    duplicate_call = true;
                }
            }
        }
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
                config_binding: None,
            });
            if let Some(model) = child_model {
                self.insert_child_model(index, model);
            }
            if !duplicate_call
                && self.capabilities.source_format
                    == crate::analysis::SourceFormat::OpenCodeSqliteV2
                && provenance == crate::analysis::RelationProvenance::SessionParentLink
                && let Some(binding) = config_binding
                && self.subagent_children[index].parent_call_id.is_some()
                && self.subagent_children[index].observed_child_models.len() == 1
                && self.subagent_children[index]
                    .observed_child_models
                    .contains(&binding.model_id)
                && self.config_binding_is_bounded(binding)
            {
                self.subagent_children[index].config_binding = Some(binding.clone());
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

    fn config_binding_is_bounded(&mut self, binding: &SubagentConfigBinding) -> bool {
        let mut valid = true;
        for (field, value) in [
            (
                "subagents.children.config_binding.agent_id",
                &binding.agent_id,
            ),
            (
                "subagents.children.config_binding.provider_id",
                &binding.provider_id,
            ),
            (
                "subagents.children.config_binding.model_id",
                &binding.model_id,
            ),
        ] {
            if value.len() > EVIDENCE_STRING_CAP {
                cap_string(field, value, &mut self.diagnostics);
                self.subagents_cap_exceeded = true;
                valid = false;
            }
            valid &= !value.trim().is_empty() && !value.chars().any(char::is_control);
        }
        valid
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
        if self.subagent_children[index]
            .config_binding
            .as_ref()
            .is_some_and(|binding| binding.model_id != model)
        {
            self.subagent_children[index].config_binding = None;
        }
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
            quota_incidents: self.quota_incidents.clone(),
            quota_incidents_capped: self.quota_incidents_capped,
            provider_incidents: self.provider_incidents.clone(),
            provider_incidents_capped: self.provider_incidents_capped,
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
            quota_incidents: record.quota_incidents,
            quota_incidents_capped: record.quota_incidents_capped,
            provider_incidents: record.provider_incidents,
            provider_incidents_capped: record.provider_incidents_capped,
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
        // The row scan selects one proven contract and excludes incompatible requests from its partial total.
        let repeated_context_accounting = match self.identity.agent.as_str() {
            "claude" | "claude-code" | "codex" => self.capabilities.repeated_context_accounting,
            _ => facts.repeated_context_accounting,
        }
        .filter(|accounting| {
            self.capabilities.token_classes
                && self.capabilities.request_context_tokens
                && (*accounting != RepeatedContextAccounting::CacheWrite
                    || self.capabilities.cache_write_tokens
                    || self.identity.agent == "pi")
        });
        let repeated_context = match repeated_context_accounting {
            None => EvidenceValue::Unsupported,
            Some(accounting) => {
                let (repeated_tokens, paid_tokens) = match accounting {
                    RepeatedContextAccounting::CacheWrite => (
                        facts.repeated_context_cache_write_tokens,
                        facts.repeated_context_cache_write_paid_tokens,
                    ),
                    RepeatedContextAccounting::UncachedInput => (
                        facts.repeated_context_uncached_input_tokens,
                        facts.repeated_context_uncached_input_paid_tokens,
                    ),
                };
                let observed = RepeatedContext {
                    accounting,
                    repeated_tokens,
                    paid_tokens,
                    pairs_considered: facts.repeated_context_pairs_considered,
                    pairs_skipped: facts.repeated_context_pairs_skipped,
                    transient_miss_episodes: facts.transient_miss_episodes,
                    possible_rehydration_episodes: facts.possible_rehydration_episodes,
                };
                if let Some(reason) = cache_partial_reason {
                    EvidenceValue::Partial { observed, reason }
                } else if facts.repeated_context_incomplete
                    || facts.repeated_context_pairs_skipped > 0
                {
                    EvidenceValue::Partial {
                        observed,
                        reason: CoverageReason::AttributionIncomplete,
                    }
                } else {
                    EvidenceValue::Complete(observed)
                }
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
            quota_incidents: self.supported_value(
                SessionQuotaEvidence {
                    incidents: self.quota_incidents.clone(),
                },
                self.capabilities.quota_incidents,
                child_dependent_partial,
                self.quota_incidents_capped,
            ),
            provider_incidents: self.supported_value(
                SessionProviderEvidence {
                    incidents: self.provider_incidents.clone(),
                },
                self.capabilities.provider_incidents,
                child_dependent_partial,
                self.provider_incidents_capped,
            ),
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
        let Some(tools) = catalog.lookup(&self.identity.agent, version, model) else {
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
                        + child.config_binding.as_ref().map_or(0, |binding| {
                            binding.agent_id.capacity()
                                + binding.provider_id.capacity()
                                + binding.model_id.capacity()
                        })
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
    pending_native_finish: Option<PreFinishState>,
    tool_result_observations: Vec<ToolResultObservation>,
    tool_result_observation_sizes: Vec<usize>,
    native_tool_result_indices: HashMap<String, usize>,
    native_tool_result_conflicts: BTreeSet<String>,
    tool_result_observations_bytes: usize,
    tool_result_observations_complete: bool,
    user_messages: Vec<UserMessage>,
    user_message_indices: HashMap<String, usize>,
    user_message_conflicts: BTreeSet<String>,
    user_messages_bytes: usize,
    user_messages_complete: bool,
    native_evidence: Vec<crate::analysis::NativeEvidenceEvent>,
    native_evidence_bytes: usize,
    native_evidence_complete: bool,
    native_admission_failure: Option<crate::analysis::NativeAdmissionFailure>,
}

impl RecordSink for CompositeSink {
    fn record(&mut self, mut record: NormalizedRecord) {
        if matches!(record, NormalizedRecord::Unusable(_)) {
            self.user_messages_complete = false;
            self.native_evidence_complete = false;
        }
        if let NormalizedRecord::ToolResult(result) = &mut record
            && let Some(context) = &mut result.context
            && let crate::analysis::ToolResultPosition::Jsonl { record_index, .. } =
                context.result_position
        {
            context.user_messages = self.user_messages.iter().filter(|request| {
                request.source_format == result.source_format
                    && request.session_id == result.session_id
                    && matches!(request.position, crate::analysis::UserMessagePosition::Jsonl { record_index: index } if index < record_index)
            }).cloned().collect();
            context.complete_through_event = context.main_session
                && self.user_messages_complete
                && context.user_messages.iter().all(|request| {
                    request.change == crate::analysis::UserMessageChange::Present
                        && request.completeness
                            == crate::analysis::UserMessageCompleteness::Complete
                });
        }
        if let NormalizedRecord::ToolResult(result) = &mut record {
            if result
                .context
                .as_ref()
                .is_some_and(|context| !context.main_session)
                && matches!(
                    result.completeness,
                    crate::analysis::ToolResultCompleteness::Complete
                        | crate::analysis::ToolResultCompleteness::Incomplete
                )
            {
                result.completeness = crate::analysis::ToolResultCompleteness::UnsafeOwnership;
            }
            self.observe_native_tool_result((**result).clone());
        }
        match &mut record {
            NormalizedRecord::PublicMessage(value) => {
                if let crate::analysis::NativeMessageObservation::Public(finalized) = self
                    .finalize_native_message(crate::analysis::NativeMessageObservation::Public(
                        (**value).clone(),
                    ))
                {
                    **value = finalized;
                }
                self.retain_native_evidence(crate::analysis::NativeEvidenceFact::Message(
                    crate::analysis::NativeMessageObservation::Public((**value).clone()),
                ));
            }
            NormalizedRecord::ToolResult(value) => {
                self.retain_native_evidence(crate::analysis::NativeEvidenceFact::Message(
                    crate::analysis::NativeMessageObservation::ToolResult((**value).clone()),
                ))
            }
            NormalizedRecord::ResponseCycle(value) => self.retain_native_evidence(
                crate::analysis::NativeEvidenceFact::ResponseCycle((**value).clone()),
            ),
            _ => {}
        }
        self.evidence.observe(&record);
        if let Some(turn_rows) = &mut self.turn_rows {
            turn_rows.observe(&record);
        }
        self.metrics.record(record);
    }

    fn user_message(&mut self, request: UserMessage) {
        if self.user_messages.iter().any(|prior| prior == &request) {
            return;
        }
        let Some(observation) = crate::analysis::public_events::user_observation(request.clone())
        else {
            self.observe_user_message(request);
            return;
        };
        let message = self
            .finalize_native_message(crate::analysis::NativeMessageObservation::User(observation));
        self.observe_user_message(request);
        self.retain_native_evidence(crate::analysis::NativeEvidenceFact::Message(message));
    }

    fn finalize_native_message(
        &mut self,
        message: crate::analysis::NativeMessageObservation,
    ) -> crate::analysis::NativeMessageObservation {
        let position = message.position();
        let (source_format, session_id) = match &message {
            crate::analysis::NativeMessageObservation::User(value) => (
                value.message.source_format,
                value.message.session_id.as_str(),
            ),
            crate::analysis::NativeMessageObservation::Public(value) => {
                (value.source_format, value.session_id.as_str())
            }
            crate::analysis::NativeMessageObservation::ToolResult(value) => {
                (value.source_format, value.session_id.as_str())
            }
        };
        let is_result = matches!(
            &message,
            crate::analysis::NativeMessageObservation::ToolResult(_)
        );
        let (prefix, clipped) = self.native_prefix(source_format, session_id, position, !is_result);
        let main = match &message {
            crate::analysis::NativeMessageObservation::User(value) => value.context.main_session,
            crate::analysis::NativeMessageObservation::Public(value) => value
                .context
                .as_ref()
                .is_some_and(|context| context.main_session),
            crate::analysis::NativeMessageObservation::ToolResult(value) => value
                .context
                .as_ref()
                .is_some_and(|context| context.main_session),
        };
        let mut message = crate::analysis::public_events::freeze_message(
            message,
            position,
            prefix,
            self.user_messages_complete && !clipped,
            main,
        );
        if let Some(failure) = &self.native_admission_failure {
            message.invalidate_admission(failure.reason);
        }
        message
    }

    fn native_activity(&mut self, activity: crate::analysis::NativeActivityObservation) {
        self.retain_native_evidence(crate::analysis::NativeEvidenceFact::Activity(activity));
    }

    fn native_context(
        &mut self,
        source_format: crate::analysis::SourceFormat,
        session_id: &str,
        position: crate::analysis::ToolResultPosition,
        main_session: bool,
    ) -> crate::analysis::NativeEventContext {
        let (user_messages, clipped) =
            self.native_prefix(source_format, session_id, position, true);
        crate::analysis::NativeEventContext {
            first_eligible_position: position,
            main_session,
            complete_through_event: main_session && self.user_messages_complete && !clipped,
            user_messages,
        }
    }

    fn session_selection(&mut self, selection: crate::analysis::NativeSessionSelectionObservation) {
        self.retain_native_evidence(crate::analysis::NativeEvidenceFact::SessionSelection(
            selection,
        ));
    }

    fn tool_result_gap(&mut self) {
        self.tool_result_observations_complete = false;
        self.user_messages_complete = false;
        self.native_evidence_complete = false;
    }

    fn pending_native_content(&mut self, reason: crate::analysis::PartialReason) {
        if self.turn_rows.is_some() && self.pending_native_finish.is_none() {
            self.pending_native_finish = Some(PreFinishState {
                metrics: self.metrics.snapshot(),
                evidence: EvidenceSnapshot {
                    record: self.evidence.coverage_record(),
                    resume: self.evidence.resume_state(),
                },
            });
        }
        let record = NormalizedRecord::Unusable(reason);
        self.evidence.observe(&record);
        self.metrics.record(record);
    }

    fn tool_result_admission_gap(&mut self, admission: crate::analysis::ToolResultAdmission) {
        use crate::analysis::{ToolResultAdmission, ToolResultCompleteness};
        let completeness = match admission {
            ToolResultAdmission::UnsafeIdentity => ToolResultCompleteness::UnsafeIdentity,
            ToolResultAdmission::UnsafeOrder => ToolResultCompleteness::UnsafeOrder,
            ToolResultAdmission::UnsafeOwnership => ToolResultCompleteness::UnsafeOwnership,
            ToolResultAdmission::Admitted | ToolResultAdmission::Unavailable => return,
        };
        let completeness = self
            .native_admission_failure
            .as_ref()
            .map_or(completeness, |failure| failure.reason);
        for result in &mut self.tool_result_observations {
            result.completeness = completeness;
            if let Some(context) = &mut result.context {
                context.main_session = false;
                context.complete_through_event = false;
            }
        }
        for event in &mut self.native_evidence {
            if self.native_admission_failure.is_some()
                || matches!(
                    event.fact,
                    crate::analysis::NativeEvidenceFact::Message(
                        crate::analysis::NativeMessageObservation::ToolResult(_)
                    )
                )
            {
                event.fact.invalidate_admission(completeness);
            }
        }
        self.native_evidence_bytes = self
            .native_evidence
            .iter()
            .map(|event| {
                serde_json::to_vec(event)
                    .expect("native evidence encodes")
                    .len()
            })
            .sum();
        while self.native_evidence_bytes > MAX_NATIVE_EVIDENCE_BYTES {
            let removed = self.native_evidence.remove(0);
            self.native_evidence_bytes -= serde_json::to_vec(&removed)
                .expect("native evidence encodes")
                .len();
        }
        self.tool_result_gap();
        self.tool_result_observation_sizes = self
            .tool_result_observations
            .iter()
            .map(|result| {
                serde_json::to_vec(result)
                    .expect("source result encodes")
                    .len()
            })
            .collect();
        self.tool_result_observations_bytes =
            self.tool_result_observation_sizes.iter().sum::<usize>()
                + self
                    .native_tool_result_conflicts
                    .iter()
                    .map(String::len)
                    .sum::<usize>();
    }

    fn native_admission_failure(&mut self, failure: crate::analysis::NativeAdmissionFailure) {
        if !failure.valid() {
            self.tool_result_gap();
            return;
        }
        let first = self.native_admission_failure.is_none();
        if first {
            self.native_admission_failure = Some(failure.clone());
        }
        self.tool_result_admission_gap(failure.admission());
        if first {
            self.retain_native_evidence(crate::analysis::NativeEvidenceFact::AdmissionFailure(
                failure,
            ));
        }
    }

    fn finish(&mut self, summary: SessionSummary) {
        // `Self::snapshot` needs the state from before `finish` mutates it.
        // Only a sink with turn rows can snapshot, so a sink without them
        // does not pay for the clone.
        if self.turn_rows.is_some() {
            self.pre_finish =
                Some(
                    self.pending_native_finish
                        .take()
                        .unwrap_or_else(|| PreFinishState {
                            metrics: self.metrics.snapshot(),
                            evidence: EvidenceSnapshot {
                                record: self.evidence.coverage_record(),
                                resume: self.evidence.resume_state(),
                            },
                        }),
                );
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
mod tests;
