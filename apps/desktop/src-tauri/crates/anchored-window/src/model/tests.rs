use super::{HeightPolicy, normalized_height_policy};

#[test]
fn content_height_configuration_is_safe_and_clamped() {
    let normalized = normalized_height_policy(HeightPolicy::Content {
        initial: f64::NAN,
        min: 500.0,
        max: 100.0,
    });

    assert_eq!(
        normalized,
        HeightPolicy::Content {
            initial: 500.0,
            min: 500.0,
            max: 500.0,
        }
    );
}
