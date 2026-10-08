use crate::analysis::{
    EvidenceValue, JevReferenceSnapshot, OrderingObservation, SessionEvidence, SourceAcceptance,
};
use crate::checks::ignored_instructions::sha256_hex;
use crate::model::{AgentKind, SkillUse};
use serde::{Deserialize, Serialize};

pub const MAX_SKILL_CANDIDATES: usize = 512;
pub const MAX_SKILL_DESCRIPTION_BYTES: usize = 16 * 1024;
pub const MAX_SKILL_FRONTMATTER_BYTES: usize = 32 * 1024;
const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
pub const MAX_SKILL_USE_EVENTS: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillScope {
    pub agent: AgentKind,
    pub project_identity: Option<String>,
    pub environment_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillDefinition {
    pub identity: String,
    pub revision: String,
    pub name: String,
    pub aliases: Vec<String>,
    /// The description, or the full Markdown when no description exists.
    pub description: String,
    /// Semantic frontmatter only. Provider requests exclude these fields.
    pub frontmatter: serde_json::Value,
    pub scope: SkillScope,
    pub enabled: bool,
    pub created_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillReferenceSource {
    Description,
    MarkdownFallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillReferenceCoverage {
    pub source: SkillReferenceSource,
    pub ranges: Vec<(usize, usize)>,
    pub total_bytes: usize,
    pub partial: bool,
}

pub(crate) fn representative_ranges(text: &str, chunk_bytes: usize) -> Vec<(usize, usize)> {
    let ranges = crate::analysis::jev::text_ranges::text_ranges(text, chunk_bytes, 0);
    if ranges.len() <= 4 {
        return ranges;
    }
    [
        0,
        (ranges.len() - 1) / 3,
        (ranges.len() - 1) / 2,
        ranges.len() - 1,
    ]
    .map(|index| ranges[index])
    .to_vec()
}

impl SkillReferenceCoverage {
    pub(crate) fn content(&self, selected_text: &str) -> serde_json::Value {
        let mut offset = 0;
        let chunks = self
            .ranges
            .iter()
            .map(|&(start, end)| {
                let text = &selected_text[offset..offset + end - start];
                offset += end - start;
                serde_json::json!({"start_byte": start, "end_byte": end, "text": text})
            })
            .collect::<Vec<_>>();
        serde_json::json!({"source": self.source, "chunks": chunks, "total_bytes": self.total_bytes, "partial": self.partial})
    }
}

impl SkillDefinition {
    fn has_description(&self) -> bool {
        self.frontmatter["description"]
            .as_str()
            .is_some_and(|description| !description.trim().is_empty())
    }

    pub(crate) fn selected_reference(
        &self,
        chunk_bytes: usize,
    ) -> (String, SkillReferenceCoverage) {
        let description = self.has_description();
        let ranges = if description && self.description.len() <= 4 * chunk_bytes {
            vec![(0, self.description.len())]
        } else {
            representative_ranges(&self.description, chunk_bytes)
        };
        let selected = ranges
            .iter()
            .map(|&(start, end)| &self.description[start..end])
            .collect::<String>();
        let coverage = SkillReferenceCoverage {
            source: if description {
                SkillReferenceSource::Description
            } else {
                SkillReferenceSource::MarkdownFallback
            },
            partial: selected.len() < self.description.len(),
            total_bytes: self.description.len(),
            ranges,
        };
        (selected, coverage)
    }

    pub(crate) fn reference_content(&self) -> serde_json::Value {
        let (text, coverage) = self.selected_reference(4096);
        coverage.content(&text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillUseStatus {
    Complete,
    Partial,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillUseIdentity {
    Exact,
    Inferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillUseLifecycle {
    Requested,
    DocumentSelected,
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillUseEvent {
    pub identity: String,
    pub identity_kind: SkillUseIdentity,
    pub lifecycle: SkillUseLifecycle,
    pub source_identity: String,
    pub source_field: String,
    pub timestamp_ms: Option<i64>,
    pub order: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillUseOrdering {
    Monotonic,
    OutOfOrder,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillUseEvidence {
    pub session_identity: String,
    pub scope: SkillScope,
    pub status: SkillUseStatus,
    pub ordering: SkillUseOrdering,
    pub events: Vec<SkillUseEvent>,
}

impl SkillUseEvidence {
    /// Existing aggregates prove a request, but do not preserve exact identity,
    /// request order, wall time, or successful execution.
    pub fn from_session(
        evidence: &SessionEvidence,
        recorded: &[SkillUse],
        scope: SkillScope,
    ) -> Result<Self, SkillInputError> {
        let agent = evidence.identity.agent.as_str();
        let agent_matches = agent == scope.agent.slug()
            || matches!(
                (scope.agent, agent),
                (AgentKind::Claude, "claude")
                    | (AgentKind::Cursor, "cursor-ide")
                    | (AgentKind::Copilot, "github-copilot")
                    | (AgentKind::Kiro, "kiro-cli")
                    | (AgentKind::Windsurf, "devin")
                    | (AgentKind::AmpCode, "amp")
            );
        if !agent_matches {
            return Err(SkillInputError::WrongSessionOrScope);
        }
        let session_identity = digest(&serde_json::json!({
            "agent": evidence.identity.agent, "session": evidence.identity.session_id,
        }));
        let (sources, mut status) = match &evidence.context_sources {
            EvidenceValue::Complete(sources) => (Some(sources), SkillUseStatus::Complete),
            EvidenceValue::Partial { observed, .. } => (Some(observed), SkillUseStatus::Partial),
            EvidenceValue::Unsupported => (None, SkillUseStatus::Unsupported),
        };
        if let Some(sources) = sources {
            status = match (&sources.skill_coverage, status) {
                (EvidenceValue::Unsupported, _) => SkillUseStatus::Unsupported,
                (EvidenceValue::Partial { .. }, _) => SkillUseStatus::Partial,
                (_, status) => status,
            };
        }
        // Inventory coverage alone cannot prove that all requests were observed.
        status = match (&evidence.tools, status) {
            (EvidenceValue::Partial { .. }, SkillUseStatus::Complete) => SkillUseStatus::Partial,
            (EvidenceValue::Unsupported, _) if !evidence.capabilities.tool_invocations => {
                SkillUseStatus::Unsupported
            }
            (EvidenceValue::Unsupported, _) => SkillUseStatus::Unknown,
            (_, status) => status,
        };
        status = match evidence.provenance.source_acceptance {
            SourceAcceptance::AcceptedFull => status,
            SourceAcceptance::AcceptedPrefix { .. } if status == SkillUseStatus::Complete => {
                SkillUseStatus::Partial
            }
            SourceAcceptance::NotObserved
            | SourceAcceptance::Unvalidated
            | SourceAcceptance::SourceChanged => SkillUseStatus::Unknown,
            _ => status,
        };
        let mut events = Vec::new();
        if let Some(sources) = sources {
            for (name, skill) in &sources.skills {
                if skill.invoked {
                    if events.len() == MAX_SKILL_USE_EVENTS {
                        status = SkillUseStatus::Partial;
                        break;
                    }
                    events.push(SkillUseEvent {
                        identity: name.clone(),
                        identity_kind: SkillUseIdentity::Inferred,
                        lifecycle: SkillUseLifecycle::Requested,
                        source_identity: session_identity.clone(),
                        source_field: "context_sources.skills.invoked".into(),
                        timestamp_ms: None,
                        order: None,
                    });
                }
            }
        }
        for usage in recorded {
            if events.len() == MAX_SKILL_USE_EVENTS {
                status = SkillUseStatus::Partial;
                break;
            }
            if usage.name.len() > 256 {
                status = SkillUseStatus::Partial;
                continue;
            }
            events.push(SkillUseEvent {
                identity: usage.name.clone(),
                identity_kind: SkillUseIdentity::Inferred,
                lifecycle: SkillUseLifecycle::Requested,
                source_identity: session_identity.clone(),
                source_field: "skill_uses".into(),
                timestamp_ms: None,
                order: None,
            });
        }
        if status == SkillUseStatus::Complete {
            status = SkillUseStatus::Partial;
        }
        Ok(Self {
            session_identity,
            scope,
            status,
            ordering: match evidence.provenance.ordering {
                OrderingObservation::OutOfOrder => SkillUseOrdering::OutOfOrder,
                OrderingObservation::Monotonic => SkillUseOrdering::Unknown,
            },
            events,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillOpportunityLimit {
    CurrentInventoryOnly,
    CreationTimeUnknown,
    WorkTimeUnknown,
    UseEvidenceIncomplete,
    UseIdentityInferred,
    UseOrderUnknown,
    InventoryIncomplete,
    SelectedUseWindowOnly,
    AggregateUseCannotProveAbsence,
    ReferenceContentPartial,
    KnownUseContextPartial,
    TaskContextPartial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillWorkContext {
    pub session_identity: String,
    pub scope: SkillScope,
    /// The timestamp of the relevant work, not the episode end.
    pub relevant_work_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EligibleSkillCandidate {
    skill: SkillDefinition,
    limitations: Vec<SkillOpportunityLimit>,
    reference: JevReferenceSnapshot,
}

impl EligibleSkillCandidate {
    pub fn skill(&self) -> &SkillDefinition {
        &self.skill
    }

    pub fn limitations(&self) -> &[SkillOpportunityLimit] {
        &self.limitations
    }

    pub fn reference_snapshot(&self) -> JevReferenceSnapshot {
        self.reference.clone()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkillOpportunitySnapshot {
    scope: SkillScope,
    skills: Vec<SkillDefinition>,
    inventory_complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillInputError {
    InvalidDefinition,
    LimitExceeded,
    WrongSessionOrScope,
    InvalidUseEvidence,
}

impl SkillOpportunitySnapshot {
    pub fn new(
        scope: SkillScope,
        skills: Vec<SkillDefinition>,
        inventory_complete: bool,
    ) -> Result<Self, SkillInputError> {
        if skills.len() > MAX_SKILL_CANDIDATES {
            return Err(SkillInputError::LimitExceeded);
        }
        if scope.environment_identity.is_empty() {
            return Err(SkillInputError::InvalidDefinition);
        }
        let mut bytes = 0usize;
        let mut identities = std::collections::BTreeSet::new();
        for skill in &skills {
            let frontmatter_bytes = skill.frontmatter.to_string().len();
            if (skill.has_description() && skill.description.len() > MAX_SKILL_DESCRIPTION_BYTES)
                || frontmatter_bytes > MAX_SKILL_FRONTMATTER_BYTES
                || skill.aliases.len() > 16
            {
                return Err(SkillInputError::LimitExceeded);
            }
            if skill.scope != scope
                || skill.identity.is_empty()
                || skill.revision.is_empty()
                || skill.name.trim().is_empty()
                || skill.name.len() > 256
                || skill.identity.len() > 256
                || skill.revision.len() > 256
                || skill.description.trim().is_empty()
                || !skill.frontmatter.is_object()
                || skill
                    .frontmatter
                    .get("description")
                    .is_some_and(|value| !value.is_null() && !value.is_string())
                || !identities.insert(&skill.identity)
                || skill
                    .aliases
                    .iter()
                    .any(|alias| alias.trim().is_empty() || alias.len() > 256)
            {
                return Err(SkillInputError::InvalidDefinition);
            }
            bytes = bytes
                .saturating_add(frontmatter_bytes + skill.reference_content().to_string().len());
        }
        if bytes > MAX_SNAPSHOT_BYTES {
            return Err(SkillInputError::LimitExceeded);
        }
        Ok(Self {
            scope,
            skills,
            inventory_complete,
        })
    }

    pub fn scope(&self) -> &SkillScope {
        &self.scope
    }

    pub fn skills(&self) -> &[SkillDefinition] {
        &self.skills
    }

    pub fn revision(&self) -> String {
        digest(&serde_json::json!((
            &self.scope,
            &self.skills,
            self.inventory_complete
        )))
    }

    pub fn eligible_candidates_with_recorded_use(
        &self,
        work: &SkillWorkContext,
        usage: &super::SkillUseSnapshot,
    ) -> Result<Vec<EligibleSkillCandidate>, SkillInputError> {
        let mut candidates = self.eligible_candidates(work, usage.evidence())?;
        for candidate in &mut candidates {
            candidate
                .limitations
                .push(if usage.publication_fence().is_some() {
                    SkillOpportunityLimit::SelectedUseWindowOnly
                } else {
                    SkillOpportunityLimit::AggregateUseCannotProveAbsence
                });
            candidate.reference.fields["limitations"] = serde_json::json!(candidate.limitations);
            candidate.reference.fields["use_snapshot_revision"] =
                serde_json::json!(usage.revision());
            candidate.reference.fields["use_coverage"] = serde_json::json!(usage.coverage());
            candidate.reference.fields["publication_fence"] =
                serde_json::json!(usage.publication_fence());
            candidate.reference.revision = digest(&candidate.reference.fields);
        }
        Ok(candidates)
    }

    pub fn eligible_candidates(
        &self,
        work: &SkillWorkContext,
        usage: &SkillUseEvidence,
    ) -> Result<Vec<EligibleSkillCandidate>, SkillInputError> {
        if work.scope != self.scope
            || usage.scope != self.scope
            || work.session_identity != usage.session_identity
        {
            return Err(SkillInputError::WrongSessionOrScope);
        }
        if usage.events.len() > MAX_SKILL_USE_EVENTS {
            return Err(SkillInputError::LimitExceeded);
        }
        if usage.session_identity.is_empty()
            || usage.events.iter().any(|event| {
                event.identity.len() > 256
                    || event.source_field.len() > 256
                    || event.source_identity != usage.session_identity
                    || event.source_field.is_empty()
            })
        {
            return Err(SkillInputError::InvalidUseEvidence);
        }
        let use_revision = digest(&serde_json::json!(usage));
        let mut candidates = Vec::new();
        for skill in &self.skills {
            if !skill.enabled {
                continue;
            }
            let mut limitations = vec![SkillOpportunityLimit::CurrentInventoryOnly];
            if skill.selected_reference(4096).1.partial {
                limitations.push(SkillOpportunityLimit::ReferenceContentPartial);
            }
            if skill.created_at_ms.is_none() {
                limitations.push(SkillOpportunityLimit::CreationTimeUnknown);
            }
            if work.relevant_work_at_ms.is_none() {
                limitations.push(SkillOpportunityLimit::WorkTimeUnknown);
            }
            if !self.inventory_complete {
                limitations.push(SkillOpportunityLimit::InventoryIncomplete);
            }
            if usage.status != SkillUseStatus::Complete {
                limitations.push(SkillOpportunityLimit::UseEvidenceIncomplete);
            }
            if usage.events.iter().any(|event| {
                event.identity_kind == SkillUseIdentity::Inferred
                    || event.lifecycle == SkillUseLifecycle::Unknown
                    || event.source_identity.is_empty()
                    || event.source_field.is_empty()
            }) {
                limitations.push(SkillOpportunityLimit::UseIdentityInferred);
            }
            if usage.ordering != SkillUseOrdering::Monotonic {
                limitations.push(SkillOpportunityLimit::UseOrderUnknown);
            }
            candidates.push(EligibleSkillCandidate {
                skill: skill.clone(),
                reference: {
                    let fields = serde_json::json!({
                        "name": skill.name, "content": skill.reference_content(),
                        "created_at_ms": skill.created_at_ms,
                        "relevant_work_at_ms": work.relevant_work_at_ms,
                        "limitations": limitations, "use_status": usage.status,
                        "use_ordering": usage.ordering, "use_revision": use_revision,
                        "definition_revision": skill.revision,
                    });
                    JevReferenceSnapshot {
                        kind: "skill_opportunity".into(),
                        identity: skill.identity.clone(),
                        revision: digest(&fields),
                        fields,
                    }
                },
                limitations,
            });
        }
        Ok(candidates)
    }
}

fn digest(value: &serde_json::Value) -> String {
    sha256_hex(value.to_string().as_bytes())
}

#[cfg(test)]
mod tests;
