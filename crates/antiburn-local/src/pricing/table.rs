// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The single source of truth for the bundled pricing catalog and the
//! model-key normalization/lookup rules.
//!
//! [`fallback_pricing`] is the bundled catalog: prices transcribed from
//! official provider pricing pages, reviewed at
//! [`crate::pricing::PRICING_CATALOG_VERSION`]. The engine has no live price
//! source.

use std::collections::HashMap;

use crate::pricing::model::ModelPricing;

/// OpenCode's model key for OpenAI `gpt-5.5` Priority short-context usage.
pub const GPT_55_FAST_MODEL_KEY: &str = "gpt-5.5-fast";

/// Antigravity placeholder observed for user-facing Claude Opus 4.6 Thinking usage.
pub const ANTIGRAVITY_PLACEHOLDER_M26_MODEL_KEY: &str = "MODEL_PLACEHOLDER_M26";

/// Antigravity placeholder observed for user-facing Claude Sonnet 4.6 Thinking usage.
pub const ANTIGRAVITY_PLACEHOLDER_M35_MODEL_KEY: &str = "MODEL_PLACEHOLDER_M35";

/// Classify a full model ID string into a display tier for grouping.
///
/// Used for aggregating costs by model family in insight prompts.
/// NOT used for pricing lookups — use [`lookup_pricing`] for that.
pub fn classify_model(model_id: &str) -> &'static str {
    let lower = model_id.to_lowercase();

    if lower.contains("opus") {
        "opus"
    } else if lower.contains("sonnet") {
        "sonnet"
    } else if lower.contains("haiku") {
        "haiku"
    } else {
        "other"
    }
}

/// Strip trailing `-YYYYMMDD` date suffix from a model ID.
///
/// Model IDs from transcripts look like `claude-opus-4-6-20260301`.
/// Catalog keys are typically `claude-opus-4-6` (without the date).
pub fn normalize_model_key(model_id: &str) -> &str {
    // Date suffixes are 8 digits preceded by a hyphen
    if model_id.len() > 9 {
        let candidate = &model_id[model_id.len() - 8..];
        if candidate.chars().all(|c| c.is_ascii_digit())
            && model_id.as_bytes()[model_id.len() - 9] == b'-'
        {
            return &model_id[..model_id.len() - 9];
        }
    }
    model_id
}

/// Look up pricing for a model ID, trying exact match then normalized key.
pub fn lookup_pricing<'a>(
    model_id: &str,
    pricing_map: &'a HashMap<String, ModelPricing>,
) -> Option<&'a ModelPricing> {
    // 1. Exact match
    if let Some(p) = pricing_map.get(model_id) {
        return Some(p);
    }
    // 2. Try with date suffix stripped
    let normalized = normalize_model_key(model_id);
    if normalized != model_id
        && let Some(p) = pricing_map.get(normalized)
    {
        return Some(p);
    }
    // 3. Try lowercase
    let lower = model_id.to_lowercase();
    if lower != model_id {
        if let Some(p) = pricing_map.get(&lower) {
            return Some(p);
        }
        let lower_normalized = normalize_model_key(&lower);
        if lower_normalized != lower.as_str()
            && let Some(p) = pricing_map.get(lower_normalized)
        {
            return Some(p);
        }
    }

    None
}

/// Return the verified OpenAI Priority short-context pricing for `gpt-5.5-fast`.
///
/// OpenCode's `gpt-5.5-fast` key is treated as OpenAI `gpt-5.5` in Priority
/// mode; this price was verified against the official OpenAI pricing table
/// and is preferred over third-party pricing snapshots for this key.
pub fn gpt_55_fast_priority_pricing() -> ModelPricing {
    ModelPricing {
        input_cost_per_token: 1.25e-05,
        output_cost_per_token: 7.5e-05,
        cache_read_cost_per_token: 1.25e-06,
        cache_write_cost_per_token: 0.0,
    }
}

