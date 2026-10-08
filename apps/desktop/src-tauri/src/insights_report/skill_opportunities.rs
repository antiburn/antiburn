use super::findings::CurrentFindingSession;
use super::*;

use crate::agent_config::{ConfigContext, skill_opportunity_snapshot};
use crate::jev::worker::JevCheckDescriptor;
use antiburn_local::checks::skill_opportunities::{
    SkillOpportunitiesResult, SkillOpportunitySnapshot,
};
use antiburn_local::model::AgentKind;
use antiburn_local::remediation::Finding;

/// Read current skill-opportunity findings from the durable assessment.
/// Unknown or stale inputs cannot produce either findings or a clean result.
pub(crate) fn skill_opportunity_findings(
    connection: &rusqlite::Connection,
    session: &CurrentFindingSession,
) -> Result<Option<Vec<Finding>>> {
    skill_opportunity_findings_for_session(
        connection,
        &session.evidence,
        IgnoredInstructionSessionIdentity {
            environment_key: &session.environment_key,
            agent: &session.agent,
            session_id: &session.session_id,
            incarnation: session.incarnation,
            source_generation: session.source_generation,
            source_fingerprint: session.source_fingerprint.as_deref(),
            published_fence: session.published_fence,
        },
        session.workspace_candidate.clone(),
    )
}

pub(super) fn skill_opportunity_findings_for_session(
    connection: &rusqlite::Connection,
    evidence: &SessionEvidence,
    session: IgnoredInstructionSessionIdentity<'_>,
    workspace_candidate: Option<PathBuf>,
) -> Result<Option<Vec<Finding>>> {
    let Some(home) = antiburn_local::paths::home_dir() else {
        return Ok(None);
    };
    skill_opportunity_findings_with_home(connection, evidence, session, workspace_candidate, &home)
}

