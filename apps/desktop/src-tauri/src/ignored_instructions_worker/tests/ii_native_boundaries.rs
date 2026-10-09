use super::*;
use antiburn_local::analysis::jev::capabilities::ModelCapabilities;
use antiburn_local::analysis::jev::{JevAnswer, JevQuestion, JevUsage};
use antiburn_local::insights::{DetectorId, ReportWindow};
use serde_json::Value;

fn native_sources() -> [(&'static str, &'static str, SourceFormat, &'static str); 3] {
    [
        (
            "claude-code",
            "scope-test",
            SourceFormat::ClaudeJsonl,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../crates/antiburn-local/tests/fixtures/claude_characterization/retained_native_results.jsonl"
            )),
        ),
        (
            "codex",
            "synthetic-root",
            SourceFormat::CodexRolloutJsonl,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../crates/antiburn-local/tests/fixtures/codex_characterization/paginated_completed.jsonl"
            )),
        ),
        (
            "pi",
            "synthetic",
            SourceFormat::PiV3Jsonl,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../crates/antiburn-local/tests/fixtures/pi_characterization/core_linear_v3.jsonl"
            )),
        ),
    ]
}

async fn prepare_native(
    store: &Store,
    candidate: &BurnCheckCandidate,
    home: &Path,
    capabilities: &ModelCapabilities,
) -> PreparedInput {
    let origin = store
        .observe_burn_check_sample_origin(candidate, CHECK_ID)
        .unwrap();
    let outcome = prepare_selected_input_with_home(
        store,
        candidate,
        None,
        None,
        home,
        &mut InstructionDiscoveryCache::default(),
        SamplingPass {
            pairs: &[],
            round: 0,
            backlog: false,
            origin: &origin,
            capabilities,
            legacy_fixture: false,
        },
    )
    .await
    .unwrap();
    let PrepareInputOutcome::Ready(input) = outcome else {
        panic!("native source must reach II preparation")
    };
    *input
}

async fn evaluate_native(
    page: &PreparedInput,
    permission: &str,
) -> ignored_instructions::AssessmentResult {
    let permission = permission.to_owned();
    let mut context = page.context.clone();
    ignored_instructions::extend_jev_context_with_history_and_capabilities(
        &mut context,
        &mut [],
        &page.page_actions,
        page.prior_history_complete,
        &ModelCapabilities::jev_default(),
    )
    .unwrap();
    let outcome = run_jev_check(
        &ignored_instructions::IgnoredInstructionsCheck,
        &context,
        JevRunProgress::default(),
        move |batch| {
            let answers = batch
                .request
                .questions
                .iter()
                .map(|(key, question)| {
                    let JevQuestion::Choice { criteria, .. } = question else {
                        panic!("expected choice")
                    };
                    let local_key = &batch.answer_owners[key].1;
                    assert!(local_key.ends_with("::decision"), "{local_key}");
                    let selected = if permission == "authoritative_approval" {
                        "uncertain"
                    } else {
                        "conflict"
                    };
                    assert!(criteria.contains_key(selected), "{key}: {selected}");
                    (
                        key.clone(),
                        JevAnswer::Choice {
                            choice: selected.into(),
                            confidence: 0.98,
                            probabilities: criteria
                                .keys()
                                .map(|choice| {
                                    (
                                        choice.clone(),
                                        if choice == selected {
                                            0.98
                                        } else {
                                            0.02 / (criteria.len() - 1) as f64
                                        },
                                    )
                                })
                                .collect(),
                        },
                    )
                })
                .collect();
            async move {
                Ok(JevResponse {
                    model: ignored_instructions::ASSESSMENT_MODEL.into(),
                    answers,
                    usage: JevUsage {
                        input_tokens: 0,
                        output_tokens: 0,
                    },
                })
            }
        },
        |_| Ok(()),
    )
    .await
    .unwrap();
    assert!(outcome.complete);
    assert!(outcome.failure.is_none());
    let mut result = outcome.result;
    result.input_revision = page.input.input_revision.clone();
    result
}

fn findings(directory: &Path) -> crate::insights_report::CurrentFindingsPage {
    crate::insights_report::list_current_findings(
        directory,
        crate::insights_report::CurrentFindingsRequest {
            environment_key: "native".into(),
            window: ReportWindow {
                start_epoch: 0,
                end_epoch: 3000,
            },
            detector: DetectorId::IgnoredInstructions,
        },
    )
    .unwrap()
}

