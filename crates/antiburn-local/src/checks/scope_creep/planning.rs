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
    pub work: Vec<WorkBinding>,
    pub context: Vec<WorkBinding>,
    pub window_ids: Vec<String>,
    pub limitation: Option<String>,
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

    /// Synchronize all candidates before selecting jobs in the shared sampler.
    /// Each window needs every question. An abstention is not completion.
    pub fn sampling_candidates(plan: &JevCheckPlan<ScopeCreepPrepared>) -> Vec<Candidate> {
        plan.prepared
            .groups
            .iter()
            .map(|group| Candidate {
                id: StableId::new("scope_work", &[group.id.as_bytes()]),
                required_answers: group
                    .window_ids
                    .iter()
                    .flat_map(|window| {
                        ScopeQuestion::ALL
                            .into_iter()
                            .map(move |question| Self::answer_identity(plan, window, question))
                    })
                    .collect(),
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
        StableId::new(
            "scope_answer",
            &[
                epoch.as_bytes(),
                window.as_bytes(),
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
        let shared = self
            .input
            .scope
            .scope_creep_context()
            .map(|_| self.shared_context.clone());
        let mut limitation = shared.as_ref().err().cloned();
        if !self.input.scope.occurrences().iter().any(|occurrence| {
            occurrence.field == JevInputField::UserMessage
                && occurrence.authority == crate::analysis::session_scope::ScopeAuthority::User
        }) {
            limitation = Some(SessionScopeError::Missing(
                crate::analysis::session_scope::ScopeMissingReason::NoUserContext,
            ));
        }
        if limitation.is_none() {
            let probe = JevWorkItem {
                id: "scope_question_fit".into(),
                window: JevInputWindow {
                    fields: json!({"bound_work": [], "supporting_activity": [], "activity_window": 0, "activity_window_count": 1}),
                    evidence: Vec::new(),
                },
                questions: questions(false),
            };
            // Charge the actual follow-up questions before any initial dispatch.
            // This probe supplies no work and never becomes a model request.
            match pack_context(
                &[probe],
                &self.input.scope,
                &self.shared_context,
                capabilities,
            ) {
                Err(error) => limitation = Some(error),
                Ok(packed) if packed.batches.is_empty() => {
                    limitation = Some(SessionScopeError::ScopeTooLarge)
                }
                Ok(_) => {}
            }
        }
        if !self.input.content.complete && limitation.is_none() {
            limitation = Some(SessionScopeError::Missing(
                crate::analysis::session_scope::ScopeMissingReason::IncompleteSource,
            ));
        }
        let semantic_epoch = StableId::new(
            "scope_semantics",
            &[
                self.scope_digest.as_bytes(),
                capabilities.model.as_bytes(),
                &serde_json::to_vec(&capabilities.model_revision)
                    .map_err(|_| JevError::InvalidCheckContext)?,
                &serde_json::to_vec(&REVISIONS).map_err(|_| JevError::InvalidCheckContext)?,
            ],
        );
        let mut groups = form_groups(&self.input)?;
        let mut work_items = Vec::new();
        if limitation.is_none() {
            for group in &mut groups {
                let items = build_windows(
                    group,
                    &self.input.content.actions,
                    self.input.scope.as_ref(),
                    &self.shared_context,
                    capabilities,
                )?;
                work_items.extend(items);
            }
            if groups
                .iter()
                .any(|group| group.limitation.as_deref() == Some("scope_context_too_large"))
            {
                limitation = Some(SessionScopeError::ScopeTooLarge);
                work_items.clear();
            }
        }
        if limitation.is_some() {
            for group in &mut groups {
                group.window_ids.clear();
            }
        }
        let skipped_item_ids = groups
            .iter()
            .filter(|group| group.window_ids.is_empty())
            .map(|group| group.id.clone())
            .collect::<Vec<_>>();
        let mut limitations = context.limitations.clone();
        if let Some(error) = &limitation {
            limitations.push(match error {
                SessionScopeError::ScopeTooLarge => "scope_context_too_large".into(),
                SessionScopeError::Missing(reason) => format!("scope_context_missing:{reason:?}"),
            });
        }
        Ok(JevCheckPlan {
            check_id: self.id().into(),
            input_revision: context.input_revision.clone(),
            revisions: REVISIONS,
            coverage: JevCoverage {
                selected_items: groups.len() - skipped_item_ids.len(),
                skipped_items: skipped_item_ids.len(),
                not_selected_items: 0,
                processing_limit_reached: limitation.is_some() || !skipped_item_ids.is_empty(),
                limitations,
            },
            skipped_item_ids,
            work_items,
            capabilities: capabilities.clone(),
            shared_context: shared.ok(),
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
        // Always settle every question. Even negative performed answers need
        // authority and sufficiency checks before a clean decision is possible.
        Ok(Some(JevWorkItem {
            id: format!("{}::followup", item.id),
            window: item.window.clone(),
            questions: questions(false),
        }))
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
        "scope_complete_through_boundary": true,
        "scope_provenance_resolved": input.scope.scope_creep_context().is_ok(),
        "assessment_boundary": {"turn": input.boundary.turn_index, "part": input.boundary.part_index},
    });
    Ok(context)
}

fn pack_context(
    items: &[JevWorkItem],
    scope: &SessionScopeSnapshot,
    shared: &JevSharedRequestContext,
    capabilities: &ModelCapabilities,
) -> Result<JevPackingResult, SessionScopeError> {
    // Keep source eligibility and capability validation at the shared boundary.
    // Charge the additional exact order and provenance fields to actual packing.
    scope.pack_scope_creep(items, capabilities)?;
    Ok(pack_work_items_with_shared_context(
        items,
        capabilities,
        shared,
    ))
}

fn binding(action: &ContentAction) -> WorkBinding {
    WorkBinding {
        reference: action.reference.clone(),
        digest: content_action_digest(action),
    }
}

fn form_groups(input: &ScopeCreepInput) -> Result<Vec<WorkGroup>, JevError> {
    let actions = &input.content.actions;
    let mut groups = Vec::new();
    let mut turns = BTreeSet::new();
    for anchor in actions
        .iter()
        .filter(|action| action.kind == "tool_input" && !action.context_only)
    {
        if !is_work_anchor(anchor) || !turns.insert(anchor.reference.turn_index) {
            continue;
        }
        // A recorded assistant turn is a candidate episode, not proof of
        // coherence. The semantic questions reject mixed tasks and partial scope.
        let calls: BTreeSet<_> = actions
            .iter()
            .filter(|action| {
                action.reference.turn_index == anchor.reference.turn_index && is_work_anchor(action)
            })
            .filter_map(|action| {
                Some((
                    action.tool_call_id.as_deref()?,
                    action.tool_name.as_deref()?,
                ))
            })
            .filter(|(call, tool)| {
                !actions.iter().any(|action| {
                    action.tool_call_id.as_deref() == Some(call)
                        && action.tool_name.as_deref() == Some(tool)
                        && input
                            .ignored_instruction_work_ids
                            .contains(&action.reference.id)
                })
            })
            .collect();
        if calls.is_empty() {
            continue;
        }
        let work: Vec<_> = actions
            .iter()
            .filter(|action| {
                action
                    .tool_call_id
                    .as_deref()
                    .zip(action.tool_name.as_deref())
                    .is_some_and(|key| calls.contains(&key))
            })
            .filter(|action| matches!(action.kind.as_str(), "tool_input" | "tool_result"))
            .collect();
        let work_ids: BTreeSet<_> = work.iter().map(|action| &action.reference.id).collect();
        // User turns define task episodes. The full latest user scope is separate.
        let start = actions
            .iter()
            .filter(|action| {
                action.authority == "user"
                    && action.reference.turn_index <= anchor.reference.turn_index
            })
            .map(|action| action.reference.turn_index)
            .max()
            .unwrap_or(0);
        let end = actions
            .iter()
            .filter(|action| {
                action.authority == "user"
                    && action.reference.turn_index > anchor.reference.turn_index
            })
            .map(|action| action.reference.turn_index)
            .min()
            .unwrap_or(u64::MAX);
        let context: Vec<_> = actions
            .iter()
            .filter(|action| {
                action.reference.turn_index >= start
                    && action.reference.turn_index < end
                    && !work_ids.contains(&action.reference.id)
            })
            .map(binding)
            .collect();
        let work = work.into_iter().map(binding).collect::<Vec<_>>();
        if groups
            .iter()
            .map(|group: &WorkGroup| group.work.len() + group.context.len())
            .sum::<usize>()
            + work.len()
            + context.len()
            > MAX_EVENTS
        {
            return Err(JevError::InvalidCheckContext);
        }
        let id = digest(&work)?;
        let has_results = calls.iter().all(|(call, tool)| {
            actions.iter().any(|action| {
                action.kind == "tool_result"
                    && action.tool_call_id.as_deref() == Some(call)
                    && action.tool_name.as_deref() == Some(tool)
            })
        });
        let multiple_inputs = calls.iter().any(|(call, tool)| {
            actions
                .iter()
                .filter(|action| {
                    action.kind == "tool_input"
                        && action.tool_call_id.as_deref() == Some(call)
                        && action.tool_name.as_deref() == Some(tool)
                })
                .count()
                != 1
        });
        let unstable = work.iter().any(|binding| !binding.reference.stable);
        groups.push(WorkGroup {
            id,
            work,
            context,
            window_ids: Vec::new(),
            limitation: if multiple_inputs {
                Some("ambiguous_tool_call_binding".into())
            } else if !has_results {
                Some("performed_result_unavailable".into())
            } else if unstable {
                Some("unstable_work_binding".into())
            } else {
                None
            },
        });
    }
    Ok(groups)
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
    if group.limitation.is_some() {
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
    if work.iter().any(|action| action.truncated)
        || group
            .context
            .iter()
            .any(|binding| by_id[binding.reference.id.as_str()].truncated)
    {
        group.limitation = Some("truncated_activity".into());
        return Ok(Vec::new());
    }
    let mut windows = Vec::new();
    let mut context = Vec::new();
    let fits = |item: &JevWorkItem| -> Result<bool, SessionScopeError> {
        let mut all_questions = item.clone();
        all_questions.questions.extend(questions(false));
        Ok(
            !pack_context(&[all_questions], scope, shared, capabilities)?
                .batches
                .is_empty(),
        )
    };
    let bare = window(group, &work, &[], 0, scope)?;
    match fits(&bare) {
        Err(SessionScopeError::ScopeTooLarge) => {
            group.limitation = Some("scope_context_too_large".into());
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(match error {
                SessionScopeError::Missing(_) => JevError::InvalidCheckContext,
                SessionScopeError::ScopeTooLarge => JevError::InvalidCheckPlan,
            });
        }
        Ok(false) => {
            group.limitation = Some("work_context_too_large".into());
            return Ok(Vec::new());
        }
        Ok(true) => {}
    }
    // Split only supporting activity at recorded event boundaries. Every window
    // retains the exact work and complete scope. Never clip an event's text.
    for binding in &group.context {
        let action = by_id[binding.reference.id.as_str()];
        context.push(action);
        let item = window(group, &work, &context, windows.len(), scope)?;
        if !fits(&item).map_err(|_| JevError::InvalidCheckContext)? {
            context.pop();
            if !context.is_empty() {
                windows.push(window(group, &work, &context, windows.len(), scope)?);
            }
            context = vec![action];
            if !fits(&window(group, &work, &context, windows.len(), scope)?)
                .map_err(|_| JevError::InvalidCheckContext)?
            {
                group.limitation = Some("supporting_event_too_large".into());
                return Ok(Vec::new());
            }
        }
    }
    if !context.is_empty() || windows.is_empty() {
        windows.push(window(group, &work, &context, windows.len(), scope)?);
    }
    let count = windows.len();
    for item in &mut windows {
        item.window.fields["activity_window_count"] = json!(count);
        item.window.fields["recorded_facts"]["all_episode_context_in_this_window"] =
            json!(count == 1);
        item.id = digest(&(&group.id, &item.window.fields, &item.window.evidence))?;
    }
    // Final serialized metadata is also charged to the fit check.
    if windows.iter().any(|item| !matches!(fits(item), Ok(true))) {
        group.limitation = Some("activity_metadata_exceeds_limit".into());
        return Ok(Vec::new());
    }
    group.window_ids = windows.iter().map(|item| item.id.clone()).collect();
    Ok(windows)
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
    let render = |actions: &[&ContentAction],
                  key: &str,
                  role: JevEvidenceRole,
                  evidence: &mut Vec<JevEvidenceReference>|
     -> Vec<Value> {
        actions.iter().enumerate().map(|(index, action)| {
            evidence.push(JevEvidenceReference { part_id: format!("{key}[{index}]"), source_id: action.reference.id.clone(), content_kind: action.kind.clone(), role });
            // Native range bindings stay local. Question and plan metadata are
            // already present in the complete shared scope at this exact order.
            let operation = action.tool_name.as_deref().zip(action.tool_call_id.as_deref()).and_then(|key| operations.get(&key));
            json!({"kind": action.kind, "role": action.turn_role, "authority": action.authority, "turn": action.reference.turn_index, "part": action.reference.part_index, "tool": action.tool_name, "operation": operation, "text": action.text, "normalized_fields": action.normalized_fields, "operation_state": action.metadata.state})
        }).collect()
    };
    let first = work
        .iter()
        .map(|action| (action.reference.turn_index, action.reference.part_index))
        .min()
        .ok_or(JevError::InvalidCheckContext)?;
    let last = work
        .iter()
        .map(|action| (action.reference.turn_index, action.reference.part_index))
        .max()
        .ok_or(JevError::InvalidCheckContext)?;
    let fields = json!({
        "bound_work": render(work, "bound_work", JevEvidenceRole::Candidate, &mut evidence),
        "supporting_activity": render(context, "supporting_activity", JevEvidenceRole::SupportingContext, &mut evidence),
        "activity_window": index, "activity_window_count": 1,
        "recorded_facts": {
            "bound_work_complete": true, "matched_call_results_present": true,
            "selected_events_untruncated": true, "all_episode_context_in_this_window": true,
            "scope_order": scope.occurrences().iter().enumerate().map(|(index, occurrence)| {
                let position = (occurrence.reference.turn_index, occurrence.reference.part_index);
                json!({"occurrence":index,"relative_to_work":if position < first { "before_work" } else if position > last { "after_work" } else { "during_work" }})
            }).collect::<Vec<_>>(),
        },
    });
    let id = digest(&(&group.id, &fields, &evidence))?;
    Ok(JevWorkItem {
        id,
        window: JevInputWindow { fields, evidence },
        questions: questions(true),
    })
}
