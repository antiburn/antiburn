use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::analysis::SourceFormat;
use crate::analysis::jev::capabilities::ModelCapabilities;
use crate::analysis::jev::*;
use crate::analysis::jev_evidence::{
    ContentAction, ContentEventReference, SessionContentEvidence, content_action_digest,
    field_capability, select_session_content,
};
use crate::analysis::session_scope::{
    SessionScopeBoundary, SessionScopeBranch, SessionScopeError, SessionScopeSnapshot,
};
use crate::checks::ignored_instructions::sha256_hex;
use crate::checks::sampling::{Candidate, StableId};

use super::questions::ScopeQuestion;
use super::{REVISIONS, ScopeCreepResult};

const INPUT_FIELDS: [JevInputField; 14] = [
    JevInputField::UserMessage,
    JevInputField::AssistantMessage,
    JevInputField::BashCommandInput,
    JevInputField::BashCommandOutput,
    JevInputField::FileEditPath,
    JevInputField::FileEditContent,
    JevInputField::ReadFilePath,
    JevInputField::ReadFileOutput,
    JevInputField::SearchFilesQuery,
    JevInputField::SearchFilesOutput,
    JevInputField::OtherToolInput,
    JevInputField::OtherToolOutput,
    JevInputField::UserAnswer,
    JevInputField::PlanReference,
];

pub const INPUT_SELECTION: JevInputSelection = JevInputSelection::from_fields(&INPUT_FIELDS);

/// Optional workflows do not become source requirements when unavailable.
pub fn input_selection_for_source(source: SourceFormat) -> JevInputSelection {
    JevInputSelection::from_fields(
        &INPUT_FIELDS
            .into_iter()
            .filter(|field| {
                !matches!(
                    field,
                    JevInputField::UserAnswer | JevInputField::PlanReference
                ) || field_capability(source, *field) != JevFieldCapability::Unavailable
            })
            .collect::<Vec<_>>(),
    )
}

const MAX_EVENTS: usize = 65_536;
const MAX_ACTIVITY_BYTES: usize = 16 * 1024 * 1024;

