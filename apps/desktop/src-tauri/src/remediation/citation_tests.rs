use super::*;
use antiburn_local::analysis::ignored_instructions::*;
use antiburn_local::insights::ReportWindow;

#[test]
fn engine_skill_publication_delivers_comparison_task_to_remediation() {
    use crate::jev::worker::JevCheckDescriptor;
    use antiburn_local::analysis::jev::{
        JevAnswer, JevCheck, JevQuestion, JevUsage, JevWorkItemResult,
    };
    use antiburn_local::checks::skill_opportunities::SkillOpportunitiesResult;

    let fixture = crate::scope_creep_worker::tests::NativeFixture::new(0);
    fixture.append(
        2,
        "assistant",
        serde_json::json!({"type":"tool","tool":"bash","callID":"parser-test","state":{"status":"completed","input":{"command":"pnpm test parser-boundaries"},"output":"Parser boundary tests passed."}}),
    );
    fixture
        .store
        .set_check_enabled(DetectorId::SkillOpportunities, true)
        .unwrap();
    fixture
        .store
        .capture_burn_check_boundaries(&["skill_opportunities"], 0)
        .unwrap();
    let home = fixture.directory.path().join("home");
    let workspace = fixture.directory.path().join("workspace");
    let skill_directory = home.join(".opencode/skills/parser-review");
    std::fs::create_dir_all(&skill_directory).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(skill_directory.join("SKILL.md"), "---\nname: parser-review\ndescription: Review parser boundaries and test malformed records.\n---\n").unwrap();
    let mut candidate = fixture.publish();
    candidate.session.cwd = Some(workspace.to_str().unwrap().to_owned());
    fixture
        .store
        .lock()
        .execute(
            "UPDATE session SET cwd = ?1 WHERE session_id = ?2",
            rusqlite::params![candidate.session.cwd, candidate.session.key.session_id],
        )
        .unwrap();
    let config =
        crate::agent_config::ConfigContext::native(AgentKind::OpenCode, &home, Some(workspace));
    let snapshot = fixture
        .store
        .load_smart_check_inputs(
            &candidate.session.key,
            candidate.published_fence,
            candidate.source_generation,
            crate::smart_check_inputs::DetectorInput::SkillOpportunities,
        )
        .unwrap();
    let inputs = fixture
        .store
        .load_smart_check_skill_inputs(snapshot.clone(), &config)
        .unwrap();
    let prepared = crate::skill_opportunities_worker::prepare(
        &candidate,
        inputs,
        crate::skill_opportunities_worker::CHECK.evaluator_revision(),
    )
    .unwrap();
    let plan = prepared
        .check
        .prepare(&prepared.check.session_context())
        .unwrap();
    let answers = plan
        .work_items
        .iter()
        .map(|item| JevWorkItemResult {
            request_id: item.id.clone(),
            work_item_id: item.id.clone(),
            model: plan.capabilities.model.clone(),
            answers: item
                .questions
                .iter()
                .map(|(id, question)| {
                    let JevQuestion::Choice { criteria, .. } = question else {
                        panic!("expected skill choice")
                    };
                    assert!(criteria.contains_key("useful_opportunity"));
                    (
                        id.clone(),
                        JevAnswer::Choice {
                            choice: "useful_opportunity".into(),
                            confidence: 1.0,
                            probabilities: criteria
                                .keys()
                                .map(|choice| {
                                    (
                                        choice.clone(),
                                        if choice == "useful_opportunity" {
                                            1.0
                                        } else {
                                            0.0
                                        },
                                    )
                                })
                                .collect(),
                        },
                    )
                })
                .collect(),
            evidence: plan
                .shared_context
                .iter()
                .flat_map(|context| &context.evidence)
                .chain(&item.window.evidence)
                .cloned()
                .collect(),
            usage: JevUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
        })
        .collect::<Vec<_>>();
    let result = prepared.check.reduce(&plan, &answers, false).unwrap();
    assert!(
        crate::skill_opportunities_worker::publication_has_assessed_coverage(&result),
        "{result:?}"
    );
    assert!(!result.findings.is_empty());
    assert!(
        result
            .findings
            .iter()
            .all(|finding| !finding.comparison.task.is_empty())
    );
    let mut publication = serde_json::to_value(&result).unwrap();
    publication["input_revision"] = serde_json::json!(prepared.durable.input_revision);
    publication["inventory_revision"] = serde_json::json!(prepared.inventory_revision);
    publication["use_revision"] = serde_json::json!(prepared.use_revision);
    let now = now_epoch();
    assert!(
        fixture
            .store
            .queue_burn_check_assessment(&prepared.durable, now, 180)
            .unwrap()
    );
    assert!(
        fixture
            .store
            .claim_burn_check_assessment(&prepared.durable, now, 300, 180)
            .unwrap()
    );
    assert!(
        fixture
            .store
            .complete_burn_check_assessment(&prepared.durable, &publication.to_string(), now, 180)
            .unwrap()
    );
    let saved = fixture
        .store
        .burn_check_assessment(&candidate.session.key, "skill_opportunities")
        .unwrap()
        .unwrap();
    let published: SkillOpportunitiesResult =
        serde_json::from_str(saved.result_json.as_deref().unwrap()).unwrap();
    let source: antiburn_local::analysis::SessionEvidence = serde_json::from_str(
        &fixture
            .store
            .evidence(&candidate.session.key)
            .unwrap()
            .unwrap()
            .evidence_json
            .unwrap(),
    )
    .unwrap();
    let finding =
        antiburn_local::remediation::Finding::skill_opportunity(&source, &published.findings[0])
            .unwrap();
    let stored = stored_skill_opportunity_evidence(finding.cause()).unwrap();
    let delivered = complete_skill_opportunity_evidence(
        stored,
        &published.findings[0],
        &snapshot.content().actions,
    );
    assert_eq!(delivered.status, BurnCheckEvidenceStatus::Available);
    for task in &published.findings[0].comparison.task {
        assert!(
            delivered
                .items
                .iter()
                .any(|item| item.reference == task.reference.id
                    && item.label == BurnCheckEvidenceLabel::Context
                    && item.source_label == "Requested task"
                    && item.excerpt == task.text)
        );
    }
    let explanation = delivered
        .comparison
        .as_ref()
        .unwrap()
        .explanation
        .as_ref()
        .unwrap();
    assert_eq!(explanation.relationship, "usefulProcedure");
    assert_eq!(
        explanation.text,
        "parser-review provides a useful procedure for the recorded work “pnpm test parser-boundaries”: “Review parser boundaries and test malformed records.”."
    );
    assert_eq!(delivered.items.len(), 4);
    assert!(
        delivered
            .items
            .iter()
            .any(|item| item.excerpt == "Parser boundary tests passed.")
    );
    assert!(
        delivered
            .items
            .iter()
            .any(|item| item.source_label == "Requested task"
                && item.excerpt == "Explain the parser fix.")
    );
    assert!(explanation.text.contains("pnpm test parser-boundaries"));
    assert!(
        explanation
            .text
            .contains("Review parser boundaries and test malformed records.")
    );
    let json = serde_json::to_value(&delivered).unwrap();
    assert_eq!(json["comparison"]["explanation"]["version"], 1);
    assert!(
        json["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["sourceLabel"] == "Requested task")
    );
}

#[test]
fn instruction_explanations_use_saved_compared_text_and_reject_other_passages() {
    let (mut cause, _) = typed_cause(PrerequisiteOutcome::NotRequired);
    let FindingCause::IgnoredInstructionConflict(finding) = &mut cause else {
        unreachable!()
    };
    finding.decision.as_mut().unwrap().explanation_basis = Some(InstructionExplanationBasis {
        schema_revision: 1,
        relationship: InstructionMismatch::RequirementConflict,
        instruction: finding.instruction_excerpt.clone(),
        action: finding.action_excerpt.clone(),
        instruction_context: vec![],
        earlier_events: vec![],
    });
    let evidence = stored_instruction_evidence(&cause).unwrap();
    let explanation = instruction_comparison(&cause, &evidence)
        .unwrap()
        .explanation
        .unwrap();
    assert!(
        explanation
            .text
            .contains(&explanation_quote(&evidence.items[0].excerpt))
    );
    assert!(
        explanation
            .text
            .contains(&explanation_quote(&evidence.items[1].excerpt))
    );
    assert_eq!(
        explanation.references,
        vec![
            evidence.items[0].reference.clone(),
            evidence.items[1].reference.clone()
        ]
    );
    let mut replaced = evidence.clone();
    replaced.items[1].excerpt = "Different operation".into();
    assert!(instruction_comparison(&cause, &replaced).is_none());
    let FindingCause::IgnoredInstructionConflict(finding) = &mut cause else {
        unreachable!()
    };
    finding.decision.as_mut().unwrap().explanation_basis = None;
    assert!(instruction_comparison(&cause, &evidence).is_none());
}

#[test]
fn compact_read_task_citations_reach_native_endpoint_at_small_context_limits() {
    use crate::over_exploring_worker::{prepare, publication, publishable_finding, tests::fixture};
    use antiburn_local::analysis::jev::{JevAnswer, JevCheck, JevUsage, JevWorkItemResult};
    use antiburn_local::checks::over_exploring::Reason;
    use antiburn_local::checks::over_exploring::{
        OverExploringCheck, PreparedAssessment, synchronize_sampling,
    };
    use antiburn_local::checks::sampling::{SamplingLimits, SamplingProgress};
    for tokens in [2048, 2050] {
        let directory = tempfile::tempdir().unwrap();
        let (store, candidate) = fixture(Some(directory.path()), "user");
        store
            .lock()
            .execute(
                "INSERT OR REPLACE INTO setting (key, value) VALUES ('internal:burnChecksEnabledAtEpochV1', '1')",
                [],
            )
            .unwrap();
        let snapshot = store
            .load_smart_check_inputs(
                &candidate.session.key,
                candidate.published_fence,
                candidate.source_generation,
                crate::smart_check_inputs::DetectorInput::OverExploring,
            )
            .unwrap();
        let mut capabilities =
            antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default();
        capabilities.runtime_context_tokens.value = Some(tokens);
        capabilities.rendering_reserve_tokens = 1024;
        let input = prepare(&candidate, snapshot.clone(), &capabilities).unwrap();
        let mut sampling = SamplingProgress::new(SamplingLimits {
            checks: 1,
            candidates_per_check: 4096,
            answers_per_candidate: 1,
            judgments_per_run: 8,
        })
        .unwrap();
        synchronize_sampling(&input.plan, &mut sampling).unwrap();
        sampling.begin_run();
        let job = std::iter::from_fn(|| sampling.choose_job())
            .find(|job| {
                input.plan.prepared.candidates.iter().any(|candidate| {
                    candidate.candidate_id == job.candidate
                        && candidate.work_item_ids.iter().any(|id| {
                            input.plan.prepared.targets[id].reason == Reason::UnrelatedFiles
                        })
                })
            })
            .unwrap();
        let mut plan = input.plan.clone();
        PreparedAssessment::select_jobs(&mut plan, &[job]).unwrap();
        let answers = plan
            .work_items
            .iter()
            .map(|item| JevWorkItemResult {
                request_id: item.id.clone(),
                work_item_id: item.id.clone(),
                model: plan.capabilities.model.clone(),
                answers: item
                    .questions
                    .keys()
                    .map(|id| {
                        (
                            id.clone(),
                            JevAnswer::Choice {
                                choice: "likely_excess".into(),
                                confidence: 1.0,
                                probabilities: BTreeMap::from([
                                    ("likely_excess".into(), 1.0),
                                    ("justified_or_minor".into(), 0.0),
                                    ("uncertain".into(), 0.0),
                                ]),
                            },
                        )
                    })
                    .collect(),
                evidence: plan
                    .shared_context
                    .iter()
                    .flat_map(|context| &context.evidence)
                    .chain(&item.window.evidence)
                    .cloned()
                    .collect(),
                usage: JevUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            })
            .collect::<Vec<_>>();
        let result = OverExploringCheck.reduce(&plan, &answers, true).unwrap();
        assert!(!result.findings.is_empty());
        assert!(result.findings.iter().all(|finding| {
            finding
                .task_evidence
                .iter()
                .any(|reference| reference.part_id.starts_with("task["))
        }));
        let saved = publication(&input, result);
        assert!(
            saved
                .assessment
                .findings
                .iter()
                .all(|finding| publishable_finding(finding, &saved)),
            "saved publication failed validation: {:?}",
            saved.assessment.findings.first()
        );
        let now = now_epoch();
        assert!(
            store
                .queue_burn_check_assessment(&input.durable, now, 180)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(&input.durable, now, 300, 180)
                .unwrap()
        );
        assert!(
            store
                .complete_burn_check_assessment(
                    &input.durable,
                    &serde_json::to_string(&saved).unwrap(),
                    now,
                    180
                )
                .unwrap()
        );
        let controller = RemediationController::new(directory.path().to_owned());
        let current = crate::insights_report::list_current_findings(
            directory.path(),
            crate::insights_report::CurrentFindingsRequest {
                environment_key: "native".into(),
                window: ReportWindow {
                    start_epoch: 0,
                    end_epoch: 2000,
                },
                detector: DetectorId::OverExploring,
            },
        )
        .unwrap();
        assert!(!current.findings.is_empty(), "no current findings");
        let targets = controller
            .list_burn_check_targets_at(
                &store,
                DetectorId::OverExploring,
                BurnCheckTargetContext {
                    environment_key: "native".into(),
                    window: ReportWindow {
                        start_epoch: 0,
                        end_epoch: 2000,
                    },
                },
                TargetListOptions {
                    now,
                    home: None,
                    cache_actions: true,
                },
            )
            .unwrap();
        let target = targets
            .targets
            .first()
            .expect("compact finding is reachable");
        let evidence = controller
            .burn_check_target_evidence(&store, &target.action_id)
            .unwrap();
        assert_eq!(
            evidence.status,
            BurnCheckEvidenceStatus::Available,
            "{tokens}"
        );
        assert!(
            evidence
                .items
                .iter()
                .any(|item| item.source_label == "Requested task"
                    && item.excerpt == "Fix the parser. Keep billing unchanged.")
        );
        assert!(evidence.comparison.as_ref().unwrap().explanation.is_some());
        let decision = &saved.assessment.findings[0];
        for forged in 0..5 {
            let mut changed = decision.clone();
            let compact = changed
                .task_evidence
                .iter_mut()
                .find(|binding| binding.part_id.starts_with("task["))
                .unwrap();
            match forged {
                0 => compact.source_id = "forged-source".into(),
                1 => compact.content_kind = "tool_result".into(),
                2 => {
                    compact.role = antiburn_local::analysis::jev::JevEvidenceRole::SupportingContext
                }
                3 => {
                    changed
                        .explanation
                        .as_mut()
                        .unwrap()
                        .snippets
                        .iter_mut()
                        .find(|snippet| snippet.reference.id == compact.source_id)
                        .unwrap()
                        .reference
                        .thread_digest = "other-thread".into()
                }
                _ => {
                    changed
                        .explanation
                        .as_mut()
                        .unwrap()
                        .snippets
                        .iter_mut()
                        .find(|snippet| snippet.reference.id == compact.source_id)
                        .unwrap()
                        .source_digest = "other-content".into()
                }
            }
            assert!(
                validated_read_task_items(&changed, snapshot.scope(), &snapshot.content().actions)
                    .is_none(),
                "{tokens}: {forged}"
            );
        }
    }
}

#[test]
fn request_only_read_citations_survive_provider_failure_without_claiming_output() {
    use crate::over_exploring_worker::{
        prepare, publication,
        tests::{fixture, reduced},
    };
    use antiburn_local::checks::over_exploring::Reason;
    let directory = tempfile::tempdir().unwrap();
    let (store, candidate) = fixture(Some(directory.path()), "user");
    store
        .lock()
        .execute("DELETE FROM turn_content WHERE kind = 'tool_result'", [])
        .unwrap();
    let snapshot = store
        .load_smart_check_inputs(
            &candidate.session.key,
            candidate.published_fence,
            candidate.source_generation,
            crate::smart_check_inputs::DetectorInput::OverExploring,
        )
        .unwrap();
    let input = prepare(
        &candidate,
        snapshot,
        &antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default(),
    )
    .unwrap();
    let result = reduced(&input, Reason::UnrelatedFiles);
    assert!(!result.findings.is_empty());
    assert!(!crate::over_exploring_worker::publication_has_clean_coverage(&result));
    assert!(
        result
            .findings
            .iter()
            .flat_map(|finding| &finding.reads)
            .all(|read| read.result_id.is_none() && read.output_digest.is_none())
    );
    let published = publication(&input, result);
    let now = now_epoch();
    assert!(
        store
            .queue_burn_check_assessment(&input.durable, now, 180)
            .unwrap()
    );
    assert!(
        store
            .claim_burn_check_assessment(&input.durable, now, 300, 180)
            .unwrap()
    );
    assert!(
        store
            .fail_burn_check_assessment_with_result(
                &input.durable,
                &crate::store::BurnCheckFailure {
                    error_category: "provider_unavailable",
                    result_json: &serde_json::to_string(&published).unwrap(),
                    progress_json: "{}",
                    retry_at_epoch: None,
                },
                now,
                180,
            )
            .unwrap()
    );
    let controller = RemediationController::new(directory.path().to_owned());
    let targets = controller
        .list_burn_check_targets_at(
            &store,
            DetectorId::OverExploring,
            BurnCheckTargetContext {
                environment_key: "native".into(),
                window: ReportWindow {
                    start_epoch: 0,
                    end_epoch: 2000,
                },
            },
            TargetListOptions {
                now,
                home: None,
                cache_actions: true,
            },
        )
        .unwrap();
    assert!(!targets.targets.is_empty());
    let target = &targets.targets[0];
    let evidence = controller
        .burn_check_target_evidence(&store, &target.action_id)
        .unwrap();
    assert_eq!(evidence.status, BurnCheckEvidenceStatus::Available);
    let finding = published
        .assessment
        .findings
        .iter()
        .find(|finding| finding.work_item_id == evidence.occurrences[0].finding_id)
        .unwrap();
    let expected: BTreeSet<_> = finding
        .task_evidence
        .iter()
        .map(|item| item.source_id.as_str())
        .chain(finding.reads.iter().map(|read| read.request_id.as_str()))
        .chain(finding.explanation.iter().flat_map(|basis| {
            basis
                .compared
                .evidence
                .iter()
                .map(|reference| reference.source_id.as_str())
        }))
        .collect();
    let actual: BTreeSet<_> = evidence
        .items
        .iter()
        .map(|item| item.reference.as_str())
        .collect();
    assert_eq!(actual, expected);
    let comparison = evidence.comparison.as_ref().unwrap();
    assert!(
        comparison
            .reads
            .iter()
            .all(|read| read.result_reference.is_none()
                && read.result_status.is_none()
                && read.returned_extent.is_none())
    );
    assert_eq!(comparison.reads.len(), finding.reads.len());
    assert!(
        evidence
            .items
            .iter()
            .filter(|item| item.label == BurnCheckEvidenceLabel::ObservedAction)
            .all(|item| item
                .limitation
                .as_deref()
                .unwrap()
                .contains("request alone does not prove returned content"))
    );
    let prompt = controller
        .copy_prompt_fix_burn_check_targets(&store, std::slice::from_ref(&target.action_id))
        .unwrap();
    assert!(prompt.prompt.contains("Requests reads"));
    assert!(
        prompt
            .prompt
            .contains("A request alone does not prove returned content")
    );
}

fn typed_cause(
    prerequisite: PrerequisiteOutcome,
) -> (
    FindingCause,
    BTreeMap<String, antiburn_local::analysis::jev_evidence::ContentAction>,
) {
    let mut cause = super::tests::saved_instruction_cause();
    let FindingCause::IgnoredInstructionConflict(evidence) = &mut cause else {
        unreachable!()
    };
    let anchor = super::tests::context_action("action", 12, "Publish the change.");
    let context = super::tests::context_action("earlier", 1, "Run the validation.");
    evidence.action_digest = content_action_digest(&anchor);
    evidence.decision = Some(DecisionRecord {
        explanation_basis: None,
        schema_revision: 1,
        source_generation: 2,
        source_fingerprint: Some("fingerprint".into()),
        publication_fence: 3,
        rule_action: RuleActionRef {
            instruction_id: evidence.instruction_id.clone(),
            instruction_digest: evidence.instruction_digest.clone(),
            rule_id: evidence.rule_id.clone(),
            rule_heading: evidence.rule_heading.clone(),
            start_line: evidence.start_line,
            end_line: evidence.end_line,
            source: evidence.source.clone(),
            provenance: evidence.provenance,
            scope: evidence.instruction_scope,
            action_id: evidence.action_id.clone(),
            action_digest: evidence.action_digest.clone(),
            action_timestamp_ms: evidence.action_timestamp_ms,
            action_stable: true,
        },
        rule_start_byte: 0,
        rule_end_byte: evidence.instruction_excerpt.len(),
        action_anchor: EvidenceIdentity {
            source: anchor.reference.clone(),
            content_digest: content_action_digest(&anchor),
            start_byte: 0,
            end_byte: anchor.text.len(),
        },
        action_authority: "assistant".into(),
        action_is_request: true,
        prerequisite,
        selected_evidence: vec![EvidenceIdentity {
            source: context.reference.clone(),
            content_digest: content_action_digest(&context),
            start_byte: 0,
            end_byte: context.text.len(),
        }],
        coverage: DecisionCoverage {
            source_complete: true,
            selected_history_complete: true,
            read_request_inventory_complete: true,
            results_excluded: true,
            user_authority_excluded: true,
            limitations: vec![],
        },
        citations: vec![
            CitationProof {
                claim: CitationClaim::RuleRequirement,
                source_ids: vec!["instruction:rule".into()],
            },
            CitationProof {
                claim: CitationClaim::AnchoredAction,
                source_ids: vec!["action".into()],
            },
            CitationProof {
                claim: if prerequisite == PrerequisiteOutcome::NotRequired {
                    CitationClaim::ObservedContext
                } else {
                    CitationClaim::PrerequisiteContrast
                },
                source_ids: if prerequisite == PrerequisiteOutcome::NotRequired {
                    vec!["earlier".into()]
                } else {
                    vec!["action".into(), "earlier".into()]
                },
            },
        ],
        context_revision: "context".into(),
        evaluator_revision: evaluator_revision(),
        model: ASSESSMENT_MODEL.into(),
    });
    (
        cause,
        BTreeMap::from([("action".into(), anchor), ("earlier".into(), context)]),
    )
}

#[test]
fn citation_proof_requires_every_selected_event_for_each_template() {
    for prerequisite in [
        PrerequisiteOutcome::NotRequired,
        PrerequisiteOutcome::EarlierRequestAbsent,
        PrerequisiteOutcome::SelectedHistoryConflict,
    ] {
        let (cause, actions) = typed_cause(prerequisite);
        let FindingCause::IgnoredInstructionConflict(evidence) = &cause else {
            unreachable!()
        };
        assert!(IgnoredInstructionDecisionProof::from_cause(&cause).is_some());
        assert_eq!(
            validated_evidence_references(evidence, &actions),
            Some((vec!["earlier".into(), "action".into()], false))
        );
        let mut missing = actions.clone();
        missing.remove("earlier");
        assert!(validated_evidence_references(evidence, &missing).is_none());
        let mut changed = actions;
        changed.get_mut("earlier").unwrap().text = "Changed context".into();
        assert!(validated_evidence_references(evidence, &changed).is_none());
        let merged = merge_instruction_evidence(
            stored_instruction_evidence(&cause),
            unavailable_instruction_evidence(),
        );
        assert_eq!(merged.status, BurnCheckEvidenceStatus::Unavailable);
        assert!(merged.decision_proof.is_none());
        assert!(merged.items.is_empty());
    }
}

#[test]
fn instruction_preview_retrieves_exact_citations_beyond_the_recent_page_limit() {
    use antiburn_local::analysis::jev::JevCheck;
    use antiburn_local::analysis::{
        ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION, SelectedContentRequest,
    };
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory(directory.path()).unwrap();
    let key = SessionKey::new("native", "claude-code", "long-history");
    {
        let connection = store.lock();
        connection.execute_batch(
            "INSERT INTO session (environment_key, agent, session_id, source_kind, source_label,
                first_seen_at, last_seen_at, source_generation, source_fingerprint)
             VALUES ('native', 'claude-code', 'long-history', 'file', 'synthetic', 'now', 'now', 2, 'fingerprint');",
        ).unwrap();
        connection.execute(
            "INSERT INTO session_evidence (environment_key, agent, session_id, status,
                analyzed_generation, processed_fingerprint, parser_revision, analyzer_revision,
                evidence_schema_revision, published_fence, claim_fence)
             VALUES ('native', 'claude-code', 'long-history', 'ready', 2, 'fingerprint', ?1, ?2, ?3, 3, 3)",
            rusqlite::params![PARSER_REVISION, ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION],
        ).unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        for index in 0..5000 {
            transaction
                .execute(
                    "INSERT INTO turn (environment_key, agent, session_id, claim_fence,
                    source_key, thread_id, turn_index, scope, role, ts_ms, uuid,
                    input_tokens, cache_read_tokens, cache_write_tokens, output_tokens,
                    is_compaction_boundary)
                 VALUES ('native', 'claude-code', 'long-history', 3, 'source', 'thread', ?1,
                    'main', 'assistant', 1000, ?2, 0, 0, 0, 0, 0)",
                    rusqlite::params![index, format!("record-{index}")],
                )
                .unwrap();
            transaction.execute(
                "INSERT INTO turn_content (turn_rowid, part_index, kind, authority, content, truncated)
                 VALUES (?1, 0, 'assistant', 'assistant', ?2, 0)",
                rusqlite::params![transaction.last_insert_rowid(), format!("Action {index}").as_bytes()],
            ).unwrap();
        }
        transaction.commit().unwrap();
    }
    let session_identity = "native\0claude-code\0long-history";
    let selection = IgnoredInstructionsCheck.input_selection();
    let positions = BTreeMap::from([("*".into(), 0)]);
    let published = store
        .published_turn_content_keyset_selected(
            &key,
            SelectedContentRequest {
                source_generation: 2,
                after_ms: None,
                source_positions: &positions,
                selection,
                cursor: None,
            },
        )
        .unwrap()
        .unwrap()
        .content;
    let initial =
        validated_instruction_content(session_identity, SourceFormat::ClaudeJsonl, published, 3, 2)
            .unwrap();
    let context = initial
        .actions
        .iter()
        .find(|action| action.reference.turn_index == 0)
        .unwrap();
    let anchor = initial
        .actions
        .iter()
        .find(|action| action.reference.turn_index == 1)
        .unwrap();
    let (mut cause, _) = typed_cause(PrerequisiteOutcome::NotRequired);
    let FindingCause::IgnoredInstructionConflict(evidence) = &mut cause else {
        unreachable!()
    };
    evidence.action_id = anchor.reference.id.clone();
    evidence.action_digest = content_action_digest(anchor);
    evidence.action_excerpt = anchor.text.clone();
    let decision = evidence.decision.as_mut().unwrap();
    decision.rule_action.action_id = evidence.action_id.clone();
    decision.rule_action.action_digest = evidence.action_digest.clone();
    decision.action_anchor = EvidenceIdentity {
        source: anchor.reference.clone(),
        content_digest: evidence.action_digest.clone(),
        start_byte: 0,
        end_byte: anchor.text.len(),
    };
    decision.selected_evidence = vec![EvidenceIdentity {
        source: context.reference.clone(),
        content_digest: content_action_digest(context),
        start_byte: 0,
        end_byte: context.text.len(),
    }];
    decision.citations[1].source_ids = vec![anchor.reference.id.clone()];
    decision.citations[2].source_ids = vec![context.reference.id.clone()];
    assert!(evidence.decision_record().is_some());

    let mut cursor = None;
    for _ in 0..16 {
        let page = store
            .published_turn_content_keyset_selected(
                &key,
                SelectedContentRequest {
                    source_generation: 2,
                    after_ms: Some(i64::MIN),
                    source_positions: &positions,
                    selection,
                    cursor: cursor.as_ref(),
                },
            )
            .unwrap()
            .unwrap();
        assert!(page.content.parts.iter().all(|part| part.turn_index > 1));
        cursor = page.next_cursor;
    }
    let references = [&anchor.reference, &context.reference];
    let context_ids = BTreeSet::from([context.reference.id.as_str()]);
    let published = store
        .published_turn_content_identities_selected(&key, 2, &references, &context_ids, selection)
        .unwrap()
        .unwrap();
    assert_eq!(published.parts.len(), 2);
    let nearby = store
        .published_turn_content_identities_selected(
            &key,
            2,
            &[&anchor.reference],
            &context_ids,
            selection,
        )
        .unwrap()
        .unwrap();
    assert_eq!(nearby.parts, published.parts);
    let following = initial
        .actions
        .iter()
        .find(|action| action.reference.turn_index == 2)
        .unwrap();
    let nearby = store
        .published_turn_content_identities_selected(
            &key,
            2,
            &[&anchor.reference],
            &BTreeSet::from([
                context.reference.id.as_str(),
                following.reference.id.as_str(),
            ]),
            selection,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        nearby
            .parts
            .iter()
            .map(|part| part.turn_index)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([0, 1, 2])
    );
    let selected =
        validated_instruction_content(session_identity, SourceFormat::ClaudeJsonl, published, 3, 2)
            .unwrap();
    let actions = selected
        .actions
        .into_iter()
        .map(|action| (action.reference.id.clone(), action))
        .collect();
    assert_eq!(
        validated_evidence_references(evidence, &actions),
        Some((
            vec![context.reference.id.clone(), anchor.reference.id.clone()],
            false
        ))
    );
    let mut changed = actions;
    changed.get_mut(&context.reference.id).unwrap().text = "Changed context".into();
    assert!(validated_evidence_references(evidence, &changed).is_none());

    for (kind, authority) in [("thinking", "assistant"), ("assistant", "unknown")] {
        store
            .lock()
            .execute(
                "UPDATE turn_content SET kind = ?1, authority = ?2
             WHERE turn_rowid IN (SELECT rowid FROM turn WHERE turn_index = 0)",
                rusqlite::params![kind, authority],
            )
            .unwrap();
        let published = store
            .published_turn_content_identities_selected(
                &key,
                2,
                &references,
                &context_ids,
                selection,
            )
            .unwrap()
            .unwrap();
        let selected = validated_instruction_content(
            session_identity,
            SourceFormat::ClaudeJsonl,
            published,
            3,
            2,
        )
        .unwrap();
        let actions = selected
            .actions
            .into_iter()
            .map(|action| (action.reference.id.clone(), action))
            .collect();
        assert!(validated_evidence_references(evidence, &actions).is_none());
    }

    store
        .lock()
        .execute("UPDATE session SET source_generation = 3", [])
        .unwrap();
    assert!(
        store
            .published_turn_content_identities_selected(
                &key,
                2,
                &references,
                &context_ids,
                selection
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn observed_context_serializes_and_rule_citation_binds_saved_text() {
    let (cause, _) = typed_cause(PrerequisiteOutcome::NotRequired);
    let saved = stored_instruction_evidence(&cause).unwrap();
    let proof = saved.decision_proof.as_ref().unwrap();
    assert_eq!(
        proof.citations[0].source_ids,
        [saved.items[0].reference.clone()]
    );
    assert_eq!(
        serde_json::to_value(proof).unwrap()["citations"][2]["claim"],
        "observed_context"
    );
}

#[test]
fn citation_proof_rejects_unbound_extra_sources() {
    let (mut cause, _) = typed_cause(PrerequisiteOutcome::NotRequired);
    let FindingCause::IgnoredInstructionConflict(evidence) = &mut cause else {
        unreachable!()
    };
    evidence
        .decision
        .as_mut()
        .unwrap()
        .citations
        .push(CitationProof {
            claim: CitationClaim::ObservedContext,
            source_ids: vec!["fabricated".into()],
        });
    assert!(IgnoredInstructionDecisionProof::from_cause(&cause).is_none());
}

#[test]
fn citation_proof_requires_saved_requirement_and_action_text() {
    for missing_rule in [false, true] {
        let (mut cause, _) = typed_cause(PrerequisiteOutcome::NotRequired);
        let FindingCause::IgnoredInstructionConflict(evidence) = &mut cause else {
            unreachable!()
        };
        if missing_rule {
            evidence.instruction_excerpt.clear();
        } else {
            evidence.action_excerpt.clear();
        }
        assert!(IgnoredInstructionDecisionProof::from_cause(&cause).is_none());
    }
}

#[test]
fn legacy_excerpts_remain_available_without_decisive_proof() {
    let cause = super::tests::saved_instruction_cause();
    let merged = merge_instruction_evidence(
        stored_instruction_evidence(&cause),
        unavailable_instruction_evidence(),
    );
    assert_eq!(merged.status, BurnCheckEvidenceStatus::Available);
    assert!(merged.decision_proof.is_none());
    assert_eq!(merged.items.len(), 2);
    assert_eq!(
        merged.items[1].explanation,
        "Saved action text that Antiburn compared with the instruction."
    );
}

#[test]
fn scope_citations_cover_the_complete_latest_scope_and_bound_work() {
    let fixture = crate::scope_creep_worker::tests::NativeFixture::new(1);
    let candidate = fixture.publish_finding();
    let controller = RemediationController::new(fixture.directory.path().to_owned());
    let targets = controller
        .list_burn_check_targets_at(
            &fixture.store,
            DetectorId::ScopeCreep,
            BurnCheckTargetContext {
                environment_key: "native".into(),
                window: antiburn_local::insights::ReportWindow {
                    start_epoch: 0,
                    end_epoch: 2000,
                },
            },
            TargetListOptions {
                now: now_epoch(),
                home: None,
                cache_actions: true,
            },
        )
        .unwrap();
    let target = &targets.targets[0];
    let cached = controller
        .cached_target(&fixture.store, &target.action_id, now_epoch())
        .unwrap();
    let FindingCause::ScopeCreep(scope) = cached.finding().cause() else {
        unreachable!()
    };
    let publication = crate::scope_creep_worker::current_publication(
        &fixture.store.lock(),
        &crate::scope_creep_worker::SourceFence::from(&candidate),
    )
    .unwrap()
    .unwrap();
    let group = scope_evidence::accepted_scope_group(&publication, scope)
        .expect("accepted group is materialized");
    let snapshot = fixture
        .store
        .load_smart_check_inputs(
            &candidate.session.key,
            candidate.published_fence,
            candidate.source_generation,
            crate::smart_check_inputs::DetectorInput::ScopeCreep,
        )
        .unwrap();
    assert!(
        scope_evidence::scope_group_evidence(
            scope,
            &group,
            snapshot.scope(),
            &snapshot.content().actions
        )
        .is_some(),
        "group={group:?} finding={scope:?} scope={:?} actions={:?}",
        snapshot.scope(),
        snapshot.content().actions
    );
    let evidence = controller
        .burn_check_target_evidence(&fixture.store, &target.action_id)
        .unwrap();
    let expected: BTreeSet<_> = scope
        .task_scope
        .iter()
        .map(|item| item.source_id.as_str())
        .chain(scope.work.iter().map(|item| item.reference.id.as_str()))
        .chain(
            group
                .context
                .iter()
                .map(|binding| binding.reference.id.as_str()),
        )
        .collect();
    let actual: BTreeSet<_> = evidence
        .items
        .iter()
        .map(|item| item.reference.as_str())
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(evidence.status, BurnCheckEvidenceStatus::Available);
    assert!(evidence.items.iter().any(|item| {
        item.label == BurnCheckEvidenceLabel::Instruction
            && item.source_label == "Requested task"
            && item.excerpt == "Fix the parser. Do not change billing."
    }));
    let comparison = evidence.comparison.as_ref().unwrap();
    assert!(
        comparison
            .explanation
            .as_ref()
            .unwrap()
            .text
            .contains("Fix the parser. Do not change billing.")
    );
    assert!(comparison.source_ranges.iter().all(|range| {
        group.selected_excerpts.iter().any(|excerpt| {
            excerpt.source.id == range.reference
                && excerpt.start_byte == range.start_byte
                && excerpt.end_byte == range.end_byte
        })
    }));
    let mut forged = scope.as_ref().clone();
    let basis = forged.explanation_basis.as_mut().unwrap();
    let task = basis
        .excerpts
        .iter_mut()
        .find(|excerpt| excerpt.scope_field.is_some())
        .unwrap();
    task.start_byte += 1;
    assert!(
        scope_evidence::scope_group_evidence(
            &forged,
            &group,
            snapshot.scope(),
            &snapshot.content().actions
        )
        .is_none()
    );
}

#[test]
fn skill_citations_include_work_current_descriptions_and_exact_recorded_use() {
    use antiburn_local::analysis::jev::{JevEvidenceReference, JevEvidenceRole};
    use antiburn_local::analysis::jev_evidence::content_action_digest;
    use antiburn_local::checks::skill_opportunities::*;
    let work = super::tests::context_action("work", 1, "Add parser boundary tests.");
    let usage = super::tests::context_action("use", 2, "Request the format skill.");
    let skill = CurrentSkillCitation {
        identity: "parser-skill".into(),
        definition_revision: "definition".into(),
        reference_revision: "reference".into(),
        name: "Parser review".into(),
        description: "Review parser boundaries.".into(),
        reference: SkillReferenceCoverage {
            source: SkillReferenceSource::Description,
            ranges: vec![(0, "Review parser boundaries.".len())],
            total_bytes: "Review parser boundaries.".len(),
            partial: false,
        },
        created_at_ms: None,
    };
    let used_skill = CurrentSkillCitation {
        identity: "format-skill".into(),
        name: "Format review".into(),
        description: "Review output formatting.".into(),
        reference: SkillReferenceCoverage {
            source: SkillReferenceSource::Description,
            ranges: vec![(0, "Review output formatting.".len())],
            total_bytes: "Review output formatting.".len(),
            partial: false,
        },
        ..skill.clone()
    };
    let finding = SkillOpportunityFinding {
        explanation_basis: Some(SkillExplanationBasis {
            version: 1,
            relationship: SkillRelationship::UsefulProcedure,
        }),
        comparison: SkillComparison {
            task: vec![],
            id: "comparison".into(),
            episode_id: "episode".into(),
            work: vec![SkillWorkCitation {
                reference: work.reference.clone(),
                text: work.text.clone(),
                timestamp_ms: work.timestamp_ms,
                kind: work.kind.clone(),
                ranges: vec![(0, work.text.len())],
                total_bytes: work.text.len(),
                source_digest: content_action_digest(&work),
                partial: work.truncated,
            }],
            skill,
            limitations: vec![],
            use_revision: "use-revision".into(),
            use_citations: vec![usage.reference.clone()],
            used_current_skills: vec![used_skill],
            inventory_revision: "inventory".into(),
            eligibility_revision: "eligibility".into(),
            absence_assessable: true,
            use_eligibility: SkillUseEligibility {
                absence: SkillAbsenceEvidence::SelectedWindowNoMatchingUse,
                equivalent_comparison_required: true,
            },
            work_context_assessable: true,
        },
        message: "Use the parser skill for similar future work.".into(),
        absence_limit: "The selected window does not prove absence across the session.".into(),
        model: "model".into(),
        revisions: SKILL_OPPORTUNITIES_REVISIONS,
        evidence: ["work", "parser-skill", "use", "format-skill"]
            .into_iter()
            .map(|id| JevEvidenceReference {
                part_id: id.into(),
                source_id: id.into(),
                content_kind: "test".into(),
                role: JevEvidenceRole::SupportingContext,
            })
            .collect(),
    };
    let cause = FindingCause::SkillOpportunity {
        evidence: Some(Box::new(finding.clone())),
        skill_name: "Parser review".into(),
        skill_description: "Review parser boundaries.".into(),
        cited_work_context: work.text.clone(),
        work_provenance: "recorded".into(),
        selected_window_limit: finding.absence_limit.clone(),
    };
    let saved = stored_skill_opportunity_evidence(&cause).unwrap();
    let complete = complete_skill_opportunity_evidence(
        saved.clone(),
        &finding,
        &[work.clone(), usage.clone()],
    );
    assert_eq!(complete.status, BurnCheckEvidenceStatus::Available);
    let explanation = complete
        .comparison
        .as_ref()
        .unwrap()
        .explanation
        .as_ref()
        .unwrap();
    assert!(explanation.text.contains("Add parser boundary tests."));
    assert!(explanation.text.contains("Review parser boundaries."));
    let actual: BTreeSet<_> = complete
        .items
        .iter()
        .map(|item| item.reference.as_str())
        .collect();
    assert_eq!(
        actual,
        BTreeSet::from(["work", "parser-skill", "use", "format-skill"])
    );
    let mut with_task = finding.clone();
    with_task.comparison.task = vec![with_task.comparison.work[0].clone()];
    with_task.comparison.task[0].reference = usage.reference.clone();
    with_task.comparison.task[0].text = usage.text.clone();
    with_task.comparison.task[0].timestamp_ms = usage.timestamp_ms;
    with_task.comparison.task[0].kind = usage.kind.clone();
    with_task.comparison.task[0].ranges = vec![(0, usage.text.len())];
    with_task.comparison.task[0].total_bytes = usage.text.len();
    with_task.comparison.task[0].source_digest = content_action_digest(&usage);
    with_task.comparison.use_citations.clear();
    let mut task_cause = cause.clone();
    let FindingCause::SkillOpportunity {
        evidence: Some(task_finding),
        ..
    } = &mut task_cause
    else {
        unreachable!()
    };
    **task_finding = with_task.clone();
    let task_saved = stored_skill_opportunity_evidence(&task_cause).unwrap();
    let delivered = complete_skill_opportunity_evidence(
        task_saved.clone(),
        &with_task,
        &[work.clone(), usage.clone()],
    );
    assert_eq!(delivered.status, BurnCheckEvidenceStatus::Available);
    assert!(delivered.items.iter().any(|item| item.reference == "use"
        && item.source_label == "Recorded task context"
        && item.excerpt == usage.text));
    assert_eq!(
        complete_skill_opportunity_evidence(task_saved, &with_task, std::slice::from_ref(&work))
            .status,
        BurnCheckEvidenceStatus::Unavailable
    );
    assert!(
        complete
            .items
            .iter()
            .find(|item| item.reference == "parser-skill")
            .unwrap()
            .limitation
            .as_ref()
            .unwrap()
            .contains("does not prove past availability")
    );
    assert_eq!(
        complete_skill_opportunity_evidence(saved.clone(), &finding, &[]).status,
        BurnCheckEvidenceStatus::Unavailable
    );
    let mut fallback = cause.clone();
    let FindingCause::SkillOpportunity {
        evidence: Some(evidence),
        ..
    } = &mut fallback
    else {
        unreachable!()
    };
    evidence.comparison.skill.reference.source = SkillReferenceSource::MarkdownFallback;
    evidence.comparison.skill.reference.partial = true;
    let partial = stored_skill_opportunity_evidence(&fallback).unwrap();
    let reference = partial
        .items
        .iter()
        .find(|item| item.reference == "parser-skill")
        .unwrap();
    assert!(reference.explanation.contains("Markdown file"));
    assert!(
        reference
            .limitation
            .as_deref()
            .unwrap()
            .contains("Only selected byte ranges")
    );
    let mut sampled = cause.clone();
    let FindingCause::SkillOpportunity {
        evidence: Some(sampled_finding),
        ..
    } = &mut sampled
    else {
        unreachable!()
    };
    let citation = &mut sampled_finding.comparison.work[0];
    citation.ranges = vec![(0, 3), (11, work.text.len())];
    citation.text = format!("{}{}", &work.text[..3], &work.text[11..]);
    citation.partial = true;
    assert!(citation.matches_action(&work));
    let sampled_saved = stored_skill_opportunity_evidence(&sampled).unwrap();
    let sampled_finding = match &sampled {
        FindingCause::SkillOpportunity {
            evidence: Some(finding),
            ..
        } => finding,
        _ => unreachable!(),
    };
    let sampled_complete = complete_skill_opportunity_evidence(
        sampled_saved.clone(),
        sampled_finding,
        &[work.clone(), usage.clone()],
    );
    assert_eq!(sampled_complete.status, BurnCheckEvidenceStatus::Available);
    let work_item = sampled_complete
        .items
        .iter()
        .find(|item| item.reference == "work")
        .unwrap();
    let limit = work_item.limitation.as_deref().unwrap();
    assert!(limit.contains("0..3, 11..26"));
    assert!(limit.contains(&sampled_finding.absence_limit));
    assert!(limit.contains("full work was not reviewed"));
    let mut changed_work = work.clone();
    changed_work.text.replace_range(4..10, "change");
    assert_eq!(changed_work.text.len(), work.text.len());
    assert_eq!(
        complete_skill_opportunity_evidence(
            sampled_saved.clone(),
            sampled_finding,
            &[changed_work, usage.clone()]
        )
        .status,
        BurnCheckEvidenceStatus::Unavailable
    );
    for invalid in 0..3 {
        let mut finding = sampled_finding.as_ref().clone();
        let citation = &mut finding.comparison.work[0];
        match invalid {
            0 => citation.partial = false,
            1 => citation.ranges[0].1 += 1,
            _ => citation.source_digest = "changed".into(),
        }
        assert_eq!(
            complete_skill_opportunity_evidence(
                sampled_saved.clone(),
                &finding,
                &[work.clone(), usage.clone()]
            )
            .status,
            BurnCheckEvidenceStatus::Unavailable
        );
    }
    let mut changed = usage;
    changed.reference.thread_digest = "other-thread".into();
    assert_eq!(
        complete_skill_opportunity_evidence(saved, &finding, &[work, changed]).status,
        BurnCheckEvidenceStatus::Unavailable
    );
}
