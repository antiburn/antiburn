use super::*;

struct CurrentPromptTargets {
    targets: Vec<CachedTarget>,
    truncated: bool,
    check_preferences_revision: u64,
}

impl RemediationController {
    pub fn copy_prompt_fix_burn_check_target_current(
        &self,
        store: &Store,
        detector: DetectorId,
        finding_id: &str,
        context: BurnCheckTargetContext,
    ) -> Result<PromptFixResult, ControllerError> {
        let (watches, prompt) = self.copy_current_prompt_selection(
            store,
            detector,
            Some(&[finding_id.to_owned()]),
            context,
        )?;
        let watch = watches
            .into_iter()
            .next()
            .map(|watch| public_watch(store, &watch))
            .transpose()?
            .flatten();
        Ok(PromptFixResult { prompt, watch })
    }

    pub fn copy_prompt_fix_burn_check_targets_current(
        &self,
        store: &Store,
        detector: DetectorId,
        finding_ids: &[String],
        context: BurnCheckTargetContext,
    ) -> Result<CheckPromptFixResult, ControllerError> {
        let (_, prompt) =
            self.copy_current_prompt_selection(store, detector, Some(finding_ids), context)?;
        Ok(CheckPromptFixResult { prompt })
    }

    pub fn copy_prompt_fix_burn_check(
        &self,
        store: &Store,
        detector: DetectorId,
        context: BurnCheckTargetContext,
    ) -> Result<CheckPromptFixResult, ControllerError> {
        let (_, prompt) = self.copy_current_prompt_selection(store, detector, None, context)?;
        Ok(CheckPromptFixResult { prompt })
    }

    fn copy_current_prompt_selection(
        &self,
        store: &Store,
        detector: DetectorId,
        finding_ids: Option<&[String]>,
        context: BurnCheckTargetContext,
    ) -> Result<(Vec<RemediationRecord>, String), ControllerError> {
        if let Some(ids) = finding_ids {
            validate_prompt_selection(ids)?;
        }
        retry_prompt_snapshot(|| {
            let snapshot = self.current_prompt_targets(store, detector, context.clone())?;
            let mut selected = Vec::new();
            let mut missing = false;
            let mut overflow = false;
            if let Some(ids) = finding_ids {
                for id in ids {
                    match snapshot
                        .targets
                        .iter()
                        .find(|target| stable_finding_id(target) == *id)
                    {
                        Some(target) => {
                            remediation_prompt(target.finding())
                                .map_err(ControllerError::PromptUnavailable)?;
                            selected.push(target.clone());
                        }
                        None => missing = true,
                    }
                }
            } else {
                let eligible = snapshot
                    .targets
                    .iter()
                    .filter(|target| remediation_prompt(target.finding()).is_ok())
                    .collect::<Vec<_>>();
                overflow = eligible.len() > MAX_CHECK_PROMPT_TARGETS;
                selected.extend(eligible.into_iter().take(MAX_CHECK_PROMPT_TARGETS).cloned());
                if selected.is_empty() && (!snapshot.targets.is_empty() || !snapshot.truncated) {
                    return Err(ControllerError::CheckPromptUnavailable);
                }
            }
            let fallback = missing || (finding_ids.is_none() && (snapshot.truncated || overflow));
            self.finish_current_prompt(
                store,
                detector,
                if missing { &[] } else { &selected },
                &snapshot,
                fallback,
                missing,
            )
        })
    }