/// Obtain content and retained scope from the same selected source publication.
/// A bounded recent window can omit earlier context. Scope completeness records
/// this limit. Enablement filters candidates, not retained dependency context.
#[derive(Debug, Clone)]
pub struct ScopeCreepInput {
    pub scope: Arc<SessionScopeSnapshot>,
    pub content: SessionContentEvidence,
    pub boundary: SessionScopeBoundary,
    pub source_generation: i64,
    /// Exact work action IDs already charged by Ignored Instructions.
    pub ignored_instruction_work_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkBinding {
    pub reference: ContentEventReference,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkGroup {
    pub id: String,
    pub semantic_digest: String,
    pub work: Vec<WorkBinding>,
    pub context: Vec<WorkBinding>,
    pub window_ids: Vec<String>,
    pub limitation: Option<String>,
    pub task_scope: Vec<JevEvidenceReference>,
    pub observation_kind: WorkObservationKind,
    #[serde(default)]
    pub selected_excerpts: Vec<super::ScopeExcerpt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkObservationKind {
    Attempt,
    Proposal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeCreepPrepared {
    pub scope_digest: String,
    pub semantic_epoch: StableId,
    pub source_generation: i64,
    pub publication_fence: i64,
    pub groups: Vec<WorkGroup>,
    pub scope_bindings: Vec<JevEvidenceReference>,
    pub session_limitation: Option<SessionScopeError>,
}

/// Persist this inventory with the adapter cursor, without source bodies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeDescriptorInventory {
    pub source_revision: String,
    pub next_action: usize,
    pub groups: Vec<WorkGroup>,
    pub complete: bool,
    #[serde(default)]
    pub limitation: Option<ScopeInventoryLimit>,
    #[serde(default)]
    pub descriptor_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeInventoryLimit {
    SerializedBudget,
}

pub const MAX_SCOPE_DESCRIPTOR_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone)]
struct InputIndex {
    actions: BTreeMap<String, usize>,
    results: BTreeMap<(String, String), Vec<usize>>,
    requests: BTreeMap<(String, String), usize>,
}

impl InputIndex {
    fn new(input: &ScopeCreepInput) -> Self {
        let mut index = Self {
            actions: BTreeMap::new(),
            results: BTreeMap::new(),
            requests: BTreeMap::new(),
        };
        for (position, action) in input.content.actions.iter().enumerate() {
            index.actions.insert(action.reference.id.clone(), position);
            if action.kind == "tool_input"
                && let Some((name, call)) =
                    action.tool_name.as_ref().zip(action.tool_call_id.as_ref())
            {
                *index
                    .requests
                    .entry((name.clone(), call.clone()))
                    .or_default() += 1;
            }
            if action.kind == "tool_result"
                && let Some((name, call)) =
                    action.tool_name.as_ref().zip(action.tool_call_id.as_ref())
            {
                index
                    .results
                    .entry((name.clone(), call.clone()))
                    .or_default()
                    .push(position);
            }
        }
        index
    }
}

/// One immutable source-bound input. This type does not own a worker or client.
#[derive(Debug, Clone)]
pub struct ScopeCreepCheck {
    pub(super) input: ScopeCreepInput,
    context: JevSessionContext,
    scope_digest: String,
    selection: JevInputSelection,
    index: InputIndex,
}

impl ScopeCreepCheck {
    /// Retain only the newest two bound operations for this snapshot.
    pub fn enumerate_descriptors(
        &self,
        inventory: &mut ScopeDescriptorInventory,
    ) -> Result<(), JevError> {
        if inventory.source_revision != self.context.input_revision {
            *inventory = ScopeDescriptorInventory {
                source_revision: self.context.input_revision.clone(),
                ..Default::default()
            };
        }
        if inventory.next_action > self.input.content.actions.len() {
            return Err(JevError::InvalidCheckContext);
        }
        if inventory.complete || inventory.limitation.is_some() {
            return Ok(());
        }
        inventory.groups.clear();
        for index in (0..self.input.content.actions.len()).rev() {
            if let Some(group) = self.descriptor_group(index)? {
                inventory.groups.push(group);
                if inventory.groups.len() == 2 {
                    break;
                }
            }
        }
        inventory.groups.reverse();
        inventory.descriptor_bytes = serde_json::to_vec(&inventory.groups)
            .map_err(|_| JevError::InvalidCheckContext)?
            .len();
        inventory.next_action = self.input.content.actions.len();
        inventory.complete = true;
        Ok(())
    }

    pub fn descriptor_candidates(
        &self,
        inventory: &ScopeDescriptorInventory,
        capabilities: &ModelCapabilities,
    ) -> Result<Vec<Candidate>, JevError> {
        self.validate_inventory(inventory)?;
        let epoch = self.semantic_epoch(capabilities)?;
        Ok(inventory
            .groups
            .iter()
            .rev()
            .take(1)
            .map(|group| group_candidate(group, epoch))
            .collect())
    }

    pub fn descriptor_chronology(inventory: &ScopeDescriptorInventory) -> Vec<StableId> {
        inventory
            .groups
            .iter()
            .rev()
            .take(1)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|group| StableId::new("scope_work", &[group.id.as_bytes()]))
            .collect()
    }

    pub fn semantic_epoch(&self, capabilities: &ModelCapabilities) -> Result<StableId, JevError> {
        Ok(StableId::new(
            "scope_semantics",
            &[
                self.context.session_identity.as_bytes(),
                capabilities.model.as_bytes(),
                &serde_json::to_vec(&capabilities.model_revision)
                    .map_err(|_| JevError::InvalidCheckContext)?,
                &serde_json::to_vec(&REVISIONS).map_err(|_| JevError::InvalidCheckContext)?,
                &serde_json::to_vec(capabilities).map_err(|_| JevError::InvalidCheckContext)?,
            ],
        ))
    }

    fn validate_inventory(&self, inventory: &ScopeDescriptorInventory) -> Result<(), JevError> {
        if inventory.source_revision != self.context.input_revision
            || inventory.next_action > self.input.content.actions.len()
            || inventory.groups.len() > self.input.content.actions.len()
            || (inventory.complete
                && (inventory.next_action != self.input.content.actions.len()
                    || inventory.limitation.is_some()))
            || (!inventory.complete
                && inventory.next_action == self.input.content.actions.len()
                && inventory.limitation.is_none())
        {
            return Err(JevError::InvalidCheckContext);
        }
        Ok(())
    }

    /// Hydrate only selected groups from the cached immutable source.
    pub fn prepare_descriptors(
        &self,
        inventory: &ScopeDescriptorInventory,
        capabilities: &ModelCapabilities,
        selected: &BTreeSet<StableId>,
    ) -> Result<JevCheckPlan<ScopeCreepPrepared>, JevError> {
        self.validate_inventory(inventory)?;
        let groups: Vec<_> = inventory
            .groups
            .iter()
            .filter(|group| selected.contains(&StableId::new("scope_work", &[group.id.as_bytes()])))
            .cloned()
            .collect();
        if groups.len() != selected.len() || selected.len() > 2 {
            return Err(JevError::InvalidCheckPlan);
        }
        for group in &groups {
            if *group != self.canonical_group(group)? {
                return Err(JevError::InvalidCheckPlan);
            }
        }
        let mut plan = self.prepare_groups(self.context(), capabilities, groups)?;
        plan.coverage.not_selected_items = inventory.groups.len().saturating_sub(selected.len());
        if plan.coverage.not_selected_items > 0 {
            plan.coverage.processing_limit_reached = true;
            plan.coverage
                .limitations
                .push("candidate_limit_reached".into());
        }
        if !inventory.complete {
            plan.coverage.processing_limit_reached = true;
            plan.coverage
                .limitations
                .push("descriptor_enumeration_incomplete".into());
        }
        if inventory.limitation.is_some() {
            plan.coverage
                .limitations
                .push("descriptor_storage_budget_reached".into());
        }
        Ok(plan)
    }

    pub(super) fn canonical_group(&self, group: &WorkGroup) -> Result<WorkGroup, JevError> {
        let anchor = group.work.first().ok_or(JevError::InvalidCheckPlan)?;
        let index = self
            .index
            .actions
            .get(&anchor.reference.id)
            .copied()
            .filter(|index| self.input.content.actions[*index].reference == anchor.reference)
            .ok_or(JevError::InvalidCheckPlan)?;
        self.descriptor_group(index)?
            .ok_or(JevError::InvalidCheckPlan)
    }

    pub(super) fn descriptor_group(&self, index: usize) -> Result<Option<WorkGroup>, JevError> {
        let Some(mut group) = form_group(&self.input, &self.index, index)? else {
            return Ok(None);
        };
        let selected = select_scope_records(
            self.input.scope.as_ref(),
            &group.work[0].reference,
            &self.index,
        );
        let records = selected
            .into_iter()
            .map(|index| {
                (
                    &self.input.scope.occurrences()[index],
                    &self.input.scope.values()[self.input.scope.occurrences()[index].value_index],
                )
            })
            .collect::<Vec<_>>();
        group.semantic_digest = digest(&(
            records,
            self.input.scope.limitations(),
            self.input.content.complete,
            &self.input.content.limitations,
        ))?;
        Ok(Some(group))
    }

    pub fn retained_descriptors(&self, groups: &[WorkGroup]) -> Result<Vec<WorkGroup>, JevError> {
        let mut retained = Vec::new();
        for previous in groups {
            for binding in &previous.work {
                let Some(index) = self.index.actions.get(&binding.reference.id).copied() else {
                    continue;
                };
                if let Some(group) = self.descriptor_group(index)?
                    && group.id == previous.id
                    && group.semantic_digest == previous.semantic_digest
                    && group.work == previous.work
                    && group.context == previous.context
                {
                    retained.push(group);
                    break;
                }
            }
        }
        Ok(retained)
    }

    pub fn new(mut input: ScopeCreepInput) -> Result<Self, JevError> {
        input
            .boundary
            .content_scope()
            .map_err(|_| JevError::InvalidCheckContext)?;
        if input.content.publication_fence != input.scope.publication_fence()
            || input.source_generation != input.scope.source_generation()
            || input.content.session_identity_digest.is_empty()
        {
            return Err(JevError::InvalidCheckContext);
        }
        let source_selection = input_selection_for_source(input.content.source_format);
        let selection = JevInputSelection::from_fields(
            &INPUT_FIELDS
                .into_iter()
                .filter(|field| {
                    source_selection.includes(*field)
                        || match field {
                            JevInputField::UserAnswer => input
                                .content
                                .actions
                                .iter()
                                .any(|action| !action.metadata.user_answers.is_empty()),
                            JevInputField::PlanReference => input
                                .content
                                .actions
                                .iter()
                                .any(|action| !action.metadata.plan_references.is_empty()),
                            _ => false,
                        }
                })
                .collect::<Vec<_>>(),
        );
        input.content = select_session_content(&input.content, selection);
        let source = sha256_hex(input.boundary.source_key.as_bytes());
        let thread = sha256_hex(input.boundary.thread_id.as_bytes());
        let in_branch = |reference: &ContentEventReference| {
            reference.source_key_digest == source
                && reference.thread_digest == thread
                && (reference.turn_index, reference.part_index)
                    <= (input.boundary.turn_index, input.boundary.part_index)
                && match &input.boundary.branch {
                    SessionScopeBranch::ProvenLinear => true,
                    SessionScopeBranch::NativeRecords(ids) => reference
                        .native_record_id
                        .as_ref()
                        .is_some_and(|id| ids.contains(id)),
                    SessionScopeBranch::Unresolved => false,
                }
        };
        let mut seen = BTreeSet::new();
        if input.content.actions.len() > MAX_EVENTS
            || input
                .content
                .actions
                .iter()
                .map(|action| action.text.len())
                .sum::<usize>()
                > MAX_ACTIVITY_BYTES
            || input.content.actions.iter().any(|action| {
                !in_branch(&action.reference)
                    || action.turn_scope != "main"
                    || !seen.insert(&action.reference.id)
            })
            || input
                .scope
                .occurrences()
                .iter()
                .any(|occurrence| !in_branch(&occurrence.reference))
        {
            return Err(JevError::InvalidCheckContext);
        }
        input
            .content
            .actions
            .sort_by_key(|action| (action.reference.turn_index, action.reference.part_index));
        let shared_context = scope_context(&input)?;
        let scope_digest = digest(&shared_context)?;
        let input_revision = digest(&json!({
            "scope": scope_digest, "activity": input.content.selected_input_digest,
            "revisions": REVISIONS, "ii_work": input.ignored_instruction_work_ids,
            "boundary": [input.boundary.turn_index, u64::from(input.boundary.part_index)],
            "generation": input.source_generation,
        }))?;
        let context = JevSessionContext {
            input_revision,
            session_identity: input.content.session_identity_digest.clone(),
            check_context: json!({"scope_digest": scope_digest}),
            limitations: input.content.limitations.clone(),
            reference_snapshots: Vec::new(),
            evidence_store: JevEvidenceStore::default(),
        };
        let index = InputIndex::new(&input);
        Ok(Self {
            input,
            context,
            scope_digest,
            selection,
            index,
        })
    }

    pub fn context(&self) -> &JevSessionContext {
        &self.context
    }

    pub fn policy_omitted_count(&self, admitted: usize) -> usize {
        self.input
            .content
            .actions
            .iter()
            .filter(|action| {
                is_work_anchor(action)
                    && !self
                        .input
                        .ignored_instruction_work_ids
                        .contains(&action.reference.id)
            })
            .count()
            .saturating_sub(admitted)
    }

    pub fn check_identity() -> StableId {
        StableId::new("scope_creep", &[b"1"])
    }

    /// A valid uncertain decision completes the same evidence revision.
    pub fn sampling_candidates(plan: &JevCheckPlan<ScopeCreepPrepared>) -> Vec<Candidate> {
        plan.prepared
            .groups
            .iter()
            .filter(|group| !group.window_ids.is_empty())
            .map(|group| Candidate {
                id: StableId::new("scope_work", &[group.id.as_bytes()]),
                required_answers: group_candidate(group, plan.prepared.semantic_epoch)
                    .required_answers,
            })
            .filter(|candidate| !candidate.required_answers.is_empty())
            .collect()
    }

    pub fn answer_identity(
        plan: &JevCheckPlan<ScopeCreepPrepared>,
        window: &str,
        question: ScopeQuestion,
    ) -> StableId {
        let epoch = String::from(plan.prepared.semantic_epoch);
        let group = plan
            .prepared
            .groups
            .iter()
            .find(|group| group.window_ids.iter().any(|id| id == window));
        if let Some(group) = group {
            return group_answer_identity(plan.prepared.semantic_epoch, group, question);
        }
        let target = group.map_or(window, |group| group.id.as_str());
        StableId::new(
            "scope_answer",
            &[
                epoch.as_bytes(),
                target.as_bytes(),
                question.key().as_bytes(),
            ],
        )
    }

    /// Keep the inventory plan for ledger synchronization. Execute only selected
    /// candidate IDs. The scheduler, not this method, starts the next pass.
    pub fn select_candidates(
        plan: &JevCheckPlan<ScopeCreepPrepared>,
        selected: &BTreeSet<StableId>,
    ) -> JevCheckPlan<ScopeCreepPrepared> {
        let mut selected_plan = plan.clone();
        selected_plan.prepared.groups.retain(|group| {
            !group.window_ids.is_empty()
                && selected.contains(&StableId::new("scope_work", &[group.id.as_bytes()]))
        });
        let windows: BTreeSet<_> = selected_plan
            .prepared
            .groups
            .iter()
            .flat_map(|group| group.window_ids.iter().cloned())
            .collect();
        selected_plan
            .work_items
            .retain(|item| windows.contains(&item.id));
        selected_plan.skipped_item_ids.clear();
        selected_plan.coverage.skipped_items = 0;
        selected_plan.coverage.selected_items = selected_plan.prepared.groups.len();
        selected_plan.coverage.not_selected_items = plan
            .coverage
            .selected_items
            .saturating_sub(selected_plan.prepared.groups.len());
        selected_plan
    }

    fn validate_context(&self, context: &JevSessionContext) -> Result<(), JevError> {
        if context.input_revision != self.context.input_revision
            || context.session_identity != self.context.session_identity
            || context.check_context != self.context.check_context
        {
            return Err(JevError::InvalidCheckContext);
        }
        Ok(())
    }
    pub(super) fn prepare_groups(
        &self,
        context: &JevSessionContext,
        capabilities: &ModelCapabilities,
        mut groups: Vec<WorkGroup>,
    ) -> Result<JevCheckPlan<ScopeCreepPrepared>, JevError> {
        self.validate_context(context)?;
        let omitted_groups = groups.len().saturating_sub(2);
        groups.reverse();
        groups.truncate(2);
        groups.reverse();
        let limitation = None;
        let semantic_epoch = self.semantic_epoch(capabilities)?;
        let mut work_items = Vec::new();
        for group in &mut groups {
            work_items.extend(build_windows(
                group,
                &self.input.content.actions,
                self.input.scope.as_ref(),
                self.input.content.complete,
                capabilities,
                &self.index,
                false,
            )?);
        }
        while work_items.len() > 1 && !super::compact::request_fits(&work_items, capabilities) {
            let removed = work_items.remove(0);
            if let Some(group) = groups
                .iter_mut()
                .find(|group| group.window_ids.contains(&removed.id))
            {
                group.window_ids.clear();
                group.limitation = Some("one_request_fit_limit".into());
            }
        }
        if work_items.len() == 1 && !super::compact::request_fits(&work_items, capabilities) {
            let item = work_items.pop().expect("one normal candidate");
            let group = groups
                .iter_mut()
                .find(|group| group.window_ids.contains(&item.id))
                .ok_or(JevError::InvalidCheckPlan)?;
            group.window_ids.clear();
            work_items = build_windows(
                group,
                &self.input.content.actions,
                self.input.scope.as_ref(),
                self.input.content.complete,
                capabilities,
                &self.index,
                true,
            )?;
        }
        let skipped_item_ids = groups
            .iter()
            .filter(|group| group.window_ids.is_empty())
            .map(|group| group.id.clone())
            .collect::<Vec<_>>();
        let mut limitations = context.limitations.clone();
        if !self.input.content.complete {
            limitations.push("partial_source_context".into());
        }
        if omitted_groups > 0 {
            limitations.push("candidate_limit_reached".into());
        }
        limitations.extend(groups.iter().filter_map(|group| group.limitation.clone()));
        limitations.sort();
        limitations.dedup();
        Ok(JevCheckPlan {
            check_id: self.id().into(),
            input_revision: context.input_revision.clone(),
            revisions: REVISIONS,
            coverage: JevCoverage {
                selected_items: groups.len() - skipped_item_ids.len(),
                skipped_items: skipped_item_ids.len(),
                not_selected_items: omitted_groups,
                processing_limit_reached: !skipped_item_ids.is_empty() || omitted_groups > 0,
                limitations,
            },
            skipped_item_ids,
            work_items,
            capabilities: capabilities.clone(),
            shared_context: None,
            prepared: ScopeCreepPrepared {
                scope_digest: self.scope_digest.clone(),
                semantic_epoch,
                source_generation: self.input.source_generation,
                publication_fence: self.input.scope.publication_fence(),
                groups,
                scope_bindings: self.input.scope.user_context().evidence,
                session_limitation: limitation,
            },
        })
    }
}

impl JevCheck for ScopeCreepCheck {
    type Prepared = ScopeCreepPrepared;
    type Result = ScopeCreepResult;

