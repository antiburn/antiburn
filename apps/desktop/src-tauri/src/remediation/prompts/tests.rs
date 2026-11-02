use super::*;
use antiburn_local::insights::ReportWindow;

struct Fixture {
    _directory: tempfile::TempDir,
    store: Store,
    controller: RemediationController,
}

impl Fixture {
    fn new(names: &[&str]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        insights_report::tests::publish_unused_resources(&store, "first", 120, names, &[]);
        let controller = RemediationController::new(directory.path().to_owned());
        Self {
            _directory: directory,
            store,
            controller,
        }
    }

    fn context(&self) -> BurnCheckTargetContext {
        BurnCheckTargetContext {
            environment_key: "native".into(),
            window: ReportWindow {
                start_epoch: 100,
                end_epoch: 200,
            },
        }
    }

    fn snapshot(&self) -> CurrentPromptTargets {
        self.controller
            .current_prompt_targets(&self.store, DetectorId::UnusedMcpServers, self.context())
            .unwrap()
    }

    fn id(&self, name: &str) -> String {
        stable_finding_id(
            self.snapshot()
                .targets
                .iter()
                .find(|target| {
                    target
                        .resource
                        .as_ref()
                        .is_some_and(|resource| resource.target.canonical_name == name)
                })
                .unwrap(),
        )
    }

    fn copy(&self, id: &str) -> PromptFixResult {
        self.controller
            .copy_prompt_fix_burn_check_target_current(
                &self.store,
                DetectorId::UnusedMcpServers,
                id,
                self.context(),
            )
            .unwrap()
    }

    fn record(&self, id: &str) -> RemediationRecord {
        self.store.remediation(id).unwrap().unwrap()
    }
}

#[test]
fn prompt_copy_preserves_every_active_state_and_ignores_stored_text() {
    for state in [
        RemediationState::WaitingForPromptUse,
        RemediationState::Reserved,
        RemediationState::Writing,
        RemediationState::RecoveryNeeded,
        RemediationState::Watching,
        RemediationState::Fixed,
        RemediationState::Recurred,
    ] {
        let fixture = Fixture::new(&["issue-776-state"]);
        let id = fixture.id("issue-776-state");
        let current = fixture.snapshot();
        let target = current
            .targets
            .iter()
            .find(|target| stable_finding_id(target) == id)
            .unwrap();
        let now = now_epoch();
        let has_boundary = matches!(
            state,
            RemediationState::Watching | RemediationState::Fixed | RemediationState::Recurred
        );
        let verified_at_epoch =
            matches!(state, RemediationState::Fixed | RemediationState::Recurred).then_some(now);
        let recurred_at_epoch = (state == RemediationState::Recurred).then_some(now);
        let (remediation, _) = fixture
            .controller
            .watch_input(target, state, has_boundary.then_some(123000), now, None)
            .unwrap();
        assert!(!remediation.result_json.contains("promptText"));
        let mut result: serde_json::Value = serde_json::from_str(&remediation.result_json).unwrap();
        result["promptText"] = json!("stale stored text");
        let display = serde_json::to_string(&StoredDisplaySnapshot {
            version: 1,
            finding_id: id.clone(),
            display: burn_check_display_facts(target, None),
        })
        .unwrap();
        let watch_id = remediation.remediation_id.clone();
        fixture
            .store
            .lock()
            .execute(
                "INSERT INTO remediation (
                remediation_id, target_key, environment_key, agent, scope_kind, scope_key,
                state, definition_json, result_json, created_at_epoch, updated_at_epoch,
                effective_boundary_ms, verified_at_epoch, recurred_at_epoch,
                origin, prompt_group_id, display_snapshot_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?11, ?12, ?13,
                 'action', 'old-group', ?14)",
                rusqlite::params![
                    watch_id,
                    remediation.target_key,
                    remediation.environment_key,
                    remediation.agent,
                    remediation.scope_kind,
                    remediation.scope_key,
                    state.as_str(),
                    remediation.definition_json,
                    result.to_string(),
                    now,
                    remediation.effective_boundary_ms,
                    verified_at_epoch,
                    recurred_at_epoch,
                    display,
                ],
            )
            .unwrap();
        let before = fixture.record(&watch_id);
        let snapshot = fixture
            .store
            .remediation_display_snapshot(&watch_id)
            .unwrap();
        let copied = fixture.copy(&id);
        assert!(copied.prompt.contains("issue-776-state"));
        assert!(!copied.prompt.contains("stale stored text"));
        assert!(!copied.prompt.contains("ABR-old-group"));
        let copied_watch = copied.watch.unwrap();
        if state == RemediationState::Recurred {
            assert_ne!(copied_watch.watch_id, watch_id);
            assert_eq!(
                copied_watch.lifecycle,
                RemediationState::WaitingForPromptUse
            );
        } else {
            assert_eq!(copied_watch.watch_id, watch_id);
            assert_eq!(copied_watch.lifecycle, state);
        }
        assert_eq!(fixture.record(&watch_id), before);
        assert_eq!(
            fixture
                .store
                .remediation_display_snapshot(&watch_id)
                .unwrap(),
            snapshot
        );
        assert_eq!(
            copied.prompt.contains(&format!("ABR-{watch_id}")),
            state == RemediationState::WaitingForPromptUse
        );
    }
}