    fn current_prompt_targets(
        &self,
        store: &Store,
        detector: DetectorId,
        context: BurnCheckTargetContext,
    ) -> Result<CurrentPromptTargets, ControllerError> {
        let (enabled, check_preferences_revision) = store
            .check_preferences_snapshot()
            .map_err(|_| ControllerError::PersistenceFailed)?;
        if !enabled.contains(&detector) {
            return Err(ControllerError::CheckPromptUnavailable);
        }
        let home = antiburn_local::paths::home_dir();
        let mut authorities = opencode_current::AuthorityCache::for_listing();
        let (targets, truncated) = if matches!(
            detector,
            DetectorId::UnusedMcpServers
                | DetectorId::UnusedBuiltInTools
                | DetectorId::UnusedSkills
        ) {
            let reduced = self.reduce_resource_report(&context)?;
            let assessment = reduced
                .resources
                .detector(detector)
                .ok_or(ControllerError::Internal)?;
            let targets = assessment
                .targets
                .iter()
                .map(|resource| {
                    let location = opencode_current::resource_authority_eligible(
                        resource,
                        &context.environment_key,
                        home.as_deref(),
                    )
                    .then(|| opencode_current::resource_location(store, resource))
                    .flatten()
                    .filter(|cwd| trusted_workspace(store, cwd).ok().flatten().is_some());
                    let authority = location.as_deref().and_then(|cwd| authorities.get(cwd));
                    self.resolve_resource_target_at_location(
                        store,
                        resource,
                        context.clone(),
                        home.as_deref(),
                        authority.zip(location.as_deref()),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            (targets, assessment.truncated)
        } else {
            let page = insights_report::list_current_findings(
                &self.data_dir,
                CurrentFindingsRequest {
                    environment_key: context.environment_key,
                    window: context.window,
                    detector,
                },
            )
            .map_err(|_| ControllerError::Internal)?;
            let mut grouped: BTreeMap<String, CachedTarget> = BTreeMap::new();
            for finding in page.findings {
                let agent = finding
                    .finding
                    .display()
                    .map_err(|_| ControllerError::Internal)?
                    .agent;
                let (key, target) = if agent == AgentKind::OpenCode {
                    let authority = finding
                        .workspace_candidate()
                        .filter(|cwd| {
                            home.is_some()
                                && opencode_subject(finding.finding.cause()).is_some()
                                && trusted_workspace(store, cwd).ok().flatten().is_some()
                        })
                        .and_then(|cwd| authorities.get(cwd));
                    self.resolve_opencode_target(store, finding, home.as_deref(), authority)?
                } else {
                    self.resolve_target(store, finding, agent, home.as_deref())?
                };
                grouped
                    .entry(key)
                    .and_modify(|entry| {
                        entry.findings.extend(target.findings.clone());
                        match (&mut entry.config, target.config.as_ref()) {
                            (Some(existing), Some(incoming))
                                if existing.operation == incoming.operation
                                    && existing.physical_key == incoming.physical_key =>
                            {
                                existing.additional_contexts.push(incoming.context.clone());
                                existing
                                    .additional_contexts
                                    .extend(incoming.additional_contexts.clone());
                            }
                            (None, None) => {}
                            _ => entry.config = None,
                        }
                    })
                    .or_insert(target);
            }
            let mut targets = grouped.into_values().collect::<Vec<_>>();
            for target in &mut targets {
                target.findings.sort_by(|left, right| {
                    right
                        .observed_at_ms
                        .cmp(&left.observed_at_ms)
                        .then_with(|| left.session_id.cmp(&right.session_id))
                });
            }
            (targets, page.truncated)
        };
        Ok(CurrentPromptTargets {
            targets,
            truncated,
            check_preferences_revision,
        })
    }

    fn finish_current_prompt(
        &self,
        store: &Store,
        detector: DetectorId,
        targets: &[CachedTarget],
        snapshot: &CurrentPromptTargets,
        fallback: bool,
        missing: bool,
    ) -> Result<(Vec<RemediationRecord>, String), ControllerError> {
        let mut base = self.format_current_prompt(store, detector, targets, fallback)?;
        if missing {
            base.push_str("\n\nSelection limit\nThe current accepted evidence does not establish the selected target. Inspect only. Do not infer a current finding, reuse earlier target facts, or apply changes from this request.");
        }
        let now = now_epoch();
        let inputs = targets
            .iter()
            .map(|target| {
                let (remediation, guards) = self.watch_input(
                    target,
                    RemediationState::WaitingForPromptUse,
                    None,
                    now,
                    None,
                )?;
                let display = serde_json::to_string(&StoredDisplaySnapshot {
                    version: 1,
                    finding_id: stable_finding_id(target),
                    display: burn_check_display_facts(target, None),
                })
                .map_err(|_| ControllerError::Internal)?;
                Ok((remediation, guards, display))
            })
            .collect::<Result<Vec<_>, ControllerError>>()?;
        if inputs.is_empty() {
            if !self.check_policy_matches(store, detector, snapshot.check_preferences_revision)? {
                return Err(ControllerError::TargetChanged);
            }
            return Ok((Vec::new(), base));
        }
        let result = store
            .enroll_prompt_remediations_for_enabled_check(
                &inputs,
                detector,
                snapshot.check_preferences_revision,
                |records| {
                    let records = records.iter().filter_map(Clone::clone).collect::<Vec<_>>();
                    let references = records
                        .iter()
                        .filter(|record| record.state == RemediationState::WaitingForPromptUse)
                        .map(|record| record.remediation_id.as_str())
                        .collect::<Vec<_>>();
                    let suffix = references
                        .iter()
                        .map(|id| format!("\n\n{PROMPT_REFERENCE_PREFIX}{id}"))
                        .collect::<String>();
                    let prompt = if base.len().saturating_add(suffix.len())
                        > antiburn_local::remediation::MAX_PROMPT_BYTES
                    {
                        self.format_current_prompt_fallback(detector, &[])?
                            .to_owned()
                            + &suffix
                    } else {
                        base.clone() + &suffix
                    };
                    if prompt.len() > antiburn_local::remediation::MAX_PROMPT_BYTES {
                        anyhow::bail!("remediation prompt exceeds its byte limit");
                    }
                    Ok((records, prompt))
                },
            )
            .map_err(|_| ControllerError::PersistenceFailed)?;
        result.ok_or(ControllerError::TargetChanged)
    }

    fn format_current_prompt(
        &self,
        store: &Store,
        detector: DetectorId,
        targets: &[CachedTarget],
        fallback: bool,
    ) -> Result<String, ControllerError> {
        for target in targets {
            remediation_prompt(target.finding()).map_err(ControllerError::PromptUnavailable)?;
        }
        let paths = representative_paths(
            store,
            targets.iter().flat_map(|target| {
                target.evidence_sessions().into_iter().map(|sample| {
                    SessionKey::new(sample.environment_key, sample.agent, sample.session_id)
                })
            }),
        )?;
        if fallback || targets.is_empty() {
            return self.format_current_prompt_fallback(detector, &paths);
        }
        let prompt = if targets.len() > 1
            && targets
                .iter()
                .all(|target| target.finding().is_advisory_resource())
        {
            let items =
                targets
                    .iter()
                    .enumerate()
                    .map(|(index, target)| {
                        let display = target
                            .finding()
                            .display()
                            .map_err(ControllerError::PromptUnavailable)?;
                        let resource = display.facts.labels.first().ok_or(
                            ControllerError::PromptUnavailable(
                                RemediationUnavailableReason::EssentialIdentityUnavailable,
                            ),
                        )?;
                        let resource = serde_json::to_string(resource)
                            .map_err(|_| ControllerError::Internal)?;
                        Ok(format!(
                            "{}. Agent: {}; scope: {}; resource: {resource}",
                            index + 1,
                            display.agent.slug(),
                            target.scope_kind
                        ))
                    })
                    .collect::<Result<Vec<_>, ControllerError>>()?;
            let base = format!(
                "{}\n\nUnused targets\n{}",
                fallback_remediation_prompt(detector)
                    .map_err(ControllerError::PromptUnavailable)?
                    .into_string(),
                items.join("\n")
            );
            prompt_with_evidence_paths(&base, &paths, None)
        } else {
            let sections = targets
                .iter()
                .enumerate()
                .map(|(index, target)| {
                    let base = remediation_prompt(target.finding())
                        .map_err(ControllerError::PromptUnavailable)?
                        .into_string();
                    let paths = representative_paths(
                        store,
                        target.evidence_sessions().into_iter().map(|sample| {
                            SessionKey::new(sample.environment_key, sample.agent, sample.session_id)
                        }),
                    )?;
                    let base = prompt_with_evidence_paths(&base, &paths, None)
                        .map_err(ControllerError::PromptUnavailable)?;
                    if targets.len() == 1 {
                        Ok(base)
                    } else {
                        Ok(format!("Exact target {}\n{base}", index + 1))
                    }
                })
                .collect::<Result<Vec<_>, ControllerError>>()?;
            prompt_with_evidence_paths(&sections.join("\n\n"), &[], None)
        };
        match prompt {
            Ok(prompt) => Ok(prompt),
            Err(RemediationUnavailableReason::PromptSizeLimit) => {
                self.format_current_prompt_fallback(detector, &paths)
            }
            Err(reason) => Err(ControllerError::PromptUnavailable(reason)),
        }
    }

    fn format_current_prompt_fallback(
        &self,
        detector: DetectorId,
        paths: &[String],
    ) -> Result<String, ControllerError> {
        let base = fallback_remediation_prompt(detector)
            .map_err(ControllerError::PromptUnavailable)?
            .into_string();
        prompt_with_evidence_paths(&base, paths, None).map_err(ControllerError::PromptUnavailable)
    }

    pub(super) fn copy_current_prompt_for_cached_targets(
        &self,
        store: &Store,
        cached: &[CachedTarget],
    ) -> Result<(Vec<RemediationRecord>, String), ControllerError> {
        let detector = cached
            .first()
            .ok_or(ControllerError::CheckPromptUnavailable)?
            .finding()
            .detector;
        if cached
            .iter()
            .any(|target| target.finding().detector != detector)
        {
            return Err(ControllerError::CheckPromptUnavailable);
        }
        for target in cached {
            remediation_prompt(target.finding()).map_err(ControllerError::PromptUnavailable)?;
        }
        // Legacy handles have no report window. Source start times do not bound session updates.
        let context = cached[0]
            .resource
            .as_ref()
            .map(|resource| resource.context.clone())
            .unwrap_or_else(|| BurnCheckTargetContext {
                environment_key: cached[0].environment_key().to_owned(),
                window: antiburn_local::insights::ReportWindow {
                    start_epoch: 0,
                    end_epoch: now_epoch().saturating_add(1),
                },
            });
        if cached.iter().any(|target| {
            target.environment_key() != context.environment_key
                || target
                    .resource
                    .as_ref()
                    .is_some_and(|resource| resource.context != context)
        }) {
            return Err(ControllerError::CheckPromptUnavailable);
        }
        let ids = cached.iter().map(stable_finding_id).collect::<Vec<_>>();
        self.copy_current_prompt_selection(store, detector, Some(&ids), context)
    }
}

fn validate_prompt_selection(ids: &[String]) -> Result<(), ControllerError> {
    if ids.is_empty()
        || ids.len() > MAX_CHECK_PROMPT_TARGETS
        || ids.iter().any(|id| id.is_empty() || id.len() > 256)
        || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(ControllerError::CheckPromptUnavailable);
    }
    Ok(())
}

fn retry_prompt_snapshot<T>(
    mut operation: impl FnMut() -> Result<T, ControllerError>,
) -> Result<T, ControllerError> {
    match operation() {
        Err(ControllerError::TargetChanged) => operation(),
        result => result,
    }
}

#[cfg(test)]
mod tests;
