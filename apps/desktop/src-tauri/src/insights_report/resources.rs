use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use antiburn_local::analysis::{
    EvidenceValue, InitialContextBreakdown, SessionEvidence, SourceOrigin, ToolClass,
};
use antiburn_local::insights::{
    DetectorId, EfficiencyReport, SessionTokenBurnEvidence, fallback_token_burn_basis_points,
};
use antiburn_local::model::AgentKind;

use crate::agent_config::{
    AdvisoryResource, EnabledState, InventoryIssue, ResourceInventory, ResourceKind, ResourceScope,
};

const MAX_OBSERVED_USES: usize = 4_096;
const MAX_RESOURCE_CANDIDATES_PER_DETECTOR: usize = 512;
const MAX_RESOURCE_TARGETS_PER_DETECTOR: usize = 512;
const MAX_SUPPORTING_SESSIONS_PER_TARGET: usize = 3;
const MAX_RESOURCE_TURN_GROUPS: usize = 4_096;

// Captured from OpenCode 1.2.15 with gpt-5.6-sol tool definitions.
const OPENCODE_BUILT_IN_TOKENS: &[(&str, u64)] = &[
    ("bash", 244),
    ("edit", 255),
    ("glob", 157),
    ("grep", 211),
    ("list", 129),
    ("read", 195),
    ("task", 486),
    ("todowrite", 208),
    ("webfetch", 249),
    ("write", 174),
];

