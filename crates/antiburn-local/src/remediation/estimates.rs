use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::insights::DetectorId;

use super::SAVINGS_METHOD_REVISION;

/// The reviewed estimate method for each burn detector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SavingsEstimateMethod {
    RepeatedContextAboveDepthCap,
    AssumedOutputReduction,
    WorkerModelPriceDifference,
    McpDefinitionExposure,
    BuiltInDefinitionReplication,
    InjectedSkillDocument,
    OldModelPriceDifference,
    FastTierPricePremium,
    CacheRehydrationPriceDifference,
}

impl SavingsEstimateMethod {
    pub const fn for_detector(detector: DetectorId) -> Self {
        match detector {
            DetectorId::SessionsOverDepth => Self::RepeatedContextAboveDepthCap,
            DetectorId::ModelOverthinking => Self::AssumedOutputReduction,
            DetectorId::OverpoweredSubagents => Self::WorkerModelPriceDifference,
            DetectorId::UnusedMcpServers => Self::McpDefinitionExposure,
            DetectorId::UnusedBuiltInTools => Self::BuiltInDefinitionReplication,
            DetectorId::UnusedSkills => Self::InjectedSkillDocument,
            DetectorId::OldModelUsage => Self::OldModelPriceDifference,
            DetectorId::OveruseOfFastMode => Self::FastTierPricePremium,
            DetectorId::CacheChurn => Self::CacheRehydrationPriceDifference,
        }
    }
}

/// Units that can be compared or added without changing their meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SavingsUnit {
    LiteralInputTokens,
    AssumedOutputTokens,
    CacheClassTokens,
    ApiEquivalentUsd,
    Improvements,
}

/// A finite numeric estimate. Signed values preserve zero and regressions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavingsValue {
    pub unit: SavingsUnit,
    pub value: f64,
}

/// Why a reviewed method cannot return a numeric result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavingsUnavailableReason {
    InvalidInterval,
    MissingEvidence,
    MissingAssumption,
    MissingComparison,
    MissingRates,
    MissingRevision,
    MissingOwnership,
    ArithmeticOverflow,
}

/// Inputs shared by methods that reprice an observed token quantity.
#[derive(Debug, Clone, PartialEq)]
pub struct PriceComparisonInput {
    pub tokens: Option<crate::pricing::ModelTokens>,
    pub baseline: Option<crate::pricing::ModelPricing>,
    pub alternative: Option<crate::pricing::ModelPricing>,
    pub pricing_revision: Option<String>,
}

/// Exact typed input for one of the nine reviewed estimate methods.
#[derive(Debug, Clone, PartialEq)]
pub enum SavingsEstimateInput {
    RepeatedContextAboveDepthCap {
        observed_tokens: Option<u64>,
        depth_cap_tokens: u64,
    },
    AssumedOutputReduction {
        observed_output_tokens: Option<u64>,
        reduction_basis_points: Option<u32>,
    },
    WorkerModelPriceDifference(PriceComparisonInput),
    McpDefinitionExposure {
        definition_tokens: Option<u64>,
        compatible_requests: Option<u64>,
    },
    BuiltInDefinitionReplication {
        replicated_tokens: Option<u64>,
    },
    InjectedSkillDocument {
        document_tokens: Option<u64>,
        compatible_requests: Option<u64>,
    },
    OldModelPriceDifference(PriceComparisonInput),
    FastTierPricePremium(PriceComparisonInput),
    CacheRehydrationPriceDifference {
        repeated_paid_tokens: Option<u64>,
        paid_input_rate: Option<f64>,
        cache_read_rate: Option<f64>,
        pricing_revision: Option<String>,
    },
}

