//! Model-key normalization and pricing lookup.
//!
//! The engine does not bundle a pricing catalog. Its caller installs a current
//! snapshot at runtime, and unknown models remain explicitly unpriced.

use std::collections::HashMap;

use crate::pricing::model::ModelPricing;

/// Strip a trailing `-YYYYMMDD` date suffix from a model ID.
pub fn normalize_model_key(model_id: &str) -> &str {
    let bytes = model_id.as_bytes();
    if bytes.len() > 9 {
        let (head, tail) = bytes.split_at(bytes.len() - 8);
        if tail.iter().all(u8::is_ascii_digit) && head[head.len() - 1] == b'-' {
            return &model_id[..model_id.len() - 9];
        }
    }
    model_id
}

/// Strip a provider namespace, normalize the date suffix, and use lowercase.
pub fn canonical_model_key(model: &str) -> String {
    let trimmed = model.trim();
    let without_namespace = trimmed
        .split_once('/')
        .map(|(_, rest)| rest)
        .or_else(|| trimmed.strip_prefix("openai."))
        .or_else(|| trimmed.strip_prefix("antigravity-"))
        .unwrap_or(trimmed);
    normalize_model_key(without_namespace).to_lowercase()
}

/// Look up pricing for a model ID, trying exact and normalized forms.
pub fn lookup_pricing<'a>(
    model_id: &str,
    pricing_map: &'a HashMap<String, ModelPricing>,
) -> Option<&'a ModelPricing> {
    if let Some(pricing) = pricing_map.get(model_id) {
        return Some(pricing);
    }

    let normalized = normalize_model_key(model_id);
    if normalized != model_id
        && let Some(pricing) = pricing_map.get(normalized)
    {
        return Some(pricing);
    }

    let lower = model_id.to_lowercase();
    if lower != model_id {
        if let Some(pricing) = pricing_map.get(&lower) {
            return Some(pricing);
        }
        let lower_normalized = normalize_model_key(&lower);
        if lower_normalized != lower.as_str()
            && let Some(pricing) = pricing_map.get(lower_normalized)
        {
            return Some(pricing);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate() -> ModelPricing {
        ModelPricing {
            input_cost_per_token: 0.1,
            output_cost_per_token: 0.2,
            cache_read_cost_per_token: 0.03,
            cache_write_cost_per_token: 0.04,
        }
    }

    #[test]
    fn normalize_strips_only_a_date_suffix() {
        assert_eq!(normalize_model_key("sample-model-20260301"), "sample-model");
        assert_eq!(normalize_model_key("sample-model"), "sample-model");
        assert_eq!(normalize_model_key("modèle-4-20260301"), "modèle-4");
        assert_eq!(normalize_model_key("€€€€"), "€€€€");
    }

    #[test]
    fn canonical_key_removes_known_namespace_forms() {
        assert_eq!(canonical_model_key("vendor/SAMPLE-20260301"), "sample");
        assert_eq!(canonical_model_key("openai.SAMPLE"), "sample");
        assert_eq!(canonical_model_key("antigravity-SAMPLE"), "sample");
    }

    #[test]
    fn lookup_accepts_case_and_date_variants_without_guessing_aliases() {
        let map = HashMap::from([("sample-model".to_string(), rate())]);
        assert!(lookup_pricing("SAMPLE-MODEL-20260301", &map).is_some());
        assert!(lookup_pricing("different-model", &map).is_none());
    }
}
