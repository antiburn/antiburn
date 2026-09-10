//! Overuse of Fast Mode: explicit fast-tier usage in delegated work.
//!
//! The evidence separates main-loop from delegated fast-tier turns.
//! A standing-default signal does not exist in the contract yet, so
//! the rule covers only the delegated-work half of the category.
//! Both shipped families recognize exactly the normalized labels
//! `fast` and `standard` (`ReportCatalogs::families`), so — unlike
//! Model Overthinking — the rule does not need to know which family
//! produced a turn to classify its speed label.
//!
//! Partial-evidence rules:
//! - Partial model evidence still permits a finding. Observed
//!   delegated fast-tier turns prove presence.
//! - Partial model evidence prevents clean. A missed record may hide
//!   delegated fast-tier work.
//! - A source without the fast-tier capability reports the contract
//!   gap: the eligibility clause admits a session on service-tier
//!   alone, but `ModelEvidence::service_tiers` is a bare marker the
//!   rule cannot read, so neither a finding nor clean is expressible.
//! - The rule counts only the normalized `fast` label. Any other
//!   recognized label (for example `"standard"`) never counts toward
//!   the finding.
//! - A `fast_modes` key with turns that normalizes to neither `fast`
//!   nor `standard` blocks clean with a contract gap: the policy has
//!   not classified the label, so absence is never provable. A finding
//!   still wins over this gap.
//! - A finding still wins when no turn reports a speed value.
//!   Otherwise the rule assesses only the turns that do report a
//!   value. A turn without the signal is not negative evidence.
//!   Claude Code writes `speed` on every main-loop turn. It omits
//!   `speed` on most delegated turns. A full-coverage rule never
//!   clears a session with subagent work. A missing signal is not
//!   negative evidence because delegated turns often omit this field.
//!   The rule skips each no-signal turn and reports the signal as
//!   missing only when no turn carries a speed value.

use crate::analysis::{FAST_SPEED_KEY, SessionEvidence};
use crate::model_catalog::{
    ModelCatalog, ReviewedModelCatalog, Support, fixed_route_target, model_control_target,
};
use crate::remediation::FindingCause;

use super::{Observation, ReportCatalogs, observed};

pub(crate) fn evaluate(evidence: &SessionEvidence, catalogs: &ReportCatalogs) -> Observation {
    // The eligibility clause accepts service-tier as an alternative to
    // fast-tier, but the rule reads only `models.fast_modes`;
    // `ModelEvidence::service_tiers` is a bare marker it cannot read.
    // Mirror the Overpowered Subagents guard: report the contract gap
    // instead of a verdict the evidence cannot support.
    if !evidence.capabilities.fast_tier {
        return Observation::ContractIncomplete;
    }
    if let Some(models) = observed(&evidence.models) {
        let catalog = ReviewedModelCatalog::new(catalogs);
        let mut contract_incomplete = false;
        let mut delegated_fast_turns = 0_u64;
        if !models.control_observations.is_empty() {
            for observation in &models.control_observations {
                let Some(speed) = observation.speed.as_ref() else {
                    continue;
                };
                if observation.turns.main_loop + observation.turns.delegated == 0 {
                    continue;
                }
                let mut target = model_control_target(
                    &evidence.identity.agent,
                    observation.provider.as_deref(),
                    observation.api.as_deref(),
                    &observation.model,
                );
                target.service_tier = Some(speed.clone());
                match catalog.resolve(&target) {
                    Support::Supported(definition) => match definition.service_tier {
                        Support::Supported(Some(speed)) if speed == FAST_SPEED_KEY => {
                            delegated_fast_turns =
                                delegated_fast_turns.saturating_add(observation.turns.delegated);
                            if delegated_fast_turns > 0
                                && delegated_fast_turns
                                    >= catalogs.fast_mode_delegated_turns_threshold
                            {
                                return Observation::Finding;
                            }
                        }
                        Support::Supported(Some(_)) | Support::Supported(None) => {}
                        Support::Unsupported { .. } | Support::Unknown { .. } => {
                            contract_incomplete = true;
                        }
                    },
                    Support::Unsupported { .. } | Support::Unknown { .. } => {
                        contract_incomplete = true;
                    }
                }
            }
            if contract_incomplete {
                return Observation::ContractIncomplete;
            }
            let coverage = models.speed_signal;
            return if coverage.present_turns == 0
                || coverage.present_turns < coverage.eligible_turns
            {
                Observation::SignalMissing
            } else {
                Observation::NoFinding
            };
        }
        let attributed = models
            .fast_modes_by_model
            .iter()
            .flat_map(|(model, speeds)| {
                speeds
                    .iter()
                    .map(move |(label, turns)| (Some(model.as_str()), label, turns))
            });
        let legacy = models
            .fast_modes
            .iter()
            .map(|(label, turns)| (None, label, turns));
        for (model, label, turns) in attributed.chain(
            models
                .fast_modes_by_model
                .is_empty()
                .then_some(legacy)
                .into_iter()
                .flatten(),
        ) {
            let turns_with_signal = turns.main_loop + turns.delegated;
            if turns_with_signal == 0 {
                continue;
            }
            let model = model.or_else(|| {
                (models.by_model.len() == 1)
                    .then(|| models.by_model.keys().next())
                    .flatten()
                    .map(String::as_str)
            });
            let Some(model) = model else {
                contract_incomplete = true;
                continue;
            };
            let Some(mut target) = fixed_route_target(&evidence.identity.agent, model) else {
                contract_incomplete = true;
                continue;
            };
            target.service_tier = Some(label.clone());
            match catalog.resolve(&target) {
                Support::Supported(definition) => match definition.service_tier {
                    Support::Supported(Some(speed)) if speed == FAST_SPEED_KEY => {
                        delegated_fast_turns = delegated_fast_turns.saturating_add(turns.delegated);
                        if delegated_fast_turns > 0
                            && delegated_fast_turns >= catalogs.fast_mode_delegated_turns_threshold
                        {
                            return Observation::Finding;
                        }
                    }
                    Support::Supported(Some(_)) | Support::Supported(None) => {}
                    Support::Unsupported { .. } | Support::Unknown { .. } => {
                        contract_incomplete = true;
                    }
                },
                Support::Unsupported { .. } | Support::Unknown { .. } => {
                    contract_incomplete = true;
                }
            }
        }
        if contract_incomplete {
            return Observation::ContractIncomplete;
        }
        // A turn without a speed value is not negative evidence. The
        // rule assesses only the turns that report one. The rule
        // reports the signal as missing only when no turn reports one.
        // This rule also covers zero eligible turns, because
        // `present_turns` is then zero too.
        let coverage = models.speed_signal;
        if coverage.present_turns == 0 || coverage.present_turns < coverage.eligible_turns {
            return Observation::SignalMissing;
        }
    }
    Observation::NoFinding
}

