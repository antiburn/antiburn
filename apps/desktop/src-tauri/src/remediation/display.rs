use super::*;
pub(super) use antiburn_local::remediation::sanitize_display_value as safe_display_value;

pub(super) fn stable_finding_id(target: &CachedTarget) -> String {
    target.target_key.clone()
}

pub(super) fn scope_display(scope: &str) -> BurnCheckScopeKind {
    match scope {
        "global" => BurnCheckScopeKind::Global,
        "project" => BurnCheckScopeKind::Project,
        "worker" => BurnCheckScopeKind::Worker,
        _ => BurnCheckScopeKind::Session,
    }
}

pub(super) fn sample_sessions(findings: &[CurrentFinding]) -> Vec<BurnCheckSampleSession> {
    let mut seen = BTreeSet::new();
    findings
        .iter()
        .filter(|finding| {
            seen.insert((
                finding.environment_key.clone(),
                finding.agent.clone(),
                finding.session_id.clone(),
            ))
        })
        .take(3)
        .map(|finding| BurnCheckSampleSession {
            environment_key: finding.environment_key.clone(),
            agent: finding.agent.clone(),
            session_id: finding.session_id.clone(),
            observed_at_ms: finding.observed_at_ms,
        })
        .collect()
}

pub(super) fn burn_check_display_facts(target: &CachedTarget) -> BurnCheckDisplayFacts {
    let finding = &target.findings[0].finding;
    let observed_at_ms = target.findings[0].observed_at_ms;
    let (resource_kind, resource_identity, current_value, replacement_value) = match finding.cause()
    {
        FindingCause::SessionsOverDepth { .. } => {
            (BurnCheckResourceKind::Session, None, None, None)
        }
        FindingCause::ModelOverthinking {
            model, reasoning, ..
        } => (
            BurnCheckResourceKind::Reasoning,
            safe_display_value(model),
            safe_display_value(reasoning),
            target
                .config
                .as_ref()
                .filter(|config| config.operation.setting == ConfigSetting::Reasoning)
                .and_then(|config| safe_display_value(&config.operation.proposed_value)),
        ),
        FindingCause::OverpoweredSubagents { worker_model, .. } => (
            BurnCheckResourceKind::Worker,
            safe_display_value(worker_model),
            safe_display_value(worker_model),
            None,
        ),
        FindingCause::UnusedMcpServer { server } => (
            BurnCheckResourceKind::McpServer,
            safe_display_value(server),
            None,
            None,
        ),
        FindingCause::UnusedBuiltInTool { tool, .. } => (
            BurnCheckResourceKind::BuiltInTool,
            safe_display_value(tool),
            None,
            None,
        ),
        FindingCause::UnusedSkill { skill } => (
            BurnCheckResourceKind::Skill,
            safe_display_value(skill),
            None,
            None,
        ),
        FindingCause::OldModelUsage {
            model, replacement, ..
        } => (
            BurnCheckResourceKind::Model,
            safe_display_value(model),
            safe_display_value(model),
            safe_display_value(replacement),
        ),
        FindingCause::OveruseOfFastMode { model, .. } => (
            BurnCheckResourceKind::Speed,
            safe_display_value(model),
            Some("fast".to_owned()),
            None,
        ),
        FindingCause::CacheChurn { model, .. } => (
            BurnCheckResourceKind::Cache,
            safe_display_value(model),
            None,
            None,
        ),
    };
    let (mut quantity, unit) = finding_quantity(finding.cause());
    if target.findings.len() > 1 && quantity.is_some() {
        quantity = target.findings.iter().try_fold(0_u64, |total, finding| {
            let (value, finding_unit) = finding_quantity(finding.finding.cause());
            (finding_unit == unit).then_some(total.saturating_add(value?))
        });
    }
    BurnCheckDisplayFacts {
        resource_kind,
        resource_identity,
        current_value,
        replacement_value,
        scope_kind: if matches!(finding.cause(), FindingCause::OverpoweredSubagents { .. }) {
            BurnCheckScopeKind::Worker
        } else {
            scope_display(&target.scope_kind)
        },
        quantity,
        quantity_unit: unit,
        observation_count: u64::try_from(target.findings.len()).unwrap_or(u64::MAX),
        first_observed_at_ms: target
            .findings
            .iter()
            .map(|finding| finding.observed_at_ms)
            .min()
            .unwrap_or(observed_at_ms),
        last_observed_at_ms: target
            .findings
            .iter()
            .map(|finding| finding.observed_at_ms)
            .max()
            .unwrap_or(observed_at_ms),
        estimate_method: Some(SavingsEstimateMethod::for_detector(finding.detector).into()),
        estimated_opportunity: display_opportunity(&target.findings),
        verification_limit: verification_limit(finding.detector),
    }
}