#[test]
fn preparation_distinguishes_missing_malformed_stale_and_empty_inputs() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for (sql, expected) in [
        (
            "DELETE FROM session_evidence",
            Some(PreparationUnavailable::EvidenceMissing),
        ),
        (
            "UPDATE session_evidence SET evidence_json = NULL",
            Some(PreparationUnavailable::EvidenceMissing),
        ),
        (
            "UPDATE session_evidence SET evidence_json = '{bad'",
            Some(PreparationUnavailable::EvidenceMalformed),
        ),
        (
            "UPDATE session_evidence SET status = 'pending'",
            Some(PreparationUnavailable::EvidenceNotPublished),
        ),
        (
            "UPDATE session_evidence SET published_fence = published_fence + 1",
            None,
        ),
    ] {
        let fixture = crate::scope_creep_worker::tests::NativeFixture::new(1);
        let candidate = fixture.publish();
        let home = tempfile::tempdir().unwrap();
        let origin = fixture
            .store
            .observe_burn_check_sample_origin(&candidate, CHECK_ID)
            .unwrap();
        fixture.store.lock().execute_batch(sql).unwrap();
        let outcome = runtime
            .block_on(prepare_selected_input_with_home(
                &fixture.store,
                &candidate,
                None,
                None,
                home.path(),
                &mut InstructionDiscoveryCache::default(),
                SamplingPass {
                    pairs: &[],
                    round: 0,
                    backlog: false,
                    origin: &origin,
                    capabilities: &ModelCapabilities::jev_default(),
                    legacy_fixture: false,
                },
            ))
            .unwrap();
        match (expected, outcome) {
            (Some(expected), PrepareInputOutcome::Unavailable(reason)) => {
                assert_eq!(reason, expected);
                assert_ne!(reason.category(), "evidence_unavailable");
            }
            (None, PrepareInputOutcome::Stale) => {}
            _ => panic!("unexpected preparation outcome for {sql}"),
        }
    }
    let fixture = crate::scope_creep_worker::tests::NativeFixture::new(1);
    let candidate = fixture.publish();
    let home = tempfile::tempdir().unwrap();
    let origin = fixture
        .store
        .observe_burn_check_sample_origin(&candidate, CHECK_ID)
        .unwrap();
    let outcome = runtime
        .block_on(prepare_selected_input_with_home(
            &fixture.store,
            &candidate,
            None,
            None,
            home.path(),
            &mut InstructionDiscoveryCache::default(),
            SamplingPass {
                pairs: &[],
                round: 0,
                backlog: false,
                origin: &origin,
                capabilities: &ModelCapabilities::jev_default(),
                legacy_fixture: false,
            },
        ))
        .unwrap();
    let PrepareInputOutcome::Ready(input) = outcome else {
        panic!("empty comparison inventory must remain ready")
    };
    let plan = ignored_instructions::IgnoredInstructionsCheck
        .prepare_with_capabilities(&input.context, &ModelCapabilities::jev_default())
        .unwrap();
    assert_eq!(plan.prepared.coverage.candidate_pairs, 0);
    assert!(!input.more_content);
}

#[test]
fn readable_instruction_enrolls_when_a_sibling_file_is_invalid_utf8() {
    let fixture = crate::scope_creep_worker::tests::NativeFixture::new(1);
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("AGENTS.md"), [0xff]).unwrap();
    std::fs::write(
        fixture.directory.path().join("AGENTS.md"),
        "Do not change billing.\n",
    )
    .unwrap();
    fixture
        .store
        .set_check_enabled(DetectorId::IgnoredInstructions, true)
        .unwrap();
    fixture
        .store
        .capture_burn_check_boundaries(&[CHECK_ID], 0)
        .unwrap();
    let mut candidate = fixture.publish();
    candidate.session.cwd = Some(fixture.directory.path().to_str().unwrap().to_owned());
    fixture
        .store
        .lock()
        .execute("UPDATE session SET cwd = ?1", [&candidate.session.cwd])
        .unwrap();
    let page = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(prepare_native(
            &fixture.store,
            &candidate,
            home.path(),
            &ModelCapabilities::jev_default(),
        ));
    assert!(page.future_only);
    assert_eq!(page.content.instructions.len(), 1);
    assert!(
        page.content.instructions[0]
            .text
            .contains("Do not change billing.")
    );
    assert!(!page.content.complete);
}

