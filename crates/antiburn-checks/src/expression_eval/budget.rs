use serde::{Deserialize, Deserializer, Serialize};

use super::{
    DEFAULT_MAX_EVALUATION_DEPTH, DEFAULT_MAX_EVALUATION_STEPS, EvaluationError, EvaluationLimits,
    limit_error,
};

/// Maximum visited iteration items across one assessment.
pub const DEFAULT_MAX_EVALUATION_ITEMS: usize = 1_000;
/// Maximum generated JSON and text bytes across one assessment.
pub const DEFAULT_MAX_GENERATED_BYTES: usize = 8 * 1024 * 1024;
/// Maximum cumulative JSON copy bytes across one assessment.
pub const DEFAULT_MAX_MATERIALIZED_BYTES: usize = 8 * 1024 * 1024;

/// Shared work counters for input preparation and action planning.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EvaluationBudget {
    limits: EvaluationLimits,
    max_items: usize,
    max_generated_bytes: usize,
    max_materialized_bytes: usize,
    steps: usize,
    visited_items: usize,
    generated_bytes: usize,
    materialized_bytes: usize,
    exhausted: bool,
}

impl Default for EvaluationBudget {
    fn default() -> Self {
        Self::new(
            EvaluationLimits::default(),
            DEFAULT_MAX_EVALUATION_ITEMS,
            DEFAULT_MAX_GENERATED_BYTES,
        )
    }
}

impl EvaluationBudget {
    /// Creates an empty budget with explicit bounds.
    #[must_use]
    pub const fn new(
        limits: EvaluationLimits,
        max_items: usize,
        max_generated_bytes: usize,
    ) -> Self {
        Self::new_with_materialization_limit(
            limits,
            max_items,
            max_generated_bytes,
            DEFAULT_MAX_MATERIALIZED_BYTES,
        )
    }

    /// Creates an empty budget with an explicit cumulative JSON copy bound.
    #[must_use]
    pub const fn new_with_materialization_limit(
        limits: EvaluationLimits,
        max_items: usize,
        max_generated_bytes: usize,
        max_materialized_bytes: usize,
    ) -> Self {
        Self {
            limits,
            max_items,
            max_generated_bytes,
            max_materialized_bytes,
            steps: 0,
            visited_items: 0,
            generated_bytes: 0,
            materialized_bytes: 0,
            exhausted: false,
        }
    }

    /// Charges a node before work starts. Depth includes enclosing templates.
    pub fn step(&mut self, depth: usize) -> Result<(), EvaluationError> {
        self.check_active()?;
        if depth > self.limits.max_depth {
            return self.fail("Expression evaluation exceeded the depth limit.");
        }
        if self.steps >= self.limits.max_steps {
            return self.fail("Expression evaluation exceeded the step limit.");
        }
        self.steps += 1;
        Ok(())
    }

    /// Charges iteration items before a loop allocates its output.
    pub fn visit_items(&mut self, count: usize) -> Result<(), EvaluationError> {
        self.check_active()?;
        if count > self.max_items.saturating_sub(self.visited_items) {
            return self.fail("Expression evaluation exceeded the visited-item limit.");
        }
        self.visited_items += count;
        Ok(())
    }

    /// Reserves generated payload bytes before allocation or append.
    pub fn reserve_generated_bytes(&mut self, count: usize) -> Result<(), EvaluationError> {
        self.check_active()?;
        if count
            > self
                .max_generated_bytes
                .saturating_sub(self.generated_bytes)
        {
            return self.fail("Expression evaluation exceeded the generated-byte limit.");
        }
        self.generated_bytes += count;
        Ok(())
    }

    /// Reserves cumulative JSON copy bytes before cloning a value.
    pub fn reserve_materialized_bytes(&mut self, count: usize) -> Result<(), EvaluationError> {
        self.check_active()?;
        if count
            > self
                .max_materialized_bytes
                .saturating_sub(self.materialized_bytes)
        {
            return self.fail("Expression evaluation exceeded the materialized-byte limit.");
        }
        self.materialized_bytes += count;
        Ok(())
    }

    /// Returns whether any shared limit has failed.
    #[must_use]
    pub const fn exhausted(&self) -> bool {
        self.exhausted
    }

    /// Checks saved counters against the production limits before restart.
    pub fn validate_for_resume(&self) -> Result<(), EvaluationError> {
        self.check_active()?;
        if self.limits != EvaluationLimits::default()
            || self.max_items != DEFAULT_MAX_EVALUATION_ITEMS
            || self.max_generated_bytes != DEFAULT_MAX_GENERATED_BYTES
            || self.max_materialized_bytes != DEFAULT_MAX_MATERIALIZED_BYTES
        {
            return Err(limit_error("Saved evaluation limits are incompatible."));
        }
        Ok(())
    }

    /// Returns consumed node and path steps.
    #[must_use]
    pub const fn steps(&self) -> usize {
        self.steps
    }

    /// Returns consumed iteration items.
    #[must_use]
    pub const fn visited_items(&self) -> usize {
        self.visited_items
    }

    /// Returns reserved generated payload bytes.
    #[must_use]
    pub const fn generated_bytes(&self) -> usize {
        self.generated_bytes
    }

    /// Returns reserved cumulative JSON copy bytes.
    #[must_use]
    pub const fn materialized_bytes(&self) -> usize {
        self.materialized_bytes
    }

    fn check_active(&self) -> Result<(), EvaluationError> {
        if self.exhausted {
            Err(limit_error("The shared evaluation budget is exhausted."))
        } else {
            Ok(())
        }
    }

    fn fail(&mut self, message: &str) -> Result<(), EvaluationError> {
        self.exhausted = true;
        Err(limit_error(message))
    }
}

impl<'de> Deserialize<'de> for EvaluationBudget {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct SavedBudget {
            limits: EvaluationLimits,
            max_items: usize,
            max_generated_bytes: usize,
            max_materialized_bytes: usize,
            steps: usize,
            visited_items: usize,
            generated_bytes: usize,
            materialized_bytes: usize,
            exhausted: bool,
        }
        let saved = SavedBudget::deserialize(deserializer)?;
        if saved.limits.max_depth > DEFAULT_MAX_EVALUATION_DEPTH
            || saved.limits.max_steps > DEFAULT_MAX_EVALUATION_STEPS
            || saved.max_items > DEFAULT_MAX_EVALUATION_ITEMS
            || saved.max_generated_bytes > DEFAULT_MAX_GENERATED_BYTES
            || saved.max_materialized_bytes > DEFAULT_MAX_MATERIALIZED_BYTES
            || saved.steps > saved.limits.max_steps
            || saved.visited_items > saved.max_items
            || saved.generated_bytes > saved.max_generated_bytes
            || saved.materialized_bytes > saved.max_materialized_bytes
        {
            return Err(serde::de::Error::custom(
                "Invalid saved evaluation counters.",
            ));
        }
        Ok(Self {
            limits: saved.limits,
            max_items: saved.max_items,
            max_generated_bytes: saved.max_generated_bytes,
            max_materialized_bytes: saved.max_materialized_bytes,
            steps: saved.steps,
            visited_items: saved.visited_items,
            generated_bytes: saved.generated_bytes,
            materialized_bytes: saved.materialized_bytes,
            exhausted: saved.exhausted,
        })
    }
}