impl From<SavingsEstimateMethod> for BurnCheckEstimateMethod {
    fn from(value: SavingsEstimateMethod) -> Self {
        match value {
            SavingsEstimateMethod::RepeatedContextAboveDepthCap => {
                Self::RepeatedContextAboveDepthCap
            }
            SavingsEstimateMethod::AssumedOutputReduction => Self::AssumedOutputReduction,
            SavingsEstimateMethod::WorkerModelPriceDifference => Self::WorkerModelPriceDifference,
            SavingsEstimateMethod::McpDefinitionExposure => Self::McpDefinitionExposure,
            SavingsEstimateMethod::BuiltInDefinitionReplication => {
                Self::BuiltInDefinitionReplication
            }
            SavingsEstimateMethod::InjectedSkillDocument => Self::InjectedSkillDocument,
            SavingsEstimateMethod::OldModelPriceDifference => Self::OldModelPriceDifference,
            SavingsEstimateMethod::FastTierPricePremium => Self::FastTierPricePremium,
            SavingsEstimateMethod::CacheRehydrationPriceDifference => {
                Self::CacheRehydrationPriceDifference
            }
        }
    }
}

pub(super) fn display_opportunity(findings: &[CurrentFinding]) -> Option<SavingsValue> {
    let mut total: Option<SavingsValue> = None;
    for finding in findings {
        let value = display_cause_opportunity(finding.finding.cause(), finding.observed_at_ms)?;
        total = Some(match total {
            None => value,
            Some(total) if total.unit == value.unit => {
                let value = total.value + value.value;
                value.is_finite().then_some(SavingsValue {
                    unit: total.unit,
                    value,
                })?
            }
            Some(_) => return None,
        });
    }
    total
}

pub(super) fn display_cause_opportunity(
    cause: &FindingCause,
    observed_at_ms: i64,
) -> Option<SavingsValue> {
    if let FindingCause::SessionsOverDepth {
        limit_tokens,
        requests,
        omitted_requests,
        ..
    } = cause
    {
        if requests.is_empty() || *omitted_requests != Some(0) {
            return None;
        }
        let tokens = requests.iter().try_fold(0_u64, |total, request| {
            total.checked_add(request.value.saturating_sub(*limit_tokens))
        })?;
        return Some(SavingsValue {
            unit: antiburn_local::remediation::SavingsUnit::LiteralInputTokens,
            value: tokens as f64,
        });
    }
    estimate_savings(
        SavingsInterval {
            boundary_ms: observed_at_ms.saturating_sub(1),
            measured_through_ms: observed_at_ms,
            recurrence_ms: None,
        },
        &display_estimate_input(cause),
    )
    .value
    .ok()
}

pub(super) fn verification_limit(detector: DetectorId) -> BurnCheckVerificationLimit {
    match detector {
        DetectorId::ModelOverthinking | DetectorId::OveruseOfFastMode => {
            BurnCheckVerificationLimit::ExactPositiveControlRequired
        }
        DetectorId::OverpoweredSubagents => {
            BurnCheckVerificationLimit::CurrentEvidenceCannotProveFix
        }
        _ => BurnCheckVerificationLimit::FreshEvidenceFromSameSourceAndTarget,
    }
}