#[test]
fn prompt_copy_reads_new_accepted_evidence_for_the_same_identity() {
    let fixture = Fixture::new(&["issue-776-fresh"]);
    let id = fixture.id("issue-776-fresh");
    let first = fixture.copy(&id);
    let watch_id = first.watch.unwrap().watch_id;
    let before = fixture.record(&watch_id);
    insights_report::tests::publish_unused_resources(
        &fixture.store,
        "latest-accepted",
        150,
        &["issue-776-fresh"],
        &[],
    );
    let copied = fixture.copy(&id);
    assert!(
        copied.prompt.contains("latest-accepted.jsonl"),
        "{}",
        copied.prompt
    );
    assert_ne!(copied.prompt, first.prompt);
    assert_eq!(copied.watch.unwrap().watch_id, watch_id);
    assert_eq!(fixture.record(&watch_id), before);
}

#[test]
fn prompt_copy_uses_exact_waiting_ids_across_old_groups_and_subsets() {
    let fixture = Fixture::new(&["issue-776-one", "issue-776-two", "issue-776-three"]);
    let one = fixture.id("issue-776-one");
    let two = fixture.id("issue-776-two");
    let three = fixture.id("issue-776-three");
    let first_id = fixture.copy(&one).watch.unwrap().watch_id;
    let second_id = fixture.copy(&two).watch.unwrap().watch_id;
    for (id, group) in [(&first_id, "old-one"), (&second_id, "old-two")] {
        fixture
            .store
            .lock()
            .execute(
                "UPDATE remediation SET prompt_group_id = ?1 WHERE remediation_id = ?2",
                rusqlite::params![group, id],
            )
            .unwrap();
    }
    let first_before = fixture.record(&first_id);
    let second_before = fixture.record(&second_id);
    let combined = fixture
        .controller
        .copy_prompt_fix_burn_check_targets_current(
            &fixture.store,
            DetectorId::UnusedMcpServers,
            &[one.clone(), two.clone(), three.clone()],
            fixture.context(),
        )
        .unwrap();
    assert!(combined.prompt.contains(&format!("ABR-{first_id}")));
    assert!(combined.prompt.contains(&format!("ABR-{second_id}")));
    assert!(!combined.prompt.contains("ABR-old-"));
    assert_eq!(combined.prompt.matches(PROMPT_REFERENCE_PREFIX).count(), 3);
    let subset = fixture
        .controller
        .copy_prompt_fix_burn_check_targets_current(
            &fixture.store,
            DetectorId::UnusedMcpServers,
            &[two, three],
            fixture.context(),
        )
        .unwrap();
    assert!(!subset.prompt.contains(&format!("ABR-{first_id}")));
    assert_eq!(subset.prompt.matches(PROMPT_REFERENCE_PREFIX).count(), 2);
    assert_eq!(fixture.record(&first_id), first_before);
    assert_eq!(fixture.record(&second_id), second_before);
}

#[test]
fn prompt_copy_matches_before_the_display_cap_and_does_not_require_live_handles() {
    let names = (0..105)
        .map(|index| format!("issue-776-target-{index:03}"))
        .collect::<Vec<_>>();
    let fixture = Fixture::new(&names.iter().map(String::as_str).collect::<Vec<_>>());
    let listing = fixture
        .controller
        .list_burn_check_targets(
            &fixture.store,
            DetectorId::UnusedMcpServers,
            fixture.context(),
        )
        .unwrap();
    assert_eq!(listing.targets.len(), MAX_TARGETS);
    let snapshot = fixture.snapshot();
    let omitted = snapshot
        .targets
        .iter()
        .find(|target| {
            !listing
                .targets
                .iter()
                .any(|listed| listed.finding_id == stable_finding_id(target))
        })
        .unwrap();
    let copied = fixture.copy(&stable_finding_id(omitted));
    assert!(
        copied
            .prompt
            .contains(&omitted.resource.as_ref().unwrap().target.canonical_name)
    );
    assert!(!copied.prompt.contains("Selection limit"));
    fixture
        .controller
        .state
        .lock()
        .unwrap()
        .targets
        .values_mut()
        .for_each(|entries| {
            for entry in entries {
                entry.created_at_epoch = now_epoch() - ID_TTL.as_secs() as i64 - 1;
            }
        });
    let listed = &listing.targets[0];
    assert_eq!(
        fixture
            .controller
            .copy_prompt_fix_burn_check_target(&fixture.store, &listed.action_id),
        Err(ControllerError::TargetExpired),
    );
    assert!(fixture.copy(&listed.finding_id).prompt.contains("Evidence"));
}