    fn id(&self) -> &'static str {
        "scope_creep"
    }
    fn revisions(&self) -> JevCheckRevisions {
        REVISIONS
    }
    fn input_selection(&self) -> JevInputSelection {
        self.selection
    }
    fn supports_incremental_reuse(&self) -> bool {
        true
    }
    fn incremental_identity(&self, _context: &JevSessionContext) -> Value {
        json!({"scope": self.scope_digest, "revisions": REVISIONS})
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
        self.validate_context(context)?;
        let mut groups = Vec::new();
        for index in (0..self.input.content.actions.len()).rev() {
            if let Some(group) = self.descriptor_group(index)? {
                groups.push(group);
                if groups.len() == 2 {
                    break;
                }
            }
        }
        groups.reverse();
        let mut plan = self.prepare_groups(context, capabilities, groups)?;
        let omitted = self
            .input
            .content
            .actions
            .iter()
            .filter(|action| {
                is_work_anchor(action)
                    && !self
                        .input
                        .ignored_instruction_work_ids
                        .contains(&action.reference.id)
            })
            .count()
            .saturating_sub(plan.prepared.groups.len());
        plan.coverage.not_selected_items += omitted;
        if omitted > 0 {
            plan.coverage.processing_limit_reached = true;
            plan.coverage
                .limitations
                .push("candidate_limit_reached".into());
        }
        Ok(plan)
    }

