// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Strict normalization into the consumed-capacity percentage domain.
//!
//! Ported from the private app under D-023 (allowlist rule
//! `usage-limit-domain`). Every function here fails closed: a value outside
//! its domain returns an error rather than a clamp. That choice is the point
//! of the module. Clamping 140% to 100% produces a full meter that looks
//! exactly like a real one, and the reader has no way to tell that the
//! provider said something we did not understand — whereas a window that is
//! simply absent is self-describing.

use super::model::{ProviderUsageError, SchemaReason};

/// Validate an already-consumed percentage in the inclusive `0..=100` domain.
///
/// A missing value stays missing. A non-finite or out-of-range one fails.
pub fn used_percent(value: Option<f64>) -> Result<Option<f64>, ProviderUsageError> {
    validate_percent(value)
}

/// Convert a value that might be a `0..=1` fraction or an already-consumed
/// percentage into the `0..=100` domain this module stores.
///
/// A provider's own usage endpoint is not always consistent about which shape
/// it emits — sometimes across the very same payload family, depending on
/// which field carried the figure. A value at or below `1.0` is read as a
/// fraction and scaled up; anything above is already a percent. The one input
/// this cannot disambiguate — a genuine one-percent reading spelled as the
/// bare integer `1` — reads as 100% instead of 1%, but every payload this
/// module has seen states single-digit-and-up percentages as two digits or a
/// larger fraction, so the cost is theoretical, and it is the cheaper mistake
/// to risk: the alternative is guessing wrong on the shape that is actually
/// common.
pub fn used_percent_or_fraction(value: Option<f64>) -> Result<Option<f64>, ProviderUsageError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !value.is_finite() {
        return Err(ProviderUsageError::Schema(SchemaReason::InvalidValue));
    }
    let percent = if (0.0..=1.0).contains(&value) {
        value * 100.0
    } else {
        value
    };
    used_percent(Some(percent))
}

fn validate_percent(value: Option<f64>) -> Result<Option<f64>, ProviderUsageError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return Err(ProviderUsageError::Schema(SchemaReason::InvalidValue));
    }
    Ok(Some(value))
}

/// Turn a provider's free-text display name into a lowercase, hyphenated
/// identifier fragment: alphanumeric runs are kept and lowercased, every run
/// of anything else collapses into one hyphen, and the ends are trimmed.
///
/// Shared by every parser that builds a model-scoped window id out of a
/// display name the provider could re-punctuate at any time (a space instead
/// of a dash, different capitalization) — the id has to stay stable across
/// those changes because it is what the sample history and the milestone
/// ledger join on, and the display name itself is not.
pub fn slugify(text: &str) -> String {
    let mut slug = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVALID: ProviderUsageError = ProviderUsageError::Schema(SchemaReason::InvalidValue);

    #[test]
    fn an_out_of_range_percentage_fails_rather_than_clamping() {
        assert_eq!(used_percent(Some(140.0)), Err(INVALID));
        assert_eq!(used_percent(Some(-1.0)), Err(INVALID));
        assert_eq!(used_percent(Some(f64::NAN)), Err(INVALID));
        assert_eq!(used_percent(Some(f64::INFINITY)), Err(INVALID));
    }

    #[test]
    fn the_domain_boundaries_themselves_are_valid() {
        assert_eq!(used_percent(Some(0.0)), Ok(Some(0.0)));
        assert_eq!(used_percent(Some(100.0)), Ok(Some(100.0)));
        assert_eq!(used_percent(None), Ok(None));
    }

    #[test]
    fn a_fraction_at_or_below_one_is_scaled_up_into_a_percent() {
        assert_eq!(used_percent_or_fraction(Some(0.81)), Ok(Some(81.0)));
        assert_eq!(used_percent_or_fraction(Some(0.0)), Ok(Some(0.0)));
        assert_eq!(used_percent_or_fraction(Some(1.0)), Ok(Some(100.0)));
    }

    #[test]
    fn a_value_above_one_is_read_as_an_already_stated_percent() {
        assert_eq!(used_percent_or_fraction(Some(81.0)), Ok(Some(81.0)));
        assert_eq!(used_percent_or_fraction(Some(100.0)), Ok(Some(100.0)));
    }

    #[test]
    fn a_fraction_or_percent_out_of_domain_still_fails_closed() {
        assert_eq!(used_percent_or_fraction(Some(140.0)), Err(INVALID));
        assert_eq!(used_percent_or_fraction(Some(-0.5)), Err(INVALID));
        assert_eq!(used_percent_or_fraction(Some(f64::NAN)), Err(INVALID));
        assert_eq!(used_percent_or_fraction(None), Ok(None));
    }

    #[test]
    fn slugify_lowercases_and_hyphenates_punctuation_runs() {
        assert_eq!(slugify("Claude Opus 4.5"), "claude-opus-4-5");
        assert_eq!(slugify("GPT-5.3-Codex-Spark"), "gpt-5-3-codex-spark");
    }

    #[test]
    fn slugify_trims_leading_and_trailing_hyphens() {
        assert_eq!(slugify("  Fable  "), "fable");
        assert_eq!(slugify("--already--hyphenated--"), "already-hyphenated");
    }
}
