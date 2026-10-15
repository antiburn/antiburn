use super::episodes::{InvestigationEpisode, MAX_EPISODES, MAX_EVENTS, OverExploringInput};
use super::questions::{QUESTION_ID, questions};
use crate::analysis::jev::capabilities::ModelCapabilities;
use crate::analysis::jev::{
    JevAnswer, JevCheck, JevCheckPlan, JevCheckRevisions, JevCoverage, JevError,
    JevEvidenceReference, JevEvidenceRole, JevInputField, JevInputSelection, JevInputWindow,
    JevRequest, JevResponse, JevSessionContext, JevSharedRequestContext, JevWorkItem,
    JevWorkItemResult, validate_jev_response,
};
use crate::analysis::jev_evidence::{
    ContentAction, ContentEventReference, JevReadResultKind, JevReadStatus, content_action_digest,
};
use crate::checks::sampling::{Candidate, SamplingError, SamplingJob, SamplingProgress, StableId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const SEMANTIC_PROBABILITY_THRESHOLD: f64 = 0.95;
pub const MAX_TARGETS_PER_TURN: usize = 3;
pub const MAX_CANDIDATE_STATE_BYTES: usize = 1024;
pub const MAX_PACKED_REQUEST_BYTES: usize = 6144;
pub const MAX_ESTIMATED_INPUT_TOKENS: u64 = 1536;
const MAX_JOBS_PER_REQUEST: usize = 1;
pub const MAX_SAMPLING_CANDIDATES: usize = 2 * MAX_EVENTS + MAX_EPISODES;
const MAX_READ_SAMPLE_BYTES: usize = 512;
const MAX_REPEAT_SAMPLE_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    UnrelatedFiles,
    ExcessiveFileBreadth,
    ExcessiveWithinFileReading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Abstention {
    IncompleteHistory,
    DeferredEpisode,
    TruncatedEvidence,
    SourceLimited,
    SampledEvidence,
    ContextTooLarge,
    InvalidEvidence,
    PartialAssessment,
    UncertainDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadBinding {
    pub request_id: String,
    pub result_id: Option<String>,
    pub output_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticOutcome {
    LikelyExcess,
    JustifiedOrMinor,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub episode_id: StableId,
    pub work_item_id: String,
    pub reason: Reason,
    pub reads: Vec<ReadBinding>,
    pub task_evidence: Vec<JevEvidenceReference>,
    pub source_evidence: Vec<JevEvidenceReference>,
    pub semantic_revision: String,
    pub model: String,
    pub revisions: JevCheckRevisions,
    pub outcome: SemanticOutcome,
    pub probability: f64,
    #[serde(default)]
    pub explanation: Option<ExplanationBasis>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExplanationBasis {
    pub version: u32,
    pub relationship: ReadRelationship,
    pub task: JevSharedRequestContext,
    pub compared: JevInputWindow,
    pub reads: Vec<super::ReadObservation>,
    pub whole_output_equal: Option<bool>,
    pub snippets: Vec<ReadSourceSnippet>,
    #[serde(default)]
    pub purpose: Option<ReadSourceSnippet>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadSourceSnippet {
    pub reference: ContentEventReference,
    pub kind: String,
    pub timestamp_ms: Option<i64>,
    pub range: (usize, usize),
    pub text: String,
    pub source_digest: String,
    pub partial: bool,
}

impl ReadSourceSnippet {
    pub fn matches_action(&self, action: &ContentAction) -> bool {
        self.reference == action.reference
            && self.kind == action.kind
            && self.timestamp_ms == action.timestamp_ms
            && self.source_digest == content_action_digest(action)
            && self.range.0 < self.range.1
            && action.text.get(self.range.0..self.range.1) == Some(self.text.as_str())
            && self.partial == (action.truncated || self.text.len() < action.text.len())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unassessed {
    pub episode_id: StableId,
    pub work_item_id: Option<String>,
    pub reason: Option<Reason>,
    pub limitation: Abstention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadRelationship {
    SelectedReadUnrelated,
    LaterReadRepeatsEarlier,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assessment {
    pub epoch: StableId,
    pub findings: Vec<Decision>,
    pub clean_episode_ids: Vec<StableId>,
    pub completed_episode_ids: Vec<StableId>,
    pub completed_work_item_ids: Vec<String>,
    pub unassessed: Vec<Unassessed>,
    pub coverage: JevCoverage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub episode_id: StableId,
    pub reason: Reason,
    pub read_indexes: Vec<usize>,
    pub bindings: Vec<ReadBinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreparedAssessment {
    pub epoch: StableId,
    #[serde(with = "shared_inventory")]
    pub targets: Arc<BTreeMap<String, Target>>,
    pub candidates: Vec<SamplingCandidate>,
    pub unassessed: Vec<Unassessed>,
    /// Keep source records once. Materialize only selected target windows.
    #[serde(with = "shared_inventory")]
    pub episodes: Arc<Vec<InvestigationEpisode>>,
    #[serde(with = "shared_inventory")]
    pub events: Arc<Vec<ContentAction>>,
    pub history_complete: bool,
    pub limitations: Vec<String>,
    #[serde(with = "shared_inventory")]
    pub task_contexts: Arc<BTreeMap<StableId, JevSharedRequestContext>>,
}

mod shared_inventory {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::Arc;

    pub fn serialize<T: Serialize, S: Serializer>(
        value: &Arc<T>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.as_ref().serialize(serializer)
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Arc<T>, D::Error> {
        T::deserialize(deserializer).map(Arc::new)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SamplingCandidate {
    pub candidate_id: StableId,
    pub episode_id: StableId,
    pub work_item_ids: Vec<String>,
    pub required_answers: Vec<StableId>,
}

pub struct OverExploringCheck;

impl JevCheck for OverExploringCheck {
    type Prepared = PreparedAssessment;
    type Result = Assessment;
    fn id(&self) -> &'static str {
        "over_exploring"
    }
    fn revisions(&self) -> JevCheckRevisions {
        JevCheckRevisions {
            projection: 11,
            chunking: 10,
            questions: 22,
            reducer: 11,
        }
    }
    fn input_selection(&self) -> JevInputSelection {
        JevInputSelection::from_fields(&[
            JevInputField::UserMessage,
            JevInputField::AssistantMessage,
            JevInputField::UserAnswer,
            JevInputField::PlanReference,
            JevInputField::ReadFileRequest,
            JevInputField::ReadFilePath,
            JevInputField::ReadFileResult,
            JevInputField::ReadFileOutput,
            JevInputField::SearchFilesQuery,
            JevInputField::SearchFilesOutput,
            JevInputField::FileEditPath,
            JevInputField::FileEditContent,
            JevInputField::BashCommandInput,
            JevInputField::BashCommandOutput,
            JevInputField::OtherToolInput,
            JevInputField::OtherToolOutput,
        ])
    }
    fn supports_incremental_reuse(&self) -> bool {
        true
    }
    fn incremental_identity(&self, context: &JevSessionContext) -> Value {
        json!({"session": context.session_identity, "input": context.input_revision, "revisions": self.revisions()})
    }
    fn prepare(
        &self,
        context: &JevSessionContext,
    ) -> Result<JevCheckPlan<Self::Prepared>, JevError> {
        self.prepare_with_capabilities(context, &ModelCapabilities::jev_default())
    }
    fn prepare_with_capabilities(
        &self,
        context: &JevSessionContext,
        capabilities: &ModelCapabilities,
    ) -> Result<JevCheckPlan<Self::Prepared>, JevError> {
        let input: OverExploringInput = serde_json::from_value(context.check_context.clone())
            .map_err(|_| JevError::InvalidCheckContext)?;
        let task_limited = input.task_context.fields["limitations"]
            .as_array()
            .is_some_and(|limits| !limits.is_empty());
        if input.session_identity != context.session_identity
            || input.events.len() > MAX_EVENTS
            || input.episodes.len() > MAX_EPISODES
        {
            return Err(JevError::InvalidCheckContext);
        }
        let epoch_bytes =
            serde_json::to_vec(&(&input.task_context, &input.task_contexts, self.revisions()))
                .map_err(|_| JevError::RequestSerialization)?;
        let epoch = StableId::new(
            "over-exploring-context",
            &[input.session_identity.as_bytes(), &epoch_bytes],
        );
        let mut prepared = PreparedAssessment {
            epoch,
            targets: Arc::new(BTreeMap::new()),
            candidates: Vec::new(),
            unassessed: Vec::new(),
            episodes: Arc::new(input.episodes),
            events: Arc::new(input.events),
            history_complete: input.complete,
            limitations: input.limitations,
            task_contexts: Arc::new(input.task_contexts),
        };
        let source_bytes = serde_json::to_vec(&(
            prepared.events.as_ref(),
            prepared.history_complete,
            &prepared.limitations,
        ))
        .map_err(|_| JevError::RequestSerialization)?;
        let source_revision = StableId::new("over-exploring-source-v3", &[&source_bytes]);
        let mut seen = BTreeSet::new();
        let mut covered = BTreeSet::new();
        let mut selected_work_item_ids = Vec::new();
        let mut ranked_targets = Vec::new();
        for episode in prepared.episodes.iter() {
            validate_episode(episode, &prepared.events)?;
            if !seen.insert(episode.id) {
                return Err(JevError::InvalidCheckContext);
            }
            if episode.events.iter().any(|index| !covered.insert(*index)) {
                return Err(JevError::InvalidCheckContext);
            }
            for (limited, limitation) in [
                (!prepared.history_complete, Abstention::IncompleteHistory),
                (
                    episode.state == super::EpisodeState::Deferred,
                    Abstention::DeferredEpisode,
                ),
                (
                    !prepared.limitations.is_empty()
                        || task_limited
                        || prepared
                            .task_contexts
                            .get(&episode.id)
                            .is_none_or(|context| {
                                !super::episodes::task_context_is_complete(context)
                            }),
                    Abstention::SourceLimited,
                ),
                (
                    prepared.events.iter().any(|event| {
                        event.truncated
                            || event
                                .metadata
                                .read_request
                                .as_ref()
                                .is_some_and(|read| read.truncated)
                            || event
                                .metadata
                                .read_result
                                .as_ref()
                                .is_some_and(|read| read.truncated)
                    }),
                    Abstention::TruncatedEvidence,
                ),
            ] {
                if limited {
                    prepared.unassessed.push(Unassessed {
                        episode_id: episode.id,
                        work_item_id: None,
                        reason: None,
                        limitation,
                    });
                }
            }
            let episode_bytes = serde_json::to_vec(&(episode, source_revision))
                .map_err(|_| JevError::RequestSerialization)?;
            let episode_revision = StableId::new("over-exploring-source-v3", &[&episode_bytes]);
            if !prepared.history_complete || !prepared.limitations.is_empty() || task_limited {
                continue;
            }
            for target in episode_targets(episode, &prepared.events, &prepared.task_contexts)? {
                ranked_targets.push((target, episode_revision));
            }
        }
        ranked_targets.sort_by_key(|(target, _)| {
            let episode = prepared
                .episodes
                .iter()
                .find(|episode| episode.id == target.episode_id)
                .expect("validated episode");
            let bytes = target
                .read_indexes
                .iter()
                .skip(usize::from(
                    target.reason == Reason::ExcessiveWithinFileReading,
                ))
                .filter_map(|index| episode.reads[*index].result.as_ref())
                .fold(0_u64, |sum, result| {
                    sum.saturating_add(result.recorded_output_bytes)
                });
            let order = prepared
                .events
                .iter()
                .position(|event| event.reference.id == target.bindings[0].request_id)
                .expect("validated request");
            (
                target.reason != Reason::ExcessiveWithinFileReading,
                std::cmp::Reverse(bytes),
                order,
            )
        });
        let mut repeats = 0;
        let mut unrelated = 0;
        for (target, episode_revision) in ranked_targets {
            let count = if target.reason == Reason::ExcessiveWithinFileReading {
                &mut repeats
            } else {
                &mut unrelated
            };
            let limit = if target.reason == Reason::ExcessiveWithinFileReading {
                2
            } else {
                1
            };
            if *count >= limit {
                prepared.unassessed.push(Unassessed {
                    episode_id: target.episode_id,
                    work_item_id: None,
                    reason: Some(target.reason),
                    limitation: Abstention::SampledEvidence,
                });
                continue;
            }
            *count += 1;
            let bytes = serde_json::to_vec(&(&target, episode_revision, epoch))
                .map_err(|_| JevError::RequestSerialization)?;
            let candidate_id = StableId::new("over-exploring-target-v3", &[&bytes]);
            let id: String = candidate_id.into();
            selected_work_item_ids.push(id.clone());
            Arc::make_mut(&mut prepared.targets).insert(id, target);
        }
        if !selected_work_item_ids.is_empty() {
            let candidate_bytes = serde_json::to_vec(&(&selected_work_item_ids, epoch))
                .map_err(|_| JevError::RequestSerialization)?;
            let candidate_id = StableId::new("over-exploring-snapshot-v2", &[&candidate_bytes]);
            let episode_id = prepared.targets[&selected_work_item_ids[0]].episode_id;
            prepared.candidates.push(SamplingCandidate {
                candidate_id,
                episode_id,
                required_answers: selected_work_item_ids
                    .iter()
                    .map(|id| answer_id(id))
                    .collect(),
                work_item_ids: selected_work_item_ids,
            });
        }
        let selected = prepared
            .candidates
            .iter()
            .take(MAX_TARGETS_PER_TURN)
            .map(|candidate| candidate.candidate_id)
            .collect::<BTreeSet<_>>();
        let mut plan = JevCheckPlan {
            check_id: self.id().into(),
            input_revision: context.input_revision.clone(),
            revisions: self.revisions(),
            work_items: Vec::new(),
            skipped_item_ids: Vec::new(),
            coverage: JevCoverage::default(),
            capabilities: capabilities.clone(),
            shared_context: Some(input.task_context),
            prepared,
        };
        materialize(&mut plan, &selected)?;
        Ok(plan)
    }
    fn reduce(
        &self,
        plan: &JevCheckPlan<Self::Prepared>,
        results: &[JevWorkItemResult],
        complete: bool,
    ) -> Result<Self::Result, JevError> {
        let empty = JevSharedRequestContext {
            fields: json!({}),
            evidence: vec![],
        };
        let shared = plan.shared_context.as_ref().unwrap_or(&empty);
        if plan.check_id != self.id() || plan.revisions != self.revisions() {
            return Err(JevError::InvalidCheckPlan);
        }
        let mut assessment = Assessment {
            epoch: plan.prepared.epoch,
            findings: Vec::new(),
            clean_episode_ids: Vec::new(),
            completed_episode_ids: Vec::new(),
            completed_work_item_ids: Vec::new(),
            unassessed: plan.prepared.unassessed.clone(),
            coverage: plan.coverage.clone(),
        };
        let mut outcomes = BTreeMap::new();
        for result in results {
            let item = plan
                .work_items
                .iter()
                .find(|item| item.id == result.work_item_id)
                .ok_or(JevError::InvalidCheckPlan)?;
            let mut evidence = shared.evidence.clone();
            evidence.extend(item.window.evidence.clone());
            if result.model != plan.capabilities.model
                || result.evidence != evidence
                || result.answers.len() != 1
                || outcomes.contains_key(&item.id)
            {
                return Err(JevError::InvalidCheckPlan);
            }
            let target = plan
                .prepared
                .targets
                .get(&item.id)
                .ok_or(JevError::InvalidCheckPlan)?;
            let episode = plan
                .prepared
                .episodes
                .iter()
                .find(|episode| episode.id == target.episode_id)
                .ok_or(JevError::InvalidCheckPlan)?;
            validate_episode(episode, &plan.prepared.events)?;
            if !plan.prepared.history_complete
                || !plan.prepared.limitations.is_empty()
                || !episode_targets(episode, &plan.prepared.events, &plan.prepared.task_contexts)?
                    .contains(target)
                || ![512, sample_budget(&item.window)]
                    .into_iter()
                    .any(|budget| {
                        compact_window(
                            episode,
                            &plan.prepared.events,
                            target,
                            &plan.prepared.task_contexts[&target.episode_id],
                            budget,
                        )
                        .is_ok_and(|mut window| {
                            if shared.fields.get("task").is_some() {
                                let mut task_fields = window.fields["task"].clone();
                                if task_fields != shared.fields["task"] {
                                    compact_task_fields(&mut task_fields);
                                }
                                if task_fields != shared.fields["task"] {
                                    return false;
                                }
                                window
                                    .fields
                                    .as_object_mut()
                                    .expect("compact object")
                                    .remove("task");
                                window.evidence.retain(|reference| {
                                    reference.role != JevEvidenceRole::Instruction
                                });
                            }
                            compact_window_matches(&window, &item.window)
                        })
                    })
            {
                return Err(JevError::InvalidCheckPlan);
            }
            let (outcome, probability) = judgment(result, item)?;
            outcomes.insert(item.id.clone(), outcome);
            // A valid uncertain answer is terminal, but never a clean result.
            assessment.completed_work_item_ids.push(item.id.clone());
            match outcome {
                SemanticOutcome::LikelyExcess => {
                    let relationship = match target.reason {
                        Reason::UnrelatedFiles => ReadRelationship::SelectedReadUnrelated,
                        Reason::ExcessiveWithinFileReading => {
                            ReadRelationship::LaterReadRepeatsEarlier
                        }
                        Reason::ExcessiveFileBreadth => return Err(JevError::InvalidCheckPlan),
                    };
                    let (snippets, purpose) =
                        selected_snippets(&item.window, &plan.prepared.events)?;
                    assessment.findings.push(Decision {
                        episode_id: target.episode_id,
                        work_item_id: item.id.clone(),
                        reason: target.reason,
                        reads: target.bindings.clone(),
                        task_evidence: shared
                            .evidence
                            .iter()
                            .chain(
                                item.window.evidence.iter().filter(|evidence| {
                                    evidence.role == JevEvidenceRole::Instruction
                                }),
                            )
                            .cloned()
                            .collect(),
                        source_evidence: item
                            .window
                            .evidence
                            .iter()
                            .filter(|evidence| evidence.role != JevEvidenceRole::Instruction)
                            .cloned()
                            .collect(),
                        semantic_revision: plan.input_revision.clone(),
                        model: plan.capabilities.model.clone(),
                        revisions: plan.revisions,
                        outcome,
                        probability,
                        explanation: Some(ExplanationBasis {
                            version: 3,
                            relationship,
                            task: JevSharedRequestContext {
                                fields: plan.prepared.task_contexts[&target.episode_id]
                                    .fields
                                    .clone(),
                                evidence: shared
                                    .evidence
                                    .iter()
                                    .chain(item.window.evidence.iter().filter(|evidence| {
                                        evidence.role == JevEvidenceRole::Instruction
                                    }))
                                    .cloned()
                                    .collect(),
                            },
                            compared: item.window.clone(),
                            reads: plan
                                .prepared
                                .episodes
                                .iter()
                                .find(|episode| episode.id == target.episode_id)
                                .expect("validated episode")
                                .reads
                                .iter()
                                .enumerate()
                                .filter(|(index, _)| target.read_indexes.contains(index))
                                .map(|(_, read)| read.clone())
                                .collect(),
                            whole_output_equal: if target.reason
                                == Reason::ExcessiveWithinFileReading
                                && target.bindings.len() >= 2
                            {
                                target.bindings[0]
                                    .output_digest
                                    .as_ref()
                                    .zip(target.bindings[1].output_digest.as_ref())
                                    .map(|(earlier, later)| earlier == later)
                            } else {
                                None
                            },
                            snippets,
                            purpose,
                        }),
                    });
                }
                SemanticOutcome::JustifiedOrMinor => {}
                SemanticOutcome::Uncertain => assessment.unassessed.push(Unassessed {
                    episode_id: target.episode_id,
                    work_item_id: Some(item.id.clone()),
                    reason: Some(target.reason),
                    limitation: Abstention::UncertainDecision,
                }),
            }
        }
        for item in &plan.work_items {
            if !outcomes.contains_key(&item.id) {
                let target = &plan.prepared.targets[&item.id];
                assessment.unassessed.push(Unassessed {
                    episode_id: target.episode_id,
                    work_item_id: Some(item.id.clone()),
                    reason: Some(target.reason),
                    limitation: Abstention::PartialAssessment,
                });
            }
        }
        for episode in plan.prepared.episodes.iter() {
            let ids: Vec<_> = plan
                .prepared
                .targets
                .iter()
                .filter(|(_, target)| target.episode_id == episode.id)
                .map(|(id, _)| id)
                .collect();
            if complete && !ids.is_empty() && ids.iter().all(|id| outcomes.contains_key(*id)) {
                assessment.completed_episode_ids.push(episode.id);
                if ids
                    .iter()
                    .all(|id| outcomes[*id] == SemanticOutcome::JustifiedOrMinor)
                    && !assessment
                        .unassessed
                        .iter()
                        .any(|item| item.episode_id == episode.id)
                {
                    assessment.clean_episode_ids.push(episode.id);
                }
            }
        }
        if assessment
            .findings
            .iter()
            .any(|decision| !plan.prepared.validates_decision(decision))
        {
            return Err(JevError::InvalidCheckPlan);
        }
        Ok(assessment)
    }
}

fn validate_episode(
    episode: &InvestigationEpisode,
    source: &[ContentAction],
) -> Result<(), JevError> {
    if source.windows(2).any(|pair| {
        (pair[0].reference.turn_index, pair[0].reference.part_index)
            >= (pair[1].reference.turn_index, pair[1].reference.part_index)
    }) || source.first().is_some_and(|first| {
        source.iter().any(|event| {
            event.reference.source_key_digest != first.reference.source_key_digest
                || event.reference.thread_digest != first.reference.thread_digest
                || event.kind == "thinking"
                || event.turn_role == "thinking"
        })
    }) {
        return Err(JevError::InvalidCheckContext);
    }
    if !episode
        .before
        .iter()
        .chain(&episode.events)
        .chain(&episode.subsequent)
        .copied()
        .eq(0..source.len())
    {
        return Err(JevError::InvalidCheckContext);
    }
    let mut seen = BTreeSet::new();
    for read in &episode.reads {
        let request = episode
            .events
            .iter()
            .map(|index| &source[*index])
            .find(|event| event.reference.id == read.request.reference_id)
            .ok_or(JevError::InvalidCheckContext)?;
        if !seen.insert(&read.request.reference_id)
            || request.context_only
            || !request.reference.stable
            || request.kind != "tool_input"
            || request.tool_call_id.as_deref().is_none_or(str::is_empty)
            || request.metadata.read_request.as_ref() != Some(&read.request)
        {
            return Err(JevError::InvalidCheckContext);
        }
        if let Some(result) = &read.result {
            let event = episode
                .events
                .iter()
                .map(|index| &source[*index])
                .find(|event| event.reference.id == result.reference_id)
                .ok_or(JevError::InvalidCheckContext)?;
            if event.metadata.read_result.as_ref() != Some(result)
                || !event.reference.stable
                || event.kind != "tool_result"
                || event.tool_call_id != request.tool_call_id
                || event.tool_name != request.tool_name
                || (event.reference.turn_index, event.reference.part_index)
                    <= (request.reference.turn_index, request.reference.part_index)
                || result.reference_id != event.reference.id
                || result.request_reference_id.as_deref() != Some(&read.request.reference_id)
                || result.recorded_output_digest
                    != crate::checks::ignored_instructions::sha256_hex(event.text.as_bytes())
                || result.recorded_output_bytes
                    != u64::try_from(event.text.len()).map_err(|_| JevError::InvalidCheckContext)?
            {
                return Err(JevError::InvalidCheckContext);
            }
        }
    }
    Ok(())
}

fn episode_targets(
    episode: &InvestigationEpisode,
    events: &[ContentAction],
    task_contexts: &BTreeMap<StableId, JevSharedRequestContext>,
) -> Result<Vec<Target>, JevError> {
    let make = |reason, indexes: Vec<usize>| Target {
        episode_id: episode.id,
        reason,
        bindings: indexes
            .iter()
            .map(|index| {
                let read = &episode.reads[*index];
                ReadBinding {
                    request_id: read.request.reference_id.clone(),
                    result_id: read
                        .result
                        .as_ref()
                        .map(|result| result.reference_id.clone()),
                    output_digest: read
                        .result
                        .as_ref()
                        .map(|result| result.recorded_output_digest.clone()),
                }
            })
            .collect(),
        read_indexes: indexes,
    };
    for read in &episode.reads {
        if read.request.paths.len() != 1
            || read.request.reference_id.is_empty()
            || read.request.paths[0].trim().is_empty()
        {
            return Err(JevError::InvalidCheckContext);
        }
    }
    let mut targets = Vec::new();
    let task = task_contexts
        .get(&episode.id)
        .ok_or(JevError::InvalidCheckContext)?;
    if !super::episodes::task_context_is_complete(task) {
        return Ok(Vec::new());
    }
    let named_paths = explicitly_named_paths(task);
    let mut index = 0;
    while index + 1 < episode.reads.len() {
        let first = &episode.reads[index];
        let Some(first_result) = first.result.as_ref() else {
            index += 1;
            continue;
        };
        if source_action(events, &first.request.reference_id)?.truncated
            || source_action(events, &first_result.reference_id)?.truncated
            || first.request.truncated
            || first.request.paths[0].len() > 192
            || !super::has_known_returned_extent(first_result)
            || first_result.status != JevReadStatus::Success
            || first_result.kind != JevReadResultKind::File
            || first_result.truncated
        {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        let mut redundant_bytes = 0_u64;
        while end < episode.reads.len() {
            let previous = &episode.reads[end - 1];
            let later = &episode.reads[end];
            let Some(previous_result) = previous.result.as_ref() else {
                break;
            };
            let Some(later_result) = later.result.as_ref() else {
                break;
            };
            let same_output = first.request.paths == later.request.paths
                && (first.request.cwd == later.request.cwd
                    && (first.request.cwd.is_some()
                        || super::is_absolute_source_path(&first.request.paths[0])))
                && first.request.extent == later.request.extent
                && first.request.native_extent == later.request.native_extent
                && first_result.returned_extent.is_some()
                && first_result.returned_extent == later_result.returned_extent
                && first_result.recorded_output_digest == later_result.recorded_output_digest
                && later_result.status == JevReadStatus::Success
                && later_result.kind == JevReadResultKind::File
                && !later_result.truncated
                && !later.request.truncated
                && !source_action(events, &later.request.reference_id)?.truncated
                && !source_action(events, &later_result.reference_id)?.truncated;
            if !same_output {
                break;
            }
            let Some(result_position) = episode
                .events
                .iter()
                .position(|event| events[*event].reference.id == previous_result.reference_id)
            else {
                break;
            };
            let Some(request_position) = episode
                .events
                .iter()
                .position(|event| events[*event].reference.id == later.request.reference_id)
            else {
                break;
            };
            if result_position.checked_add(1) != Some(request_position) {
                break;
            }
            redundant_bytes = redundant_bytes.saturating_add(later_result.recorded_output_bytes);
            end += 1;
        }
        if end - index >= 2 && redundant_bytes >= 8192 {
            targets.push(make(
                Reason::ExcessiveWithinFileReading,
                (index..end).collect(),
            ));
            index = end;
        } else {
            index += 1;
        }
    }

    if episode.state == super::EpisodeState::Complete {
        let mut unrelated = Vec::new();
        for (index, read) in episode.reads.iter().enumerate() {
            if named_paths.contains(&normalize_read_path(&read.request.paths[0]))
                || read.request.paths[0].len() > 192
                || read.request.truncated
                || source_action(events, &read.request.reference_id)?.truncated
                || targets.iter().any(|target| {
                    target.read_indexes.iter().any(|target_index| {
                        episode.reads[*target_index].request.reference_id
                            == read.request.reference_id
                    })
                })
            {
                continue;
            }
            if purpose_before_request(episode, events, &read.request.reference_id)?.is_some_and(
                |purpose| {
                    purpose.truncated
                        || purpose.context_only
                        || !purpose.reference.stable
                        || purpose.text.len() > 192
                },
            ) {
                continue;
            }
            let Some(result) = read.result.as_ref() else {
                continue;
            };
            if result.status != JevReadStatus::Success
                || result.kind != JevReadResultKind::File
                || result.truncated
                || result.recorded_output_bytes < 8192
                || source_action(events, &result.reference_id)?.truncated
            {
                continue;
            }
            unrelated.push((index, result.recorded_output_bytes));
        }
        if let Some((index, _)) = unrelated
            .into_iter()
            .max_by_key(|(index, bytes)| (*bytes, std::cmp::Reverse(*index)))
        {
            targets.push(make(Reason::UnrelatedFiles, vec![index]));
        }
    }
    Ok(targets)
}

fn purpose_before_request<'a>(
    episode: &InvestigationEpisode,
    events: &'a [ContentAction],
    request_id: &str,
) -> Result<Option<&'a ContentAction>, JevError> {
    let request_position = episode
        .events
        .iter()
        .position(|index| events[*index].reference.id == request_id)
        .ok_or(JevError::InvalidCheckContext)?;
    Ok(episode.events[..request_position]
        .iter()
        .rev()
        .map(|index| &events[*index])
        .find(|event| event.kind == "assistant" && !event.text.trim().is_empty()))
}

fn normalize_read_path(path: &str) -> String {
    path.trim()
        .trim_start_matches("./")
        .replace('\\', "/")
        .to_lowercase()
}

fn explicitly_named_paths(task: &JevSharedRequestContext) -> BTreeSet<String> {
    fn visit(value: &Value, paths: &mut BTreeSet<String>) {
        match value {
            Value::String(text) => {
                for token in text.split(|character: char| {
                    character.is_whitespace()
                        || matches!(
                            character,
                            '\"' | '\'' | '`' | '(' | ')' | ',' | ';' | ':' | '='
                        )
                }) {
                    let token =
                        token.trim_matches(|character: char| matches!(character, '.' | '[' | ']'));
                    let path = normalize_read_path(token);
                    if path.contains('/')
                        && path
                            .rsplit('/')
                            .next()
                            .is_some_and(|name| name.contains('.'))
                    {
                        paths.insert(path);
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    visit(value, paths);
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    visit(value, paths);
                }
            }
            _ => {}
        }
    }
    let mut paths = BTreeSet::new();
    visit(&task.fields, &mut paths);
    paths
}

fn selected_snippets(
    window: &JevInputWindow,
    source: &[ContentAction],
) -> Result<(Vec<ReadSourceSnippet>, Option<ReadSourceSnippet>), JevError> {
    let mut snippets = Vec::new();
    let sample = if window.fields["read"].is_object() {
        selected_snippet(window, source, "read.sample.text", "/read/sample", true)?
    } else if window.fields.pointer("/repeat/header_sample").is_none() {
        None
    } else {
        selected_snippet(
            window,
            source,
            "repeat.header_sample.text",
            "/repeat/header_sample",
            true,
        )?
    };
    if let Some(sample) = sample {
        snippets.push(sample);
    }
    let purpose = if window.fields.get("purpose").is_none() {
        None
    } else {
        selected_snippet(window, source, "purpose.text", "/purpose", false)?
    };
    Ok((snippets, purpose))
}

fn selected_snippet(
    window: &JevInputWindow,
    source: &[ContentAction],
    evidence_part: &str,
    field_pointer: &str,
    file_sample: bool,
) -> Result<Option<ReadSourceSnippet>, JevError> {
    let Some(selected) = window.fields.pointer(field_pointer) else {
        return Err(JevError::InvalidCheckPlan);
    };
    if selected.is_null() {
        if window
            .evidence
            .iter()
            .any(|reference| reference.part_id == evidence_part)
        {
            return Err(JevError::InvalidCheckPlan);
        }
        return Ok(None);
    }
    let reference = window
        .evidence
        .iter()
        .find(|reference| reference.part_id == evidence_part)
        .ok_or(JevError::InvalidCheckPlan)?;
    let action = source_action(source, &reference.source_id)?;
    let range = selected
        .get("range")
        .and_then(Value::as_array)
        .filter(|range| range.len() == 2)
        .ok_or(JevError::InvalidCheckPlan)?;
    let start = range[0]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(JevError::InvalidCheckPlan)?;
    let end = range[1]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or(JevError::InvalidCheckPlan)?;
    let text = selected
        .get("text")
        .and_then(Value::as_str)
        .ok_or(JevError::InvalidCheckPlan)?;
    if start >= end || end - start != text.len() || action.text.get(start..end) != Some(text) {
        return Err(JevError::InvalidCheckPlan);
    }
    if file_sample {
        let content = recorded_file_content(action);
        let content_start = action
            .text
            .find(content)
            .ok_or(JevError::InvalidCheckPlan)?;
        if start != content_start
            || !content.starts_with(text)
            || text.len() > MAX_READ_SAMPLE_BYTES
        {
            return Err(JevError::InvalidCheckPlan);
        }
    } else if start != 0 || end != action.text.len() || action.truncated {
        return Err(JevError::InvalidCheckPlan);
    }
    Ok(Some(ReadSourceSnippet {
        reference: action.reference.clone(),
        kind: action.kind.clone(),
        timestamp_ms: action.timestamp_ms,
        range: (start, end),
        text: text.into(),
        source_digest: content_action_digest(action),
        partial: action.truncated || text.len() < action.text.len(),
    }))
}

fn compact_window(
    episode: &InvestigationEpisode,
    source: &[ContentAction],
    target: &Target,
    task: &JevSharedRequestContext,
    budget: usize,
) -> Result<JevInputWindow, JevError> {
    if !super::episodes::task_context_is_complete(task) {
        return Err(JevError::InvalidCheckContext);
    }
    let first_request_id = target
        .read_indexes
        .first()
        .and_then(|index| episode.reads.get(*index))
        .map(|read| read.request.reference_id.as_str())
        .ok_or(JevError::InvalidCheckPlan)?;
    let purpose_action = purpose_before_request(episode, source, first_request_id)?;
    if purpose_action.is_some_and(|action| {
        action.truncated
            || action.context_only
            || !action.reference.stable
            || action.text.len() > 192
    }) {
        return Err(JevError::InvalidCheckContext);
    }
    let purpose =
        purpose_action.map(|action| json!({"range": [0, action.text.len()], "text": action.text}));
    let mut evidence = task.evidence.clone();
    if let Some(action) = purpose_action {
        evidence.push(JevEvidenceReference {
            part_id: "purpose.text".into(),
            source_id: action.reference.id.clone(),
            content_kind: action.kind.clone(),
            role: JevEvidenceRole::SupportingContext,
        });
    }
    if target.reason == Reason::ExcessiveWithinFileReading && target.read_indexes.len() >= 2 {
        let reads = target
            .read_indexes
            .iter()
            .map(|index| {
                let read = episode
                    .reads
                    .get(*index)
                    .ok_or(JevError::InvalidCheckPlan)?;
                evidence.push(JevEvidenceReference {
                    part_id: format!("reads[{index}].request"),
                    source_id: read.request.reference_id.clone(),
                    content_kind: "tool_input".into(),
                    role: JevEvidenceRole::Candidate,
                });
                let result = read.result.as_ref().ok_or(JevError::InvalidCheckContext)?;
                evidence.push(JevEvidenceReference {
                    part_id: format!("reads[{index}].result"),
                    source_id: result.reference_id.clone(),
                    content_kind: "tool_result".into(),
                    role: JevEvidenceRole::Candidate,
                });
                Ok(result.recorded_output_bytes)
            })
            .collect::<Result<Vec<_>, JevError>>()?;
        let redundant_bytes = reads
            .iter()
            .skip(1)
            .copied()
            .fold(0_u64, u64::saturating_add);
        let first = &episode.reads[target.read_indexes[0]];
        let first_result = first.result.as_ref().ok_or(JevError::InvalidCheckContext)?;
        let first_result_action = source_action(source, &first_result.reference_id)?;
        let header_sample = file_sample(first_result_action, MAX_REPEAT_SAMPLE_BYTES)?;
        if header_sample.is_some() {
            evidence.push(JevEvidenceReference {
                part_id: "repeat.header_sample.text".into(),
                source_id: first_result.reference_id.clone(),
                content_kind: "tool_result".into(),
                role: JevEvidenceRole::Candidate,
            });
        }
        return Ok(JevInputWindow {
            fields: json!({
                "task": task.fields,
                "purpose": purpose,
                "repeat": {
                    "path": first.request.paths[0],
                    "extent": first_result.returned_extent,
                    "count": target.read_indexes.len(),
                    "redundant_output_bytes": redundant_bytes,
                    "complete_outputs_equal": true,
                    "earlier_result_available": true,
                    "complete_successful_file_results": true,
                    "intervening_work": false,
                    "header_sample": header_sample,
                    "sample_only": true,
                    "starts_at_file_beginning": super::episodes::read_starts_at_file_beginning(first_result),
                },
            }),
            evidence,
        });
    }

    if target.reason == Reason::UnrelatedFiles {
        let index = *target
            .read_indexes
            .first()
            .ok_or(JevError::InvalidCheckPlan)?;
        let read = episode.reads.get(index).ok_or(JevError::InvalidCheckPlan)?;
        let result = read.result.as_ref().ok_or(JevError::InvalidCheckContext)?;
        let action = source_action(source, &result.reference_id)?;
        let sample = file_sample(action, budget.min(MAX_READ_SAMPLE_BYTES))?;
        if budget != 0 && sample.is_none() {
            return Err(JevError::InvalidCheckContext);
        }
        evidence.push(JevEvidenceReference {
            part_id: "read.request".into(),
            source_id: read.request.reference_id.clone(),
            content_kind: "tool_input".into(),
            role: JevEvidenceRole::Candidate,
        });
        if sample.is_some() {
            evidence.push(JevEvidenceReference {
                part_id: "read.sample.text".into(),
                source_id: result.reference_id.clone(),
                content_kind: "tool_result".into(),
                role: JevEvidenceRole::Candidate,
            });
        }
        return Ok(JevInputWindow {
            fields: json!({
                "task": task.fields,
                "purpose": purpose,
                "read": {
                    "path": read.request.paths[0],
                    "returned_bytes": result.recorded_output_bytes,
                    "starts_at_file_beginning": super::episodes::read_starts_at_file_beginning(result),
                    "sample": sample,
                    "sample_only": true,
                },
            }),
            evidence,
        });
    }

    Err(JevError::InvalidCheckPlan)
}

fn file_sample(action: &ContentAction, maximum_bytes: usize) -> Result<Option<Value>, JevError> {
    let content = recorded_file_content(action);
    let sample = complete_leading_lines(content, maximum_bytes);
    if sample.is_empty() || !sample.chars().any(char::is_alphabetic) {
        return Ok(None);
    }
    let start = action
        .text
        .find(content)
        .ok_or(JevError::InvalidCheckContext)?;
    let end = start
        .checked_add(sample.len())
        .ok_or(JevError::InvalidCheckContext)?;
    Ok(Some(json!({"range": [start, end], "text": sample})))
}

fn source_action<'a>(source: &'a [ContentAction], id: &str) -> Result<&'a ContentAction, JevError> {
    source
        .iter()
        .find(|action| action.reference.id == id)
        .ok_or(JevError::InvalidCheckContext)
}

fn recorded_file_content(action: &ContentAction) -> &str {
    let text = action.text.as_str();
    if !action
        .metadata
        .read_result
        .as_ref()
        .and_then(|result| result.extent_contract.as_deref())
        .is_some_and(|contract| contract.starts_with("opencode/read.ts@"))
    {
        return text;
    }
    let Some((_, content)) = text.split_once("\n<type>file</type>\n<content>\n") else {
        return text;
    };
    let end = content.find("\n\n(End of file").unwrap_or(content.len());
    &content[..end]
}

fn complete_leading_lines(text: &str, maximum_bytes: usize) -> String {
    let mut end = 0_usize;
    for line in text.split_inclusive('\n') {
        if end.saturating_add(line.len()) > maximum_bytes {
            break;
        }
        end += line.len();
    }
    text[..end].to_owned()
}

fn judgment(
    result: &JevWorkItemResult,
    item: &JevWorkItem,
) -> Result<(SemanticOutcome, f64), JevError> {
    validate_jev_response(
        &JevResponse {
            model: result.model.clone(),
            answers: result.answers.clone(),
            usage: result.usage,
        },
        &JevRequest {
            model: result.model.clone(),
            state: Value::Null,
            questions: item.questions.clone(),
        },
    )?;
    let Some(JevAnswer::Choice {
        choice,
        probabilities,
        ..
    }) = result.answers.get(QUESTION_ID)
    else {
        return Err(JevError::ResponseAnswerTypeMismatch);
    };
    let probability = probabilities[choice];
    let unique_winner = probabilities
        .iter()
        .all(|(candidate, value)| candidate == choice || *value < probability);
    let outcome = if probability < SEMANTIC_PROBABILITY_THRESHOLD || !unique_winner {
        SemanticOutcome::Uncertain
    } else {
        match choice.as_str() {
            "likely_excess" => SemanticOutcome::LikelyExcess,
            "justified_or_minor" => SemanticOutcome::JustifiedOrMinor,
            "uncertain" => SemanticOutcome::Uncertain,
            _ => return Err(JevError::ResponseAnswerTypeMismatch),
        }
    };
    Ok((outcome, probability))
}

fn answer_id(work_item: &str) -> StableId {
    StableId::new(
        "over-exploring-answer-v3",
        &[work_item.as_bytes(), QUESTION_ID.as_bytes()],
    )
}
fn check_identity() -> StableId {
    StableId::new("smart-check", &[b"over_exploring"])
}

pub fn synchronize_sampling(
    plan: &JevCheckPlan<PreparedAssessment>,
    progress: &mut SamplingProgress,
) -> Result<(), SamplingError> {
    progress.synchronize(
        check_identity(),
        plan.prepared.epoch,
        &plan
            .prepared
            .candidates
            .iter()
            .map(|candidate| Candidate {
                id: candidate.candidate_id,
                required_answers: candidate.required_answers.clone(),
            })
            .collect::<Vec<_>>(),
    )
}

fn materialize(
    plan: &mut JevCheckPlan<PreparedAssessment>,
    selected: &BTreeSet<StableId>,
) -> Result<(), JevError> {
    plan.shared_context = None;
    plan.work_items.clear();
    plan.skipped_item_ids.clear();
    plan.prepared.unassessed.retain(|item| {
        item.work_item_id.is_none()
            || !matches!(
                item.limitation,
                Abstention::ContextTooLarge | Abstention::SampledEvidence
            )
    });
    for candidate in plan
        .prepared
        .candidates
        .iter()
        .filter(|candidate| selected.contains(&candidate.candidate_id))
    {
        for id in &candidate.work_item_ids {
            let target = &plan.prepared.targets[id];
            let episode = plan
                .prepared
                .episodes
                .iter()
                .find(|episode| episode.id == target.episode_id)
                .ok_or(JevError::InvalidCheckPlan)?;
            let window = match compact_window(
                episode,
                &plan.prepared.events,
                target,
                &plan.prepared.task_contexts[&target.episode_id],
                512,
            ) {
                Ok(window) => window,
                Err(JevError::InvalidCheckContext) => {
                    plan.skipped_item_ids.push(id.clone());
                    plan.prepared.unassessed.push(Unassessed {
                        episode_id: target.episode_id,
                        work_item_id: Some(id.clone()),
                        reason: Some(target.reason),
                        limitation: Abstention::SourceLimited,
                    });
                    continue;
                }
                Err(error) => return Err(error),
            };
            let item = JevWorkItem {
                id: id.clone(),
                window,
                questions: questions(target.reason),
            };
            let sampled = item.window.fields["read"]["sample_only"] == true;
            let task_available = !plan.prepared.task_contexts[&target.episode_id]
                .evidence
                .is_empty()
                && !item.window.fields["task"].is_null();
            if task_available {
                if sampled {
                    plan.prepared.unassessed.push(Unassessed {
                        episode_id: target.episode_id,
                        work_item_id: Some(id.clone()),
                        reason: Some(target.reason),
                        limitation: Abstention::SampledEvidence,
                    });
                }
                plan.work_items.push(item);
            } else {
                plan.skipped_item_ids.push(id.clone());
                plan.prepared.unassessed.push(Unassessed {
                    episode_id: target.episode_id,
                    work_item_id: Some(id.clone()),
                    reason: Some(target.reason),
                    limitation: if task_available {
                        Abstention::ContextTooLarge
                    } else {
                        Abstention::SourceLimited
                    },
                });
            }
        }
    }
    if let Some(task) = plan
        .work_items
        .first()
        .map(|item| item.window.fields["task"].clone())
        && plan.work_items.iter().all(|item| {
            item.window.fields["task"] == task
                && item
                    .window
                    .evidence
                    .iter()
                    .filter(|reference| reference.role == JevEvidenceRole::Instruction)
                    .eq(plan.work_items[0]
                        .window
                        .evidence
                        .iter()
                        .filter(|reference| reference.role == JevEvidenceRole::Instruction))
        })
    {
        plan.shared_context = Some(JevSharedRequestContext {
            fields: json!({"task": task}),
            evidence: plan.work_items[0]
                .window
                .evidence
                .iter()
                .filter(|reference| reference.role == JevEvidenceRole::Instruction)
                .cloned()
                .collect(),
        });
        for item in &mut plan.work_items {
            item.window
                .fields
                .as_object_mut()
                .expect("compact object")
                .remove("task");
            item.window
                .evidence
                .retain(|reference| reference.role != JevEvidenceRole::Instruction);
        }
    }
    if plan.work_items.len() == 1
        && plan.capabilities.uses_compact_requests()
        && plan.prepared.targets[&plan.work_items[0].id].reason
            == Reason::ExcessiveWithinFileReading
        && let Some(shared) = &mut plan.shared_context
        && let Some(task) = shared.fields.get_mut("task")
    {
        compact_task_fields(task);
    }
    while !plan.work_items.is_empty()
        && !snapshot_fits(
            &plan.work_items,
            &plan.capabilities,
            plan.shared_context.as_ref(),
        )?
    {
        if plan.work_items.len() == 1
            && let Some(window) = reduced_singleton_window(plan)?
        {
            plan.work_items[0] = window;
            if snapshot_fits(
                &plan.work_items,
                &plan.capabilities,
                plan.shared_context.as_ref(),
            )? {
                break;
            }
        }
        let item = plan.work_items.pop().expect("nonempty selection");
        let target = &plan.prepared.targets[&item.id];
        plan.prepared.unassessed.push(Unassessed {
            episode_id: target.episode_id,
            work_item_id: Some(item.id.clone()),
            reason: Some(target.reason),
            limitation: Abstention::ContextTooLarge,
        });
        plan.skipped_item_ids.push(item.id);
    }
    for candidate in &mut plan.prepared.candidates {
        candidate
            .work_item_ids
            .retain(|id| plan.work_items.iter().any(|item| &item.id == id));
        candidate.required_answers = candidate
            .work_item_ids
            .iter()
            .map(|id| answer_id(id))
            .collect();
    }
    plan.prepared
        .candidates
        .retain(|candidate| !candidate.work_item_ids.is_empty());
    plan.coverage = JevCoverage {
        selected_items: plan.work_items.len(),
        not_selected_items: plan
            .prepared
            .unassessed
            .iter()
            .filter(|item| {
                item.work_item_id.is_none() && item.limitation == Abstention::SampledEvidence
            })
            .count(),
        skipped_items: plan.skipped_item_ids.len(),
        limitations: plan
            .prepared
            .unassessed
            .iter()
            .map(|item| format!("{:?}", item.limitation))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        ..Default::default()
    };
    Ok(())
}

fn sample_budget(window: &JevInputWindow) -> usize {
    window.fields["read"]["sample"]["text"]
        .as_str()
        .map_or(MAX_READ_SAMPLE_BYTES, str::len)
}

fn reduced_singleton_window(
    plan: &JevCheckPlan<PreparedAssessment>,
) -> Result<Option<JevWorkItem>, JevError> {
    let item = &plan.work_items[0];
    let target = &plan.prepared.targets[&item.id];
    let episode = plan
        .prepared
        .episodes
        .iter()
        .find(|episode| episode.id == target.episode_id)
        .ok_or(JevError::InvalidCheckPlan)?;
    let mut required = item.clone();
    if target.reason == Reason::UnrelatedFiles {
        required.window = compact_window(
            episode,
            &plan.prepared.events,
            target,
            &plan.prepared.task_contexts[&target.episode_id],
            0,
        )?;
    }
    if target.reason == Reason::ExcessiveWithinFileReading
        && plan.capabilities.uses_compact_requests()
    {
        required.questions = compact_repeat_question();
        compact_repeat_window(&mut required.window);
    }
    if plan.shared_context.is_some() {
        required
            .window
            .fields
            .as_object_mut()
            .expect("compact object")
            .remove("task");
        required
            .window
            .evidence
            .retain(|reference| reference.role != JevEvidenceRole::Instruction);
    }
    if !snapshot_fits(
        std::slice::from_ref(&required),
        &plan.capabilities,
        plan.shared_context.as_ref(),
    )? {
        return Ok(None);
    }
    let packed = plan.pack(std::slice::from_ref(&required));
    let mut request = packed
        .batches
        .into_iter()
        .next()
        .ok_or(JevError::InvalidCheckPlan)?
        .request;
    let fits_request = request_fits(&request, &plan.capabilities)?;
    if !fits_request {
        return Ok(None);
    }
    if target.reason == Reason::ExcessiveWithinFileReading {
        return Ok(Some(required));
    }
    let result = episode.reads[target.read_indexes[0]]
        .result
        .as_ref()
        .ok_or(JevError::InvalidCheckPlan)?;
    let action = source_action(&plan.prepared.events, &result.reference_id)?;
    let content = recorded_file_content(action);
    let mut end = 0_usize;
    let mut accepted = 0;
    let mut accepted_sample = None;
    for line in content.split_inclusive('\n') {
        end = end.saturating_add(line.len());
        if end > MAX_READ_SAMPLE_BYTES {
            break;
        }
        let Some(sample) = file_sample(action, end)? else {
            continue;
        };
        required.window.fields["read"]["sample"] = sample.clone();
        request.state["work_items"][0]["context"] = required.window.fields.clone();
        if !candidate_state_fits(&required, plan.shared_context.as_ref())
            || !request_fits(&request, &plan.capabilities)?
        {
            break;
        }
        accepted = end;
        accepted_sample = Some(sample);
    }
    if accepted == 0 || accepted_sample.is_none() {
        return Ok(None);
    }
    required.window.fields["read"]["sample"] =
        accepted_sample.expect("accepted sample was selected");
    required.window.evidence.push(JevEvidenceReference {
        part_id: "read.sample.text".into(),
        source_id: result.reference_id.clone(),
        content_kind: "tool_result".into(),
        role: JevEvidenceRole::Candidate,
    });
    Ok(Some(required))
}

fn compact_repeat_question() -> BTreeMap<String, crate::analysis::jev::JevQuestion> {
    use crate::analysis::jev::JevQuestion;

    BTreeMap::from([(
        QUESTION_ID.into(),
        JevQuestion::Choice {
            instructions: json!({"question":"Did an earlier identical result meet task without a required repeat, comparison, or verification? Explicit needs justify repeats; unclear fit means uncertain."}),
            criteria: BTreeMap::from([
                (
                    "likely_excess".into(),
                    json!("Result met task; later fetches add nothing."),
                ),
                (
                    "justified_or_minor".into(),
                    json!("Task or purpose requires repeat, comparison, or verification."),
                ),
                (
                    "uncertain".into(),
                    json!("Task or purpose is unclear about reuse."),
                ),
            ]),
        },
    )])
}

fn compact_repeat_window(window: &mut JevInputWindow) {
    let fields = window.fields.as_object_mut().expect("compact object");
    fields["repeat"]
        .as_object_mut()
        .expect("repeat facts object")
        .remove("header_sample");
    fields["repeat"]
        .as_object_mut()
        .expect("repeat facts object")
        .remove("sample_only");
    window
        .evidence
        .retain(|reference| reference.part_id != "repeat.header_sample.text");
}

fn compact_task_fields(value: &mut Value) {
    let Some(fields) = value.as_object_mut() else {
        return;
    };
    if fields.get("antecedent_context_omitted") == Some(&Value::Bool(false)) {
        fields.remove("antecedent_context_omitted");
    }
    if fields.get("partial") == Some(&Value::Bool(false)) {
        fields.remove("partial");
    }
    if fields
        .get("limitations")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        fields.remove("limitations");
    }
}

fn compact_window_matches(canonical: &JevInputWindow, actual: &JevInputWindow) -> bool {
    let mut canonical = canonical.clone();
    if actual.fields.get("purpose").is_none() {
        canonical
            .fields
            .as_object_mut()
            .expect("compact object")
            .remove("purpose");
        canonical
            .evidence
            .retain(|reference| reference.part_id != "purpose.text");
    }
    if actual.fields["repeat"].is_object()
        && actual.fields.pointer("/repeat/header_sample").is_none()
    {
        canonical.fields["repeat"]
            .as_object_mut()
            .expect("repeat facts object")
            .remove("header_sample");
        canonical
            .evidence
            .retain(|reference| reference.part_id != "repeat.header_sample.text");
    }
    if actual.fields["repeat"].is_object() && actual.fields.pointer("/repeat/sample_only").is_none()
    {
        canonical.fields["repeat"]
            .as_object_mut()
            .expect("repeat facts object")
            .remove("sample_only");
    }
    canonical == *actual
}

fn candidate_state_fits(item: &JevWorkItem, shared: Option<&JevSharedRequestContext>) -> bool {
    let mut fields = item.window.fields.clone();
    if let Some(shared) = shared {
        fields["task"] = shared.fields["task"].clone();
    }
    serde_json::to_vec(&fields).is_ok_and(|bytes| bytes.len() <= MAX_CANDIDATE_STATE_BYTES)
}

fn request_fits(request: &JevRequest, capabilities: &ModelCapabilities) -> Result<bool, JevError> {
    crate::analysis::jev::request_fits_limits(
        request,
        capabilities,
        crate::analysis::jev::JevRequestLimits {
            request_bytes: MAX_PACKED_REQUEST_BYTES,
            estimated_input_tokens: MAX_ESTIMATED_INPUT_TOKENS,
        },
    )
}

fn snapshot_fits(
    items: &[JevWorkItem],
    capabilities: &ModelCapabilities,
    shared: Option<&JevSharedRequestContext>,
) -> Result<bool, JevError> {
    if items.iter().any(|item| !candidate_state_fits(item, shared)) {
        return Ok(false);
    }
    crate::analysis::jev::work_items_fit_one_request(
        items,
        capabilities,
        shared,
        crate::analysis::jev::JevRequestLimits {
            request_bytes: MAX_PACKED_REQUEST_BYTES,
            estimated_input_tokens: MAX_ESTIMATED_INPUT_TOKENS,
        },
    )
}

impl PreparedAssessment {
    pub fn validates_decision(&self, decision: &Decision) -> bool {
        let Some(target) = self.targets.get(&decision.work_item_id) else {
            return false;
        };
        let Some(episode) = self
            .episodes
            .iter()
            .find(|episode| episode.id == target.episode_id)
        else {
            return false;
        };
        let Some(task) = self.task_contexts.get(&target.episode_id) else {
            return false;
        };
        let Some(basis) = &decision.explanation else {
            return false;
        };
        self.history_complete
            && self.limitations.is_empty()
            && decision.has_valid_explanation()
            && decision.revisions == OverExploringCheck.revisions()
            && decision.reads == target.bindings
            && decision.reason == target.reason
            && decision.episode_id == target.episode_id
            && basis.task.fields == task.fields
            && basis.task.evidence
                == task
                    .evidence
                    .iter()
                    .filter(|reference| reference.role == JevEvidenceRole::Instruction)
                    .cloned()
                    .collect::<Vec<_>>()
            && validate_episode(episode, &self.events).is_ok()
            && episode_targets(episode, &self.events, &self.task_contexts)
                .is_ok_and(|targets| targets.contains(target))
            && basis.snippets.iter().all(|snippet| {
                self.events
                    .iter()
                    .any(|action| snippet.matches_action(action))
            })
            && basis.purpose.as_ref().is_none_or(|purpose| {
                self.events
                    .iter()
                    .any(|action| purpose.matches_action(action))
            })
            && [512, sample_budget(&basis.compared)]
                .into_iter()
                .any(|budget| {
                    compact_window(episode, &self.events, target, task, budget).is_ok_and(
                        |mut window| {
                            if basis.compared.fields.get("task").is_none() {
                                window
                                    .fields
                                    .as_object_mut()
                                    .expect("compact object")
                                    .remove("task");
                                window.evidence.retain(|reference| {
                                    reference.role != JevEvidenceRole::Instruction
                                });
                            }
                            compact_window_matches(&window, &basis.compared)
                                && decision.source_evidence
                                    == basis
                                        .compared
                                        .evidence
                                        .iter()
                                        .filter(|reference| {
                                            reference.role != JevEvidenceRole::Instruction
                                        })
                                        .cloned()
                                        .collect::<Vec<_>>()
                        },
                    )
                })
    }

    pub fn select_jobs(
        plan: &mut JevCheckPlan<Self>,
        jobs: &[SamplingJob],
    ) -> Result<(), JevError> {
        if jobs.len() > MAX_JOBS_PER_REQUEST {
            return Err(JevError::InvalidCheckPlan);
        }
        let mut selected = BTreeSet::new();
        for job in jobs {
            if job.check != check_identity()
                || job.epoch != plan.prepared.epoch
                || !plan
                    .prepared
                    .candidates
                    .iter()
                    .any(|candidate| candidate.candidate_id == job.candidate)
                || !selected.insert(job.candidate)
            {
                return Err(JevError::InvalidCheckPlan);
            }
        }
        materialize(plan, &selected)
    }
    /// Record valid answers, including uncertainty. Provider failures remain pending.
    pub fn record_completion(
        &self,
        result: &Assessment,
        job: &SamplingJob,
        progress: &mut SamplingProgress,
    ) -> Result<(), SamplingError> {
        if job.check != check_identity() || job.epoch != self.epoch || result.epoch != self.epoch {
            return Err(SamplingError::StaleJob);
        }
        let candidate = self
            .candidates
            .iter()
            .find(|candidate| candidate.candidate_id == job.candidate)
            .ok_or(SamplingError::StaleJob)?;
        if !candidate
            .work_item_ids
            .iter()
            .all(|id| result.completed_work_item_ids.contains(id))
        {
            return Err(SamplingError::IncompleteCandidate);
        }
        for answer in &candidate.required_answers {
            progress.record_reduced_answer(job, *answer)?;
        }
        progress.complete_candidate(job)
    }
}
