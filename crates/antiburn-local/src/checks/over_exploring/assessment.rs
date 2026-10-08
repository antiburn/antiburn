use super::episodes::{InvestigationEpisode, MAX_EPISODES, MAX_EVENTS, OverExploringInput};
use super::questions::{QUESTION_ID, questions};
use crate::analysis::jev::capabilities::ModelCapabilities;
use crate::analysis::jev::text_ranges::text_ranges;
use crate::analysis::jev::{
    JevAnswer, JevCheck, JevCheckPlan, JevCheckRevisions, JevCoverage, JevError,
    JevEvidenceReference, JevEvidenceRole, JevInputField, JevInputSelection, JevInputWindow,
    JevRequest, JevResponse, JevSessionContext, JevSharedRequestContext, JevWorkItem,
    JevWorkItemResult, pack_work_items_with_shared_context, validate_jev_response,
};
use crate::analysis::jev_evidence::{ContentAction, JevReadResultKind, JevReadStatus, JevReadUnit};
use crate::checks::sampling::{Candidate, SamplingError, SamplingJob, SamplingProgress, StableId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub const SEMANTIC_PROBABILITY_THRESHOLD: f64 = 0.75;
pub const MAX_TARGETS_PER_TURN: usize = 3;
pub const EVENT_RANGE_BYTES: usize = 2048;
pub const MAX_EVENT_RANGES: usize = 3;
pub const MAX_WINDOW_TEXT_BYTES: usize = 8192;
pub const MAX_SUPPORTING_EVENTS: usize = 16;
pub const MAX_SAMPLING_CANDIDATES: usize = 2 * MAX_EVENTS + MAX_EPISODES;

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
    UnknownObservedExtent,
    ContextTooLarge,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unassessed {
    pub episode_id: StableId,
    pub work_item_id: Option<String>,
    pub reason: Option<Reason>,
    pub limitation: Abstention,
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
            projection: 5,
            chunking: 6,
            questions: 11,
            reducer: 7,
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
            || input.task_context.evidence.is_empty()
            || input.events.len() > MAX_EVENTS
            || input.episodes.len() > MAX_EPISODES
        {
            return Err(JevError::InvalidCheckContext);
        }
        let epoch_bytes = serde_json::to_vec(&(&input.task_context, self.revisions()))
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
        let source_revision = StableId::new("over-exploring-source-v2", &[&source_bytes]);
        let mut seen = BTreeSet::new();
        let mut covered = BTreeSet::new();
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
                                context.evidence.is_empty() || context.fields["partial"] == true
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
            let episode_revision = StableId::new("over-exploring-source-v2", &[&episode_bytes]);
            for target in episode_targets(episode, &mut prepared.unassessed)? {
                let bytes = serde_json::to_vec(&(&target, episode_revision, epoch))
                    .map_err(|_| JevError::RequestSerialization)?;
                let candidate_id = StableId::new("over-exploring-target-v2", &[&bytes]);
                let id: String = candidate_id.into();
                prepared.candidates.push(SamplingCandidate {
                    candidate_id,
                    episode_id: episode.id,
                    work_item_ids: vec![id.clone()],
                    required_answers: vec![answer_id(&id)],
                });
                Arc::make_mut(&mut prepared.targets).insert(id, target);
            }
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
        let shared = plan
            .shared_context
            .as_ref()
            .ok_or(JevError::InvalidCheckPlan)?;
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
            let (outcome, probability) = judgment(result, item)?;
            outcomes.insert(item.id.clone(), outcome);
            // A valid uncertain answer is terminal, but never a clean result.
            assessment.completed_work_item_ids.push(item.id.clone());
            let target = plan
                .prepared
                .targets
                .get(&item.id)
                .ok_or(JevError::InvalidCheckPlan)?;
            match outcome {
                SemanticOutcome::LikelyExcess => assessment.findings.push(Decision {
                    episode_id: target.episode_id,
                    work_item_id: item.id.clone(),
                    reason: target.reason,
                    reads: target.bindings.clone(),
                    task_evidence: shared.evidence.clone(),
                    source_evidence: item.window.evidence.clone(),
                    semantic_revision: plan.input_revision.clone(),
                    model: plan.capabilities.model.clone(),
                    revisions: plan.revisions,
                    outcome,
                    probability,
                }),
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
        Ok(assessment)
    }
}

fn validate_episode(
    episode: &InvestigationEpisode,
    source: &[ContentAction],
) -> Result<(), JevError> {
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
    unassessed: &mut Vec<Unassessed>,
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
    let mut paths = BTreeMap::<(Option<&str>, &str), Vec<usize>>::new();
    for (index, read) in episode.reads.iter().enumerate() {
        if read.request.paths.len() != 1 || read.request.reference_id.is_empty() {
            return Err(JevError::InvalidCheckContext);
        }
        paths
            .entry((read.request.cwd.as_deref(), &read.request.paths[0]))
            .or_default()
            .push(index);
    }
    let mut targets = Vec::new();
    // Set-wide breadth comes first. Counts rank targets; they do not prove excess.
    if paths.len() > 1 {
        targets.push(make(
            Reason::ExcessiveFileBreadth,
            (0..episode.reads.len()).collect(),
        ));
    }
    for indexes in paths.values() {
        targets.push(make(Reason::UnrelatedFiles, indexes.clone()));
        let supported = indexes.iter().all(|index| {
            episode.reads[*index].result.as_ref().is_some_and(|result| {
                result.status == JevReadStatus::Success
                    && result.kind == JevReadResultKind::File
                    && result.returned_extent.as_ref().is_some_and(|extent| {
                        extent.unit != JevReadUnit::Unknown
                            && extent.offset.is_some()
                            && (extent.limit.is_some_and(|limit| limit > 0)
                                || extent
                                    .end_inclusive
                                    .zip(extent.offset)
                                    .is_some_and(|(end, start)| end >= start))
                    })
            })
        });
        if supported {
            targets.push(make(Reason::ExcessiveWithinFileReading, indexes.clone()));
        } else {
            unassessed.push(Unassessed {
                episode_id: episode.id,
                work_item_id: None,
                reason: Some(Reason::ExcessiveWithinFileReading),
                limitation: Abstention::UnknownObservedExtent,
            });
        }
    }
    Ok(targets)
}

fn selected_events(
    episode: &InvestigationEpisode,
    source: &[ContentAction],
    target: &Target,
) -> Result<BTreeSet<usize>, JevError> {
    let target_ids = target
        .bindings
        .iter()
        .flat_map(|binding| {
            std::iter::once(binding.request_id.as_str()).chain(binding.result_id.as_deref())
        })
        .collect::<BTreeSet<_>>();
    let targets = episode
        .events
        .iter()
        .copied()
        .filter(|index| target_ids.contains(source[*index].reference.id.as_str()))
        .collect::<BTreeSet<_>>();
    let first = *targets.first().ok_or(JevError::InvalidCheckContext)?;
    let mut supporting = BTreeSet::new();
    let (task_start, task_end) = task_bounds(source, first);
    for events in [&episode.before, &episode.events, &episode.subsequent] {
        supporting.extend(
            events
                .iter()
                .rev()
                .filter(|index| {
                    **index >= task_start && **index < task_end && !targets.contains(index)
                })
                .take(2)
                .copied(),
        );
    }
    let mut ranked = episode
        .before
        .iter()
        .chain(&episode.events)
        .chain(&episode.subsequent)
        .copied()
        .filter(|index| *index >= task_start && *index < task_end && !targets.contains(index))
        .collect::<Vec<_>>();
    ranked.sort_by_key(|index| {
        let event = &source[*index];
        let priority = if event.authority == "user" {
            0
        } else if event.metadata.read_request.is_some() || event.metadata.read_result.is_some() {
            1
        } else if event.kind == "tool_input" || event.kind == "tool_result" {
            2
        } else {
            3
        };
        let distance = targets
            .iter()
            .fold(index.abs_diff(first), |distance, target| {
                distance.min(index.abs_diff(*target))
            });
        (priority, distance, *index)
    });
    for index in ranked {
        if supporting.len() == MAX_SUPPORTING_EVENTS {
            break;
        }
        supporting.insert(index);
    }
    supporting.extend(targets);
    Ok(supporting)
}

fn task_bounds(source: &[ContentAction], first: usize) -> (usize, usize) {
    let is_task =
        |index: &usize| source[*index].kind == "user" && source[*index].authority == "user";
    let start = (0..=first).rev().find(is_task).unwrap_or(0);
    let end = (first + 1..source.len())
        .find(is_task)
        .unwrap_or(source.len());
    (start, end)
}

fn window(
    episode: &InvestigationEpisode,
    source: &[ContentAction],
    target: &Target,
    history_complete: bool,
) -> Result<JevInputWindow, JevError> {
    let mut evidence = Vec::new();
    let mut budget = MAX_WINDOW_TEXT_BYTES;
    let selected_sources = selected_events(episode, source, target)?;
    let first = selected_sources
        .iter()
        .copied()
        .find(|index| {
            target
                .bindings
                .iter()
                .any(|binding| binding.request_id == source[*index].reference.id)
        })
        .ok_or(JevError::InvalidCheckContext)?;
    let (task_start, task_end) = task_bounds(source, first);
    let mut fields = json!({"reason": target.reason, "target_read_indexes": target.read_indexes,
        "history_complete": history_complete, "episode_state": episode.state,
        "limits": {"event_range_bytes": EVENT_RANGE_BYTES, "max_event_ranges": MAX_EVENT_RANGES,
            "window_text_bytes": MAX_WINDOW_TEXT_BYTES, "max_supporting_events": MAX_SUPPORTING_EVENTS,
            "range_unit": "utf8_bytes", "end_exclusive": true},
        "event_selection": {"source_events": source.len(), "selected_events": selected_sources.len(),
            "task_events": task_end - task_start,
            "partial": (task_start..task_end).any(|index| !selected_sources.contains(&index))}});
    let is_target = |event: &ContentAction| {
        target.bindings.iter().any(|binding| {
            binding.request_id == event.reference.id
                || binding.result_id.as_deref() == Some(&event.reference.id)
        })
    };
    let mut selected = BTreeMap::new();
    // Target records consume the text budget before supporting context.
    for priority in [true, false] {
        for (key, events) in [
            ("events", &episode.events),
            ("before", &episode.before),
            ("subsequent", &episode.subsequent),
        ] {
            for (index, source_index) in events
                .iter()
                .filter(|index| selected_sources.contains(index))
                .enumerate()
            {
                let event = source
                    .get(*source_index)
                    .ok_or(JevError::InvalidCheckContext)?;
                if is_target(event) != priority {
                    continue;
                }
                let ranges = text_ranges(&event.text, EVENT_RANGE_BYTES, 0);
                let indexes: BTreeSet<_> = if ranges.len() <= MAX_EVENT_RANGES {
                    (0..ranges.len()).collect()
                } else {
                    [0, ranges.len() / 2, ranges.len() - 1]
                        .into_iter()
                        .collect()
                };
                let mut excerpts = Vec::new();
                let mut retained = 0;
                for range_index in indexes {
                    let (start, end) = ranges[range_index];
                    if end - start > budget {
                        continue;
                    }
                    budget -= end - start;
                    retained += end - start;
                    let part_id = format!("{key}[{index}].ranges[{}].text", excerpts.len());
                    excerpts
                        .push(json!({"start": start, "end": end, "text": &event.text[start..end]}));
                    evidence.push(JevEvidenceReference {
                        part_id,
                        source_id: event.reference.id.clone(),
                        content_kind: event.kind.clone(),
                        role: if priority {
                            JevEvidenceRole::Candidate
                        } else {
                            JevEvidenceRole::SupportingContext
                        },
                    });
                }
                selected.insert((key, index), json!({"source_index": source_index, "role": event.turn_role, "kind": event.kind,
                    "authority": event.authority, "tool": event.tool_name, "timestamp_ms": event.timestamp_ms,
                    "source_truncated": event.truncated, "source_bytes": event.text.len(),
                    "range_count": ranges.len(), "partial": retained < event.text.len(), "ranges": excerpts}));
            }
        }
    }
    for (key, events) in [
        ("before", &episode.before),
        ("events", &episode.events),
        ("subsequent", &episode.subsequent),
    ] {
        let count = events
            .iter()
            .filter(|index| selected_sources.contains(index))
            .count();
        let records = (0..count)
            .map(|index| {
                selected
                    .remove(&(key, index))
                    .ok_or(JevError::InvalidCheckContext)
            })
            .collect::<Result<Vec<_>, _>>()?;
        fields[key] = json!(records);
    }
    let event_indexes = episode
        .events
        .iter()
        .filter(|index| selected_sources.contains(index))
        .copied()
        .collect::<Vec<_>>();
    let reads: Vec<_> = episode.reads.iter().enumerate().filter(|(_, read)| {
        event_indexes.iter().any(|index| source[*index].reference.id == read.request.reference_id)
    }).map(|(index, read)| {
        let request_index = event_indexes.iter().position(|index| source[*index].reference.id == read.request.reference_id)
            .ok_or(JevError::InvalidCheckContext)?;
        let result_index = read.result.as_ref().and_then(|result| event_indexes.iter()
            .position(|index| source[*index].reference.id == result.reference_id));
        let request_source_index = event_indexes[request_index];
        let result_source_index = read.result.as_ref().and_then(|result| episode.events.iter().copied()
            .find(|index| source[*index].reference.id == result.reference_id));
        Ok(json!({"read_index": index, "is_target": target.read_indexes.contains(&index),
            "request_event_index": request_index, "result_event_index": result_index,
            "request_source_index": request_source_index, "result_source_index": result_source_index,
            "request_timestamp_ms": source[request_source_index].timestamp_ms,
            "result_timestamp_ms": result_source_index.and_then(|index| source[index].timestamp_ms),
            "result_content_selected": result_index.is_some(),
            "requested": {"paths": read.request.paths, "cwd": read.request.cwd, "extent": read.request.extent,
                "native_extent": read.request.native_extent, "truncated": read.request.truncated, "extent_contract": read.request.extent_contract},
            "observed": read.result.as_ref().map(|result| json!({"status": result.status, "kind": result.kind,
                "extent": result.returned_extent, "output_bytes": result.recorded_output_bytes,
                "truncated": result.truncated, "extent_contract": result.extent_contract}))}))
    }).collect::<Result<_, JevError>>()?;
    fields["reads"] = json!(reads);
    Ok(JevInputWindow { fields, evidence })
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
    let outcome = if probability < SEMANTIC_PROBABILITY_THRESHOLD {
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
        "over-exploring-answer-v2",
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
    let mut contexts = BTreeMap::new();
    let mut evidence = Vec::new();
    for candidate in plan
        .prepared
        .candidates
        .iter()
        .filter(|candidate| selected.contains(&candidate.candidate_id))
    {
        let context = plan
            .prepared
            .task_contexts
            .get(&candidate.episode_id)
            .ok_or(JevError::InvalidCheckPlan)?;
        if contexts
            .insert(candidate.episode_id, context.fields.clone())
            .is_none()
        {
            evidence.extend(context.evidence.clone());
        }
    }
    evidence.sort_by(|left, right| left.source_id.cmp(&right.source_id));
    evidence.dedup_by(|left, right| left.source_id == right.source_id);
    plan.shared_context = Some(JevSharedRequestContext {
        fields: if contexts.len() == 1 {
            contexts.into_values().next().expect("one task")
        } else {
            json!({"tasks": contexts})
        },
        evidence,
    });
    let shared = plan
        .shared_context
        .as_ref()
        .expect("selected task contexts");
    plan.work_items.clear();
    plan.skipped_item_ids.clear();
    plan.prepared.unassessed.retain(|item| {
        !matches!(
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
            let mut item = JevWorkItem {
                id: id.clone(),
                window: window(
                    episode,
                    &plan.prepared.events,
                    target,
                    plan.prepared.history_complete,
                )?,
                questions: questions(target.reason),
            };
            item.window.fields["source_limits"] = json!(plan.prepared.limitations);
            let sampled = item.window.fields["event_selection"]["partial"] == true
                || ["before", "events", "subsequent"].iter().any(|key| {
                    item.window.fields[*key]
                        .as_array()
                        .is_some_and(|events| events.iter().any(|event| event["partial"] == true))
                });
            let packed = pack_work_items_with_shared_context(
                std::slice::from_ref(&item),
                &plan.capabilities,
                shared,
            );
            let task_available = !plan.prepared.task_contexts[&target.episode_id]
                .evidence
                .is_empty();
            if packed.skipped_item_ids.is_empty() && task_available {
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
    plan.coverage = JevCoverage {
        selected_items: selected.len(),
        not_selected_items: plan.prepared.candidates.len() - selected.len(),
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

impl PreparedAssessment {
    pub fn select_jobs(
        plan: &mut JevCheckPlan<Self>,
        jobs: &[SamplingJob],
    ) -> Result<(), JevError> {
        if jobs.len() > MAX_TARGETS_PER_TURN {
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
