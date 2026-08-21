// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! On-device session cost estimation — the engine's *local estimate* layer over
//! [`crate::pricing`].
//!
//! The pricing primitives — the bundled catalog, model-key normalization, and the
//! per-model cost math — live in [`crate::pricing`]. This module is the thin
//! analysis-side adapter over them: it holds the bundled table as a process-wide
//! singleton that a caller may overlay with a newer snapshot
//! ([`install_runtime_pricing`]), strips the `[..]` context-window tag some
//! transcripts carry (which the catalog's normalization intentionally leaves to
//! callers), and maps a [`crate::pricing::Cost`] into the analysis-facing
//! [`SessionCost`].
//!
//! Every figure produced here is an estimate computed on this machine from the
//! bundled catalog; nothing is fetched. Cost is priced **per-model**: a session
//! that mixes models (e.g. Opus on the main thread, Haiku on subagents) is billed
//! at each model's own rate rather than entirely at the most expensive one. When
//! no model in a session is priceable, [`price_breakdown`] returns `None` so a
//! caller can render "unknown" rather than a misleading `$0`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::pricing::{ModelPricing, ModelTokens, authoritative_openai_pricing, fallback_pricing};

use crate::analysis::engine::SessionCost;

static PRICING_GENERATION: AtomicU64 = AtomicU64::new(0);

/// The cold-start pricing table, built once from `crate::pricing::fallback_pricing()`.
/// Held in a `OnceLock` so lookups borrow a stable reference and the table is allocated
/// a single time per process.
fn pricing_table() -> &'static std::sync::RwLock<HashMap<String, ModelPricing>> {
    static TABLE: std::sync::OnceLock<std::sync::RwLock<HashMap<String, ModelPricing>>> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(|| std::sync::RwLock::new(fallback_pricing()))
}

/// Replace the process runtime overrides while retaining bundled fallback
/// coverage. Runtime entries win on ordinary normalized model keys; verified
/// OpenAI entries remain authoritative over downloaded snapshots.
pub fn install_runtime_pricing(runtime: HashMap<String, ModelPricing>) {
    let merged = merged_pricing(runtime);
    let models = merged.len();
    let mut table = pricing_table()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if *table == merged {
        return;
    }
    *table = merged;
    let generation = PRICING_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    ::tracing::info!(event = "runtime_pricing_installed", generation, models);
}

/// Monotonic process-local version used to invalidate cached cost-bearing
/// metrics whenever a newer runtime pricing snapshot is installed.
pub fn pricing_generation() -> u64 {
    PRICING_GENERATION.load(Ordering::Relaxed)
}

fn merged_pricing(runtime: HashMap<String, ModelPricing>) -> HashMap<String, ModelPricing> {
    let mut merged = fallback_pricing();
    for (model, pricing) in runtime {
        let stripped = strip_window_tag(&model);
        let normalized = crate::pricing::normalize_model_key(stripped).to_lowercase();
        merged.insert(normalized, pricing);
    }
    merged.extend(authoritative_openai_pricing());
    merged
}

/// Strip a trailing `[..]` context-window tag (e.g. `claude-opus-4-7[1m]`) so the key
/// matches the pricing table. [`crate::pricing::lookup_pricing`] handles lowercasing
/// and the `-YYYYMMDD` date suffix, but leaves this transcript-side window tag to the
/// caller.
pub fn strip_window_tag(model: &str) -> &str {
    match model.find('[') {
        Some(idx) => model[..idx].trim_end(),
        None => model,
    }
}

/// Look up pricing for a model id, stripping the window tag first. Returns the
/// catalog's [`ModelPricing`] (`*_cost_per_token` fields), or `None` for unknown models.
pub fn lookup_pricing(model: &str) -> Option<ModelPricing> {
    let table = pricing_table()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::pricing::lookup_pricing(strip_window_tag(model), &table).cloned()
}

