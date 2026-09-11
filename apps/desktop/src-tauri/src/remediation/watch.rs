use super::*;

pub(crate) fn evaluate_dirty_remediation(
    data_dir: &Path,
    store: &Store,
    record: &RemediationRecord,
    now: i64,
) -> Result<bool> {
    let definition = parse_watch_definition(&record.definition_json)?;
    validate_envelope_version(&record.result_json, "remediation result")?;
    if !remediation_policy_is_current(&definition) {
        return store.replace_remediation_result(
            &record.remediation_id,
            record.dirty_revision,
            &RemediationResult {
                state: record.state,
                result_json: json!({"version": 1, "verification": {"status": "verificationUnavailable"}, "savings": {"status": "unavailable"}}).to_string(),
                evaluated_at_epoch: now,
                transition_at_ms: None,
            },
        );
    }
    let Some(detector) = DetectorId::from_key(&definition.detector) else {
        return Ok(false);
    };
    if !watch_verification_available(&definition, &record.scope_kind, &record.agent, detector) {
        return store.replace_remediation_result_with_contribution(
            &record.remediation_id,
            record.dirty_revision,
            &RemediationResult {
                state: record.state,
                result_json: json!({"version": 1, "verification": {"status": "verificationUnavailable"}, "savings": {"status": "unavailable"}}).to_string(),
                evaluated_at_epoch: now,
                transition_at_ms: None,
            },
            None,
        );
    }
    if definition.old_model.is_none() {
        return evaluate_generic_remediation(data_dir, store, record, &definition, detector, now);
    }
    let (Some(old_model), Some(replacement), Some(boundary_ms), Some(_)) = (
        definition.old_model.as_ref(),
        definition.replacement.as_ref(),
        record.effective_boundary_ms,
        definition.physical_target_key.as_ref(),
    ) else {
        let result = RemediationResult {
            state: record.state,
            result_json: json!({"version": 1, "verification": {"status": "verificationUnavailable"}, "savings": {"status": "unavailable"}}).to_string(),
            evaluated_at_epoch: now,
            transition_at_ms: None,
        };
        return store.replace_remediation_result(
            &record.remediation_id,
            record.dirty_revision,
            &result,
        );
    };
    let target = OldModelVerificationTarget {
        scope: record.scope_key.clone(),
        provider: definition.provider.clone(),
        api: definition.api.clone(),
        old_model: old_model.clone(),
        replacement: replacement.clone(),
    };
    let terminal = matches!(
        record.state,
        RemediationState::Fixed | RemediationState::Recurred
    );
    let display_snapshot = terminal
        .then(|| store.remediation_display_snapshot(&record.remediation_id))
        .transpose()?
        .flatten();
    let fixed_at_ms = terminal.then(|| {
        display_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.verified_boundary_ms)
            .or_else(|| stored_observed_at_ms(&record.result_json))
            .unwrap_or(boundary_ms)
    });
    let snapshot = insights_report::old_model_remediation_evidence(
        data_dir,
        record,
        &definition,
        boundary_ms,
        fixed_at_ms,
    )?;
    let stage = if terminal {
        VerificationStage::Fixed
    } else {
        VerificationStage::Watching
    };
    let verification_boundary = fixed_at_ms.unwrap_or(boundary_ms);
    let verification = verify_old_model(
        &target,
        stage,
        verification_boundary,
        &snapshot.observations,
    );
    if terminal && verification.outcome != VerificationOutcome::Recurred {
        let proof = verify_old_model(
            &target,
            VerificationStage::Watching,
            boundary_ms,
            &snapshot.observations,
        );
        match proof.outcome {
            VerificationOutcome::Unknown(_) => {
                return store.mark_remediation_evaluated(
                    &record.remediation_id,
                    record.dirty_revision,
                    now,
                );
            }
            VerificationOutcome::StillUnresolved => {
                if record.state == RemediationState::Recurred {
                    return store.mark_remediation_evaluated(
                        &record.remediation_id,
                        record.dirty_revision,
                        now,
                    );
                }
                return revoke_terminal_proof(store, record, now);
            }
            VerificationOutcome::Fixed | VerificationOutcome::Recurred => {}
        }
    }
    if record.state == RemediationState::Recurred
        && verification.outcome != VerificationOutcome::Recurred
    {
        return store.mark_remediation_evaluated(
            &record.remediation_id,
            record.dirty_revision,
            now,
        );
    }
    let state = match verification.outcome {
        VerificationOutcome::Fixed => RemediationState::Fixed,
        VerificationOutcome::Recurred => RemediationState::Recurred,
        _ => record.state,
    };
    let savings = old_model_savings(
        &definition,
        boundary_ms,
        snapshot.measured_through_ms,
        snapshot.recurrence_ms,
        snapshot.replacement_tokens,
        snapshot.token_overflow,
    );
    let verification_status =
        if terminal && !matches!(verification.outcome, VerificationOutcome::Recurred) {
            VerificationStatus::Fixed {
                method_revision: verification.method_revision,
                evidence_revision: snapshot.evidence_revision.clone(),
            }
        } else {
            match verification.outcome {
                VerificationOutcome::Fixed => VerificationStatus::Fixed {
                    method_revision: verification.method_revision,
                    evidence_revision: snapshot.evidence_revision,
                },
                VerificationOutcome::StillUnresolved => VerificationStatus::StillUnresolved {
                    method_revision: verification.method_revision,
                    evidence_revision: snapshot.evidence_revision,
                },
                VerificationOutcome::Recurred => VerificationStatus::Recurred {
                    method_revision: verification.method_revision,
                    evidence_revision: snapshot.evidence_revision,
                },
                VerificationOutcome::Unknown(
                    VerificationUnknownReason::MissingPostBoundaryEvidence,
                ) => VerificationStatus::Watching {
                    reason: Some(VerificationReason::MissingPostBoundaryEvidence),
                    method_revision: Some(verification.method_revision),
                    evidence_revision: Some(snapshot.evidence_revision),
                },
                VerificationOutcome::Unknown(VerificationUnknownReason::UnsupportedEvidence) => {
                    VerificationStatus::VerificationUnavailable
                }
            }
        };
    let stored_fixed_at_ms =
        if terminal && !matches!(verification.outcome, VerificationOutcome::Recurred) {
            fixed_at_ms
        } else {
            verification.observed_at_ms
        };
    let result = RemediationResult {
        state,
        result_json: json!({
            "version": 1,
            "verification": verification_status,
            "savings": savings,
            "observedAtMs": stored_fixed_at_ms,
        })
        .to_string(),
        evaluated_at_epoch: verification
            .observed_at_ms
            .map_or(now, |value| value.saturating_div(1_000))
            .max(record.created_at_epoch),
        transition_at_ms: verification.observed_at_ms,
    };
    let contribution = evaluation_contribution(
        store,
        record,
        &savings,
        state,
        snapshot
            .recurrence_ms
            .or(snapshot.measured_through_ms)
            .or(stored_fixed_at_ms),
        now.saturating_mul(1_000),
    )?;
    let updated = store.replace_remediation_result_with_contribution(
        &record.remediation_id,
        record.dirty_revision,
        &result,
        contribution.as_ref(),
    )?;
    if updated {
        update_display_boundaries(
            store,
            &record.remediation_id,
            (state == RemediationState::Fixed || state == RemediationState::Recurred)
                .then_some(stored_fixed_at_ms)
                .flatten(),
            (state == RemediationState::Recurred)
                .then_some(snapshot.recurrence_ms)
                .flatten(),
        )?;
    }
    Ok(updated)
}

