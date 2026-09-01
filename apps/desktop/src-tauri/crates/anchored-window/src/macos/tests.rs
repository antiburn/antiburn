use crate::model::AnchorRegion;

use super::{FrameRequest, Rect, frame_contains, horizontal_origin, vertical_origin};

#[test]
fn vertical_item_alignment_clamps_to_both_screen_margins() {
    assert_eq!(vertical_origin(800.0, 0.0, 200.0, 0.0, 800.0, 8.0), 592.0);
    assert_eq!(vertical_origin(400.0, 100.0, 200.0, 0.0, 800.0, 8.0), 100.0);
    assert_eq!(vertical_origin(180.0, 100.0, 200.0, 0.0, 800.0, 8.0), 8.0);
}

#[test]
fn native_frame_hit_test_uses_half_open_edges() {
    let frame = Rect {
        x: 100.0,
        y: 200.0,
        width: 80.0,
        height: 60.0,
    };

    assert!(frame_contains(frame, 100.0, 200.0));
    assert!(frame_contains(frame, 179.9, 259.9));
    assert!(!frame_contains(frame, 180.0, 230.0));
    assert!(!frame_contains(frame, 140.0, 260.0));
}

#[test]
fn horizontal_alignment_uses_anchor_and_work_area_in_order() {
    let anchor = Rect {
        x: 500.0,
        y: 0.0,
        width: 100.0,
        height: 300.0,
    };
    let work_area = Rect {
        x: 0.0,
        y: 0.0,
        width: 1200.0,
        height: 800.0,
    };
    let request = FrameRequest {
        width: 300.0,
        height: Some(200.0),
        anchor_region: AnchorRegion {
            top: 0.0,
            height: 48.0,
        },
        gap: 8.0,
        screen_margin: 8.0,
    };

    assert_eq!(horizontal_origin(anchor, work_area, 200.0, &request), 192.0);
}
