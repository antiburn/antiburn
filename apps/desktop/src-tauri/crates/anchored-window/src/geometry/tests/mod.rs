mod fixtures;

use super::place_left_preferred;

#[test]
fn placement_covers_fit_clamp_negative_origin_and_scale() {
    for case in fixtures::cases() {
        assert_eq!(
            place_left_preferred(
                case.anchor,
                case.work,
                case.width,
                case.height,
                case.scale,
                8.0,
                8.0,
            ),
            case.expected,
            "{}",
            case.name,
        );
    }
}