fn evaluate_generic_remediation(
    data_dir: &Path,
    store: &Store,
    record: &RemediationRecord,
    definition: &WatchDefinition,
    detector: DetectorId,
    now: i64,
) -> Result<bool> {
    if definition.catalog_revision
        != Some(antiburn_local::insights::ReportCatalogs::default().revision)
    {
        return store.replace_remediation_result(
            &record.remediation_id,
            record.dirty_revision,
            &RemediationResult {
                state: record.state,
                result_json: json!({"version": 1, "verification": {"status": "verificationUnavailable"}, "savings": {"status": "unavailable"}}).to_string(),
                evaluated_at_epoch: now,
                transition_at_ms: None,
            },
        );
    }
    let boundary_ms = record.effective_boundary_ms.unwrap_or(0);
    let terminal = matches!(
        record.state,
        RemediationState::Fixed | RemediationState::Recurred
    );
    let display_snapshot = terminal
        .then(|| store.remediation_display_snapshot(&record.remediation_id))
        .transpose()?
        .flatten();
    let fixed_at_ms = terminal.then(|| {
        display_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.verified_boundary_ms)
            .or_else(|| stored_observed_at_ms(&record.result_json))
            .unwrap_or(boundary_ms)
    });
    let stage = if terminal {
        VerificationStage::Fixed
    } else {
        VerificationStage::Watching
    };
    let verification_boundary = fixed_at_ms.unwrap_or(boundary_ms);
    let page = insights_report::remediation_assessments(
        data_dir,
        &record.environment_key,
        &record.agent,
        detector,
        boundary_ms,
    )?;
    let mut assessments = Vec::new();
    for assessment in page.assessments {
        if !session_starts_after_boundary(assessment.started_at_ms, boundary_ms) {
            continue;
        }
        if assessment.source_format != definition.source_format.value() {
            continue;
        }
        let project_scope = assessment
            .workspace_candidate
            .as_deref()
            .and_then(|path| trusted_workspace(store, path).ok().flatten())
            .and_then(|path| hashed_workspace_key(store, &path).ok());
        let session_scope = store
            .provider_account_secret()
            .ok()
            .map(|secret| session_scope_key(&secret, &record.agent, &assessment.session_id));
        let scope_matches = if definition.config_setting.as_deref() == Some("reasoning")
            && let Some(target) = definition.physical_target_key.as_deref()
        {
            assessment.effective_reasoning_target_hash.as_deref() == Some(target)
                && assessment.effective_reasoning_scope.as_deref()
                    == Some(record.scope_kind.as_str())
        } else {
            scope_identity_matches(
                &record.scope_kind,
                &record.scope_key,
                project_scope.as_deref(),
                session_scope.as_deref(),
            )
        };
        if !scope_matches {
            continue;
        }
        let mut target_present = false;
        if let FindingAssessment::Findings(findings) = &assessment.assessment {
            for (index, finding) in findings.iter().enumerate() {
                let identity = finding.canonical_identity(&record.scope_key);
                let present = identity == definition.canonical_identity;
                target_present |= present;
                assessments.push(TargetAssessment {
                    observed_at_ms: assessment
                        .finding_observed_at_ms
                        .get(index)
                        .copied()
                        .flatten()
                        .unwrap_or(assessment.observed_at_ms),
                    identity,
                    target_present: present,
                    assessment: assessment.assessment.clone(),
                });
            }
        }
        if !target_present {
            let positive_resolution =
                positive_control_resolution(detector, definition, &assessment);
            let positive_control_only = matches!(
                detector,
                DetectorId::ModelOverthinking
                    | DetectorId::OverpoweredSubagents
                    | DetectorId::OveruseOfFastMode
            );
            assessments.push(TargetAssessment {
                observed_at_ms: positive_resolution.unwrap_or(assessment.observed_at_ms),
                identity: definition.canonical_identity.clone(),
                target_present: false,
                assessment: if page.truncated
                    || (positive_control_only && positive_resolution.is_none())
                {
                    FindingAssessment::Unavailable(FindingUnavailableReason::IncompleteEvidence)
                } else {
                    assessment.assessment
                },
            });
        }
    }
    let verification = verify_prompt_watch(
        detector,
        definition.source_format.value(),
        &definition.canonical_identity,
        stage,
        verification_boundary,
        &assessments,
    );
    if terminal && verification.outcome != VerificationOutcome::Recurred {
        let proof = verify_prompt_watch(
            detector,
            definition.source_format.value(),
            &definition.canonical_identity,
            VerificationStage::Watching,
            boundary_ms,
            &assessments,
        );
        match proof.outcome {
            VerificationOutcome::Unknown(_) => {
                return store.mark_remediation_evaluated(
                    &record.remediation_id,
                    record.dirty_revision,
                    now,
                );
            }
            VerificationOutcome::StillUnresolved => {
                if record.state == RemediationState::Recurred {
                    return store.mark_remediation_evaluated(
                        &record.remediation_id,
                        record.dirty_revision,
                        now,
                    );
                }
                return revoke_terminal_proof(store, record, now);
            }
            VerificationOutcome::Fixed | VerificationOutcome::Recurred => {}
        }
    }
    if record.state == RemediationState::Recurred
        && verification.outcome != VerificationOutcome::Recurred
    {
        return store.mark_remediation_evaluated(
            &record.remediation_id,
            record.dirty_revision,
            now,
        );
    }
    let state = match verification.outcome {
        VerificationOutcome::Fixed => RemediationState::Fixed,
        VerificationOutcome::Recurred => RemediationState::Recurred,
        _ => record.state,
    };
    let evidence_revision = format!(
        "evidence-{}-{}",
        ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION
    );
    let verification_status =
        if terminal && !matches!(verification.outcome, VerificationOutcome::Recurred) {
            VerificationStatus::Fixed {
                method_revision: verification.method_revision,
                evidence_revision,
            }
        } else {
            verification_status(&verification, evidence_revision)
        };
    let stored_fixed_at_ms =
        if terminal && !matches!(verification.outcome, VerificationOutcome::Recurred) {
            fixed_at_ms
        } else {
            verification.observed_at_ms
        };
    let savings = SavingsStatus::Unavailable;
    let result = RemediationResult {
        state,
        result_json: json!({
            "version": 1,
            "verification": verification_status,
            "savings": savings,
            "observedAtMs": stored_fixed_at_ms,
        })
        .to_string(),
        evaluated_at_epoch: verification
            .observed_at_ms
            .map_or(now, |value| value.saturating_div(1_000)),
        transition_at_ms: verification.observed_at_ms,
    };
    let contribution = evaluation_contribution(
        store,
        record,
        &savings,
        state,
        verification.observed_at_ms.or(stored_fixed_at_ms),
        now.saturating_mul(1_000),
    )?;
    let updated = store.replace_remediation_result_with_contribution(
        &record.remediation_id,
        record.dirty_revision,
        &result,
        contribution.as_ref(),
    )?;
    if updated {
        update_display_boundaries(
            store,
            &record.remediation_id,
            (state == RemediationState::Fixed || state == RemediationState::Recurred)
                .then_some(stored_fixed_at_ms)
                .flatten(),
            (state == RemediationState::Recurred)
                .then_some(verification.observed_at_ms)
                .flatten(),
        )?;
    }
    Ok(updated)
}

