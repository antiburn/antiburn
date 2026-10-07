//! Reusable TypeSafe request and check contracts.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::checks::ignored_instructions::digest_hex as format_digest_hex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub mod classification;
pub mod compact_ids;
pub mod edit_hunks;
pub mod exact_facts;
pub mod obligations;
pub mod text_ranges;

pub const PINNED_MODEL: &str = "jev-1.13.0";
/// Bound the full serialized request before checking the returned token usage.
pub const MAX_REQUEST_BYTES: usize = 60 * 1024;
/// Use a conservative byte proxy for Jev's state-plus-question token limit.
pub const MAX_STATE_AND_LONGEST_QUESTION_BYTES: usize = 30 * 1024;
/// Jev rejects requests that exceed its current total input-token limit.
pub const MAX_REQUEST_TOKENS: u64 = 64 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
pub const MAX_QUESTIONS_PER_REQUEST: usize = 128;
pub const MAX_PARALLEL_REQUESTS: usize = 16;
const REQUEST_START_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// One independently selectable normalized session input field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum JevInputField {
    UserMessage = 0,
    AssistantMessage = 1,
    BashCommandInput = 2,
    BashCommandOutput = 3,
    FileEditPath = 4,
    FileEditContent = 5,
    ReadFilePath = 6,
    ReadFileOutput = 7,
    SearchFilesQuery = 8,
    SearchFilesOutput = 9,
    OtherToolInput = 10,
    OtherToolOutput = 11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevFieldCapability {
    Supported,
    Conditional,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevFieldAvailabilityState {
    Excluded,
    Unsupported,
    NotObserved,
    Observed,
}

/// Bounded semantic input fields captured by the source-normalization path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevNormalizedCategory {
    BashCommand,
    FileEdit,
    ReadFile,
    SearchFiles,
    OtherTool,
}

impl JevNormalizedCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BashCommand => "bash_command",
            Self::FileEdit => "file_edit",
            Self::ReadFile => "read_file",
            Self::SearchFiles => "search_files",
            Self::OtherTool => "other_tool",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "bash_command" => Some(Self::BashCommand),
            "file_edit" => Some(Self::FileEdit),
            "read_file" => Some(Self::ReadFile),
            "search_files" => Some(Self::SearchFiles),
            "other_tool" => Some(Self::OtherTool),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct JevNormalizedFields {
    #[serde(default)]
    pub category: Option<JevNormalizedCategory>,
    pub values: BTreeMap<JevInputField, String>,
    pub malformed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JevFieldAvailability {
    pub field: JevInputField,
    pub selected: bool,
    pub capability: JevFieldCapability,
    pub state: JevFieldAvailabilityState,
    pub observed_parts: u32,
    pub empty_parts: u32,
    pub malformed_parts: u32,
    pub truncated_parts: u32,
}

/// Source-independent requirements a check declares before projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct JevEvidenceRequirements {
    pub fields: JevInputSelection,
    /// Check-owned reference kinds, such as instruction policy snapshots.
    pub references: Vec<String>,
}

pub const MAX_SELECTED_EVIDENCE_PARTS: usize = 288;
pub const MAX_SELECTED_EVIDENCE_BYTES: usize = 1024 * 1024 + 128 * 1024;

/// Immutable selected evidence from one published fence. Local source IDs
/// never enter provider state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JevEvidenceRange {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JevEvidenceStore {
    fields: BTreeMap<String, BTreeMap<JevInputField, JevEvidenceRange>>,
    text: String,
    publication_fence: Option<i64>,
}

/// One policy/reference snapshot supplied outside transcript evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevReferenceSnapshot {
    pub kind: String,
    pub identity: String,
    pub revision: String,
    pub fields: Value,
}

impl JevEvidenceStore {
    pub fn for_publication(publication_fence: i64) -> Self {
        Self {
            publication_fence: Some(publication_fence),
            ..Self::default()
        }
    }

    pub fn insert(
        &mut self,
        source_id: impl Into<String>,
        field: JevInputField,
        value: String,
    ) -> Result<(), JevError> {
        let source_id = source_id.into();
        if source_id.is_empty() {
            return Err(JevError::InvalidCheckContext);
        }
        if self
            .fields
            .get(&source_id)
            .is_some_and(|fields| fields.contains_key(&field))
        {
            return Err(JevError::InvalidCheckContext);
        }
        let new_parts = self.fields.len() + usize::from(!self.fields.contains_key(&source_id));
        if new_parts > MAX_SELECTED_EVIDENCE_PARTS
            || self.text.len().saturating_add(value.len()) > MAX_SELECTED_EVIDENCE_BYTES
        {
            return Err(JevError::InvalidCheckContext);
        }
        let start = self.text.len();
        self.text.push_str(&value);
        let end = self.text.len();
        self.fields
            .entry(source_id)
            .or_default()
            .insert(field, JevEvidenceRange { start, end });
        Ok(())
    }

    pub fn get(&self, source_id: &str, field: JevInputField) -> Option<&str> {
        self.fields
            .get(source_id)?
            .get(&field)
            .and_then(|range| self.text.get(range.start..range.end))
    }

    pub fn total_bytes(&self) -> usize {
        self.text.len()
    }

    pub fn publication_fence(&self) -> Option<i64> {
        self.publication_fence
    }
}

/// Check-owned access to normalized session fields. An empty selection is safe by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct JevInputSelection(u16);

impl JevInputSelection {
    pub const NONE: Self = Self(0);
    pub const ALL: Self = Self((1 << 12) - 1);

    pub const fn from_fields(fields: &[JevInputField]) -> Self {
        let mut bits = 0u16;
        let mut index = 0;
        while index < fields.len() {
            bits |= 1 << fields[index] as u8;
            index += 1;
        }
        Self(bits)
    }

    pub const fn includes(self, field: JevInputField) -> bool {
        self.0 & (1 << field as u8) != 0
    }