#[tokio::test]
async fn four_native_agents_prepare_publish_and_report_without_invented_authority() {
    for (agent, id, format, records) in native_sources().into_iter().chain([(
        "opencode",
        "ii-native",
        SourceFormat::OpenCodeSqliteV2,
        "",
    )]) {
        let directory = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        std::fs::write(
            directory.path().join("AGENTS.md"),
            "Do not run Bash commands.\n",
        )
        .unwrap();
        store.capture_burn_check_boundaries(&[CHECK_ID], 0).unwrap();
        let mut record = synthetic_session_record(id, directory.path(), "native-v1", 9);
        record.key.agent = agent.into();
        let source = if agent == "opencode" {
            let path = directory.path().join("native.db");
            initialize_opencode_database(&path, id).unwrap();
            append_messages(
                &path,
                id,
                &[
                    SyntheticMessage {
                        native_id: Some("human-task"),
                        role: "user",
                        parts: vec![SyntheticPart::Text("Inspect the queue.".into())],
                    },
                    SyntheticMessage {
                        native_id: Some("failed-test"),
                        role: "assistant",
                        parts: vec![SyntheticPart::Tool {
                            name: "bash",
                            input: json!({"command": "cargo test --lib"}),
                            output: "FAILED (failures=1)",
                        }],
                    },
                ],
                1000,
            )
            .unwrap();
            RawSource::Sqlite(path)
        } else {
            record.source_kind = "file".into();
            let path = directory.path().join("native.jsonl");
            let records = if agent == "claude-code" {
                records
                    .lines()
                    .enumerate()
                    .map(|(index, line)| {
                        let mut row: Value = serde_json::from_str(line).unwrap();
                        row["timestamp"] = json!(format!("2026-01-01T00:00:{index:02}Z"));
                        row.to_string()
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n"
            } else if agent == "pi" {
                let command = json!({"type":"message", "id":"test-call", "parentId":"selected", "timestamp":"2026-01-01T00:00:07Z", "message":{"role":"assistant", "content":[{"type":"toolCall", "id":"test-id", "name":"bash", "arguments":{"command":"cargo test --lib"}}]}});
                format!("{}\n{command}\n", records.trim_end())
            } else {
                records.to_owned()
            };
            std::fs::write(&path, records).unwrap();
            RawSource::File(path)
        };
        store
            .upsert_sessions(&[record.clone()], &crate::agents::evidence_cohort())
            .unwrap();
        publish_native_source(&store, agent, id, source, format, "native-v1", 100).unwrap();
        let mut candidate = store
            .burn_check_candidates(CHECK_ID, NOW, IDLE_SECS, 16)
            .unwrap()
            .pop()
            .unwrap();
        candidate.historical = true;
        candidate.boundary_positions.clear();
        let capabilities = ModelCapabilities::jev_default();
        let page = prepare_native(&store, &candidate, home.path(), &capabilities).await;
        assert_eq!(page.content.source_format, format);
        assert!(!page.future_only);
        assert!(
            page.content.actions.iter().any(|action| action
                .metadata
                .human_text
                .as_ref()
                .is_some_and(|fact| fact.matches_action(action))),
            "{agent} retains source-backed human task text"
        );
        assert!(
            page.content
                .instructions
                .iter()
                .all(|rule| rule.provenance == InstructionProvenance::CurrentFileComparison)
        );
        let result = evaluate_native(&page, "independent").await;
        assert!(!result.findings.is_empty(), "{agent}: {result:?}");
        for finding in &result.findings {
            assert!(matches!(
                resolve_content_reference(
                    &page.content,
                    page.content.publication_fence,
                    &page.content.selected_input_digest,
                    &finding.reference.action_id
                ),
                ContentReferenceResolution::Found(_)
            ));
        }
        if agent == "codex" || agent == "opencode" {
            let failed = page
                .content
                .actions
                .iter()
                .find(|action| {
                    action.kind == "tool_result" && action.text.contains("FAILED (failures=1)")
                })
                .expect("failed native command output remains selected");
            let binding = failed
                .metadata
                .command_result
                .as_ref()
                .expect("failed output has an exact command binding");
            assert!(binding.matches_action(failed));
            assert!(
                page.content
                    .actions
                    .iter()
                    .any(|action| binding.matches_request(action))
            );
            assert!(failed.metadata.human_text.is_none());
        }
        for action in page.content.actions.iter().filter(|action| {
            action.text.contains("<skill")
                || action.text.contains("Base directory for this skill:")
                || action.text.contains("The tool output says:")
        }) {
            assert!(
                action.metadata.human_text.is_none(),
                "{agent} skill and notification text must not authorize work"
            );
        }
        std::fs::write(
            directory.path().join("AGENTS.md"),
            "Get maintainer approval before running Bash commands.\n",
        )
        .unwrap();
        let approval_page = prepare_native(&store, &candidate, home.path(), &capabilities).await;
        let approval = evaluate_native(&approval_page, "authoritative_approval").await;
        assert!(
            approval.findings.is_empty(),
            "{agent} must not invent human approval"
        );
        assert!(!approval.unassessed_comparisons.is_empty(), "{agent}");
        std::fs::write(
            directory.path().join("AGENTS.md"),
            "Do not run Bash commands.\n",
        )
        .unwrap();
        assert!(
            store
                .queue_burn_check_assessment(&page.input, NOW, IDLE_SECS)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(&page.input, NOW, LEASE_SECS, IDLE_SECS)
                .unwrap()
        );
        if agent == "codex" {
            let changed_capabilities = ModelCapabilities {
                model_revision: Some("changed-native-runtime".into()),
                ..capabilities.clone()
            };
            let changed =
                prepare_native(&store, &candidate, home.path(), &changed_capabilities).await;
            assert_ne!(changed.input.input_revision, page.input.input_revision);
            assert!(
                store
                    .queue_burn_check_assessment(&changed.input, NOW, IDLE_SECS)
                    .unwrap()
            );
            assert!(
                !store
                    .complete_burn_check_assessment(
                        &page.input,
                        &serde_json::to_string(&result).unwrap(),
                        NOW + 1,
                        IDLE_SECS
                    )
                    .unwrap()
            );
            assert!(findings(directory.path()).findings.is_empty());
            assert!(
                store
                    .queue_burn_check_assessment(&page.input, NOW + 1, IDLE_SECS)
                    .unwrap()
            );
            assert!(
                store
                    .claim_burn_check_assessment(&page.input, NOW + 1, LEASE_SECS, IDLE_SECS)
                    .unwrap()
            );
        }
        assert!(
            store
                .complete_burn_check_assessment(
                    &page.input,
                    &serde_json::to_string(&result).unwrap(),
                    NOW + 1,
                    IDLE_SECS
                )
                .unwrap()
        );
        drop(store);
        let store = Store::open(directory.path()).unwrap();
        let published = findings(directory.path());
        assert_eq!(published.findings.len(), result.findings.len(), "{agent}");
        assert!(
            published
                .findings
                .iter()
                .all(|finding| finding.agent == agent
                    && finding.published_fence == page.input.published_fence)
        );
        let report = crate::insights_report::reduce_report_blocking_with_home(
            directory.path(),
            crate::insights_report::ReportRequest {
                environment_key: "native".into(),
                window: ReportWindow {
                    start_epoch: 0,
                    end_epoch: 3000,
                },
                computed_at_epoch: NOW + 1,
            },
            home.path(),
        )
        .unwrap();
        assert_eq!(
            report.report.detectors[DetectorId::IgnoredInstructions.index()].finding,
            1,
            "{agent}"
        );
        let controller =
            crate::remediation::RemediationController::new(directory.path().to_owned());
        let targets = controller
            .list_burn_check_targets(
                &store,
                DetectorId::IgnoredInstructions,
                crate::remediation::BurnCheckTargetContext {
                    environment_key: "native".into(),
                    window: ReportWindow {
                        start_epoch: 0,
                        end_epoch: 3000,
                    },
                },
            )
            .unwrap();
        let target = targets
            .targets
            .first()
            .expect("native finding has a product target");
        let evidence = controller
            .burn_check_target_evidence(&store, &target.action_id)
            .unwrap();
        assert_eq!(
            evidence.status,
            crate::remediation::BurnCheckEvidenceStatus::Available,
            "{agent} native evidence must validate"
        );
        assert!(evidence.decision_proof.is_some(), "{agent}");
        for finding in &result.findings {
            let occurrence = evidence
                .occurrences
                .iter()
                .find(|occurrence| occurrence.finding_id == finding.id)
                .expect("each native finding has validated evidence");
            assert_eq!(
                occurrence.status,
                crate::remediation::BurnCheckEvidenceStatus::Available
            );
            assert!(occurrence.decision_proof.is_some());
            let action = occurrence
                .items
                .iter()
                .find(|item| {
                    item.label == crate::remediation::BurnCheckEvidenceLabel::ObservedAction
                })
                .expect("validated evidence contains the exact action");
            assert_eq!(action.reference, finding.reference.action_id);
            assert_eq!(action.excerpt, finding.action_excerpt);
        }
        let prompt = controller
            .copy_prompt_fix_burn_check_target(&store, &target.action_id)
            .unwrap();
        assert!(prompt.prompt.contains(&format!("Agent: {agent:?}")));
        assert!(
            prompt
                .prompt
                .contains("We do not know if it had the same text when the action happened.")
        );
        assert!(prompt.prompt.contains("Remediation reference: ABR-"));
        record.source_fingerprint = Some("native-v2".into());
        record.activity_cursor = "native-v2".into();
        store
            .upsert_sessions(&[record], &crate::agents::evidence_cohort())
            .unwrap();
        assert!(
            findings(directory.path()).findings.is_empty(),
            "{agent} stale source must leave the report"
        );
        let stale_evidence = controller
            .burn_check_target_evidence(&store, &target.action_id)
            .unwrap();
        assert_eq!(
            stale_evidence.status,
            crate::remediation::BurnCheckEvidenceStatus::Unavailable
        );
        assert!(stale_evidence.decision_proof.is_none());
        assert!(stale_evidence.items.is_empty());
        assert!(
            controller
                .copy_prompt_fix_burn_check_target(&store, &target.action_id)
                .is_err(),
            "{agent} stale target must not produce a prompt"
        );
        assert!(
            !store
                .complete_burn_check_assessment(
                    &page.input,
                    &serde_json::to_string(&result).unwrap(),
                    NOW + 2,
                    IDLE_SECS
                )
                .unwrap()
        );
    }
}

const CONFIRMATION: &str = include_str!(
    "../../../../../../crates/antiburn-local/tests/fixtures/ignored_instructions/authority_confirmation.json"
);

#[test]
fn native_confirmation_contract_marks_approval_and_execution_as_unavailable() {
    let fixture: Value = serde_json::from_str(CONFIRMATION).expect("confirmation contract parses");
    let cases = fixture["cases"].as_array().expect("cases are listed");
    for (format, family) in [
        ("ClaudeJsonl", "approval_boundary"),
        ("CodexRolloutJsonl", "approval_boundary"),
        ("PiV3Jsonl", "approval_boundary"),
        ("OpenCodeSqliteV2", "approval_boundary"),
    ] {
        assert!(
            cases.iter().any(|case| {
                case["format"] == format
                    && case["family"] == family
                    && case["expected"] == "unassessed"
                    && case["citations"].as_array().is_some_and(Vec::is_empty)
            }),
            "{format} approval remains unknown without a native authority claim"
        );
    }

    for (format, family) in [
        ("ClaudeJsonl", "execution_proof"),
        ("CodexRolloutJsonl", "execution_proof"),
        ("PiV3Jsonl", "execution_proof"),
        ("OpenCodeSqliteV2", "execution_proof"),
    ] {
        assert!(
            cases.iter().any(|case| {
                case["format"] == format
                    && case["family"] == family
                    && case["expected"] == "unassessed"
            }),
            "{format} execution success requires evidence the selected source does not provide"
        );
    }
}

#[test]
fn unknown_authority_and_failed_command_output_are_not_success_evidence() {
    let fixture: Value = serde_json::from_str(CONFIRMATION).expect("confirmation contract parses");
    let cases = fixture["cases"].as_array().expect("cases are listed");
    let failed = cases
        .iter()
        .find(|case| case["id"] == "confirmation-12")
        .expect("native output exclusion case exists");
    assert_eq!(failed["expected"], "unassessed");
    assert!(
        failed["selected"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| { event[1] != "BashCommandOutput" })
    );
    assert!(
        failed["rationale"]
            .as_str()
            .unwrap()
            .contains("failure output is excluded")
    );

    let authority = cases
        .iter()
        .find(|case| case["id"] == "confirmation-18")
        .expect("unknown tool authority case exists");
    assert_eq!(authority["expected"], "unassessed");
    assert!(authority["citations"].as_array().unwrap().is_empty());
}
