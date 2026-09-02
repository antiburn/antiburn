//! Stable pricing fixtures for integration tests.

use std::collections::HashMap;
use std::sync::Once;

use antiburn_local::pricing::ModelPricing;

static INSTALL: Once = Once::new();

pub fn install() {
    INSTALL.call_once(|| {
        fn rate(input: f64, output: f64, read: f64, write: f64) -> ModelPricing {
            ModelPricing {
                input_cost_per_token: input,
                output_cost_per_token: output,
                cache_read_cost_per_token: read,
                cache_write_cost_per_token: write,
            }
        }

        let opus = rate(5e-6, 25e-6, 0.5e-6, 10e-6);
        antiburn_local::analysis::install_runtime_pricing(HashMap::from([
            (
                "claude-3-5-haiku".to_string(),
                rate(0.8e-6, 4e-6, 0.08e-6, 1.6e-6),
            ),
            (
                "claude-haiku-4-5".to_string(),
                rate(1e-6, 5e-6, 0.1e-6, 2e-6),
            ),
            ("claude-opus-4-6".to_string(), opus.clone()),
            ("claude-opus-4-7".to_string(), opus.clone()),
            ("claude-opus-4-8".to_string(), opus),
            (
                "claude-sonnet-4-6".to_string(),
                rate(3e-6, 15e-6, 0.3e-6, 6e-6),
            ),
            (
                "claude-fable-5".to_string(),
                rate(10e-6, 50e-6, 1e-6, 20e-6),
            ),
            ("gpt-5.4".to_string(), rate(2.5e-6, 15e-6, 0.25e-6, 0.0)),
            ("gpt-5.5".to_string(), rate(5e-6, 30e-6, 0.5e-6, 0.0)),
            (
                "gpt-5.6-sol".to_string(),
                rate(5e-6, 30e-6, 0.5e-6, 6.25e-6),
            ),
        ]));
    });
}