#[test]
fn missing_target_has_no_exact_facts_or_watch_and_whole_check_truncation_is_bounded() {
    let names = (0..105)
        .map(|index| format!("issue-776-limit-{index:03}"))
        .collect::<Vec<_>>();
    let fixture = Fixture::new(&names.iter().map(String::as_str).collect::<Vec<_>>());
    let copied = fixture.copy("no-longer-present");
    assert!(copied.prompt.contains("Inspect only"));
    assert!(!copied.prompt.contains("issue-776-limit"));
    assert!(!copied.prompt.contains("Failed check"));
    assert!(copied.watch.is_none());
    let whole = fixture
        .controller
        .copy_prompt_fix_burn_check(
            &fixture.store,
            DetectorId::UnusedMcpServers,
            fixture.context(),
        )
        .unwrap();
    assert!(!whole.prompt.contains("issue-776-limit"));
    assert!(whole.prompt.len() <= antiburn_local::remediation::MAX_PROMPT_BYTES);
}

#[test]
fn disappeared_target_does_not_reuse_previous_prompt_facts_or_watch() {
    let fixture = Fixture::new(&["issue-776-disappeared"]);
    let id = fixture.id("issue-776-disappeared");
    let watch_id = fixture.copy(&id).watch.unwrap().watch_id;
    let before = fixture.record(&watch_id);
    let mut session = fixture
        .store
        .session(&SessionKey::new("native", "claude-code", "first"))
        .unwrap()
        .unwrap();
    session.source_fingerprint = Some("changed-source".into());
    session.activity_cursor = "changed-source".into();
    fixture
        .store
        .upsert_sessions(&[session], &crate::agents::evidence_cohort())
        .unwrap();
    let copied = fixture.copy(&id);
    assert!(copied.prompt.contains("Inspect only"));
    assert!(!copied.prompt.contains("issue-776-disappeared"));
    assert!(!copied.prompt.contains(PROMPT_REFERENCE_PREFIX));
    assert!(copied.watch.is_none());
    assert_eq!(fixture.record(&watch_id), before);
}

#[test]
fn full_watch_capacity_does_not_block_prompt_copy() {
    let fixture = Fixture::new(&["issue-776-capacity"]);
    let id = fixture.id("issue-776-capacity");
    fixture.store.lock().execute_batch(
        "WITH RECURSIVE entries(n) AS (VALUES(1) UNION ALL SELECT n + 1 FROM entries WHERE n < 1000)
         INSERT INTO remediation (
             remediation_id, target_key, environment_key, agent, scope_kind, scope_key,
             state, definition_json, result_json, created_at_epoch, updated_at_epoch, origin)
         SELECT 'capacity-' || n, 'capacity-' || n, 'native', 'claude-code', 'global', 'scope',
             'waitingForPromptUse', '{\"version\":1}', '{\"version\":1}', 1, 1, 'action' FROM entries;",
    ).unwrap();
    let copied = fixture.copy(&id);
    assert!(copied.prompt.contains("issue-776-capacity"));
    assert!(!copied.prompt.contains(PROMPT_REFERENCE_PREFIX));
    assert!(copied.watch.is_none());
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row("SELECT count(*) FROM remediation", [], |row| row
                .get::<_, usize>(0))
            .unwrap(),
        1000
    );
}

