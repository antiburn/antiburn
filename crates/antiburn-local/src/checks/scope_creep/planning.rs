use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::analysis::SourceFormat;
use crate::analysis::jev::capabilities::ModelCapabilities;
use crate::analysis::jev::text_ranges::text_ranges;
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

use super::questions::{ScopeQuestion, questions};
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

/// Obtain content and the full scope from the same selected source publication.
/// Enablement filters candidates, never the full scope or dependency context.
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
}

/// One immutable source-bound input. This type does not own a worker or client.
#[derive(Debug, Clone)]
pub struct ScopeCreepCheck {
    pub(super) input: ScopeCreepInput,
    context: JevSessionContext,
    scope_digest: String,
    shared_context: JevSharedRequestContext,
    selection: JevInputSelection,
}

impl ScopeCreepCheck {
    /// Append at most 256 descriptors and yield after one second.
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
        let started = Instant::now();
        let mut emitted = 0;
        while inventory.next_action < self.input.content.actions.len()
            && emitted < 256
            && started.elapsed() < Duration::from_secs(1)
        {
            let index = inventory.next_action;
            if let Some(group) = self.descriptor_group(index)? {
                inventory.groups.push(group);
                emitted += 1;
            }
            inventory.next_action += 1;
        }
        inventory.complete = inventory.next_action == self.input.content.actions.len();
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
            .map(|group| group_candidate(group, epoch))
            .collect())
    }

    pub fn descriptor_chronology(inventory: &ScopeDescriptorInventory) -> Vec<StableId> {
        inventory
            .groups
            .iter()
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
            ],
        ))
    }

    fn validate_inventory(&self, inventory: &ScopeDescriptorInventory) -> Result<(), JevError> {
        if inventory.source_revision != self.context.input_revision
            || inventory.next_action > self.input.content.actions.len()
            || inventory.groups.len() > inventory.next_action
            || inventory.complete != (inventory.next_action == self.input.content.actions.len())
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
        if groups.len() != selected.len() || selected.len() > 4 {
            return Err(JevError::InvalidCheckPlan);
        }
        for group in &groups {
            if *group != self.canonical_group(group)? {
                return Err(JevError::InvalidCheckPlan);
            }
        }
        let mut plan = self.prepare_groups(self.context(), capabilities, groups)?;
        plan.coverage.not_selected_items = inventory.groups.len().saturating_sub(selected.len());
        if !inventory.complete {
            plan.coverage.processing_limit_reached = true;
            plan.coverage
                .limitations
                .push("descriptor_enumeration_incomplete".into());
        }
        Ok(plan)
    }

    pub(super) fn canonical_group(&self, group: &WorkGroup) -> Result<WorkGroup, JevError> {
        let anchor = group.work.first().ok_or(JevError::InvalidCheckPlan)?;
        let index = self
            .input
            .content
            .actions
            .iter()
            .position(|action| action.reference == anchor.reference)
            .ok_or(JevError::InvalidCheckPlan)?;
        self.descriptor_group(index)?
            .ok_or(JevError::InvalidCheckPlan)
    }

    fn descriptor_group(&self, index: usize) -> Result<Option<WorkGroup>, JevError> {
        let Some(mut group) = form_group(&self.input, index)? else {
            return Ok(None);
        };
        let selected = select_scope_records(
            self.input.scope.as_ref(),
            &self.shared_context,
            &group.work[0].reference,
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
        validate_scope_activity(&input)?;
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
        Ok(Self {
            input,
            context,
            scope_digest,
            shared_context,
            selection,
        })
    }

    pub fn context(&self) -> &JevSessionContext {
        &self.context
    }

    pub fn check_identity() -> StableId {
        StableId::new("scope_creep", &[b"1"])
    }

    /// A valid uncertain decision completes the same evidence revision.
    pub fn sampling_candidates(plan: &JevCheckPlan<ScopeCreepPrepared>) -> Vec<Candidate> {
        plan.prepared
            .groups
            .iter()
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
        let limitation = None;
        let semantic_epoch = self.semantic_epoch(capabilities)?;
        let mut work_items = Vec::new();
        for group in &mut groups {
            work_items.extend(build_windows(
                group,
                &self.input.content.actions,
                self.input.scope.as_ref(),
                &self.shared_context,
                capabilities,
            )?);
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
                not_selected_items: 0,
                processing_limit_reached: !skipped_item_ids.is_empty(),
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
        let groups = (0..self.input.content.actions.len())
            .filter_map(|index| self.descriptor_group(index).transpose())
            .collect::<Result<Vec<_>, _>>()?;
        self.prepare_groups(context, capabilities, groups)
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

fn validate_scope_activity(input: &ScopeCreepInput) -> Result<(), JevError> {
    let actions: BTreeMap<_, _> = input
        .content
        .actions
        .iter()
        .map(|action| (&action.reference.id, action))
        .collect();
    let mut represented = BTreeSet::new();
    for occurrence in input.scope.occurrences() {
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
        if input.scope.values().get(occurrence.value_index) != Some(&value) {
            return Err(JevError::InvalidCheckContext);
        }
        represented.insert((&action.reference.id, occurrence.field));
    }
    for action in input.content.actions.iter() {
        for (field, required) in [
            (
                JevInputField::UserMessage,
                action.authority == "user" && matches!(action.kind.as_str(), "user" | "user_text"),
            ),
            (
                JevInputField::UserAnswer,
                !action.metadata.user_answers.is_empty(),
            ),
            (
                JevInputField::PlanReference,
                !action.metadata.plan_references.is_empty(),
            ),
        ] {
            if required && !represented.contains(&(&action.reference.id, field)) {
                return Err(JevError::InvalidCheckContext);
            }
        }
        for source in action
            .metadata
            .user_answers
            .iter()
            .map(|answer| &answer.source)
            .chain(
                action
                    .metadata
                    .plan_references
                    .iter()
                    .map(|plan| &plan.source),
            )
        {
            if !input.scope.occurrences().iter().any(|occurrence| {
                occurrence.reference == action.reference
                    && occurrence.native_source.as_ref() == Some(source)
            }) {
                return Err(JevError::InvalidCheckContext);
            }
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

pub(super) fn form_group(
    input: &ScopeCreepInput,
    index: usize,
) -> Result<Option<WorkGroup>, JevError> {
    let actions = &input.content.actions;
    let anchor = actions.get(index).ok_or(JevError::InvalidCheckContext)?;
    if anchor.context_only {
        return Ok(None);
    }
    let proposal = matches!(anchor.kind.as_str(), "assistant" | "assistant_text")
        && anchor.authority == "assistant";
    if !is_work_anchor(anchor) && !proposal {
        return Ok(None);
    }
    if proposal
        && actions
            .iter()
            .skip_while(|action| action.reference.id != anchor.reference.id)
            .skip(1)
            .take_while(|action| action.authority != "user")
            .any(|action| {
                is_work_anchor(action)
                    && action
                        .reference
                        .turn_index
                        .abs_diff(anchor.reference.turn_index)
                        <= 2
            })
    {
        return Ok(None);
    }
    if input
        .ignored_instruction_work_ids
        .contains(&anchor.reference.id)
    {
        return Ok(None);
    }
    let work: Vec<_> = actions
        .iter()
        .filter(|action| {
            action.reference.id == anchor.reference.id
                || (!proposal
                    && action.kind == "tool_result"
                    && action.tool_call_id == anchor.tool_call_id
                    && action.tool_name == anchor.tool_name)
        })
        .collect();
    let work_ids: BTreeSet<_> = work.iter().map(|action| &action.reference.id).collect();
    let start = actions
        .iter()
        .filter(|action| {
            action.authority == "user" && action.reference.turn_index <= anchor.reference.turn_index
        })
        .map(|action| action.reference.turn_index)
        .max()
        .unwrap_or(0);
    let end = actions
        .iter()
        .filter(|action| {
            action.authority == "user" && action.reference.turn_index > anchor.reference.turn_index
        })
        .map(|action| action.reference.turn_index)
        .min()
        .unwrap_or(u64::MAX);
    let context: Vec<_> = actions
        .iter()
        .filter(|action| {
            action.reference.turn_index >= start
                && action.reference.turn_index < end
                && action
                    .reference
                    .turn_index
                    .abs_diff(anchor.reference.turn_index)
                    <= 2
                && !work_ids.contains(&action.reference.id)
        })
        .take(4)
        .map(binding)
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
        context,
        window_ids: Vec::new(),
        task_scope: Vec::new(),
        observation_kind: if proposal {
            WorkObservationKind::Proposal
        } else {
            WorkObservationKind::Attempt
        },
        limitation: if !has_results && !proposal {
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

fn build_windows(
    group: &mut WorkGroup,
    actions: &[ContentAction],
    scope: &SessionScopeSnapshot,
    shared: &JevSharedRequestContext,
    capabilities: &ModelCapabilities,
) -> Result<Vec<JevWorkItem>, JevError> {
    if group.limitation.as_deref() == Some("unstable_work_binding") {
        return Ok(Vec::new());
    }
    let by_id: BTreeMap<_, _> = actions
        .iter()
        .map(|action| (action.reference.id.as_str(), action))
        .collect();
    let work: Vec<_> = group
        .work
        .iter()
        .map(|binding| by_id[binding.reference.id.as_str()])
        .collect();
    if work.iter().any(|action| action.truncated) {
        group.limitation = Some("truncated_activity".into());
    }
    let mut context = Vec::new();
    for binding in &group.context {
        let action = by_id[binding.reference.id.as_str()];
        context.push(action);
    }
    let mut item = window(group, &work, &context, 0, scope)?;
    let activity_partial = item.window.fields["recorded_facts"]["activity_content_partial"] == true;
    if activity_partial {
        group.limitation = Some("activity_content_partial".into());
    }
    let occurrences = shared.fields["occurrences"]
        .as_array()
        .ok_or(JevError::InvalidCheckContext)?;
    let values = shared.fields["values"]
        .as_array()
        .ok_or(JevError::InvalidCheckContext)?;
    let selected = select_scope_records(scope, shared, &work[0].reference);
    group.task_scope = selected
        .iter()
        .filter_map(|index| shared.evidence.get(*index).cloned())
        .enumerate()
        .map(|(index, mut reference)| {
            reference.part_id = format!("task_scope[{index}]");
            reference
        })
        .collect();
    let mut clipped = selected.len() < occurrences.len();
    let record_bytes = (64 * 1024 / selected.len().max(1)).min(8192);
    let records = selected
        .into_iter()
        .map(|index| {
            let occurrence = &occurrences[index];
            let value_index = scope.occurrences()[index].value_index;
            let value = &values[value_index];
            let encoded =
                serde_json::to_string(value).map_err(|_| JevError::InvalidCheckContext)?;
            let text = value.as_str().unwrap_or(&encoded);
            let value = if text.len() > record_bytes {
                clipped = true;
                let mut excerpt = content_excerpt(text, (record_bytes / 4).max(32), &[]);
                excerpt["range_source"] = json!(if value.is_string() {
                    "source_text"
                } else {
                    "scope_value_json"
                });
                excerpt
            } else {
                value.clone()
            };
            Ok(json!({"occurrence": occurrence, "content": value}))
        })
        .collect::<Result<Vec<_>, JevError>>()?;
    if clipped && group.limitation.is_none() {
        group.limitation = Some("scope_window_partial".into());
    }
    item.window.fields["task_scope"] = json!(records);
    item.window.fields["limitations"] = json!({"scope_window_partial": clipped, "activity_content_partial": activity_partial, "activity": group.limitation, "source": shared.fields["limitations"]});
    item.window.evidence.extend(group.task_scope.clone());
    item.id = digest(&(&group.id, &item.window.fields, &item.window.evidence))?;
    if pack_work_items_with_capabilities(&[item.clone()], capabilities)
        .batches
        .is_empty()
    {
        group.limitation = Some("work_context_too_large".into());
        return Ok(Vec::new());
    }
    group.window_ids = vec![item.id.clone()];
    Ok(vec![item])
}

fn select_scope_records(
    scope: &SessionScopeSnapshot,
    shared: &JevSharedRequestContext,
    work: &ContentEventReference,
) -> BTreeSet<usize> {
    use crate::analysis::session_scope::ScopeAuthority;
    const MAX_AUTHORITY_RECORDS: usize = 48;
    const MAX_REPLY_CONTEXT_RECORDS: usize = 16;
    let position = (work.turn_index, work.part_index);
    let authoritative: Vec<_> = scope
        .occurrences()
        .iter()
        .enumerate()
        .filter(|(index, occurrence)| {
            occurrence.authority == ScopeAuthority::User
                || shared.fields["occurrences"][*index]["recorded_user_approved_plan"] == true
        })
        .map(|(index, _)| index)
        .collect();
    let mut selected: BTreeSet<_> = if authoritative.len() <= MAX_AUTHORITY_RECORDS {
        authoritative.iter().copied().collect()
    } else {
        let before = authoritative
            .iter()
            .copied()
            .filter(|index| {
                let reference = &scope.occurrences()[*index].reference;
                (reference.turn_index, reference.part_index) <= position
            })
            .collect::<Vec<_>>();
        let after = authoritative
            .iter()
            .copied()
            .filter(|index| {
                let reference = &scope.occurrences()[*index].reference;
                (reference.turn_index, reference.part_index) > position
            })
            .collect::<Vec<_>>();
        authoritative
            .iter()
            .copied()
            .take(16)
            .chain(before.into_iter().rev().take(16))
            .chain(after.iter().copied().take(8))
            .chain(after.iter().copied().rev().take(8))
            .collect()
    };
    // Short replies need the preceding proposal before unrelated assistant text.
    let mut replies: Vec<_> = selected.iter().copied().collect();
    replies.sort_by_key(|index| {
        scope.values()[scope.occurrences()[*index].value_index]
            .as_str()
            .map(str::len)
            .unwrap_or(usize::MAX)
    });
    let mut supporting = BTreeSet::new();
    for index in replies {
        if let Some(previous) = scope.occurrences()[..index]
            .iter()
            .enumerate()
            .rev()
            .take_while(|(_, occurrence)| occurrence.authority != ScopeAuthority::User)
            .find(|(_, occurrence)| occurrence.field == JevInputField::AssistantMessage)
        {
            supporting.insert(previous.0);
        }
        if supporting.len() == MAX_REPLY_CONTEXT_RECORDS {
            break;
        }
    }
    selected.extend(supporting);
    selected
}

fn window(
    group: &WorkGroup,
    work: &[&ContentAction],
    context: &[&ContentAction],
    index: usize,
    scope: &SessionScopeSnapshot,
) -> Result<JevWorkItem, JevError> {
    let mut evidence = Vec::new();
    let mut operations = BTreeMap::new();
    let anchor = &work[0].reference;
    let task = scope
        .occurrences()
        .iter()
        .rev()
        .find(|occurrence| {
            occurrence.authority == crate::analysis::session_scope::ScopeAuthority::User
                && (
                    occurrence.reference.turn_index,
                    occurrence.reference.part_index,
                ) <= (anchor.turn_index, anchor.part_index)
        })
        .and_then(|occurrence| scope.values()[occurrence.value_index].as_str())
        .unwrap_or("");
    let terms = task
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.len() >= 5)
        .take(32)
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    let chunk_bytes = (4096 / (work.len() + context.len()).max(1) / 4).clamp(32, 512);
    let mut partial = false;
    for action in work.iter().chain(context) {
        if let Some(key) = action
            .tool_name
            .as_deref()
            .zip(action.tool_call_id.as_deref())
        {
            let ordinal = operations.len();
            operations.entry(key).or_insert(ordinal);
        }
    }
    let mut render = |actions: &[&ContentAction],
                      key: &str,
                      role: JevEvidenceRole,
                      evidence: &mut Vec<JevEvidenceReference>|
     -> Vec<Value> {
        actions.iter().enumerate().map(|(index, action)| {
            evidence.push(JevEvidenceReference { part_id: format!("{key}[{index}]"), source_id: action.reference.id.clone(), content_kind: action.kind.clone(), role });
            let operation = action.tool_name.as_deref().zip(action.tool_call_id.as_deref()).and_then(|key| operations.get(&key));
            let mut content = content_excerpt(&action.text, chunk_bytes, &terms);
            content["range_source"] = json!("selected_action_text");
            content["partial"] = json!(content["partial"] == true || action.truncated);
            partial |= content["partial"] == true || action.truncated;
            let mut normalized = serde_json::to_value(&action.normalized_fields).expect("serialize normalized fields");
            if let Some(fields) = &action.normalized_fields {
                for (field, value) in &fields.values {
                    let field_chunk_bytes = (chunk_bytes / fields.values.len().max(1)).max(32);
                    if value.len() > 4 * field_chunk_bytes {
                        let key = serde_json::to_value(field).expect("serialize field").as_str().expect("field name").to_owned();
                        let mut excerpt = content_excerpt(value, field_chunk_bytes, &terms);
                        excerpt["range_source"] = json!("normalized_field");
                        if *field == JevInputField::BashCommandInput {
                            let first_line = value.split_inclusive('\n').next().unwrap_or(value);
                            excerpt["exact_command_prefix"] = if first_line.len() <= 1024 { json!({"text": first_line, "start_byte": 0, "end_byte": first_line.len(), "governed_options_complete": false}) } else { Value::Null };
                        }
                        normalized["values"][key] = excerpt;
                        partial = true;
                    }
                }
            }
            json!({"kind": action.kind, "role": action.turn_role, "authority": action.authority, "turn": action.reference.turn_index, "part": action.reference.part_index, "tool": action.tool_name, "operation": operation, "text": if content["partial"] == true { Value::Null } else { json!(action.text) }, "content": content, "normalized_fields": normalized, "operation_state": action.metadata.state})
        }).collect()
    };
    let fields = json!({
        "bound_work": render(work, "bound_work", JevEvidenceRole::Candidate, &mut evidence),
        "supporting_activity": render(context, "supporting_activity", JevEvidenceRole::SupportingContext, &mut evidence),
        "activity_window": index, "activity_window_count": 1,
        "recorded_facts": {
            "matched_call_results_present": work.iter().any(|action| action.kind == "tool_result"),
            "selected_events_untruncated": work.iter().chain(context).all(|action| !action.truncated),
            "activity_content_partial": partial,
        },
    });
    let id = digest(&(&group.id, &fields, &evidence))?;
    Ok(JevWorkItem {
        id,
        window: JevInputWindow { fields, evidence },
        questions: questions(),
    })
}

fn content_excerpt(text: &str, chunk_bytes: usize, terms: &[String]) -> Value {
    let ranges = text_ranges(text, chunk_bytes, 0);
    let selected = if text.len() <= 4 * chunk_bytes {
        vec![(0, text.len())]
    } else {
        let middle = (ranges.len() - 1) / 2;
        let relevant = ranges
            .iter()
            .enumerate()
            .find(|&(index, &(start, end))| {
                if index == 0 || index == middle || index == ranges.len() - 1 {
                    return false;
                }
                let child = text[start..end].to_lowercase();
                terms.iter().any(|term| child.contains(term))
            })
            .map(|(index, _)| index)
            .unwrap_or((ranges.len() - 1) / 3);
        BTreeSet::from([0, relevant, middle, ranges.len() - 1])
            .into_iter()
            .map(|index| ranges[index])
            .collect()
    };
    let selected_bytes = selected
        .iter()
        .map(|(start, end)| end - start)
        .sum::<usize>();
    json!({"chunks": selected.iter().map(|&(start, end)| json!({"start_byte": start, "end_byte": end, "text": &text[start..end]})).collect::<Vec<_>>(), "total_bytes": text.len(), "partial": selected_bytes < text.len(), "range_source": "source_text"})
}
