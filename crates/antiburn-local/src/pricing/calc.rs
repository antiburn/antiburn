// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Per-model cost calculation.
//!
//! [`calculate_cost`] prices a per-model token breakdown against a pricing map,
//! summing each model's cost independently. Models with no pricing entry are
//! returned as data in [`CostResult::unpriced_models`]; the caller decides
//! what to do about them, and unknown prices stay explicitly unavailable
//! instead of appearing as partial totals. The module itself performs no
//! I/O.

use std::collections::HashMap;

use crate::pricing::model::{Cost, ModelPricing, ModelTokens};
use crate::pricing::table::lookup_pricing;

/// The outcome of pricing a token breakdown: the cost plus any models that
/// could not be priced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CostResult {
    pub cost: Cost,
    /// Model keys present in the breakdown with no entry in the pricing map,
    /// in breakdown iteration order. Includes zero-token models.
    pub unpriced_models: Vec<String>,
}

/// Calculate cache-write cost, applying the one-hour premium only to the
/// explicitly classified subset of cache-creation tokens.
///
/// One-hour prompt-cache writes are priced at 2x base input. Any remaining
/// cache-creation tokens use the model's configured default write rate.
pub fn calculate_cache_write_cost(tokens: &ModelTokens, pricing: &ModelPricing) -> f64 {
    let one_hour_tokens = tokens
        .cache_creation_1h_tokens
        .min(tokens.cache_creation_tokens);
    let default_tokens = tokens.cache_creation_tokens - one_hour_tokens;

    default_tokens as f64 * pricing.cache_write_cost_per_token
        + one_hour_tokens as f64 * pricing.input_cost_per_token * 2.0
}