/// Return OpenAI prices verified against the official OpenAI pricing table.
///
/// These exact model keys must override downloaded pricing. The current pricing shape
/// represents only standard, short-context rates, so this deliberately does not add
/// service-tier, regional, long-context, or `gpt-5.6-codex` aliases.
pub fn authoritative_openai_pricing() -> HashMap<String, ModelPricing> {
    let gpt_56_sol = ModelPricing {
        input_cost_per_token: 5e-06,
        output_cost_per_token: 3e-05,
        cache_read_cost_per_token: 5e-07,
        cache_write_cost_per_token: 6.25e-06,
    };

    HashMap::from([
        (
            GPT_55_FAST_MODEL_KEY.to_string(),
            gpt_55_fast_priority_pricing(),
        ),
        ("gpt-5.6".to_string(), gpt_56_sol.clone()),
        ("gpt-5.6-sol".to_string(), gpt_56_sol.clone()),
        // Codex session metadata can retain the provider namespace.
        ("openai.gpt-5.6-sol".to_string(), gpt_56_sol),
        (
            "gpt-5.6-terra".to_string(),
            ModelPricing {
                input_cost_per_token: 2.5e-06,
                output_cost_per_token: 1.5e-05,
                cache_read_cost_per_token: 2.5e-07,
                cache_write_cost_per_token: 3.125e-06,
            },
        ),
        (
            "gpt-5.6-luna".to_string(),
            ModelPricing {
                input_cost_per_token: 1e-06,
                output_cost_per_token: 6e-06,
                cache_read_cost_per_token: 1e-07,
                cache_write_cost_per_token: 1.25e-06,
            },
        ),
    ])
}

