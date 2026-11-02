use std::{collections::HashMap, io::ErrorKind, path::Path};

use antiburn_local::pricing::{ModelPricing, ModelTokens, calculate_cost};
use serde::Deserialize;

use crate::config::PriceOutlierConfig;

const DEFAULT_PRICING_SNAPSHOT_FILENAME: &str = "model-pricing.json";
const MAX_PRICING_SNAPSHOT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_MODELS: usize = 50_000;
const MAX_MODEL_ID_BYTES: usize = 512;
const SNAPSHOT_SCHEMA: u32 = 2;
const LEGACY_SNAPSHOT_SCHEMA: u32 = 1;

#[derive(Debug, Deserialize)]
struct PricingSnapshot {
    schema: u32,
    source: String,
    version: String,
    models: HashMap<String, ModelPricing>,
}

pub(super) fn load_pricing(
    database_path: &Path,
    override_path: Option<&Path>,
) -> Result<Option<HashMap<String, ModelPricing>>, String> {
    let path = override_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| database_path.with_file_name(DEFAULT_PRICING_SNAPSHOT_FILENAME));
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not inspect model pricing snapshot: {error}")),
    };
    if metadata.len() > MAX_PRICING_SNAPSHOT_BYTES {
        return Err("model pricing snapshot exceeds the size limit".to_owned());
    }
    let contents = std::fs::read(&path)
        .map_err(|error| format!("could not read model pricing snapshot: {error}"))?;
    let snapshot: PricingSnapshot = serde_json::from_slice(&contents)
        .map_err(|error| format!("invalid model pricing snapshot: {error}"))?;
    validate_snapshot(&snapshot)?;
    Ok(Some(snapshot.models))
}

pub(super) fn session_cost(
    model_breakdown_json: Option<&str>,
    pricing_breakdown_json: Option<&str>,
    pricing: &HashMap<String, ModelPricing>,
) -> Option<f64> {
    let _: HashMap<String, ModelTokens> = serde_json::from_str(model_breakdown_json?).ok()?;
    let pricing_breakdown: HashMap<String, ModelTokens> =
        serde_json::from_str(pricing_breakdown_json?).ok()?;
    if pricing_breakdown.is_empty() {
        return None;
    }
    let result = calculate_cost(&pricing_breakdown, pricing);
    if !result.unpriced_models.is_empty() {
        return None;
    }
    let total = result.cost.total_usd;
    (total.is_finite() && total >= 0.0).then_some(total)
}

pub(super) fn outlier_threshold(costs: &[f64], policy: PriceOutlierConfig) -> Option<f64> {
    if costs.len() < policy.minimum_priced_sessions {
        return None;
    }
    let threshold = policy.floor_usd.max(policy.median_multiple * median(costs));
    threshold.is_finite().then_some(threshold)
}

fn median(costs: &[f64]) -> f64 {
    let mut sorted = costs.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    }
}

fn validate_snapshot(snapshot: &PricingSnapshot) -> Result<(), String> {
    if !matches!(snapshot.schema, SNAPSHOT_SCHEMA | LEGACY_SNAPSHOT_SCHEMA)
        || snapshot.source != "models.dev"
    {
        return Err("unsupported model pricing snapshot format".to_owned());
    }
    if !valid_catalog_version(&snapshot.version)
        || snapshot.models.is_empty()
        || snapshot.models.len() > MAX_MODELS
    {
        return Err("model pricing snapshot has no usable catalog".to_owned());
    }
    let valid = snapshot.models.iter().all(|(model, pricing)| {
        model.len() <= MAX_MODEL_ID_BYTES
            && [
                pricing.input_cost_per_token,
                pricing.output_cost_per_token,
                pricing.cache_read_cost_per_token,
                pricing.cache_write_cost_per_token,
            ]
            .iter()
            .all(|rate| rate.is_finite() && *rate >= 0.0)
    });
    if !valid {
        return Err("model pricing snapshot contains an invalid price".to_owned());
    }
    Ok(())
}