fn skill_opportunity_findings_with_home(
    connection: &rusqlite::Connection,
    evidence: &SessionEvidence,
    session: IgnoredInstructionSessionIdentity<'_>,
    workspace_candidate: Option<PathBuf>,
    home: &Path,
) -> Result<Option<Vec<Finding>>> {
    if session.environment_key != "native"
        || evidence.identity.agent != session.agent
        || evidence.identity.session_id != session.session_id
        || !antiburn_local::analysis::smart_check_source_supported(
            session.agent,
            evidence.capabilities.source_format,
        )
    {
        return Ok(None);
    }

    let Some(snapshot) = current_skill_snapshot(session.agent, workspace_candidate, home) else {
        return Ok(None);
    };

    let stored = connection
        .query_row(
            "SELECT incarnation, source_generation, source_fingerprint, published_fence,
                    status, input_revision, result_revision, evaluator_revision, result_json
               FROM burn_check_assessment
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND check_id = ?4",
            params![
                session.environment_key,
                session.agent,
                session.session_id,
                antiburn_local::checks::skill_opportunities::SKILL_OPPORTUNITIES_CHECK_ID,
            ],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .optional()?;
    let Some((
        incarnation,
        source_generation,
        source_fingerprint,
        published_fence,
        status,
        input_revision,
        result_revision,
        evaluator_revision,
        result_json,
    )) = stored
    else {
        return Ok(None);
    };
    let Some(input_revision) = input_revision else {
        return Ok(None);
    };
    if incarnation != session.incarnation
        || source_generation != session.source_generation
        || source_fingerprint.as_deref() != session.source_fingerprint
        || published_fence != session.published_fence
        || result_revision.as_deref() != Some(input_revision.as_str())
        || evaluator_revision.as_deref()
            != Some(
                crate::skill_opportunities_worker::CHECK
                    .evaluator_revision()
                    .as_str(),
            )
    {
        return Ok(None);
    }
    let Some(result_json) = result_json else {
        return Ok(None);
    };
    let Ok(result) = serde_json::from_str::<SkillOpportunitiesResult>(&result_json) else {
        return Ok(None);
    };
    if !publication_revisions_match(&result_json, &input_revision, &snapshot.revision(), &result) {
        return Ok(None);
    }
    let complete = status == "completed" && result.complete;
    let partial_findings = matches!(status.as_str(), "completed" | "failed")
        && !result.complete
        && !result.findings.is_empty();
    if (!complete && !partial_findings)
        || !crate::skill_opportunities_worker::publication_has_assessed_coverage(&result)
    {
        return Ok(None);
    }

    let mut findings = Vec::with_capacity(result.findings.len());
    for opportunity in &result.findings {
        let comparison = &opportunity.comparison;
        if comparison.inventory_revision != snapshot.revision()
            || !snapshot
                .skills()
                .iter()
                .any(|skill| comparison.skill.matches_definition(skill))
        {
            return Ok(None);
        }
        let Some(finding) = Finding::skill_opportunity(evidence, opportunity) else {
            return Ok(None);
        };
        findings.push(finding);
    }
    if !complete && findings.is_empty() {
        return Ok(None);
    }
    Ok(Some(findings))
}

pub(super) fn current_skill_snapshot(
    agent: &str,
    workspace_candidate: Option<PathBuf>,
    home: &Path,
) -> Option<SkillOpportunitySnapshot> {
    let agent = AgentKind::from_slug(agent)?;
    let context = ConfigContext::native(agent, home, workspace_candidate);
    skill_opportunity_snapshot(&context).ok()
}

pub(super) fn publication_revisions_match(
    json: &str,
    input_revision: &str,
    inventory_revision: &str,
    result: &SkillOpportunitiesResult,
) -> bool {
    #[derive(serde::Deserialize)]
    struct Revisions {
        input_revision: String,
        inventory_revision: String,
        use_revision: String,
    }
    let Ok(saved) = serde_json::from_str::<Revisions>(json) else {
        return false;
    };
    saved.input_revision == input_revision
        && saved.inventory_revision == inventory_revision
        && !saved.use_revision.is_empty()
        && result.decisions.iter().all(|decision| {
            decision.comparison.inventory_revision == saved.inventory_revision
                && decision.comparison.use_revision == saved.use_revision
        })
        && result.findings.iter().all(|finding| {
            finding.comparison.inventory_revision == saved.inventory_revision
                && finding.comparison.use_revision == saved.use_revision
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use antiburn_local::analysis::jev::{JevAnswer, JevCheck, JevUsage, JevWorkItemResult};

    fn assert_current_skill_report(
        store: &crate::store::Store,
        candidate: &crate::store::BurnCheckCandidate,
        home: &Path,
        workspace: Option<PathBuf>,
        recorded_skill: Option<&str>,
    ) {
        let agent = AgentKind::from_slug(&candidate.session.key.agent).unwrap();
        let root = home.join(match agent {
            AgentKind::Claude => ".claude/skills",
            AgentKind::Codex => ".codex/skills",
            AgentKind::OpenCode => ".config/opencode/skills",
            AgentKind::Pi => ".pi/agent/skills",
            _ => unreachable!(),
        });
        for name in recorded_skill
            .into_iter()
            .chain(["unused-review", "future-review"])
        {
            let path = root.join(name).join("SKILL.md");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let text = if name == "future-review" {
                "# Review API boundaries\nCheck parser inputs and add boundary tests.\n".repeat(600)
            } else {
                format!("---\nname: {name}\ndescription: Review API boundaries and tests.\n---\n")
            };
            std::fs::write(&path, text).unwrap();
        }
        if agent == AgentKind::Codex {
            std::fs::write(
                home.join(".codex/config.toml"),
                format!(
                    "[projects.{}]\ntrust_level = \"trusted\"\n",
                    serde_json::to_string(
                        workspace
                            .as_ref()
                            .unwrap()
                            .canonicalize()
                            .unwrap()
                            .to_str()
                            .unwrap()
                    )
                    .unwrap()
                ),
            )
            .unwrap();
        }
        for (name, text) in [
            ("malformed", "---\nname: [\n---\n"),
            (
                "unsupported",
                "---\nname: unsupported\ndescription: 42\n---\n",
            ),
        ] {
            let path = root.join(name).join("SKILL.md");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let context = ConfigContext::native(agent, home, workspace.clone());
        let snapshot = store
            .load_smart_check_inputs(
                &candidate.session.key,
                candidate.published_fence,
                candidate.source_generation,
                crate::smart_check_inputs::DetectorInput::SkillOpportunities,
            )
            .unwrap();
        let inputs = store
            .load_smart_check_skill_inputs(snapshot, &context)
            .unwrap();
        let prepared = crate::skill_opportunities_worker::prepare(
            candidate,
            inputs,
            crate::skill_opportunities_worker::CHECK.evaluator_revision(),
        )
        .unwrap();
        let plan = prepared
            .check
            .prepare(&prepared.check.session_context())
            .unwrap();
        assert!(!plan.work_items.is_empty(), "{agent:?}");
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
                    .map(|question| {
                        let choice = "useful_opportunity";
                        (
                            question.clone(),
                            JevAnswer::Choice {
                                choice: choice.into(),
                                confidence: 1.0,
                                probabilities: std::collections::BTreeMap::from([
                                    ("useful_opportunity".into(), 1.0),
                                    ("no_opportunity".into(), 0.0),
                                    ("uncertain".into(), 0.0),
                                ]),
                            },
                        )
                    })
                    .collect(),
                evidence: plan
                    .shared_context
                    .as_ref()
                    .unwrap()
                    .evidence
                    .iter()
                    .chain(&item.window.evidence)
                    .cloned()
                    .collect(),
                usage: JevUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
            })
            .collect::<Vec<_>>();
        let result = prepared.check.reduce(&plan, &answers, true).unwrap();
        assert!(!result.findings.is_empty(), "{agent:?}");
        assert!(
            crate::skill_opportunities_worker::publication_has_assessed_coverage(&result),
            "{agent:?}"
        );
        let saved_json = |result: &SkillOpportunitiesResult| {
            let mut saved = serde_json::to_value(result).unwrap();
            saved["input_revision"] = serde_json::json!(prepared.durable.input_revision);
            saved["inventory_revision"] = serde_json::json!(prepared.inventory_revision);
            saved["use_revision"] = serde_json::json!(prepared.use_revision);
            saved.to_string()
        };
        let restored: SkillOpportunitiesResult =
            serde_json::from_str(&saved_json(&result)).unwrap();
        assert!(
            crate::skill_opportunities_worker::publication_has_assessed_coverage(&restored),
            "{agent:?}: serialized coverage must remain valid"
        );
        assert!(
            store
                .queue_burn_check_assessment(&prepared.durable, 1000, 180)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(&prepared.durable, 1000, 300, 180)
                .unwrap()
        );
        assert!(
            store
                .complete_burn_check_assessment(&prepared.durable, &saved_json(&result), 1001, 180)
                .unwrap()
        );
        let evidence: SessionEvidence = serde_json::from_str(
            &store
                .evidence(&candidate.session.key)
                .unwrap()
                .unwrap()
                .evidence_json
                .unwrap(),
        )
        .unwrap();
        let current_inventory = skill_opportunity_snapshot(&context).unwrap();
        assert_eq!(
            current_inventory.revision(),
            prepared.inventory_revision,
            "{agent:?}"
        );
        assert!(
            publication_revisions_match(
                &saved_json(&result),
                &prepared.durable.input_revision,
                &current_inventory.revision(),
                &result
            ),
            "{agent:?}"
        );
        assert!(
            current_inventory
                .skills()
                .iter()
                .all(|skill| skill.name != "malformed" && skill.name != "unsupported"),
            "{agent:?}: unsupported siblings must not become definitions"
        );
        assert!(
            result.findings.iter().all(|finding| finding.comparison.limitations.contains(
                &antiburn_local::checks::skill_opportunities::SkillOpportunityLimit::InventoryIncomplete
            )),
            "{agent:?}: admitted findings must retain incomplete inventory limits"
        );
        assert!(
            result
                .findings
                .iter()
                .all(|opportunity| Finding::skill_opportunity(&evidence, opportunity).is_some()),
            "{agent:?}: source-bound findings must pass the engine gate"
        );
        assert!(
            result.findings[0]
                .comparison
                .work
                .iter()
                .any(|work| work.reference.native_record_id.is_some()),
            "{agent:?}: the finding retains native source identity"
        );
        let mut invalid_native_identity = result.findings[0].clone();
        invalid_native_identity.comparison.work[0]
            .reference
            .native_record_id = Some(String::new());
        assert!(Finding::skill_opportunity(&evidence, &invalid_native_identity).is_none());
        let mut unstable = result.findings[0].clone();
        unstable.comparison.work[0].reference.stable = false;
        assert!(Finding::skill_opportunity(&evidence, &unstable).is_none());
        let mut partial_work = result.findings[0].clone();
        partial_work.comparison.absence_assessable = false;
        partial_work.comparison.use_eligibility.absence =
            antiburn_local::checks::skill_opportunities::SkillAbsenceEvidence::Unassessable;
        partial_work.comparison.work_context_assessable = false;
        assert!(Finding::skill_opportunity(&evidence, &partial_work).is_some());
        partial_work.revisions.questions -= 1;
        assert!(Finding::skill_opportunity(&evidence, &partial_work).is_none());
        let read = || {
            skill_opportunity_findings_with_home(
                &store.lock(),
                &evidence,
                IgnoredInstructionSessionIdentity {
                    environment_key: &candidate.session.key.environment_key,
                    agent: &candidate.session.key.agent,
                    session_id: &candidate.session.key.session_id,
                    incarnation: candidate.incarnation,
                    source_generation: candidate.source_generation,
                    source_fingerprint: candidate.source_fingerprint.as_deref(),
                    published_fence: candidate.published_fence,
                },
                workspace.clone(),
                home,
            )
            .unwrap()
        };
        assert_eq!(
            current_skill_snapshot(
                candidate.session.key.agent.as_str(),
                workspace.clone(),
                home
            )
            .unwrap()
            .revision(),
            prepared.inventory_revision,
            "{agent:?}"
        );
        for opportunity in &result.findings {
            assert!(
                current_inventory
                    .skills()
                    .iter()
                    .any(|skill| opportunity.comparison.skill.matches_definition(skill)),
                "{agent:?}: {:?} must bind {:?}",
                opportunity.comparison.skill,
                current_inventory.skills()
            );
        }
        let findings = read()
            .unwrap_or_else(|| panic!("{agent:?}: current report must retain validated findings"));
        assert!(!findings.is_empty(), "{agent:?}");
        assert!(result.findings.iter().any(|finding| finding.comparison.skill.reference.partial && finding.comparison.skill.reference.source == antiburn_local::checks::skill_opportunities::SkillReferenceSource::MarkdownFallback), "{agent:?}: selected fallback ranges must reach the report");
        let review = |json: &str| {
            super::super::progress::published_coverage(
                "skill_opportunities",
                &prepared.durable.input_revision,
                json,
                Some(&current_inventory.revision()),
            )
        };
        assert!(review(&saved_json(&result)).is_some(), "{agent:?}");
        let mut outcomes = result.clone();
        outcomes.decisions[0].outcome =
            antiburn_local::checks::skill_opportunities::SkillOpportunityOutcome::Uncertain;
        let (counts, _) = review(&saved_json(&outcomes)).unwrap();
        assert_eq!(counts.uncertain, 1);
        outcomes.decisions[0].outcome =
            antiburn_local::checks::skill_opportunities::SkillOpportunityOutcome::Unassessed;
        outcomes.decisions[0].judgments = None;
        let (counts, _) = review(&saved_json(&outcomes)).unwrap();
        assert_eq!(counts.uncertain, 0);
        let mut mismatched_use: serde_json::Value =
            serde_json::from_str(&saved_json(&result)).unwrap();
        mismatched_use["use_revision"] = serde_json::json!("different-use");
        assert!(
            review(&mismatched_use.to_string()).is_none(),
            "{agent:?}: saved comparisons must bind the publication use revision"
        );
        assert!(
            antiburn_local::remediation::remediation_prompt(&findings[0])
                .unwrap()
                .as_str()
                .contains("does not verify past work")
        );
        let mut partial = result.clone();
        partial.complete = false;
        partial.coverage.not_selected_items = 1;
        store.lock().execute("UPDATE burn_check_assessment SET result_json = ?1, status = 'failed', last_error_category = 'sampling_incomplete' WHERE check_id = 'skill_opportunities'", [saved_json(&partial)]).unwrap();
        assert!(read().is_some(), "{agent:?}");
        partial.coverage.not_selected_items = 0;
        store.lock().execute("UPDATE burn_check_assessment SET result_json = ?1 WHERE check_id = 'skill_opportunities'", [saved_json(&partial)]).unwrap();
        assert!(read().is_some(), "{agent:?}");
        let sibling_failure = prepared
            .check
            .reduce(&plan, &answers[..answers.len() - 1], false)
            .unwrap();
        assert!(!sibling_failure.complete);
        assert!(!sibling_failure.findings.is_empty());
        assert!(sibling_failure.decisions.iter().any(|decision| decision.outcome == antiburn_local::checks::skill_opportunities::SkillOpportunityOutcome::Unassessed));
        store.lock().execute("UPDATE burn_check_assessment SET result_json = ?1 WHERE check_id = 'skill_opportunities'", [saved_json(&sibling_failure)]).unwrap();
        store.lock().execute("UPDATE burn_check_assessment SET last_error_category = 'provider_error' WHERE check_id = 'skill_opportunities'", []).unwrap();
        assert!(
            read().is_some(),
            "{agent:?}: a failed sibling does not erase validated positives"
        );
        let mut no_positive = sibling_failure.clone();
        no_positive.findings.clear();
        for decision in &mut no_positive.decisions {
            if decision.outcome
                == antiburn_local::checks::skill_opportunities::SkillOpportunityOutcome::Advisory
            {
                decision.outcome = antiburn_local::checks::skill_opportunities::SkillOpportunityOutcome::NoOpportunity;
            }
        }
        store.lock().execute("UPDATE burn_check_assessment SET result_json = ?1 WHERE check_id = 'skill_opportunities'", [saved_json(&no_positive)]).unwrap();
        assert!(
            read().is_none(),
            "{agent:?}: a failed assessment cannot become clean"
        );
        let mut incomplete = result.clone();
        incomplete.coverage.selected_items = 0;
        store.lock().execute("UPDATE burn_check_assessment SET result_json = ?1, status = 'completed', last_error_category = NULL WHERE check_id = 'skill_opportunities'", [saved_json(&incomplete)]).unwrap();
        assert!(read().is_none(), "{agent:?}");
        store.lock().execute("UPDATE burn_check_assessment SET result_json = ?1, status = 'failed', last_error_category = 'provider_error' WHERE check_id = 'skill_opportunities'", [saved_json(&result)]).unwrap();
        assert_eq!(read().is_some(), !result.complete, "{agent:?}");
        store.lock().execute("UPDATE burn_check_assessment SET status = 'completed', last_error_category = NULL WHERE check_id = 'skill_opportunities'", []).unwrap();
        assert!(read().is_some(), "{agent:?}");
        store.lock().execute("UPDATE burn_check_assessment SET result_revision = 'stale' WHERE check_id = 'skill_opportunities'", []).unwrap();
        assert!(read().is_none(), "{agent:?}");
        store.lock().execute("UPDATE burn_check_assessment SET result_revision = input_revision WHERE check_id = 'skill_opportunities'", []).unwrap();
        std::fs::write(
            root.join("unused-review/SKILL.md"),
            "---\nname: unused-review\ndescription: Review a different procedure.\n---\n",
        )
        .unwrap();
        assert!(read().is_none(), "{agent:?}");
    }

    #[test]
    fn native_skill_reports_bind_current_inventory_status_revisions_and_coverage() {
        use crate::scope_creep_worker::tests::native_sources;
        for (agent, session, format, records) in native_sources::sources() {
            let directory = tempfile::tempdir().unwrap();
            let home = directory.path().join("home");
            let workspace = directory.path().join("workspace");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(&workspace).unwrap();
            let records = native_sources::records_with_work(agent, records)
                .lines()
                .enumerate()
                .map(|(index, line)| {
                    let mut row: serde_json::Value = serde_json::from_str(line).unwrap();
                    row["timestamp"] = serde_json::json!(format!("2099-01-01T00:00:{index:02}Z"));
                    row.to_string()
                })
                .collect::<Vec<_>>()
                .join("\n");
            let store = crate::store::Store::open(directory.path()).unwrap();
            for detector in [DetectorId::SkillOpportunities, DetectorId::ScopeCreep] {
                store.set_check_enabled(detector, true).unwrap();
            }
            store
                .capture_burn_check_boundaries(
                    &[crate::scope_creep_worker::CHECK_ID, "skill_opportunities"],
                    0,
                )
                .unwrap();
            let candidate =
                native_sources::publish(&store, agent, session, format, &records, &workspace);
            let recorded_skill = match agent {
                "codex" => "verify",
                "claude-code" => "api-review",
                "pi" => "boundary-review",
                _ => unreachable!(),
            };
            assert_current_skill_report(
                &store,
                &candidate,
                &home,
                Some(workspace),
                Some(recorded_skill),
            );
        }
    }

    #[test]
    fn opencode_skill_report_uses_the_same_publication_gate() {
        let fixture = crate::scope_creep_worker::tests::NativeFixture::new(1);
        fixture
            .store
            .set_check_enabled(DetectorId::SkillOpportunities, true)
            .unwrap();
        let source =
            rusqlite::Connection::open(fixture.directory.path().join("opencode.db")).unwrap();
        source.execute_batch("UPDATE message SET time_created = time_created + 4102444800000, time_updated = time_updated + 4102444800000; UPDATE part SET time_created = time_created + 4102444800000, time_updated = time_updated + 4102444800000;").unwrap();
        fixture
            .store
            .capture_burn_check_boundaries(&["skill_opportunities"], 0)
            .unwrap();
        let candidate = fixture.publish();
        let home = fixture.directory.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        assert_current_skill_report(&fixture.store, &candidate, &home, None, None);
    }

    #[test]
    fn publication_revisions_require_current_inventory_and_bound_use_even_without_findings() {
        let result = SkillOpportunitiesResult {
            findings: Vec::new(),
            decisions: Vec::new(),
            coverage: Default::default(),
            complete: true,
        };
        let json =
            r#"{"input_revision":"input","inventory_revision":"inventory","use_revision":"use"}"#;
        assert!(publication_revisions_match(
            json,
            "input",
            "inventory",
            &result
        ));
        assert!(!publication_revisions_match(
            json,
            "changed-input",
            "inventory",
            &result
        ));
        assert!(!publication_revisions_match(
            json,
            "input",
            "changed-inventory",
            &result
        ));
        assert!(!publication_revisions_match(
            "{}",
            "input",
            "inventory",
            &result
        ));
        assert!(!publication_revisions_match(
            r#"{"input_revision":"input","inventory_revision":"inventory","use_revision":""}"#,
            "input",
            "inventory",
            &result,
        ));
    }
}
