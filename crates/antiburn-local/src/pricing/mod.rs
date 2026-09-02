//! Local, dependency-light pricing primitives.
//!
//! This module owns model-key normalization and cost calculation. The engine
//! performs no network access and bundles no pricing catalog.

pub mod calc;
pub mod model;
pub mod table;

pub use calc::{CostResult, calculate_cost};
pub use model::{Cost, ModelPricing, ModelTokens};
pub use table::{canonical_model_key, lookup_pricing, normalize_model_key};
