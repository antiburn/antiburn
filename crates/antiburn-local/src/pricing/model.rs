//! Canonical pricing value types.
//!
//! The pure data types the engine prices with: per-token model rates, a
//! per-model token breakdown, and the resulting cost. They carry no I/O and
//! no provider-specific dependencies.

use serde::{Deserialize, Serialize};

/// Model pricing rates (cost per token).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPricing {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
    pub cache_read_cost_per_token: f64,
    pub cache_write_cost_per_token: f64,
}

/// One model's token counts within a session or step.
///
/// Counts are non-negative (`u64`); callers whose source type is signed convert
/// at the boundary, saturating any negative value to zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelTokens {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    /// Cache-creation tokens explicitly classified as one-hour writes.
    ///
    /// This is a subset of `cache_creation_tokens`; the remainder is priced
    /// with the model's default cache-write rate.
    #[serde(default)]
    pub cache_creation_1h_tokens: u64,
}

/// Estimated cost in USD, broken down by token category.
///
/// `total_usd` is the sum of the component fields; the breakdown is preserved
/// so UIs can render where the cost came from without recomputing it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub total_usd: f64,
    pub input_usd: f64,
    pub output_usd: f64,
    pub cache_read_usd: f64,
    pub cache_write_usd: f64,
}