fn revoke_terminal_proof(store: &Store, record: &RemediationRecord, now: i64) -> Result<bool> {
    store.replace_remediation_result_with_contribution(
        &record.remediation_id,
        record.dirty_revision,
        &RemediationResult {
            state: record.state,
            result_json: json!({
                "version": 1,
                "verification": {"status": "verificationUnavailable"},
                "savings": {"status": "unavailable"},
            })
            .to_string(),
            evaluated_at_epoch: now,
            transition_at_ms: None,
        },
        None,
    )
}

fn evaluation_contribution(
    store: &Store,
    record: &RemediationRecord,
    savings: &SavingsStatus,
    state: RemediationState,
    ends_at_ms: Option<i64>,
    updated_at_ms: i64,
) -> Result<Option<RemediationContribution>> {
    if !matches!(state, RemediationState::Fixed | RemediationState::Recurred) {
        return Ok(None);
    }
    if matches!(
        savings,
        SavingsStatus::Unknown {
            reason: SavingsUnknownReason::MissingEvidence,
            ..
        }
    ) {
        return Ok(None);
    }
    let Some(snapshot) = store.remediation_display_snapshot(&record.remediation_id)? else {
        return Ok(None);
    };
    let display = parse_display_snapshot(&snapshot.display_snapshot_json)?;
    let definition = parse_watch_definition(&record.definition_json)?;
    let api_equivalent_cost_avoided_usd = match savings {
        SavingsStatus::Known {
            api_equivalent_cost_avoided_usd,
            ..
        } => Some(*api_equivalent_cost_avoided_usd),
        _ => None,
    };
    let facts = AggregateSavings {
        version: 1,
        token_savings: None,
        api_equivalent_cost_avoided_usd,
        improvement_count: Some(1),
        method: display.display.estimate_method,
    };
    let ends_at_ms = ends_at_ms
        .unwrap_or(
            snapshot
                .verified_boundary_ms
                .unwrap_or(snapshot.effective_boundary_ms),
        )
        .max(snapshot.effective_boundary_ms);
    Ok(Some(RemediationContribution {
        owner_key: format!("allocation:v1:{}", record.remediation_id),
        remediation_id: record.remediation_id.clone(),
        detector_id: definition.detector,
        origin: snapshot.origin,
        display_snapshot_json: snapshot.display_snapshot_json,
        facts_json: serde_json::to_string(&facts)?,
        starts_at_ms: snapshot.effective_boundary_ms,
        ends_at_ms,
        updated_at_ms: updated_at_ms.max(ends_at_ms),
    }))
}