    fn reconcile(
        &self,
        item: &JevWorkItem,
        result: &JevWorkItemResult,
        context: &JevSessionContext,
    ) -> Result<Option<JevWorkItem>, JevError> {
        self.validate_context(context)?;
        if result.work_item_id != item.id {
            return Err(JevError::InvalidCheckPlan);
        }
        Ok(None)
    }

    fn reduce(
        &self,
        plan: &JevCheckPlan<Self::Prepared>,
        results: &[JevWorkItemResult],
        complete: bool,
    ) -> Result<Self::Result, JevError> {
        super::reduction::reduce(self, plan, results, complete)
    }
}

pub(super) fn digest(value: &impl Serialize) -> Result<String, JevError> {
    Ok(sha256_hex(
        &serde_json::to_vec(value).map_err(|_| JevError::InvalidCheckContext)?,
    ))
}

pub(super) fn validate_scope_activity(
    scope: &SessionScopeSnapshot,
    content: &[ContentAction],
    selected: &BTreeSet<usize>,
) -> Result<(), JevError> {
    if scope.occurrences().windows(2).any(|pair| {
        (pair[0].reference.turn_index, pair[0].reference.part_index)
            > (pair[1].reference.turn_index, pair[1].reference.part_index)
    }) {
        return Err(JevError::InvalidCheckContext);
    }
    let actions: BTreeMap<_, _> = content
        .iter()
        .map(|action| (&action.reference.id, action))
        .collect();
    for index in selected {
        let occurrence = &scope.occurrences()[*index];
        let action = actions
            .get(&occurrence.reference.id)
            .ok_or(JevError::InvalidCheckContext)?;
        if action.reference != occurrence.reference {
            return Err(JevError::InvalidCheckContext);
        }
        let value = match occurrence.field {
            JevInputField::UserMessage if action.authority == "user" => json!(action.text),
            JevInputField::AssistantMessage if action.authority == "assistant" => {
                json!(action.text)
            }
            JevInputField::UserAnswer => scope_value(
                action
                    .metadata
                    .user_answers
                    .iter()
                    .find(|answer| Some(&answer.source) == occurrence.native_source.as_ref())
                    .ok_or(JevError::InvalidCheckContext)?,
            )?,
            JevInputField::PlanReference => scope_value(
                action
                    .metadata
                    .plan_references
                    .iter()
                    .find(|plan| Some(&plan.source) == occurrence.native_source.as_ref())
                    .ok_or(JevError::InvalidCheckContext)?,
            )?,
            _ => return Err(JevError::InvalidCheckContext),
        };
        if scope.values().get(occurrence.value_index) != Some(&value) {
            return Err(JevError::InvalidCheckContext);
        }
    }
    Ok(())
}

