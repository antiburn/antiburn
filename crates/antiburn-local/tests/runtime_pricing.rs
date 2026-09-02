use std::collections::HashMap;

use antiburn_local::analysis::{install_runtime_pricing, price_breakdown, pricing_generation};
use antiburn_local::pricing::{ModelPricing, ModelTokens};

#[test]
fn runtime_snapshot_is_the_only_pricing_source() {
    let breakdown = HashMap::from([(
        "sample-model".to_string(),
        ModelTokens {
            input_tokens: 1,
            ..Default::default()
        },
    )]);
    assert!(price_breakdown(&breakdown).is_none());
    let before = pricing_generation();
    let snapshot = HashMap::from([(
        "SAMPLE-MODEL-20260901".to_string(),
        ModelPricing {
            input_cost_per_token: 0.1,
            output_cost_per_token: 0.2,
            cache_read_cost_per_token: 0.03,
            cache_write_cost_per_token: 0.04,
        },
    )]);

    assert!(install_runtime_pricing(snapshot.clone()));
    assert_eq!(pricing_generation(), before + 1);
    assert!(price_breakdown(&breakdown).is_some());

    assert!(!install_runtime_pricing(snapshot));
    assert_eq!(pricing_generation(), before + 1);

    assert!(install_runtime_pricing(HashMap::new()));
    assert!(price_breakdown(&breakdown).is_none());
}