fn valid_catalog_version(version: &str) -> bool {
    let bytes = version.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs};

    use antiburn_local::pricing::ModelPricing;
    use serde_json::json;
    use tempfile::tempdir;

    use super::{load_pricing, outlier_threshold, session_cost};
    use crate::config::Config;

    fn snapshot(prices: serde_json::Value) -> String {
        json!({
            "schema": 2,
            "source": "models.dev",
            "version": "2026-10-16",
            "fetched_at": "2026-10-16T00:00:00Z",
            "etag": null,
            "models": prices
        })
        .to_string()
    }

    fn rates(input: f64) -> serde_json::Value {
        json!({
            "input_cost_per_token": input,
            "output_cost_per_token": 0.0,
            "cache_read_cost_per_token": 0.0,
            "cache_write_cost_per_token": 0.0
        })
    }

    fn breakdown(input_tokens: u64) -> String {
        json!({
            "test-model": {
                "input_tokens": input_tokens,
                "output_tokens": 0,
                "cache_read_tokens": 0,
                "cache_creation_tokens": 0,
                "cache_creation_1h_tokens": 0
            }
        })
        .to_string()
    }

    #[test]
    fn loads_valid_current_and_legacy_snapshot_schemas() {
        let temp = tempdir().unwrap();
        let database = temp.path().join("antiburn.sqlite3");
        let path = temp.path().join("model-pricing.json");
        fs::write(
            &path,
            snapshot(json!({"test-model": rates(0.000001)}))
                .replace("\"schema\":2", "\"schema\":1"),
        )
        .unwrap();
        assert!(load_pricing(&database, None).unwrap().is_some());
        fs::write(&path, snapshot(json!({"test-model": rates(0.000001)}))).unwrap();
        assert!(load_pricing(&database, None).unwrap().is_some());
    }

    #[test]
    fn rejects_unsafe_or_unusable_snapshots_and_treats_missing_as_unavailable() {
        let temp = tempdir().unwrap();
        let database = temp.path().join("antiburn.sqlite3");
        let path = temp.path().join("model-pricing.json");
        assert!(load_pricing(&database, None).unwrap().is_none());
        for content in [
            snapshot(json!({})),
            snapshot(json!({"test-model": rates(-1.0)})),
            r#"{"schema":2,"source":"models.dev","version":"2026-10-16","models":{"test-model":{"input_cost_per_token":1e999,"output_cost_per_token":0,"cache_read_cost_per_token":0,"cache_write_cost_per_token":0}}}"#.to_owned(),
            snapshot(json!({"test-model": {"input_cost_per_token": "bad"}})),
            snapshot(json!({"test-model": rates(0.000001)})).replace("2026-10-16", "bad-date"),
        ] {
            fs::write(&path, content).unwrap();
            assert!(load_pricing(&database, None).is_err());
        }
    }

    #[test]
    fn calculates_only_fully_priced_cached_breakdowns() {
        let prices = HashMap::from([(
            "test-model".to_owned(),
            ModelPricing {
                input_cost_per_token: 0.000001,
                output_cost_per_token: 0.0,
                cache_read_cost_per_token: 0.0,
                cache_write_cost_per_token: 0.0,
            },
        )]);
        assert_eq!(
            session_cost(Some("{}"), Some(&breakdown(1_000_000)), &prices),
            Some(1.0)
        );
        assert!(session_cost(Some("not-json"), Some(&breakdown(1_000_000)), &prices).is_none());
        assert!(session_cost(Some("{}"), Some("not-json"), &prices).is_none());
        assert!(session_cost(Some("{}"), Some(&breakdown(1_000_000)), &HashMap::new()).is_none());
        assert!(session_cost(Some("{}"), Some("{}"), &prices).is_none());
    }

    #[test]
    fn threshold_uses_the_floor_and_median_multiple() {
        let config = Config::defaults();
        assert_eq!(
            outlier_threshold(&[0.1; 8], config.price_outlier),
            Some(2.0)
        );
        assert_eq!(
            outlier_threshold(&[1.0; 8], config.price_outlier),
            Some(3.0)
        );
        assert_eq!(outlier_threshold(&[1.0; 7], config.price_outlier), None);
    }
}