fn scope_value(value: &impl Serialize) -> Result<Value, JevError> {
    let mut value = serde_json::to_value(value).map_err(|_| JevError::InvalidCheckContext)?;
    value
        .as_object_mut()
        .ok_or(JevError::InvalidCheckContext)?
        .remove("source");
    Ok(value)
}

fn scope_context(input: &ScopeCreepInput) -> Result<JevSharedRequestContext, JevError> {
    let mut context = input.scope.user_context();
    let occurrences = context.fields["occurrences"]
        .as_array_mut()
        .ok_or(JevError::InvalidCheckContext)?;
    for (occurrence, source) in occurrences.iter_mut().zip(input.scope.occurrences()) {
        occurrence["turn"] = json!(source.reference.turn_index);
        occurrence["part"] = json!(source.reference.part_index);
        let value = &input.scope.values()[source.value_index];
        occurrence["recorded_user_approved_plan"] = json!(
            source.field == JevInputField::PlanReference
                && source.authority
                    == crate::analysis::session_scope::ScopeAuthority::SupportingContext
                && value["status"] == "approved"
                && value["origin"] == "user"
        );
    }
    context.fields["recorded_facts"] = json!({
        "scope_complete_through_boundary": input.content.complete,
        "scope_provenance_resolved": input.scope.scope_creep_context().is_ok(),
        "assessment_boundary": {"turn": input.boundary.turn_index, "part": input.boundary.part_index},
    });
    Ok(context)
}