impl SavingsEstimateInput {
    pub const fn method(&self) -> SavingsEstimateMethod {
        match self {
            Self::RepeatedContextAboveDepthCap { .. } => {
                SavingsEstimateMethod::RepeatedContextAboveDepthCap
            }
            Self::AssumedOutputReduction { .. } => SavingsEstimateMethod::AssumedOutputReduction,
            Self::WorkerModelPriceDifference(_) => {
                SavingsEstimateMethod::WorkerModelPriceDifference
            }
            Self::McpDefinitionExposure { .. } => SavingsEstimateMethod::McpDefinitionExposure,
            Self::BuiltInDefinitionReplication { .. } => {
                SavingsEstimateMethod::BuiltInDefinitionReplication
            }
            Self::InjectedSkillDocument { .. } => SavingsEstimateMethod::InjectedSkillDocument,
            Self::OldModelPriceDifference(_) => SavingsEstimateMethod::OldModelPriceDifference,
            Self::FastTierPricePremium(_) => SavingsEstimateMethod::FastTierPricePremium,
            Self::CacheRehydrationPriceDifference { .. } => {
                SavingsEstimateMethod::CacheRehydrationPriceDifference
            }
        }
    }
}

/// One estimate result pinned to its method and revisions.
#[derive(Debug, Clone, PartialEq)]
pub struct SavingsEstimate {
    pub method: SavingsEstimateMethod,
    pub method_revision: u32,
    pub pricing_revision: Option<String>,
    pub value: Result<SavingsValue, SavingsUnavailableReason>,
}

/// Calculates one reviewed estimate without converting unavailable facts into zero.
pub fn estimate_savings(
    interval: SavingsInterval,
    input: &SavingsEstimateInput,
) -> SavingsEstimate {
    let method = input.method();
    let mut estimate = SavingsEstimate {
        method,
        method_revision: SAVINGS_METHOD_REVISION,
        pricing_revision: None,
        value: Err(SavingsUnavailableReason::InvalidInterval),
    };
    if !valid_savings_interval(interval) {
        return estimate;
    }
    estimate.value = match input {
        SavingsEstimateInput::RepeatedContextAboveDepthCap {
            observed_tokens,
            depth_cap_tokens,
        } => observed_tokens.map_or(Err(SavingsUnavailableReason::MissingEvidence), |observed| {
            known_value(
                SavingsUnit::LiteralInputTokens,
                observed.saturating_sub(*depth_cap_tokens) as f64,
            )
        }),
        SavingsEstimateInput::AssumedOutputReduction {
            observed_output_tokens,
            reduction_basis_points,
        } => match (observed_output_tokens, reduction_basis_points) {
            (None, _) => Err(SavingsUnavailableReason::MissingEvidence),
            (_, None) => Err(SavingsUnavailableReason::MissingAssumption),
            (Some(tokens), Some(basis_points)) if *basis_points <= 10_000 => known_value(
                SavingsUnit::AssumedOutputTokens,
                *tokens as f64 * f64::from(*basis_points) / 10_000.0,
            ),
            _ => Err(SavingsUnavailableReason::MissingAssumption),
        },
        SavingsEstimateInput::WorkerModelPriceDifference(input)
        | SavingsEstimateInput::OldModelPriceDifference(input)
        | SavingsEstimateInput::FastTierPricePremium(input) => {
            estimate.pricing_revision = valid_revision(input.pricing_revision.as_ref());
            price_comparison(input, estimate.pricing_revision.is_some())
        }
        SavingsEstimateInput::McpDefinitionExposure {
            definition_tokens,
            compatible_requests,
        }
        | SavingsEstimateInput::InjectedSkillDocument {
            document_tokens: definition_tokens,
            compatible_requests,
        } => checked_product(*definition_tokens, *compatible_requests)
            .and_then(|value| known_value(SavingsUnit::LiteralInputTokens, value as f64)),
        SavingsEstimateInput::BuiltInDefinitionReplication { replicated_tokens } => {
            replicated_tokens.map_or(Err(SavingsUnavailableReason::MissingEvidence), |value| {
                known_value(SavingsUnit::LiteralInputTokens, value as f64)
            })
        }
        SavingsEstimateInput::CacheRehydrationPriceDifference {
            repeated_paid_tokens,
            paid_input_rate,
            cache_read_rate,
            pricing_revision,
        } => {
            estimate.pricing_revision = valid_revision(pricing_revision.as_ref());
            match (repeated_paid_tokens, paid_input_rate, cache_read_rate) {
                (None, _, _) => Err(SavingsUnavailableReason::MissingEvidence),
                (_, None, _) | (_, _, None) => Err(SavingsUnavailableReason::MissingRates),
                (Some(tokens), Some(paid), Some(cache))
                    if estimate.pricing_revision.is_some()
                        && paid.is_finite()
                        && *paid >= 0.0
                        && cache.is_finite()
                        && *cache >= 0.0 =>
                {
                    known_value(
                        SavingsUnit::ApiEquivalentUsd,
                        *tokens as f64 * (*paid - *cache),
                    )
                }
                (_, _, _) if estimate.pricing_revision.is_none() => {
                    Err(SavingsUnavailableReason::MissingRevision)
                }
                _ => Err(SavingsUnavailableReason::MissingRates),
            }
        }
    };
    estimate
}

