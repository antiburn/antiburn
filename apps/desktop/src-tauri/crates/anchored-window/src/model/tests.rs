use super::normalized_heights;

#[test]
fn content_height_configuration_is_safe_and_clamped() {
    assert_eq!(
        normalized_heights(f64::NAN, 500.0, 100.0),
        (500.0, 500.0, 500.0)
    );
}