fn binding(action: &ContentAction) -> WorkBinding {
    WorkBinding {
        reference: action.reference.clone(),
        digest: content_action_digest(action),
    }
}

fn form_group(
    input: &ScopeCreepInput,
    lookup: &InputIndex,
    index: usize,
) -> Result<Option<WorkGroup>, JevError> {
    let actions = &input.content.actions;
    let anchor = actions.get(index).ok_or(JevError::InvalidCheckContext)?;
    if anchor.context_only {
        return Ok(None);
    }
    if !is_work_anchor(anchor) {
        return Ok(None);
    }
    if input
        .ignored_instruction_work_ids
        .contains(&anchor.reference.id)
    {
        return Ok(None);
    }
    let mut positions = vec![index];
    let unique_request = anchor
        .tool_name
        .as_ref()
        .zip(anchor.tool_call_id.as_ref())
        .is_some_and(|(name, call)| lookup.requests.get(&(name.clone(), call.clone())) == Some(&1));
    if let Some(key) = anchor.tool_name.as_ref().zip(anchor.tool_call_id.as_ref())
        && let Some(results) = lookup.results.get(&(key.0.clone(), key.1.clone()))
    {
        positions.extend(results.iter().copied());
    }
    positions.sort_unstable();
    positions.dedup();
    let work: Vec<_> = positions
        .into_iter()
        .map(|position| &actions[position])
        .collect();
    let work = work.into_iter().map(binding).collect::<Vec<_>>();
    let id = digest(&work)?;
    let has_results = work
        .iter()
        .any(|work| work.reference.id != anchor.reference.id);
    let unstable = work.iter().any(|binding| !binding.reference.stable);
    Ok(Some(WorkGroup {
        id,
        semantic_digest: String::new(),
        work,
        context: Vec::new(),
        window_ids: Vec::new(),
        task_scope: Vec::new(),
        observation_kind: WorkObservationKind::Attempt,
        selected_excerpts: Vec::new(),
        limitation: if !unique_request {
            Some("operation_request_ambiguous".into())
        } else if !has_results {
            Some("attempt_result_unavailable".into())
        } else if unstable {
            Some("unstable_work_binding".into())
        } else {
            None
        },
    }))
}