pub(super) fn finding_causes(
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
) -> Vec<FindingCause> {
    let Some(models) = observed(&evidence.models) else {
        return Vec::new();
    };
    let catalog = ReviewedModelCatalog::new(catalogs);
    let mut grouped =
        std::collections::BTreeMap::<(Option<String>, Option<String>, String), u64>::new();
    if !models.control_observations.is_empty() {
        for observation in &models.control_observations {
            let Some(raw_speed) = observation.speed.as_ref() else {
                continue;
            };
            if observation.turns.delegated == 0 {
                continue;
            }
            let mut target = model_control_target(
                &evidence.identity.agent,
                observation.provider.as_deref(),
                observation.api.as_deref(),
                &observation.model,
            );
            target.service_tier = Some(raw_speed.clone());
            let Support::Supported(definition) = catalog.resolve(&target) else {
                continue;
            };
            if matches!(definition.service_tier, Support::Supported(Some(ref speed)) if speed == FAST_SPEED_KEY)
            {
                *grouped
                    .entry((
                        observation.provider.clone(),
                        observation.api.clone(),
                        observation.model.clone(),
                    ))
                    .or_default() += observation.turns.delegated;
            }
        }
    } else {
        for (model, speeds) in &models.fast_modes_by_model {
            for (raw_speed, turns) in speeds {
                if turns.delegated == 0 {
                    continue;
                }
                let Some(mut target) = fixed_route_target(&evidence.identity.agent, model) else {
                    continue;
                };
                target.service_tier = Some(raw_speed.clone());
                let Support::Supported(definition) = catalog.resolve(&target) else {
                    continue;
                };
                if matches!(definition.service_tier, Support::Supported(Some(ref speed)) if speed == FAST_SPEED_KEY)
                {
                    *grouped.entry((None, None, model.clone())).or_default() += turns.delegated;
                }
            }
        }
        if models.fast_modes_by_model.is_empty() && models.by_model.len() == 1 {
            let model = models.by_model.keys().next().expect("one model");
            for (raw_speed, turns) in &models.fast_modes {
                if turns.delegated == 0 {
                    continue;
                }
                let Some(mut target) = fixed_route_target(&evidence.identity.agent, model) else {
                    continue;
                };
                target.service_tier = Some(raw_speed.clone());
                let Support::Supported(definition) = catalog.resolve(&target) else {
                    continue;
                };
                if matches!(definition.service_tier, Support::Supported(Some(ref speed)) if speed == FAST_SPEED_KEY)
                {
                    *grouped.entry((None, None, model.clone())).or_default() += turns.delegated;
                }
            }
        }
    }
    let total_turns = grouped
        .values()
        .fold(0_u64, |total, turns| total.saturating_add(*turns));
    if total_turns == 0 || total_turns < catalogs.fast_mode_delegated_turns_threshold {
        return Vec::new();
    }
    grouped
        .into_iter()
        .filter(|(_, turns)| *turns > 0)
        .map(
            |((provider, api, model), delegated_turns)| FindingCause::OveruseOfFastMode {
                provider,
                api,
                model,
                delegated_turns,
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{
        CoverageReason, EvidenceValue, ModelControlObservation, ModelTokens, SignalCoverage,
        TurnCounts,
    };

    /// Builds evidence with one `FAST_SPEED_KEY` entry and full speed-
    /// signal coverage: every eligible turn carried a speed value.
    fn with_fast_turns(main_loop: u64, delegated: u64, partial: bool) -> SessionEvidence {
        with_speed_entry(FAST_SPEED_KEY, main_loop, delegated, partial)
    }

    fn with_speed_entry(
        label: &str,
        main_loop: u64,
        delegated: u64,
        partial: bool,
    ) -> SessionEvidence {
        let mut evidence = claude_evidence("fast");
        let EvidenceValue::Complete(mut models) = evidence.models else {
            unreachable!()
        };
        let turns = TurnCounts {
            main_loop,
            delegated,
        };
        models.fast_modes.insert(label.to_owned(), turns.clone());
        models
            .fast_modes_by_model
            .entry("claude-sonnet-4-6".to_owned())
            .or_default()
            .insert(label.to_owned(), turns);
        models
            .by_model
            .insert("claude-sonnet-4-6".to_owned(), ModelTokens::default());
        let turns = main_loop + delegated;
        models.speed_signal = SignalCoverage {
            eligible_turns: turns,
            present_turns: turns,
        };
        evidence.models = if partial {
            EvidenceValue::Partial {
                observed: models,
                reason: CoverageReason::MalformedRecord,
            }
        } else {
            EvidenceValue::Complete(models)
        };
        evidence
    }

    #[test]
    fn delegated_fast_turns_are_a_finding_even_from_partial_evidence() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_fast_turns(0, 1, false), &catalogs),
            Observation::Finding
        );
        assert_eq!(
            evaluate(&with_fast_turns(0, 1, true), &catalogs),
            Observation::Finding
        );
    }

    #[test]
    fn main_loop_fast_turns_alone_are_no_finding() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_fast_turns(5, 0, false), &catalogs),
            Observation::NoFinding
        );
    }

    #[test]
    fn split_route_observations_count_only_reviewed_delegated_fast_turns() {
        let catalogs = ReportCatalogs {
            fast_mode_delegated_turns_threshold: 2,
            ..ReportCatalogs::default()
        };
        for (provider, speed, delegated, expected) in [
            ("anthropic", "fast", 1, Observation::Finding),
            ("anthropic", "priority", 1, Observation::Finding),
            ("anthropic", "standard", 1, Observation::NoFinding),
            ("anthropic", "fast", 0, Observation::NoFinding),
            ("gateway", "fast", 1, Observation::ContractIncomplete),
            ("anthropic", "unknown", 1, Observation::ContractIncomplete),
        ] {
            let mut evidence = with_fast_turns(0, 1, false);
            evidence.identity.agent = "claude-code".to_owned();
            let EvidenceValue::Complete(models) = &mut evidence.models else {
                unreachable!()
            };
            models.control_observations = vec![
                ModelControlObservation {
                    provider: Some("anthropic".to_owned()),
                    api: Some("messages".to_owned()),
                    model: "claude-sonnet-5".to_owned(),
                    effort: Some("low".to_owned()),
                    speed: Some("fast".to_owned()),
                    last_ts_ms: 200,
                    turns: TurnCounts {
                        main_loop: 0,
                        delegated: 1,
                    },
                },
                ModelControlObservation {
                    provider: Some(provider.to_owned()),
                    api: Some("messages".to_owned()),
                    model: "claude-sonnet-5".to_owned(),
                    effort: Some("high".to_owned()),
                    speed: Some(speed.to_owned()),
                    last_ts_ms: 200,
                    turns: TurnCounts {
                        main_loop: 1 - delegated,
                        delegated,
                    },
                },
            ];
            models.speed_signal = SignalCoverage {
                eligible_turns: 2,
                present_turns: 2,
            };
            assert_eq!(
                evaluate(&evidence, &catalogs),
                expected,
                "{provider}/{speed}/{delegated}"
            );
            let EvidenceValue::Complete(models) = &mut evidence.models else {
                unreachable!()
            };
            models.control_observations.reverse();
            assert_eq!(evaluate(&evidence, &catalogs), expected);
        }
    }

    #[test]
    fn zero_threshold_still_requires_a_delegated_fast_turn() {
        let catalogs = ReportCatalogs {
            fast_mode_delegated_turns_threshold: 0,
            ..ReportCatalogs::default()
        };
        let mut evidence = with_fast_turns(1, 0, false);
        let EvidenceValue::Complete(models) = &mut evidence.models else {
            unreachable!()
        };
        models.control_observations.push(ModelControlObservation {
            provider: Some("anthropic".to_owned()),
            api: Some("messages".to_owned()),
            model: "claude-sonnet-5".to_owned(),
            effort: None,
            speed: Some("fast".to_owned()),
            last_ts_ms: 200,
            turns: TurnCounts {
                main_loop: 1,
                delegated: 0,
            },
        });
        assert_eq!(evaluate(&evidence, &catalogs), Observation::NoFinding);
    }

    #[test]
    fn legacy_fast_turns_aggregate_across_models_and_normalized_labels() {
        let catalogs = ReportCatalogs {
            fast_mode_delegated_turns_threshold: 2,
            ..ReportCatalogs::default()
        };
        let mut evidence = with_fast_turns(0, 1, false);
        let EvidenceValue::Complete(models) = &mut evidence.models else {
            unreachable!()
        };
        models
            .fast_modes_by_model
            .entry("claude-opus-5".to_owned())
            .or_default()
            .insert(
                " Fast ".to_owned(),
                TurnCounts {
                    main_loop: 0,
                    delegated: 1,
                },
            );
        assert_eq!(evaluate(&evidence, &catalogs), Observation::Finding);
    }

    #[test]
    fn a_delegated_standard_turn_never_counts_as_fast_mode_overuse() {
        // A delegated turn on the "standard" speed label must never
        // produce a finding: only the FAST_SPEED_KEY entry counts.
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_speed_entry("standard", 0, 1, false), &catalogs),
            Observation::NoFinding
        );
    }

    #[test]
    fn a_service_tier_only_source_reports_the_contract_gap() {
        let catalogs = ReportCatalogs::default();
        let mut evidence = claude_evidence("service-tier-only");
        evidence.capabilities.fast_tier = false;
        evidence.capabilities.service_tier = true;

        assert_eq!(
            evaluate(&evidence, &catalogs),
            Observation::ContractIncomplete
        );
    }

    #[test]
    fn no_speed_signal_present_reports_the_signal_as_missing() {
        // Zero eligible turns leaves `present_turns` at zero too. This
        // also proves the eligible_turns == 0 case stays missing.
        let catalogs = ReportCatalogs::default();
        let evidence = claude_evidence("no-eligible-turns");

        assert_eq!(evaluate(&evidence, &catalogs), Observation::SignalMissing);
    }

    #[test]
    fn partial_speed_signal_coverage_without_a_finding_is_missing() {
        let catalogs = ReportCatalogs::default();
        let mut evidence = claude_evidence("partial-speed-coverage");
        let EvidenceValue::Complete(mut models) = evidence.models else {
            unreachable!()
        };
        models.speed_signal = SignalCoverage {
            eligible_turns: 3,
            present_turns: 1,
        };
        evidence.models = EvidenceValue::Complete(models);

        assert_eq!(evaluate(&evidence, &catalogs), Observation::SignalMissing);
    }

    #[test]
    fn full_speed_signal_coverage_without_a_finding_is_no_finding() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_fast_turns(5, 0, false), &catalogs),
            Observation::NoFinding
        );
    }

    #[test]
    fn a_finding_wins_over_partial_speed_signal_coverage() {
        let catalogs = ReportCatalogs::default();
        let mut evidence = with_fast_turns(0, 1, false);
        let EvidenceValue::Complete(mut models) = evidence.models else {
            unreachable!()
        };
        models.speed_signal = SignalCoverage {
            eligible_turns: 3,
            present_turns: 1,
        };
        evidence.models = EvidenceValue::Complete(models);

        assert_eq!(evaluate(&evidence, &catalogs), Observation::Finding);
    }

    #[test]
    fn an_unrecognized_speed_label_with_turns_is_contract_incomplete() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_speed_entry("synthetic-tier", 0, 1, false), &catalogs),
            Observation::ContractIncomplete
        );
    }

    #[test]
    fn a_finding_wins_over_an_unrecognized_speed_label_elsewhere_in_the_session() {
        let catalogs = ReportCatalogs::default();
        let mut evidence = with_fast_turns(0, 1, false);
        let EvidenceValue::Complete(mut models) = evidence.models else {
            unreachable!()
        };
        models.fast_modes.insert(
            "synthetic-tier".to_owned(),
            TurnCounts {
                main_loop: 0,
                delegated: 1,
            },
        );
        evidence.models = EvidenceValue::Complete(models);

        assert_eq!(evaluate(&evidence, &catalogs), Observation::Finding);
    }

    #[test]
    fn label_normalization_trims_and_lowercases_before_policy_lookup() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_speed_entry("  Fast  ", 0, 1, false), &catalogs),
            Observation::Finding
        );
    }
}
