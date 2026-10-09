use std::collections::{BTreeMap, BTreeSet};

use antiburn_local::analysis::jev::{
    JevAnswer, JevCheck, JevError, JevRunProgress, JevSessionContext, JevUsage, JevWorkItem,
    JevWorkItemResult, PINNED_MODEL, pack_work_items,
};
use serde::{Deserialize, Serialize};

use antiburn_local::analysis::ignored_instructions::{CandidateComparison, CounterEvidence};

#[derive(Serialize, Deserialize)]
pub(super) struct CompactCarriedComparisons {
    rule_texts: Vec<String>,
    events: Vec<CounterEvidence>,
    comparisons: Vec<CompactComparison>,
}

#[derive(Serialize, Deserialize)]
struct CompactComparison {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_binding: Option<antiburn_local::analysis::ignored_instructions::ActionSourceBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prerequisite_episode:
        Option<antiburn_local::analysis::ignored_instructions::PrerequisiteEpisode>,
    id: String,
    reference: antiburn_local::analysis::ignored_instructions::RuleActionRef,
    source_thread_digest: String,
    source_turn_index: u64,
    source_turn_scope: String,
    rule_text: usize,
    #[serde(default)]
    instruction_context:
        Vec<antiburn_local::analysis::ignored_instructions::InstructionContextRange>,
    rule_text_start: usize,
    rule_text_end: usize,
    action: usize,
    action_text_start: usize,
    action_text_end: usize,
    context: Vec<usize>,
    context_truncated: bool,
    counterevidence: Vec<usize>,
    earlier_history_truncated: bool,
    prior_history_complete: bool,
}

impl CompactCarriedComparisons {
    pub(super) fn from_comparisons(comparisons: &[CandidateComparison]) -> Self {
        let mut rule_texts = Vec::new();
        let mut rule_indices = BTreeMap::new();
        let mut events = Vec::new();
        let mut event_indices = BTreeMap::<CounterEvidence, usize>::new();
        let mut event_index = |event: &CounterEvidence| {
            *event_indices.entry(event.clone()).or_insert_with(|| {
                let index = events.len();
                events.push(event.clone());
                index
            })
        };
        let comparisons = comparisons
            .iter()
            .map(|comparison| {
                let rule_text = *rule_indices
                    .entry(comparison.rule_text.clone())
                    .or_insert_with(|| {
                        let index = rule_texts.len();
                        rule_texts.push(comparison.rule_text.clone());
                        index
                    });
                CompactComparison {
                    source_binding: comparison.source_binding.clone(),
                    prerequisite_episode: comparison.prerequisite_episode.clone(),
                    id: comparison.id.clone(),
                    reference: comparison.reference.clone(),
                    source_thread_digest: comparison.source_thread_digest.clone(),
                    source_turn_index: comparison.source_turn_index,
                    source_turn_scope: comparison.source_turn_scope.clone(),
                    rule_text,
                    instruction_context: comparison.instruction_context.clone(),
                    rule_text_start: comparison.rule_text_start,
                    rule_text_end: comparison.rule_text_end,
                    action: event_index(&comparison.action),
                    action_text_start: comparison.action_text_start,
                    action_text_end: comparison.action_text_end,
                    context: comparison.context.iter().map(&mut event_index).collect(),
                    context_truncated: comparison.context_truncated,
                    counterevidence: comparison
                        .counterevidence
                        .iter()
                        .map(&mut event_index)
                        .collect(),
                    earlier_history_truncated: comparison.earlier_history_truncated,
                    prior_history_complete: comparison.prior_history_complete,
                }
            })
            .collect();
        Self {
            rule_texts,
            events,
            comparisons,
        }
    }