fn group_candidate(group: &WorkGroup, epoch: StableId) -> Candidate {
    Candidate {
        id: StableId::new("scope_work", &[group.id.as_bytes()]),
        required_answers: ScopeQuestion::ALL
            .into_iter()
            .map(|question| group_answer_identity(epoch, group, question))
            .collect(),
    }
}

fn group_answer_identity(epoch: StableId, group: &WorkGroup, question: ScopeQuestion) -> StableId {
    let epoch = String::from(epoch);
    let context = String::from(StableId::new(
        "scope_supporting_context",
        &group
            .context
            .iter()
            .map(|binding| binding.digest.as_bytes())
            .collect::<Vec<_>>(),
    ));
    StableId::new(
        "scope_answer",
        &[
            epoch.as_bytes(),
            group.id.as_bytes(),
            group.semantic_digest.as_bytes(),
            context.as_bytes(),
            question.key().as_bytes(),
        ],
    )
}

fn is_work_anchor(action: &ContentAction) -> bool {
    action.kind == "tool_input"
        && action.authority == "assistant"
        && !action.context_only
        && action.tool_call_id.is_some()
        && action.tool_name.is_some()
        && action.metadata.user_answers.is_empty()
        && action.metadata.plan_references.is_empty()
        && !action.normalized_fields.as_ref().is_some_and(|fields| {
            matches!(
                fields.category,
                Some(JevNormalizedCategory::ReadFile | JevNormalizedCategory::SearchFiles)
            )
        })
}