pub(super) fn positive_control_resolution(
    detector: DetectorId,
    definition: &WatchDefinition,
    assessment: &insights_report::CurrentDetectorAssessment,
) -> Option<i64> {
    if assessment.assessment != FindingAssessment::Clean {
        return None;
    }
    assessment
        .control_observations
        .iter()
        .filter(|observation| {
            observation.provider == definition.provider
                && observation.api == definition.api
                && Some(observation.model.as_str()) == definition.target_model.as_deref()
                && match detector {
                    DetectorId::ModelOverthinking => observation
                        .effort
                        .as_deref()
                        .zip(definition.target_control.as_deref())
                        .is_some_and(|(observed, baseline)| observed != baseline),
                    DetectorId::OveruseOfFastMode => {
                        observation
                            .speed
                            .as_deref()
                            .is_some_and(|speed| speed != "fast")
                            && observation.turns.delegated > 0
                    }
                    _ => false,
                }
        })
        .map(|observation| observation.last_ts_ms)
        .max()
}

fn update_display_boundaries(
    store: &Store,
    remediation_id: &str,
    verified_boundary_ms: Option<i64>,
    recurred_boundary_ms: Option<i64>,
) -> Result<()> {
    let Some(mut snapshot) = store.remediation_display_snapshot(remediation_id)? else {
        return Ok(());
    };
    snapshot.verified_boundary_ms = snapshot.verified_boundary_ms.or(verified_boundary_ms);
    snapshot.recurred_boundary_ms = snapshot.recurred_boundary_ms.or(recurred_boundary_ms);
    store.upsert_remediation_display_snapshot(&snapshot)?;
    Ok(())
}