pub(super) fn display_estimate_input(cause: &FindingCause) -> SavingsEstimateInput {
    use antiburn_local::remediation::{BuiltInToolTokens, PriceComparisonInput};

    match cause {
        FindingCause::SessionsOverDepth {
            maximum_tokens,
            limit_tokens,
            ..
        } => SavingsEstimateInput::RepeatedContextAboveDepthCap {
            observed_tokens: Some(*maximum_tokens),
            depth_cap_tokens: *limit_tokens,
        },
        FindingCause::ModelOverthinking { .. } => SavingsEstimateInput::AssumedOutputReduction {
            observed_output_tokens: None,
            reduction_basis_points: None,
        },
        FindingCause::OverpoweredSubagents { .. } => {
            SavingsEstimateInput::WorkerModelPriceDifference(PriceComparisonInput {
                tokens: None,
                baseline: None,
                alternative: None,
                pricing_revision: None,
            })
        }
        FindingCause::UnusedMcpServer { .. } => SavingsEstimateInput::McpDefinitionExposure {
            definition_tokens: None,
            compatible_requests: None,
        },
        FindingCause::UnusedBuiltInTool { tokens, .. } => {
            SavingsEstimateInput::BuiltInDefinitionReplication {
                replicated_tokens: match tokens {
                    BuiltInToolTokens::Definition(_) => None,
                    BuiltInToolTokens::Replicated(value) => u64::try_from(*value).ok(),
                },
            }
        }
        FindingCause::UnusedSkill { .. } => SavingsEstimateInput::InjectedSkillDocument {
            document_tokens: None,
            compatible_requests: None,
        },
        FindingCause::OldModelUsage { .. } => {
            SavingsEstimateInput::OldModelPriceDifference(PriceComparisonInput {
                tokens: None,
                baseline: None,
                alternative: None,
                pricing_revision: None,
            })
        }
        FindingCause::OveruseOfFastMode { .. } => {
            SavingsEstimateInput::FastTierPricePremium(PriceComparisonInput {
                tokens: None,
                baseline: None,
                alternative: None,
                pricing_revision: None,
            })
        }
        FindingCause::CacheChurn {
            repeated_tokens, ..
        } => SavingsEstimateInput::CacheRehydrationPriceDifference {
            repeated_paid_tokens: Some(*repeated_tokens),
            paid_input_rate: None,
            cache_read_rate: None,
            pricing_revision: None,
        },
    }
}

pub(super) fn finding_quantity(
    cause: &FindingCause,
) -> (Option<u64>, Option<BurnCheckQuantityUnit>) {
    use antiburn_local::remediation::BuiltInToolTokens;

    match cause {
        FindingCause::SessionsOverDepth { maximum_tokens, .. } => {
            (Some(*maximum_tokens), Some(BurnCheckQuantityUnit::Tokens))
        }
        FindingCause::ModelOverthinking { turns, .. }
        | FindingCause::OldModelUsage { turns, .. } => {
            (Some(*turns), Some(BurnCheckQuantityUnit::Turns))
        }
        FindingCause::OverpoweredSubagents { .. }
        | FindingCause::UnusedMcpServer { .. }
        | FindingCause::UnusedSkill { .. } => (Some(1), Some(BurnCheckQuantityUnit::Resources)),
        FindingCause::UnusedBuiltInTool { tokens, .. } => (
            Some(match tokens {
                BuiltInToolTokens::Definition(value) => *value,
                BuiltInToolTokens::Replicated(value) => u64::try_from(*value).unwrap_or(u64::MAX),
            }),
            Some(BurnCheckQuantityUnit::Tokens),
        ),
        FindingCause::OveruseOfFastMode {
            delegated_turns, ..
        } => (Some(*delegated_turns), Some(BurnCheckQuantityUnit::Turns)),
        FindingCause::CacheChurn {
            repeated_tokens, ..
        } => (Some(*repeated_tokens), Some(BurnCheckQuantityUnit::Tokens)),
    }
}

pub(super) fn prompt_with_evidence_paths(
    base: &str,
    paths: &[String],
    remediation_id: Option<&str>,
) -> Result<String, RemediationUnavailableReason> {
    let suffix = remediation_id
        .map(|id| format!("\n\n{PROMPT_REFERENCE_PREFIX}{id}"))
        .unwrap_or_default();
    if base.len().saturating_add(suffix.len()) > antiburn_local::remediation::MAX_PROMPT_BYTES {
        return Err(RemediationUnavailableReason::PromptSizeLimit);
    }
    let mut prompt = base.to_owned();
    let heading =
        "\n\nRepresentative session evidence (inspect only; not configuration edit targets):";
    let mut included = 0;
    for path in paths.iter().take(3) {
        let Ok(quoted) = serde_json::to_string(path) else {
            continue;
        };
        let line = format!("\n- {quoted}");
        let extra = if included == 0 {
            heading.len().saturating_add(line.len())
        } else {
            line.len()
        };
        if prompt
            .len()
            .saturating_add(extra)
            .saturating_add(suffix.len())
            > antiburn_local::remediation::MAX_PROMPT_BYTES
        {
            continue;
        }
        if included == 0 {
            prompt.push_str(heading);
        }
        prompt.push_str(&line);
        included += 1;
    }
    prompt.push_str(&suffix);
    Ok(prompt)
}