/// Estimate the cost of a session from a per-model token breakdown, pricing each model at
/// its own rate. Returns `None` when *no* model in the breakdown is priceable, so the UI
/// shows no badge rather than a wrong `$0`.
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
        return None; // a partial subject total would silently undercount
    }
    let c = result.cost;
    Some(SessionCost {
        total_usd: c.total_usd,
        input_usd: c.input_usd,
        output_usd: c.output_usd,
        cache_read_usd: c.cache_read_usd,
        cache_write_usd: c.cache_write_usd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(input: u64, output: u64, cache_read: u64, cache_creation: u64) -> ModelTokens {
        ModelTokens {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_creation,
            cache_creation_1h_tokens: 0,
        }
    }

    #[test]
    fn strip_window_tag_drops_bracketed_suffix() {
        assert_eq!(strip_window_tag("claude-opus-4-7[1m]"), "claude-opus-4-7");
        assert_eq!(strip_window_tag("claude-opus-4-6"), "claude-opus-4-6");
    }

    #[test]
    fn lookup_resolves_window_and_date_suffixes() {
        let base = lookup_pricing("claude-opus-4-6").unwrap();
        let windowed = lookup_pricing("claude-opus-4-7[1m]").unwrap();
        let dated = lookup_pricing("claude-opus-4-6-20260301").unwrap();
        // 4-7 shares the 4-6 tier; date/window variants resolve to the same rates.
        assert_eq!(base.input_cost_per_token, windowed.input_cost_per_token);
        assert_eq!(base.input_cost_per_token, dated.input_cost_per_token);
    }

    #[test]
    fn unknown_model_is_not_priceable() {
        assert!(lookup_pricing("some-unreleased-model").is_none());
        let mut b = HashMap::new();
        b.insert(
            "some-unreleased-model".to_string(),
            tokens(1000, 1000, 0, 0),
        );
        assert!(price_breakdown(&b).is_none());
    }

    #[test]
    fn empty_breakdown_yields_no_estimate() {
        assert!(price_breakdown(&HashMap::new()).is_none());
    }

    #[test]
    fn single_model_sums_four_components() {
        // Opus 4.6: in 5e-06, out 2.5e-05, cr 5e-07, cw 1e-05.
        let mut b = HashMap::new();
        b.insert(
            "claude-opus-4-6".to_string(),
            tokens(1_000_000, 1_000_000, 1_000_000, 1_000_000),
        );
        let c = price_breakdown(&b).unwrap();
        assert!((c.input_usd - 5.0).abs() < 1e-9);
        assert!((c.output_usd - 25.0).abs() < 1e-9);
        assert!((c.cache_read_usd - 0.5).abs() < 1e-9);
        assert!((c.cache_write_usd - 10.0).abs() < 1e-9);
        assert!((c.total_usd - 40.5).abs() < 1e-9);
    }

    #[test]
    fn one_hour_cache_writes_use_twice_the_input_rate() {
        let mut model_tokens = tokens(0, 0, 0, 1_000_000);
        model_tokens.cache_creation_1h_tokens = 1_000_000;
        let breakdown = HashMap::from([("claude-opus-4-6".to_string(), model_tokens)]);

        let cost = price_breakdown(&breakdown).unwrap();
        assert!((cost.cache_write_usd - 10.0).abs() < 1e-9);
        assert!((cost.total_usd - 10.0).abs() < 1e-9);
    }

    #[test]
    fn mixed_models_price_per_model_not_at_most_expensive() {
        // 1M output tokens on Opus (main) + 1M output tokens on Haiku (subagent).
        let mut b = HashMap::new();
        b.insert("claude-opus-4-6".to_string(), tokens(0, 1_000_000, 0, 0));
        b.insert("claude-haiku-4-5".to_string(), tokens(0, 1_000_000, 0, 0));
        let per_model = price_breakdown(&b).unwrap();
        // Per-model: Opus output 2.5e-05 + Haiku output 5e-06 = $25 + $5 = $30.
        assert!((per_model.total_usd - 30.0).abs() < 1e-9);

        // The old behavior priced both buckets at the most-expensive model (Opus):
        // 2M * 2.5e-05 = $50. Per-model must be strictly cheaper.
        let mut all_opus = HashMap::new();
        all_opus.insert("claude-opus-4-6".to_string(), tokens(0, 2_000_000, 0, 0));
        let overstated = price_breakdown(&all_opus).unwrap();
        assert!((overstated.total_usd - 50.0).abs() < 1e-9);
        assert!(per_model.total_usd < overstated.total_usd);
    }

    #[test]
    fn any_unpriced_model_suppresses_the_partial_subject_total() {
        // One known + one unknown model must not silently price only the known
        // portion of the addressed session.
        let mut b = HashMap::new();
        b.insert("claude-haiku-4-5".to_string(), tokens(0, 1_000_000, 0, 0));
        b.insert(
            "some-unreleased-model".to_string(),
            tokens(0, 1_000_000, 0, 0),
        );
        assert!(price_breakdown(&b).is_none());
    }

    #[test]
    fn runtime_prices_override_bundled_entries_without_erasing_fallbacks() {
        let runtime_sonnet = ModelPricing {
            input_cost_per_token: 9.0,
            output_cost_per_token: 8.0,
            cache_read_cost_per_token: 7.0,
            cache_write_cost_per_token: 6.0,
        };
        let stale_gpt_56 = ModelPricing {
            input_cost_per_token: 1.0,
            output_cost_per_token: 2.0,
            cache_read_cost_per_token: 3.0,
            cache_write_cost_per_token: 4.0,
        };
        let merged = merged_pricing(HashMap::from([
            (
                "CLAUDE-SONNET-5-20260713".to_string(),
                runtime_sonnet.clone(),
            ),
            ("gpt-5.6-terra".to_string(), stale_gpt_56),
        ]));
        assert_eq!(merged.get("claude-sonnet-5"), Some(&runtime_sonnet));
        assert!(merged.contains_key("claude-opus-4-6"));
        assert_eq!(
            merged.get("gpt-5.6-terra"),
            authoritative_openai_pricing().get("gpt-5.6-terra"),
        );
    }
}