fn stored_observed_at_ms(result_json: &str) -> Option<i64> {
    stored_result(result_json).ok()?.observed_at_ms
}

pub(super) fn scope_identity_matches(
    kind: &str,
    expected: &str,
    project: Option<&str>,
    session: Option<&str>,
) -> bool {
    match kind {
        "project" => project == Some(expected),
        "session" => session == Some(expected),
        _ => false,
    }
}

pub(super) const fn session_starts_after_boundary(started_at_ms: i64, boundary_ms: i64) -> bool {
    started_at_ms > boundary_ms
}

fn verification_status(
    verification: &antiburn_local::remediation::VerificationResult,
    evidence_revision: String,
) -> VerificationStatus {
    match verification.outcome {
        VerificationOutcome::Fixed => VerificationStatus::Fixed {
            method_revision: verification.method_revision,
            evidence_revision,
        },
        VerificationOutcome::StillUnresolved => VerificationStatus::StillUnresolved {
            method_revision: verification.method_revision,
            evidence_revision,
        },
        VerificationOutcome::Recurred => VerificationStatus::Recurred {
            method_revision: verification.method_revision,
            evidence_revision,
        },
        VerificationOutcome::Unknown(VerificationUnknownReason::MissingPostBoundaryEvidence) => {
            VerificationStatus::Watching {
                reason: Some(VerificationReason::MissingPostBoundaryEvidence),
                method_revision: Some(verification.method_revision),
                evidence_revision: Some(evidence_revision),
            }
        }
        VerificationOutcome::Unknown(VerificationUnknownReason::UnsupportedEvidence) => {
            VerificationStatus::VerificationUnavailable
        }
    }
}