    pub const fn bits(self) -> u16 {
        self.0
    }
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
    ProgressStorageFailure,
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
            Self::ProgressStorageFailure => {
                formatter.write_str("Burn Check progress could not be saved")
            }
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
pub struct JevSessionContext {
    /// Revision of the normalized facts supplied to this check.
    pub input_revision: String,
    pub session_identity: String,
    /// Immutable, check-owned facts selected from normalized session data.
    pub check_context: Value,
    pub limitations: Vec<String>,
    #[serde(default)]
    pub reference_snapshots: Vec<JevReferenceSnapshot>,
    /// Runtime-only view over the same selected, fenced input. Rebuilt from
    /// the source on continuation instead of copied into progress JSON.
    #[serde(skip, default)]
    pub evidence_store: JevEvidenceStore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevCheckRevisions {
    pub projection: u32,
    pub chunking: u32,
    pub questions: u32,
    pub reducer: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevEvidenceRole {
    Instruction,
    Candidate,
    SupportingContext,
}

/// Maps one request-local input path to a stable source identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevEvidenceReference {
    pub part_id: String,
    pub source_id: String,
    pub content_kind: String,
    pub role: JevEvidenceRole,
}

/// One check-owned bounded input window. `fields` is the only evidence sent to
/// Jev; `evidence` stays local and binds its request paths to source identities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevInputWindow {
    pub fields: Value,
    pub evidence: Vec<JevEvidenceReference>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevWorkItem {
    /// Stable local identity. The request uses a separate request-local label.
    pub id: String,
    pub window: JevInputWindow,
    pub questions: BTreeMap<String, JevQuestion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevCheckPlan<Prepared = Value> {
    pub check_id: String,
    pub input_revision: String,
    pub revisions: JevCheckRevisions,
    pub work_items: Vec<JevWorkItem>,
    pub skipped_item_ids: Vec<String>,
    pub coverage: JevCoverage,
    /// Check-owned typed preparation and reducer state.
    pub prepared: Prepared,
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
    pub evidence: Vec<JevEvidenceReference>,
    pub model: String,
    pub usage: JevUsage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevRequestBatch {
    /// Local progress identity. It is separate from the provider cache digest.
    pub id: String,
    pub request: JevRequest,
    pub work_item_ids: Vec<String>,
    pub work_item_digests: BTreeMap<String, String>,
    /// Maps returned question IDs to their owning work item and local ID.
    pub answer_owners: BTreeMap<String, (String, String)>,
    /// Keeps source identities local while answers return to their work items.
    pub evidence_owners: BTreeMap<String, Vec<JevEvidenceReference>>,
    /// Digest of only the serialized Jev request.
    pub digest: String,
    pub serialized_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct JevPackingResult {
    pub batches: Vec<JevRequestBatch>,
    pub skipped_item_ids: Vec<String>,
}

/// Charge request, local bindings, serialization, and response space to admission.
/// This charge is not an allocator measurement of the complete assessment.
pub fn jev_batch_resident_bytes(batch: &JevRequestBatch) -> Result<usize, JevError> {
    let local_bytes = json_bytes(
        &(
            &batch.id,
            &batch.work_item_ids,
            &batch.work_item_digests,
            &batch.answer_owners,
            &batch.evidence_owners,
            &batch.digest,
        ),
        usize::MAX,
    )
    .map_err(|_| JevError::RequestSerialization)?;
    let request_bytes = validate_jev_request(&batch.request)?;
    let bytes = request_bytes
        .saturating_add(local_bytes)
        .saturating_mul(2)
        .saturating_add(MAX_RESPONSE_BYTES);
    const MAX_RESIDENT_BATCH_BYTES: usize = 512 * 1024;
    if bytes > MAX_RESIDENT_BATCH_BYTES {
        return Err(JevError::RequestTooLarge {
            bytes,
            maximum: MAX_RESIDENT_BATCH_BYTES,
        });
    }
    Ok(bytes)
}

/// A check owns its normalized-fact projection, input windows, typed questions,
/// evidence mapping, revisions, and deterministic reduction. Each work item is
/// one bounded window. Its local evidence references mark instruction,
/// candidate, and supporting-context parts without sending source IDs to Jev.
/// The shared runner owns request packing, transport, limits, caching, usage
/// reservations, retries, progress persistence, cancellation, and resume.
pub trait JevCheck {
    type Prepared: Serialize + DeserializeOwned;
    type Result: Serialize;

    fn id(&self) -> &'static str;

    fn revisions(&self) -> JevCheckRevisions;

    fn input_selection(&self) -> JevInputSelection {
        JevInputSelection::NONE
    }

    fn supports_incremental_reuse(&self) -> bool {
        false
    }

    fn incremental_identity(&self, _context: &JevSessionContext) -> Value {
        Value::Null
    }

    fn evidence_requirements(&self) -> JevEvidenceRequirements {
        JevEvidenceRequirements {
            fields: self.input_selection(),
            references: Vec::new(),
        }
    }

    /// Retrieve an allowed field by its local binding. The store contains
    /// only fields selected at the published source boundary.
    fn retrieve_evidence<'a>(
        &self,
        context: &'a JevSessionContext,
        reference: &JevEvidenceReference,
        field: JevInputField,
    ) -> Result<Option<&'a str>, JevError> {
        if !self.evidence_requirements().fields.includes(field) {
            return Err(JevError::InvalidCheckContext);
        }
        Ok(context.evidence_store.get(&reference.source_id, field))
    }

    /// Select normalized facts, build bounded windows, and prepare questions.
    fn prepare(
        &self,
        context: &JevSessionContext,
    ) -> Result<JevCheckPlan<Self::Prepared>, JevError>;

    /// Classify bounded reference inputs before the candidate request stage.
    fn classifications(&self, _context: &JevSessionContext) -> Result<Vec<JevWorkItem>, JevError> {
        Ok(Vec::new())
    }

    /// Keep missing or uncertain properties visible when matching candidates.
    fn apply_classifications(
        &self,
        _plan: &mut JevCheckPlan<Self::Prepared>,
        _results: &BTreeMap<String, JevWorkItemResult>,
        _context: &JevSessionContext,
    ) -> Result<(), JevError> {
        Ok(())
    }

    /// Return a uniquely identified bounded follow-up window when an initial
    /// result needs cross-chunk context. The runner applies the same caps.
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
        plan: &JevCheckPlan<Self::Prepared>,
        results: &[JevWorkItemResult],
        complete: bool,
    ) -> Result<Self::Result, JevError>;
}

/// Bounded admission to preparation and execution for one check assessment.
pub struct JevOrchestrationPermit(tokio::sync::SemaphorePermit<'static>);

pub async fn admit_jev_orchestration() -> Result<JevOrchestrationPermit, JevError> {
    orchestration_slot()
        .acquire()
        .await
        .map(JevOrchestrationPermit)
        .map_err(|_| JevError::Cancelled)
}

/// Pack check-owned work items into requests under the full serialized-byte
/// ceiling. Each question receives an explicit path to its item context.
pub fn pack_work_items(items: &[JevWorkItem]) -> JevPackingResult {
    pack_work_items_bounded(items, MAX_REQUEST_BYTES)
}

fn pack_work_items_bounded(items: &[JevWorkItem], maximum_bytes: usize) -> JevPackingResult {
    pack_item_refs(items.iter(), maximum_bytes)
}

fn pack_item_refs<'a>(
    items: impl Iterator<Item = &'a JevWorkItem>,
    maximum_bytes: usize,
) -> JevPackingResult {
    let mut result = JevPackingResult::default();
    let mut current: Vec<&JevWorkItem> = Vec::new();
    let mut costs = PackingCosts::default();
    let mut retained_bytes = 0;
    for item in items {
        if build_batch(&[item], maximum_bytes).is_none() {
            result.skipped_item_ids.push(item.id.clone());
            continue;
        }
        let next = costs.add(item, current.len(), maximum_bytes);
        if next.is_none() {
            if !current.is_empty() {
                if let Some(batch) = build_batch(&current, maximum_bytes) {
                    retain_packed_batch(&mut result, batch, &mut retained_bytes);
                }
                current.clear();
            }
            costs = PackingCosts::default();
        }
        costs = next.unwrap_or_else(|| {
            costs
                .add(item, 0, maximum_bytes)
                .expect("the single-item request passes exact limits")
        });
        current.push(item);
    }
    if !current.is_empty()
        && let Some(batch) = build_batch(&current, maximum_bytes)
    {
        retain_packed_batch(&mut result, batch, &mut retained_bytes);
    }
    result
}

const MAX_QUEUED_BYTES: usize = 16 * 1024 * 1024;

fn retain_packed_batch(result: &mut JevPackingResult, batch: JevRequestBatch, bytes: &mut usize) {
    match jev_batch_resident_bytes(&batch) {
        Ok(size) if size <= MAX_QUEUED_BYTES.saturating_sub(*bytes) => {
            *bytes += size;
            result.batches.push(batch);
        }
        _ => result.skipped_item_ids.extend(batch.work_item_ids),
    }
}

#[derive(Clone, Copy, Default)]
struct PackingCosts {
    state_items: usize,
    questions: usize,
    question_count: usize,
    longest_question: usize,
}

impl PackingCosts {
    fn add(mut self, item: &JevWorkItem, index: usize, maximum: usize) -> Option<Self> {
        #[derive(Serialize)]
        struct StateItem<'a> {
            id: String,
            context: &'a Value,
        }
        self.state_items += json_bytes(
            &StateItem {
                id: format!("w_{index}"),
                context: &item.window.fields,
            },
            maximum,
        )
        .ok()?
            + usize::from(index > 0);
        for question in item.questions.values() {
            let question = question
                .clone()
                .with_context_path(&format!("work_items[{index}].context"));
            let bytes = json_bytes(&question, maximum).ok()?;
            self.questions += 69 + bytes + usize::from(self.question_count > 0);
            self.question_count += 1;
            self.longest_question = self.longest_question.max(bytes);
        }
        let empty = JevRequest {
            model: PINNED_MODEL.to_owned(),
            state: json!({"work_items": []}),
            questions: BTreeMap::new(),
        };
        let state = json_bytes(&empty.state, maximum).ok()? + self.state_items;
        let request = json_bytes(&empty, maximum).ok()? + self.state_items + self.questions;
        (self.question_count <= MAX_QUESTIONS_PER_REQUEST
            && request <= maximum
            && state + self.longest_question <= MAX_STATE_AND_LONGEST_QUESTION_BYTES)
            .then_some(self)
    }
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
        let evidence = batch
            .evidence_owners
            .get(work_item_id)
            .cloned()
            .ok_or(JevError::InvalidCheckPlan)?;
        results.push(JevWorkItemResult {
            request_id: batch.id.clone(),
            work_item_id: work_item_id.clone(),
            answers,
            evidence,
            model: response.model.clone(),
            usage: response.usage,
        });
    }
    Ok(results)
}

