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

/// Convert a remaining percentage into the consumed percentage this module
/// stores.
///
/// Presentation may invert it again; storage and arithmetic never do.
#[allow(dead_code)] // Awaits the first source that reports remaining rather than used.
pub fn remaining_percent(value: Option<f64>) -> Result<Option<f64>, ProviderUsageError> {
    Ok(validate_percent(value)?.map(|remaining| 100.0 - remaining))
}

/// Convert a remaining fraction in `0..=1` into consumed percent in `0..=100`.
#[allow(dead_code)] // Awaits the first source that reports a fraction.
pub fn remaining_fraction(value: Option<f64>) -> Result<Option<f64>, ProviderUsageError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(ProviderUsageError::Schema(SchemaReason::InvalidValue));
    }
    Ok(Some((1.0 - value) * 100.0))
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

/// Derive a consumed percentage, but only when both a non-negative usage and
/// a positive limit are known.
///
/// A partial input stays unknown rather than becoming a guess; an invalid one
/// fails.
#[allow(dead_code)] // Awaits the first source that discloses raw amounts.
pub fn percent_from_amounts(
    used: Option<f64>,
    limit: Option<f64>,
) -> Result<Option<f64>, ProviderUsageError> {
    match (used, limit) {
        (Some(used), Some(limit))
            if used.is_finite() && limit.is_finite() && used >= 0.0 && limit > 0.0 =>
        {
            validate_percent(Some(used / limit * 100.0))
        }
        (None, _) | (_, None) => Ok(None),
        _ => Err(ProviderUsageError::Schema(SchemaReason::InvalidValue)),
    }
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
    fn remaining_inverts_into_consumed() {
        assert_eq!(remaining_percent(Some(25.0)), Ok(Some(75.0)));
        assert_eq!(remaining_fraction(Some(0.25)), Ok(Some(75.0)));
        assert_eq!(remaining_fraction(Some(1.5)), Err(INVALID));
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
    fn amounts_need_both_halves_before_they_become_a_percentage() {
        assert_eq!(percent_from_amounts(Some(5.0), Some(20.0)), Ok(Some(25.0)));
        // Half the pair is not a quarter of an answer.
        assert_eq!(percent_from_amounts(Some(5.0), None), Ok(None));
        assert_eq!(percent_from_amounts(None, Some(20.0)), Ok(None));
        // A zero limit is not an infinite percentage.
        assert_eq!(percent_from_amounts(Some(5.0), Some(0.0)), Err(INVALID));
        // And an over-limit amount is a payload we did not understand.
        assert_eq!(percent_from_amounts(Some(30.0), Some(20.0)), Err(INVALID));
    }
}
