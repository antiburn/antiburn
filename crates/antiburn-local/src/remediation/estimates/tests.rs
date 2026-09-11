use super::*;

fn pricing(input: f64) -> crate::pricing::ModelPricing {
    crate::pricing::ModelPricing {
        input_cost_per_token: input,
        output_cost_per_token: 0.0,
        cache_read_cost_per_token: 0.0,
        cache_write_cost_per_token: 0.0,
    }
}

fn savings_interval() -> SavingsInterval {
    SavingsInterval {
        boundary_ms: 100,
        measured_through_ms: 200,
        recurrence_ms: None,
    }
}

fn old_model_savings(old_rate: f64, replacement_rate: f64) -> OldModelSavingsEstimate {
    estimate_old_model_savings(&OldModelSavingsInput {
        interval: savings_interval(),
        tokens: Some(crate::pricing::ModelTokens {
            input_tokens: 100,
            ..crate::pricing::ModelTokens::default()
        }),
        old_pricing: Some(pricing(old_rate)),
        replacement_pricing: Some(pricing(replacement_rate)),
        pricing_revision: Some("pricing-7".to_owned()),
    })
}

#[test]
fn old_model_savings_preserve_positive_zero_and_negative_differences() {
    for (old_rate, replacement_rate, expected_cost) in
        [(2.0, 1.0, 100.0), (1.0, 1.0, 0.0), (1.0, 2.0, -100.0)]
    {
        let OldModelSavingsEstimate::Known(savings) = old_model_savings(old_rate, replacement_rate)
        else {
            panic!("expected known savings");
        };
        assert_eq!(savings.method_revision, SAVINGS_METHOD_REVISION);
        assert_eq!(savings.pricing_revision, "pricing-7");
        assert_eq!(savings.api_equivalent_cost_avoided_usd, expected_cost);
    }
}

#[test]
fn savings_return_typed_unknown_for_missing_inputs_and_overflow() {
    let missing_rates = OldModelSavingsInput {
        interval: savings_interval(),
        tokens: Some(crate::pricing::ModelTokens::default()),
        old_pricing: None,
        replacement_pricing: Some(pricing(1.0)),
        pricing_revision: Some("pricing-7".to_owned()),
    };
    assert_eq!(
        estimate_old_model_savings(&missing_rates),
        OldModelSavingsEstimate::Unknown(OldModelSavingsUnknownReason::MissingRates)
    );
    assert_eq!(
        estimate_old_model_savings(&OldModelSavingsInput {
            tokens: None,
            old_pricing: Some(pricing(1.0)),
            ..missing_rates.clone()
        }),
        OldModelSavingsEstimate::Unknown(OldModelSavingsUnknownReason::MissingEvidence)
    );
    assert_eq!(
        estimate_old_model_savings(&OldModelSavingsInput {
            pricing_revision: None,
            tokens: Some(crate::pricing::ModelTokens::default()),
            old_pricing: Some(pricing(1.0)),
            ..missing_rates
        }),
        OldModelSavingsEstimate::Unknown(OldModelSavingsUnknownReason::MissingRevision)
    );
    assert_eq!(
        estimate_old_model_savings(&OldModelSavingsInput {
            interval: savings_interval(),
            tokens: Some(crate::pricing::ModelTokens {
                input_tokens: u64::MAX,
                ..crate::pricing::ModelTokens::default()
            }),
            old_pricing: Some(pricing(f64::MAX)),
            replacement_pricing: Some(pricing(0.0)),
            pricing_revision: Some("pricing-7".to_owned()),
        }),
        OldModelSavingsEstimate::Unknown(OldModelSavingsUnknownReason::ArithmeticOverflow)
    );
}

#[test]
fn zero_tokens_preserve_known_zero_savings() {
    let OldModelSavingsEstimate::Known(savings) =
        estimate_old_model_savings(&OldModelSavingsInput {
            interval: savings_interval(),
            tokens: Some(crate::pricing::ModelTokens::default()),
            old_pricing: Some(pricing(2.0)),
            replacement_pricing: Some(pricing(1.0)),
            pricing_revision: Some("pricing-7".to_owned()),
        })
    else {
        panic!("expected known savings");
    };
    assert_eq!(savings.api_equivalent_cost_avoided_usd, 0.0);
}

#[test]
fn savings_require_a_valid_interval() {
    for interval in [
        SavingsInterval {
            boundary_ms: 100,
            measured_through_ms: 100,
            recurrence_ms: None,
        },
        SavingsInterval {
            boundary_ms: 100,
            measured_through_ms: 201,
            recurrence_ms: Some(200),
        },
    ] {
        assert_eq!(
            estimate_old_model_savings(&OldModelSavingsInput {
                interval,
                tokens: Some(crate::pricing::ModelTokens::default()),
                old_pricing: Some(pricing(1.0)),
                replacement_pricing: Some(pricing(0.5)),
                pricing_revision: Some("pricing-7".to_owned()),
            }),
            OldModelSavingsEstimate::Unknown(OldModelSavingsUnknownReason::MissingEvidence)
        );
    }
}