fn build_batch(items: &[&JevWorkItem], maximum_bytes: usize) -> Option<JevRequestBatch> {
    let mut state_items = Vec::with_capacity(items.len());
    let mut questions = BTreeMap::new();
    let mut answer_owners = BTreeMap::new();
    let mut evidence_owners = BTreeMap::new();
    for (index, item) in items.iter().enumerate() {
        if item.id.is_empty()
            || item.window.evidence.iter().any(|evidence| {
                evidence.part_id.is_empty()
                    || evidence.source_id.is_empty()
                    || evidence.content_kind.is_empty()
            })
        {
            return None;
        }
        let part_ids = item
            .window
            .evidence
            .iter()
            .map(|evidence| evidence.part_id.as_str())
            .collect::<BTreeSet<_>>();
        if part_ids.len() != item.window.evidence.len() {
            return None;
        }
        state_items.push(json!({"id": format!("w_{index}"), "context": item.window.fields}));
        evidence_owners.insert(item.id.clone(), item.window.evidence.clone());
        for (question_id, question) in &item.questions {
            let response_id = format!(
                "q_{}",
                digest_hex(format!("{index}\0{question_id}").as_bytes())
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
    let mut serialized = JsonMeasure::new(maximum_bytes);
    if serde_json::to_writer(&mut serialized, &request).is_err() {
        return None;
    }
    let state_bytes = json_bytes(&request.state, MAX_STATE_AND_LONGEST_QUESTION_BYTES).ok()?;
    let longest_question_bytes = request
        .questions
        .values()
        .map(|question| json_bytes(question, MAX_STATE_AND_LONGEST_QUESTION_BYTES))
        .collect::<Result<Vec<_>, _>>()
        .ok()?
        .into_iter()
        .max()
        .unwrap_or_default();
    if state_bytes.saturating_add(longest_question_bytes) > MAX_STATE_AND_LONGEST_QUESTION_BYTES {
        return None;
    }
    let digest = format_digest_hex(serialized.hash.finalize().as_slice());
    use std::io::Write as _;
    let mut identity = JsonMeasure::new(usize::MAX);
    identity.write_all(digest.as_bytes()).ok()?;
    for item in items {
        identity.write_all(b"\0").ok()?;
        identity.write_all(item.id.as_bytes()).ok()?;
    }
    identity.write_all(b"\0").ok()?;
    serde_json::to_writer(&mut identity, &evidence_owners).ok()?;
    let id = format_digest_hex(identity.hash.finalize().as_slice());
    Some(JevRequestBatch {
        id,
        request,
        work_item_ids: items.iter().map(|item| item.id.clone()).collect(),
        work_item_digests: items
            .iter()
            .map(|item| {
                let mut writer = JsonMeasure::new(usize::MAX);
                serde_json::to_writer(&mut writer, item).ok()?;
                Some((
                    item.id.clone(),
                    format_digest_hex(writer.hash.finalize().as_slice()),
                ))
            })
            .collect::<Option<BTreeMap<_, _>>>()?,
        answer_owners,
        evidence_owners,
        digest,
        serialized_bytes: serialized.bytes,
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

/// Use the distribution to select an option when the returned choice disagrees.
pub fn highest_probability_choice<'a>(
    choice: &'a str,
    probabilities: &'a BTreeMap<String, f64>,
) -> Option<&'a str> {
    let (highest, maximum) = probabilities
        .iter()
        .max_by(|left, right| left.1.total_cmp(right.1))?;
    if probabilities
        .get(choice)
        .is_some_and(|selected| *selected + 0.000_001 >= *maximum)
    {
        Some(choice)
    } else {
        Some(highest)
    }
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
    let bytes = json_bytes(request, usize::MAX).map_err(|_| JevError::RequestSerialization)?;
    if bytes > MAX_REQUEST_BYTES {
        return Err(JevError::RequestTooLarge {
            bytes,
            maximum: MAX_REQUEST_BYTES,
        });
    }
    let state_bytes =
        json_bytes(&request.state, usize::MAX).map_err(|_| JevError::RequestSerialization)?;
    let longest_question_bytes = request
        .questions
        .values()
        .map(|question| json_bytes(question, usize::MAX))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| JevError::RequestSerialization)?
        .into_iter()
        .max()
        .unwrap_or_default();
    if state_bytes.saturating_add(longest_question_bytes) > MAX_STATE_AND_LONGEST_QUESTION_BYTES {
        return Err(JevError::RequestTooLarge {
            bytes: state_bytes.saturating_add(longest_question_bytes),
            maximum: MAX_STATE_AND_LONGEST_QUESTION_BYTES,
        });
    }
    Ok(bytes)
}

/// Durable typed answers and local source bindings for one check assessment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct JevRunProgress {
    pub input_revision: String,
    pub results: BTreeMap<String, JevWorkItemResult>,
    pub completed_batch_ids: BTreeSet<String>,
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

/// Execute check-owned windows, reconciliation, and deterministic reduction.
/// The shared desktop runner supplies batch execution and progress storage.
pub async fn run_jev_check<C, E, Fut, S>(
    check: &C,
    context: &JevSessionContext,
    progress: JevRunProgress,
    execute: E,
    save_progress: S,
) -> Result<JevExecutionOutcome<C::Result>, JevError>
where
    C: JevCheck,
    E: Fn(std::sync::Arc<JevRequestBatch>) -> Fut + Sync,
    Fut: std::future::Future<Output = Result<JevResponse, JevError>> + Send,
    S: FnMut(&JevRunProgress) -> Result<(), JevError>,
{
    // Admit orchestration before preparation allocates request batches.
    let orchestration = admit_jev_orchestration().await?;
    let mut plan = check.prepare(context)?;
    run_jev_check_admitted(
        check,
        context,
        &mut plan,
        progress,
        execute,
        save_progress,
        orchestration.0,
    )
    .await
}

/// Execute a plan prepared by an admitted caller without preparing it again.
pub async fn run_jev_check_prepared<C, E, Fut, S>(
    check: &C,
    context: &JevSessionContext,
    plan: &mut JevCheckPlan<C::Prepared>,
    progress: JevRunProgress,
    orchestration: JevOrchestrationPermit,
    execute: E,
    save_progress: S,
) -> Result<JevExecutionOutcome<C::Result>, JevError>
where
    C: JevCheck,
    E: Fn(std::sync::Arc<JevRequestBatch>) -> Fut + Sync,
    Fut: std::future::Future<Output = Result<JevResponse, JevError>> + Send,
    S: FnMut(&JevRunProgress) -> Result<(), JevError>,
{
    run_jev_check_admitted(
        check,
        context,
        plan,
        progress,
        execute,
        save_progress,
        orchestration.0,
    )
    .await
}

async fn run_jev_check_admitted<C, E, Fut, S>(
    check: &C,
    context: &JevSessionContext,
    plan: &mut JevCheckPlan<C::Prepared>,
    mut progress: JevRunProgress,
    execute: E,
    mut save_progress: S,
    _orchestration: tokio::sync::SemaphorePermit<'static>,
) -> Result<JevExecutionOutcome<C::Result>, JevError>
where
    C: JevCheck,
    E: Fn(std::sync::Arc<JevRequestBatch>) -> Fut + Sync,
    Fut: std::future::Future<Output = Result<JevResponse, JevError>> + Send,
    S: FnMut(&JevRunProgress) -> Result<(), JevError>,
{
    // Validate plans from both preparation paths at the shared boundary.
    if plan.check_id != check.id()
        || plan.input_revision != context.input_revision
        || plan.revisions != check.revisions()
        || !valid_work_item_ids(&plan.work_items)
    {
        return Err(JevError::InvalidCheckPlan);
    }
    let requirements = check.evidence_requirements();
    if requirements.fields != check.input_selection() {
        return Err(JevError::InvalidCheckContext);
    }
    if requirements.references.iter().any(|required| {
        !context
            .reference_snapshots
            .iter()
            .any(|snapshot| &snapshot.kind == required)
    }) {
        return Err(JevError::InvalidCheckContext);
    }
    let classifications = check.classifications(context)?;
    if !valid_work_item_ids(&classifications) {
        return Err(JevError::InvalidCheckPlan);
    }
    let mut scope_writer = JsonMeasure::new(usize::MAX);
    serde_json::to_writer(
        &mut scope_writer,
        &(
            check.id(),
            &context.session_identity,
            plan.revisions,
            &requirements,
            (!check.supports_incremental_reuse()).then_some(&context.reference_snapshots),
            PINNED_MODEL,
            check.incremental_identity(context),
            crate::analysis::PARSER_REVISION,
        ),
    )
    .map_err(|_| JevError::InvalidCheckContext)?;
    let reuse_scope = format!(
        "reuse-scope:{}",
        format_digest_hex(scope_writer.hash.finalize().as_slice())
    );
    let progress_revision = progress_revision(check.id(), context, plan.revisions, &requirements)?;
    if progress.input_revision != progress_revision {
        if check.supports_incremental_reuse() && progress.completed_batch_ids.contains(&reuse_scope)
        {
            progress.input_revision = progress_revision;
            progress
                .completed_batch_ids
                .retain(|id| id.starts_with("reuse-item:"));
        } else {
            progress = JevRunProgress {
                input_revision: progress_revision,
                ..JevRunProgress::default()
            };
        }
    }
    progress
        .completed_batch_ids
        .retain(|id| !id.starts_with("reuse-scope:") || id == &reuse_scope);
    progress.completed_batch_ids.insert(reuse_scope.clone());
    progress.failed_item_ids.clear();
    let mut failure = None;
    let mut failure_batch: Option<String> = None;
    let mut complete = true;

    retain_matching_results(&classifications, &mut progress)?;
    let packed = pack_item_refs(
        classifications
            .iter()
            .filter(|item| !progress.results.contains_key(&item.id)),
        MAX_REQUEST_BYTES,
    );
    if !packed.skipped_item_ids.is_empty() {
        complete = false;
        progress.failed_item_ids.extend(packed.skipped_item_ids);
    }
    execute_batches(&execute, packed.batches, |batch, response| {
        match response {
            Ok(response) => {
                for result in unpack_jev_response(&batch, &response)? {
                    progress.results.insert(result.work_item_id.clone(), result);
                }
                progress.completed_batch_ids.insert(batch.id.clone());
                progress.request_count = progress.request_count.saturating_add(1);
            }
            Err(error) => {
                complete = false;
                progress
                    .failed_item_ids
                    .extend(batch.work_item_ids.iter().cloned());
                record_failure(&mut failure, &mut failure_batch, batch.id.clone(), error);
            }
        }
        save_progress(&progress)
    })
    .await?;
    check.apply_classifications(plan, &progress.results, context)?;
    retain_matching_results(&plan.work_items, &mut progress)?;

    if !valid_result_evidence(
        &plan.work_items,
        progress.results.values().filter(|result| {
            plan.work_items
                .iter()
                .any(|item| item.id == result.work_item_id)
        }),
    ) {
        return Err(JevError::InvalidCheckPlan);
    }
    let initial = pack_item_refs(
        plan.work_items
            .iter()
            .filter(|item| !progress.results.contains_key(&item.id)),
        MAX_REQUEST_BYTES,
    );
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
    let initial_batches = if failure
        .as_ref()
        .is_none_or(can_continue_after_batch_failure)
    {
        initial.batches
    } else {
        Vec::new()
    };
    execute_batches(&execute, initial_batches, |batch, response| {
        match response {
            Ok(response) => {
                let results = unpack_jev_response(&batch, &response)?;
                for result in results {
                    progress.results.insert(result.work_item_id.clone(), result);
                }
                progress.completed_batch_ids.insert(batch.id.clone());
                progress.request_count = progress.request_count.saturating_add(1);
            }
            Err(error) => {
                complete = false;
                progress
                    .failed_item_ids
                    .extend(batch.work_item_ids.iter().cloned());
                record_failure(&mut failure, &mut failure_batch, batch.id.clone(), error);
            }
        }
        save_progress(&progress)
    })
    .await?;
    if failure
        .as_ref()
        .is_some_and(|error| !can_continue_after_batch_failure(error))
    {
        complete = false;
    }

    let mut reconciliation_items = Vec::new();
    if failure
        .as_ref()
        .is_none_or(can_continue_after_batch_failure)
    {
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

    retain_matching_results(&reconciliation_items, &mut progress)?;
    let reconciliation = pack_item_refs(
        reconciliation_items
            .iter()
            .filter(|item| !progress.results.contains_key(&item.id)),
        MAX_REQUEST_BYTES,
    );
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
    let all_work_items = plan
        .work_items
        .iter()
        .chain(reconciliation_items.iter())
        .chain(classifications.iter())
        .collect::<Vec<_>>();
    if !valid_work_item_ids(all_work_items.iter().copied()) {
        return Err(JevError::InvalidCheckPlan);
    }
    let reconciliation_batches = if failure
        .as_ref()
        .is_none_or(can_continue_after_batch_failure)
    {
        reconciliation.batches.into_iter().collect()
    } else {
        Vec::new()
    };
    execute_batches(&execute, reconciliation_batches, |batch, response| {
        match response {
            Ok(response) => {
                let results = unpack_jev_response(&batch, &response)?;
                for result in results {
                    progress.results.insert(result.work_item_id.clone(), result);
                }
                progress.completed_batch_ids.insert(batch.id.clone());
                progress.request_count = progress.request_count.saturating_add(1);
            }
            Err(error) => {
                complete = false;
                progress
                    .failed_item_ids
                    .extend(batch.work_item_ids.iter().cloned());
                record_failure(&mut failure, &mut failure_batch, batch.id.clone(), error);
            }
        }
        save_progress(&progress)
    })
    .await?;

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
    if !complete {
        plan.coverage
            .limitations
            .push("assessment_incomplete".to_owned());
        plan.coverage.limitations.sort();
        plan.coverage.limitations.dedup();
    }
    let active_ids = all_work_items
        .iter()
        .map(|item| item.id.as_str())
        .collect::<BTreeSet<_>>();
    progress
        .results
        .retain(|id, _| active_ids.contains(id.as_str()));
    let active_requests = progress
        .results
        .values()
        .map(|result| result.request_id.as_str())
        .collect::<BTreeSet<_>>();
    progress.completed_batch_ids.retain(|id| {
        id == &reuse_scope
            || active_requests.contains(id.as_str())
            || reusable_item_id(id).is_some_and(|item_id| active_ids.contains(item_id))
    });
    progress
        .completed_batch_ids
        .retain(|id| reusable_item_id(id).is_none_or(|item_id| active_ids.contains(item_id)));
    if !valid_result_evidence(all_work_items.iter().copied(), progress.results.values()) {
        return Err(JevError::InvalidCheckPlan);
    }
    let results = progress.results.values().cloned().collect::<Vec<_>>();
    let result = check.reduce(plan, &results, complete)?;
    let terminal_failure = failure.filter(|error| !can_continue_after_batch_failure(error));
    Ok(JevExecutionOutcome {
        result,
        progress,
        complete,
        failure: terminal_failure,
    })
}

fn reusable_item_id(marker: &str) -> Option<&str> {
    marker
        .strip_prefix("reuse-item:")
        .and_then(|rest| rest.rsplit_once(':'))
        .map(|(item_id, _)| item_id)
}

fn retain_matching_results(
    items: &[JevWorkItem],
    progress: &mut JevRunProgress,
) -> Result<(), JevError> {
    for item in items {
        let mut writer = JsonMeasure::new(usize::MAX);
        serde_json::to_writer(&mut writer, item).map_err(|_| JevError::InvalidCheckPlan)?;
        let prefix = format!("reuse-item:{}:", item.id);
        let digest = format!(
            "{prefix}{}",
            format_digest_hex(writer.hash.finalize().as_slice())
        );
        if !progress.completed_batch_ids.contains(&digest)
            || progress.results.get(&item.id).is_some_and(|result| {
                result.work_item_id != item.id
                    || result.model != PINNED_MODEL
                    || result.evidence != item.window.evidence
                    || !answers_match_item(item, &result.answers)
            })
        {
            progress.results.remove(&item.id);
        }
        if let Some(result) = progress.results.get(&item.id) {
            progress
                .completed_batch_ids
                .insert(result.request_id.clone());
        }
        progress
            .completed_batch_ids
            .retain(|id| reusable_item_id(id) != Some(item.id.as_str()));
        progress.completed_batch_ids.insert(digest);
    }
    Ok(())
}

fn answers_match_item(item: &JevWorkItem, answers: &BTreeMap<String, JevAnswer>) -> bool {
    let Some(batch) = build_batch(&[item], MAX_REQUEST_BYTES) else {
        return false;
    };
    let response_answers = batch
        .answer_owners
        .iter()
        .filter_map(|(remote_id, (_, local_id))| {
            answers
                .get(local_id)
                .map(|answer| (remote_id.clone(), answer.clone()))
        })
        .collect();
    validate_jev_response(
        &JevResponse {
            model: PINNED_MODEL.to_owned(),
            answers: response_answers,
            usage: JevUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
        },
        &batch.request,
    )
    .is_ok()
}

fn valid_work_item_ids<'a>(items: impl IntoIterator<Item = &'a JevWorkItem>) -> bool {
    let mut ids = BTreeSet::new();
    items
        .into_iter()
        .all(|item| !item.id.is_empty() && ids.insert(item.id.as_str()))
}

fn record_failure(
    failure: &mut Option<JevError>,
    batch_id: &mut Option<String>,
    id: String,
    error: JevError,
) {
    let replace = failure.as_ref().is_none_or(|previous_error| {
        (can_continue_after_batch_failure(previous_error)
            && !can_continue_after_batch_failure(&error))
            || (can_continue_after_batch_failure(previous_error)
                == can_continue_after_batch_failure(&error)
                && batch_id.as_ref().is_none_or(|previous| &id < previous))
    });
    if replace {
        *batch_id = Some(id);
        *failure = Some(error);
    }
}

fn can_continue_after_batch_failure(error: &JevError) -> bool {
    matches!(
        error,
        JevError::RequestOutcomeUnknown
            | JevError::RateLimited { .. }
            | JevError::ProviderOverloaded { .. }
            | JevError::ProviderUnavailable
    )
}

fn valid_result_evidence<'a>(
    items: impl IntoIterator<Item = &'a JevWorkItem>,
    mut results: impl Iterator<Item = &'a JevWorkItemResult>,
) -> bool {
    let expected = items
        .into_iter()
        .map(|item| (item.id.as_str(), item.window.evidence.as_slice()))
        .collect::<BTreeMap<_, _>>();
    results.all(|result| {
        expected
            .get(result.work_item_id.as_str())
            .is_some_and(|evidence| *evidence == result.evidence.as_slice())
    })
}

fn progress_revision(
    check_id: &str,
    context: &JevSessionContext,
    revisions: JevCheckRevisions,
    requirements: &JevEvidenceRequirements,
) -> Result<String, JevError> {
    use std::io::Write as _;
    let mut writer = JsonMeasure::new(usize::MAX);
    let identity = format!(
        "{check_id}\0{}\0{}\0{}\0{}\0{}\0{}",
        context.session_identity,
        context.input_revision,
        revisions.projection,
        revisions.chunking,
        revisions.questions,
        revisions.reducer,
    );
    writer
        .write_all(identity.as_bytes())
        .map_err(|_| JevError::InvalidCheckContext)?;
    serde_json::to_writer(&mut writer, &(requirements, &context.reference_snapshots))
        .map_err(|_| JevError::InvalidCheckContext)?;
    Ok(format_digest_hex(writer.hash.finalize().as_slice()))
}

fn orchestration_slot() -> &'static tokio::sync::Semaphore {
    static SLOT: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| tokio::sync::Semaphore::new(1))
}

