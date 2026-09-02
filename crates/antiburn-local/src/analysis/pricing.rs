//! On-device session cost estimation over a caller-installed pricing snapshot.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::analysis::engine::SessionCost;
use crate::pricing::{ModelPricing, ModelTokens};

static PRICING_GENERATION: AtomicU64 = AtomicU64::new(0);

fn pricing_table() -> &'static std::sync::RwLock<HashMap<String, ModelPricing>> {
    static TABLE: std::sync::OnceLock<std::sync::RwLock<HashMap<String, ModelPricing>>> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(|| std::sync::RwLock::new(initial_pricing()))
}

#[cfg(not(any(test, feature = "test-instrumentation")))]
fn initial_pricing() -> HashMap<String, ModelPricing> {
    HashMap::new()
}

// These fixtures keep cost tests deterministic. Production builds start empty.
#[cfg(any(test, feature = "test-instrumentation"))]
fn initial_pricing() -> HashMap<String, ModelPricing> {
    fn price(input: f64, output: f64, cache_read: f64, cache_write: f64) -> ModelPricing {
        ModelPricing {
            input_cost_per_token: input,
            output_cost_per_token: output,
            cache_read_cost_per_token: cache_read,
            cache_write_cost_per_token: cache_write,
        }
    }

    let opus = price(5e-6, 25e-6, 0.5e-6, 10e-6);
    HashMap::from([
        ("claude-opus-4-6".to_string(), opus.clone()),
        ("claude-opus-4-7".to_string(), opus.clone()),
        ("claude-opus-4-8".to_string(), opus),
        (
            "claude-sonnet-4-5".to_string(),
            price(3e-6, 15e-6, 0.3e-6, 6e-6),
        ),
        (
            "claude-sonnet-4-6".to_string(),
            price(3e-6, 15e-6, 0.3e-6, 6e-6),
        ),
        (
            "claude-3-5-haiku".to_string(),
            price(0.8e-6, 4e-6, 0.08e-6, 1.6e-6),
        ),
        (
            "claude-haiku-4-5".to_string(),
            price(1e-6, 5e-6, 0.1e-6, 2e-6),
        ),
        (
            "claude-fable-5".to_string(),
            price(10e-6, 50e-6, 1e-6, 20e-6),
        ),
        ("gpt-5.4".to_string(), price(2.5e-6, 15e-6, 0.25e-6, 0.0)),
        ("gpt-5.6".to_string(), price(5e-6, 30e-6, 0.5e-6, 6.25e-6)),
        (
            "gpt-5.6-sol".to_string(),
            price(5e-6, 30e-6, 0.5e-6, 6.25e-6),
        ),
    ])
}

fn normalized_pricing(runtime: HashMap<String, ModelPricing>) -> HashMap<String, ModelPricing> {
    runtime
        .into_iter()
        .map(|(model, pricing)| {
            let stripped = strip_window_tag(&model);
            let normalized = crate::pricing::normalize_model_key(stripped).to_lowercase();
            (normalized, pricing)
        })
        .collect()
}

/// Replace the active runtime snapshot. Returns whether the table changed.
pub fn install_runtime_pricing(runtime: HashMap<String, ModelPricing>) -> bool {
    let runtime = normalized_pricing(runtime);
    let models = runtime.len();
    let mut table = pricing_table()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if *table == runtime {
        return false;
    }
    *table = runtime;
    let generation = PRICING_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    ::tracing::info!(event = "runtime_pricing_installed", generation, models);
    true
}

/// Return the process-local generation of the active pricing snapshot.
pub fn pricing_generation() -> u64 {
    PRICING_GENERATION.load(Ordering::Relaxed)
}

/// Strip a trailing context-window tag from a transcript model ID.
pub fn strip_window_tag(model: &str) -> &str {
    match model.find('[') {
        Some(index) => model[..index].trim_end(),
        None => model,
    }
}

/// Look up one model in the active pricing snapshot.
pub fn lookup_pricing(model: &str) -> Option<ModelPricing> {
    let table = pricing_table()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::pricing::lookup_pricing(strip_window_tag(model), &table).cloned()
}

/// Estimate a complete per-model token breakdown.
pub fn price_breakdown(breakdown: &HashMap<String, ModelTokens>) -> Option<SessionCost> {
    if breakdown.is_empty() {
        return None;
    }
    let table = pricing_table()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let result = crate::pricing::calculate_cost(breakdown, &table);
    if !result.unpriced_models.is_empty() {
        for model in &result.unpriced_models {
            ::tracing::trace!(event = "model_unpriced", model);
        }
        return None;
    }
    let cost = result.cost;
    Some(SessionCost {
        total_usd: cost.total_usd,
        input_usd: cost.input_usd,
        output_usd: cost.output_usd,
        cache_read_usd: cost.cache_read_usd,
        cache_write_usd: cost.cache_write_usd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_and_date_suffixes_resolve() {
        let base = lookup_pricing("claude-opus-4-7").unwrap();
        assert_eq!(lookup_pricing("claude-opus-4-7[1m]"), Some(base.clone()));
        assert_eq!(lookup_pricing("claude-opus-4-7-20260301"), Some(base));
    }

    #[test]
    fn unknown_or_partial_breakdowns_have_no_estimate() {
        let unknown = HashMap::from([(
            "unknown-model".to_string(),
            ModelTokens {
                output_tokens: 1,
                ..Default::default()
            },
        )]);
        assert!(price_breakdown(&unknown).is_none());
        assert!(price_breakdown(&HashMap::new()).is_none());
    }

    #[test]
    fn runtime_keys_are_normalized_before_installation() {
        let rate = ModelPricing {
            input_cost_per_token: 1.0,
            output_cost_per_token: 2.0,
            cache_read_cost_per_token: 3.0,
            cache_write_cost_per_token: 4.0,
        };
        let normalized = normalized_pricing(HashMap::from([(
            "SAMPLE-MODEL-20260301[1m]".to_string(),
            rate.clone(),
        )]));
        assert_eq!(normalized.get("sample-model"), Some(&rate));
    }
}