#[test]
fn prompt_larger_than_watch_metadata_limit_stays_within_the_prompt_limit() {
    let names = (0..100)
        .map(|index| format!("issue-776-large-{index:03}-{}", "a".repeat(220)))
        .collect::<Vec<_>>();
    let fixture = Fixture::new(&names.iter().map(String::as_str).collect::<Vec<_>>());
    let ids = fixture
        .snapshot()
        .targets
        .iter()
        .filter(|target| {
            target.resource.as_ref().is_some_and(|resource| {
                resource
                    .target
                    .canonical_name
                    .starts_with("issue-776-large-")
            })
        })
        .map(stable_finding_id)
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 100);
    let copied = fixture
        .controller
        .copy_prompt_fix_burn_check_targets_current(
            &fixture.store,
            DetectorId::UnusedMcpServers,
            &ids,
            fixture.context(),
        )
        .unwrap();
    assert!(
        copied.prompt.len() > 32 * 1024,
        "{} bytes",
        copied.prompt.len()
    );
    assert!(copied.prompt.len() <= antiburn_local::remediation::MAX_PROMPT_BYTES);
    assert!(copied.prompt.contains("issue-776-large-099"));
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row(
                "SELECT count(*) FROM remediation WHERE result_json LIKE '%promptText%'",
                [],
                |row| row.get::<_, usize>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn exact_prompt_sections_keep_their_own_evidence_paths() {
    let fixture = Fixture::new(&[]);
    let sessions = [
        "first-target",
        "second-target",
        "third-target",
        "fourth-target",
    ];
    for session in sessions {
        insights_report::tests::publish_reasoning_at_cwd(
            &fixture.store,
            session,
            120,
            120000,
            &Path::new("/synthetic").join(session),
            "max",
        );
        fixture
            .store
            .lock()
            .execute(
                "UPDATE session_evidence
                    SET effective_reasoning_target_hash = ?1,
                        effective_reasoning_scope = 'global', effective_reasoning = 'max'
                  WHERE environment_key = 'native' AND agent = 'claude-code' AND session_id = ?2",
                rusqlite::params![format!("reasoning-target-{session}"), session],
            )
            .unwrap();
    }
    let snapshot = fixture
        .controller
        .current_prompt_targets(
            &fixture.store,
            DetectorId::ModelOverthinking,
            fixture.context(),
        )
        .unwrap();
    assert_eq!(snapshot.targets.len(), sessions.len());
    let ids = snapshot
        .targets
        .iter()
        .map(stable_finding_id)
        .collect::<Vec<_>>();
    let copied = fixture
        .controller
        .copy_prompt_fix_burn_check_targets_current(
            &fixture.store,
            DetectorId::ModelOverthinking,
            &ids,
            fixture.context(),
        )
        .unwrap();
    let sections = copied
        .prompt
        .split("Exact target ")
        .skip(1)
        .collect::<Vec<_>>();
    assert_eq!(sections.len(), snapshot.targets.len());
    for (target, section) in snapshot.targets.iter().zip(sections) {
        let session = &target.findings[0].session_id;
        let path = format!("/home/avery/.claude/{session}.jsonl");
        assert!(
            section.contains(&path),
            "missing evidence path for {session}"
        );
        for other in sessions {
            if other != session.as_str() {
                let path = format!("/home/avery/.claude/{other}.jsonl");
                assert!(
                    !section.contains(&path),
                    "unrelated evidence path for {other}"
                );
            }
        }
    }
    assert!(copied.prompt.len() <= antiburn_local::remediation::MAX_PROMPT_BYTES);
}

#[test]
fn aggregate_prompt_overflow_uses_the_bounded_check_fallback() {
    let fixture = Fixture::new(&[]);
    insights_report::tests::publish_reasoning_at_cwd(
        &fixture.store,
        "reasoning",
        120,
        120000,
        Path::new("/synthetic/project"),
        "max",
    );
    let snapshot = fixture
        .controller
        .current_prompt_targets(
            &fixture.store,
            DetectorId::ModelOverthinking,
            fixture.context(),
        )
        .unwrap();
    let targets = vec![snapshot.targets.first().unwrap().clone(); MAX_CHECK_PROMPT_TARGETS];
    let prompt = fixture
        .controller
        .format_current_prompt(
            &fixture.store,
            DetectorId::ModelOverthinking,
            &targets,
            false,
        )
        .unwrap();
    assert!(prompt.contains("Check to inspect\nModel overthinking"));
    assert!(!prompt.contains("Exact target"));
    assert!(prompt.len() <= antiburn_local::remediation::MAX_PROMPT_BYTES);
}

#[test]
fn prompt_without_a_watch_rechecks_policy_after_reading_evidence() {
    let fixture = Fixture::new(&["issue-776-policy"]);
    let snapshot = fixture.snapshot();
    fixture
        .store
        .set_check_enabled(DetectorId::UnusedMcpServers, false)
        .unwrap();
    assert_eq!(
        fixture.controller.finish_current_prompt(
            &fixture.store,
            DetectorId::UnusedMcpServers,
            &[],
            &snapshot,
            true,
            true,
        ),
        Err(ControllerError::TargetChanged)
    );
    assert_eq!(
        fixture
            .controller
            .copy_prompt_fix_burn_check_target_current(
                &fixture.store,
                DetectorId::UnusedMcpServers,
                "missing",
                fixture.context(),
            ),
        Err(ControllerError::CheckPromptUnavailable)
    );
}

#[test]
fn prompt_copy_retries_one_stale_policy_snapshot() {
    let fixture = Fixture::new(&["issue-776-retry"]);
    let mut attempts = 0;
    let (_, prompt) = retry_prompt_snapshot(|| {
        attempts += 1;
        let snapshot = fixture.snapshot();
        if attempts == 1 {
            fixture
                .store
                .set_check_enabled(DetectorId::UnusedMcpServers, false)
                .unwrap();
            fixture
                .store
                .set_check_enabled(DetectorId::UnusedMcpServers, true)
                .unwrap();
        }
        fixture.controller.finish_current_prompt(
            &fixture.store,
            DetectorId::UnusedMcpServers,
            &snapshot.targets,
            &snapshot,
            false,
            false,
        )
    })
    .unwrap();
    assert_eq!(attempts, 2);
    assert!(prompt.contains("issue-776-retry"));
    let mut attempts = 0;
    let result: Result<(), _> = retry_prompt_snapshot(|| {
        attempts += 1;
        Err(ControllerError::TargetChanged)
    });
    assert_eq!(result, Err(ControllerError::TargetChanged));
    assert_eq!(attempts, 2);
}

#[test]
fn whole_check_copies_eligible_targets_without_copying_unavailable_targets() {
    let fixture = Fixture::new(&["issue-776-safe", "token=not-a-real-credential"]);
    let copied = fixture
        .controller
        .copy_prompt_fix_burn_check(
            &fixture.store,
            DetectorId::UnusedMcpServers,
            fixture.context(),
        )
        .unwrap();
    assert!(copied.prompt.contains("issue-776-safe"));
    assert!(!copied.prompt.contains("not-a-real-credential"));
    assert_eq!(copied.prompt.matches(PROMPT_REFERENCE_PREFIX).count(), 1);
}

#[test]
fn bounded_fallback_does_not_bypass_known_target_policy() {
    let fixture = Fixture::new(&["issue-776-policy-gate"]);
    let target = fixture
        .snapshot()
        .targets
        .into_iter()
        .find(|target| {
            target
                .resource
                .as_ref()
                .is_some_and(|resource| resource.target.canonical_name == "issue-776-policy-gate")
        })
        .unwrap();
    for (agent, source, cause, expected) in [
        (
            AgentKind::Copilot,
            SourceFormat::ClaudeJsonl,
            FindingCause::UnusedMcpServer {
                server: "issue-776-policy-gate".into(),
                tokens: None,
                cost_usd: None,
                pricing_revision: None,
            },
            RemediationUnavailableReason::UnsupportedSourceFormat,
        ),
        (
            AgentKind::Claude,
            SourceFormat::CodexRolloutJsonl,
            FindingCause::UnusedMcpServer {
                server: "issue-776-policy-gate".into(),
                tokens: None,
                cost_usd: None,
                pricing_revision: None,
            },
            RemediationUnavailableReason::UnsupportedSourceFormat,
        ),
        (
            AgentKind::Claude,
            SourceFormat::ClaudeJsonl,
            FindingCause::UnusedBuiltInTool {
                tool: "Read".into(),
                tokens: antiburn_local::remediation::BuiltInToolTokens::Unavailable,
                cost_usd: None,
                pricing_revision: None,
            },
            RemediationUnavailableReason::ProtectedBuiltInTool,
        ),
    ] {
        let mut target = target.clone();
        target.resource.as_mut().unwrap().finding =
            Finding::advisory_resource(agent, source, cause).unwrap();
        assert_eq!(
            fixture.controller.format_current_prompt(
                &fixture.store,
                target.finding().detector,
                &[target],
                true,
            ),
            Err(ControllerError::PromptUnavailable(expected))
        );
    }
}

#[test]
fn prompt_selection_rejects_empty_duplicate_and_oversized_batches() {
    for ids in [
        Vec::new(),
        vec!["same".into(), "same".into()],
        vec!["x".repeat(257)],
        vec!["x".into(); 101],
    ] {
        assert_eq!(
            validate_prompt_selection(&ids),
            Err(ControllerError::CheckPromptUnavailable)
        );
    }
}