fn select_scope_records(
    scope: &SessionScopeSnapshot,
    _work: &ContentEventReference,
    _lookup: &InputIndex,
) -> BTreeSet<usize> {
    (0..scope.occurrences().len()).collect()
}

fn build_windows(
    group: &mut WorkGroup,
    actions: &[ContentAction],
    scope: &SessionScopeSnapshot,
    content_complete: bool,
    capabilities: &ModelCapabilities,
    lookup: &InputIndex,
    reduced: bool,
) -> Result<Vec<JevWorkItem>, JevError> {
    if group.limitation.is_some() {
        return Ok(Vec::new());
    }
    if group.work.iter().any(|binding| !binding.reference.stable) {
        group.limitation = Some("unstable_work_binding".into());
        return Ok(Vec::new());
    }
    let work: Vec<_> = group
        .work
        .iter()
        .map(|binding| &actions[lookup.actions[&binding.reference.id]])
        .collect();
    if work.len() != 2 || work[1].kind != "tool_result" {
        group.limitation = Some("operation_outcome_unbound".into());
        return Ok(Vec::new());
    }
    if work
        .iter()
        .any(|action| action.truncated || !action.reference.stable)
    {
        group.limitation = Some("operation_evidence_incomplete".into());
        return Ok(Vec::new());
    }
    let authority_complete =
        content_complete && scope.limitations().is_empty() && scope.scope_creep_context().is_ok();
    if !authority_complete {
        group.limitation = Some("source_context_incomplete".into());
        return Ok(Vec::new());
    }
    let selected: BTreeSet<_> = (0..scope.occurrences().len()).collect();
    super::compact::build_window(
        group,
        &work,
        actions,
        scope,
        capabilities,
        &selected,
        reduced,
    )
}