/// The bundled pricing catalog.
///
/// Prices are sourced from official provider pricing pages and stored as
/// per-token costs (not per-million). Because [`ModelPricing`] cannot represent
/// long-context tiers, tiered Gemini and Grok entries use the providers'
/// standard short-context rates.
pub fn fallback_pricing() -> HashMap<String, ModelPricing> {
    let mut map = HashMap::new();

    // Claude cache writes default to the one-hour rate: 2x uncached input.
    // Opus 4.5 / 4.6 / 4.7 / 4.8: $5/$25 per 1M tokens
    let opus_new = ModelPricing {
        input_cost_per_token: 5e-06,
        output_cost_per_token: 2.5e-05,
        cache_read_cost_per_token: 5e-07,
        cache_write_cost_per_token: 1e-05,
    };
    map.insert("claude-opus-4-5".to_string(), opus_new.clone());
    map.insert("claude-opus-4-6".to_string(), opus_new.clone());
    map.insert("claude-opus-4-7".to_string(), opus_new.clone());
    map.insert("claude-opus-4-8".to_string(), opus_new.clone());
    map.insert("claude-opus-5".to_string(), opus_new.clone());
    // Cursor and provider-qualified spellings observed in session metadata.
    map.insert("claude-opus-4.6".to_string(), opus_new.clone());
    map.insert("claude-opus-4.7".to_string(), opus_new.clone());
    map.insert("claude-opus-4.8".to_string(), opus_new.clone());
    map.insert("anthropic/claude-opus-4.8".to_string(), opus_new.clone());
    map.insert(
        "claude-opus-4-8-thinking-high".to_string(),
        opus_new.clone(),
    );
    map.insert(
        "antigravity-claude-opus-4-6-thinking".to_string(),
        opus_new.clone(),
    );
    map.insert(ANTIGRAVITY_PLACEHOLDER_M26_MODEL_KEY.to_string(), opus_new);

    // Fable 5 / Mythos 5: $10/$50 per 1M tokens
    let claude_5 = ModelPricing {
        input_cost_per_token: 1e-05,
        output_cost_per_token: 5e-05,
        cache_read_cost_per_token: 1e-06,
        cache_write_cost_per_token: 2e-05,
    };
    map.insert("claude-fable-5".to_string(), claude_5.clone());
    map.insert("claude-mythos-5".to_string(), claude_5.clone());
    map.insert("claude-fable-5-thinking-high".to_string(), claude_5);

    // Sonnet 5: $2/$10 per 1M tokens, cached input $0.20 / 1M.
    map.insert(
        "claude-sonnet-5".to_string(),
        ModelPricing {
            input_cost_per_token: 2e-06,
            output_cost_per_token: 1e-05,
            cache_read_cost_per_token: 2e-07,
            cache_write_cost_per_token: 4e-06,
        },
    );

    // Opus 4 / 4.1: $15/$75 per 1M tokens
    let opus_old = ModelPricing {
        input_cost_per_token: 1.5e-05,
        output_cost_per_token: 7.5e-05,
        cache_read_cost_per_token: 1.5e-06,
        cache_write_cost_per_token: 3e-05,
    };
    map.insert("claude-opus-4".to_string(), opus_old.clone());
    map.insert("claude-opus-4-1".to_string(), opus_old.clone());
    map.insert("claude-3-opus".to_string(), opus_old);

    // Sonnet: $3/$15 per 1M tokens
    let sonnet = ModelPricing {
        input_cost_per_token: 3e-06,
        output_cost_per_token: 1.5e-05,
        cache_read_cost_per_token: 3e-07,
        cache_write_cost_per_token: 6e-06,
    };
    map.insert("claude-sonnet-4-6".to_string(), sonnet.clone());
    map.insert("claude-sonnet-4.6".to_string(), sonnet.clone());
    map.insert("claude-sonnet-4-5".to_string(), sonnet.clone());
    map.insert("claude-sonnet-4".to_string(), sonnet.clone());
    map.insert("claude-3-5-sonnet".to_string(), sonnet.clone());
    map.insert(ANTIGRAVITY_PLACEHOLDER_M35_MODEL_KEY.to_string(), sonnet);

    // Haiku 4.5: $1/$5 per 1M tokens
    let haiku45 = ModelPricing {
        input_cost_per_token: 1e-06,
        output_cost_per_token: 5e-06,
        cache_read_cost_per_token: 1e-07,
        cache_write_cost_per_token: 2e-06,
    };
    map.insert("claude-haiku-4-5".to_string(), haiku45.clone());
    map.insert("claude-haiku-4.5".to_string(), haiku45);

    // Haiku 3.5: $0.80/$4.00 per 1M tokens
    let haiku35 = ModelPricing {
        input_cost_per_token: 8e-07,
        output_cost_per_token: 4e-06,
        cache_read_cost_per_token: 8e-08,
        cache_write_cost_per_token: 1.6e-06,
    };
    map.insert("claude-3-5-haiku".to_string(), haiku35.clone());
    map.insert("claude-haiku-3-5".to_string(), haiku35);

    // Haiku 3: $0.25/$1.25 per 1M tokens
    let haiku3 = ModelPricing {
        input_cost_per_token: 2.5e-07,
        output_cost_per_token: 1.25e-06,
        cache_read_cost_per_token: 3e-08,
        cache_write_cost_per_token: 5e-07,
    };
    map.insert("claude-3-haiku".to_string(), haiku3);

    // GPT-5.4: $2.50 / $15.00 per 1M tokens, cached input $0.25 / 1M tokens.
    let gpt_54 = ModelPricing {
        input_cost_per_token: 2.5e-06,
        output_cost_per_token: 1.5e-05,
        cache_read_cost_per_token: 2.5e-07,
        cache_write_cost_per_token: 0.0,
    };
    map.insert("gpt-5.4".to_string(), gpt_54.clone());
    map.insert("gpt-5.4-2026-03-17".to_string(), gpt_54);

    // GPT-5.4 mini: $0.75 / $4.50 per 1M tokens, cached input $0.075 / 1M tokens.
    let gpt_54_mini = ModelPricing {
        input_cost_per_token: 7.5e-07,
        output_cost_per_token: 4.5e-06,
        cache_read_cost_per_token: 7.5e-08,
        cache_write_cost_per_token: 0.0,
    };
    map.insert("gpt-5.4-mini".to_string(), gpt_54_mini.clone());
    map.insert("gpt-5.4-mini-2026-03-17".to_string(), gpt_54_mini);

    // GPT-5.4 nano: $0.20 / $1.25 per 1M tokens, cached input $0.02 / 1M tokens.
    let gpt_54_nano = ModelPricing {
        input_cost_per_token: 2e-07,
        output_cost_per_token: 1.25e-06,
        cache_read_cost_per_token: 2e-08,
        cache_write_cost_per_token: 0.0,
    };
    map.insert("gpt-5.4-nano".to_string(), gpt_54_nano.clone());
    map.insert("gpt-5.4-nano-2026-03-17".to_string(), gpt_54_nano);

    // GPT-5.5: $5.00 / $30.00 per 1M tokens, cached input $0.50 / 1M tokens.
    let gpt_55 = ModelPricing {
        input_cost_per_token: 5e-06,
        output_cost_per_token: 3e-05,
        cache_read_cost_per_token: 5e-07,
        cache_write_cost_per_token: 0.0,
    };
    map.insert("gpt-5.5".to_string(), gpt_55);

    // Cursor Composer 2.5 standard and fast rates. Cursor does not charge
    // separately for cache writes in its published token pricing.
    map.insert(
        "composer-2.5".to_string(),
        ModelPricing {
            input_cost_per_token: 5e-07,
            output_cost_per_token: 2.5e-06,
            cache_read_cost_per_token: 2e-07,
            cache_write_cost_per_token: 0.0,
        },
    );
    map.insert(
        "composer-2.5-fast".to_string(),
        ModelPricing {
            input_cost_per_token: 3e-06,
            output_cost_per_token: 1.5e-05,
            cache_read_cost_per_token: 5e-07,
            cache_write_cost_per_token: 0.0,
        },
    );

    let gemini_31_pro = ModelPricing {
        input_cost_per_token: 2e-06,
        output_cost_per_token: 1.2e-05,
        cache_read_cost_per_token: 2e-07,
        cache_write_cost_per_token: 0.0,
    };
    map.insert("gemini-3.1-pro".to_string(), gemini_31_pro.clone());
    map.insert("gemini-3.1-pro-preview".to_string(), gemini_31_pro);
    map.insert(
        "grok-4.5".to_string(),
        ModelPricing {
            input_cost_per_token: 2e-06,
            output_cost_per_token: 6e-06,
            cache_read_cost_per_token: 3e-07,
            cache_write_cost_per_token: 0.0,
        },
    );

    // Verified GPT-5.5 Priority and GPT-5.6 standard prices are also forced
    // over external pricing snapshots by runtime consumers.
    map.extend(authoritative_openai_pricing());

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pricing_map() -> HashMap<String, ModelPricing> {
        fallback_pricing()
    }

    #[test]
    fn test_classify_opus() {
        assert_eq!(classify_model("claude-opus-4-5-20250929"), "opus");
        assert_eq!(classify_model("claude-opus-4-6-20260301"), "opus");
        assert_eq!(classify_model("claude-opus-4-1-20250929"), "opus");
    }

    #[test]
    fn test_classify_sonnet() {
        assert_eq!(classify_model("claude-sonnet-4-5-20250929"), "sonnet");
        assert_eq!(classify_model("claude-sonnet-4-20250514"), "sonnet");
        assert_eq!(classify_model("claude-3-5-sonnet-20241022"), "sonnet");
    }

    #[test]
    fn test_classify_haiku() {
        assert_eq!(classify_model("claude-haiku-4-5-20251001"), "haiku");
        assert_eq!(classify_model("claude-3-5-haiku-20241022"), "haiku");
        assert_eq!(classify_model("claude-3-haiku-20240307"), "haiku");
    }

    #[test]
    fn test_classify_unknown() {
        assert_eq!(classify_model("gpt-4o"), "other");
        assert_eq!(classify_model("unknown-model"), "other");
    }

    #[test]
    fn test_classify_case_insensitive() {
        assert_eq!(classify_model("Claude-Opus-4-5-20250929"), "opus");
        assert_eq!(classify_model("CLAUDE-SONNET-4-5-20250929"), "sonnet");
    }

    #[test]
    fn test_normalize_strips_date() {
        assert_eq!(
            normalize_model_key("claude-opus-4-6-20260301"),
            "claude-opus-4-6"
        );
        assert_eq!(
            normalize_model_key("claude-sonnet-4-5-20250929"),
            "claude-sonnet-4-5"
        );
        assert_eq!(
            normalize_model_key("claude-3-5-haiku-20241022"),
            "claude-3-5-haiku"
        );
    }

    #[test]
    fn test_normalize_no_date() {
        assert_eq!(normalize_model_key("claude-opus-4-6"), "claude-opus-4-6");
        assert_eq!(normalize_model_key("gpt-4o"), "gpt-4o");
    }

    #[test]
    fn test_normalize_short_string() {
        assert_eq!(normalize_model_key("short"), "short");
        assert_eq!(normalize_model_key(""), "");
    }

    #[test]
    fn test_lookup_exact_match() {
        let map = test_pricing_map();
        assert!(lookup_pricing("claude-opus-4-6", &map).is_some());
    }

    #[test]
    fn test_lookup_with_date_suffix() {
        let map = test_pricing_map();
        let p = lookup_pricing("claude-opus-4-6-20260301", &map).unwrap();
        assert!((p.input_cost_per_token - 5e-06).abs() < 1e-10);
    }

    #[test]
    fn test_lookup_unknown_model() {
        let map = test_pricing_map();
        assert!(lookup_pricing("gpt-4o", &map).is_none());
    }

    #[test]
    fn test_lookup_does_not_alias_gpt53_codex_to_gpt52_codex() {
        let mut map = HashMap::new();
        map.insert(
            "gpt-5.2-codex".to_string(),
            ModelPricing {
                input_cost_per_token: 1.23,
                output_cost_per_token: 4.56,
                cache_read_cost_per_token: 7.89,
                cache_write_cost_per_token: 0.12,
            },
        );

        assert!(lookup_pricing("gpt-5.3-codex", &map).is_none());
    }

    #[test]
    fn test_fallback_pricing_includes_gpt54_mini() {
        let map = fallback_pricing();
        let pricing = map.get("gpt-5.4-mini").expect("missing gpt-5.4-mini");
        assert!((pricing.input_cost_per_token - 7.5e-07).abs() < 1e-12);
        assert!((pricing.output_cost_per_token - 4.5e-06).abs() < 1e-12);
        assert!((pricing.cache_read_cost_per_token - 7.5e-08).abs() < 1e-12);
        assert_eq!(pricing.cache_write_cost_per_token, 0.0);
    }

    #[test]
    fn test_fallback_pricing_includes_gpt54_nano() {
        let map = fallback_pricing();
        let pricing = map.get("gpt-5.4-nano").expect("missing gpt-5.4-nano");
        assert!((pricing.input_cost_per_token - 2e-07).abs() < 1e-12);
        assert!((pricing.output_cost_per_token - 1.25e-06).abs() < 1e-12);
        assert!((pricing.cache_read_cost_per_token - 2e-08).abs() < 1e-12);
        assert_eq!(pricing.cache_write_cost_per_token, 0.0);
    }

    #[test]
    fn test_fallback_pricing_includes_gpt54() {
        let map = fallback_pricing();
        let pricing = map.get("gpt-5.4").expect("missing gpt-5.4");
        assert!((pricing.input_cost_per_token - 2.5e-06).abs() < 1e-12);
        assert!((pricing.output_cost_per_token - 1.5e-05).abs() < 1e-12);
        assert!((pricing.cache_read_cost_per_token - 2.5e-07).abs() < 1e-12);
        assert_eq!(pricing.cache_write_cost_per_token, 0.0);
    }

    fn assert_fallback_pricing(
        model: &str,
        input_cost_per_token: f64,
        output_cost_per_token: f64,
        cache_read_cost_per_token: f64,
        cache_write_cost_per_token: f64,
    ) {
        let map = fallback_pricing();
        let pricing = map.get(model).unwrap_or_else(|| panic!("missing {model}"));
        assert!((pricing.input_cost_per_token - input_cost_per_token).abs() < 1e-12);
        assert!((pricing.output_cost_per_token - output_cost_per_token).abs() < 1e-12);
        assert!((pricing.cache_read_cost_per_token - cache_read_cost_per_token).abs() < 1e-12);
        assert!((pricing.cache_write_cost_per_token - cache_write_cost_per_token).abs() < 1e-12);
    }

    #[test]
    fn test_fallback_pricing_includes_claude_opus_47() {
        assert_fallback_pricing("claude-opus-4-7", 5e-06, 2.5e-05, 5e-07, 1e-05);
    }

    #[test]
    fn test_fallback_pricing_includes_claude_opus_48() {
        assert_fallback_pricing("claude-opus-4-8", 5e-06, 2.5e-05, 5e-07, 1e-05);
    }

    #[test]
    fn test_fallback_pricing_includes_claude_fable_5() {
        assert_fallback_pricing("claude-fable-5", 1e-05, 5e-05, 1e-06, 2e-05);
    }

    #[test]
    fn test_fallback_pricing_includes_claude_mythos_5() {
        assert_fallback_pricing("claude-mythos-5", 1e-05, 5e-05, 1e-06, 2e-05);
    }

    #[test]
    fn test_fallback_pricing_includes_claude_sonnet_5() {
        assert_fallback_pricing("claude-sonnet-5", 2e-06, 1e-05, 2e-07, 4e-06);
    }

    #[test]
    fn test_fallback_pricing_includes_observed_claude_aliases() {
        for model in [
            "claude-opus-4.6",
            "claude-opus-4.7",
            "claude-opus-4.8",
            "anthropic/claude-opus-4.8",
            "claude-opus-4-8-thinking-high",
            "antigravity-claude-opus-4-6-thinking",
            "claude-opus-5",
        ] {
            assert_fallback_pricing(model, 5e-06, 2.5e-05, 5e-07, 1e-05);
        }
        assert_fallback_pricing("claude-fable-5-thinking-high", 1e-05, 5e-05, 1e-06, 2e-05);
        assert_fallback_pricing("claude-sonnet-4.6", 3e-06, 1.5e-05, 3e-07, 6e-06);
        assert_fallback_pricing("claude-haiku-4.5", 1e-06, 5e-06, 1e-07, 2e-06);
    }

    #[test]
    fn test_fallback_pricing_includes_observed_provider_models() {
        assert_fallback_pricing("composer-2.5", 5e-07, 2.5e-06, 2e-07, 0.0);
        assert_fallback_pricing("composer-2.5-fast", 3e-06, 1.5e-05, 5e-07, 0.0);
        assert_fallback_pricing("gemini-3.1-pro", 2e-06, 1.2e-05, 2e-07, 0.0);
        assert_fallback_pricing("gemini-3.1-pro-preview", 2e-06, 1.2e-05, 2e-07, 0.0);
        assert_fallback_pricing("grok-4.5", 2e-06, 6e-06, 3e-07, 0.0);
    }

    #[test]
    fn test_ambiguous_or_subscription_only_observed_models_remain_unpriced() {
        let pricing = fallback_pricing();
        for model in [
            "gemini-default",
            "gemma4",
            "codex-auto-review",
            "gpt-5.3-codex-spark",
            "MODEL_PLACEHOLDER_M20",
            "MODEL_PLACEHOLDER_M71",
        ] {
            assert!(!pricing.contains_key(model), "{model} must remain unpriced");
        }
    }

    #[test]
    fn test_fallback_pricing_includes_gpt55() {
        assert_fallback_pricing("gpt-5.5", 5e-06, 3e-05, 5e-07, 0.0);
    }

    #[test]
    fn test_fallback_pricing_starts_billing_cache_writes_at_gpt56() {
        assert_fallback_pricing("gpt-5.6", 5e-06, 3e-05, 5e-07, 6.25e-06);
        assert_fallback_pricing("gpt-5.6-sol", 5e-06, 3e-05, 5e-07, 6.25e-06);
        assert_fallback_pricing("gpt-5.6-terra", 2.5e-06, 1.5e-05, 2.5e-07, 3.125e-06);
        assert_fallback_pricing("gpt-5.6-luna", 1e-06, 6e-06, 1e-07, 1.25e-06);

        let pricing = fallback_pricing();
        for model in ["gpt-5.6", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            let expected = pricing[model].input_cost_per_token * 1.25;
            assert!(
                (pricing[model].cache_write_cost_per_token - expected).abs() < 1e-15,
                "{model} cache writes use the official 1.25x premium",
            );
        }
        assert_eq!(
            pricing["gpt-5.5"].cache_write_cost_per_token, 0.0,
            "pre-GPT-5.6 cache writes remain free",
        );
    }

    #[test]
    fn test_authoritative_openai_pricing_has_only_verified_exact_keys() {
        let pricing = authoritative_openai_pricing();
        let mut keys = pricing.keys().map(String::as_str).collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "gpt-5.5-fast",
                "gpt-5.6",
                "gpt-5.6-luna",
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "openai.gpt-5.6-sol",
            ]
        );
        assert_eq!(pricing["gpt-5.6"], pricing["gpt-5.6-sol"]);
        assert_eq!(pricing["gpt-5.6-sol"], pricing["openai.gpt-5.6-sol"]);
    }

    #[test]
    fn test_all_canonical_claude_fallbacks_use_one_hour_write_rate() {
        for (model, pricing) in fallback_pricing()
            .into_iter()
            .filter(|(model, _)| model.starts_with("claude-"))
        {
            assert_eq!(
                pricing.cache_write_cost_per_token,
                pricing.input_cost_per_token * 2.0,
                "{model} must default cache writes to the one-hour rate",
            );
        }
    }

    #[test]
    fn test_fallback_pricing_includes_gpt_55_fast_priority() {
        let pricing = fallback_pricing();
        let gpt_55_fast = pricing
            .get("gpt-5.5-fast")
            .expect("missing gpt-5.5-fast priority pricing");

        assert!((gpt_55_fast.input_cost_per_token - 1.25e-05).abs() < 1e-12);
        assert!((gpt_55_fast.output_cost_per_token - 7.5e-05).abs() < 1e-12);
        assert!((gpt_55_fast.cache_read_cost_per_token - 1.25e-06).abs() < 1e-12);
        assert_eq!(gpt_55_fast.cache_write_cost_per_token, 0.0);

        let standard = pricing.get("gpt-5.5").expect("missing gpt-5.5 pricing");
        assert!((standard.input_cost_per_token - 5e-06).abs() < 1e-12);
        assert!((standard.output_cost_per_token - 3e-05).abs() < 1e-12);
        assert!((standard.cache_read_cost_per_token - 5e-07).abs() < 1e-12);
    }

    #[test]
    fn test_fallback_pricing_includes_antigravity_placeholder_aliases() {
        let pricing = fallback_pricing();

        let m26 = pricing
            .get("MODEL_PLACEHOLDER_M26")
            .expect("missing M26 placeholder pricing");
        let opus = pricing
            .get("claude-opus-4-6")
            .expect("missing canonical Opus pricing");
        assert_eq!(m26.input_cost_per_token, opus.input_cost_per_token);
        assert_eq!(m26.output_cost_per_token, opus.output_cost_per_token);
        assert_eq!(
            m26.cache_read_cost_per_token,
            opus.cache_read_cost_per_token
        );
        assert_eq!(
            m26.cache_write_cost_per_token,
            opus.cache_write_cost_per_token
        );

        let m35 = pricing
            .get("MODEL_PLACEHOLDER_M35")
            .expect("missing M35 placeholder pricing");
        let sonnet = pricing
            .get("claude-sonnet-4-6")
            .expect("missing canonical Sonnet pricing");
        assert_eq!(m35.input_cost_per_token, sonnet.input_cost_per_token);
        assert_eq!(m35.output_cost_per_token, sonnet.output_cost_per_token);
        assert_eq!(
            m35.cache_read_cost_per_token,
            sonnet.cache_read_cost_per_token
        );
        assert_eq!(
            m35.cache_write_cost_per_token,
            sonnet.cache_write_cost_per_token
        );

        assert!(
            !pricing.contains_key("MODEL_PLACEHOLDER_M50"),
            "checkpoint/shadow M50 should not be priced"
        );
    }
}