fn known_value(unit: SavingsUnit, value: f64) -> Result<SavingsValue, SavingsUnavailableReason> {
    value
        .is_finite()
        .then_some(SavingsValue { unit, value })
        .ok_or(SavingsUnavailableReason::ArithmeticOverflow)
}

fn checked_product(left: Option<u64>, right: Option<u64>) -> Result<u64, SavingsUnavailableReason> {
    let (left, right) = left
        .zip(right)
        .ok_or(SavingsUnavailableReason::MissingEvidence)?;
    left.checked_mul(right)
        .ok_or(SavingsUnavailableReason::ArithmeticOverflow)
}

fn valid_revision(revision: Option<&String>) -> Option<String> {
    revision.filter(|value| !value.is_empty()).cloned()
}

fn price_comparison(
    input: &PriceComparisonInput,
    revision_valid: bool,
) -> Result<SavingsValue, SavingsUnavailableReason> {
    if !revision_valid {
        return Err(SavingsUnavailableReason::MissingRevision);
    }
    let Some(tokens) = input.tokens.as_ref() else {
        return Err(SavingsUnavailableReason::MissingEvidence);
    };
    let (Some(baseline), Some(alternative)) = (input.baseline.as_ref(), input.alternative.as_ref())
    else {
        return Err(SavingsUnavailableReason::MissingComparison);
    };
    if !valid_pricing(baseline) || !valid_pricing(alternative) {
        return Err(SavingsUnavailableReason::MissingRates);
    }
    known_value(
        SavingsUnit::ApiEquivalentUsd,
        pricing_cost(tokens, baseline) - pricing_cost(tokens, alternative),
    )
}

/// One durable allocation supplied to aggregate accounting.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnedSavingsValue {
    pub owner_key: Option<String>,
    pub value: SavingsValue,
}

/// Adds values only when every value has one unique durable owner and one unit.
pub fn aggregate_owned_savings(
    values: &[OwnedSavingsValue],
) -> Result<Option<SavingsValue>, SavingsUnavailableReason> {
    let Some(first) = values.first() else {
        return Ok(None);
    };
    let mut owners = BTreeSet::new();
    let mut total = 0.0;
    for value in values {
        let owner = value
            .owner_key
            .as_ref()
            .filter(|owner| !owner.is_empty())
            .ok_or(SavingsUnavailableReason::MissingOwnership)?;
        if value.value.unit != first.value.unit || !owners.insert(owner) {
            return Err(SavingsUnavailableReason::MissingOwnership);
        }
        total += value.value.value;
        if !total.is_finite() {
            return Err(SavingsUnavailableReason::ArithmeticOverflow);
        }
    }
    Ok(Some(SavingsValue {
        unit: first.value.unit,
        value: total,
    }))
}

/// The effective activity interval represented by a savings aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavingsInterval {
    pub boundary_ms: i64,
    pub measured_through_ms: i64,
    pub recurrence_ms: Option<i64>,
}