#[test]
fn all_nine_estimate_methods_keep_their_reviewed_units() {
    let comparison = PriceComparisonInput {
        tokens: Some(crate::pricing::ModelTokens {
            input_tokens: 100,
            ..crate::pricing::ModelTokens::default()
        }),
        baseline: Some(pricing(2.0)),
        alternative: Some(pricing(1.0)),
        pricing_revision: Some("pricing-7".to_owned()),
    };
    let inputs = [
        SavingsEstimateInput::RepeatedContextAboveDepthCap {
            observed_tokens: Some(500),
            depth_cap_tokens: 400,
        },
        SavingsEstimateInput::AssumedOutputReduction {
            observed_output_tokens: Some(200),
            reduction_basis_points: Some(2_500),
        },
        SavingsEstimateInput::WorkerModelPriceDifference(comparison.clone()),
        SavingsEstimateInput::McpDefinitionExposure {
            definition_tokens: Some(10),
            compatible_requests: Some(2),
        },
        SavingsEstimateInput::BuiltInDefinitionReplication {
            replicated_tokens: Some(30),
        },
        SavingsEstimateInput::InjectedSkillDocument {
            document_tokens: Some(20),
            compatible_requests: Some(2),
        },
        SavingsEstimateInput::OldModelPriceDifference(comparison.clone()),
        SavingsEstimateInput::FastTierPricePremium(comparison),
        SavingsEstimateInput::CacheRehydrationPriceDifference {
            repeated_paid_tokens: Some(100),
            paid_input_rate: Some(2.0),
            cache_read_rate: Some(1.0),
            pricing_revision: Some("pricing-7".to_owned()),
        },
    ];
    let expected_units = [
        SavingsUnit::LiteralInputTokens,
        SavingsUnit::AssumedOutputTokens,
        SavingsUnit::ApiEquivalentUsd,
        SavingsUnit::LiteralInputTokens,
        SavingsUnit::LiteralInputTokens,
        SavingsUnit::LiteralInputTokens,
        SavingsUnit::ApiEquivalentUsd,
        SavingsUnit::ApiEquivalentUsd,
        SavingsUnit::ApiEquivalentUsd,
    ];
    for ((detector, input), unit) in DetectorId::ALL.into_iter().zip(inputs).zip(expected_units) {
        assert_eq!(
            input.method(),
            SavingsEstimateMethod::for_detector(detector)
        );
        assert_eq!(
            estimate_savings(savings_interval(), &input)
                .value
                .unwrap()
                .unit,
            unit
        );
    }
}

#[test]
fn typed_estimates_preserve_zero_negative_unknown_and_overflow() {
    let zero = estimate_savings(
        savings_interval(),
        &SavingsEstimateInput::RepeatedContextAboveDepthCap {
            observed_tokens: Some(300),
            depth_cap_tokens: 400,
        },
    );
    assert_eq!(zero.value.unwrap().value, 0.0);
    let negative = estimate_savings(
        savings_interval(),
        &SavingsEstimateInput::FastTierPricePremium(PriceComparisonInput {
            tokens: Some(crate::pricing::ModelTokens {
                input_tokens: 1,
                ..crate::pricing::ModelTokens::default()
            }),
            baseline: Some(pricing(1.0)),
            alternative: Some(pricing(2.0)),
            pricing_revision: Some("pricing-7".to_owned()),
        }),
    );
    assert_eq!(negative.value.unwrap().value, -1.0);
    assert_eq!(
        estimate_savings(
            savings_interval(),
            &SavingsEstimateInput::InjectedSkillDocument {
                document_tokens: Some(10),
                compatible_requests: None,
            },
        )
        .value,
        Err(SavingsUnavailableReason::MissingEvidence)
    );
    assert_eq!(
        estimate_savings(
            savings_interval(),
            &SavingsEstimateInput::McpDefinitionExposure {
                definition_tokens: Some(u64::MAX),
                compatible_requests: Some(2),
            },
        )
        .value,
        Err(SavingsUnavailableReason::ArithmeticOverflow)
    );
}

#[test]
fn aggregation_rejects_overlap_missing_owners_and_mixed_units() {
    let value = |owner: Option<&str>, unit| OwnedSavingsValue {
        owner_key: owner.map(str::to_owned),
        value: SavingsValue { unit, value: 2.0 },
    };
    assert_eq!(
        aggregate_owned_savings(&[
            value(Some("request-a"), SavingsUnit::LiteralInputTokens),
            value(Some("request-b"), SavingsUnit::LiteralInputTokens),
        ])
        .unwrap()
        .unwrap()
        .value,
        4.0
    );
    for values in [
        vec![
            value(Some("request-a"), SavingsUnit::LiteralInputTokens),
            value(Some("request-a"), SavingsUnit::LiteralInputTokens),
        ],
        vec![
            value(Some("request-a"), SavingsUnit::LiteralInputTokens),
            value(Some("request-b"), SavingsUnit::ApiEquivalentUsd),
        ],
        vec![value(None, SavingsUnit::LiteralInputTokens)],
    ] {
        assert_eq!(
            aggregate_owned_savings(&values),
            Err(SavingsUnavailableReason::MissingOwnership)
        );
    }
}
