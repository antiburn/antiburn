//! Native accounting facts remain separate from pricing estimates and billing.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::Usage;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenClasses {
    pub input_tokens: u64,
    pub visible_output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl TokenClasses {
    pub(crate) fn parse(value: &Value) -> Option<Self> {
        let tokens = Self {
            input_tokens: value.get("input")?.as_u64()?,
            visible_output_tokens: value.get("output")?.as_u64()?,
            reasoning_tokens: value.get("reasoning")?.as_u64()?,
            cache_read_tokens: value.pointer("/cache/read")?.as_u64()?,
            cache_write_tokens: value.pointer("/cache/write")?.as_u64()?,
        };
        let usage = tokens.normalized_usage()?;
        [
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_tokens,
            usage.cache_creation_tokens,
        ]
        .into_iter()
        .all(|count| i64::try_from(count).is_ok())
        .then_some(tokens)
    }

    /// Shared output includes reasoning once because both use the output rate.
    pub fn normalized_usage(self) -> Option<Usage> {
        Some(Usage {
            input_tokens: self.input_tokens,
            output_tokens: self
                .visible_output_tokens
                .checked_add(self.reasoning_tokens)?,
            cache_read_tokens: self.cache_read_tokens,
            cache_creation_tokens: self.cache_write_tokens,
            cache_creation_1h_tokens: 0,
        })
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        let sum = Self {
            input_tokens: self.input_tokens.checked_add(other.input_tokens)?,
            visible_output_tokens: self
                .visible_output_tokens
                .checked_add(other.visible_output_tokens)?,
            reasoning_tokens: self.reasoning_tokens.checked_add(other.reasoning_tokens)?,
            cache_read_tokens: self
                .cache_read_tokens
                .checked_add(other.cache_read_tokens)?,
            cache_write_tokens: self
                .cache_write_tokens
                .checked_add(other.cache_write_tokens)?,
        };
        sum.normalized_usage()?;
        Some(sum)
    }

    fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            input_tokens: self.input_tokens.checked_sub(other.input_tokens)?,
            visible_output_tokens: self
                .visible_output_tokens
                .checked_sub(other.visible_output_tokens)?,
            reasoning_tokens: self.reasoning_tokens.checked_sub(other.reasoning_tokens)?,
            cache_read_tokens: self
                .cache_read_tokens
                .checked_sub(other.cache_read_tokens)?,
            cache_write_tokens: self
                .cache_write_tokens
                .checked_sub(other.cache_write_tokens)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CostProvenance {
    ProducerCalculated,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalculatedCost {
    pub usd: f64,
    pub provenance: CostProvenance,
}

impl CalculatedCost {
    /// A recorded zero does not establish available rates or free work.
    pub fn producer_calculated(usd: f64) -> Option<Self> {
        (usd.is_finite() && usd >= 0.0).then_some(Self {
            usd,
            provenance: CostProvenance::ProducerCalculated,
        })
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        Self::producer_calculated(self.usd + other.usd)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepAccounting {
    pub native_variant: Option<String>,
    pub token_classes: Option<TokenClasses>,
    pub calculated_cost: Option<CalculatedCost>,
}

impl StepAccounting {
    pub fn is_valid(&self) -> bool {
        self.native_variant.as_ref().is_none_or(|variant| {
            variant.len() <= super::EVIDENCE_STRING_CAP && !variant.contains('\0')
        }) && self
            .token_classes
            .is_none_or(|tokens| tokens.normalized_usage().is_some())
            && self
                .calculated_cost
                .is_none_or(|cost| cost.usd.is_finite() && cost.usd >= 0.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceAccounting {
    pub aggregate_tokens: Option<TokenClasses>,
    /// Sum of known retained counts. Overflow makes the sum unknown.
    pub retained_tokens: Option<TokenClasses>,
    pub aggregate_calculated_cost: Option<CalculatedCost>,
    /// Sum of known retained calculated costs, not an invoice total.
    pub retained_calculated_cost: Option<CalculatedCost>,
    /// Source states merge before totals, so opposite differences cannot cancel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_token_reconciliation: Option<UsageReconciliation<TokenClasses>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_cost_reconciliation: Option<UsageReconciliation<CalculatedCost>>,
}

impl Default for SourceAccounting {
    fn default() -> Self {
        Self {
            aggregate_tokens: None,
            retained_tokens: Some(TokenClasses::default()),
            aggregate_calculated_cost: None,
            retained_calculated_cost: CalculatedCost::producer_calculated(0.0),
            source_token_reconciliation: None,
            source_cost_reconciliation: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "camelCase")]
pub enum UsageReconciliation<T> {
    Unknown,
    Matched,
    Unallocated(T),
    Inconsistent,
}

impl<T> UsageReconciliation<T> {
    /// Source defects cannot cancel when inclusive totals combine sources.
    pub fn checked_merge(self, other: Self, add: impl FnOnce(T, T) -> Option<T>) -> Self {
        match (self, other) {
            (Self::Inconsistent, _) | (_, Self::Inconsistent) => Self::Inconsistent,
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::Matched, result) | (result, Self::Matched) => result,
            (Self::Unallocated(a), Self::Unallocated(b)) => {
                add(a, b).map_or(Self::Unknown, Self::Unallocated)
            }
        }
    }
}

impl SourceAccounting {
    pub fn token_reconciliation(self) -> UsageReconciliation<TokenClasses> {
        if let Some(reconciliation) = self.source_token_reconciliation {
            return reconciliation;
        }
        let (Some(aggregate), Some(retained)) = (self.aggregate_tokens, self.retained_tokens)
        else {
            return UsageReconciliation::Unknown;
        };
        match aggregate.checked_sub(retained) {
            None => UsageReconciliation::Inconsistent,
            Some(remainder) if remainder == TokenClasses::default() => UsageReconciliation::Matched,
            Some(remainder) => UsageReconciliation::Unallocated(remainder),
        }
    }

    pub fn cost_reconciliation(self) -> UsageReconciliation<CalculatedCost> {
        if let Some(reconciliation) = self.source_cost_reconciliation {
            return reconciliation;
        }
        let (Some(aggregate), Some(retained)) = (
            self.aggregate_calculated_cost,
            self.retained_calculated_cost,
        ) else {
            return UsageReconciliation::Unknown;
        };
        if !aggregate.usd.is_finite()
            || !retained.usd.is_finite()
            || aggregate.usd < 0.0
            || retained.usd < 0.0
        {
            return UsageReconciliation::Inconsistent;
        }
        let difference = aggregate.usd - retained.usd;
        if difference.abs() <= 0.000_000_001 {
            UsageReconciliation::Matched
        } else if let Some(cost) = CalculatedCost::producer_calculated(difference) {
            UsageReconciliation::Unallocated(cost)
        } else {
            UsageReconciliation::Inconsistent
        }
    }

    pub(crate) fn observe(&mut self, step: &StepAccounting) {
        if let Some(tokens) = step.token_classes {
            self.retained_tokens = self.retained_tokens.and_then(|sum| sum.checked_add(tokens));
        }
        if let Some(cost) = step.calculated_cost {
            self.retained_calculated_cost = self
                .retained_calculated_cost
                .and_then(|sum| sum.checked_add(cost));
        }
    }

    /// Inclusive values sum separate sources and keep their reconciliation states.
    pub fn checked_merge(self, other: Self) -> Self {
        Self {
            source_token_reconciliation: Some(
                self.token_reconciliation()
                    .checked_merge(other.token_reconciliation(), TokenClasses::checked_add),
            ),
            source_cost_reconciliation: Some(
                self.cost_reconciliation()
                    .checked_merge(other.cost_reconciliation(), CalculatedCost::checked_add),
            ),
            aggregate_tokens: self
                .aggregate_tokens
                .zip(other.aggregate_tokens)
                .and_then(|(a, b)| a.checked_add(b)),
            retained_tokens: self
                .retained_tokens
                .zip(other.retained_tokens)
                .and_then(|(a, b)| a.checked_add(b)),
            aggregate_calculated_cost: self
                .aggregate_calculated_cost
                .zip(other.aggregate_calculated_cost)
                .and_then(|(a, b)| a.checked_add(b)),
            retained_calculated_cost: self
                .retained_calculated_cost
                .zip(other.retained_calculated_cost)
                .and_then(|(a, b)| a.checked_add(b)),
        }
    }
}
