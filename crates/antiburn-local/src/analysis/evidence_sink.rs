// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::{BTreeMap, BTreeSet};

use crate::analysis::evidence::{
    CacheEvidence, ChurnCounts, CompactionBoundary, CompactionEvidence, ContextEvidence,
    ContextSourceEvidence, CoverageReason, DepthExample, EligibilityEvidence, EvidenceCoverage,
    EvidenceSource, EvidenceValue, LoadedSource, MAX_COMPACTION_BOUNDARIES, MAX_CONTEXT_SOURCES,
    MAX_EVIDENCE_EXAMPLES, MAX_MODEL_TRANSITIONS, MAX_MODELS, MAX_SUBAGENT_CHILDREN,
    MAX_TIER_LABELS, MAX_TOOL_NAMES, MAX_UNRECOGNIZED_TYPES, ModelEvidence, ModelTokens,
    ModelTransition, OrderingObservation, ParseDiagnostics, RelationConfidence, SessionEvidence,
    SessionEvidenceIdentity, SessionProvenance, SessionTimeRange, SourceAcceptance,
    SourceCapabilities, SourceKind, SubagentChild, SubagentEvidence, SubagentExample, ToolClass,
    ToolEvidence, ToolUse, TurnCounts, cap_string, insert_diagnostic_field,
};
use crate::analysis::interface::{
    ContextSourceKind, EvidenceObservation, NormalizedRecord, RecordSink, SessionSummary,
    VisitOutcome,
};
use crate::analysis::metrics_sink::SessionMetricsAccumulator;
use crate::analysis::model::{CompactionTrigger, EventSource, NormalizedEvent, Role};
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
    last_ts_ms: Option<i64>,
    first_ts_ms: Option<i64>,
    timestamped_turns: u64,
    turns: u64,
    assistant_turns: u64,
    tool_turns: u64,
    depth_eligible_turns: u64,
    max_request_context_tokens: u64,
    depth_examples: Vec<DepthExample>,
    context_cap_exceeded: bool,
    tools: BTreeMap<String, ToolUse>,
    invoked_skills: BTreeSet<String>,
    tools_cap_exceeded: bool,
    skills: BTreeMap<String, LoadedSource>,
    mcp_servers: BTreeMap<String, LoadedSource>,
    context_sources_cap_exceeded: bool,
    models: BTreeMap<String, ModelTokens>,
    unattributed_turns: u64,
    effort_tiers: BTreeMap<String, TurnCounts>,
    fast_modes: BTreeMap<String, TurnCounts>,
    models_cap_exceeded: bool,
    subagent_spawn_count: u64,
    delegated_turns: u64,
    subagent_children: Vec<SubagentChild>,
    subagent_examples: Vec<SubagentExample>,
    subagents_cap_exceeded: bool,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    fresh_input_tokens: u64,
    model_transitions: Vec<ModelTransition>,
    active_model: Option<String>,
    previous_turn_ts: Option<i64>,
    longest_idle_gap_ms: i64,
    idle_gap_ms_total: i64,
    manual_compactions: u64,
    cache_cap_exceeded: bool,
    compaction_boundaries: Vec<CompactionBoundary>,
    compactions_cap_exceeded: bool,
    summary_observed: bool,
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
            first_ts_ms: None,
            timestamped_turns: 0,
            turns: 0,
            assistant_turns: 0,
            tool_turns: 0,
            depth_eligible_turns: 0,
            max_request_context_tokens: 0,
            depth_examples: Vec::new(),
            context_cap_exceeded: false,
            tools: BTreeMap::new(),
            invoked_skills: BTreeSet::new(),
            tools_cap_exceeded: false,
            skills: BTreeMap::new(),
            mcp_servers: BTreeMap::new(),
            context_sources_cap_exceeded: false,
            models: BTreeMap::new(),
            unattributed_turns: 0,
            effort_tiers: BTreeMap::new(),
            fast_modes: BTreeMap::new(),
            models_cap_exceeded: false,
            subagent_spawn_count: 0,
            delegated_turns: 0,
            subagent_children: Vec::new(),
            subagent_examples: Vec::new(),
            subagents_cap_exceeded: false,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            fresh_input_tokens: 0,
            model_transitions: Vec::new(),
            active_model: None,
            previous_turn_ts: None,
            longest_idle_gap_ms: 0,
            idle_gap_ms_total: 0,
            manual_compactions: 0,
            cache_cap_exceeded: false,
            compaction_boundaries: Vec::new(),
            compactions_cap_exceeded: false,
            summary_observed: false,
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
        self.turns = self.turns.saturating_add(1);
        self.assistant_turns = self
            .assistant_turns
            .saturating_add(u64::from(event.role == Role::Assistant));
        self.tool_turns = self
            .tool_turns
            .saturating_add(u64::from(event.role == Role::Tool));
        let depth = event.usage.context_tokens();
        self.depth_eligible_turns = self
            .depth_eligible_turns
            .saturating_add(u64::from(depth > 0));
        self.max_request_context_tokens = self.max_request_context_tokens.max(depth);

        if let Some(timestamp) = event.ts_ms {
            if self.last_ts_ms.is_some_and(|last| timestamp < last) {
                self.ordering = OrderingObservation::OutOfOrder;
            }
            self.last_ts_ms = Some(timestamp);
            self.first_ts_ms = Some(
                self.first_ts_ms
                    .map_or(timestamp, |first| first.min(timestamp)),
            );
            self.timestamped_turns = self.timestamped_turns.saturating_add(1);
            if depth > 0 {
                let model = event.model.as_deref().map(|model| {
                    let capped = cap_string(
                        "context.top_depth_examples.model",
                        model,
                        &mut self.diagnostics,
                    );
                    if capped.len() != model.len() {
                        self.context_cap_exceeded = true;
                    }
                    capped
                });
                self.push_depth_example(DepthExample {
                    ts_ms: timestamp,
                    depth_tokens: depth,
                    model,
                });
            }
        }

        self.observe_model(event);
        self.observe_cache_and_compaction(event);

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

    fn observe_model(&mut self, event: &NormalizedEvent) {
        let delegated = event.source == EventSource::Subagent;
        if event.role == Role::Assistant {
            if let Some(model) = event.model.as_deref() {
                let capped = cap_string("models.by_model", model, &mut self.diagnostics);
                if capped.len() != model.len() {
                    self.models_cap_exceeded = true;
                }
                if let Some(tokens) = self.models.get_mut(&capped) {
                    add_model_tokens(tokens, event);
                } else if self.models.len() == MAX_MODELS {
                    self.models_cap_exceeded = true;
                    self.note_collection_cap("models.by_model");
                } else {
                    let mut tokens = ModelTokens::default();
                    add_model_tokens(&mut tokens, event);
                    self.models.insert(capped, tokens);
                }
            } else {
                self.unattributed_turns = self.unattributed_turns.saturating_add(1);
            }
        }
        if let Some(tier) = event.thinking_mode.as_deref() {
            let (truncated, capped, diagnostic_capped) = insert_turn_count(
                &mut self.effort_tiers,
                tier,
                "models.effort_tiers",
                delegated,
                &mut self.diagnostics,
            );
            self.models_cap_exceeded |= truncated || capped;
            self.session_cap_exceeded |= diagnostic_capped;
        }
        if let Some(tier) = event.speed.as_deref() {
            let (truncated, capped, diagnostic_capped) = insert_turn_count(
                &mut self.fast_modes,
                tier,
                "models.fast_modes",
                delegated,
                &mut self.diagnostics,
            );
            self.models_cap_exceeded |= truncated || capped;
            self.session_cap_exceeded |= diagnostic_capped;
        }
    }

    fn observe_cache_and_compaction(&mut self, event: &NormalizedEvent) {
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(event.usage.cache_read_tokens);
        self.cache_creation_tokens = self
            .cache_creation_tokens
            .saturating_add(event.usage.cache_creation_tokens);
        self.fresh_input_tokens = self
            .fresh_input_tokens
            .saturating_add(event.usage.input_tokens);
        if let Some(ts_ms) = event.ts_ms {
            if let Some(previous) = self.previous_turn_ts {
                let gap = ts_ms.saturating_sub(previous).max(0);
                self.longest_idle_gap_ms = self.longest_idle_gap_ms.max(gap);
                self.idle_gap_ms_total = self.idle_gap_ms_total.saturating_add(gap);
            }
            self.previous_turn_ts = Some(ts_ms);
        }
        if let Some(model) = event.model.as_deref() {
            if let Some(previous) = self.active_model.as_deref()
                && previous != model
                && let Some(ts_ms) = event.ts_ms
            {
                let from_model = cap_string(
                    "cache.model_transitions.from_model",
                    previous,
                    &mut self.diagnostics,
                );
                let to_model = cap_string(
                    "cache.model_transitions.to_model",
                    model,
                    &mut self.diagnostics,
                );
                if from_model.len() != previous.len() || to_model.len() != model.len() {
                    self.cache_cap_exceeded = true;
                }
                if self.model_transitions.len() == MAX_MODEL_TRANSITIONS {
                    self.cache_cap_exceeded = true;
                    self.note_collection_cap("cache.model_transitions");
                } else {
                    self.model_transitions.push(ModelTransition {
                        ts_ms,
                        from_model,
                        to_model,
                    });
                }
            }
            self.active_model = Some(model.to_owned());
        }
        if event.is_compaction_boundary {
            self.manual_compactions = self.manual_compactions.saturating_add(u64::from(
                event.compaction_trigger == Some(CompactionTrigger::Manual),
            ));
            if self.compaction_boundaries.len() == MAX_COMPACTION_BOUNDARIES {
                self.compactions_cap_exceeded = true;
                self.note_collection_cap("compactions.boundaries");
            } else {
                self.compaction_boundaries.push(CompactionBoundary {
                    ts_ms: event.ts_ms.unwrap_or(0),
                    trigger: event.compaction_trigger,
                    pre_tokens: event.compaction_pre_tokens,
                    post_tokens: event.compaction_post_tokens,
                });
            }
        }
    }

    fn push_depth_example(&mut self, example: DepthExample) {
        self.depth_examples.push(example);
        self.depth_examples.sort_by(|left, right| {
            right
                .depth_tokens
                .cmp(&left.depth_tokens)
                .then_with(|| left.ts_ms.cmp(&right.ts_ms))
        });
        if self.depth_examples.len() > MAX_EVIDENCE_EXAMPLES {
            self.depth_examples.truncate(MAX_EVIDENCE_EXAMPLES);
            self.context_cap_exceeded = true;
            self.note_collection_cap("context.top_depth_examples");
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
            EvidenceObservation::DelegatedTurn { is_sidechain } => {
                self.delegated_turns = self
                    .delegated_turns
                    .saturating_add(u64::from(*is_sidechain));
            }
            EvidenceObservation::UnrecognizedType { discriminator } => {
                let original_len = discriminator.len();
                let discriminator = cap_string(
                    "diagnostics.unrecognized_types",
                    discriminator,
                    &mut self.diagnostics,
                );
                if discriminator.len() != original_len {
                    self.session_cap_exceeded = true;
                }
                if !self.diagnostics.unrecognized_types.contains(&discriminator) {
                    if self.diagnostics.unrecognized_types.len() == MAX_UNRECOGNIZED_TYPES {
                        self.session_cap_exceeded = true;
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
        let parent_model = parent_model.map(|model| {
            let capped = cap_string("subagents.parent_model", model, &mut self.diagnostics);
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
                parent_model: parent_model.clone(),
                child_model: EvidenceValue::Unsupported,
                confidence: RelationConfidence::Observed,
                provenance,
            });
        }
        if let Some(ts_ms) = ts_ms {
            if self.subagent_examples.len() == MAX_EVIDENCE_EXAMPLES {
                self.subagents_cap_exceeded = true;
                self.note_collection_cap("subagents.examples");
            } else {
                self.subagent_examples.push(SubagentExample {
                    ts_ms,
                    parent_model,
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
        let (field, map) = match kind {
            ContextSourceKind::Skill => ("context_sources.skills", &mut self.skills),
            ContextSourceKind::McpServer => ("context_sources.mcp_servers", &mut self.mcp_servers),
        };
        let capped_name = cap_string(field, name, &mut self.diagnostics);
        if capped_name.len() != name.len() {
            self.context_sources_cap_exceeded = true;
        }
        let capped_description = description.map(|value| {
            let capped = cap_string("context_sources.description", value, &mut self.diagnostics);
            if capped.len() != value.len() {
                self.context_sources_cap_exceeded = true;
            }
            capped
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
        }
    }

    /// Folds the end-of-stream facts without taking them.
    pub fn observe_summary(&mut self, summary: &SessionSummary) {
        self.capabilities.cache_write_tokens = summary.cache_write_tokens_available;
        self.summary_observed = true;
    }

    /// Attaches the source outcome after the adapter returns.
    pub fn observe_source_outcome(&mut self, outcome: VisitOutcome) {
        if matches!(outcome, VisitOutcome::AcceptedPrefix { .. }) {
            self.set_record_loss_reason(CoverageReason::PinnedPrefix);
        }
        self.source_acceptance = SourceAcceptance::from(outcome);
    }

    pub fn evidence(&self) -> SessionEvidence {
        let context = ContextEvidence {
            max_request_context_tokens: self.max_request_context_tokens,
            top_depth_examples: self.depth_examples.clone(),
        };
        let time_range = SessionTimeRange {
            first_ts_ms: self.first_ts_ms.unwrap_or(0),
            last_ts_ms: self.last_ts_ms.unwrap_or(0),
            timestamped_turns: self.timestamped_turns,
        };
        let eligibility = EligibilityEvidence {
            turns: self.turns,
            assistant_turns: self.assistant_turns,
            tool_turns: self.tool_turns,
            depth_eligible_turns: self.depth_eligible_turns,
        };
        let tools = self.classified_tools();
        let context_sources = self.context_sources();
        let models = ModelEvidence {
            by_model: self.models.clone(),
            unattributed_turns: self.unattributed_turns,
            effort_tiers: self.effort_tiers.clone(),
            fast_modes: self.fast_modes.clone(),
            service_tiers: EvidenceValue::Unsupported,
        };
        let subagents = SubagentEvidence {
            spawn_count: self.subagent_spawn_count,
            delegated_turns: self.delegated_turns,
            children: self.subagent_children.clone(),
            examples: self.subagent_examples.clone(),
        };
        let cache = CacheEvidence {
            cache_read_tokens: self.cache_read_tokens,
            cache_creation_tokens: self.cache_creation_tokens,
            fresh_input_tokens: self.fresh_input_tokens,
            model_transitions: self.model_transitions.clone(),
            longest_idle_gap_ms: self.longest_idle_gap_ms,
            idle_gap_ms_total: self.idle_gap_ms_total,
            user_controlled_churn: ChurnCounts {
                manual_compactions: self.manual_compactions,
            },
            previous_turn: EvidenceValue::Unsupported,
            provider_eviction: EvidenceValue::Unsupported,
        };
        let compactions = CompactionEvidence {
            boundaries: self.compaction_boundaries.clone(),
        };
        let coverage_reason = self.record_loss_reason.or(self
            .session_cap_exceeded
            .then_some(CoverageReason::CapExceeded));
        let coverage =
            coverage_reason.map_or(EvidenceCoverage::Complete, EvidenceCoverage::Partial);

        SessionEvidence {
            schema_revision: EVIDENCE_SCHEMA_REVISION,
            identity: self.identity.clone(),
            context: self.supported_value(
                context,
                self.capabilities.request_context_tokens,
                self.context_cap_exceeded,
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
            diagnostics: self.diagnostics.clone(),
            time_range: self.supported_value(
                time_range,
                self.capabilities.timestamps_and_order,
                false,
            ),
            eligibility: self.supported_value(eligibility, true, false),
            tools: self.supported_value(
                ToolEvidence { by_name: tools },
                self.capabilities.tool_invocations,
                self.tools_cap_exceeded,
            ),
            context_sources: self.supported_value(
                context_sources,
                self.capabilities.skill_mcp_attribution,
                self.context_sources_cap_exceeded,
            ),
            models: if !self.capabilities.model_identity || !self.capabilities.token_classes {
                EvidenceValue::Unsupported
            } else if let Some(reason) = self.record_loss_reason {
                EvidenceValue::Partial {
                    observed: models,
                    reason,
                }
            } else if self.models_cap_exceeded {
                EvidenceValue::Partial {
                    observed: models,
                    reason: CoverageReason::CapExceeded,
                }
            } else if self.unattributed_turns > 0 {
                EvidenceValue::Partial {
                    observed: models,
                    reason: CoverageReason::AttributionIncomplete,
                }
            } else {
                EvidenceValue::Complete(models)
            },
            subagents: self.supported_value(
                subagents,
                self.capabilities.subagent_relationships,
                self.subagents_cap_exceeded,
            ),
            cache: self.supported_value(cache, true, self.cache_cap_exceeded),
            compactions: self.supported_value(
                compactions,
                self.capabilities.compaction_boundaries,
                self.compactions_cap_exceeded,
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

    fn supported_value<T>(
        &self,
        observed: T,
        supported: bool,
        cap_exceeded: bool,
    ) -> EvidenceValue<T> {
        if !supported {
            EvidenceValue::Unsupported
        } else if let Some(reason) = self.record_loss_reason {
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
        if self.record_loss_reason.is_none() {
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

fn add_model_tokens(tokens: &mut ModelTokens, event: &NormalizedEvent) {
    tokens.input = tokens.input.saturating_add(event.usage.input_tokens);
    tokens.output = tokens.output.saturating_add(event.usage.output_tokens);
    tokens.cache_read = tokens
        .cache_read
        .saturating_add(event.usage.cache_read_tokens);
    tokens.cache_creation = tokens
        .cache_creation
        .saturating_add(event.usage.cache_creation_tokens);
    tokens.turns = tokens.turns.saturating_add(1);
    if let Some(ts_ms) = event.ts_ms {
        if tokens.turns == 1 || tokens.first_ts_ms == 0 {
            tokens.first_ts_ms = ts_ms;
        } else {
            tokens.first_ts_ms = tokens.first_ts_ms.min(ts_ms);
        }
        tokens.last_ts_ms = tokens.last_ts_ms.max(ts_ms);
    }
}

fn insert_turn_count(
    map: &mut BTreeMap<String, TurnCounts>,
    value: &str,
    field: &'static str,
    delegated: bool,
    diagnostics: &mut ParseDiagnostics,
) -> (bool, bool, bool) {
    let capped_value = cap_string(field, value, diagnostics);
    let truncated = capped_value.len() != value.len();
    if let Some(counts) = map.get_mut(&capped_value) {
        increment_turn_count(counts, delegated);
        return (truncated, false, false);
    }
    if map.len() == MAX_TIER_LABELS {
        let diagnostic_capped = insert_diagnostic_field(&mut diagnostics.capped_collections, field);
        return (truncated, true, diagnostic_capped);
    }
    let mut counts = TurnCounts::default();
    increment_turn_count(&mut counts, delegated);
    map.insert(capped_value, counts);
    (truncated, false, false)
}

fn increment_turn_count(counts: &mut TurnCounts, delegated: bool) {
    if delegated {
        counts.delegated = counts.delegated.saturating_add(1);
    } else {
        counts.main_loop = counts.main_loop.saturating_add(1);
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
}

impl CompositeSink {
    pub fn new(metrics: SessionMetricsAccumulator, evidence: SessionEvidenceAccumulator) -> Self {
        Self { metrics, evidence }
    }

    pub fn metrics(&self) -> Option<SessionMetrics> {
        self.evidence.can_publish().then(|| self.metrics.metrics())
    }

    pub fn evidence(&self) -> Option<SessionEvidence> {
        self.evidence
            .can_publish()
            .then(|| self.evidence.evidence())
    }

    pub fn observe_source_outcome(&mut self, outcome: VisitOutcome) {
        self.evidence.observe_source_outcome(outcome);
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
        self.metrics.record(record);
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.evidence.observe_summary(&summary);
        self.metrics.finish(summary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::model::{EventSource, Usage};
    use crate::analysis::{PartialReason, RawSource, VendorAdapter};

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

    fn metric_record(context_tokens: u64, ts_ms: Option<i64>) -> NormalizedRecord {
        let mut event = NormalizedEvent::new(Role::Assistant);
        event.source = EventSource::Parent;
        event.ts_ms = ts_ms;
        event.usage = Usage {
            input_tokens: context_tokens,
            ..Usage::default()
        };
        NormalizedRecord::MetricsEvent(Box::new(event))
    }

    #[test]
    fn context_depth_is_the_maximum_across_events() {
        let mut accumulator = accumulator(true);
        accumulator.record(metric_record(5, Some(1)));
        accumulator.record(metric_record(12, Some(2)));
        accumulator.finish(SessionSummary::default());

        let EvidenceValue::Complete(context) = accumulator.evidence().context else {
            panic!("context must be complete");
        };
        assert_eq!(context.max_request_context_tokens, 12);
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
        let mut composite = CompositeSink::new(
            SessionMetricsAccumulator::new("claude", "attachment"),
            SessionEvidenceAccumulator::new(EvidenceSource {
                agent: "claude".to_owned(),
                session_id: "attachment".to_owned(),
                kind: SourceKind::Jsonl,
                capabilities: SourceCapabilities::claude(),
            }),
        );
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
    fn an_unmodelled_type_still_degrades_and_records_its_discriminator() {
        let input = crate::analysis::SessionInput {
            agent: "claude".to_owned(),
            session_id: "unknown".to_owned(),
            source: RawSource::Jsonl(r#"{"type":"telemetry_ping","payload":"private"}"#.to_owned()),
        };
        let mut composite = CompositeSink::new(
            SessionMetricsAccumulator::new("claude", "unknown"),
            SessionEvidenceAccumulator::new(EvidenceSource {
                agent: "claude".to_owned(),
                session_id: "unknown".to_owned(),
                kind: SourceKind::Jsonl,
                capabilities: SourceCapabilities::claude(),
            }),
        );
        let outcome = crate::analysis::ClaudeAdapter
            .visit(&input, &mut composite)
            .expect("unknown record must be skipped");
        composite.observe_source_outcome(outcome);
        let evidence = composite.evidence().expect("evidence");
        assert_eq!(
            evidence.coverage,
            EvidenceCoverage::Partial(CoverageReason::UnrecognizedRecordType)
        );
        assert_eq!(
            evidence.diagnostics.unrecognized_types,
            BTreeSet::from(["telemetry_ping".to_owned()])
        );
    }

    #[test]
    fn record_loss_reason_outranks_session_cap_reason() {
        let source = EvidenceSource {
            agent: "a".repeat(512),
            session_id: "s1".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        };
        let mut accumulator = SessionEvidenceAccumulator::new(source);
        accumulator.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
        assert_eq!(
            accumulator.evidence().coverage,
            EvidenceCoverage::Partial(CoverageReason::MalformedRecord)
        );
    }
}
