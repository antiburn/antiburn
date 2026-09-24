//! Reusable TypeSafe request, context-chunk, and check contracts.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const PINNED_MODEL: &str = "jev-1.13.0";
pub const MAX_REQUEST_BYTES: usize = 8 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
pub const MAX_QUESTIONS_PER_REQUEST: usize = 128;
pub const DEFAULT_CONTEXT_CHUNK_BYTES: usize = 4 * 1024;
pub const MAX_REQUESTS_PER_ASSESSMENT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevRequestStage {
    Initial,
    Relevance,
    Reconciliation,
}

/// Sanitized failures shared by request preparation, TypeSafe transport, and
/// answer validation. Variants never contain credentials, request bodies, or
/// provider response text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JevError {
    RequestSerialization,
    EmptyQuestions,
    QuestionLimitExceeded,
    UnsupportedModel,
    RequestTooLarge { bytes: usize, maximum: usize },
    ResponseModelMismatch,
    ResponseAnswerCountMismatch,
    ResponseAnswerMissing,
    ResponseAnswerTypeMismatch,
    InvalidChoiceDistribution,
    InvalidNoulProbability,
    InvalidScoreDistribution,
    InvalidProbabilitySum,
    ChoiceNotMostProbable,
    WorkItemHasNoAnswers,
    AuthenticationRejected,
    InvalidRequestSchema,
    RateLimited { retry_after: Option<Duration> },
    ProviderOverloaded { retry_after: Option<Duration> },
    ProviderUnavailable,
    RequestOutcomeUnknown,
    ResponseTooLarge,
    ResponseDecode,
    ResponseUsageExceeded,
    UsageLimitReached,
    RequestLimitReached,
    Cancelled,
    InvalidCheckContext,
    InvalidCheckPlan,
}

impl fmt::Display for JevError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestSerialization => {
                formatter.write_str("Jev request could not be serialized")
            }
            Self::EmptyQuestions => formatter.write_str("Jev request has no questions"),
            Self::QuestionLimitExceeded => {
                formatter.write_str("Jev request exceeds the question limit")
            }
            Self::UnsupportedModel => formatter.write_str("Jev request uses an unsupported model"),
            Self::RequestTooLarge { bytes, maximum } => {
                write!(
                    formatter,
                    "Jev request is {bytes} bytes; the limit is {maximum} bytes"
                )
            }
            Self::ResponseModelMismatch => {
                formatter.write_str("Jev response model does not match the request")
            }
            Self::ResponseAnswerCountMismatch => {
                formatter.write_str("Jev response has an unexpected answer count")
            }
            Self::ResponseAnswerMissing => formatter.write_str("Jev response is missing an answer"),
            Self::ResponseAnswerTypeMismatch => {
                formatter.write_str("Jev response answer has the wrong type")
            }
            Self::InvalidChoiceDistribution => {
                formatter.write_str("Jev response has an invalid choice distribution")
            }
            Self::InvalidNoulProbability => {
                formatter.write_str("Jev response has an invalid Noul probability")
            }
            Self::InvalidScoreDistribution => {
                formatter.write_str("Jev response has an invalid score distribution")
            }
            Self::InvalidProbabilitySum => {
                formatter.write_str("Jev response probabilities do not sum to one")
            }
            Self::ChoiceNotMostProbable => {
                formatter.write_str("Jev response choice is not the most probable option")
            }
            Self::WorkItemHasNoAnswers => {
                formatter.write_str("Jev response has no answers for a work item")
            }
            Self::AuthenticationRejected => formatter.write_str("TypeSafe rejected the API key"),
            Self::InvalidRequestSchema => {
                formatter.write_str("TypeSafe rejected the request schema")
            }
            Self::RateLimited { .. } => formatter.write_str("TypeSafe rate-limited the request"),
            Self::ProviderOverloaded { .. } => {
                formatter.write_str("TypeSafe is temporarily overloaded")
            }
            Self::ProviderUnavailable => formatter.write_str("TypeSafe is unavailable"),
            Self::RequestOutcomeUnknown => {
                formatter.write_str("TypeSafe request outcome is unknown")
            }
            Self::ResponseTooLarge => {
                formatter.write_str("TypeSafe response exceeds the size limit")
            }
            Self::ResponseDecode => formatter.write_str("TypeSafe response could not be decoded"),
            Self::ResponseUsageExceeded => {
                formatter.write_str("TypeSafe response exceeded the usage limit")
            }
            Self::UsageLimitReached => formatter.write_str("Jev usage limit was reached"),
            Self::RequestLimitReached => formatter.write_str("Jev request limit was reached"),
            Self::Cancelled => formatter.write_str("Jev assessment was cancelled"),
            Self::InvalidCheckContext => formatter.write_str("Jev check context is invalid"),
            Self::InvalidCheckPlan => formatter.write_str("Jev check plan is invalid"),
        }
    }
}