// Captured from Pi 0.52.12 with the default Anthropic tool definitions.
const PI_BUILT_IN_TOKENS: &[(&str, u64)] =
    &[("bash", 181), ("edit", 185), ("read", 155), ("write", 135)];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ResourceAssessmentScope {
    Global,
    Project(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ResourceTargetKey {
    agent: AgentKind,
    kind: ResourceKind,
    normalized_name: String,
    scope: ResourceAssessmentScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceSupportingSession {
    pub environment_key: String,
    pub agent: String,
    pub session_id: String,
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnusedResourceTarget {
    pub agent: AgentKind,
    pub kind: ResourceKind,
    pub canonical_name: String,
    pub scope: ResourceAssessmentScope,
    pub observations: u64,
    pub indexed: bool,
    pub replicated_tokens: Option<u128>,
    pub estimated_token_burn_basis_points: Option<u16>,
    pub supporting_sessions: Vec<ResourceSupportingSession>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResourceDetectorAssessment {
    pub candidate_count: u64,
    pub used_count: u64,
    pub unused_count: u64,
    pub clean: bool,
    pub unavailable: bool,
    pub truncated: bool,
    pub replicated_tokens: Option<u128>,
    replicated_tokens_by_session: Option<BTreeMap<usize, u128>>,
    pub estimated_token_burn_basis_points: Option<u16>,
    pub finding_agents: BTreeSet<String>,
    pub clean_agents: BTreeSet<String>,
    pub targets: Vec<UnusedResourceTarget>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResourceAssessment {
    detectors: BTreeMap<DetectorId, ResourceDetectorAssessment>,
}

impl ResourceAssessment {
    pub(crate) fn detector(&self, detector: DetectorId) -> Option<&ResourceDetectorAssessment> {
        self.detectors.get(&detector)
    }

    pub(crate) fn measured_finding_tokens_by_session(&self) -> Option<Vec<(usize, u128)>> {
        let mut measured = false;
        let mut totals = BTreeMap::<usize, u128>::new();
        for assessment in self.detectors.values() {
            if assessment.unused_count == 0 {
                continue;
            }
            let Some(by_session) = &assessment.replicated_tokens_by_session else {
                continue;
            };
            measured = true;
            for (session, tokens) in by_session {
                let total = totals.entry(*session).or_default();
                *total = total.checked_add(*tokens)?;
            }
        }
        measured.then(|| totals.into_iter().collect())
    }
}

#[derive(Debug, Clone)]
struct CandidateState {
    resource: AdvisoryResource,
    scope: ResourceAssessmentScope,
    observations: u64,
    supporting_sessions: Vec<ResourceSupportingSession>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ResourceTurnKey {
    agent: AgentKind,
    project_root: Option<PathBuf>,
    session_index: usize,
    context_tokens: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ObservedScope {
    Global,
    Project(PathBuf),
    Context(Option<PathBuf>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ObservedIdentityKind {
    ExactResource,
    ToolName,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ObservedUseKey {
    agent: AgentKind,
    kind: ResourceKind,
    name: String,
    scope: ObservedScope,
    identity_kind: ObservedIdentityKind,
}

#[derive(Debug, Clone)]
struct SessionContext<'a> {
    environment_key: &'a str,
    agent: AgentKind,
    session_id: &'a str,
    project_root: Option<&'a Path>,
    observed_at_ms: i64,
}

#[derive(Default)]
pub(crate) struct ResourceAssessmentBuilder {
    candidates: BTreeMap<ResourceTargetKey, CandidateState>,
    uses: BTreeMap<ObservedUseKey, Vec<ResourceSupportingSession>>,
    session_samples: BTreeMap<(AgentKind, Option<PathBuf>), Vec<ResourceSupportingSession>>,
    limited: BTreeSet<(AgentKind, ResourceKind)>,
    scanned: BTreeSet<(AgentKind, ResourceKind)>,
    cohort_agents: BTreeSet<AgentKind>,
    use_cap_exceeded: bool,
    estimate_cap_exceeded: bool,
    candidate_cap_exceeded: BTreeSet<ResourceKind>,
    turns: BTreeMap<ResourceTurnKey, u64>,
    measured_resources: BTreeMap<ObservedUseKey, BTreeMap<usize, u128>>,
    measured_resource_session_entries: usize,
}

impl ResourceAssessmentBuilder {
    pub(crate) fn mark_window_incomplete(&mut self) {
        for agent in resource_assessment_agents() {
            for kind in resource_kinds() {
                self.limited.insert((agent, kind));
            }
        }
    }

    pub(crate) fn mark_scan_failed(&mut self, agent: AgentKind) {
        for kind in resource_kinds() {
            self.limited.insert((agent, kind));
        }
    }

    pub(crate) fn observe_inventory(
        &mut self,
        inventory: ResourceInventory,
        project_root: Option<&Path>,
    ) {
        for kind in resource_kinds() {
            self.scanned.insert((inventory.agent, kind));
        }
        for issue in inventory.issues {
            self.observe_inventory_issue(inventory.agent, issue);
        }
        for resource in inventory.resources {
            if resource.enabled == EnabledState::Disabled {
                continue;
            }
            let scope = match resource.scope {
                ResourceScope::Global => ResourceAssessmentScope::Global,
                ResourceScope::Project => {
                    let Some(root) = project_root else {
                        self.limited.insert((resource.agent, resource.kind));
                        continue;
                    };
                    ResourceAssessmentScope::Project(root.to_owned())
                }
                ResourceScope::Unknown => {
                    self.limited.insert((resource.agent, resource.kind));
                    continue;
                }
            };
            self.add_candidate(resource, scope, None);
        }
    }

    pub(crate) fn observe_session(
        &mut self,
        environment_key: &str,
        agent: AgentKind,
        session_id: &str,
        project_root: Option<&Path>,
        evidence: &SessionEvidence,
        initial_context: Option<&InitialContextBreakdown>,
    ) {
        self.cohort_agents.insert(agent);
        let observed_at_ms = match &evidence.time_range {
            EvidenceValue::Complete(range)
            | EvidenceValue::Partial {
                observed: range, ..
            } => range.last_ts_ms,
            EvidenceValue::Unsupported => 0,
        };
        let context = SessionContext {
            environment_key,
            agent,
            session_id,
            project_root,
            observed_at_ms,
        };
        let sample = session_sample(&context);
        merge_samples(
            self.session_samples.entry((agent, None)).or_default(),
            [sample.clone()],
        );
        if let Some(project_root) = project_root {
            merge_samples(
                self.session_samples
                    .entry((agent, Some(project_root.to_owned())))
                    .or_default(),
                [sample],
            );
        }
        for (key, state) in &mut self.candidates {
            let matching_scope = match (&key.scope, project_root) {
                (ResourceAssessmentScope::Global, _) => true,
                (ResourceAssessmentScope::Project(root), Some(project_root)) => {
                    root == project_root
                }
                (ResourceAssessmentScope::Project(_), None) => false,
            };
            if key.agent != agent || !matching_scope {
                continue;
            }
            merge_samples(&mut state.supporting_sessions, [session_sample(&context)]);
        }

        self.observe_tool_uses(&context, evidence);
        self.observe_context_sources(&context, evidence);
        if let Some(initial_context) = initial_context {
            self.observe_initial_context(&context, initial_context);
        }
    }

    /// Records direct use without creating a historical finding or target.
    pub(crate) fn observe_positive_uses(
        &mut self,
        environment_key: &str,
        agent: AgentKind,
        session_id: &str,
        project_root: Option<&Path>,
        evidence: &SessionEvidence,
        initial_context: Option<&InitialContextBreakdown>,
    ) {
        let observed_at_ms = match &evidence.time_range {
            EvidenceValue::Complete(range)
            | EvidenceValue::Partial {
                observed: range, ..
            } => range.last_ts_ms,
            EvidenceValue::Unsupported => 0,
        };
        let context = SessionContext {
            environment_key,
            agent,
            session_id,
            project_root,
            observed_at_ms,
        };
        self.observe_tool_uses(&context, evidence);
        self.observe_context_source_uses(&context, evidence);
        if let Some(initial_context) = initial_context {
            self.observe_initial_context(&context, initial_context);
        }
    }

    pub(crate) fn observe_turn(
        &mut self,
        agent: AgentKind,
        project_root: Option<&Path>,
        session_index: usize,
        context_tokens: u128,
    ) {
        let key = ResourceTurnKey {
            agent,
            project_root: project_root.map(Path::to_owned),
            session_index,
            context_tokens,
        };
        if !self.turns.contains_key(&key) && self.turns.len() == MAX_RESOURCE_TURN_GROUPS {
            self.estimate_cap_exceeded = true;
            return;
        }
        self.turns
            .entry(key)
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(1);
    }

    pub(crate) fn observe_resource_estimates(
        &mut self,
        agent: AgentKind,
        project_root: Option<&Path>,
        session_index: usize,
        evidence: &SessionTokenBurnEvidence,
    ) {
        for (kind, sources) in [
            (ResourceKind::McpServer, evidence.mcp_sources.as_ref()),
            (
                ResourceKind::BuiltInTool,
                evidence.built_in_tool_sources.as_ref(),
            ),
        ] {
            let Some(sources) = sources else {
                continue;
            };
            for source in sources {
                if source.replicated_tokens == 0 || source.invoked {
                    continue;
                }
                let Some(scope) = measured_source_scope(&source.scope, project_root) else {
                    continue;
                };
                let key = ObservedUseKey {
                    agent,
                    kind,
                    name: source.name.clone(),
                    scope,
                    identity_kind: ObservedIdentityKind::ExactResource,
                };
                if !self.measured_resources.contains_key(&key)
                    && self.measured_resources.len() == MAX_OBSERVED_USES
                {
                    self.estimate_cap_exceeded = true;
                    continue;
                }
                let sessions = self.measured_resources.entry(key).or_default();
                let new_session = !sessions.contains_key(&session_index);
                if new_session && self.measured_resource_session_entries == MAX_RESOURCE_TURN_GROUPS
                {
                    self.estimate_cap_exceeded = true;
                    continue;
                }
                if new_session {
                    self.measured_resource_session_entries += 1;
                }
                let entry = sessions.entry(session_index).or_default();
                let Some(total) = entry.checked_add(source.replicated_tokens) else {
                    self.estimate_cap_exceeded = true;
                    continue;
                };
                *entry = total;
            }
        }
    }

    pub(crate) fn finish(mut self, report: &EfficiencyReport) -> ResourceAssessment {
        let mut used = BTreeSet::new();
        let mut ambiguous = BTreeSet::new();
        for (key, samples) in &self.uses {
            let matching = self.matching_candidates(key);
            if matching.len() == 1 {
                let candidate = matching.into_iter().next().expect("one matching candidate");
                used.insert(candidate.clone());
                if let Some(state) = self.candidates.get_mut(&candidate) {
                    merge_samples(&mut state.supporting_sessions, samples.iter().cloned());
                }
            } else if matching.len() > 1 {
                for candidate in matching {
                    if key.kind == ResourceKind::McpServer
                        && matches!(key.agent, AgentKind::OpenCode | AgentKind::Pi)
                    {
                        ambiguous.insert(candidate.clone());
                    }
                    self.limited.insert((candidate.agent, candidate.kind));
                }
            }
        }

        let mut result = ResourceAssessment::default();
        for kind in resource_kinds() {
            let detector = detector_for_kind(kind);
            let candidates = self
                .candidates
                .iter()
                .filter(|(key, _)| key.kind == kind)
                .collect::<Vec<_>>();
            let mut assessment = ResourceDetectorAssessment {
                candidate_count: candidates.len() as u64,
                used_count: candidates
                    .iter()
                    .filter(|(key, _)| used.contains(*key))
                    .count() as u64,
                unused_count: candidates
                    .iter()
                    .filter(|(key, _)| !used.contains(*key) && !ambiguous.contains(*key))
                    .count() as u64,
                ..ResourceDetectorAssessment::default()
            };
            let mut missing_supporting_session = false;
            let mut replicated_tokens_by_session = Some(BTreeMap::<usize, u128>::new());
            assessment.truncated = self.candidate_cap_exceeded.contains(&kind);
            for (key, state) in candidates {
                if used.contains(key) || ambiguous.contains(key) {
                    continue;
                }
                if state.supporting_sessions.is_empty() {
                    missing_supporting_session = true;
                    continue;
                }
                if assessment.targets.len() == MAX_RESOURCE_TARGETS_PER_DETECTOR {
                    assessment.truncated = true;
                    continue;
                }
                assessment
                    .finding_agents
                    .insert(key.agent.slug().to_owned());
                let target_tokens_by_session = (!self.estimate_cap_exceeded)
                    .then(|| self.replicated_tokens_by_session(key, state))
                    .flatten();
                let replicated_tokens = target_tokens_by_session.as_ref().and_then(|tokens| {
                    tokens.values().copied().try_fold(0_u128, u128::checked_add)
                });
                if let (Some(assessment_tokens), Some(target_tokens)) =
                    (&mut replicated_tokens_by_session, target_tokens_by_session)
                {
                    for (session, tokens) in target_tokens {
                        let entry = assessment_tokens.entry(session).or_default();
                        let Some(total) = entry.checked_add(tokens) else {
                            replicated_tokens_by_session = None;
                            break;
                        };
                        *entry = total;
                    }
                } else {
                    replicated_tokens_by_session = None;
                }
                assessment.targets.push(UnusedResourceTarget {
                    agent: key.agent,
                    kind,
                    canonical_name: state.resource.canonical_name.clone(),
                    scope: state.scope.clone(),
                    observations: state.observations,
                    indexed: state
                        .resource
                        .provenance
                        .contains(&crate::agent_config::ResourceProvenance::IndexedSession),
                    replicated_tokens,
                    estimated_token_burn_basis_points: replicated_tokens.and_then(|tokens| {
                        report.estimated_token_burn_for_attributed_tokens(tokens)
                    }),
                    supporting_sessions: state.supporting_sessions.clone(),
                });
            }
            assessment.truncated |= missing_supporting_session;
            assessment.unused_count = assessment.targets.len() as u64;
            assessment.replicated_tokens_by_session = (!assessment.truncated)
                .then_some(replicated_tokens_by_session)
                .flatten();
            assessment.replicated_tokens = assessment
                .replicated_tokens_by_session
                .as_ref()
                .and_then(|tokens| tokens.values().copied().try_fold(0_u128, u128::checked_add));
            assessment.estimated_token_burn_basis_points = if assessment.unused_count > 0 {
                assessment
                    .replicated_tokens
                    .and_then(|tokens| report.estimated_token_burn_for_attributed_tokens(tokens))
                    .or_else(|| {
                        fallback_token_burn_basis_points(
                            detector,
                            assessment.unused_count,
                            report.assessed_sessions,
                        )
                    })
            } else {
                None
            };
            let relevant_agents = resource_assessment_agents()
                .into_iter()
                .filter(|agent| {
                    self.cohort_agents.contains(agent)
                        || self
                            .candidates
                            .keys()
                            .any(|candidate| candidate.agent == *agent && candidate.kind == kind)
                })
                .collect::<Vec<_>>();
            let complete = !self.use_cap_exceeded
                && !assessment.truncated
                && !relevant_agents.is_empty()
                && relevant_agents.iter().all(|agent| {
                    self.scanned.contains(&(*agent, kind))
                        && !self.limited.contains(&(*agent, kind))
                });
            assessment.clean = assessment.targets.is_empty() && complete;
            assessment.unavailable = assessment.targets.is_empty() && !assessment.clean;
            if assessment.clean {
                assessment.clean_agents = relevant_agents
                    .into_iter()
                    .map(|agent| agent.slug().to_owned())
                    .collect();
            }
            result.detectors.insert(detector, assessment);
        }
        result
    }

    fn observe_inventory_issue(&mut self, agent: AgentKind, issue: InventoryIssue) {
        match issue.kind {
            Some(kind) => {
                self.limited.insert((agent, kind));
            }
            None => {
                for kind in resource_kinds() {
                    self.limited.insert((agent, kind));
                }
            }
        }
    }

    fn add_candidate(
        &mut self,
        resource: AdvisoryResource,
        scope: ResourceAssessmentScope,
        sample: Option<ResourceSupportingSession>,
    ) {
        if resource.kind == ResourceKind::BuiltInTool
            && !antiburn_local::analysis::tool_catalog::optional_built_in_tool(
                resource.agent.slug(),
                &resource.canonical_name,
            )
        {
            return;
        }
        let key = ResourceTargetKey {
            agent: resource.agent,
            kind: resource.kind,
            normalized_name: normalize_name(&resource.canonical_name),
            scope: scope.clone(),
        };
        if !self.candidates.contains_key(&key)
            && self
                .candidates
                .keys()
                .filter(|candidate| candidate.kind == resource.kind)
                .count()
                == MAX_RESOURCE_CANDIDATES_PER_DETECTOR
        {
            self.candidate_cap_exceeded.insert(resource.kind);
            self.limited.insert((resource.agent, resource.kind));
            return;
        }
        if self
            .candidates
            .get(&key)
            .is_some_and(|state| state.resource.canonical_name != resource.canonical_name)
        {
            self.limited.insert((resource.agent, resource.kind));
        }
        let state = self
            .candidates
            .entry(key)
            .or_insert_with(|| CandidateState {
                resource: resource.clone(),
                scope,
                observations: 0,
                supporting_sessions: Vec::new(),
            });
        let scope_samples = match &state.scope {
            ResourceAssessmentScope::Global => self.session_samples.get(&(resource.agent, None)),
            ResourceAssessmentScope::Project(root) => self
                .session_samples
                .get(&(resource.agent, Some(root.clone()))),
        };
        if let Some(samples) = scope_samples {
            merge_samples(&mut state.supporting_sessions, samples.iter().cloned());
        }
        for provenance in &resource.provenance {
            if !state.resource.provenance.contains(provenance) {
                state.resource.provenance.push(*provenance);
            }
        }
        state.resource.provenance.sort_unstable();
        if state.resource.definition_tokens.is_none() {
            state.resource.definition_tokens = resource.definition_tokens;
        }
        state.observations = state.observations.saturating_add(1);
        if let Some(sample) = sample {
            merge_samples(&mut state.supporting_sessions, [sample]);
        }
    }

    fn observe_tool_uses(&mut self, context: &SessionContext<'_>, evidence: &SessionEvidence) {
        let tools = match &evidence.tools {
            EvidenceValue::Unsupported => {
                for kind in resource_kinds() {
                    self.limited.insert((context.agent, kind));
                }
                return;
            }
            EvidenceValue::Partial { observed, .. } => {
                for kind in resource_kinds() {
                    self.limited.insert((context.agent, kind));
                }
                observed
            }
            EvidenceValue::Complete(observed) => observed,
        };
        for (name, tool) in &tools.by_name {
            if tool.calls == 0 {
                continue;
            }
            match tool.class {
                ToolClass::Skill => self.add_use(
                    context,
                    ResourceKind::Skill,
                    name,
                    ObservedScope::Context(context.project_root.map(Path::to_owned)),
                    ObservedIdentityKind::ToolName,
                ),
                ToolClass::Mcp => self.add_use(
                    context,
                    ResourceKind::McpServer,
                    name,
                    ObservedScope::Context(context.project_root.map(Path::to_owned)),
                    ObservedIdentityKind::ToolName,
                ),
                ToolClass::Unclassified => {
                    self.add_use(
                        context,
                        ResourceKind::BuiltInTool,
                        name,
                        ObservedScope::Context(context.project_root.map(Path::to_owned)),
                        ObservedIdentityKind::ToolName,
                    );
                    if matches!(context.agent, AgentKind::OpenCode | AgentKind::Pi) {
                        self.add_use(
                            context,
                            ResourceKind::McpServer,
                            name,
                            ObservedScope::Context(context.project_root.map(Path::to_owned)),
                            ObservedIdentityKind::ToolName,
                        );
                    }
                }
            }
        }
    }

    fn observe_context_sources(
        &mut self,
        context: &SessionContext<'_>,
        evidence: &SessionEvidence,
    ) {
        let sources = match &evidence.context_sources {
            EvidenceValue::Unsupported => {
                for kind in resource_kinds() {
                    self.limited.insert((context.agent, kind));
                }
                return;
            }
            EvidenceValue::Partial { observed, .. } | EvidenceValue::Complete(observed) => observed,
        };
        self.observe_nested_coverage(context.agent, ResourceKind::Skill, &sources.skill_coverage);
        self.observe_nested_coverage(
            context.agent,
            ResourceKind::McpServer,
            &sources.mcp_coverage,
        );
        for (kind, values) in [
            (ResourceKind::Skill, &sources.skills),
            (ResourceKind::McpServer, &sources.mcp_servers),
        ] {
            for (name, source) in values {
                let Some(scope) = source_scope(&source.origin, context.project_root) else {
                    self.limited.insert((context.agent, kind));
                    continue;
                };
                let sample = session_sample(context);
                self.add_candidate(
                    AdvisoryResource {
                        agent: context.agent,
                        kind,
                        canonical_name: name.clone(),
                        enabled: EnabledState::Unknown,
                        scope: resource_scope(&scope),
                        provenance: vec![crate::agent_config::ResourceProvenance::IndexedSession],
                        definition_tokens: None,
                    },
                    assessment_scope(&scope),
                    Some(sample),
                );
                if source.invoked {
                    self.add_use(
                        context,
                        kind,
                        name,
                        scope,
                        ObservedIdentityKind::ExactResource,
                    );
                }
            }
        }
        let definitions = match &sources.tool_definitions {
            EvidenceValue::Unsupported => {
                self.limited
                    .insert((context.agent, ResourceKind::BuiltInTool));
                return;
            }
            EvidenceValue::Partial { observed, .. } => {
                self.limited
                    .insert((context.agent, ResourceKind::BuiltInTool));
                observed
            }
            EvidenceValue::Complete(observed) => observed,
        };
        for (name, definition) in definitions {
            if definition.tokens == 0 || definition.deferred || is_situational(context.agent, name)
            {
                continue;
            }
            self.add_candidate(
                AdvisoryResource {
                    agent: context.agent,
                    kind: ResourceKind::BuiltInTool,
                    canonical_name: name.clone(),
                    enabled: EnabledState::Unknown,
                    scope: ResourceScope::Global,
                    provenance: vec![crate::agent_config::ResourceProvenance::IndexedSession],
                    definition_tokens: Some(u64::from(definition.tokens)),
                },
                ResourceAssessmentScope::Global,
                Some(session_sample(context)),
            );
            if definition.invoked {
                self.add_use(
                    context,
                    ResourceKind::BuiltInTool,
                    name,
                    ObservedScope::Global,
                    ObservedIdentityKind::ExactResource,
                );
            }
        }
    }

    fn observe_context_source_uses(
        &mut self,
        context: &SessionContext<'_>,
        evidence: &SessionEvidence,
    ) {
        let sources = match &evidence.context_sources {
            EvidenceValue::Partial { observed, .. } | EvidenceValue::Complete(observed) => observed,
            EvidenceValue::Unsupported => return,
        };
        for (kind, values) in [
            (ResourceKind::Skill, &sources.skills),
            (ResourceKind::McpServer, &sources.mcp_servers),
        ] {
            for (name, source) in values {
                if !source.invoked {
                    continue;
                }
                let Some(scope) = source_scope(&source.origin, context.project_root) else {
                    continue;
                };
                self.add_use(
                    context,
                    kind,
                    name,
                    scope,
                    ObservedIdentityKind::ExactResource,
                );
            }
        }
        let definitions = match &sources.tool_definitions {
            EvidenceValue::Partial { observed, .. } | EvidenceValue::Complete(observed) => observed,
            EvidenceValue::Unsupported => return,
        };
        for (name, definition) in definitions {
            if definition.invoked {
                self.add_use(
                    context,
                    ResourceKind::BuiltInTool,
                    name,
                    ObservedScope::Global,
                    ObservedIdentityKind::ExactResource,
                );
            }
        }
    }

    fn observe_initial_context(
        &mut self,
        context: &SessionContext<'_>,
        initial_context: &InitialContextBreakdown,
    ) {
        for source in &initial_context.sources {
            let Some(name) = source.source_name.as_deref() else {
                continue;
            };
            if matches!(
                name,
                "Other skills" | "Other MCP servers" | "Other built-in tools"
            ) {
                let kind = match source.source.as_str() {
                    "skill_instructions" => Some(ResourceKind::Skill),
                    "mcp_instructions" => Some(ResourceKind::McpServer),
                    "builtin_tool" => Some(ResourceKind::BuiltInTool),
                    _ => None,
                };
                if let Some(kind) = kind {
                    self.limited.insert((context.agent, kind));
                }
                continue;
            }
            if source.use_count == 0 {
                continue;
            }
            let kind = match source.source.as_str() {
                "skill_instructions" => ResourceKind::Skill,
                "mcp_instructions" => ResourceKind::McpServer,
                "builtin_tool" if !source.deferred => ResourceKind::BuiltInTool,
                _ => continue,
            };
            let scope = match source.origin {
                SourceOrigin::Bundled | SourceOrigin::User => ObservedScope::Global,
                SourceOrigin::Project => context.project_root.map_or_else(
                    || ObservedScope::Context(None),
                    |root| ObservedScope::Project(root.to_owned()),
                ),
                SourceOrigin::Plugin | SourceOrigin::Unknown => {
                    ObservedScope::Context(context.project_root.map(Path::to_owned))
                }
            };
            self.add_use(
                context,
                kind,
                name,
                scope,
                ObservedIdentityKind::ExactResource,
            );
        }
    }

    fn observe_nested_coverage(
        &mut self,
        agent: AgentKind,
        kind: ResourceKind,
        coverage: &EvidenceValue<()>,
    ) {
        if !matches!(coverage, EvidenceValue::Complete(())) {
            self.limited.insert((agent, kind));
        }
    }

    fn add_use(
        &mut self,
        context: &SessionContext<'_>,
        kind: ResourceKind,
        name: &str,
        scope: ObservedScope,
        identity_kind: ObservedIdentityKind,
    ) {
        if name.trim().is_empty() {
            return;
        }
        let key = ObservedUseKey {
            agent: context.agent,
            kind,
            name: name.to_owned(),
            scope,
            identity_kind,
        };
        if !self.uses.contains_key(&key) && self.uses.len() == MAX_OBSERVED_USES {
            self.use_cap_exceeded = true;
            self.limited.insert((context.agent, kind));
            return;
        }
        let samples = self.uses.entry(key).or_default();
        merge_samples(samples, [session_sample(context)]);
    }

    fn matching_candidates(&self, observed: &ObservedUseKey) -> Vec<ResourceTargetKey> {
        let named = self
            .candidates
            .keys()
            .filter(|candidate| {
                candidate.agent == observed.agent
                    && candidate.kind == observed.kind
                    && identity_matches(candidate, observed)
            })
            .cloned()
            .collect::<Vec<_>>();
        match &observed.scope {
            ObservedScope::Global => named
                .into_iter()
                .filter(|candidate| candidate.scope == ResourceAssessmentScope::Global)
                .collect(),
            ObservedScope::Project(root) => named
                .into_iter()
                .filter(|candidate| {
                    candidate.scope == ResourceAssessmentScope::Project(root.clone())
                })
                .collect(),
            ObservedScope::Context(root) => {
                if let Some(root) = root {
                    let project = named
                        .iter()
                        .filter(|candidate| {
                            candidate.scope == ResourceAssessmentScope::Project(root.clone())
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    if !project.is_empty() {
                        return project;
                    }
                }
                named
                    .into_iter()
                    .filter(|candidate| candidate.scope == ResourceAssessmentScope::Global)
                    .collect()
            }
        }
    }

    fn replicated_tokens_by_session(
        &self,
        key: &ResourceTargetKey,
        state: &CandidateState,
    ) -> Option<BTreeMap<usize, u128>> {
        if key.kind == ResourceKind::McpServer
            || (key.kind == ResourceKind::BuiltInTool
                && matches!(key.agent, AgentKind::Claude | AgentKind::Codex))
        {
            return self.measured_tokens_by_session(key);
        }
        let definition_tokens = state.resource.definition_tokens.or_else(|| {
            (key.kind == ResourceKind::BuiltInTool)
                .then(|| pinned_built_in_tokens(key.agent, &state.resource.canonical_name))
                .flatten()
        })?;
        if definition_tokens == 0 {
            return None;
        }
        self.turns.iter().try_fold(
            BTreeMap::<usize, u128>::new(),
            |mut totals, (turn, count)| {
                if turn.agent != key.agent
                    || turn.context_tokens < u128::from(definition_tokens)
                    || !self.turn_applies(key, turn)
                {
                    return Some(totals);
                }
                let tokens = u128::from(definition_tokens).checked_mul(u128::from(*count))?;
                let entry = totals.entry(turn.session_index).or_default();
                *entry = entry.checked_add(tokens)?;
                Some(totals)
            },
        )
    }

    fn measured_tokens_by_session(&self, key: &ResourceTargetKey) -> Option<BTreeMap<usize, u128>> {
        self.measured_resources.iter().try_fold(
            None::<BTreeMap<usize, u128>>,
            |total, (observed, tokens)| {
                let matching = self.matching_candidates(observed);
                if matching.len() == 1 && matching[0] == *key {
                    let mut total = total.unwrap_or_default();
                    for (session, tokens) in tokens {
                        let entry = total.entry(*session).or_default();
                        *entry = entry.checked_add(*tokens)?;
                    }
                    Some(Some(total))
                } else {
                    Some(total)
                }
            },
        )?
    }

    fn turn_applies(&self, key: &ResourceTargetKey, turn: &ResourceTurnKey) -> bool {
        match &key.scope {
            ResourceAssessmentScope::Project(root) => turn.project_root.as_ref() == Some(root),
            ResourceAssessmentScope::Global => !turn.project_root.as_ref().is_some_and(|root| {
                self.candidates.keys().any(|candidate| {
                    candidate.agent == key.agent
                        && candidate.kind == key.kind
                        && candidate.normalized_name == key.normalized_name
                        && candidate.scope == ResourceAssessmentScope::Project(root.clone())
                })
            }),
        }
    }
}

fn pinned_built_in_tokens(agent: AgentKind, name: &str) -> Option<u64> {
    let comparable = antiburn_local::analysis::tool_catalog::comparable_tool_name(name);
    let catalog = match agent {
        AgentKind::OpenCode => OPENCODE_BUILT_IN_TOKENS,
        AgentKind::Pi => PI_BUILT_IN_TOKENS,
        _ => return None,
    };
    catalog.iter().find_map(|(candidate, tokens)| {
        (antiburn_local::analysis::tool_catalog::comparable_tool_name(candidate) == comparable)
            .then_some(*tokens)
    })
}

fn identity_matches(candidate: &ResourceTargetKey, observed: &ObservedUseKey) -> bool {
    if observed.identity_kind == ObservedIdentityKind::ExactResource {
        return candidate.normalized_name == normalize_name(&observed.name);
    }
    match candidate.kind {
        ResourceKind::BuiltInTool => {
            let observed = if matches!(candidate.agent, AgentKind::Claude | AgentKind::Codex) {
                last_dot_segment(&observed.name)
            } else {
                &observed.name
            };
            candidate.normalized_name == normalize_name(observed)
        }
        ResourceKind::Skill => {
            let observed = normalize_name(&observed.name);
            candidate.normalized_name == observed
                || (matches!(candidate.agent, AgentKind::Claude | AgentKind::Codex)
                    && !observed.contains(':')
                    && candidate
                        .normalized_name
                        .rsplit_once(':')
                        .is_some_and(|(_, suffix)| suffix == observed))
        }
        ResourceKind::McpServer => {
            mcp_tool_matches(candidate.agent, &candidate.normalized_name, &observed.name)
        }
    }
}

fn mcp_tool_matches(agent: AgentKind, server: &str, tool: &str) -> bool {
    let tool = normalize_name(tool);
    match agent {
        AgentKind::Claude | AgentKind::Codex => tool
            .strip_prefix("mcp__")
            .and_then(|value| value.split_once("__"))
            .is_some_and(|(observed, _)| observed == server),
        AgentKind::OpenCode => tool.starts_with(&format!("{server}_")),
        AgentKind::Pi => {
            let server = server
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric() || character == '_' {
                        character
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            tool.starts_with(&format!("mcp_{server}_"))
        }
        _ => false,
    }
}

fn measured_source_scope(scope: &str, project_root: Option<&Path>) -> Option<ObservedScope> {
    if scope.contains(":cwd:") {
        return project_root.map(|root| ObservedScope::Project(root.to_owned()));
    }
    match scope.rsplit(':').next()? {
        "bundled" | "user" => Some(ObservedScope::Global),
        "project" => project_root.map(|root| ObservedScope::Project(root.to_owned())),
        _ => None,
    }
}

fn source_scope(
    origin: &EvidenceValue<SourceOrigin>,
    project: Option<&Path>,
) -> Option<ObservedScope> {
    let origin = match origin {
        EvidenceValue::Complete(origin)
        | EvidenceValue::Partial {
            observed: origin, ..
        } => *origin,
        EvidenceValue::Unsupported => return None,
    };
    match origin {
        SourceOrigin::Bundled | SourceOrigin::User => Some(ObservedScope::Global),
        SourceOrigin::Project => project.map(|root| ObservedScope::Project(root.to_owned())),
        SourceOrigin::Plugin | SourceOrigin::Unknown => None,
    }
}

fn assessment_scope(scope: &ObservedScope) -> ResourceAssessmentScope {
    match scope {
        ObservedScope::Global => ResourceAssessmentScope::Global,
        ObservedScope::Project(root) => ResourceAssessmentScope::Project(root.clone()),
        ObservedScope::Context(_) => unreachable!("candidate scopes are exact"),
    }
}

fn resource_scope(scope: &ObservedScope) -> ResourceScope {
    match scope {
        ObservedScope::Global => ResourceScope::Global,
        ObservedScope::Project(_) => ResourceScope::Project,
        ObservedScope::Context(_) => ResourceScope::Unknown,
    }
}

fn session_sample(context: &SessionContext<'_>) -> ResourceSupportingSession {
    ResourceSupportingSession {
        environment_key: context.environment_key.to_owned(),
        agent: context.agent.slug().to_owned(),
        session_id: context.session_id.to_owned(),
        observed_at_ms: context.observed_at_ms,
    }
}

fn merge_samples(
    target: &mut Vec<ResourceSupportingSession>,
    samples: impl IntoIterator<Item = ResourceSupportingSession>,
) {
    for sample in samples {
        if target.iter().any(|current| {
            current.environment_key == sample.environment_key
                && current.agent == sample.agent
                && current.session_id == sample.session_id
        }) {
            continue;
        }
        if target.len() == MAX_SUPPORTING_SESSIONS_PER_TARGET {
            break;
        }
        target.push(sample);
    }
}

fn is_situational(agent: AgentKind, name: &str) -> bool {
    let comparable = antiburn_local::analysis::tool_catalog::comparable_tool_name(name);
    antiburn_local::analysis::tool_catalog::situational_tools(agent.slug())
        .iter()
        .any(|name| {
            antiburn_local::analysis::tool_catalog::comparable_tool_name(name) == comparable
        })
}

fn normalize_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn last_dot_segment(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

pub(crate) const fn detector_for_kind(kind: ResourceKind) -> DetectorId {
    match kind {
        ResourceKind::McpServer => DetectorId::UnusedMcpServers,
        ResourceKind::BuiltInTool => DetectorId::UnusedBuiltInTools,
        ResourceKind::Skill => DetectorId::UnusedSkills,
    }
}

const fn resource_kinds() -> [ResourceKind; 3] {
    [
        ResourceKind::McpServer,
        ResourceKind::BuiltInTool,
        ResourceKind::Skill,
    ]
}

/// Agents with resource evidence and inventory support.
///
/// Product tiers do not limit evidence assessment or remediation.
pub(crate) const fn resource_assessment_agents() -> [AgentKind; 11] {
    [
        AgentKind::Claude,
        AgentKind::Codex,
        AgentKind::Cursor,
        AgentKind::Copilot,
        AgentKind::Cline,
        AgentKind::OpenCode,
        AgentKind::Kiro,
        AgentKind::AmpCode,
        AgentKind::Antigravity,
        AgentKind::Windsurf,
        AgentKind::Pi,
    ]
}

#[cfg(test)]
mod tests;