/// Input for one idempotent old-model savings recomputation.
#[derive(Debug, Clone, PartialEq)]
pub struct OldModelSavingsInput {
    pub interval: SavingsInterval,
    pub tokens: Option<crate::pricing::ModelTokens>,
    pub old_pricing: Option<crate::pricing::ModelPricing>,
    pub replacement_pricing: Option<crate::pricing::ModelPricing>,
    pub pricing_revision: Option<String>,
}

/// Supported API-equivalent savings. Negative cost means the replacement costs more.
#[derive(Debug, Clone, PartialEq)]
pub struct OldModelSavings {
    pub method_revision: u32,
    pub pricing_revision: String,
    pub api_equivalent_cost_avoided_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OldModelSavingsUnknownReason {
    MissingRates,
    MissingEvidence,
    MissingRevision,
    ArithmeticOverflow,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OldModelSavingsEstimate {
    Known(OldModelSavings),
    Unknown(OldModelSavingsUnknownReason),
}

/// Recomputes cumulative old-model savings without I/O or retained state.
pub fn estimate_old_model_savings(input: &OldModelSavingsInput) -> OldModelSavingsEstimate {
    if !valid_savings_interval(input.interval) {
        return OldModelSavingsEstimate::Unknown(OldModelSavingsUnknownReason::MissingEvidence);
    }
    let Some(pricing_revision) = input
        .pricing_revision
        .as_ref()
        .filter(|revision| !revision.is_empty())
    else {
        return OldModelSavingsEstimate::Unknown(OldModelSavingsUnknownReason::MissingRevision);
    };
    let Some(tokens) = input.tokens.as_ref() else {
        return OldModelSavingsEstimate::Unknown(OldModelSavingsUnknownReason::MissingEvidence);
    };
    let (Some(old_pricing), Some(replacement_pricing)) = (
        input.old_pricing.as_ref(),
        input.replacement_pricing.as_ref(),
    ) else {
        return OldModelSavingsEstimate::Unknown(OldModelSavingsUnknownReason::MissingRates);
    };
    if !valid_pricing(old_pricing) || !valid_pricing(replacement_pricing) {
        return OldModelSavingsEstimate::Unknown(OldModelSavingsUnknownReason::MissingRates);
    }
    let old_cost = pricing_cost(tokens, old_pricing);
    let replacement_cost = pricing_cost(tokens, replacement_pricing);
    let difference = old_cost - replacement_cost;
    if !old_cost.is_finite() || !replacement_cost.is_finite() || !difference.is_finite() {
        return OldModelSavingsEstimate::Unknown(OldModelSavingsUnknownReason::ArithmeticOverflow);
    }
    OldModelSavingsEstimate::Known(OldModelSavings {
        method_revision: SAVINGS_METHOD_REVISION,
        pricing_revision: pricing_revision.clone(),
        api_equivalent_cost_avoided_usd: difference,
    })
}

fn valid_savings_interval(interval: SavingsInterval) -> bool {
    interval.measured_through_ms > interval.boundary_ms
        && interval.recurrence_ms.is_none_or(|recurrence_ms| {
            recurrence_ms > interval.boundary_ms && interval.measured_through_ms <= recurrence_ms
        })
}

fn pricing_cost(
    tokens: &crate::pricing::ModelTokens,
    pricing: &crate::pricing::ModelPricing,
) -> f64 {
    tokens.input_tokens as f64 * pricing.input_cost_per_token
        + tokens.output_tokens as f64 * pricing.output_cost_per_token
        + tokens.cache_read_tokens as f64 * pricing.cache_read_cost_per_token
        + crate::pricing::calc::calculate_cache_write_cost(tokens, pricing)
}

fn valid_pricing(pricing: &crate::pricing::ModelPricing) -> bool {
    [
        pricing.input_cost_per_token,
        pricing.output_cost_per_token,
        pricing.cache_read_cost_per_token,
        pricing.cache_write_cost_per_token,
    ]
    .into_iter()
    .all(|rate| rate.is_finite() && rate >= 0.0)
}

#[cfg(test)]
mod tests;