async fn execute_batches<E, Fut, S>(
    execute: &E,
    batches: Vec<JevRequestBatch>,
    mut settle: S,
) -> Result<(), JevError>
where
    E: Fn(std::sync::Arc<JevRequestBatch>) -> Fut + Sync,
    Fut: std::future::Future<Output = Result<JevResponse, JevError>> + Send,
    S: FnMut(
        std::sync::Arc<JevRequestBatch>,
        Result<JevResponse, JevError>,
    ) -> Result<(), JevError>,
{
    let queued_bytes = batches.iter().try_fold(0usize, |total, batch| {
        total
            .checked_add(jev_batch_resident_bytes(batch)?)
            .ok_or(JevError::RequestSerialization)
    })?;
    if queued_bytes > MAX_QUEUED_BYTES {
        return Err(JevError::RequestTooLarge {
            bytes: queued_bytes,
            maximum: MAX_QUEUED_BYTES,
        });
    }
    type BatchFuture<'a> = Pin<
        Box<
            dyn Future<
                    Output = (
                        std::sync::Arc<JevRequestBatch>,
                        Result<JevResponse, JevError>,
                    ),
                > + Send
                + 'a,
        >,
    >;

    let mut pending = batches.into_iter().collect::<VecDeque<_>>();
    let mut tasks: Vec<Option<BatchFuture<'_>>> = std::iter::repeat_with(|| None)
        .take(MAX_PARALLEL_REQUESTS)
        .collect();
    let mut next_start: Option<Pin<Box<tokio::time::Sleep>>> = None;
    let mut stop_pending = false;

    std::future::poll_fn(|context| {
        for task in &mut tasks {
            let ready = task
                .as_mut()
                .and_then(|task| match task.as_mut().poll(context) {
                    std::task::Poll::Ready(result) => Some(result),
                    std::task::Poll::Pending => None,
                });
            if let Some((batch, result)) = ready {
                let result = result.and_then(|response| {
                    validate_jev_response(&response, &batch.request)?;
                    Ok(response)
                });
                stop_pending |= result
                    .as_ref()
                    .is_err_and(|error| !can_continue_after_batch_failure(error));
                if let Err(error) = settle(batch, result) {
                    return std::task::Poll::Ready(Err(error));
                }
                *task = None;
            }
        }

        let start_ready = next_start
            .as_mut()
            .is_none_or(|sleep| sleep.as_mut().poll(context).is_ready());
        if !stop_pending
            && !pending.is_empty()
            && start_ready
            && let Some(slot) = tasks.iter().position(Option::is_none)
            && let Some(batch) = pending.pop_front()
        {
            let batch = std::sync::Arc::new(batch);
            let request = execute(std::sync::Arc::clone(&batch));
            tasks[slot] = Some(Box::pin(async move { (batch, request.await) }));
            next_start = Some(Box::pin(tokio::time::sleep(REQUEST_START_INTERVAL)));
            context.waker().wake_by_ref();
        }

        if tasks.iter().all(Option::is_none) && (stop_pending || pending.is_empty()) {
            std::task::Poll::Ready(Ok(()))
        } else {
            std::task::Poll::Pending
        }
    })
    .await
}