    pub(super) fn restore(self) -> Result<Vec<CandidateComparison>, JevError> {
        self.comparisons
            .into_iter()
            .map(|entry| {
                let event = |index: usize| {
                    self.events
                        .get(index)
                        .cloned()
                        .ok_or(JevError::InvalidCheckPlan)
                };
                Ok(CandidateComparison {
                    source_binding: entry.source_binding,
                    prerequisite_episode: entry.prerequisite_episode,
                    id: entry.id,
                    reference: entry.reference,
                    source_thread_digest: entry.source_thread_digest,
                    source_turn_index: entry.source_turn_index,
                    source_turn_scope: entry.source_turn_scope,
                    rule_text: self
                        .rule_texts
                        .get(entry.rule_text)
                        .cloned()
                        .ok_or(JevError::InvalidCheckPlan)?,
                    instruction_context: entry.instruction_context,
                    rule_text_start: entry.rule_text_start,
                    rule_text_end: entry.rule_text_end,
                    action: event(entry.action)?,
                    action_text_start: entry.action_text_start,
                    action_text_end: entry.action_text_end,
                    context: entry
                        .context
                        .into_iter()
                        .map(event)
                        .collect::<Result<_, _>>()?,
                    context_truncated: entry.context_truncated,
                    counterevidence: entry
                        .counterevidence
                        .into_iter()
                        .map(event)
                        .collect::<Result<_, _>>()?,
                    earlier_history_truncated: entry.earlier_history_truncated,
                    prior_history_complete: entry.prior_history_complete,
                })
            })
            .collect()
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct CompactProgress {
    version: u8,
    input_revision: String,
    completed_batch_ids: BTreeSet<String>,
    failed_item_ids: BTreeSet<String>,
    request_count: usize,
    answers: Vec<Option<Vec<JevAnswer>>>,
    #[serde(default)]
    followup_answers: Vec<Option<Vec<JevAnswer>>>,
    usage_by_batch: BTreeMap<String, JevUsage>,
    #[serde(default)]
    request_by_item: BTreeMap<String, String>,
    #[serde(default)]
    extra_results: BTreeMap<String, JevWorkItemResult>,
}

impl CompactProgress {
    pub(super) fn revision_matches(&self) -> bool {
        self.version == 3
    }

    pub(super) fn is_empty(&self) -> bool {
        self.answers.iter().all(Option::is_none)
            && self.followup_answers.iter().all(Option::is_none)
            && self.extra_results.is_empty()
            && self.completed_batch_ids.is_empty()
    }
    #[cfg(test)]
    pub(super) fn from_progress<C: JevCheck>(
        check: &C,
        context: &JevSessionContext,
        progress: &JevRunProgress,
        work_items: &[JevWorkItem],
    ) -> Result<Self, JevError> {
        Self::from_progress_cached(check, context, progress, work_items, &mut BTreeMap::new())
    }

    pub(super) fn from_progress_cached<C: JevCheck>(
        check: &C,
        context: &JevSessionContext,
        progress: &JevRunProgress,
        work_items: &[JevWorkItem],
        followups: &mut BTreeMap<String, Option<JevWorkItem>>,
    ) -> Result<Self, JevError> {
        let mut usage_by_batch = BTreeMap::new();
        let mut saved_ids = BTreeSet::new();
        let answers = work_items
            .iter()
            .map(|item| {
                let Some(result) = progress.results.get(&item.id) else {
                    return Ok(None);
                };
                if result.answers.keys().ne(item.questions.keys())
                    || result.evidence != item.window.evidence
                {
                    return Ok(None);
                }
                usage_by_batch.insert(result.request_id.clone(), result.usage);
                saved_ids.insert(result.work_item_id.clone());
                Ok(Some(result.answers.values().cloned().collect()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let followup_answers = work_items
            .iter()
            .map(|item| {
                let Some(initial) = progress.results.get(&item.id) else {
                    return Ok(None);
                };
                if !followups.contains_key(&item.id) {
                    followups.insert(item.id.clone(), check.reconcile(item, initial, context)?);
                }
                let Some(followup) = followups.get(&item.id).and_then(Option::as_ref) else {
                    return Ok(None);
                };
                let Some(result) = progress.results.get(&followup.id) else {
                    return Ok(None);
                };
                if result.answers.keys().ne(followup.questions.keys())
                    || result.evidence != followup.window.evidence
                {
                    return Ok(None);
                }
                usage_by_batch.insert(result.request_id.clone(), result.usage);
                saved_ids.insert(result.work_item_id.clone());
                Ok(Some(result.answers.values().cloned().collect()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            version: 3,
            input_revision: progress.input_revision.clone(),
            completed_batch_ids: progress.completed_batch_ids.clone(),
            failed_item_ids: progress.failed_item_ids.clone(),
            request_count: progress.request_count,
            answers,
            followup_answers,
            usage_by_batch,
            extra_results: progress
                .results
                .iter()
                .filter(|(id, _)| !saved_ids.contains(*id))
                .map(|(id, result)| (id.clone(), result.clone()))
                .collect(),
            request_by_item: progress
                .results
                .iter()
                .map(|(id, result)| (id.clone(), result.request_id.clone()))
                .collect(),
        })
    }

    pub(super) fn restore<C: JevCheck>(
        self,
        check: &C,
        context: &JevSessionContext,
        work_items: &[JevWorkItem],
    ) -> Result<JevRunProgress, JevError> {
        if !self.revision_matches()
            || self.answers.len() != work_items.len()
            || (!self.followup_answers.is_empty()
                && self.followup_answers.len() != work_items.len())
        {
            return Err(JevError::InvalidCheckPlan);
        }
        let mut results = self.extra_results;
        restore_batch_results(
            work_items,
            self.answers,
            &self.completed_batch_ids,
            &self.usage_by_batch,
            &self.request_by_item,
            &mut results,
        )?;
        if !self.followup_answers.is_empty() {
            let mut followups = Vec::new();
            let mut ordered = Vec::new();
            for (item, saved) in work_items.iter().zip(self.followup_answers) {
                let followup = results
                    .get(&item.id)
                    .map(|initial| check.reconcile(item, initial, context))
                    .transpose()?
                    .flatten();
                if let Some(followup) = followup {
                    followups.push(followup);
                    ordered.push(saved);
                } else if saved.is_some() {
                    return Err(JevError::InvalidCheckPlan);
                }
            }
            restore_batch_results(
                &followups,
                ordered,
                &self.completed_batch_ids,
                &self.usage_by_batch,
                &self.request_by_item,
                &mut results,
            )?;
        }
        Ok(JevRunProgress {
            input_revision: self.input_revision,
            results,
            completed_batch_ids: self.completed_batch_ids,
            failed_item_ids: self.failed_item_ids,
            request_count: self.request_count,
        })
    }
}

fn restore_batch_results(
    items: &[JevWorkItem],
    answers: Vec<Option<Vec<JevAnswer>>>,
    completed: &BTreeSet<String>,
    usage: &BTreeMap<String, JevUsage>,
    request_by_item: &BTreeMap<String, String>,
    results: &mut BTreeMap<String, JevWorkItemResult>,
) -> Result<(), JevError> {
    let packed = if request_by_item.is_empty() {
        pack_work_items(items)
    } else {
        antiburn_local::analysis::jev::JevPackingResult::default()
    };
    let owners = packed
        .batches
        .iter()
        .flat_map(|batch| {
            batch
                .work_item_ids
                .iter()
                .map(move |id| (id.as_str(), batch.id.as_str()))
        })
        .collect::<BTreeMap<_, _>>();
    for (item, answers) in items.iter().zip(answers) {
        let Some(answers) = answers else { continue };
        if answers.len() != item.questions.len() {
            return Err(JevError::InvalidCheckPlan);
        }
        let batch_id = request_by_item
            .get(&item.id)
            .map(String::as_str)
            .or_else(|| owners.get(item.id.as_str()).copied())
            .ok_or(JevError::InvalidCheckPlan)?;
        if !completed.contains(batch_id) {
            return Err(JevError::InvalidCheckPlan);
        }
        let usage = *usage.get(batch_id).ok_or(JevError::InvalidCheckPlan)?;
        results.insert(
            item.id.clone(),
            JevWorkItemResult {
                request_id: batch_id.to_owned(),
                work_item_id: item.id.clone(),
                answers: item.questions.keys().cloned().zip(answers).collect(),
                evidence: item.window.evidence.clone(),
                model: PINNED_MODEL.to_owned(),
                usage,
            },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{
        AssessmentCursor, parse_checkpoint, parse_cursor, serialize_cursor,
        serialize_selected_cursor_progress,
    };
    use super::*;
    use antiburn_local::analysis::jev::{
        JevCheck, JevEvidenceReference, JevEvidenceRole, JevInputWindow, JevQuestion,
        JevSessionContext,
    };
    use serde_json::json;

    #[test]
    fn source_bound_comparisons_survive_durable_reload_and_legacy_has_no_binding() {
        use antiburn_local::analysis::ignored_instructions::*;
        let actions = (1..=12)
            .map(|index| ContentAction {
                reference: ContentEventReference {
                    id: format!("action-{index}"),
                    source_key_digest: "source".into(),
                    thread_digest: "thread".into(),
                    turn_index: index,
                    native_record_id: None,
                    part_index: 0,
                    stable: true,
                },
                timestamp_ms: Some(index as i64),
                turn_role: "assistant".into(),
                turn_scope: "main".into(),
                authority: "assistant".into(),
                kind: "assistant_text".into(),
                text: "Published without validation.".into(),
                tool_name: None,
                tool_call_id: None,
                normalized_fields: None,
                metadata: Default::default(),
                truncated: false,
                context_only: false,
            })
            .collect();
        let input = AssessmentInput {
            content: SessionContentEvidence {
                session_identity_digest: "session".into(),
                source_format: antiburn_local::analysis::SourceFormat::ClaudeJsonl,
                publication_fence: 3,
                selected_input_digest: "selected".into(),
                actions,
                instructions: vec![
                    snapshot_from_text(
                        "AGENTS.md",
                        "Request validation before publishing.".into(),
                        InstructionProvenance::RecordedInjection,
                        InstructionScope::Project,
                    )
                    .unwrap(),
                ],
                complete: true,
                limitations: vec![],
                excluded_thinking_parts: 0,
                field_availability: vec![],
            },
            prior_history_complete: true,
            activity_after_ms: None,
            boundary_positions: BTreeMap::new(),
            source_generation: 2,
            source_fingerprint: Some("fingerprint".into()),
            incarnation: 1,
            comparison_after: None,
        };
        let context = build_jev_context(&input).unwrap();
        let mut plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
        let expanded = plan
            .prepared
            .comparisons
            .iter_mut()
            .find(|comparison| comparison.reference.action_id == "action-12")
            .unwrap();
        let earlier = &input.content.actions[..11];
        expanded.prerequisite_episode = Some(PrerequisiteEpisode {
            selected_actions: earlier.to_vec(),
            events: earlier
                .iter()
                .map(|action| CounterEvidence {
                    action_id: action.reference.id.clone(),
                    source_order: action.reference.turn_index,
                    role: action.turn_role.clone(),
                    kind: action.kind.clone(),
                    timestamp_ms: action.timestamp_ms,
                    tool_name: None,
                    text: action.text.clone(),
                    truncated: false,
                })
                .collect(),
            identities: earlier
                .iter()
                .map(|action| EvidenceIdentity {
                    source: action.reference.clone(),
                    content_digest: content_action_digest(action),
                    start_byte: 0,
                    end_byte: action.text.len(),
                })
                .collect(),
            complete_selected_history: true,
            revision: "episode-revision".into(),
        });
        let comparisons = &plan.prepared.comparisons;
        assert!(!comparisons.is_empty());
        assert!(
            comparisons
                .iter()
                .all(|comparison| comparison.source_binding.is_some())
        );
        let expanded = comparisons
            .iter()
            .find(|comparison| comparison.reference.action_id == "action-12")
            .unwrap();
        assert!(expanded.prerequisite_episode.as_ref().unwrap().events.len() > 3);
        let compact = CompactCarriedComparisons::from_comparisons(comparisons);
        let saved = serde_json::to_string(&compact).unwrap();
        let restored: CompactCarriedComparisons = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored.restore().unwrap(), *comparisons);
        let mut legacy: serde_json::Value = serde_json::from_str(&saved).unwrap();
        for comparison in legacy["comparisons"].as_array_mut().unwrap() {
            comparison.as_object_mut().unwrap().remove("source_binding");
            comparison
                .as_object_mut()
                .unwrap()
                .remove("prerequisite_episode");
        }
        let legacy: CompactCarriedComparisons = serde_json::from_value(legacy).unwrap();
        for comparison in legacy.restore().unwrap() {
            assert!(comparison.source_binding.is_none());
            let rebuilt = extend_comparison_with_history(&comparison, &input.content.actions, true);
            assert!(rebuilt.source_binding.is_none());
        }
    }

    #[test]
    fn page_boundary_checkpoint_restores_with_a_different_next_page_layout() {
        let context = JevSessionContext {
            input_revision: "page-two".to_owned(),
            session_identity: "session".to_owned(),
            check_context: json!(null),
            limitations: Vec::new(),
            evidence_store: Default::default(),
            reference_snapshots: Vec::new(),
        };
        let cursor = AssessmentCursor {
            input_revision: Some("revision".to_owned()),
            comparison_after: Some("next-comparison".to_owned()),
            ..Default::default()
        };
        let selected = crate::store::SelectedContentProgress {
            revision: crate::store::SELECTED_CONTENT_PROGRESS_REVISION,
            cursor: None,
        };
        let saved = serialize_selected_cursor_progress(
            &cursor,
            &cursor.progress,
            &[],
            &context,
            &mut BTreeMap::new(),
            Some(&selected),
        )
        .unwrap();
        let (restored, compact, _) = parse_checkpoint(&saved).unwrap();
        assert_eq!(
            restored.comparison_after.as_deref(),
            Some("next-comparison")
        );
        let next_items = vec![JevWorkItem {
            id: "new-item".to_owned(),
            window: JevInputWindow {
                fields: json!({"new": true}),
                evidence: Vec::new(),
            },
            questions: BTreeMap::from([(
                "new-question".to_owned(),
                JevQuestion::Noul {
                    instructions: json!("Assess the new item"),
                    criteria: None,
                },
            )]),
        }];
        assert!(compact.unwrap().is_empty());
        let reset = CompactProgress::from_progress(
            &antiburn_local::analysis::ignored_instructions::IgnoredInstructionsCheck,
            &context,
            &JevRunProgress::default(),
            &next_items,
        )
        .unwrap();
        assert!(
            reset
                .restore(
                    &antiburn_local::analysis::ignored_instructions::IgnoredInstructionsCheck,
                    &context,
                    &next_items,
                )
                .is_ok()
        );
    }

    #[test]
    fn complete_page_checkpoint_is_small_and_restores_exact_answers_and_bindings() {
        let work_items = (0..256)
            .map(|index| {
                let id = format!("{index:064x}");
                JevWorkItem {
                    id: id.clone(),
                    window: JevInputWindow {
                        fields: json!({
                            "text": format!("action {index}"),
                            "instruction_targets": (0..8)
                                .map(|question| json!({"comparison_id": format!("{id}{question:02}")}))
                                .collect::<Vec<_>>(),
                        }),
                        evidence: (0..4)
                            .map(|part| JevEvidenceReference {
                                part_id: format!("instruction_targets[{part}].instruction.text"),
                                source_id: format!("{index:064x}:{part:064x}"),
                                content_kind: "instruction_rule".to_owned(),
                                role: JevEvidenceRole::Instruction,
                            })
                            .collect(),
                    },
                    questions: (0..8)
                        .map(|question| {
                            (
                                format!("target-{id}{question:02}::decision"),
                                JevQuestion::Choice {
                                    instructions: json!("Assess the rule/action pair."),
                                    criteria: BTreeMap::from([
                                        ("conflict".to_owned(), json!("Conflicts")),
                                        ("no_issue".to_owned(), json!("No issue")),
                                        ("pending_completion".to_owned(), json!("Pending completion")),
                                        ("uncertain".to_owned(), json!("Unclear")),
                                    ]),
                                },
                            )
                        })
                        .collect(),
                }
            })
            .collect::<Vec<_>>();
        let packed = pack_work_items(&work_items);
        assert!(packed.skipped_item_ids.is_empty());
        let mut progress = JevRunProgress {
            input_revision: "test-revision".to_owned(),
            request_count: packed.batches.len(),
            ..JevRunProgress::default()
        };
        for batch in &packed.batches {
            progress.completed_batch_ids.insert(batch.id.clone());
            for id in &batch.work_item_ids {
                let item = work_items.iter().find(|item| &item.id == id).unwrap();
                progress.results.insert(
                    id.clone(),
                    JevWorkItemResult {
                        request_id: batch.id.clone(),
                        work_item_id: id.clone(),
                        answers: item
                            .questions
                            .keys()
                            .map(|question| {
                                (
                                    question.clone(),
                                    JevAnswer::Choice {
                                        choice: "no_issue".to_owned(),
                                        probabilities: BTreeMap::from([
                                            ("conflict".to_owned(), 0.01),
                                            ("no_issue".to_owned(), 0.97),
                                            ("pending_completion".to_owned(), 0.01),
                                            ("uncertain".to_owned(), 0.01),
                                        ]),
                                        confidence: 0.97,
                                    },
                                )
                            })
                            .collect(),
                        evidence: item.window.evidence.clone(),
                        model: PINNED_MODEL.to_owned(),
                        usage: JevUsage {
                            input_tokens: 16000,
                            output_tokens: 4400,
                        },
                    },
                );
            }
        }
        assert!(serde_json::to_string(&progress).unwrap().len() > 512 * 1024);
        let context = JevSessionContext {
            input_revision: "test-revision".to_owned(),
            session_identity: "test-session".to_owned(),
            check_context: json!(null),
            limitations: Vec::new(),
            evidence_store: antiburn_local::analysis::jev::JevEvidenceStore::default(),
            reference_snapshots: Vec::new(),
        };
        let check = antiburn_local::analysis::ignored_instructions::IgnoredInstructionsCheck;
        let first = &work_items[0];
        for answer in progress
            .results
            .get_mut(&first.id)
            .unwrap()
            .answers
            .values_mut()
        {
            let JevAnswer::Choice {
                choice,
                probabilities,
                ..
            } = answer
            else {
                unreachable!()
            };
            *choice = "pending_completion".to_owned();
            probabilities.insert("pending_completion".to_owned(), 0.97);
            probabilities.insert("no_issue".to_owned(), 0.01);
        }
        let followup = check
            .reconcile(first, &progress.results[&first.id], &context)
            .unwrap();
        assert!(followup.is_none());
        let last = work_items.last().unwrap();
        let repacked = pack_work_items(std::slice::from_ref(last))
            .batches
            .remove(0);
        assert_ne!(progress.results[&last.id].request_id, repacked.id);
        progress.completed_batch_ids.insert(repacked.id.clone());
        progress.results.get_mut(&last.id).unwrap().request_id = repacked.id;
        progress.request_count += 1;
        let compact =
            CompactProgress::from_progress(&check, &context, &progress, &work_items).unwrap();
        let stored = serde_json::to_string(&compact).unwrap();
        assert!(
            stored.len() < 512 * 1024,
            "checkpoint is {} bytes",
            stored.len()
        );
        let restored: CompactProgress = serde_json::from_str(&stored).unwrap();
        assert_eq!(
            restored.restore(&check, &context, &work_items).unwrap(),
            progress
        );
        for old_version in [1, 2] {
            let mut old: CompactProgress = serde_json::from_str(&stored).unwrap();
            old.version = old_version;
            assert!(matches!(
                old.restore(&check, &context, &work_items),
                Err(JevError::InvalidCheckPlan)
            ));
        }
        let cursor = AssessmentCursor {
            input_revision: Some("revision".to_owned()),
            progress: progress.clone(),
            ..AssessmentCursor::default()
        };
        let saved = serialize_cursor(&cursor, &work_items, &context).unwrap();
        assert!(saved.len() < 512 * 1024, "cursor is {} bytes", saved.len());
        let (header, checkpoint) = parse_cursor(&saved).unwrap();
        assert_eq!(header.input_revision, cursor.input_revision);
        assert_eq!(
            checkpoint
                .unwrap()
                .restore(&check, &context, &work_items)
                .unwrap(),
            progress
        );
    }
}
