use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::analysis::jev::capabilities::ModelCapabilities;
use crate::analysis::jev::{
    JevAnswer, JevCheck, JevCheckPlan, JevCheckRevisions, JevCoverage, JevError,
    JevEvidenceReference, JevEvidenceRole, JevInputField, JevInputSelection, JevInputWindow,
    JevRequest, JevResponse, JevSessionContext, JevWorkItem, JevWorkItemResult,
    pack_work_items_with_shared_context, validate_jev_response,
};
use crate::analysis::jev_evidence::{ContentAction, JevReadResultKind, JevReadStatus, JevReadUnit};
use crate::checks::sampling::{Candidate, SamplingError, SamplingJob, SamplingProgress, StableId};

use super::episodes::{EpisodeState, InvestigationEpisode, OverExploringInput};
use super::questions::{GATES, questions};

/// Gate on the selected outcome probability. Choice confidence measures the
/// same distribution's concentration; it is not a second probability of truth.
pub const SEMANTIC_PROBABILITY_THRESHOLD: f64 = 0.90;
pub const WITHIN_FILE_SUBSTANTIAL_PROBABILITY_THRESHOLD: f64 = 0.70;
const MAX_PREPARED_BYTES: usize = 16 * 1024 * 1024;

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
    MissingReadResult,
    AmbiguousReadBinding,
    UnknownObservedExtent,
    TruncatedEvidence,
    ContextTooLarge,
    PreparationLimitReached,
    PartialAssessment,
    UncertainDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadBinding {
    pub request_id: String,
    pub result_id: String,
    pub output_digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub episode_id: StableId,
    pub work_item_id: String,
    pub reason: Reason,
    /// These reads support the bounded reason claim. A breadth finding does
    /// not assert that every cited read is unnecessary.
    pub reads: Vec<ReadBinding>,
    pub task_evidence: Vec<JevEvidenceReference>,
    pub semantic_revision: String,
    pub model: String,
    pub revisions: JevCheckRevisions,
    pub judgments: EvidenceJudgments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticOutcome {
    Supported,
    Justified,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceJudgments {
    pub relevance: SemanticOutcome,
    pub useful_information: SemanticOutcome,
    pub justified_breadth: SemanticOutcome,
    pub justified_extent: SemanticOutcome,
    pub later_use: SemanticOutcome,
    pub substantial: SemanticOutcome,
    pub sufficiency: SemanticOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unassessed {
    pub episode_id: StableId,
    #[serde(default)]
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
    pub targets: BTreeMap<String, Target>,
    pub candidates: Vec<SamplingCandidate>,
    pub unassessed: Vec<Unassessed>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SamplingCandidate {
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
            projection: 2,
            chunking: 1,
            questions: 7,
            reducer: 4,
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
        if input.session_identity != context.session_identity
            || input.task_context.evidence.is_empty()
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
            targets: BTreeMap::new(),
            candidates: Vec::new(),
            unassessed: Vec::new(),
        };
        let mut items = Vec::new();
        let mut skipped = Vec::new();
        let mut prepared_bytes = 0usize;
        let mut episodes = input.episodes.iter().collect::<Vec<_>>();
        // Read count ranks episodes. It does not establish any verdict.
        episodes.sort_by_key(|episode| std::cmp::Reverse(episode.reads.len()));
        let mut seen = BTreeSet::new();
        for episode in episodes {
            if !seen.insert(episode.id) {
                return Err(JevError::InvalidCheckContext);
            }
            if let Some(limitation) = episode_limit(episode, input.complete) {
                prepared.unassessed.push(Unassessed {
                    episode_id: episode.id,
                    work_item_id: None,
                    reason: None,
                    limitation,
                });
                continue;
            }
            let targets = episode_targets(episode, &mut prepared.unassessed)?;
            let mut episode_items = Vec::new();
            let mut episode_targets = BTreeMap::new();
            let mut episode_bytes = 0usize;
            let mut preparation_limit = false;
            for target in targets {
                let window = window(episode, &target)?;
                let questions = questions();
                let bytes = serde_json::to_vec(&(episode.id, &target, &window, &questions, epoch))
                    .map_err(|_| JevError::RequestSerialization)?;
                let id: String = StableId::new("over-exploring-work-v1", &[&bytes]).into();
                episode_bytes = episode_bytes
                    .saturating_add(bytes.len())
                    .saturating_add(id.len());
                if prepared_bytes.saturating_add(episode_bytes) > MAX_PREPARED_BYTES {
                    preparation_limit = true;
                    break;
                }
                episode_targets.insert(id.clone(), target);
                episode_items.push(JevWorkItem {
                    id,
                    window,
                    questions,
                });
            }
            if preparation_limit {
                prepared.unassessed.push(Unassessed {
                    episode_id: episode.id,
                    work_item_id: None,
                    reason: None,
                    limitation: Abstention::PreparationLimitReached,
                });
                continue;
            }
            let packed = pack_work_items_with_shared_context(
                &episode_items,
                capabilities,
                &input.task_context,
            );
            if !packed.skipped_item_ids.is_empty() {
                skipped.extend(episode_items.iter().map(|item| item.id.clone()));
                prepared.unassessed.push(Unassessed {
                    episode_id: episode.id,
                    work_item_id: None,
                    reason: None,
                    limitation: Abstention::ContextTooLarge,
                });
                continue;
            }
            if episode_items.is_empty() {
                continue;
            }
            let candidate = SamplingCandidate {
                episode_id: episode.id,
                work_item_ids: episode_items.iter().map(|item| item.id.clone()).collect(),
                required_answers: episode_items
                    .iter()
                    .flat_map(|item| GATES.map(|gate| answer_id(&item.id, gate)))
                    .collect(),
            };
            prepared.candidates.push(candidate);
            prepared_bytes += episode_bytes;
            prepared.targets.extend(episode_targets);
            items.extend(episode_items);
        }
        let coverage = JevCoverage {
            selected_items: prepared.candidates.len(),
            processing_limit_reached: prepared
                .unassessed
                .iter()
                .any(|item| item.limitation == Abstention::PreparationLimitReached),
            skipped_items: prepared
                .unassessed
                .iter()
                .map(|item| item.episode_id)
                .collect::<BTreeSet<_>>()
                .len(),
            limitations: prepared
                .unassessed
                .iter()
                .map(|item| format!("{:?}", item.limitation))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            ..Default::default()
        };
        Ok(JevCheckPlan {
            check_id: self.id().into(),
            input_revision: context.input_revision.clone(),
            revisions: self.revisions(),
            work_items: items,
            skipped_item_ids: skipped,
            coverage,
            capabilities: capabilities.clone(),
            shared_context: Some(input.task_context),
            prepared,
        })
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
                || result.answers.len() != GATES.len()
                || outcomes.contains_key(&item.id)
            {
                return Err(JevError::InvalidCheckPlan);
            }
            let reason = plan
                .prepared
                .targets
                .get(&item.id)
                .ok_or(JevError::InvalidCheckPlan)?
                .reason;
            let judgments = judgments(result, item, reason)?;
            outcomes.insert(item.id.clone(), (verdict(judgments, reason), judgments));
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
        let selected: BTreeSet<_> = plan.work_items.iter().map(|item| &item.id).collect();
        for candidate in &plan.prepared.candidates {
            if !candidate
                .work_item_ids
                .iter()
                .any(|id| selected.contains(id))
            {
                continue;
            }
            let start = assessment.findings.len();
            for id in &candidate.work_item_ids {
                let target = plan
                    .prepared
                    .targets
                    .get(id)
                    .ok_or(JevError::InvalidCheckPlan)?;
                match outcomes.get(id) {
                    Some((Verdict::Clean, _)) => continue,
                    Some((Verdict::Unknown, _)) | None => {
                        assessment.unassessed.push(Unassessed {
                            episode_id: candidate.episode_id,
                            work_item_id: Some(id.clone()),
                            reason: Some(target.reason),
                            limitation: if outcomes.contains_key(id) {
                                Abstention::UncertainDecision
                            } else {
                                Abstention::PartialAssessment
                            },
                        });
                        continue;
                    }
                    Some((Verdict::Finding, _)) => {}
                }
                assessment.findings.push(Decision {
                    episode_id: candidate.episode_id,
                    work_item_id: id.clone(),
                    reason: target.reason,
                    reads: target.bindings.clone(),
                    task_evidence: shared.evidence.clone(),
                    semantic_revision: plan.input_revision.clone(),
                    model: plan.capabilities.model.clone(),
                    revisions: plan.revisions,
                    judgments: outcomes[id].1,
                });
            }
            if !complete
                && !assessment
                    .unassessed
                    .iter()
                    .any(|item| item.episode_id == candidate.episode_id)
            {
                assessment.unassessed.push(Unassessed {
                    episode_id: candidate.episode_id,
                    work_item_id: None,
                    reason: None,
                    limitation: Abstention::PartialAssessment,
                });
            }
            // Unsupported extent remains unassessed, even when other reasons finish.
            if !assessment
                .unassessed
                .iter()
                .any(|item| item.episode_id == candidate.episode_id)
            {
                assessment.completed_episode_ids.push(candidate.episode_id);
                assessment
                    .completed_work_item_ids
                    .extend(candidate.work_item_ids.clone());
                if start == assessment.findings.len() {
                    assessment.clean_episode_ids.push(candidate.episode_id);
                }
            }
        }
        Ok(assessment)
    }
}

fn episode_limit(episode: &InvestigationEpisode, complete: bool) -> Option<Abstention> {
    if !complete {
        return Some(Abstention::IncompleteHistory);
    }
    if episode.state == EpisodeState::Deferred {
        return Some(Abstention::DeferredEpisode);
    }
    if episode
        .reads
        .iter()
        .any(|read| read.request.paths.len() != 1)
    {
        return Some(Abstention::AmbiguousReadBinding);
    }
    if episode
        .before
        .iter()
        .chain(&episode.events)
        .chain(&episode.subsequent)
        .any(|event| event.truncated)
    {
        return Some(Abstention::TruncatedEvidence);
    }
    if episode.reads.iter().any(|read| {
        read.request.truncated || read.result.as_ref().is_some_and(|result| result.truncated)
    }) {
        return Some(Abstention::TruncatedEvidence);
    }
    if episode.reads.iter().any(|read| {
        read.output.is_none()
            || read.result.as_ref().is_none_or(|result| {
                result.status != JevReadStatus::Success || result.kind != JevReadResultKind::File
            })
    }) {
        return Some(Abstention::MissingReadResult);
    }
    if episode.events.iter().any(|event| {
        event.metadata.read_result.as_ref().is_some_and(|result| {
            if episode.events.iter().any(|request| {
                request.context_only
                    && result.request_reference_id.as_deref() == Some(&request.reference.id)
            }) {
                return false;
            }
            !episode.reads.iter().any(|read| {
                read.result
                    .as_ref()
                    .is_some_and(|observed| observed.reference_id == result.reference_id)
            })
        })
    }) {
        return Some(Abstention::MissingReadResult);
    }
    None
}

fn episode_targets(
    episode: &InvestigationEpisode,
    unassessed: &mut Vec<Unassessed>,
) -> Result<Vec<Target>, JevError> {
    let make = |reason, indexes: Vec<usize>| -> Result<Target, JevError> {
        Ok(Target {
            episode_id: episode.id,
            reason,
            bindings: indexes
                .iter()
                .map(|index| {
                    let result = episode.reads[*index]
                        .result
                        .as_ref()
                        .ok_or(JevError::InvalidCheckContext)?;
                    Ok(ReadBinding {
                        request_id: episode.reads[*index].request.reference_id.clone(),
                        result_id: result.reference_id.clone(),
                        output_digest: result.recorded_output_digest.clone(),
                    })
                })
                .collect::<Result<_, JevError>>()?,
            read_indexes: indexes,
        })
    };
    let mut targets = Vec::new();
    let mut paths: BTreeMap<(Option<&str>, &str), Vec<usize>> = BTreeMap::new();
    for (index, read) in episode.reads.iter().enumerate() {
        targets.push(make(Reason::UnrelatedFiles, vec![index])?);
        let path = read
            .request
            .paths
            .first()
            .ok_or(JevError::InvalidCheckContext)?;
        paths
            .entry((read.request.cwd.as_deref(), path))
            .or_default()
            .push(index);
    }
    if paths.len() > 1 {
        targets.push(make(
            Reason::ExcessiveFileBreadth,
            (0..episode.reads.len()).collect(),
        )?);
    }
    for indexes in paths.values() {
        let supported = indexes.iter().all(|index| {
            episode.reads[*index]
                .result
                .as_ref()
                .and_then(|result| result.returned_extent.as_ref())
                .is_some_and(|extent| {
                    extent.unit != JevReadUnit::Unknown
                        && extent.offset.is_some()
                        && (extent.limit.is_some_and(|limit| limit > 0)
                            || extent
                                .end_inclusive
                                .zip(extent.offset)
                                .is_some_and(|(end, start)| end >= start))
                })
        });
        if supported {
            targets.push(make(Reason::ExcessiveWithinFileReading, indexes.clone())?);
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

fn event_fields(event: &ContentAction) -> Value {
    json!({"role": event.turn_role, "kind": event.kind, "authority": event.authority, "tool": event.tool_name, "text": event.text, "timestamp_ms": event.timestamp_ms})
}

fn window(episode: &InvestigationEpisode, target: &Target) -> Result<JevInputWindow, JevError> {
    let mut evidence = Vec::new();
    for (key, events) in [
        ("before", &episode.before),
        ("events", &episode.events),
        ("subsequent", &episode.subsequent),
    ] {
        for (index, event) in events.iter().enumerate() {
            evidence.push(JevEvidenceReference {
                part_id: format!("{key}[{index}]"),
                source_id: event.reference.id.clone(),
                content_kind: event.kind.clone(),
                role: if target.bindings.iter().any(|binding| {
                    binding.request_id == event.reference.id
                        || binding.result_id == event.reference.id
                }) {
                    JevEvidenceRole::Candidate
                } else {
                    JevEvidenceRole::SupportingContext
                },
            });
        }
    }
    let reads: Vec<_> = episode.reads.iter().enumerate().map(|(index, read)| {
        let result = read.result.as_ref().ok_or(JevError::InvalidCheckContext)?;
        let request_index = episode.events.iter().position(|event| event.reference.id == read.request.reference_id).ok_or(JevError::InvalidCheckContext)?;
        let result_index = episode.events.iter().position(|event| event.reference.id == result.reference_id).ok_or(JevError::InvalidCheckContext)?;
        Ok(json!({
            "read_index": index, "is_target": target.read_indexes.contains(&index),
            "request_event_index": request_index, "result_event_index": result_index,
            "requested": {"paths": read.request.paths, "cwd": read.request.cwd, "extent": read.request.extent, "native_extent": read.request.native_extent, "truncated": read.request.truncated, "extent_contract": read.request.extent_contract},
            "observed": {"status": result.status, "kind": result.kind, "extent": result.returned_extent, "output_bytes": result.recorded_output_bytes, "truncated": result.truncated, "extent_contract": result.extent_contract},
        }))
    }).collect::<Result<_, JevError>>()?;
    Ok(JevInputWindow {
        fields: json!({"reason": target.reason, "target_read_indexes": target.read_indexes, "reads": reads, "before": episode.before.iter().map(event_fields).collect::<Vec<_>>(), "events": episode.events.iter().map(event_fields).collect::<Vec<_>>(), "subsequent": episode.subsequent.iter().map(event_fields).collect::<Vec<_>>() }),
        evidence,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Finding,
    Clean,
    Unknown,
}

fn judgments(
    result: &JevWorkItemResult,
    item: &JevWorkItem,
    reason: Reason,
) -> Result<EvidenceJudgments, JevError> {
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
    let mut values = Vec::new();
    for gate in GATES {
        let Some(JevAnswer::Choice {
            choice,
            probabilities,
            ..
        }) = result.answers.get(gate)
        else {
            return Err(JevError::ResponseAnswerTypeMismatch);
        };
        let threshold = if reason == Reason::ExcessiveWithinFileReading && gate == "substantial" {
            WITHIN_FILE_SUBSTANTIAL_PROBABILITY_THRESHOLD
        } else {
            SEMANTIC_PROBABILITY_THRESHOLD
        };
        values.push(if probabilities[choice] < threshold {
            SemanticOutcome::Unknown
        } else {
            match choice.as_str() {
                "supported" => SemanticOutcome::Supported,
                "justified" => SemanticOutcome::Justified,
                "unknown" => SemanticOutcome::Unknown,
                _ => return Err(JevError::ResponseAnswerTypeMismatch),
            }
        });
    }
    Ok(EvidenceJudgments {
        relevance: values[0],
        useful_information: values[1],
        justified_breadth: values[2],
        justified_extent: values[3],
        later_use: values[4],
        substantial: values[5],
        sufficiency: values[6],
    })
}

fn verdict(judgments: EvidenceJudgments, reason: Reason) -> Verdict {
    use SemanticOutcome::{Justified, Supported, Unknown};
    if judgments.sufficiency != Supported {
        return Verdict::Unknown;
    }
    let reason_gates = match reason {
        Reason::UnrelatedFiles => vec![judgments.relevance],
        Reason::ExcessiveFileBreadth => vec![judgments.justified_breadth],
        Reason::ExcessiveWithinFileReading => vec![
            // Within-file excess requires relevant files, not unrelated work.
            match judgments.relevance {
                Justified => Supported,
                Supported => Justified,
                Unknown => Unknown,
            },
            judgments.justified_extent,
        ],
    };
    let required = reason_gates
        .into_iter()
        .chain([
            judgments.useful_information,
            judgments.later_use,
            judgments.substantial,
        ])
        .collect::<Vec<_>>();
    if required.contains(&Justified) {
        return Verdict::Clean;
    }
    if required.contains(&Unknown) {
        return Verdict::Unknown;
    }
    Verdict::Finding
}

fn answer_id(work_item: &str, gate: &str) -> StableId {
    StableId::new(
        "over-exploring-answer-v1",
        &[work_item.as_bytes(), gate.as_bytes()],
    )
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
                id: candidate.episode_id,
                required_answers: candidate.required_answers.clone(),
            })
            .collect::<Vec<_>>(),
    )
}

fn check_identity() -> StableId {
    StableId::new("smart-check", &[b"over_exploring"])
}

impl PreparedAssessment {
    /// Select jobs from the shared worker's bounded run. Do not start a run here.
    pub fn select_jobs(
        plan: &mut JevCheckPlan<Self>,
        jobs: &[SamplingJob],
    ) -> Result<(), JevError> {
        let mut selected = BTreeSet::new();
        for job in jobs {
            if job.check != check_identity()
                || job.epoch != plan.prepared.epoch
                || !plan
                    .prepared
                    .candidates
                    .iter()
                    .any(|candidate| candidate.episode_id == job.candidate)
                || !selected.insert(job.candidate)
            {
                return Err(JevError::InvalidCheckPlan);
            }
        }
        plan.work_items.retain(|item| {
            plan.prepared
                .targets
                .get(&item.id)
                .is_some_and(|target| selected.contains(&target.episode_id))
        });
        plan.coverage.selected_items = selected.len();
        plan.coverage.not_selected_items = plan.prepared.candidates.len() - selected.len();
        Ok(())
    }

    /// Commit an episode only after complete, certain reduction. Partial and
    /// extent-limited episodes retain their sampling gap.
    pub fn record_completion(
        &self,
        result: &Assessment,
        job: &SamplingJob,
        progress: &mut SamplingProgress,
    ) -> Result<(), SamplingError> {
        if job.check != check_identity() || job.epoch != self.epoch || result.epoch != self.epoch {
            return Err(SamplingError::StaleJob);
        }
        if !result.completed_episode_ids.contains(&job.candidate) {
            return Err(SamplingError::IncompleteCandidate);
        }
        let candidate = self
            .candidates
            .iter()
            .find(|candidate| candidate.episode_id == job.candidate)
            .ok_or(SamplingError::StaleJob)?;
        if !candidate
            .work_item_ids
            .iter()
            .all(|id| result.completed_work_item_ids.contains(id))
        {
            return Err(SamplingError::StaleJob);
        }
        for answer in &candidate.required_answers {
            progress.record_reduced_answer(job, *answer)?;
        }
        progress.complete_candidate(job)
    }
}