struct JsonMeasure {
    bytes: usize,
    maximum: usize,
    hash: Sha256,
}

impl JsonMeasure {
    fn new(maximum: usize) -> Self {
        Self {
            bytes: 0,
            maximum,
            hash: Sha256::new(),
        }
    }
}

impl std::io::Write for JsonMeasure {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.bytes) {
            return Err(std::io::Error::other("JSON byte limit exceeded"));
        }
        self.bytes += bytes.len();
        self.hash.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn json_bytes(value: &impl Serialize, maximum: usize) -> Result<usize, serde_json::Error> {
    let mut writer = JsonByteCount { bytes: 0, maximum };
    serde_json::to_writer(&mut writer, value)?;
    Ok(writer.bytes)
}

struct JsonByteCount {
    bytes: usize,
    maximum: usize,
}

impl std::io::Write for JsonByteCount {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.bytes) {
            return Err(std::io::Error::other("JSON byte limit exceeded"));
        }
        self.bytes += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
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
    use std::sync::atomic::Ordering;

    #[test]
    fn reusable_identity_preserves_results_for_prefix_related_ids() {
        for reverse in [false, true] {
            let mut items = vec![
                item("screen", "screen evidence"),
                item("screen::followup", "followup evidence"),
                item("screen:child", "child evidence"),
                item("screening", "unrelated evidence"),
            ];
            if reverse {
                items.reverse();
            }
            let mut progress = JevRunProgress::default();
            for item in &items {
                let marker = format!(
                    "reuse-item:{}:{}",
                    item.id,
                    digest_hex(&serde_json::to_vec(item).unwrap())
                );
                progress.completed_batch_ids.insert(marker);
            }
            let batch = pack_work_items(&items).batches.remove(0);
            for result in unpack_jev_response(&batch, &response_for(&batch.request)).unwrap() {
                progress.results.insert(result.work_item_id.clone(), result);
            }
            let expected_results = progress.results.clone();
            let expected_markers = progress.completed_batch_ids.clone();

            for item in &items {
                retain_matching_results(std::slice::from_ref(item), &mut progress).unwrap();
                assert_eq!(progress.results, expected_results);
                assert!(expected_markers.is_subset(&progress.completed_batch_ids));
            }
            let mut changed = item("screen", "changed screen evidence");
            retain_matching_results(std::slice::from_ref(&changed), &mut progress).unwrap();
            assert!(!progress.results.contains_key("screen"));
            for id in ["screen::followup", "screen:child", "screening"] {
                assert_eq!(progress.results.get(id), expected_results.get(id));
                let marker = expected_markers
                    .iter()
                    .find(|marker| reusable_item_id(marker) == Some(id))
                    .unwrap();
                assert!(progress.completed_batch_ids.contains(marker));
            }
            changed.id = "screen::followup".to_owned();
            retain_matching_results(std::slice::from_ref(&changed), &mut progress).unwrap();
            assert!(!progress.results.contains_key("screen::followup"));
            assert_eq!(progress.results.len(), 2);
        }
    }

    struct DurableCheck;

    impl JevCheck for DurableCheck {
        type Prepared = Value;
        type Result = usize;

        fn id(&self) -> &'static str {
            "durable_test"
        }

        fn revisions(&self) -> JevCheckRevisions {
            check_revisions(1, 1, 1, 1)
        }

        fn supports_incremental_reuse(&self) -> bool {
            true
        }

        fn prepare(&self, context: &JevSessionContext) -> Result<JevCheckPlan<Value>, JevError> {
            let work_items = context.check_context["items"]
                .as_array()
                .ok_or(JevError::InvalidCheckContext)?
                .iter()
                .map(|value| {
                    let id = value.as_str().ok_or(JevError::InvalidCheckContext)?;
                    Ok(item(id, id))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(JevCheckPlan {
                check_id: self.id().to_owned(),
                input_revision: context.input_revision.clone(),
                revisions: self.revisions(),
                work_items,
                skipped_item_ids: Vec::new(),
                coverage: JevCoverage::default(),
                prepared: Value::Null,
            })
        }

        fn reduce(
            &self,
            _plan: &JevCheckPlan<Value>,
            results: &[JevWorkItemResult],
            _complete: bool,
        ) -> Result<usize, JevError> {
            Ok(results.len())
        }
    }

    fn durable_context(revision: &str, items: &[&str], reference: &str) -> JevSessionContext {
        JevSessionContext {
            input_revision: revision.to_owned(),
            session_identity: "same-session".to_owned(),
            check_context: json!({"items": items}),
            limitations: Vec::new(),
            evidence_store: JevEvidenceStore::default(),
            reference_snapshots: vec![JevReferenceSnapshot {
                kind: "instruction_snapshot".to_owned(),
                identity: reference.to_owned(),
                revision: reference.to_owned(),
                fields: json!({"other_file": reference}),
            }],
        }
    }

    #[tokio::test]
    async fn durable_answers_survive_append_restart_and_regroup_without_provider_calls() {
        let check = DurableCheck;
        let first = run_jev_check(
            &check,
            &durable_context("first", &["a", "b"], "original"),
            JevRunProgress::default(),
            |batch| async move { Ok(response_for(&batch.request)) },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert_eq!(first.result, 2);
        let saved = serde_json::to_string(&first.progress).unwrap();
        let restarted: JevRunProgress = serde_json::from_str(&saved).unwrap();
        let restarted = run_jev_check(
            &check,
            &durable_context("first", &["a", "b"], "original"),
            restarted,
            |_| async { panic!("restart must not dispatch") },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert_eq!(restarted.result, 2);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let appended = run_jev_check(
            &check,
            &durable_context("appended", &["b", "a", "c"], "unrelated-change"),
            restarted.progress,
            |batch| {
                calls.fetch_add(1, Ordering::SeqCst);
                async move { Ok(response_for(&batch.request)) }
            },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(appended.result, 3);
        let regrouped: JevRunProgress =
            serde_json::from_str(&serde_json::to_string(&appended.progress).unwrap()).unwrap();
        let regrouped = run_jev_check(
            &check,
            &durable_context("regrouped", &["c", "a", "b"], "another-file"),
            regrouped,
            |_| async { panic!("unchanged items must not dispatch") },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert!(regrouped.complete);
        assert_eq!(regrouped.result, 3);
    }

    #[tokio::test]
    async fn durable_answer_rejects_stale_binding_and_invalid_answer() {
        let check = DurableCheck;
        let context = durable_context("first", &["a"], "original");
        let first = run_jev_check(
            &check,
            &context,
            JevRunProgress::default(),
            |batch| async move { Ok(response_for(&batch.request)) },
            |_| Ok(()),
        )
        .await
        .unwrap();
        for (index, mut progress) in [first.progress.clone(), first.progress]
            .into_iter()
            .enumerate()
        {
            if index == 0 {
                progress.results.get_mut("a").unwrap().evidence[0].source_id = "stale".to_owned();
            } else {
                progress.results.get_mut("a").unwrap().answers.clear();
            }
            let calls = std::sync::atomic::AtomicUsize::new(0);
            let outcome = run_jev_check(
                &check,
                &durable_context("next", &["a"], "other"),
                progress,
                |batch| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(response_for(&batch.request)) }
                },
                |_| Ok(()),
            )
            .await
            .unwrap();
            assert!(outcome.complete);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn partial_failure_category_does_not_depend_on_completion_order() {
        for reverse in [false, true] {
            let mut failures = vec![
                ("batch-z", JevError::Cancelled),
                ("batch-a", JevError::RequestOutcomeUnknown),
            ];
            if reverse {
                failures.reverse();
            }
            let mut failure = None;
            let mut batch = None;
            for (id, error) in failures {
                record_failure(&mut failure, &mut batch, id.to_owned(), error);
            }
            assert_eq!(failure, Some(JevError::Cancelled));
            assert_eq!(batch.as_deref(), Some("batch-z"));
        }
    }

    #[test]
    fn incremental_packing_matches_exact_trials_and_profiles_scaling() {
        fn reference(items: &[JevWorkItem]) -> JevPackingResult {
            let mut packed = JevPackingResult::default();
            let mut current = Vec::new();
            for item in items {
                current.push(item);
                if build_batch(&current, MAX_REQUEST_BYTES).is_none() {
                    current.pop();
                    if !current.is_empty() {
                        packed
                            .batches
                            .push(build_batch(&current, MAX_REQUEST_BYTES).unwrap());
                    }
                    current.clear();
                    current.push(item);
                    if build_batch(&current, MAX_REQUEST_BYTES).is_none() {
                        current.clear();
                        packed.skipped_item_ids.push(item.id.clone());
                    }
                }
            }
            if !current.is_empty() {
                packed
                    .batches
                    .push(build_batch(&current, MAX_REQUEST_BYTES).unwrap());
            }
            packed
        }
        for count in [8, 32, 128] {
            let items = (0..count)
                .map(|index| {
                    let mut item = item(&index.to_string(), &"é😀\\\"\n".repeat(40 + index % 7));
                    item.questions = (0..1 + index % 4)
                        .map(|question| (format!("q-{question}"), choice()))
                        .collect();
                    item
                })
                .collect::<Vec<_>>();
            assert_eq!(pack_work_items(&items), reference(&items));
            let started = std::time::Instant::now();
            for _ in 0..5 {
                std::hint::black_box(reference(&items));
            }
            let reference_us = started.elapsed().as_micros();
            let started = std::time::Instant::now();
            for _ in 0..5 {
                std::hint::black_box(pack_work_items(&items));
            }
            println!(
                "phase6 pack items={count} runs=5 exact_trial_us={reference_us} incremental_us={}",
                started.elapsed().as_micros()
            );
        }
    }

    #[test]
    fn work_item_dispatch_identity_survives_repacking_but_not_changed_evidence() {
        let first = item("one", "original evidence");
        let second = item("two", "other evidence");
        let together = pack_work_items(&[first.clone(), second]).batches.remove(0);
        let alone = pack_work_items(&[first]).batches.remove(0);
        assert_ne!(together.digest, alone.digest);
        assert_eq!(
            together.work_item_digests["one"],
            alone.work_item_digests["one"]
        );
        let changed = pack_work_items(&[item("one", "changed evidence")])
            .batches
            .remove(0);
        assert_ne!(
            alone.work_item_digests["one"],
            changed.work_item_digests["one"]
        );
    }

    #[tokio::test]
    async fn global_orchestration_admits_before_preparation_and_cancels_waiters() {
        let held = orchestration_slot().acquire().await.unwrap();
        let context = JevSessionContext {
            session_identity: "synthetic-session".to_owned(),
            input_revision: "synthetic-revision".to_owned(),
            check_context: Value::Null,
            limitations: Vec::new(),
            reference_snapshots: Vec::new(),
            evidence_store: JevEvidenceStore::default(),
        };
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let outcome = tokio::time::timeout(
            Duration::from_millis(20),
            run_jev_check(
                &ManyItemsCheck,
                &context,
                JevRunProgress::default(),
                |batch| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(response_for(&batch.request)) }
                },
                |_| Ok(()),
            ),
        )
        .await;
        assert!(outcome.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(held);
        let permit = tokio::time::timeout(Duration::from_secs(10), orchestration_slot().acquire())
            .await
            .unwrap()
            .unwrap();
        drop(permit);
    }

    #[test]
    fn packing_caps_retained_batches_and_reports_every_excluded_item() {
        let items = (0..300)
            .map(|index| item(&index.to_string(), &"x".repeat(16 * 1024)))
            .collect::<Vec<_>>();
        let packed = pack_work_items(&items);
        let bytes: usize = packed
            .batches
            .iter()
            .map(|batch| jev_batch_resident_bytes(batch).unwrap())
            .sum();
        assert!(bytes <= MAX_QUEUED_BYTES);
        assert!(bytes > MAX_QUEUED_BYTES / 2);
        assert!(!packed.skipped_item_ids.is_empty());
        assert_eq!(
            packed
                .batches
                .iter()
                .map(|batch| batch.work_item_ids.len())
                .sum::<usize>()
                + packed.skipped_item_ids.len(),
            items.len()
        );
        println!(
            "phase6 queue items=300 retained_charge_bytes={bytes} batches={} skipped={}",
            packed.batches.len(),
            packed.skipped_item_ids.len()
        );
    }

    #[tokio::test]
    async fn queued_and_local_binding_bytes_are_checked_before_dispatch() {
        let mut large = item("large-binding", "small provider state");
        large.window.evidence[0].source_id = "x".repeat(512 * 1024);
        let packed = pack_work_items(&[large]);
        assert!(packed.batches.is_empty());
        assert_eq!(packed.skipped_item_ids, ["large-binding"]);
        let batches = (0..300)
            .map(|index| {
                pack_work_items(&[item(&index.to_string(), "small")])
                    .batches
                    .remove(0)
            })
            .collect();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let outcome = execute_batches(
            &|batch: std::sync::Arc<JevRequestBatch>| {
                calls.fetch_add(1, Ordering::SeqCst);
                async move { Ok(response_for(&batch.request)) }
            },
            batches,
            |_, _| Ok(()),
        )
        .await;
        assert!(matches!(
            outcome,
            Err(JevError::RequestTooLarge {
                maximum: 16777216,
                ..
            })
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn bounded_json_writer_matches_serialization_and_hash_without_scratch_buffers() {
        let value = json!({"text": "line\n\"quoted\" é😀", "items": [1, 2, 3]});
        let bytes = serde_json::to_vec(&value).unwrap();
        let mut writer = JsonMeasure::new(bytes.len());
        serde_json::to_writer(&mut writer, &value).unwrap();
        assert_eq!(writer.bytes, bytes.len());
        assert_eq!(
            format_digest_hex(writer.hash.finalize().as_slice()),
            digest_hex(&bytes)
        );
        assert!(json_bytes(&value, bytes.len() - 1).is_err());
    }

    #[tokio::test]
    async fn responses_checkpoint_before_slow_wave_tail_and_keep_partial_failures() {
        let batches = (0..3)
            .map(|index| {
                pack_work_items(&[item(&index.to_string(), "evidence")])
                    .batches
                    .remove(0)
            })
            .collect::<Vec<_>>();
        let completed = std::sync::atomic::AtomicUsize::new(0);
        let mut settled = Vec::new();
        let started = std::time::Instant::now();
        execute_batches(
            &|batch: std::sync::Arc<JevRequestBatch>| {
                let completed = &completed;
                async move {
                    let id = batch.work_item_ids[0].clone();
                    tokio::time::sleep(Duration::from_millis(if id == "0" { 500 } else { 20 }))
                        .await;
                    completed.fetch_add(1, Ordering::SeqCst);
                    if id == "2" {
                        Err(JevError::RequestOutcomeUnknown)
                    } else {
                        Ok(response_for(&batch.request))
                    }
                }
            },
            batches,
            |batch, result| {
                if settled.is_empty() {
                    assert_eq!(batch.work_item_ids, ["1"]);
                    assert_eq!(completed.load(Ordering::SeqCst), 1);
                    assert!(started.elapsed() < Duration::from_millis(500));
                }
                settled.push((batch.work_item_ids[0].clone(), result.is_ok()));
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(
            settled,
            [
                ("1".to_owned(), true),
                ("2".to_owned(), false),
                ("0".to_owned(), true)
            ]
        );
        println!(
            "phase6 first checkpoint precedes 500ms tail; wave_ms={}",
            started.elapsed().as_millis()
        );
    }

    #[tokio::test]
    async fn failed_checkpoint_stops_dispatch_before_another_paid_request() {
        let batches = (0..3)
            .map(|index| {
                pack_work_items(&[item(&index.to_string(), "evidence")])
                    .batches
                    .remove(0)
            })
            .collect();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let result = execute_batches(
            &|batch: std::sync::Arc<JevRequestBatch>| {
                calls.fetch_add(1, Ordering::SeqCst);
                async move { Ok(response_for(&batch.request)) }
            },
            batches,
            |_, _| Err(JevError::ProgressStorageFailure),
        )
        .await;
        assert_eq!(result, Err(JevError::ProgressStorageFailure));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

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
            window: JevInputWindow {
                fields: json!({"text": text}),
                evidence: vec![JevEvidenceReference {
                    part_id: "text".to_owned(),
                    source_id: id.to_owned(),
                    content_kind: "assistant_text".to_owned(),
                    role: JevEvidenceRole::Candidate,
                }],
            },
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
        assert_eq!(batch.evidence_owners["one"][0].source_id, "one");
        assert!(!batch.request.state.to_string().contains("source_id"));
        assert!(!batch.request.state.to_string().contains("\"one\""));
        let results = unpack_jev_response(batch, &response_for(&batch.request)).unwrap();
        assert_eq!(results[0].evidence, batch.evidence_owners["one"]);
        let instructions = &batch.request.questions.values().next().unwrap();
        assert!(
            matches!(instructions, JevQuestion::Choice { instructions, .. } if instructions.to_string().contains("work_items["))
        );
    }

    #[test]
    fn evidence_store_uses_bounded_byte_ranges_and_rejects_duplicate_bindings() {
        let mut store = JevEvidenceStore::for_publication(9);
        store
            .insert(
                "event-1",
                JevInputField::AssistantMessage,
                "first".to_owned(),
            )
            .unwrap();
        assert_eq!(
            store.insert(
                "event-1",
                JevInputField::AssistantMessage,
                "updated".to_owned(),
            ),
            Err(JevError::InvalidCheckContext)
        );
        assert_eq!(store.total_bytes(), "first".len());
        assert_eq!(
            store.get("event-1", JevInputField::AssistantMessage),
            Some("first")
        );
        assert_eq!(store.publication_fence(), Some(9));

        let mut bounded = JevEvidenceStore::default();
        bounded
            .insert(
                "event-1",
                JevInputField::AssistantMessage,
                "x".repeat(MAX_SELECTED_EVIDENCE_BYTES),
            )
            .unwrap();
        assert_eq!(
            bounded.insert("event-2", JevInputField::AssistantMessage, "x".to_owned()),
            Err(JevError::InvalidCheckContext)
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
    fn state_and_question_budget_is_checked_separately_from_full_request_size() {
        let large_state = item("large-state", &"x".repeat(32 * 1024));
        let packed = pack_work_items(std::slice::from_ref(&large_state));
        assert!(packed.batches.is_empty());
        assert_eq!(packed.skipped_item_ids, vec!["large-state"]);

        let request = JevRequest {
            model: PINNED_MODEL.to_owned(),
            state: large_state.window.fields,
            questions: large_state.questions,
        };
        assert!(matches!(
            validate_jev_request(&request),
            Err(JevError::RequestTooLarge { .. })
        ));
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

    #[test]
    fn valid_distribution_is_accepted_when_choice_disagrees() {
        let request = JevRequest {
            model: PINNED_MODEL.to_owned(),
            state: json!("sample"),
            questions: BTreeMap::from([("q".to_owned(), choice())]),
        };
        let response = JevResponse {
            model: PINNED_MODEL.to_owned(),
            answers: BTreeMap::from([(
                "q".to_owned(),
                JevAnswer::Choice {
                    choice: "yes".to_owned(),
                    probabilities: BTreeMap::from([
                        ("yes".to_owned(), 0.1),
                        ("no".to_owned(), 0.9),
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
        let JevAnswer::Choice {
            choice,
            probabilities,
            ..
        } = &response.answers["q"]
        else {
            panic!("test response uses Choice")
        };
        assert_eq!(
            highest_probability_choice(choice, probabilities),
            Some("no")
        );
        assert_eq!(
            highest_probability_choice(
                "yes",
                &BTreeMap::from([("yes".to_owned(), 0.5), ("no".to_owned(), 0.5)])
            ),
            Some("yes")
        );
        let mut invalid = response;
        if let JevAnswer::Choice { probabilities, .. } = invalid.answers.get_mut("q").unwrap() {
            probabilities.insert("no".to_owned(), 1.2);
        }
        assert_eq!(
            validate_jev_response(&invalid, &request),
            Err(JevError::InvalidChoiceDistribution)
        );
    }

    struct ResumeCheck {
        revisions: JevCheckRevisions,
    }

    impl JevCheck for ResumeCheck {
        type Prepared = Value;
        type Result = Value;

        fn id(&self) -> &'static str {
            "resume_test"
        }

        fn revisions(&self) -> JevCheckRevisions {
            self.revisions
        }

        fn prepare(
            &self,
            context: &JevSessionContext,
        ) -> Result<JevCheckPlan<Self::Prepared>, JevError> {
            let questions = BTreeMap::from([("decision".to_owned(), choice())]);
            let work_items = ["first", "second"]
                .into_iter()
                .map(|id| JevWorkItem {
                    id: id.to_owned(),
                    window: JevInputWindow {
                        fields: json!({"text": "x".repeat(20_000)}),
                        evidence: vec![JevEvidenceReference {
                            part_id: "text".to_owned(),
                            source_id: format!("event-{id}"),
                            content_kind: "assistant_text".to_owned(),
                            role: JevEvidenceRole::Candidate,
                        }],
                    },
                    questions: questions.clone(),
                })
                .collect::<Vec<_>>();
            Ok(JevCheckPlan {
                check_id: self.id().to_owned(),
                input_revision: context.input_revision.clone(),
                revisions: self.revisions(),
                work_items,
                skipped_item_ids: Vec::new(),
                coverage: JevCoverage {
                    selected_items: 2,
                    ..JevCoverage::default()
                },
                prepared: Value::Null,
            })
        }

        fn reduce(
            &self,
            plan: &JevCheckPlan<Self::Prepared>,
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

    struct ManyItemsCheck;

    impl JevCheck for ManyItemsCheck {
        type Prepared = Value;
        type Result = usize;

        fn id(&self) -> &'static str {
            "many_items_test"
        }

        fn revisions(&self) -> JevCheckRevisions {
            JevCheckRevisions {
                projection: 1,
                chunking: 1,
                questions: 1,
                reducer: 1,
            }
        }

        fn prepare(
            &self,
            context: &JevSessionContext,
        ) -> Result<JevCheckPlan<Self::Prepared>, JevError> {
            let questions = BTreeMap::from([("decision".to_owned(), choice())]);
            let work_items = (0..70)
                .map(|index| JevWorkItem {
                    id: format!("item-{index}"),
                    window: JevInputWindow {
                        fields: json!({"text": "synthetic evidence ".repeat(280)}),
                        evidence: vec![JevEvidenceReference {
                            part_id: "text".to_owned(),
                            source_id: format!("event-{index}"),
                            content_kind: "assistant_text".to_owned(),
                            role: JevEvidenceRole::Candidate,
                        }],
                    },
                    questions: questions.clone(),
                })
                .collect::<Vec<_>>();
            Ok(JevCheckPlan {
                check_id: self.id().to_owned(),
                input_revision: context.input_revision.clone(),
                revisions: self.revisions(),
                work_items,
                skipped_item_ids: Vec::new(),
                coverage: JevCoverage::default(),
                prepared: Value::Null,
            })
        }

        fn reduce(
            &self,
            _plan: &JevCheckPlan<Self::Prepared>,
            results: &[JevWorkItemResult],
            complete: bool,
        ) -> Result<Self::Result, JevError> {
            if !complete {
                return Err(JevError::InvalidCheckPlan);
            }
            Ok(results.len())
        }
    }

    struct OutputEnabledCheck;

    impl JevCheck for OutputEnabledCheck {
        type Prepared = Value;
        type Result = usize;

        fn id(&self) -> &'static str {
            "output_enabled_test"
        }

        fn revisions(&self) -> JevCheckRevisions {
            check_revisions(1, 1, 1, 1)
        }

        fn input_selection(&self) -> JevInputSelection {
            JevInputSelection::from_fields(&[JevInputField::BashCommandOutput])
        }

        fn prepare(&self, context: &JevSessionContext) -> Result<JevCheckPlan<Value>, JevError> {
            let evidence = JevEvidenceReference {
                part_id: "tool_output".to_owned(),
                source_id: "local-event-1".to_owned(),
                content_kind: "tool_result".to_owned(),
                role: JevEvidenceRole::Candidate,
            };
            let output = self
                .retrieve_evidence(context, &evidence, JevInputField::BashCommandOutput)?
                .ok_or(JevError::InvalidCheckContext)?;
            Ok(JevCheckPlan {
                check_id: self.id().to_owned(),
                input_revision: context.input_revision.clone(),
                revisions: self.revisions(),
                work_items: vec![JevWorkItem {
                    id: "output-item".to_owned(),
                    window: JevInputWindow {
                        fields: json!({"tool_output": output}),
                        evidence: vec![evidence],
                    },
                    questions: BTreeMap::from([("decision".to_owned(), choice())]),
                }],
                skipped_item_ids: Vec::new(),
                coverage: JevCoverage {
                    selected_items: 1,
                    ..JevCoverage::default()
                },
                prepared: Value::Null,
            })
        }

        fn reduce(
            &self,
            _plan: &JevCheckPlan<Value>,
            results: &[JevWorkItemResult],
            complete: bool,
        ) -> Result<Self::Result, JevError> {
            if !complete {
                return Err(JevError::InvalidCheckPlan);
            }
            Ok(results.len())
        }
    }

    struct EmptyCountingCheck {
        prepare_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl JevCheck for EmptyCountingCheck {
        type Prepared = Value;
        type Result = usize;

        fn id(&self) -> &'static str {
            "empty_counting_test"
        }

        fn revisions(&self) -> JevCheckRevisions {
            check_revisions(1, 1, 1, 1)
        }

        fn prepare(&self, context: &JevSessionContext) -> Result<JevCheckPlan<Value>, JevError> {
            self.prepare_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(JevCheckPlan {
                check_id: self.id().to_owned(),
                input_revision: context.input_revision.clone(),
                revisions: self.revisions(),
                work_items: Vec::new(),
                skipped_item_ids: Vec::new(),
                coverage: JevCoverage::default(),
                prepared: Value::Null,
            })
        }

        fn reduce(
            &self,
            _plan: &JevCheckPlan<Value>,
            results: &[JevWorkItemResult],
            _complete: bool,
        ) -> Result<usize, JevError> {
            Ok(results.len())
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
    async fn prepared_runner_uses_the_admitted_plan_without_preparing_again() {
        let check = EmptyCountingCheck {
            prepare_calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let context = JevSessionContext {
            input_revision: "prepared-input".to_owned(),
            session_identity: "prepared-session".to_owned(),
            check_context: Value::Null,
            limitations: Vec::new(),
            evidence_store: JevEvidenceStore::default(),
            reference_snapshots: Vec::new(),
        };
        let orchestration = admit_jev_orchestration().await.unwrap();
        let mut plan = check.prepare(&context).unwrap();
        let calls = check.prepare_calls.clone();
        let result = run_jev_check_prepared(
            &check,
            &context,
            &mut plan,
            JevRunProgress::default(),
            orchestration,
            |_| async { unreachable!("empty prepared plan must not dispatch") },
            |_| Ok(()),
        )
        .await
        .unwrap();

        assert!(result.complete);
        assert_eq!(result.result, 0);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn generic_runner_resumes_partial_progress_without_repeating_completed_batches() {
        let context = JevSessionContext {
            input_revision: "immutable-input".to_owned(),
            session_identity: "synthetic-session".to_owned(),
            check_context: Value::Null,
            limitations: Vec::new(),
            evidence_store: JevEvidenceStore::default(),
            reference_snapshots: Vec::new(),
        };
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let mut saved = Vec::new();
        let check = ResumeCheck {
            revisions: check_revisions(1, 1, 1, 1),
        };
        let first = run_jev_check(
            &check,
            &context,
            JevRunProgress::default(),
            |batch| {
                let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
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
        assert_eq!(first.failure, None);
        assert_eq!(first.progress.results.len(), 1);
        assert_eq!(
            first
                .progress
                .results
                .values()
                .map(|result| &result.request_id)
                .collect::<BTreeSet<_>>()
                .len(),
            1
        );
        assert_eq!(saved.len(), 2);

        let resumed_calls = std::sync::atomic::AtomicUsize::new(0);
        let resumed = run_jev_check(
            &check,
            &context,
            first.progress,
            |batch| {
                resumed_calls.fetch_add(1, Ordering::SeqCst);
                async move { Ok(response_for(&batch.request)) }
            },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert!(resumed.complete);
        assert_eq!(resumed_calls.load(Ordering::SeqCst), 1);
        assert_eq!(resumed.progress.results.len(), 2);
        assert_eq!(resumed.progress.request_count, 2);
        assert_eq!(
            resumed.progress.results["first"].evidence[0].source_id,
            "event-first"
        );
        assert_eq!(resumed.result["completed"], true);
    }

    #[tokio::test]
    async fn check_contract_revisions_invalidate_saved_work() {
        let context = JevSessionContext {
            input_revision: "immutable-input".to_owned(),
            session_identity: "synthetic-session".to_owned(),
            check_context: Value::Null,
            limitations: Vec::new(),
            evidence_store: JevEvidenceStore::default(),
            reference_snapshots: Vec::new(),
        };
        let original_check = ResumeCheck {
            revisions: check_revisions(1, 1, 1, 1),
        };
        let original = run_jev_check(
            &original_check,
            &context,
            JevRunProgress::default(),
            |batch| async move { Ok(response_for(&batch.request)) },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert!(original.complete);
        assert_eq!(original.progress.results.len(), 2);
        assert_ne!(original.progress.input_revision, context.input_revision);

        for revisions in [
            check_revisions(2, 1, 1, 1),
            check_revisions(1, 2, 1, 1),
            check_revisions(1, 1, 2, 1),
            check_revisions(1, 1, 1, 2),
        ] {
            let changed_check = ResumeCheck { revisions };
            let calls = std::sync::atomic::AtomicUsize::new(0);
            let changed = run_jev_check(
                &changed_check,
                &context,
                original.progress.clone(),
                |batch| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(response_for(&batch.request)) }
                },
                |_| Ok(()),
            )
            .await
            .unwrap();
            assert!(changed.complete);
            assert_eq!(changed.progress.results.len(), 2);
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }

        let mut changed_reference = context.clone();
        changed_reference
            .reference_snapshots
            .push(JevReferenceSnapshot {
                kind: "policy".to_owned(),
                identity: "policy-file".to_owned(),
                revision: "revision-2".to_owned(),
                fields: json!({"rule": "changed policy"}),
            });
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let changed = run_jev_check(
            &original_check,
            &changed_reference,
            original.progress.clone(),
            |batch| {
                calls.fetch_add(1, Ordering::SeqCst);
                async move { Ok(response_for(&batch.request)) }
            },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert!(changed.complete);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn runner_rejects_saved_answers_with_changed_source_bindings() {
        let context = JevSessionContext {
            input_revision: "immutable-input".to_owned(),
            session_identity: "synthetic-session".to_owned(),
            check_context: Value::Null,
            limitations: Vec::new(),
            evidence_store: JevEvidenceStore::default(),
            reference_snapshots: Vec::new(),
        };
        let check = ResumeCheck {
            revisions: check_revisions(1, 1, 1, 1),
        };
        let original = run_jev_check(
            &check,
            &context,
            JevRunProgress::default(),
            |batch| async move { Ok(response_for(&batch.request)) },
            |_| Ok(()),
        )
        .await
        .unwrap();
        let mut stale = original.progress;
        stale.results.get_mut("first").unwrap().evidence[0].source_id = "other-source".to_owned();

        let calls = std::sync::atomic::AtomicUsize::new(0);
        let resumed = run_jev_check(
            &check,
            &context,
            stale,
            |batch| {
                calls.fetch_add(1, Ordering::SeqCst);
                async move { Ok(response_for(&batch.request)) }
            },
            |_| Ok(()),
        )
        .await
        .unwrap();
        assert!(resumed.complete);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            resumed.progress.results["first"].evidence[0].source_id,
            "event-first"
        );
    }

    #[tokio::test]
    async fn generic_runner_packs_many_items_into_bounded_parallel_requests() {
        let context = JevSessionContext {
            input_revision: "many-items-input".to_owned(),
            session_identity: "synthetic-session".to_owned(),
            check_context: Value::Null,
            limitations: Vec::new(),
            evidence_store: JevEvidenceStore::default(),
            reference_snapshots: Vec::new(),
        };
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let in_flight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let maximum_in_flight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let outcome = run_jev_check(
            &ManyItemsCheck,
            &context,
            JevRunProgress::default(),
            |batch| {
                calls.fetch_add(1, Ordering::SeqCst);
                let in_flight = std::sync::Arc::clone(&in_flight);
                let maximum_in_flight = std::sync::Arc::clone(&maximum_in_flight);
                async move {
                    let active = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum_in_flight.fetch_max(active, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    Ok(response_for(&batch.request))
                }
            },
            |_| Ok(()),
        )
        .await
        .unwrap();

        assert!(outcome.complete);
        let call_count = calls.load(Ordering::SeqCst);
        assert!(call_count > 1);
        assert!(call_count < 64, "larger requests reduce provider calls");
        assert!(maximum_in_flight.load(Ordering::SeqCst) > 1);
        assert!(maximum_in_flight.load(Ordering::SeqCst) <= MAX_PARALLEL_REQUESTS);
        assert_eq!(outcome.progress.request_count, call_count);
        assert_eq!(outcome.result, 70);
    }

    #[tokio::test]
    async fn second_check_selects_output_fields_through_the_shared_runner() {
        let mut context = JevSessionContext {
            input_revision: "output-enabled-input".to_owned(),
            session_identity: "output-enabled-session".to_owned(),
            check_context: Value::Null,
            limitations: Vec::new(),
            evidence_store: JevEvidenceStore::for_publication(7),
            reference_snapshots: Vec::new(),
        };
        context
            .evidence_store
            .insert(
                "local-event-1",
                JevInputField::BashCommandOutput,
                "OUTPUT_ENABLED_SENTINEL".to_owned(),
            )
            .unwrap();
        let requests = std::sync::atomic::AtomicUsize::new(0);
        let outcome = run_jev_check(
            &OutputEnabledCheck,
            &context,
            JevRunProgress::default(),
            |batch| {
                requests.fetch_add(1, Ordering::SeqCst);
                assert!(
                    batch
                        .request
                        .state
                        .to_string()
                        .contains("OUTPUT_ENABLED_SENTINEL")
                );
                async move { Ok(response_for(&batch.request)) }
            },
            |_| Ok(()),
        )
        .await
        .unwrap();

        assert_eq!(
            OutputEnabledCheck.input_selection().bits(),
            1 << JevInputField::BashCommandOutput as u8
        );
        assert!(outcome.complete);
        assert_eq!(outcome.result, 1);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(outcome.progress.request_count, 1);
        assert_eq!(context.evidence_store.publication_fence(), Some(7));
    }

    fn check_revisions(
        projection: u32,
        chunking: u32,
        questions: u32,
        reducer: u32,
    ) -> JevCheckRevisions {
        JevCheckRevisions {
            projection,
            chunking,
            questions,
            reducer,
        }
    }
}