impl std::error::Error for JevError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevRequest {
    pub model: String,
    pub state: Value,
    pub questions: BTreeMap<String, JevQuestion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum JevQuestion {
    Choice {
        instructions: Value,
        criteria: BTreeMap<String, Value>,
    },
    Noul {
        instructions: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<Value>,
    },
    Score {
        instructions: Value,
        criteria: Vec<Value>,
    },
}

impl JevQuestion {
    fn with_context_path(self, path: &str) -> Self {
        match self {
            Self::Choice {
                instructions,
                criteria,
            } => Self::Choice {
                instructions: contextual_instructions(instructions, path),
                criteria,
            },
            Self::Noul {
                instructions,
                criteria,
            } => Self::Noul {
                instructions: contextual_instructions(instructions, path),
                criteria,
            },
            Self::Score {
                instructions,
                criteria,
            } => Self::Score {
                instructions: contextual_instructions(instructions, path),
                criteria,
            },
        }
    }
}

fn contextual_instructions(instructions: Value, path: &str) -> Value {
    json!({
        "question": instructions,
        "context_path": format!("Use the evidence at `{path}`."),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevResponse {
    pub model: String,
    pub answers: BTreeMap<String, JevAnswer>,
    pub usage: JevUsage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum JevAnswer {
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Noul {
        noul: f64,
    },
    Score {
        score: f64,
        legend: BTreeMap<String, String>,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevContextUnit {
    pub event_id: String,
    pub branch_id: String,
    pub timestamp_ms: Option<i64>,
    pub context: Value,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevContextChunk {
    pub id: String,
    pub branch_id: String,
    pub event_ids: Vec<String>,
    /// Event IDs also present in the prior chunk. They identify overlap and
    /// must not count as independent evidence.
    pub overlap_event_ids: Vec<String>,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub state: Value,
    pub truncated_event_ids: Vec<String>,
    pub omitted_event_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevSessionContext {
    pub input_revision: String,
    pub session_identity: String,
    pub chunks: Vec<JevContextChunk>,
    /// Check-owned immutable material such as parsed instructions or a
    /// policy snapshot. It stays separate from generic session content.
    pub check_context: Value,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevWorkItem {
    pub id: String,
    pub context: Value,
    pub context_chunk_ids: Vec<String>,
    pub event_ids: Vec<String>,
    pub questions: BTreeMap<String, JevQuestion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevCheckPlan {
    pub check_id: String,
    pub input_revision: String,
    pub work_items: Vec<JevWorkItem>,
    pub skipped_item_ids: Vec<String>,
    pub coverage: JevCoverage,
    /// Check-owned immutable reducer state. The shared executor stores this
    /// with the job so it can resume and reduce after an application restart.
    pub check_data: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct JevCoverage {
    pub selected_items: usize,
    pub skipped_items: usize,
    pub not_selected_items: usize,
    pub processing_limit_reached: bool,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevWorkItemResult {
    pub request_id: String,
    pub work_item_id: String,
    pub answers: BTreeMap<String, JevAnswer>,
    pub model: String,
    pub usage: JevUsage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevRequestBatch {
    pub id: String,
    pub request: JevRequest,
    pub work_item_ids: Vec<String>,
    /// Maps returned question IDs to their owning work item and local ID.
    pub answer_owners: BTreeMap<String, (String, String)>,
    pub digest: String,
    pub serialized_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct JevPackingResult {
    pub batches: Vec<JevRequestBatch>,
    pub skipped_item_ids: Vec<String>,
}

/// A Jev-backed check defines policy while the shared executor defines
/// mechanics. Implementations prepare typed questions from the supplied
/// bounded session context, add a follow-up item only when the first answer
/// needs cross-chunk evidence, and reduce typed results into their own
/// serializable result. They do not own HTTP, credentials, retries, request
/// packing, limits, persistence, or scheduling.
pub trait JevCheck {
    type Result: Serialize;

    fn id(&self) -> &'static str;

    fn prepare(&self, context: &JevSessionContext) -> Result<JevCheckPlan, JevError>;

    /// Return a bounded follow-up work item when an initial result needs
    /// cross-chunk context. The shared runner counts it against the same caps.
    fn reconcile(
        &self,
        _work_item: &JevWorkItem,
        _initial_result: &JevWorkItemResult,
        _context: &JevSessionContext,
    ) -> Result<Option<JevWorkItem>, JevError> {
        Ok(None)
    }

    fn reduce(
        &self,
        plan: &JevCheckPlan,
        results: &[JevWorkItemResult],
        complete: bool,
    ) -> Result<Self::Result, JevError>;
}

/// Split ordered evidence into bounded context windows. Branches never share
/// a chunk. One prior event overlaps when it fits; overlap IDs remain explicit.
pub fn chunk_context_units(
    units: &[JevContextUnit],
    max_bytes: usize,
    overlap_units: usize,
) -> Vec<JevContextChunk> {
    let max_bytes = max_bytes.clamp(256, MAX_REQUEST_BYTES);
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < units.len() {
        let branch_id = units[start].branch_id.clone();
        let mut selected = Vec::new();
        let mut selected_bytes = 0usize;
        let mut cursor = start;
        while cursor < units.len() && units[cursor].branch_id == branch_id {
            let serialized = serde_json::to_vec(&units[cursor].context).unwrap_or_default();
            let size = serialized.len();
            if size > max_bytes {
                cursor += 1;
                continue;
            }
            if !selected.is_empty() && selected_bytes.saturating_add(size) > max_bytes {
                break;
            }
            selected.push(cursor);
            selected_bytes = selected_bytes.saturating_add(size);
            cursor += 1;
        }
        let previous_overlap: Vec<usize> = if chunks
            .last()
            .is_some_and(|chunk: &JevContextChunk| chunk.branch_id == branch_id)
        {
            chunks
                .last()
                .map(|chunk| {
                    chunk
                        .event_ids
                        .iter()
                        .rev()
                        .take(overlap_units)
                        .filter_map(|id| units.iter().position(|unit| &unit.event_id == id))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        for overlap_index in previous_overlap.iter().rev().copied() {
            if !selected.contains(&overlap_index) {
                let size = serde_json::to_vec(&units[overlap_index].context)
                    .map(|serialized| serialized.len())
                    .unwrap_or_default();
                if selected_bytes.saturating_add(size) <= max_bytes {
                    selected.insert(0, overlap_index);
                    selected_bytes = selected_bytes.saturating_add(size);
                }
            }
        }
        let overlap_ids: BTreeSet<String> = previous_overlap
            .iter()
            .map(|index| units[*index].event_id.clone())
            .collect();
        let event_ids: Vec<_> = selected
            .iter()
            .map(|index| units[*index].event_id.clone())
            .collect();
        let truncated_event_ids: Vec<_> = selected
            .iter()
            .filter(|index| units[**index].truncated)
            .map(|index| units[*index].event_id.clone())
            .collect();
        let omitted_event_ids: Vec<_> = (start..cursor)
            .filter(|index| {
                serde_json::to_vec(&units[*index].context)
                    .map(|serialized| serialized.len() > max_bytes)
                    .unwrap_or(true)
            })
            .map(|index| units[index].event_id.clone())
            .collect();
        let timestamps: Vec<_> = selected
            .iter()
            .filter_map(|index| units[*index].timestamp_ms)
            .collect();
        let state = json!({
            "events": selected.iter().map(|index| &units[*index].context).collect::<Vec<_>>(),
            "event_ids": event_ids,
            "overlap_event_ids": overlap_ids,
            "truncated_event_ids": truncated_event_ids,
        });
        let identity = format!("{branch_id}\0{}", event_ids.join("\0"));
        chunks.push(JevContextChunk {
            id: digest_hex(identity.as_bytes()),
            branch_id: branch_id.clone(),
            event_ids,
            overlap_event_ids: overlap_ids.into_iter().collect(),
            start_ms: timestamps.iter().min().copied(),
            end_ms: timestamps.iter().max().copied(),
            state,
            truncated_event_ids,
            omitted_event_ids,
        });
        if cursor <= start {
            start += 1;
        } else {
            start = cursor;
        }
    }
    chunks
}

/// Pack check-owned work items into requests under the full serialized-byte
/// ceiling. Each question receives an explicit path to its item context.
pub fn pack_work_items(items: &[JevWorkItem]) -> JevPackingResult {
    let mut result = JevPackingResult::default();
    let mut current: Vec<JevWorkItem> = Vec::new();
    for item in items {
        if item.questions.is_empty() {
            result.skipped_item_ids.push(item.id.clone());
            continue;
        }
        let mut candidate = current.clone();
        candidate.push(item.clone());
        if candidate.len() > MAX_QUESTIONS_PER_REQUEST || build_batch(&candidate).is_none() {
            if !current.is_empty() {
                if let Some(batch) = build_batch(&current) {
                    result.batches.push(batch);
                }
                current.clear();
            }
            if build_batch(std::slice::from_ref(item)).is_some() {
                current.push(item.clone());
            } else {
                result.skipped_item_ids.push(item.id.clone());
            }
        } else {
            current = candidate;
        }
    }
    if !current.is_empty()
        && let Some(batch) = build_batch(&current)
    {
        result.batches.push(batch);
    }
    result
}

/// Map one validated batch response back to check-owned work items.
pub fn unpack_jev_response(
    batch: &JevRequestBatch,
    response: &JevResponse,
) -> Result<Vec<JevWorkItemResult>, JevError> {
    validate_jev_response(response, &batch.request)?;
    let mut results = Vec::with_capacity(batch.work_item_ids.len());
    for work_item_id in &batch.work_item_ids {
        let answers = batch
            .answer_owners
            .iter()
            .filter(|(_, (owner, _))| owner == work_item_id)
            .filter_map(|(remote_id, (_, local_id))| {
                response
                    .answers
                    .get(remote_id)
                    .map(|answer| (local_id.clone(), answer.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        if answers.is_empty() {
            return Err(JevError::WorkItemHasNoAnswers);
        }
        results.push(JevWorkItemResult {
            request_id: batch.id.clone(),
            work_item_id: work_item_id.clone(),
            answers,
            model: response.model.clone(),
            usage: response.usage,
        });
    }
    Ok(results)
}

fn build_batch(items: &[JevWorkItem]) -> Option<JevRequestBatch> {
    let mut state_items = Vec::with_capacity(items.len());
    let mut questions = BTreeMap::new();
    let mut answer_owners = BTreeMap::new();
    for (index, item) in items.iter().enumerate() {
        state_items.push(json!({"id": item.id, "context": item.context}));
        for (question_id, question) in &item.questions {
            let response_id = format!(
                "q_{}",
                digest_hex(format!("{}\0{}", item.id, question_id).as_bytes())
            );
            questions.insert(
                response_id.clone(),
                question
                    .clone()
                    .with_context_path(&format!("work_items[{index}].context")),
            );
            answer_owners.insert(response_id, (item.id.clone(), question_id.clone()));
        }
    }
    if questions.len() > MAX_QUESTIONS_PER_REQUEST {
        return None;
    }
    let request = JevRequest {
        model: PINNED_MODEL.to_owned(),
        state: json!({"work_items": state_items}),
        questions,
    };
    let serialized = serde_json::to_vec(&request).ok()?;
    if serialized.len() > MAX_REQUEST_BYTES {
        return None;
    }
    let digest = digest_hex(&serialized);
    let id = digest.clone();
    Some(JevRequestBatch {
        id,
        request,
        work_item_ids: items.iter().map(|item| item.id.clone()).collect(),
        answer_owners,
        digest,
        serialized_bytes: serialized.len(),
    })
}

/// Validate the shared typed HTTP contract before storing a response.
pub fn validate_jev_response(response: &JevResponse, request: &JevRequest) -> Result<(), JevError> {
    if response.model != request.model {
        return Err(JevError::ResponseModelMismatch);
    }
    if response.answers.len() != request.questions.len() {
        return Err(JevError::ResponseAnswerCountMismatch);
    }
    for (id, question) in &request.questions {
        let answer = response
            .answers
            .get(id)
            .ok_or(JevError::ResponseAnswerMissing)?;
        match (question, answer) {
            (
                JevQuestion::Choice { criteria, .. },
                JevAnswer::Choice {
                    choice,
                    probabilities,
                    confidence,
                },
            ) => {
                if !criteria.contains_key(choice)
                    || probabilities.len() != criteria.len()
                    || probabilities.keys().any(|key| !criteria.contains_key(key))
                    || probabilities
                        .values()
                        .any(|value| !valid_probability(*value))
                    || !valid_probability(*confidence)
                {
                    return Err(JevError::InvalidChoiceDistribution);
                }
                validate_probability_sum(probabilities.values().copied())?;
                let maximum = probabilities.values().copied().fold(0.0, f64::max);
                if probabilities.get(choice).copied().unwrap_or_default() + 0.000_001 < maximum {
                    return Err(JevError::ChoiceNotMostProbable);
                }
            }
            (JevQuestion::Noul { .. }, JevAnswer::Noul { noul }) => {
                if !valid_probability(*noul) {
                    return Err(JevError::InvalidNoulProbability);
                }
            }
            (
                JevQuestion::Score { criteria, .. },
                JevAnswer::Score {
                    score,
                    legend,
                    probabilities,
                    confidence,
                },
            ) => {
                if !score.is_finite()
                    || !valid_probability(*confidence)
                    || legend.len() != criteria.len()
                    || probabilities.len() != criteria.len()
                    || probabilities
                        .values()
                        .any(|value| !valid_probability(*value))
                {
                    return Err(JevError::InvalidScoreDistribution);
                }
                validate_probability_sum(probabilities.values().copied())?;
            }
            _ => return Err(JevError::ResponseAnswerTypeMismatch),
        }
    }
    Ok(())
}

pub fn validate_jev_request(request: &JevRequest) -> Result<usize, JevError> {
    if request.model != PINNED_MODEL {
        return Err(JevError::UnsupportedModel);
    }
    if request.questions.is_empty() {
        return Err(JevError::EmptyQuestions);
    }
    if request.questions.len() > MAX_QUESTIONS_PER_REQUEST {
        return Err(JevError::QuestionLimitExceeded);
    }
    let bytes = serde_json::to_vec(request).map_err(|_| JevError::RequestSerialization)?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(JevError::RequestTooLarge {
            bytes: bytes.len(),
            maximum: MAX_REQUEST_BYTES,
        });
    }
    Ok(bytes.len())
}

/// Durable, source-free progress for one generic Burn Check assessment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct JevRunProgress {
    pub input_revision: String,
    pub results: BTreeMap<String, JevWorkItemResult>,
    pub completed_request_digests: BTreeSet<String>,
    pub failed_item_ids: BTreeSet<String>,
    pub request_count: usize,
}

/// One complete or partial reduction from the shared check runner.
#[derive(Debug, Clone, PartialEq)]
pub struct JevExecutionOutcome<R> {
    pub result: R,
    pub progress: JevRunProgress,
    pub complete: bool,
    pub failure: Option<JevError>,
}

/// Execute initial comparisons, check-owned reconciliation, then one
/// deterministic whole-scope reduction. The caller owns transport, cache,
/// usage reservations, cancellation, and durable storage.
pub async fn run_jev_check<C, E, Fut, S>(
    check: &C,
    context: &JevSessionContext,
    mut progress: JevRunProgress,
    mut execute: E,
    mut save_progress: S,
) -> Result<JevExecutionOutcome<C::Result>, JevError>
where
    C: JevCheck,
    E: FnMut(JevRequestBatch) -> Fut,
    Fut: std::future::Future<Output = Result<JevResponse, JevError>>,
    S: FnMut(&JevRunProgress) -> Result<(), JevError>,
{
    let mut plan = check.prepare(context)?;
    if plan.check_id != check.id() || plan.input_revision != context.input_revision {
        return Err(JevError::InvalidCheckPlan);
    }
    if progress.input_revision != plan.input_revision {
        progress = JevRunProgress {
            input_revision: plan.input_revision.clone(),
            ..JevRunProgress::default()
        };
    }
    progress.failed_item_ids.clear();
    let mut failure = None;
    let mut complete = true;

    let initial = pack_work_items(&plan.work_items);
    if !initial.skipped_item_ids.is_empty() {
        complete = false;
        plan.coverage.skipped_items += initial.skipped_item_ids.len();
        plan.coverage
            .limitations
            .push("request_packing_limit".to_owned());
        progress
            .failed_item_ids
            .extend(initial.skipped_item_ids.iter().cloned());
    }
    for batch in initial.batches {
        if progress.completed_request_digests.contains(&batch.digest) {
            continue;
        }
        if progress.request_count >= MAX_REQUESTS_PER_ASSESSMENT {
            failure = Some(JevError::RequestLimitReached);
            complete = false;
            progress.failed_item_ids.extend(batch.work_item_ids);
            break;
        }
        let response = match execute(batch.clone()).await {
            Ok(response) => response,
            Err(error) => {
                complete = false;
                progress.failed_item_ids.extend(batch.work_item_ids);
                failure = Some(error);
                break;
            }
        };
        let results = unpack_jev_response(&batch, &response)?;
        for result in results {
            progress.results.insert(result.work_item_id.clone(), result);
        }
        progress.completed_request_digests.insert(batch.digest);
        progress.request_count = progress.request_count.saturating_add(1);
        save_progress(&progress)?;
    }

    let mut reconciliation_items = Vec::new();
    if failure.is_none() {
        for work_item in &plan.work_items {
            let Some(initial_result) = progress.results.get(&work_item.id) else {
                complete = false;
                progress.failed_item_ids.insert(work_item.id.clone());
                continue;
            };
            if let Some(reconciliation) = check.reconcile(work_item, initial_result, context)? {
                reconciliation_items.push(reconciliation);
            }
        }
    } else {
        complete = false;
    }

    let reconciliation = pack_work_items(&reconciliation_items);
    if !reconciliation.skipped_item_ids.is_empty() {
        complete = false;
        plan.coverage.skipped_items += reconciliation.skipped_item_ids.len();
        plan.coverage
            .limitations
            .push("reconciliation_packing_limit".to_owned());
        progress
            .failed_item_ids
            .extend(reconciliation.skipped_item_ids.iter().cloned());
    }
    for batch in reconciliation.batches {
        if progress.completed_request_digests.contains(&batch.digest) {
            continue;
        }
        if progress.request_count >= MAX_REQUESTS_PER_ASSESSMENT {
            failure = Some(JevError::RequestLimitReached);
            complete = false;
            progress.failed_item_ids.extend(batch.work_item_ids);
            break;
        }
        let response = match execute(batch.clone()).await {
            Ok(response) => response,
            Err(error) => {
                complete = false;
                progress.failed_item_ids.extend(batch.work_item_ids);
                failure = Some(error);
                break;
            }
        };
        let results = unpack_jev_response(&batch, &response)?;
        for result in results {
            progress.results.insert(result.work_item_id.clone(), result);
        }
        progress.completed_request_digests.insert(batch.digest);
        progress.request_count = progress.request_count.saturating_add(1);
        save_progress(&progress)?;
    }

    for item in &plan.work_items {
        if !progress.results.contains_key(&item.id) {
            progress.failed_item_ids.insert(item.id.clone());
        }
    }
    for item in &reconciliation_items {
        if !progress.results.contains_key(&item.id) {
            progress.failed_item_ids.insert(item.id.clone());
        }
    }
    if !progress.failed_item_ids.is_empty() || failure.is_some() {
        complete = false;
    }
    plan.coverage.processing_limit_reached |= matches!(
        failure,
        Some(JevError::RequestLimitReached | JevError::UsageLimitReached)
    );
    if !complete {
        plan.coverage
            .limitations
            .push("assessment_incomplete".to_owned());
        plan.coverage.limitations.sort();
        plan.coverage.limitations.dedup();
    }
    let results = progress.results.values().cloned().collect::<Vec<_>>();
    let result = check.reduce(&plan, &results, complete)?;
    Ok(JevExecutionOutcome {
        result,
        progress,
        complete,
        failure,
    })
}

fn digest_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn valid_probability(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

fn validate_probability_sum(values: impl Iterator<Item = f64>) -> Result<(), JevError> {
    let sum = values.sum::<f64>();
    if (sum - 1.0).abs() > 0.02 {
        Err(JevError::InvalidProbabilitySum)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice() -> JevQuestion {
        JevQuestion::Choice {
            instructions: json!("Which option applies?"),
            criteria: BTreeMap::from([
                ("yes".to_owned(), json!("Yes")),
                ("no".to_owned(), json!("No")),
            ]),
        }
    }

    fn item(id: &str, text: &str) -> JevWorkItem {
        JevWorkItem {
            id: id.to_owned(),
            context: json!({"text": text}),
            context_chunk_ids: vec!["chunk".to_owned()],
            event_ids: vec![id.to_owned()],
            questions: BTreeMap::from([("q".to_owned(), choice())]),
        }
    }

    #[test]
    fn request_packing_keeps_questions_with_their_context_and_stays_bounded() {
        let items = vec![item("one", "small evidence"), item("two", "other evidence")];
        let packed = pack_work_items(&items);
        assert!(packed.skipped_item_ids.is_empty());
        assert_eq!(packed.batches.len(), 1);
        let batch = &packed.batches[0];
        assert!(batch.serialized_bytes <= MAX_REQUEST_BYTES);
        assert_eq!(batch.work_item_ids, vec!["one", "two"]);
        assert_eq!(batch.answer_owners.len(), 2);
        let instructions = &batch.request.questions.values().next().unwrap();
        assert!(
            matches!(instructions, JevQuestion::Choice { instructions, .. } if instructions.to_string().contains("work_items["))
        );
    }

    #[test]
    fn oversized_work_items_are_marked_skipped_instead_of_truncated() {
        let oversized = item("large", &"x".repeat(MAX_REQUEST_BYTES * 2));
        let packed = pack_work_items(&[oversized]);
        assert!(packed.batches.is_empty());
        assert_eq!(packed.skipped_item_ids, vec!["large"]);
    }

    #[test]
    fn context_chunking_preserves_order_branch_boundaries_and_overlap_ids() {
        let units = (0..5)
            .map(|index| JevContextUnit {
                event_id: format!("event-{index}"),
                branch_id: if index == 4 { "other" } else { "main" }.to_owned(),
                timestamp_ms: Some(index),
                context: json!({"text": "x".repeat(120)}),
                truncated: false,
            })
            .collect::<Vec<_>>();
        let chunks = chunk_context_units(&units, 400, 1);
        assert!(chunks.len() >= 3);
        assert_eq!(chunks[0].event_ids[0], "event-0");
        assert!(chunks.iter().any(|chunk| chunk.branch_id == "other"));
        for chunk in chunks
            .iter()
            .filter(|chunk| chunk.branch_id == "main")
            .skip(1)
        {
            assert!(!chunk.overlap_event_ids.is_empty());
            assert!(
                chunk
                    .overlap_event_ids
                    .iter()
                    .all(|event| chunk.event_ids.contains(event))
            );
        }
    }

    #[test]
    fn response_validation_checks_answer_type_distribution_and_model() {
        let request = JevRequest {
            model: PINNED_MODEL.to_owned(),
            state: json!({"text": "sample"}),
            questions: BTreeMap::from([("q".to_owned(), choice())]),
        };
        let response = JevResponse {
            model: PINNED_MODEL.to_owned(),
            answers: BTreeMap::from([(
                "q".to_owned(),
                JevAnswer::Choice {
                    choice: "yes".to_owned(),
                    probabilities: BTreeMap::from([
                        ("yes".to_owned(), 0.9),
                        ("no".to_owned(), 0.1),
                    ]),
                    confidence: 0.8,
                },
            )]),
            usage: JevUsage {
                input_tokens: 10,
                output_tokens: 2,
            },
        };
        assert_eq!(validate_jev_response(&response, &request), Ok(()));
        let mut invalid = response;
        invalid.model = "jev-latest".to_owned();
        assert_eq!(
            validate_jev_response(&invalid, &request),
            Err(JevError::ResponseModelMismatch)
        );
    }

    struct ResumeCheck;

    impl JevCheck for ResumeCheck {
        type Result = Value;

        fn id(&self) -> &'static str {
            "resume_test"
        }

        fn prepare(&self, context: &JevSessionContext) -> Result<JevCheckPlan, JevError> {
            let questions = BTreeMap::from([("decision".to_owned(), choice())]);
            let work_items = ["first", "second"]
                .into_iter()
                .map(|id| JevWorkItem {
                    id: id.to_owned(),
                    context: json!({"text": "x".repeat(5_000)}),
                    context_chunk_ids: Vec::new(),
                    event_ids: vec![format!("event-{id}")],
                    questions: questions.clone(),
                })
                .collect::<Vec<_>>();
            Ok(JevCheckPlan {
                check_id: self.id().to_owned(),
                input_revision: context.input_revision.clone(),
                work_items,
                skipped_item_ids: Vec::new(),
                coverage: JevCoverage {
                    selected_items: 2,
                    ..JevCoverage::default()
                },
                check_data: Value::Null,
            })
        }

        fn reduce(
            &self,
            plan: &JevCheckPlan,
            results: &[JevWorkItemResult],
            complete: bool,
        ) -> Result<Self::Result, JevError> {
            if plan.check_id != self.id() {
                return Err(JevError::InvalidCheckPlan);
            }
            Ok(json!({
                "completed": complete,
                "work_item_ids": results.iter().map(|result| result.work_item_id.as_str()).collect::<Vec<_>>(),
            }))
        }
    }

    fn response_for(request: &JevRequest) -> JevResponse {
        let answers = request
            .questions
            .iter()
            .map(|(id, question)| {
                let JevQuestion::Choice { criteria, .. } = question else {
                    panic!("synthetic check uses Choice questions")
                };
                let choice = criteria.keys().next().unwrap().clone();
                let probabilities = criteria
                    .keys()
                    .map(|option| (option.clone(), if option == &choice { 1.0 } else { 0.0 }))
                    .collect();
                (
                    id.clone(),
                    JevAnswer::Choice {
                        choice,
                        probabilities,
                        confidence: 1.0,
                    },
                )
            })
            .collect();
        JevResponse {
            model: request.model.clone(),
            answers,
            usage: JevUsage {
                input_tokens: 10,
                output_tokens: 1,
            },
        }
    }

    #[tokio::test]
    async fn generic_runner_resumes_partial_progress_without_repeating_completed_batches() {
        let context = JevSessionContext {
            input_revision: "immutable-input".to_owned(),
            session_identity: "synthetic-session".to_owned(),
            chunks: Vec::new(),
            check_context: Value::Null,
            limitations: Vec::new(),
        };
        let mut calls = 0;
        let mut saved = Vec::new();
        let first = run_jev_check(
            &ResumeCheck,
            &context,
            JevRunProgress::default(),
            |batch| {
                calls += 1;
                let call = calls;
                async move {
                    if call == 2 {
                        Err(JevError::ProviderUnavailable)
                    } else {
                        Ok(response_for(&batch.request))
                    }
                }
            },
            |progress| {
                saved.push(progress.clone());
                Ok(())
            },
        )
        .await
        .unwrap();
        assert!(!first.complete);
        assert_eq!(first.failure, Some(JevError::ProviderUnavailable));
        assert_eq!(first.progress.results.len(), 1);
        assert_eq!(first.progress.completed_request_digests.len(), 1);
        assert_eq!(saved.len(), 1);

        let mut resumed_calls = 0;
        let resumed = run_jev_check(
            &ResumeCheck,
            &context,
            first.progress,
            |batch| {
                resumed_calls += 1;
                async move { Ok(response_for(&batch.request)) }
            },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert!(resumed.complete);
        assert_eq!(resumed_calls, 1);
        assert_eq!(resumed.progress.results.len(), 2);
        assert_eq!(resumed.progress.request_count, 2);
        assert_eq!(resumed.result["completed"], true);
    }

    #[test]
    fn limit_errors_name_jev_and_remain_distinct() {
        assert_eq!(
            JevError::UsageLimitReached.to_string(),
            "Jev usage limit was reached"
        );
        assert_eq!(
            JevError::RequestLimitReached.to_string(),
            "Jev request limit was reached"
        );
    }
}