fn old_model_savings(
    definition: &WatchDefinition,
    boundary_ms: i64,
    measured: Option<i64>,
    recurrence: Option<i64>,
    tokens: Option<antiburn_local::pricing::ModelTokens>,
    token_overflow: bool,
) -> SavingsStatus {
    if token_overflow {
        return SavingsStatus::Unknown {
            reason: SavingsUnknownReason::ArithmeticOverflow,
            method_revision: SAVINGS_METHOD_REVISION,
        };
    }
    let estimate = measured
        .map(|measured_through_ms| {
            estimate_old_model_savings(&OldModelSavingsInput {
                interval: SavingsInterval {
                    boundary_ms,
                    measured_through_ms: recurrence.unwrap_or(measured_through_ms),
                    recurrence_ms: recurrence,
                },
                tokens,
                old_pricing: definition.old_pricing.clone(),
                replacement_pricing: definition.replacement_pricing.clone(),
                pricing_revision: definition.pricing_revision.clone(),
            })
        })
        .unwrap_or(OldModelSavingsEstimate::Unknown(
            OldModelSavingsUnknownReason::MissingEvidence,
        ));
    match estimate {
        OldModelSavingsEstimate::Known(value) => SavingsStatus::Known {
            method: SavingsMethod::OldModelPriceDifference,
            method_revision: value.method_revision,
            pricing_revision: value.pricing_revision,
            api_equivalent_cost_avoided_usd: value.api_equivalent_cost_avoided_usd,
            measured_through_ms: recurrence
                .or(measured)
                .expect("known savings has an interval"),
            recurrence_ms: recurrence,
        },
        OldModelSavingsEstimate::Unknown(reason) => SavingsStatus::Unknown {
            reason: match reason {
                OldModelSavingsUnknownReason::MissingRates => SavingsUnknownReason::MissingRates,
                OldModelSavingsUnknownReason::MissingEvidence => {
                    SavingsUnknownReason::MissingEvidence
                }
                OldModelSavingsUnknownReason::MissingRevision => {
                    SavingsUnknownReason::MissingRevision
                }
                OldModelSavingsUnknownReason::ArithmeticOverflow => {
                    SavingsUnknownReason::ArithmeticOverflow
                }
            },
            method_revision: SAVINGS_METHOD_REVISION,
        },
    }
}