/// Calculate total estimated cost from a per-model token breakdown.
///
/// Sums across all models in `breakdown`, looking up the per-token cost for
/// each model. Models not found in the pricing map are skipped and recorded in
/// [`CostResult::unpriced_models`].
pub fn calculate_cost(
    breakdown: &HashMap<String, ModelTokens>,
    pricing_map: &HashMap<String, ModelPricing>,
) -> CostResult {
    let mut cost = Cost::default();
    let mut unpriced_models = Vec::new();

    for (model_id, tokens) in breakdown {
        let Some(pricing) = lookup_pricing(model_id, pricing_map) else {
            unpriced_models.push(model_id.clone());
            continue;
        };

        let input = tokens.input_tokens as f64 * pricing.input_cost_per_token;
        let output = tokens.output_tokens as f64 * pricing.output_cost_per_token;
        let cache_read = tokens.cache_read_tokens as f64 * pricing.cache_read_cost_per_token;
        let cache_write = calculate_cache_write_cost(tokens, pricing);

        cost.input_usd += input;
        cost.output_usd += output;
        cost.cache_read_usd += cache_read;
        cost.cache_write_usd += cache_write;
        cost.total_usd += input + output + cache_read + cache_write;
    }

    CostResult {
        cost,
        unpriced_models,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::table::fallback_pricing;

    fn breakdown(entries: &[(&str, ModelTokens)]) -> HashMap<String, ModelTokens> {
        entries
            .iter()
            .map(|(model, tokens)| (model.to_string(), tokens.clone()))
            .collect()
    }

    #[test]
    fn test_calculate_cost_sonnet() {
        let pricing = fallback_pricing();
        let breakdown = breakdown(&[(
            "claude-sonnet-4-5-20250929",
            ModelTokens {
                input_tokens: 1_000_000,
                output_tokens: 100_000,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cache_creation_1h_tokens: 0,
            },
        )]);

        let result = calculate_cost(&breakdown, &pricing);
        // Sonnet: 1M * $3/M + 100K * $15/M = $3 + $1.5 = $4.5
        assert!((result.cost.total_usd - 4.5).abs() < 0.001);
        assert!((result.cost.input_usd - 3.0).abs() < 0.001);
        assert!((result.cost.output_usd - 1.5).abs() < 0.001);
        assert!(result.unpriced_models.is_empty());
    }

    #[test]
    fn test_calculate_cost_opus_46() {
        let pricing = fallback_pricing();
        let breakdown = breakdown(&[(
            "claude-opus-4-6-20260301",
            ModelTokens {
                input_tokens: 1_000_000,
                output_tokens: 100_000,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cache_creation_1h_tokens: 0,
            },
        )]);

        let result = calculate_cost(&breakdown, &pricing);
        // Opus 4.6: 1M * $5/M + 100K * $25/M = $5 + $2.5 = $7.5
        assert!((result.cost.total_usd - 7.5).abs() < 0.001);
    }

    #[test]
    fn test_calculate_cost_with_cache() {
        let pricing = fallback_pricing();
        let breakdown = breakdown(&[(
            "claude-sonnet-4-5-20250929",
            ModelTokens {
                input_tokens: 500_000,
                output_tokens: 50_000,
                cache_read_tokens: 300_000,
                cache_creation_tokens: 200_000,
                cache_creation_1h_tokens: 0,
            },
        )]);

        let result = calculate_cost(&breakdown, &pricing);
        // Sonnet: 500K*$3/M + 50K*$15/M + 300K*$0.30/M + 200K*$6/M
        // = $1.5 + $0.75 + $0.09 + $1.20 = $3.54
        assert!((result.cost.total_usd - 3.54).abs() < 0.01);
        assert!((result.cost.cache_read_usd - 0.09).abs() < 0.01);
        assert!((result.cost.cache_write_usd - 1.20).abs() < 0.01);
    }

    #[test]
    fn test_calculate_cost_prices_one_hour_cache_writes_separately() {
        let pricing = HashMap::from([(
            "custom-model".to_string(),
            ModelPricing {
                input_cost_per_token: 1e-05,
                output_cost_per_token: 0.0,
                cache_read_cost_per_token: 0.0,
                cache_write_cost_per_token: 1.25e-05,
            },
        )]);
        let breakdown = breakdown(&[(
            "custom-model",
            ModelTokens {
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 200_000,
                cache_creation_1h_tokens: 150_000,
            },
        )]);

        let result = calculate_cost(&breakdown, &pricing);
        // Custom pricing: 50K five-minute writes at $12.50/M plus 150K one-hour
        // writes at $20/M = $0.625 + $3.00 = $3.625.
        assert!((result.cost.cache_write_usd - 3.625).abs() < 0.001);
        assert!((result.cost.total_usd - 3.625).abs() < 0.001);
    }

    #[test]
    fn test_calculate_cost_multiple_models() {
        let pricing = fallback_pricing();
        let breakdown = breakdown(&[
            (
                "claude-haiku-4-5-20251001",
                ModelTokens {
                    input_tokens: 1_000_000,
                    output_tokens: 200_000,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_creation_1h_tokens: 0,
                },
            ),
            (
                "claude-opus-4-5-20250929",
                ModelTokens {
                    input_tokens: 100_000,
                    output_tokens: 50_000,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_creation_1h_tokens: 0,
                },
            ),
        ]);

        let result = calculate_cost(&breakdown, &pricing);
        // Haiku 4.5: 1M*$1/M + 200K*$5/M = $1 + $1 = $2
        // Opus 4.5: 100K*$5/M + 50K*$25/M = $0.5 + $1.25 = $1.75
        // Total: $3.75
        assert!((result.cost.total_usd - 3.75).abs() < 0.01);
    }

    #[test]
    fn test_calculate_cost_gpt_55_fast_uses_priority_fallback() {
        let pricing = fallback_pricing();
        let breakdown = breakdown(&[(
            "gpt-5.5-fast",
            ModelTokens {
                input_tokens: 1_000_000,
                output_tokens: 100_000,
                cache_read_tokens: 500_000,
                cache_creation_tokens: 0,
                cache_creation_1h_tokens: 0,
            },
        )]);

        let result = calculate_cost(&breakdown, &pricing);
        // gpt-5.5 Priority: 1M*$12.50/M + 100K*$75/M + 500K*$1.25/M.
        assert!((result.cost.total_usd - 20.625).abs() < 0.001);
    }

    #[test]
    fn test_calculate_cost_empty_breakdown() {
        let pricing = fallback_pricing();
        let result = calculate_cost(&HashMap::new(), &pricing);
        assert_eq!(result, CostResult::default());
    }

    #[test]
    fn test_calculate_cost_reports_unpriced_models_as_data() {
        let pricing = fallback_pricing();
        let breakdown = breakdown(&[(
            "totally-unknown-model",
            ModelTokens {
                input_tokens: 1_000_000,
                output_tokens: 100_000,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cache_creation_1h_tokens: 0,
            },
        )]);

        let result = calculate_cost(&breakdown, &pricing);
        assert_eq!(result.cost.total_usd, 0.0);
        assert_eq!(result.unpriced_models, vec!["totally-unknown-model"]);
    }
}
