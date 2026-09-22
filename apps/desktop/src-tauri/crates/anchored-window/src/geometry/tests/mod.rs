use super::{
    CursorProximity, Point, Rect, classify_cursor, fit_companion_frame, place_left_preferred,
};

mod fixtures;

#[test]
fn scaled_companion_size_and_position_share_the_fitted_work_area() {
    for dpi in [1.0, 1.25, 2.0] {
        for zoom in [0.9, 1.0, 1.25, 2.0] {
            let work = Rect {
                x: -1280.0 * dpi,
                y: 32.0 * dpi,
                width: 1280.0 * dpi,
                height: 800.0 * dpi,
            };
            let anchor = Rect {
                x: -400.0 * dpi,
                y: 650.0 * dpi,
                width: 380.0 * dpi,
                height: 60.0 * dpi,
            };
            let margin = 8.0 * zoom * dpi;
            let frame = fit_companion_frame(
                anchor,
                work,
                (420.0 * zoom * dpi, 700.0 * zoom * dpi),
                8.0 * zoom * dpi,
                margin,
            );
            assert!(frame.x >= work.x + margin);
            assert!(frame.y >= work.y + margin);
            assert!(frame.x + frame.width <= work.x + work.width - margin);
            assert!(frame.y + frame.height <= work.y + work.height - margin);
            assert_eq!(frame.width.fract(), 0.0);
            assert_eq!(frame.height.fract(), 0.0);
        }
    }
}

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

#[test]
fn cursor_classification_distinguishes_exact_bounds_tolerance_and_outside() {
    let bounds = Rect {
        x: 100.0,
        y: 200.0,
        width: 300.0,
        height: 150.0,
    };

    for point in [
        Point { x: 100.0, y: 200.0 },
        Point {
            x: 399.999,
            y: 349.999,
        },
    ] {
        assert_eq!(
            classify_cursor(bounds, point, 12.0, 1.0),
            CursorProximity::Inside,
            "{point:?} should be inside the exact bounds",
        );
    }

    for point in [
        Point { x: 88.0, y: 275.0 },
        Point { x: 412.0, y: 275.0 },
        Point { x: 250.0, y: 188.0 },
        Point { x: 250.0, y: 362.0 },
        Point { x: 88.0, y: 188.0 },
        Point { x: 412.0, y: 362.0 },
    ] {
        assert_eq!(
            classify_cursor(bounds, point, 12.0, 1.0),
            CursorProximity::WithinTolerance,
            "{point:?} should be inside the tolerance bounds",
        );
    }

    for point in [
        Point {
            x: 87.999,
            y: 275.0,
        },
        Point {
            x: 412.001,
            y: 275.0,
        },
        Point {
            x: 250.0,
            y: 187.999,
        },
        Point {
            x: 250.0,
            y: 362.001,
        },
        Point {
            x: 87.999,
            y: 187.999,
        },
    ] {
        assert_eq!(
            classify_cursor(bounds, point, 12.0, 1.0),
            CursorProximity::Outside,
            "{point:?} should be outside the tolerance bounds",
        );
    }
}

#[test]
fn cursor_classification_scales_logical_tolerance_to_physical_coordinates() {
    let bounds = Rect {
        x: 100.0,
        y: 200.0,
        width: 300.0,
        height: 150.0,
    };

    assert_eq!(
        classify_cursor(bounds, Point { x: 424.0, y: 275.0 }, 12.0, 2.0),
        CursorProximity::WithinTolerance,
    );
    assert_eq!(
        classify_cursor(
            bounds,
            Point {
                x: 424.001,
                y: 275.0
            },
            12.0,
            2.0
        ),
        CursorProximity::Outside,
    );
}

#[test]
fn cursor_classification_sanitizes_invalid_tolerance_and_scale() {
    let bounds = Rect {
        x: 100.0,
        y: 200.0,
        width: 300.0,
        height: 150.0,
    };
    let beyond_right_edge = Point { x: 400.0, y: 275.0 };

    for tolerance in [-12.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            classify_cursor(bounds, beyond_right_edge, tolerance, 1.0),
            CursorProximity::Outside,
            "{tolerance:?} should produce zero tolerance",
        );
    }

    for scale in [-1.0, 0.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            classify_cursor(bounds, Point { x: 412.0, y: 275.0 }, 12.0, scale),
            CursorProximity::WithinTolerance,
            "{scale:?} should use a scale factor of one",
        );
        assert_eq!(
            classify_cursor(
                bounds,
                Point {
                    x: 412.001,
                    y: 275.0
                },
                12.0,
                scale
            ),
            CursorProximity::Outside,
            "{scale:?} should use a scale factor of one",
        );
    }
}
