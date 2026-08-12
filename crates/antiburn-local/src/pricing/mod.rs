// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local, dependency-light pricing primitives.
//!
//! This module owns the bundled LLM pricing catalog, model-key
//! normalization, and the per-model cost calculation. It is intentionally
//! **std-only** — no database, async, network, or platform-specific
//! dependencies — and performs no runtime price sourcing: the bundled
//! catalog is the only price source, and models it cannot price are
//! reported as explicitly unpriced rather than folded into partial totals.
//!
//! # Modules
//!
//! - [`model`] — canonical value types ([`model::ModelPricing`],
//!   [`model::ModelTokens`], [`model::Cost`]).
//! - [`table`] — the bundled pricing catalog plus key normalization and lookup.
//! - [`calc`] — the per-model cost calculation ([`calc::calculate_cost`]).

pub mod calc;
pub mod model;
pub mod table;

/// Version of the bundled pricing catalog, as a `YYYY-MM-DD` review date.
///
/// Prices in [`table::fallback_pricing`] are transcribed from official
/// provider pricing pages; bump this date whenever the catalog changes.
pub const PRICING_CATALOG_VERSION: &str = "2026-08-12";

// Flat re-exports so consumers can write `pricing::calculate_cost`,
// `pricing::ModelTokens`, `pricing::fallback_pricing()`, etc.
pub use calc::{CostResult, calculate_cost};
pub use model::{Cost, ModelPricing, ModelTokens};
pub use table::{
    ANTIGRAVITY_PLACEHOLDER_M26_MODEL_KEY, ANTIGRAVITY_PLACEHOLDER_M35_MODEL_KEY,
    GPT_55_FAST_MODEL_KEY, authoritative_openai_pricing, classify_model, fallback_pricing,
    gpt_55_fast_priority_pricing, lookup_pricing, normalize_model_key,
};
